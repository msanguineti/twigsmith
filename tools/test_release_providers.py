"""Shared release transport, npm isolation, locking and source eligibility checks.

Provider requests are mocked; Git checks use a disposable local shared clone.
No published release fixture or live service is required.
"""
from io import BytesIO
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch
from urllib.error import HTTPError

import release_providers as publish


class NpmDiagnosticTests(unittest.TestCase):
    def test_npm_configuration_ignores_inherited_settings_and_keeps_token_out_of_arguments(self):
        provider = publish.Providers(publish.HTTP())
        def run(arguments, **kwargs):
            environment = kwargs['env']
            self.assertEqual(Path(environment['NPM_CONFIG_USERCONFIG']).read_text(),
                             '//registry.npmjs.org/:_authToken=${NODE_AUTH_TOKEN}\n')
            self.assertEqual(Path(environment['NPM_CONFIG_GLOBALCONFIG']).read_text(), '')
            self.assertEqual(environment['NPM_CONFIG_REGISTRY'], 'https://registry.npmjs.org')
            self.assertNotIn('npm_config_proxy', environment)
            self.assertNotIn('secret-token', ' '.join(arguments))
            return subprocess.CompletedProcess(arguments, 0, stdout='published', stderr='')
        with patch.dict(os.environ, {'NODE_AUTH_TOKEN': 'secret-token', 'npm_config_proxy': 'http://untrusted'}), \
                patch.object(subprocess, 'run', side_effect=run):
            self.assertEqual(provider.npm_command(['publish', 'candidate.tgz']), 'published')

    def test_fixture_npm_uses_only_disposable_registry_credentials(self):
        provider = publish.Providers(publish.HTTP(fixture='http://127.0.0.1:1234'))
        def run(arguments, **kwargs):
            environment = kwargs['env']
            self.assertNotIn('NODE_AUTH_TOKEN', environment)
            self.assertNotIn('NPM_TOKEN', environment)
            self.assertEqual(Path(environment['NPM_CONFIG_USERCONFIG']).read_text(),
                             '//127.0.0.1:1234/:_authToken=disposable-fixture\n')
            return subprocess.CompletedProcess(arguments, 0, stdout='', stderr='')
        with patch.dict(os.environ, {'NODE_AUTH_TOKEN': 'secret', 'NPM_TOKEN': 'secret'}), \
                patch.object(subprocess, 'run', side_effect=run):
            provider.npm_command(['publish', 'candidate.tgz'])

    def test_anonymous_npm_exec_does_not_inherit_publication_credentials(self):
        credentials = {'GH_TOKEN': 'x', 'GITHUB_TOKEN': 'x', 'NODE_AUTH_TOKEN': 'x',
                       'NPM_TOKEN': 'x', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN': 'x',
                       'ACTIONS_ID_TOKEN_REQUEST_URL': 'x'}
        result = subprocess.CompletedProcess([], 0, stdout='', stderr='')
        provider = publish.Providers(publish.HTTP())
        with patch.dict(os.environ, credentials), patch.object(subprocess, 'run', return_value=result) as command:
            provider.npm_command(['exec', '--offline', '--', 'twigsmith', '--version'], anonymous=True)
        for key in credentials:
            self.assertFalse(key in command.call_args.kwargs['env'], key + ' leaked')

    def test_semver_precedence_and_invalid_versions(self):
        versions = ['0.1.0-alpha', '0.1.0-alpha.1', '0.1.0-alpha.beta',
                    '0.1.0-beta', '0.1.0-rc.2', '0.1.0-rc.10', '0.1.0', '0.2.0']
        self.assertEqual(sorted(versions, key=publish.semver_key), versions)
        for version in ('1.2', '1.02.3', '1.2.3-01', '1.2.3+a..b'):
            with self.subTest(version=version), self.assertRaises(publish.PublicationError):
                publish.semver_key(version)

    def test_failure_reports_codes_and_stages_without_raw_credentials_or_provider_text(self):
        secret = 'opaque-sensitive-value'
        stderr = ('npm verbose oidc Failed token exchange request with body message: '
                  'workflow permission denied ' + secret + '\n'
                  'npm http fetch POST 403 https://registry.npmjs.org/private?token=' + secret + '\n'
                  'npm error code ENEEDAUTH\n'
                  'npm error code EOPAQUE_SENSITIVE_VALUE\n'
                  'npm error Authorization: Bearer ' + secret + '\n')
        result = subprocess.CompletedProcess([], 1, stdout=secret, stderr=stderr)
        provider = publish.Providers(publish.HTTP())
        with patch.object(subprocess, 'run', return_value=result) as command:
            with self.assertRaises(publish.PublicationError) as error:
                provider.npm_command(['publish', 'candidate.tgz'])
        self.assertEqual(str(error.exception),
                         'npm operation failed (exit 1; code ENEEDAUTH,unrecognized; HTTP 403; '
                         'OIDC exchange rejected; mentions workflow,permission); remote state must be observed')
        self.assertNotIn(secret, str(error.exception))
        self.assertNotIn('EOPAQUE_SENSITIVE_VALUE', str(error.exception))
        self.assertNotIn('https:', str(error.exception))
        environment = command.call_args.kwargs['env']
        self.assertEqual(environment['NPM_CONFIG_LOGLEVEL'], 'verbose')
        self.assertEqual(environment['NPM_CONFIG_LOGS_MAX'], '0')


