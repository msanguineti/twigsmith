import importlib.util
from pathlib import Path
import unittest

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

    def validate(self, sha='squash', *, history=None, pulls=None, runs=None):
        def command(*args):
            self.assertEqual(args, ('git', 'rev-list', '--first-parent', 'refs/remotes/origin/main'))
            return history if history is not None else f'squash\n{source.BASELINE}\nolder'
        def api(path):
            if '/commits/' in path:
                return [pulls if pulls is not None else [self.pr]]
            self.assertIn('foundation.yml/runs?head_sha=head&event=pull_request&status=success', path)
            return [{'workflow_runs': runs if runs is not None else [self.run]}]
        return source.validate_source(sha, command=command, api=api, repo='o/r')

    def test_explicit_integration_and_verified_baseline(self):
        self.assertEqual(self.validate(), {'source': 'squash', 'pull_request': 7, 'run_url': 'run'})
        def no_api(_):
            self.fail('baseline must not need a pull request')
        evidence = source.validate_source(source.BASELINE,
            command=lambda *args: source.BASELINE, api=no_api)
        self.assertEqual(evidence, {'source': source.BASELINE, 'baseline': True})

    def test_main_ancestry_baseline_and_merged_main_identity_required(self):
        for sha, history in [('other', None), ('older', None), ('squash', 'squash')]:
            with self.subTest(sha=sha, history=history), self.assertRaisesRegex(ValueError, 'first-parent'):
                self.validate(sha, history=history)
        for changes in ({'merge_commit_sha': 'other'}, {'merged_at': None}, {'base': {'ref': 'other'}}):
            with self.subTest(changes=changes), self.assertRaisesRegex(ValueError, 'No merged main'):
                self.validate(pulls=[self.pr | changes])

    def test_missing_success_fails_closed(self):
        for runs in ([], [self.run | {'conclusion': 'failure'}], [self.run | {'event': 'push'}]):
            with self.subTest(runs=runs), self.assertRaisesRegex(ValueError, 'No successful'):
                self.validate(runs=runs)


if __name__ == '__main__':
    unittest.main()
