"""Reject foreign/expired Actions inputs before any candidate is downloaded."""
from copy import deepcopy
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
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


if __name__ == '__main__':
    unittest.main()