class TransportTests(unittest.TestCase):
    def test_fixture_origin_and_live_credential_destinations_are_restricted(self):
        for origin in ('https://example.com', 'http://localhost:1234',
                       'http://127.0.0.1:1234/private', 'http://user@127.0.0.1:1234',
                       'http://127.0.0.1:1234?query=secret'):
            with self.subTest(origin=origin), self.assertRaises(publish.PublicationError):
                publish.HTTP(fixture=origin)
        with self.assertRaisesRegex(publish.PublicationError, 'credentials'):
            publish.HTTP(fixture='http://127.0.0.1:1234', token='secret')
        http = publish.HTTP(token='secret')
        with patch.object(http.opener, 'open') as opened:
            for url in ('https://example.com/private', 'http://api.github.com/private',
                        'https://user@api.github.com/private', 'https://api.github.com:444/private'):
                with self.subTest(url=url), self.assertRaisesRegex(publish.PublicationError, 'outside'):
                    http.request(url, authenticated=True)
            for origin in ('github.com', 'registry.npmjs.org', 'release-assets.githubusercontent.com'):
                with self.subTest(origin=origin), self.assertRaisesRegex(publish.PublicationError, 'credentials'):
                    http.request('https://' + origin + '/private', authenticated=True)
        opened.assert_not_called()

    def test_download_redirect_strips_credentials_and_rechecks_origin(self):
        http = publish.HTTP(token='secret')
        url = 'https://api.github.com/repos/example/assets/1'
        target = 'https://release-assets.githubusercontent.com/download'
        redirect = HTTPError(url, 302, 'redirect', {'Location': target}, None)
        with patch.object(http.opener, 'open', side_effect=[redirect, BytesIO(b'archive')]) as opened:
            self.assertEqual(http.request(url, binary=True, authenticated=True), b'archive')
        first, second = [call.args[0] for call in opened.call_args_list]
        self.assertEqual(first.get_header('Authorization'), 'Bearer secret')
        self.assertIsNone(second.get_header('Authorization'))
        self.assertEqual(second.full_url, target)
        for target in ('https://example.com/steal', 'http://release-assets.githubusercontent.com/download'):
            redirect = HTTPError(url, 302, 'redirect', {'Location': target}, None)
            with self.subTest(target=target), patch.object(http.opener, 'open', side_effect=redirect) as opened, \
                    self.assertRaises(publish.PublicationError):
                http.request(url, binary=True, authenticated=True)
            self.assertEqual(opened.call_count, 1)

    def test_fixture_redirect_cannot_escape_loopback_origin(self):
        http = publish.HTTP(fixture='http://127.0.0.1:1234')
        url = http.fixture + '/asset'
        redirect = HTTPError(url, 302, 'redirect', {'Location': 'https://example.com/steal'}, None)
        with patch.object(http.opener, 'open', side_effect=redirect) as opened, \
                self.assertRaisesRegex(publish.PublicationError, 'outside'):
            http.request(url, binary=True)
        self.assertEqual(opened.call_count, 1)

    def test_tag_lookup_peels_annotated_tags_and_bounds_cycles(self):
        provider = publish.Providers(publish.HTTP())
        with patch.object(provider.http, 'request', side_effect=[
                {'object': {'type': 'tag', 'sha': 'a' * 40}},
                {'object': {'type': 'commit', 'sha': 'b' * 40}}]) as request:
            self.assertEqual(provider.tag_source('v1.0.0'), 'b' * 40)
        self.assertTrue(request.call_args.args[0].endswith('/git/tags/' + 'a' * 40))
        with patch.object(provider.http, 'request', return_value=None):
            self.assertIsNone(provider.tag_source('missing'))
        for obj in ({'type': 'tag', 'sha': 'a' * 40}, {'type': 'tree', 'sha': 'b' * 40}):
            with self.subTest(obj=obj), patch.object(provider.http, 'request', return_value={'object': obj}) as request, \
                    self.assertRaisesRegex(publish.PublicationError, 'bounded commit'):
                provider.tag_source('bad')
            self.assertLessEqual(request.call_count, 6)

    def test_provider_pagination_accumulates_complete_pages_and_bounds_unknown_end(self):
        provider = publish.Providers(publish.HTTP())
        for full, last in (([{}] * 100, []), ({'workflow_runs': [{}] * 100}, {'workflow_runs': []})):
            with self.subTest(full=type(full).__name__), \
                    patch.object(provider.http, 'request', side_effect=[full, last]) as request:
                self.assertEqual(provider.pages('repos/example/releases?per_page=100'), [full, last])
            self.assertTrue(request.call_args.args[0].endswith('&page=2'))
        with patch.object(provider.http, 'request', return_value=[{}] * 100) as request, \
                self.assertRaisesRegex(publish.PublicationError, 'bounded limit'):
            provider.pages('repos/example/releases')
        self.assertEqual(request.call_count, 100)


class SourceAndLockTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name) / 'repository'
        subprocess.run(['git', 'clone', '--shared', '--no-checkout', str(publish.ROOT), str(self.repo)],
                       check=True, capture_output=True)
        self.source = subprocess.check_output(['git', '-C', str(publish.ROOT), 'rev-parse',
                                               'refs/remotes/origin/main'], text=True).strip()
        self.git('update-ref', 'refs/remotes/origin/main', self.source)
        self.record = {'tag': 'v-test-release-provider', 'publication': {'source': self.source}}
        self.providers = Mock()
        self.providers.tag_source.return_value = self.source
        pull = {'number': 3, 'merge_commit_sha': self.source, 'merged_at': 'now',
                'base': {'ref': 'main'}, 'head': {'sha': 'a' * 40, 'ref': 'fixture',
                                               'repo': {'full_name': publish.REPO}}}
        run = {'event': 'pull_request', 'conclusion': 'success', 'head_sha': 'a' * 40,
               'head_branch': 'fixture', 'head_repository': {'full_name': publish.REPO},
               'html_url': 'https://github.com/' + publish.REPO + '/actions/runs/1'}
        self.providers.pages.side_effect = lambda path: [[pull]] if '/commits/' in path else [{'workflow_runs': [run]}]

    def git(self, *arguments):
        return subprocess.check_output(['git', '-C', str(self.repo), *arguments], text=True).strip()

    def test_real_main_source_and_pr_run_are_required_before_publication(self):
        self.assertTrue(publish.source_eligible(self.repo, self.record, self.providers, False))
        self.providers.pages.return_value = []
        self.providers.pages.side_effect = None
        with self.assertRaisesRegex(ValueError, 'No merged main pull request'):
            publish.source_eligible(self.repo, self.record, self.providers, False)
        bad = {**self.record, 'publication': {'source': '0' * 40}}
        with self.assertRaisesRegex(ValueError, 'first-parent'):
            publish.source_eligible(self.repo, bad, self.providers, False)

    def test_execution_requires_existing_unchanged_remote_tag(self):
        self.providers.tag_source.return_value = None
        self.assertFalse(publish.source_eligible(self.repo, self.record, self.providers, False))
        with self.assertRaisesRegex(publish.PublicationError, 'must already exist'):
            publish.source_eligible(self.repo, self.record, self.providers, True)
        self.providers.tag_source.return_value = 'b' * 40
        with self.assertRaisesRegex(publish.PublicationError, 'remote tag differs'):
            publish.source_eligible(self.repo, self.record, self.providers, True)

    def test_conflicting_local_tag_is_rejected_using_real_git_resolution(self):
        self.git('update-ref', 'refs/tags/' + self.record['tag'], self.git('rev-parse', self.source + '^'))
        with self.assertRaisesRegex(publish.PublicationError, 'local tag differs'):
            publish.source_eligible(self.repo, self.record, self.providers, False)

    def test_lock_excludes_same_process_and_fresh_invocation_then_releases(self):
        with publish.exclusive(self.repo):
            with self.assertRaisesRegex(publish.PublicationError, 'holds'):
                with publish.exclusive(self.repo):
                    self.fail('concurrent publication acquired the lock')
            duplicate = subprocess.run([sys.executable, '-c',
                'import sys; sys.path.insert(0, sys.argv[1]); import release_providers; '
                'release_providers.exclusive(sys.argv[2]).__enter__()',
                str(publish.ROOT / 'tools'), str(self.repo)], capture_output=True, text=True)
            self.assertNotEqual(duplicate.returncode, 0)
            self.assertIn('holds the repository lock', duplicate.stderr)
        with publish.exclusive(self.repo):
            pass


if __name__ == '__main__':
    unittest.main()
