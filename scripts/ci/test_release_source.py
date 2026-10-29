import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('release_source', Path(__file__).with_name('select-release-source.py'))
source = importlib.util.module_from_spec(spec)
spec.loader.exec_module(source)


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.pr = {'number': 7, 'head': {'sha': 'head', 'ref': 'branch', 'repo': {'full_name': 'o/r'}}, 'base': {'ref': 'main'},
                   'merged_at': 'now', 'merge_commit_sha': 'squash'}
        self.run = {'event': 'pull_request', 'head_sha': 'head', 'conclusion': 'success',
                    'pull_requests': [], 'head_branch': 'branch', 'head_repository': {'full_name': 'o/r'}, 'html_url': 'run'}

    def test_squash_identity_and_missing_validation(self):
        self.assertEqual(source.eligible_pull([self.pr], [{'workflow_runs': [self.run]}], 'squash'), (7, 'run'))
        for changes in ({'conclusion': 'failure'}, {'head_sha': 'different'},
                        {'head_branch': 'other'}, {'head_repository': {'full_name': 'other/repo'}}, {'event': 'push'}):
            run = self.run | changes
            self.assertIsNone(source.eligible_pull([self.pr], [{'workflow_runs': [run]}], 'squash'))
        self.assertIsNone(source.eligible_pull([self.pr], [{'workflow_runs': [self.run]}], 'other'))
        for missing in (None, ''):
            pr = self.pr | {'head': self.pr['head'] | {'sha': missing}}
            run = self.run | {'head_sha': missing}
            self.assertIsNone(source.eligible_pull([pr], [{'workflow_runs': [run]}], 'squash'))

    def test_entry_points_and_main_ancestry(self):
        with tempfile.TemporaryDirectory() as d:
            output = Path(d, 'output')
            env = {'EVENT_NAME': 'workflow_dispatch', 'EVENT_REF': 'refs/heads/main',
                   'GITHUB_SHA': 'squash', 'RUNNER_TEMP': d, 'GITHUB_OUTPUT': str(output), 'GITHUB_REPOSITORY': 'o/r'}
            def command(*args):
                if args[:2] == ('git', 'rev-list'):
                    return f'squash\n{source.BASELINE}'
                return 'squash'
            def api(path):
                return [[self.pr]] if '/commits/' in path else [{'workflow_runs': [self.run]}]
            with patch.dict(os.environ, env, clear=True), patch.object(source, 'command', side_effect=command), patch.object(source, 'api', side_effect=api):
                source.select()
                self.assertEqual(Path(d, 'candidate-sha').read_text().strip(), 'squash')
                os.environ['EVENT_REF'] = 'refs/heads/feature'
                with self.assertRaisesRegex(ValueError, 'main'):
                    source.select()
                os.environ.update(EVENT_NAME='release', RELEASE_TAG='v1')
                source.select()
                with patch.object(source, 'command', side_effect=lambda *args: 'unrelated' if args[:2] == ('git', 'rev-list') else 'squash'):
                    with self.assertRaisesRegex(ValueError, 'first-parent'):
                        source.select()
                with patch.object(source, 'command', return_value='different'):
                    with self.assertRaisesRegex(ValueError, 'tag does not match'):
                        source.select()


    def test_missing_success_fails_closed(self):
        with tempfile.TemporaryDirectory() as d:
            env = {'EVENT_NAME': 'workflow_dispatch', 'EVENT_REF': 'refs/heads/main',
                   'GITHUB_SHA': 'squash', 'RUNNER_TEMP': d, 'GITHUB_OUTPUT': str(Path(d, 'output')), 'GITHUB_REPOSITORY': 'o/r'}
            def command(*args):
                return f'squash\n{source.BASELINE}' if args[:2] == ('git', 'rev-list') else 'squash'
            def api(path):
                return [[self.pr]] if '/commits/' in path else [{'workflow_runs': []}]
            with patch.dict(os.environ, env, clear=True), patch.object(source, 'command', side_effect=command), patch.object(source, 'api', side_effect=api):
                with self.assertRaisesRegex(ValueError, 'No successful'):
                    source.select()
                self.assertFalse(Path(d, 'candidate-sha').exists())


if __name__ == '__main__':
    unittest.main()
