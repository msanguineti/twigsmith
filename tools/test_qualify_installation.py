"""Synthetic trust-boundary tests; these never claim browser/Windows qualification."""
from copy import deepcopy
import contextlib
import base64
import hashlib
import io
import json
import os
import shutil
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
import native_package
from test_native_package import pe_fixture, macho_fixture


RUNTIME_MANIFEST = {'version': 'fixture', 'target': 'x86_64-pc-windows-msvc',
                    'inspection': {name: {'msvc_runtime_imports': ['VCRUNTIME140.dll', 'api-ms-win-crt-heap-l1-1-0.dll']}
                                   for name in ('twigsmith.exe', 'tgi.exe')}}


class FakeResponse(io.BytesIO):
    def __init__(self, body=b'{}', code=200, headers=None, url=probe.REDIST):
        super().__init__(body)
        self.code, self.headers, self.url = code, headers or {}, url


class InstallationBoundaryTests(unittest.TestCase):
    def test_native_aliases_bind_individual_manifest_hashes_and_selected_npm_payload(self):
        # Real ZIP/tar and PE/Mach-O formats with synthetic bytes: no runtime claim.
        for target in ('x86_64-pc-windows-msvc', 'aarch64-apple-darwin', 'x86_64-apple-darwin'):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                metadata = native_package.target_metadata(target)
                image = pe_fixture() if metadata['os'] == 'windows' else macho_fixture(metadata['arch'])
                main, alias = native_package.executable_names(target)
                payloads = {main: image + b'primary-command-fixture',
                            alias: image + b'alias-command-fixture', 'LICENSE': b'MIT License\nfixture'}
                provenance = {
                    'kind': 'ci-candidate',
                    'source': {'revision': '1' * 40, 'tree': '2' * 40, 'dirty': False},
                    'build': {'target': target, 'profile': 'release', 'locked': True, 'compiler': 'fixture'},
                    'toolchain': {'pin': 'fixture', 'rustc': 'rustc fixture\nhost: ' + target, 'cargo': 'fixture'},
                    'environment': {'system': 'synthetic format fixture only', 'machine': metadata['arch'],
                                    'ci': {'GITHUB_REPOSITORY': 'msanguineti/twigsmith', 'GITHUB_SHA': '1' * 40,
                                           'GITHUB_RUN_ID': '1', 'GITHUB_RUN_ATTEMPT': '1'}},
                }
                archive = native_package.write_bundle(root, '0.1.0', payloads, provenance, 1, target=target)
                manifest_path = archive.with_name(native_package.archive_stem(archive) + '.manifest.json')
                manifest = json.loads(manifest_path.read_text())
                item = {'target': target, 'archive': archive.name,
                        'sha256': native_package.digest(archive.read_bytes()),
                        'manifest_sha256': native_package.digest(manifest_path.read_bytes()),
                        'payload_sha256': manifest['files'][main]['sha256'],
                        'source': provenance['source'], 'ci': provenance['environment']['ci']}
                approval = {'version': '0.1.0', 'requirements': {target: manifest['requirements']}}
                self.assertNotEqual(manifest['files'][main]['sha256'], manifest['files'][alias]['sha256'])
                self.assertEqual(probe.verify_candidate(archive, approval, item), manifest)
                for key, value in (('payload_sha256', manifest['files'][alias]['sha256']),
                                   ('manifest_sha256', '0' * 64), ('sha256', '0' * 64)):
                    changed = deepcopy(item)
                    changed[key] = value
                    with self.subTest(field=key), self.assertRaises(ValueError):
                        probe.verify_candidate(archive, approval, changed)
                # A validly repacked second alias still needs fresh archive/manifest approval.
                payloads[alias] += b'changed-alias'
                native_package.write_bundle(root, '0.1.0', payloads, provenance, 1, target=target)
                with self.assertRaisesRegex(ValueError, 'archive differs'):
                    probe.verify_candidate(archive, approval, item)
                changed = deepcopy(item)
                changed['sha256'] = native_package.digest(archive.read_bytes())
                with self.assertRaisesRegex(ValueError, 'manifest differs'):
                    probe.verify_candidate(archive, approval, changed)

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

    def test_command_timeout_preserves_redacted_partial_progress_and_nonzero_status(self):
        error = subprocess.TimeoutExpired(['unrecorded', 'https://private.invalid/?sig=private'], 300,
            output=b'layer-a Download complete\nlayer-b Extracting https://signed.invalid/?sig=private',
            stderr=b'partial-progress private-token')
        with patch.dict(os.environ, {'GH_TOKEN': 'private-token'}), \
             patch.object(probe.subprocess, 'run', side_effect=error):
            result = probe.command(['docker', 'pull', probe.IMAGE], timeout=300)
        self.assertTrue(result['timed_out'])
        self.assertNotEqual(result['exit'], 0)
        self.assertEqual(result['timeout_seconds'], 300)
        self.assertIn('layer-a Download complete', result['stdout'])
        self.assertIn('layer-b Extracting', result['stdout'])
        self.assertIn('partial-progress', result['stderr'])
        self.assertNotIn('private-token', json.dumps(result))
        self.assertNotIn('sig=', json.dumps(result))
        self.assertNotIn('unrecorded', json.dumps(result))

    def test_structured_command_preserves_parseable_long_json_while_progress_is_bounded(self):
        body = json.dumps({'path': 'a' * 30000})
        completed = subprocess.CompletedProcess([], 0, body, '')
        with patch.object(probe.subprocess, 'run', return_value=completed):
            structured = probe.command(['structured-query'], max_stdout=None)
            progress = probe.command(['progress-query'])
        self.assertEqual(json.loads(structured['stdout']), {'path': 'a' * 30000})
        self.assertFalse(structured['stdout_truncated'])
        self.assertEqual(len(progress['stdout']), 24000)
        self.assertTrue(progress['stdout_truncated'])

    def test_host_helper_selects_and_reports_actual_core_76_without_51_fallback(self):
        with tempfile.TemporaryDirectory() as temporary:
            selected = Path(temporary) / 'pwsh.exe'
            selected.write_bytes(b'never-executed-host-seam')
            identity = {'executable': str(selected), 'version': '7.6.7', 'edition': 'Core',
                        'home': str(selected.parent), 'module_search_path': 'selected-module-search',
                        'modules': {command: {'status': 'observed', 'name': module, 'version': '1.2.6',
                                             'path': str(selected.parent / module / 'module.psd1')}
                                    for command, module in (('Expand-Archive', 'Microsoft.PowerShell.Archive'),
                                                            ('Get-AuthenticodeSignature', 'Microsoft.PowerShell.Security'))}}
            inspected = {'exit': 0, 'stdout': json.dumps(identity), 'stderr': ''}
            probe.powershell_host.cache_clear()
            self.addCleanup(probe.powershell_host.cache_clear)
            with patch.object(probe.shutil, 'which', return_value=str(selected)), \
                 patch.object(probe, 'command', side_effect=[inspected, {'exit': 0, 'stdout': 'helper-result', 'stderr': ''}]) as run:
                result = probe.powershell('Get-ExecutionPolicy -List')
                self.assertEqual(run.call_count, 2)
                self.assertEqual(run.call_args.args[0][0], str(selected))
                self.assertNotIn('powershell.exe', run.call_args.args[0])
                self.assertEqual(result['host_powershell'], identity)
                self.assertIn('Get-Command', run.call_args_list[0].args[0][-1])
                self.assertIn('$env:PSModulePath', run.call_args_list[0].args[0][-1])
            for version, edition in (('5.1.0', 'Desktop'), ('7.5.4', 'Core'), ('7.7.0', 'Core')):
                probe.powershell_host.cache_clear()
                identity.update(version=version, edition=edition)
                with patch.object(probe.shutil, 'which', return_value=str(selected)), \
                     patch.object(probe, 'command', return_value={'exit': 0, 'stdout': json.dumps(identity), 'stderr': ''}), \
                     self.assertRaises(ValueError):
                    probe.powershell_host()
            probe.powershell_host.cache_clear()
            identity.update(version='7.6.7', edition='Core')
            identity['modules']['Get-AuthenticodeSignature']['status'] = 'unavailable'
            with patch.object(probe.shutil, 'which', return_value=str(selected)), \
                 patch.object(probe, 'command', return_value={'exit': 0, 'stdout': json.dumps(identity), 'stderr': ''}), \
                 self.assertRaisesRegex(ValueError, 'module identities'):
                probe.powershell_host()
            probe.powershell_host.cache_clear()
            with patch.object(probe.shutil, 'which', return_value=None), self.assertRaises(RuntimeError):
                probe.powershell_host()

    def test_ads_enumeration_distinguishes_absence_unavailability_and_zone_class(self):
        for observed, state, present in (
            ({'streams': [{'Stream': ':$DATA', 'Length': 1}], 'zone_stream_present': False, 'zone_identifier': ''},
             'observed_absent', False),
            ({'streams': [{'Stream': 'Zone.Identifier', 'Length': 20}], 'zone_stream_present': True,
              'zone_identifier': '[ZoneTransfer]\r\nZoneId=3\r\n'}, 'observed_present', True),
            ({'streams': [{'Stream': 'Zone.Identifier', 'Length': 20}], 'zone_stream_present': True,
              'zone_identifier': '[ZoneTransfer]\r\nZoneId=2\r\n'}, 'observed_present', False)):
            with patch.object(probe.sys, 'platform', 'win32'), \
                 patch.object(probe, 'powershell', return_value={'exit': 0, 'stdout': json.dumps(observed), 'stderr': ''}) as query:
                metadata = probe.security_metadata(Path('original-file'))
            self.assertEqual(metadata['state'], state)
            self.assertEqual(metadata['present'], present)
            self.assertIn('Get-Item -LiteralPath $p -Stream *', query.call_args.args[0])
            self.assertIn('[System.IO.File]::OpenRead($p)', query.call_args.args[0])
        with patch.object(probe.sys, 'platform', 'win32'), \
             patch.object(probe, 'powershell', return_value={'exit': 1, 'stdout': '', 'stderr': 'file cannot be read'}):
            metadata = probe.security_metadata(Path('unreadable-file'))
        self.assertEqual(metadata['state'], 'unavailable')
        self.assertFalse(metadata['present'])
        with patch.object(probe.sys, 'platform', 'win32'), \
             patch.object(probe, 'powershell', return_value={'exit': 0, 'stdout': '{}', 'stderr': ''}):
            self.assertEqual(probe.security_metadata(Path('unknown-file'))['state'], 'unavailable')

    def test_windows_extracted_metadata_is_observed_without_forging_inheritance(self):
        marks = {'outer': {'state': 'observed_present', 'present': True, 'zone_id': 3},
                 'archive': {'state': 'observed_absent', 'present': False},
                 'twigsmith.exe': {'state': 'observed_absent', 'present': False},
                 'tgi.exe': {'state': 'observed_present', 'present': True, 'zone_id': 3}}
        with patch.object(probe.sys, 'platform', 'win32'):
            self.assertTrue(probe.extraction_metadata_observed(marks))
            for name in marks:
                unavailable = deepcopy(marks)
                unavailable[name]['state'] = 'unavailable'
                self.assertFalse(probe.extraction_metadata_observed(unavailable))
            for zone in (None, 0, 1, 2, 4):
                wrong_origin = deepcopy(marks)
                wrong_origin['outer']['zone_id'] = zone
                self.assertFalse(probe.extraction_metadata_observed(wrong_origin))
        with patch.object(probe.sys, 'platform', 'darwin'):
            self.assertFalse(probe.extraction_metadata_observed(marks))
            self.assertTrue(probe.extraction_metadata_observed({'outer': {'present': True}, 'alias': {'present': True}}))

    def test_runtime_pull_timeout_retains_progress_and_observes_image_state_separately(self):
        report = {'checks': {}}
        args = SimpleNamespace(archive=Path('approved-candidate.zip'), redist_sha256='1' * 64)
        outcomes = [{'exit': 0, 'stdout': json.dumps({'OSType': 'windows', 'ServerVersion': 'actual-fixture'}), 'stderr': ''},
                    {'exit': 124, 'timed_out': True, 'stdout': 'layer-a Download complete', 'stderr': 'layer-b Extracting'},
                    {'exit': 1, 'stdout': '', 'stderr': 'image not yet complete'}]
        with patch.object(probe.sys, 'platform', 'win32'), \
             patch.object(probe, 'verify_candidate', return_value=RUNTIME_MANIFEST), \
             patch.object(probe.shutil, 'which', return_value='docker-fixture'), \
             patch.object(probe, 'command', side_effect=outcomes) as command:
            probe.runtime_probe(args, {}, {}, report)
        self.assertEqual(command.call_args_list[1].kwargs['timeout'], 900)
        self.assertEqual(command.call_args_list[2].args[0][1:3], ['image', 'inspect'])
        self.assertEqual(report['container_engine']['ServerVersion'], 'actual-fixture')
        self.assertTrue(report['container_pull']['timed_out'])
        self.assertIn('Download complete', report['container_pull']['stdout'])
        self.assertIn('image not yet complete', report['container_image_inspection']['stderr'])
        self.assertEqual(report['checks']['isolated_container']['status'], 'unmet')

    def test_timeout_client_can_observe_completed_official_image_without_claiming_pull_success(self):
        report = {'checks': {}}
        args = SimpleNamespace(archive=Path('approved-candidate.zip'), redist_sha256='1' * 64)
        identity = {'Id': 'sha256:' + '1' * 64,
                    'RepoDigests': ['mcr.microsoft.com/windows/servercore@sha256:' + '2' * 64]}
        outcomes = [{'exit': 0, 'stdout': json.dumps({'OSType': 'windows'}), 'stderr': ''},
                    {'exit': 124, 'timed_out': True, 'stdout': 'last layer complete', 'stderr': ''},
                    {'exit': 0, 'stdout': json.dumps([identity]), 'stderr': ''}]
        with patch.object(probe.sys, 'platform', 'win32'), \
             patch.object(probe, 'verify_candidate', return_value=RUNTIME_MANIFEST), \
             patch.object(probe.shutil, 'which', return_value='docker-fixture'), \
             patch.object(probe, 'command', side_effect=outcomes), \
             patch.object(probe.native_package, 'verify', side_effect=RuntimeError('bounded stop after image selection')) as prepare, \
             self.assertRaisesRegex(RuntimeError, 'bounded stop'):
            probe.runtime_probe(args, {}, {}, report)
        prepare.assert_called_once()
        self.assertTrue(report['container_pull']['timed_out'])
        self.assertEqual(report['container_pull']['exit'], 124)
        self.assertEqual(report['container_image_inspection']['exit'], 0)
        self.assertFalse(probe.passed(report, probe.RUNTIME_ROWS))

    def test_download_progress_uses_matching_guid_and_requires_browser_completion(self):
        begin = {'event': 'Browser.downloadWillBegin', 'guid': 'selected', 'suggestedFilename': 'original.zip'}
        events = [begin, {'event': 'Browser.downloadProgress', 'guid': 'selected', 'state': 'inProgress'}]
        self.assertEqual(probe.selected_download_progress(events)['state'], 'inProgress')
        events.append({'event': 'Browser.downloadProgress', 'guid': 'other', 'state': 'completed'})
        self.assertEqual(probe.selected_download_progress(events)['state'], 'inProgress')
        events.append({'event': 'Browser.downloadProgress', 'guid': 'selected', 'state': 'completed'})
        self.assertEqual(probe.selected_download_progress(events)['state'], 'completed')
        events.append({'event': 'Browser.downloadProgress', 'guid': 'selected', 'state': 'canceled'})
        self.assertEqual(probe.selected_download_progress(events)['state'], 'canceled')
        events.append({'event': 'Browser.downloadWillBegin', 'guid': 'extra'})
        with self.assertRaisesRegex(RuntimeError, 'more than one'):
            probe.selected_download_progress(events)

    def test_cdp_download_events_never_retain_signed_url(self):
        event = {'method': 'Browser.downloadWillBegin', 'params': {'guid': 'selected',
                 'suggestedFilename': 'original.zip', 'url': 'https://storage.invalid/a?sig=private'}}
        response = {'id': 1, 'result': {}}
        wire = io.BytesIO(b''.join(bytes([0x81, 126]) + len(data).to_bytes(2, 'big') + data
                                   for data in (json.dumps(event).encode(), json.dumps(response).encode())))
        client = probe.CDP.__new__(probe.CDP)
        client.events, client.sequence = [], 0
        client.exact = wire.read
        client.frame = lambda opcode, data: None
        client.call('Browser.getVersion')
        self.assertEqual(client.events, [{'event': 'Browser.downloadWillBegin', 'guid': 'selected',
                                         'suggestedFilename': 'original.zip'}])
        self.assertNotIn('sig=', json.dumps(client.events))

    def test_browser_waits_for_delayed_headers_and_requires_matching_completed_download(self):
        for outcome in ('completed', 'canceled', 'wrong-hash', 'missing-completion'):
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                directory, profile = root / 'download', root / 'profile'
                directory.mkdir(); profile.mkdir()
                original = directory / 'original.zip'
                payload = b'fully-readable-but-still-in-progress'
                (profile / 'DevToolsActivePort').write_text('12345\n/devtools/browser/probe\n')
                state = {'closed': False, 'now': 0, 'requested': False, 'begun': False, 'calls': []}
                url = 'https://signed.invalid/private?sig=secret'

                class Browser:
                    def __init__(self, endpoint):
                        self.events = []
                    def call(self, method, params=None):
                        state['calls'].append(method)
                        if method == 'Browser.setDownloadBehavior':
                            assert params['eventsEnabled']
                        elif method == 'Target.createTarget':
                            assert params == {'url': url}
                            state['requested'] = True
                            return {'targetId': 'page'}
                        elif method == 'Browser.getVersion':
                            if state['requested'] and state['now'] >= 12 and not state['begun']:
                                # Headers arrive after the former page socket's 10s
                                # timeout; target creation itself has already returned.
                                original.write_bytes(payload)
                                self.events.extend([
                                    {'event': 'Browser.downloadWillBegin', 'guid': 'selected', 'suggestedFilename': original.name},
                                    {'event': 'Browser.downloadProgress', 'guid': 'selected', 'state': 'inProgress'}])
                                state['begun'] = True
                            if state['begun'] and state['now'] >= 13 and outcome != 'missing-completion':
                                terminal = 'canceled' if outcome == 'canceled' else 'completed'
                                self.events.append({'event': 'Browser.downloadProgress', 'guid': 'selected', 'state': terminal})
                        elif method == 'Browser.close':
                            state['closed'] = True
                            original.unlink(missing_ok=True)
                        else:
                            raise AssertionError('unexpected page operation: ' + method)
                        return {'product': 'native-browser-fixture'}
                    def close(self):
                        pass

                def sleep(_):
                    if state['now'] < 12:
                        self.assertFalse(original.exists())
                    state['now'] += 1

                process = SimpleNamespace(poll=lambda: None, wait=lambda timeout: 0)
                observation = {}
                expected = '0' * 64 if outcome == 'wrong-hash' else hashlib.sha256(payload).hexdigest()
                with patch.object(probe, 'browser_binary', return_value=Path('native-browser-fixture')), \
                     patch.object(probe, 'CDP', Browser), \
                     patch.object(probe.subprocess, 'Popen', return_value=process), \
                     patch.object(probe.time, 'monotonic', side_effect=lambda: state['now']), \
                     patch.object(probe.time, 'sleep', side_effect=sleep):
                    if outcome == 'completed':
                        with probe.browser_download(url, directory, profile, expected, observation) as (path, observed):
                            self.assertEqual(state['now'], 13)
                            self.assertFalse(state['closed'])
                            self.assertTrue(path.is_file())
                            self.assertEqual(observed['download_events'][-1]['state'], 'completed')
                    else:
                        failure, message = (ValueError, 'hash mismatch') if outcome == 'wrong-hash' else (
                            RuntimeError, 'canceled' if outcome == 'canceled' else 'did not complete')
                        with self.assertRaisesRegex(failure, message):
                            with probe.browser_download(url, directory, profile, expected, observation):
                                self.fail('unverified download must not yield a candidate')
                self.assertTrue(state['closed'])
                self.assertFalse(original.exists())
                self.assertTrue(observation['lifecycle'][-2]['files'])
                self.assertEqual(observation['lifecycle'][-1]['files'], [])
                self.assertNotIn('Page.navigate', state['calls'])
                self.assertNotIn('signed.invalid', json.dumps(observation))
                self.assertNotIn('secret', json.dumps(observation))

    def test_browser_timeout_identifies_operation_after_cleanup_without_protocol_input(self):
        stages = ('browser_websocket', 'Browser.getVersion', 'Browser.setDownloadBehavior',
                  'Target.createTarget', 'download_progress_poll')
        for stage in stages:
            with self.subTest(stage=stage), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                directory, profile = root / 'download', root / 'profile'
                directory.mkdir(); profile.mkdir()
                (profile / 'DevToolsActivePort').write_text('12345\n/devtools/browser/probe\n')
                closed = []

                def fail(name):
                    if name == stage:
                        raise TimeoutError('timed out https://signed.invalid/?sig=secret')

                class Browser:
                    def __init__(self, url):
                        fail('browser_websocket')
                        self.events = []
                        self.version_calls = 0
                    def call(self, method, params=None):
                        if method == 'Browser.getVersion':
                            self.version_calls += 1
                            fail('Browser.getVersion' if self.version_calls == 1 else 'download_progress_poll')
                        else:
                            fail(method)
                        if method == 'Browser.close':
                            closed.append('browser')
                        if method == 'Target.createTarget':
                            return {'targetId': 'page'}
                        return {'product': 'native-browser-fixture'}
                    def close(self):
                        closed.append('websocket')

                observation = {}
                process = SimpleNamespace(poll=lambda: None, wait=lambda timeout: 0)
                with patch.object(probe, 'browser_binary', return_value=Path('native-browser-fixture')), \
                     patch.object(probe, 'CDP', Browser), \
                     patch.object(probe.subprocess, 'Popen', return_value=process):
                    with self.assertRaises(TimeoutError) as error:
                        with probe.browser_download('https://signed.invalid/?sig=secret', directory,
                                                    profile, '0' * 64, observation):
                            self.fail('timeout must not qualify a download')
                self.assertEqual(observation['operations'][-1], {'name': stage, 'status': 'failed'})
                self.assertTrue(all(item['status'] == 'completed' for item in observation['operations'][:-1]))
                self.assertEqual(observation['lifecycle'][-1]['stage'], 'after_browser_close')
                saved = json.dumps(probe.clean({'error': str(error.exception), 'browser_lifecycle': observation}))
                self.assertNotIn('signed.invalid', saved)
                self.assertNotIn('secret', saved)
                self.assertNotIn('private protocol response', saved)

    def test_browser_original_removed_after_close_is_explicitly_unmet(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            report = {'checks': {name: {'status': 'passed'} for name in probe.BROWSER_ROWS}}
            item = {'artifact': {'artifact_id': '17', 'sha256': '0' * 64}}
            @contextlib.contextmanager
            def download(url, directory, profile, expected_hash, observation):
                original = directory / 'original.zip'
                original.write_bytes(b'original')
                observation['lifecycle'] = []
                yield original, observation
                original.unlink()
            args = SimpleNamespace(evidence_dir=root)
            with patch.object(probe.sys, 'platform', 'win32'), \
                 patch.object(probe.release_stage, 'inspect_platform_request'), \
                 patch.object(probe, 'signed_redirect', return_value='signed-url-not-retained'), \
                 patch.object(probe, 'browser_download', side_effect=download), \
                 patch.object(probe, 'inspect_browser_download'), \
                 patch.object(probe, 'windows_policy', return_value={'available': True}) as policy, \
                 patch.object(probe, 'security_metadata', return_value={'present': False}):
                probe.browser_probe(args, {}, item, report)
            self.assertEqual(policy.call_count, 2)
            self.assertEqual(report['checks']['original_retained_after_browser_close']['status'], 'unmet')
            self.assertFalse(probe.passed(report, probe.BROWSER_ROWS))

    def test_public_browser_uses_direct_native_url_hash_without_actions_authority(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            item = {'archive': 'twigsmith-1.0.0-rc.1-aarch64-apple-darwin.tar.gz',
                    'sha256': native_package.digest(b'public-original'),
                    'artifact': {'artifact_id': '17', 'sha256': '0' * 64}}
            report = {'checks': {}}
            @contextlib.contextmanager
            def download(url, directory, profile, expected_hash, observation):
                self.assertEqual(url, 'https://github.com/msanguineti/twigsmith/releases/download/v1.0.0-rc.1/' + item['archive'])
                self.assertEqual(expected_hash, item['sha256'])
                original = directory / item['archive']
                original.write_bytes(b'public-original')
                observation['lifecycle'] = []
                yield original, observation
            args = SimpleNamespace(evidence_dir=root, public_release=True)
            with patch.object(probe.sys, 'platform', 'darwin'), \
                 patch.object(probe.release_stage, 'inspect_platform_request') as actions, \
                 patch.object(probe, 'signed_redirect') as redirect, \
                 patch.object(probe, 'browser_download', side_effect=download), \
                 patch.object(probe, 'inspect_browser_download'), \
                 patch.object(probe, 'security_metadata', return_value={'present': True}):
                probe.browser_probe(args, {'tag': 'v1.0.0-rc.1'}, item, report)
            actions.assert_not_called()
            redirect.assert_not_called()
            self.assertEqual(report['checks']['original_retained_after_browser_close']['status'], 'passed')

    def test_public_original_archive_is_verified_before_os_extraction_and_no_outer_zip(self):
        import qualify_public_release
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            original = root / 'twigsmith-1.0.0-rc.1-aarch64-apple-darwin.tar.gz'
            original.write_bytes(b'original-security-metadata-preserved')
            item = {'archive': original.name, 'manifest_sha256': '1' * 64}
            args = SimpleNamespace(evidence_dir=root, public_release=True)
            report = {'checks': {}}
            with patch.object(probe, 'security_metadata', return_value={'present': True}), \
                 patch.object(qualify_public_release, 'public_download') as download, \
                 patch.object(probe, 'verify_candidate', side_effect=ValueError('public manifest mismatch')) as verify, \
                 patch.object(probe.release_stage, 'extract_artifact') as wrapper, \
                 patch.object(probe, 'os_extract') as extractor:
                with self.assertRaisesRegex(ValueError, 'manifest mismatch'):
                    probe.inspect_browser_download(args, {'tag': 'v1.0.0-rc.1'}, item, report,
                                                   original, {'lifecycle': []})
            self.assertEqual(download.call_count, 2)
            self.assertEqual(verify.call_args.args[0], original)
            wrapper.assert_not_called()
            extractor.assert_not_called()
            self.assertEqual(original.read_bytes(), b'original-security-metadata-preserved')

    def test_no_runtime_daemon_captures_read_only_host_state_and_stays_unmet(self):
        report = {'checks': {}}
        args = SimpleNamespace(archive=Path('approved-candidate.zip'), redist_sha256='1' * 64)
        with patch.object(probe.sys, 'platform', 'win32'), \
             patch.object(probe, 'verify_candidate', return_value=RUNTIME_MANIFEST), \
             patch.object(probe.shutil, 'which', return_value='docker-fixture'), \
             patch.object(probe, 'command', return_value={'exit': 1, 'stdout': '', 'stderr': 'no named pipe'}) as command, \
             patch.object(probe, 'runtime_host_state', return_value={'exit': 0, 'stdout': 'stopped-service', 'stderr': ''}) as host:
            probe.runtime_probe(args, {}, {}, report)
        host.assert_called_once()
        self.assertEqual(command.call_count, 1)
        self.assertEqual(report['checks']['isolated_container']['status'], 'unmet')
        self.assertIn('stopped-service', report['runtime_host_state']['stdout'])
        with patch.object(probe, 'powershell', return_value={}) as powershell:
            probe.runtime_host_state()
        script = powershell.call_args.args[0]
        self.assertIn('Get-CimInstance Win32_Service', script)
        self.assertNotIn('Start-Service', script)
        self.assertNotIn('Set-Service', script)

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

    def test_encoded_transport_binds_exact_decoded_script_and_handles_unicode(self):
        for script in (probe.CONTAINER_SCRIPT, "Write-Output 'δοκιμή'"):
            arguments = probe.encoded_powershell_arguments(script)
            self.assertEqual(arguments[0], '-EncodedCommand')
            self.assertEqual(base64.b64decode(arguments[1], validate=True), script.encode('utf-16-le'))
        transport = probe.container_script_transport()
        payload = base64.b64decode(probe.encoded_powershell_arguments(probe.CONTAINER_SCRIPT)[1], validate=True)
        self.assertEqual(transport['decoded_script_sha256'], hashlib.sha256(payload).hexdigest())
        self.assertEqual(transport['decoded_script_bytes'], len(payload))
        self.assertEqual(transport['encoding'], 'utf-16-le')
        self.assertFalse(transport['stdin_required'])

    def test_encoded_multiline_script_executes_real_powershell_logic_and_json(self):
        # Optional local process evidence; never a Windows-container/runtime pass.
        selected = shutil.which('powershell.exe') if os.name == 'nt' else None
        selected = selected or shutil.which('pwsh')
        local = Path(__file__).resolve().parents[1] / '.dev/cache/shell-runtimes/pwsh/pwsh'
        if selected is None and local.is_file():
            selected = str(local)
        if selected is None:
            self.skipTest('PowerShell is unavailable for the actual process transport regression')
        script = r'''$ErrorActionPreference='Stop'
function Values {
  foreach ($number in @(1,2)) { $number * 2 }
}
$values=@(Values)
if ($values.Count -eq 2) {
  [pscustomobject]@{transport='observed';count=$values.Count;values=$values;nested=@{sum=($values[0]+$values[1])}} | ConvertTo-Json -Depth 4 -Compress
}
'''
        environment = probe.child_env()
        if Path(selected).name.casefold() == 'powershell.exe':
            # This test deliberately covers Windows PowerShell 5.1. Prevent a
            # parent pwsh 7 module path from selecting incompatible modules;
            # the production container has its own independent environment.
            environment = {name: value for name, value in environment.items()
                           if name.casefold() != 'psmodulepath'}
        output = subprocess.run([selected, '-NoLogo', '-NoProfile', '-NonInteractive',
                                 *probe.encoded_powershell_arguments(script)],
                                capture_output=True, text=True, encoding='utf-8', errors='replace',
                                timeout=30, env=environment)
        self.assertEqual(output.returncode, 0, output.stderr)
        self.assertEqual(output.stderr, '')
        self.assertEqual(json.loads(output.stdout), {'transport': 'observed', 'count': 2,
                                                    'values': [2, 4], 'nested': {'sum': 6}})

    def test_container_encodes_fixed_script_and_removes_only_named_container(self):
        calls = []
        def docker(command, **kwargs):
            calls.append((command, kwargs))
            if command[1] == 'run':
                self.assertNotIn('--interactive', command)
                self.assertIn('powershell.exe', command)
                self.assertNotIn('pwsh', command)
                self.assertNotIn('input', kwargs)
                self.assertEqual(command[-2], '-EncodedCommand')
                self.assertEqual(base64.b64decode(command[-1]).decode('utf-16-le'), probe.CONTAINER_SCRIPT)
                self.assertNotIn('-ExecutionPolicy', command)
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

    def test_runtime_requirement_is_bound_to_each_verified_pe_import(self):
        self.assertEqual(probe.required_runtime_dlls(RUNTIME_MANIFEST), ['VCRUNTIME140.dll'])
        for name in ('vcruntime140_clr0400.dll', 'api-ms-win-crt-heap-l1-1-0.dll', 'msvcp140.dll'):
            changed = deepcopy(RUNTIME_MANIFEST)
            changed['inspection']['tgi.exe']['msvc_runtime_imports'] = [name]
            with self.subTest(name=name), self.assertRaises(ValueError):
                probe.required_runtime_dlls(changed)

    def test_runtime_requires_specific_loader_failure_and_both_real_successes(self):
        observation = {
            'before': [], 'absent': {name: {'exit': -1073741515} for name in ('twigsmith', 'tgi')},
            'installer_exit': 0, 'after': [{'name': 'vcruntime140.dll', 'version': 'fixture'}],
            'installed': {name: {'exit': 0, 'stdout': 'twigsmith 1.0.0\n', 'stderr': ''}
                          for name in ('twigsmith', 'tgi')},
        }
        required = probe.RUNTIME_ROWS[2:]
        report = {'checks': {}}
        probe.record_runtime_observation(report, observation, '1.0.0', ['VCRUNTIME140.dll'])
        self.assertTrue(probe.passed(report, required))
        unrelated = deepcopy(observation)
        unrelated['before'] = [{'name': 'vcruntime140_clr0400.dll', 'version': 'unrelated-CLR-runtime'}]
        report = {'checks': {}}
        probe.record_runtime_observation(report, unrelated, '1.0.0', ['VCRUNTIME140.dll'])
        self.assertTrue(probe.passed(report, required))
        mutations = [lambda value: value['absent']['tgi'].update(exit=1),
                     lambda value: value['absent']['twigsmith'].clear(),
                     lambda value: value.update(before=[{'name': 'vcruntime140.dll'}]),
                     lambda value: value.update(installer_exit=1603),
                     lambda value: value.update(after=[]),
                     lambda value: value.update(after=[{'name': 'vcruntime140_clr0400.dll'}]),
                     lambda value: value['installed']['tgi'].update(stdout='twigsmith other'),
                     lambda value: value['installed'].pop('twigsmith')]
        for mutation in mutations:
            changed = deepcopy(observation)
            mutation(changed)
            report = {'checks': {}}
            probe.record_runtime_observation(report, changed, '1.0.0', ['VCRUNTIME140.dll'])
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
