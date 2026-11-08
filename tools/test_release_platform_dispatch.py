"""Authenticate the publication attempt and require its complete consumer matrix."""
from copy import deepcopy
from datetime import datetime, timezone
import json
import os
import subprocess
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import release_platform_dispatch as dispatch
import qualify_public_release as consumer
from test_release_set import platform_fixture
from test_npm_platform_package import candidate


class DispatchTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.base = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.base.cleanup)
        with patch('test_npm_platform_package.candidate', side_effect=lambda *args: candidate(*args, version='1.0.0-rc.1')):
            cls.approval, _ = platform_fixture(Path(cls.base.name) / 'set')
        cls.approval['publication'] = {'source': '1' * 40, 'release_id': 1,
            'prerelease': True, 'channel': 'next', 'staging_channel': 'candidate-v1-rc-1'}

    def setUp(self):
        self.request = {'publisher_revision': 'a' * 40, 'approval': deepcopy(self.approval)}
        self.env = {'GITHUB_ACTIONS': 'true', 'GITHUB_REPOSITORY': dispatch.REPO,
                    'GITHUB_REF': 'refs/heads/main', 'GITHUB_SHA': 'a' * 40,
                    'GITHUB_WORKFLOW': 'Publish approved prerelease',
                    'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_RUN_ID': '27', 'GITHUB_RUN_ATTEMPT': '2'}
        self.run = {'id': 27, 'run_attempt': 2, 'repository': {'full_name': dispatch.REPO},
                    'head_repository': {'full_name': dispatch.REPO}, 'head_sha': 'a' * 40,
                    'head_branch': 'main', 'event': 'workflow_dispatch', 'path': '.github/workflows/publish.yml'}
        self.jobs = [{'name': 'Public installation (' + target + ')', 'status': 'completed', 'conclusion': 'success'}
                     for target in sorted(dispatch.TARGETS)]
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.directory = Path(self.tmp.name)
        self.paths = {}
        for target in sorted(dispatch.TARGETS):
            path = self.directory / ('public-consumption-' + target + '-2') / 'public-consumption.json'
            path.parent.mkdir()
            checks = {'native', 'npm_local', 'npm_global', 'npx_twigsmith', 'npx_tgi'}
            if target != 'x86_64-unknown-linux-gnu':
                checks.add('browser')
            details = {}
            for name in checks:
                details[name] = {'passed': True,
                    'report': name + ('/installation.json' if name == 'browser' else '/qualification.json'),
                    'identity': consumer.expected_identity(self.approval, target, name),
                    'aliases': {alias: {'passed': True, 'exit': 0, 'version': self.approval['version']}
                                for alias in ('twigsmith', 'tgi')},
                    'cases': [{'suite': suite, 'name': case, 'passed': True, 'exit': 0}
                              for suite, cases in consumer.expected_cases(name).items() for case in cases]}
                if name.startswith('npx_'):
                    details[name].update(cold_cache=True, invoked_alias=name[4:])
            report = {'schema': 1, 'approval_sha256': dispatch.approval_digest(self.request['approval']),
                      'target': target, 'observed_at': datetime.now(timezone.utc).isoformat(),
                      'producer': {'publisher_revision': 'a' * 40, 'run_id': '27', 'run_attempt': '2'},
                      'checks': details, 'passed': True}
            path.write_text(json.dumps(report))
            self.paths[target] = path

    def fetch(self, path):
        return {'jobs': self.jobs} if '/jobs?' in path else self.run

    def evidence(self):
        return dispatch.consumption_evidence(self.directory, self.request, self.fetch, self.env)

    def test_complete_current_attempt_has_exactly_four_reports(self):
        self.assertEqual(len(self.evidence()), 4)

    def test_request_binds_protected_server_main_and_all_original_artifacts(self):
        calls = []
        def fetch(path):
            calls.append(path)
            if path.endswith('/branches/main'):
                return {'commit': {'sha': self.request['publisher_revision']}, 'protected': True}
            for item in [*self.approval['native'], self.approval['npm']]:
                identity = item['artifact']
                source = item.get('source', self.approval['npm']['packaging']['source'])['revision']
                if path.endswith('/artifacts/' + identity['artifact_id']):
                    return {'id': int(identity['artifact_id']), 'name': identity['name'],
                            'expired': False, 'digest': 'sha256:' + identity['sha256'],
                            'workflow_run': {'id': int(identity['run_id']), 'head_sha': source}}
                if path.endswith('/runs/' + identity['run_id']):
                    return {'id': int(identity['run_id']), 'run_attempt': int(identity['run_attempt']),
                            'repository': {'full_name': dispatch.REPO}, 'head_repository': {'full_name': dispatch.REPO},
                            'status': 'completed', 'conclusion': 'success', 'event': 'workflow_dispatch',
                            'path': identity['workflow'], 'head_sha': source}
            self.fail('unexpected API read')
        self.assertEqual(len(dispatch.inspect_request(self.request, fetch)), 5)
        for main in ({'commit': {'sha': 'b' * 40}, 'protected': True},
                     {'commit': {'sha': 'a' * 40}, 'protected': False}):
            with self.subTest(main=main), self.assertRaisesRegex(ValueError, 'protected current main'):
                dispatch.inspect_request(self.request, lambda _: main)
        changed = deepcopy(self.request)
        changed['approval']['publication']['prerelease'] = False
        with self.assertRaises(ValueError):
            dispatch.inspect_request(changed, lambda _: self.fail('read before rejection'))
        changed = deepcopy(self.request)
        changed['approval']['native'][0]['source']['revision'] = 'b' * 40
        with self.assertRaises(ValueError):
            dispatch.inspect_request(changed, lambda _: self.fail('read before rejection'))
        changed = deepcopy(self.request)
        changed['approval']['publication']['staging_channel'] = 'latest'
        with self.assertRaises(ValueError):
            dispatch.inspect_request(changed, lambda _: self.fail('read before rejection'))

    def test_consumer_validation_imports_without_unix_lock_module(self):
        subprocess.run([sys.executable, '-c',
            "import sys; sys.modules['fcntl'] = None; import release_platform_publish, qualify_public_release"],
            cwd=Path(__file__).resolve().parent, check=True, capture_output=True)

    def test_cross_job_baseline_preserves_all_advertised_channels(self):
        names = {'twigsmith'}
        state = {'tags': {name: {'latest': '0.1.0', 'next': '0.0.1'} for name in names}}
        baseline = {'schema': 1, 'approval_sha256': dispatch.approval_digest(self.request['approval']),
                    'producer': dispatch.producer(self.env), 'channels': dispatch.advertised_channels(state),
                    'site_sha256': '1' * 64}
        dispatch.validate_baseline(baseline, self.request, state, 'stage', self.env)
        state['tags']['twigsmith']['next'] = self.request['approval']['version']
        with self.assertRaisesRegex(ValueError, 'changed since'):
            dispatch.validate_baseline(baseline, self.request, state, 'stage', self.env)
        dispatch.validate_baseline(baseline, self.request, state, 'promote', self.env)
        for name in names:
            for tag in ('latest', 'next'):
                original = state['tags'][name][tag]
                state['tags'][name][tag] = '2.0.0'
                with self.subTest(name=name, tag=tag), self.assertRaisesRegex(ValueError, 'changed since'):
                    dispatch.validate_baseline(baseline, self.request, state, 'promote', self.env)
                state['tags'][name][tag] = original
        baseline['producer']['run_attempt'] = '1'
        with self.assertRaisesRegex(ValueError, 'exact approved workflow attempt'):
            dispatch.validate_baseline(baseline, self.request, state, 'promote', self.env)

    def test_old_five_package_baseline_and_schema_two_request_refused(self):
        state = {'tags': {'twigsmith': {'latest': '0.1.0', 'next': '0.0.1'}}}
        baseline = {'schema': 1, 'approval_sha256': dispatch.approval_digest(self.request['approval']),
                    'producer': dispatch.producer(self.env), 'channels': dispatch.advertised_channels(state),
                    'site_sha256': '1' * 64}
        baseline['channels']['twigsmith-linux-x64-gnu'] = {'latest': None, 'next': None}
        with self.assertRaisesRegex(ValueError, 'exact approved workflow attempt'):
            dispatch.validate_baseline(baseline, self.request, state, 'stage', self.env)
        self.request['approval'].update(schema=2, topology='optional-platform-packages')
        with self.assertRaisesRegex(ValueError, 'invalid integrated'):
            dispatch.inspect_request(self.request, lambda _: self.fail('provider read before stale approval refusal'))

    def test_wrong_context_refuses(self):
        for key, value in [('GITHUB_REF', 'refs/heads/other'), ('GITHUB_REPOSITORY', 'fork/repo'),
                           ('GITHUB_SHA', 'b' * 40), ('GITHUB_EVENT_NAME', 'pull_request'),
                           ('GITHUB_RUN_ATTEMPT', '0')]:
            env = {**self.env, key: value}
            with self.subTest(key=key), self.assertRaises(ValueError):
                dispatch.workflow_context(self.request, env)

    def test_failed_skipped_missing_duplicate_jobs_refuse(self):
        for status in ('failure', 'skipped', 'cancelled', None):
            self.jobs[0]['conclusion'] = status
            with self.subTest(status=status), self.assertRaises(ValueError):
                self.evidence()
        self.jobs[0]['conclusion'] = 'success'
        job = self.jobs.pop()
        with self.assertRaises(ValueError):
            self.evidence()
        self.jobs.extend((job, deepcopy(job)))
        with self.assertRaises(ValueError):
            self.evidence()

    def test_old_attempt_foreign_head_and_workflow_refuse(self):
        for key, value in [('run_attempt', 1), ('head_sha', 'b' * 40), ('event', 'pull_request'),
                           ('path', '.github/workflows/other.yml'), ('head_branch', 'other'),
                           ('head_repository', {'full_name': 'fork/repo'})]:
            original = self.run[key]
            self.run[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.evidence()
            self.run[key] = original

    def test_zero_children_tampered_payload_or_warm_npx_cache_refuse(self):
        path = self.paths['x86_64-unknown-linux-gnu']
        original = json.loads(path.read_text())
        mutations = [('native', 'cases', []), ('npm_global', 'identity', {}),
                     ('npx_tgi', 'cold_cache', False), ('native', 'aliases', {})]
        for route, field, value in mutations:
            report = deepcopy(original)
            report['checks'][route][field] = value
            path.write_text(json.dumps(report))
            with self.subTest(route=route, field=field), self.assertRaises(ValueError):
                self.evidence()
        path.write_text(json.dumps(original))

    def test_missing_extra_symlink_reports_refuse(self):
        path = next(iter(self.paths.values()))
        data = path.read_bytes()
        path.unlink()
        with self.assertRaises(ValueError):
            self.evidence()
        other = self.directory / 'extra'
        other.write_bytes(data)
        path.symlink_to(other)
        with self.assertRaises(ValueError):
            self.evidence()

    def test_mixed_approval_producer_target_time_and_missing_case_refuse(self):
        path = next(iter(self.paths.values()))
        original = json.loads(path.read_text())
        for key, value in [('approval_sha256', '0' * 64), ('producer', {}), ('target', 'other'),
                           ('observed_at', '2020-01-01T00:00:00Z'), ('passed', False), ('checks', {})]:
            path.write_text(json.dumps({**original, key: value}))
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.evidence()
        path.write_text(json.dumps(original))


if __name__ == '__main__':
    unittest.main()
