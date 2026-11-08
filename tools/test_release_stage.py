"""Reject foreign/expired Actions inputs before any candidate is downloaded."""
from copy import deepcopy
import io
import json
import os
import sys
from pathlib import Path
import tempfile
import unittest
import warnings
import zipfile
from unittest.mock import patch

import release_stage


class PlatformStageTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        from test_release_set import platform_fixture
        cls.base_tmp = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.base_tmp.cleanup)
        cls.base = Path(cls.base_tmp.name) / 'fixture'
        cls.approved, cls.artifacts = platform_fixture(cls.base)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.approval = deepcopy(self.approved)
        self.runs, self.metadata = {}, {}
        for item in [*self.approval['native'], self.approval['npm']]:
            identity = item['artifact']
            source = item.get('source', self.approval['npm']['packaging']['source'])['revision']
            self.runs[identity['run_id']] = {'id': int(identity['run_id']),
                'run_attempt': int(identity['run_attempt']),
                'repository': {'full_name': release_stage.REPO}, 'head_repository': {'full_name': release_stage.REPO},
                'status': 'completed', 'conclusion': 'success', 'head_sha': source,
                'event': 'workflow_dispatch', 'path': identity['workflow']}
            self.metadata[identity['artifact_id']] = {'id': int(identity['artifact_id']),
                'name': identity['name'], 'expired': False, 'digest': 'sha256:' + identity['sha256'],
                'workflow_run': {'id': int(identity['run_id']), 'head_sha': source}}

    def fetch(self, path):
        identity = path.rsplit('/', 1)[1]
        return self.metadata[identity] if '/artifacts/' in path else self.runs[identity]

    def acquire(self, output, **kwargs):
        return release_stage.acquire_platform_set(self.approval, output, self.fetch,
            kwargs.pop('download', self.artifacts.__getitem__), kwargs.pop('validate_sources', lambda _: None), **kwargs)

    def test_exact_successful_canonical_artifacts_acquire_atomic_read_only_bundle(self):
        before = {str(path.relative_to(self.base)): path.read_bytes()
                  for path in self.base.rglob('*') if path.is_file()}
        output = self.root / 'bundle'
        record = self.acquire(output)
        self.assertEqual(record, self.approval)
        self.assertEqual(json.loads((output / 'identity.json').read_text()), record)
        self.assertEqual({p.name for p in output.iterdir()}, {'native', 'npm', 'identity.json'})
        self.assertEqual(len(list((output / 'native').iterdir())), 12)
        self.assertEqual(len(list((output / 'npm').iterdir())), 2)
        self.assertEqual(before, {str(path.relative_to(self.base)): path.read_bytes()
                                for path in self.base.rglob('*') if path.is_file()})
        with self.assertRaisesRegex(ValueError, 'must be new'):
            self.acquire(output)

    def test_wrong_attempt_workflow_run_source_digest_and_artifact_refused_before_download(self):
        identity = self.approval['native'][0]['artifact']
        run = self.runs[identity['run_id']]
        artifact = self.metadata[identity['artifact_id']]
        mutations = [(run, 'run_attempt', 2), (run, 'path', '.github/workflows/other.yml'),
                     (run, 'conclusion', 'failure'), (run, 'event', 'release'), (run, 'head_sha', 'f' * 40),
                     (run, 'head_repository', {'full_name': 'fork/repo'}),
                     (artifact, 'expired', True), (artifact, 'name', 'unapproved-artifact'),
                     (artifact, 'id', 999), (artifact, 'digest', 'sha256:' + '0' * 64),
                     (artifact, 'workflow_run', {'id': 99, 'head_sha': '1' * 40})]
        for record, field, value in mutations:
            original = record[field]
            record[field] = value
            def no_download(_):
                self.fail('download occurred before canonical metadata verification')
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.acquire(self.root / field, download=no_download)
            record[field] = original

    def test_source_eligibility_and_corrupt_download_leave_no_output(self):
        def fail(_):
            raise ValueError('source is not eligible')
        output = self.root / 'ineligible'
        with self.assertRaisesRegex(ValueError, 'eligible'):
            self.acquire(output, validate_sources=fail)
        self.assertFalse(output.exists())
        calls = []
        def corrupt(identity):
            calls.append(identity)
            data = self.artifacts[identity]
            return data if len(calls) < 5 else data + b'changed'
        output = self.root / 'corrupt'
        with self.assertRaisesRegex(ValueError, 'ZIP differs'):
            self.acquire(output, download=corrupt)
        self.assertFalse(output.exists())
        self.assertEqual(list(self.root.glob('.release-acquire-*')), [])

    def test_expiry_or_run_attempt_change_during_download_is_not_staged(self):
        def expire(identity):
            self.metadata[identity]['expired'] = True
            return self.artifacts[identity]
        output = self.root / 'expired'
        with self.assertRaisesRegex(ValueError, 'artifact differs'):
            self.acquire(output, download=expire)
        self.assertFalse(output.exists())

    def test_changed_input_during_copy_and_partial_copy_failure_are_atomic(self):
        import release_set
        original = release_stage.shutil.copyfile
        for mode in ('corrupt', 'fail'):
            count = 0
            def copy(source, target):
                nonlocal count
                count += 1
                if count == 5 and mode == 'fail':
                    raise OSError('injected disk failure')
                result = original(source, target)
                if count == 5:
                    Path(target).write_bytes(Path(target).read_bytes() + b'changed')
                return result
            output = self.root / mode
            with patch.object(release_stage.shutil, 'copyfile', side_effect=copy):
                with self.assertRaises((ValueError, OSError)):
                    release_stage.prepare_platform_set(self.base, output, self.approval)
            self.assertFalse(output.exists())
            self.assertEqual(list(self.root.glob('.release-platform-*')), [])
        self.assertEqual(release_set.verify_platform_set(self.base / 'native', self.base / 'npm', self.approval), self.approval)

    def test_retired_operations_are_rejected_before_reading_requests(self):
        import subprocess
        for operation in ('prepare', 'publish', 'validate', 'inspect-event', 'event-request', 'inspect-prepare', 'inspect-publish'):
            result = subprocess.run([sys.executable, str(Path(release_stage.__file__)),
                                     operation, str(self.root / 'nonexistent-request')],
                                    capture_output=True, text=True)
            with self.subTest(operation=operation):
                self.assertEqual(result.returncode, 2)
                self.assertIn('invalid choice', result.stderr)
                self.assertNotIn('No such file', result.stderr)

    def test_only_current_single_installer_approval_is_accepted_before_provider_reads(self):
        for stale in ({'native': {}}, dict(self.approval, schema=2, topology='optional-platform-packages')):
            with self.subTest(schema=stale.get('schema')), self.assertRaisesRegex(ValueError, 'invalid integrated'):
                release_stage.inspect_platform_request(stale, lambda _: self.fail('provider read before refusal'))

    def test_dispatch_requires_current_canonical_main(self):
        env = {'GITHUB_REPOSITORY': release_stage.REPO, 'GITHUB_REF': 'refs/heads/main', 'GITHUB_SHA': '3' * 40}
        request = {'publisher_revision': '3' * 40}
        with patch.dict(os.environ, env, clear=True), patch.object(release_stage.subprocess, 'check_output', return_value='3' * 40):
            release_stage.check_dispatch(request)
            for field, value in [('GITHUB_REPOSITORY', 'fork/repo'), ('GITHUB_REF', 'refs/heads/feature'),
                                 ('GITHUB_SHA', '4' * 40)]:
                old = os.environ[field]
                os.environ[field] = value
                with self.subTest(field=field), self.assertRaises(ValueError):
                    release_stage.check_dispatch(request)
                os.environ[field] = old
        with patch.dict(os.environ, env, clear=True), patch.object(release_stage.subprocess, 'check_output', return_value='4' * 40):
            with self.assertRaisesRegex(ValueError, 'main advanced'):
                release_stage.check_dispatch(request)
        for value in ('0', '../secret', '1\nother=bad', '-1'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                release_stage.positive(value)

    def test_archive_allowlist_blocks_traversal_duplicate_links_and_extras(self):
        import release_set
        identity = deepcopy(self.approval['native'][0]['artifact'])
        cases = [('safe', 'extra'), ('safe', 'safe'), ('../escape',), ('safe',)]
        for index, names in enumerate(cases):
            data = io.BytesIO()
            with warnings.catch_warnings():
                warnings.simplefilter('ignore', UserWarning)
                with zipfile.ZipFile(data, 'w') as zipped:
                    for name in names:
                        entry = zipfile.ZipInfo(name)
                        if index == 3:
                            entry.create_system = 3
                            entry.external_attr = 0o120777 << 16
                        zipped.writestr(entry, b'bad')
            identity['sha256'] = release_set.digest(data.getvalue())
            output = self.root / str(index)
            with self.subTest(case=index), self.assertRaises(ValueError):
                release_stage.extract_artifact(data.getvalue(), identity, output, ['safe'])
            self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
