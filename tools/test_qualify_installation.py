"""Synthetic trust-boundary tests; these never claim browser/Windows qualification."""
from copy import deepcopy
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import urllib.error
import zipfile

import qualify_installation as probe
import release_stage


class FakeResponse(io.BytesIO):
    def __init__(self, body=b'{}', code=200, headers=None, url=probe.REDIST):
        super().__init__(body)
        self.code, self.headers, self.url = code, headers or {}, url


class InstallationBoundaryTests(unittest.TestCase):
    def test_token_only_reaches_canonical_api_and_redirects_are_not_followed(self):
        response = FakeResponse()
        seen = []
        class Opener:
            def open(self, request, timeout):
                seen.append(request)
                return response
        with patch.dict(os.environ, {'GH_TOKEN': 'secret-api-token'}), \
             patch.object(probe.urllib.request, 'build_opener', return_value=Opener()) as build:
            self.assertEqual(probe.api_json('repos/msanguineti/twigsmith/actions/artifacts/17'), {})
            self.assertIs(build.call_args.args[0], probe.NoRedirect)
            request = seen[0]
            self.assertEqual(request.full_url, 'https://api.github.com/repos/msanguineti/twigsmith/actions/artifacts/17')
            self.assertEqual(request.get_header('Authorization'), 'Bearer secret-api-token')
            for path in ('https://evil.invalid', 'repos/other/repo/actions/artifacts/1',
                         'repos/msanguineti/twigsmith/actions/../secrets',
                         'repos/msanguineti/twigsmith/actions/artifacts/1?token=x'):
                with self.subTest(path=path), self.assertRaises(ValueError):
                    probe.api_json(path)
        self.assertIsNone(probe.NoRedirect().redirect_request(None, None, 302, '', {}, 'https://other.invalid'))

    def test_redirect_is_https_known_storage_and_contains_no_credentials(self):
        good = 'https://production.blob.core.windows.net/path/artifact.zip?sig=secret'
        with patch.object(probe, 'api_response', return_value=(302, {'Location': good}, b'')):
            self.assertEqual(probe.signed_redirect('17'), good)
            for identity in ('../17', '0', '17?token=x'):
                with self.subTest(identity=identity), self.assertRaises(ValueError):
                    probe.signed_redirect(identity)
        for location in ('http://production.blob.core.windows.net/a', 'https://evil.invalid/a',
                         'https://api.github.com/a', 'https://user:pw@production.blob.core.windows.net/a',
                         'https://production.blob.core.windows.net:444/a'):
            with self.subTest(location=location), \
                 patch.object(probe, 'api_response', return_value=(302, {'Location': location}, b'')), \
                 self.assertRaises(ValueError):
                probe.signed_redirect('17')

    def test_evidence_redacts_urls_tokens_and_child_credentials(self):
        with patch.dict(os.environ, {'GH_TOKEN': 'private-gh', 'GITHUB_TOKEN': 'private-github',
                                     'ACTIONS_RUNTIME_TOKEN': 'private-runtime'}):
            value = probe.clean({'error': 'private-gh https://storage.invalid/a?sig=secret private-github',
                                 'nested': ['https://api.github.com/path']})
            self.assertNotIn('private', json.dumps(value))
            self.assertNotIn('https://', json.dumps(value))
            self.assertNotIn('sig=', json.dumps(value))
            self.assertFalse({'GH_TOKEN', 'GITHUB_TOKEN', 'ACTIONS_RUNTIME_TOKEN'} & set(probe.child_env()))
            with patch.object(probe.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, '', '')) as run:
                probe.command(['payload', '--version'])
                self.assertNotIn('GH_TOKEN', run.call_args.kwargs['env'])
                self.assertEqual(run.call_args.kwargs['timeout'], probe.TIMEOUT)

    def test_unmet_skipped_missing_or_zero_rows_never_pass(self):
        self.assertFalse(probe.passed({'checks': {}}, ()))
        self.assertFalse(probe.passed({'checks': {}}, probe.BROWSER_ROWS))
        report = {'checks': {name: {'status': 'passed'} for name in probe.BROWSER_ROWS}}
        self.assertTrue(probe.passed(report, probe.BROWSER_ROWS))
        for status in ('skipped', 'unmet', 'failed'):
            report['checks']['ordinary_git_journey']['status'] = status
            self.assertFalse(probe.passed(report, probe.BROWSER_ROWS))

    def test_original_zip_hash_and_allowlist_precede_os_extraction(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = io.BytesIO()
            with zipfile.ZipFile(archive, 'w') as zipped:
                zipped.writestr('../escape', b'hostile')
            data = archive.getvalue()
            identity = {'sha256': hashlib.sha256(data).hexdigest()}
            with self.assertRaises(ValueError):
                release_stage.extract_artifact(data, identity, root / 'checked', ['candidate.zip'])
            self.assertFalse((root / 'checked').exists())
            identity['sha256'] = '0' * 64
            with self.assertRaisesRegex(ValueError, 'hash'):
                release_stage.extract_artifact(data, identity, root / 'checked', ['../escape'])
            self.assertFalse((root / 'escape').exists())

    def test_browser_failure_preserves_explicit_unmet_rows_and_sanitizes_error(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            approval = root / 'approval.json'
            approval.write_text('{}')
            args = SimpleNamespace(approval=approval, target='native-fixture', operation='browser',
                                   evidence_dir=root / 'evidence', harness_dir=None)
            with patch.object(probe, 'approved', return_value=({}, {'artifact': 'fixture'})), \
                 patch.object(probe, 'browser_probe', side_effect=RuntimeError('blocked https://signed.invalid/?sig=secret')):
                self.assertEqual(probe.run(args), 1)
            report = json.loads((args.evidence_dir / 'installation.json').read_text())
            self.assertFalse(report['passed'])
            self.assertEqual(set(report['checks']), set(probe.BROWSER_ROWS))
            self.assertTrue(all(row['status'] == 'unmet' for row in report['checks'].values()))
            self.assertNotIn('sig=', json.dumps(report))

    def test_mac_native_archive_utility_uses_original_and_preserves_produced_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / 'twigsmith-fixture.tar.gz'
            archive.write_bytes(b'original-verified-archive-fixture')
            payload = b'original-extracted-payload-fixture'
            expected = {'twigsmith': {'sha256': hashlib.sha256(payload).hexdigest()}}
            output = root / 'expanded'
            produced = root / 'twigsmith-fixture'
            inode = []
            def native_open(args):
                self.assertEqual(args, ['/usr/bin/open', '-a',
                    '/System/Library/CoreServices/Applications/Archive Utility.app', str(archive)])
                produced.mkdir()
                binary = produced / 'twigsmith'
                binary.write_bytes(payload)
                inode.append(binary.stat().st_ino)
                return {'exit': 0, 'stdout': '', 'stderr': ''}
            with patch.object(probe.sys, 'platform', 'darwin'), patch.object(probe, 'command', side_effect=native_open):
                probe.os_extract(archive, output, expected)
            extracted = output / 'twigsmith-fixture/twigsmith'
            self.assertEqual(extracted.read_bytes(), payload)
            self.assertEqual(extracted.stat().st_ino, inode[0])
            self.assertEqual(archive.read_bytes(), b'original-verified-archive-fixture')
            self.assertFalse(produced.exists())
            produced.mkdir()
            with patch.object(probe.sys, 'platform', 'darwin'), \
                 patch.object(probe, 'command') as command, self.assertRaisesRegex(ValueError, 'already exists'):
                probe.os_extract(archive, root / 'other', expected)
            command.assert_not_called()

    def test_container_attaches_script_stdin_and_removes_only_named_container(self):
        calls = []
        def docker(command, **kwargs):
            calls.append((command, kwargs))
            if command[1] == 'run':
                self.assertIn('--interactive', command)
                self.assertEqual(kwargs['input'], probe.CONTAINER_SCRIPT)
                self.assertEqual(command[-2:], ['-Command', '-'])
                self.assertIn('--isolation=process', command)
                self.assertIn('--network=none', command)
                self.assertIn('type=bind,source=candidate-fixture,target=C:\\candidate,readonly', command)
                raise subprocess.TimeoutExpired('synthetic-docker', 300)
            return {'exit': 0, 'stdout': '', 'stderr': ''}
        with patch.object(probe, 'command', side_effect=docker), self.assertRaises(subprocess.TimeoutExpired):
            probe.run_container('docker-fixture', 'sha256:exact-image', 'candidate-fixture', 'isolated-fixture')
        self.assertEqual(calls[-1][0], ['docker-fixture', 'rm', '--force', 'isolated-fixture'])

    def test_windows_policy_unavailable_cannot_be_confused_with_signature_success(self):
        expected = {'defender', 'execution_policy', 'uac', 'explorer_HKLM', 'explorer_HKCU'}
        expected.update('explorer_machine_' + name for name in ('EnableSmartScreen', 'ShellSmartScreenLevel'))
        expected.update('edge_' + hive + '_' + name for hive in ('HKLM', 'HKCU') for name in
                        ('SmartScreenEnabled', 'PreventSmartScreenPromptOverride',
                         'PreventSmartScreenPromptOverrideForFiles', 'SmartScreenForTrustedDownloadsEnabled'))
        checks = {name: {'status': 'observed', 'value': {'state': 'not_configured'}} for name in expected}
        def result(value):
            return {'exit': 0, 'stdout': json.dumps(value), 'stderr': ''}
        with patch.object(probe, 'powershell', return_value=result({'checks': checks})) as powershell:
            self.assertTrue(probe.windows_policy()['available'])
            script = powershell.call_args.args[0]
            for query in ('Get-MpComputerStatus', 'Get-ExecutionPolicy -List', 'SmartScreenEnabled', 'EnableLUA',
                          'HKLM:\\SOFTWARE\\Policies\\Microsoft\\Windows\\System',
                          'EnableSmartScreen', 'ShellSmartScreenLevel'):
                self.assertIn(query, script)
        for mutation in (lambda value: value.pop('defender'),
                         lambda value: value.pop('explorer_machine_EnableSmartScreen'),
                         lambda value: value['defender'].update(status='unavailable', error='disabled service')):
            changed = deepcopy(checks)
            mutation(changed)
            with patch.object(probe, 'powershell', return_value=result({'checks': changed})):
                self.assertFalse(probe.windows_policy()['available'])
        with patch.object(probe, 'powershell', return_value=result({'Status': 'NotSigned'})):
            self.assertFalse(probe.windows_policy()['available'])

    def test_runtime_requires_specific_loader_failure_and_both_real_successes(self):
        observation = {
            'before': [], 'absent': {name: {'exit': -1073741515} for name in ('twigsmith', 'tgi')},
            'installer_exit': 0, 'after': [{'name': 'vcruntime140.dll', 'version': 'fixture'}],
            'installed': {name: {'exit': 0, 'stdout': 'twigsmith 1.0.0\n', 'stderr': ''}
                          for name in ('twigsmith', 'tgi')},
        }
        required = probe.RUNTIME_ROWS[2:]
        report = {'checks': {}}
        probe.record_runtime_observation(report, observation, '1.0.0')
        self.assertTrue(probe.passed(report, required))
        mutations = [lambda value: value['absent']['tgi'].update(exit=1),
                     lambda value: value['absent']['twigsmith'].clear(),
                     lambda value: value.update(before=[{'name': 'vcruntime140.dll'}]),
                     lambda value: value.update(installer_exit=1603),
                     lambda value: value.update(after=[]),
                     lambda value: value['installed']['tgi'].update(stdout='twigsmith other'),
                     lambda value: value['installed'].pop('twigsmith')]
        for mutation in mutations:
            changed = deepcopy(observation)
            mutation(changed)
            report = {'checks': {}}
            probe.record_runtime_observation(report, changed, '1.0.0')
            self.assertFalse(probe.passed(report, required))

    def test_redist_rejects_unapproved_hash_and_foreign_redirect(self):
        for url in ('http://aka.ms/vc14/vc_redist.x64.exe', 'https://download.microsoft.com.evil.invalid/a',
                    'https://user:pass@download.microsoft.com/a'):
            with self.subTest(url=url), self.assertRaises(ValueError):
                probe.validate_microsoft_url(url)
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'redist.exe'
            class Opener:
                def open(self, url, timeout):
                    return FakeResponse(b'wrong-installer')
            with patch.object(probe.urllib.request, 'build_opener', return_value=Opener()), \
                 patch.object(probe, 'powershell') as signature, self.assertRaisesRegex(ValueError, 'SHA-256'):
                probe.redist_download(path, '0' * 64)
            signature.assert_not_called()

    def test_redist_requires_microsoft_signature_and_records_frozen_bytes(self):
        body = b'synthetic-installer-never-executed'
        expected = hashlib.sha256(body).hexdigest()
        class Opener:
            def open(self, url, timeout):
                return FakeResponse(body)
        with tempfile.TemporaryDirectory() as temporary, \
             patch.object(probe.urllib.request, 'build_opener', return_value=Opener()):
            root = Path(temporary)
            for index, (status, subject) in enumerate((('NotSigned', 'O=Microsoft Corporation'),
                                                      ('Valid', 'O=Other Corporation'))):
                output = {'exit': 0, 'stdout': json.dumps({'status': status, 'subject': subject,
                          'thumbprint': 'fixture', 'version': '1.0'}), 'stderr': ''}
                with patch.object(probe, 'powershell', return_value=output), self.assertRaisesRegex(ValueError, 'Microsoft'):
                    probe.redist_download(root / f'refused-{index}.exe', expected)
            output['stdout'] = json.dumps({'status': 'Valid', 'subject': 'CN=Microsoft Corporation, O=Microsoft Corporation, C=US',
                                          'thumbprint': 'fixture', 'version': '14.fixture'})
            with patch.object(probe, 'powershell', return_value=output):
                record = probe.redist_download(root / 'verified.exe', expected)
            self.assertEqual(record['sha256'], expected)
            self.assertEqual(record['version'], '14.fixture')


if __name__ == '__main__':
    unittest.main()
