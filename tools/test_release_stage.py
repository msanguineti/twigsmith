"""Reject foreign/expired Actions inputs before any candidate is downloaded."""
from copy import deepcopy
import io
import importlib.util
import json
import os
import shutil
import sys
from pathlib import Path
import tempfile
import unittest
import warnings
import zipfile
from unittest.mock import patch

import release_stage


class StageTests(unittest.TestCase):
    def setUp(self):
        from test_release_set import ReleaseSetTests as NativeFixture
        self.fixture = NativeFixture('test_identity_and_mismatch_rejection')
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.native = json.loads(self.fixture.native_manifest.read_text())
        self.record = self.fixture.check()['native']
        self.request = {'publisher_revision': '3' * 40, 'artifact_id': 9, 'native': self.record}
        self.run = {'id': 1, 'run_attempt': 1, 'repository': {'full_name': release_stage.REPO},
                    'head_repository': {'full_name': release_stage.REPO}, 'status': 'completed',
                    'conclusion': 'success', 'head_sha': '1' * 40, 'event': 'workflow_dispatch',
                    'path': '.github/workflows/release.yml'}
        self.artifact = {'id': 9, 'name': 'native-candidate-' + '1' * 40, 'expired': False,
                         'workflow_run': {'id': 1, 'head_sha': '1' * 40}}

    def fetch(self, path):
        return self.artifact if '/artifacts/' in path else self.run

    def test_exact_run_and_foreign_expired_rejection(self):
        self.assertEqual(release_stage.inspect_request('prepare', self.request, self.fetch)['artifact_id'], '9')
        for field, value in [('conclusion', 'failure'), ('path', '.github/workflows/other.yml'),
                             ('head_sha', '4' * 40), ('repository', {'full_name': 'fork/repo'})]:
            old = self.run[field]
            self.run[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                release_stage.inspect_request('prepare', self.request, self.fetch)
            self.run[field] = old
        for field, value in [('expired', True), ('name', 'different'), ('workflow_run', {'id': 2, 'head_sha': '1' * 40})]:
            old = self.artifact[field]
            self.artifact[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                release_stage.inspect_request('prepare', self.request, self.fetch)
            self.artifact[field] = old

    def test_publish_run_must_match_clean_packaging_revision(self):
        record = self.fixture.check()
        request = {'publisher_revision': '3' * 40, 'candidate_run': 2, 'artifact_id': 9, 'approval': record}
        self.run.update(id=2, head_sha=record['npm']['packaging']['source']['revision'], path='.github/workflows/publish.yml')
        self.artifact.update(name='release-candidate', workflow_run={'id': 2, 'head_sha': self.run['head_sha']})
        self.assertEqual(release_stage.inspect_request('publish', request, self.fetch)['run_id'], '2')
        self.run['head_repository']['full_name'] = 'fork/repo'
        with self.assertRaises(ValueError):
            release_stage.inspect_request('publish', request, self.fetch)
        with self.assertRaises(ValueError):
            release_stage.inspect_request('publish', request, self.fetch, publisher='4' * 40)

    def test_preparation_filters_nonrelease_files_and_never_mutates_input(self):
        downloaded = self.fixture.archive.parent
        (downloaded / 'harness.tar.gz').write_bytes(b'not a release asset')
        before = {p.name: p.read_bytes() for p in downloaded.iterdir()}
        output = self.fixture.root / 'bundle'
        record = release_stage.prepare(downloaded, output, self.request)
        self.assertEqual(record['native'], self.record)
        self.assertEqual({p.name for p in (output / 'native').iterdir()},
                         {self.fixture.archive.name, self.fixture.native_manifest.name,
                          self.fixture.archive.name[:-7] + '.sha256'})
        self.assertEqual(before, {p.name: p.read_bytes() for p in downloaded.iterdir()})
        self.assertTrue((output / 'npm/manifest.json').is_file())
        bad = deepcopy(self.request)
        bad['native']['manifest_sha256'] = '0' * 64
        with self.assertRaises(ValueError):
            release_stage.prepare(downloaded, self.fixture.root / 'bad', bad)
        self.assertFalse((self.fixture.root / 'bad').exists())

    def test_stable_descriptor_checked_before_publication(self):
        site = Path(__file__).resolve().parents[1] / 'site'
        sys.path.insert(0, str(site))
        self.addCleanup(sys.path.remove, str(site))
        import candidate
        from test_candidate import CandidateTests
        fixture = CandidateTests('test_reviewed_descriptor_binds_candidate_and_requirements')
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        record, files = fixture.record, fixture.files
        record['publication'] = {'prerelease': False}
        approval = {'native': self.record,
                    'npm': {'packaging': {'source': {'dirty': False}}}}
        request = {'approval': approval}
        root = self.fixture.root / 'descriptor-checkout'
        (root / 'site/releases').mkdir(parents=True)
        shutil.copy(site / 'candidate.py', root / 'site/candidate.py')
        descriptor = root / 'site/releases/0.2.0.json'
        with patch.object(release_stage, 'ROOT', root), \
             patch('release_publish.approved_inputs', return_value=(record, files, {})):
            with self.assertRaises(FileNotFoundError):
                release_stage.validate_bundle(self.fixture.root, request)
            descriptor.write_text(json.dumps(candidate.descriptor_for(record, files)))
            self.assertEqual(release_stage.validate_bundle(self.fixture.root, request)[0], record)
            changed = json.loads(descriptor.read_text())
            changed['npm']['sha256'] = '0' * 64
            descriptor.write_text(json.dumps(changed))
            with self.assertRaisesRegex(ValueError, 'differs'):
                release_stage.validate_bundle(self.fixture.root, request)
            record['publication']['prerelease'] = True
            descriptor.unlink()
            self.assertEqual(release_stage.validate_bundle(self.fixture.root, request)[0], record)

    def test_dispatch_main_and_input_identity_guards(self):
        env = {'GITHUB_REPOSITORY': release_stage.REPO, 'GITHUB_REF': 'refs/heads/main', 'GITHUB_SHA': '3' * 40}
        with patch.dict(os.environ, env, clear=True), patch.object(release_stage.subprocess, 'check_output', return_value='3' * 40):
            release_stage.check_dispatch(self.request)
            os.environ['GITHUB_REF'] = 'refs/heads/feature'
            with self.assertRaises(ValueError):
                release_stage.check_dispatch(self.request)
        for value in ('0', '../secret', '1\nother=bad', '-1'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                release_stage.positive(value)

    def test_release_continuation_binds_original_run_release_and_candidate(self):
        record = self.fixture.check()
        record['publication'] = {'source': record['native']['source']['revision'],
                                 'channel': 'next', 'staging_channel': 'candidate', 'prerelease': True}
        request = {'candidate_run': 2, 'artifact_id': 9, 'approval': record}
        body = '<!-- twigsmith-release-request\n' + json.dumps(request) + '\n-->'
        release = {'id': 44, 'tag_name': record['tag'], 'body': body, 'prerelease': True,
                   'published_at': '2026-01-02T00:00:00Z', 'draft': False}
        event = {'action': 'published', 'repository': {'full_name': release_stage.REPO},
                 'source': record['native']['source']['revision'], 'run_id': '3', 'run_attempt': '1',
                 'release': deepcopy(release)}
        event_run = {'run_id': '3', 'attempt': '1', 'source': event['source']}
        self.run.update(id=2, head_sha=record['npm']['packaging']['source']['revision'],
                        path='.github/workflows/publish.yml', updated_at='2026-01-01T00:00:00Z')
        self.artifact.update(name='release-candidate', workflow_run={'id': 2, 'head_sha': self.run['head_sha']},
                             created_at='2026-01-01T00:00:00Z')
        def fetch(path):
            if '/releases/' in path:
                return release
            return self.fetch(path)
        value = release_stage.request_from_event(event, event_run, '3' * 40, fetch)
        self.assertEqual(value['approval']['publication']['release_id'], 44)
        for field, value in [('action', 'edited'), ('source', 'f' * 40),
                             ('repository', {'full_name': 'fork/repo'}), ('run_attempt', '2')]:
            old = event[field]
            event[field] = value
            with self.subTest(event_field=field), self.assertRaises(ValueError):
                release_stage.request_from_event(event, event_run, '3' * 40, fetch)
            event[field] = old
        for field, value in [('body', body + ' changed'), ('tag_name', 'v0.0.1'), ('id', 45), ('prerelease', False)]:
            old = release[field]
            release[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                release_stage.request_from_event(event, event_run, '3' * 40, fetch)
            release[field] = old
        self.artifact['created_at'] = '2026-01-03T00:00:00Z'
        with self.assertRaisesRegex(ValueError, 'before release'):
            release_stage.request_from_event(event, event_run, '3' * 40, fetch)

    def test_release_event_run_must_be_canonical_success_and_exact_artifact(self):
        run = deepcopy(self.run)
        run.update(id=3, event='release', run_attempt=2)
        artifact = {'id': 17, 'name': 'release-event-3-2', 'expired': False,
                    'workflow_run': {'id': 3, 'head_sha': run['head_sha']}}
        def fetch(path):
            return {'artifacts': [artifact]} if '/artifacts?' in path else run
        self.assertEqual(release_stage.inspect_event_run(3, 2, fetch)['artifact_id'], '17')
        for field, value in [('conclusion', 'failure'), ('event', 'workflow_dispatch'),
                             ('head_repository', {'full_name': 'fork/repo'}), ('run_attempt', 1)]:
            old = run[field]
            run[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                release_stage.inspect_event_run(3, 2, fetch)
            run[field] = old
        artifact['expired'] = True
        with self.assertRaises(ValueError):
            release_stage.inspect_event_run(3, 2, fetch)
        artifact['expired'] = False
        artifact['workflow_run']['head_sha'] = 'e' * 40
        with self.assertRaises(ValueError):
            release_stage.inspect_event_run(3, 2, fetch)



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
                     (run, 'conclusion', 'failure'), (run, 'head_sha', 'f' * 40),
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

    def test_legacy_write_boundaries_explicitly_refuse_complete_platform_set(self):
        request = {'approval': self.approval}
        with self.assertRaisesRegex(ValueError, 'legacy publisher'):
            release_stage.inspect_request('publish', request, lambda _: self.fail('provider read before refusal'))
        with self.assertRaisesRegex(ValueError, 'legacy publisher'):
            release_stage.validate_bundle(self.base, request)

    def test_legacy_guards_refuse_stale_integrated_schema_two(self):
        request = {'approval': {'schema': 2, 'topology': 'optional-platform-packages'}}
        with self.assertRaisesRegex(ValueError, 'legacy publisher'):
            release_stage.inspect_request('publish', request, lambda _: self.fail('provider read before refusal'))
        with self.assertRaisesRegex(ValueError, 'legacy publisher'):
            release_stage.validate_bundle(self.base, request)
        stale = deepcopy(self.approval)
        stale.update(schema=2, topology='optional-platform-packages')
        with self.assertRaisesRegex(ValueError, 'invalid integrated'):
            release_stage.inspect_platform_request(stale, lambda _: self.fail('provider read before refusal'))

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
