"""Disposable HTTP providers + real npm exercise publication effects and recovery.

No endpoint here is a live GitHub/npm service. C compiles an inert version fixture;
real native qualification remains separate from these publication-boundary tests.
"""
import base64
import copy
import importlib.util
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import time
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

import native_package
import npm_package
import release_publish as publish
import release_set


class Fixture:
    def __init__(self, source):
        self.source = source
        self.release = None
        self.other_releases = []
        self.assets = {}
        self.package = None
        self.tgz = None
        self.tags = {'next': '0.0.1', 'latest': '0.0.0'}
        self.effects = []
        self.fail = None
        self.lost = None
        self.unknown = False
        self.timeout_after = None
        self.truncated_after = None
        self.unknown_after = None
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def reply(self, value, code=200, binary=False):
                data = value if binary else json.dumps(value).encode()
                self.send_response(code)
                self.send_header('Content-Type', 'application/octet-stream' if binary else 'application/json')
                self.send_header('Content-Length', str(len(data)))
                self.send_header('Cache-Control', 'public, max-age=300')
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                path = urlsplit(self.path).path
                if fixture.unknown:
                    return self.reply({}, 503)
                if '/git/ref/tags/' in path:
                    return self.reply({'object': {'type': 'commit', 'sha': fixture.source}})
                if '/commits/' in path and path.endswith('/pulls'):
                    return self.reply([{'number': 3, 'merge_commit_sha': fixture.source, 'merged_at': 'now',
                        'base': {'ref': 'main'}, 'head': {'sha': 'a'*40, 'ref': 'fixture', 'repo': {'full_name': publish.REPO}}}])
                if '/actions/workflows/' in path:
                    return self.reply({'workflow_runs': [{'event': 'pull_request', 'conclusion': 'success',
                        'head_sha': 'a'*40, 'head_branch': 'fixture', 'head_repository': {'full_name': publish.REPO},
                        'html_url': 'https://github.com/msanguineti/twigsmith/actions/runs/1'}]})
                if '/releases/tags/' in path:
                    published = fixture.release if fixture.release and not fixture.release['draft'] else None
                    return self.reply(published or {}, 200 if published else 404)
                if path.endswith('/releases'):
                    entries = fixture.other_releases + ([fixture.release] if fixture.release else [])
                    page = int(parse_qs(urlsplit(self.path).query).get('page', ['1'])[0])
                    return self.reply(entries[(page - 1) * 100:page * 100])
                if path.endswith('/releases/1/assets'):
                    return self.reply([{'name': name, 'id': i + 10, 'state': 'uploaded', 'digest': 'sha256:UNTRUSTED'}
                                       for i, name in enumerate(fixture.assets)])
                if '/releases/assets/' in path:
                    name = list(fixture.assets)[int(path.rsplit('/', 1)[1]) - 10]
                    return self.reply(fixture.assets[name], binary=True)
                if '/releases/download/' in path:
                    name = path.rsplit('/', 1)[1]
                    return self.reply(fixture.assets[name], binary=True)
                if path == '/-/package/twigsmith/dist-tags':
                    return self.reply(fixture.tags)
                if path.startswith('/twigsmith/-/'):
                    return self.reply(fixture.tgz, binary=True) if fixture.tgz else self.reply({}, 404)
                if path == '/twigsmith':
                    if fixture.package:
                        return self.reply({'name': 'twigsmith', 'dist-tags': fixture.tags,
                                           'versions': {fixture.package['version']: fixture.package}})
                    return self.reply({}, 404)
                if path.startswith('/twigsmith/'):
                    return self.reply(fixture.package or {}, 200 if fixture.package else 404)
                return self.reply({}, 404)

            def mutate(self):
                path = urlsplit(self.path).path
                body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
                if path.endswith('/releases'):
                    label = 'create draft'
                elif path.endswith('/assets'):
                    label = 'upload ' + parse_qs(urlsplit(self.path).query)['name'][0]
                elif path.endswith('/releases/1'):
                    label = 'publish release'
                elif path == '/twigsmith':
                    label = 'publish npm'
                elif path.startswith('/-/package/twigsmith/dist-tags/'):
                    label = 'promote channel'
                else:
                    return self.reply({}, 404)
                fixture.effects.append(label)
                if fixture.fail == label or fixture.fail == 'upload' and label.startswith('upload '):
                    return self.reply({'message': 'rejected fixture operation'}, 403)
                if label == 'create draft':
                    fixture.release = json.loads(body) | {'id': 1}
                elif label.startswith('upload '):
                    fixture.assets[label[7:]] = body
                elif label == 'publish release':
                    fixture.release.update(json.loads(body))
                elif label == 'publish npm':
                    data = json.loads(body)
                    version, package = next(iter(data['versions'].items()))
                    fixture.package = package
                    fixture.package['dist']['tarball'] = fixture.origin + '/twigsmith/-/twigsmith-' + version + '.tgz'
                    fixture.tgz = base64.b64decode(next(iter(data['_attachments'].values()))['data'])
                    fixture.tags.update(data['dist-tags'])
                elif label == 'promote channel':
                    fixture.tags[path.rsplit('/', 1)[1]] = json.loads(body)
                if fixture.unknown_after == label:
                    fixture.unknown = True
                if fixture.truncated_after == label:
                    fixture.truncated_after = None
                    self.send_response(201)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', '100')
                    self.end_headers()
                    self.wfile.write(b'{')
                    self.close_connection = True
                    return
                if fixture.timeout_after == label:
                    fixture.timeout_after = None
                    time.sleep(0.2)
                    self.connection.close()
                    return
                if fixture.lost == label:
                    fixture.lost = None
                    self.connection.shutdown(socket.SHUT_RDWR)
                    self.connection.close()
                    return
                return self.reply(fixture.release or {'ok': True}, 201)

            do_POST = mutate
            do_PATCH = mutate
            do_PUT = mutate

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.origin = 'http://127.0.0.1:' + str(self.server.server_port)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


class PublicationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.root = Path(cls.tmp.name)
        cls.repo = Path(__file__).resolve().parents[1]
        cls.source = subprocess.check_output(['git', '-C', str(cls.repo), 'rev-parse', 'c43a43a'], text=True).strip()
        c = cls.root / 'version.c'
        c.write_text('#include <stdio.h>\nint main(void) { puts("twigsmith 0.1.0-beta.1"); return 0; }\n')
        binary = cls.root / 'version'
        subprocess.run(['cc', '-o', str(binary), str(c)], check=True, capture_output=True)
        payload = binary.read_bytes()
        native = cls.root / 'native'
        native.mkdir()
        provenance = {'kind': 'ci-candidate', 'source': {'revision': cls.source, 'tree': '2'*40, 'dirty': False},
            'build': {'target': native_package.TARGET, 'profile': 'release'},
            'toolchain': {'rustc': 'fixture', 'cargo': 'fixture'},
            'environment': {'system': 'fixture', 'ci': {'GITHUB_REPOSITORY': publish.REPO,
                'GITHUB_SHA': cls.source, 'GITHUB_RUN_ID': '1', 'GITHUB_RUN_ATTEMPT': '1'}}}
        cls.archive = native_package.write_bundle(native, '0.1.0-beta.1',
            {'twigsmith': payload, 'tgi': payload, 'LICENSE': (cls.repo / 'LICENSE').read_bytes()}, provenance, 1)
        cls.npm = cls.root / 'npm'
        package = npm_package.make_package(cls.repo, cls.archive, cls.npm,
            npm_package.sha256(cls.archive), release_set.digest(payload))
        manifest = cls.archive.with_name(cls.archive.name[:-7] + '.manifest.json')
        cls.approval = release_set.verify(cls.archive, cls.npm, npm_package.sha256(cls.archive),
            release_set.digest(payload), npm_package.sha256(package), 'v0.1.0-beta.1',
            npm_package.sha256(manifest), npm_package.sha256(cls.npm / 'manifest.json'))
        cls.approval['publication'] = {'source': cls.source, 'channel': 'next',
                                     'staging_channel': 'candidate-beta-1', 'prerelease': True, 'release_id': 1}
        c.write_text('#include <stdio.h>\nint main(void) { puts("twigsmith 0.1.0"); return 0; }\n')
        subprocess.run(['cc', '-o', str(binary), str(c)], check=True, capture_output=True)
        payload = binary.read_bytes()
        stable_native = cls.root / 'stable-native'
        stable_native.mkdir()
        cls.stable_archive = native_package.write_bundle(stable_native, '0.1.0',
            {'twigsmith': payload, 'tgi': payload, 'LICENSE': (cls.repo / 'LICENSE').read_bytes()}, provenance, 1)
        cls.stable_npm = cls.root / 'stable-npm'
        package = npm_package.make_package(cls.repo, cls.stable_archive, cls.stable_npm,
            npm_package.sha256(cls.stable_archive), release_set.digest(payload))
        manifest = cls.stable_archive.with_name(cls.stable_archive.name[:-7] + '.manifest.json')
        cls.stable_approval = release_set.verify(cls.stable_archive, cls.stable_npm,
            npm_package.sha256(cls.stable_archive), release_set.digest(payload), npm_package.sha256(package),
            'v0.1.0', npm_package.sha256(manifest), npm_package.sha256(cls.stable_npm / 'manifest.json'))
        cls.stable_approval['publication'] = {'source': cls.source, 'channel': 'latest',
            'staging_channel': 'candidate-stable', 'prerelease': False, 'release_id': 1}

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def setUp(self):
        self.fixture = Fixture(self.source)
        self.addCleanup(self.fixture.close)
        self.providers = publish.Providers(publish.HTTP(fixture=self.fixture.origin, timeout=1))
        self.record, self.files, self.expected = publish.approved_inputs(self.archive, self.npm, self.approval)
        self.fixture.release = {'id': 1, 'tag_name': self.record['tag'], 'draft': False, 'prerelease': True}

    def run_release(self, **kwargs):
        return publish.resume(self.record, self.files, self.expected, self.providers, **kwargs)

    def ready_native(self):
        self.fixture.release = {'id': 1, 'tag_name': self.record['tag'], 'draft': False, 'prerelease': True}
        self.fixture.assets = {name: self.files[name] for name in self.expected}

    def use_stable(self):
        self.record, self.files, self.expected = publish.approved_inputs(
            self.stable_archive, self.stable_npm, self.stable_approval)
        self.fixture.release = {'id': 1, 'tag_name': self.record['tag'], 'draft': False, 'prerelease': False}

    def test_stable_visible_release_retry_and_site_only_after_verified_channel(self):
        self.use_stable()
        first = sorted(self.expected)[0]
        self.fixture.fail = 'upload'
        with self.assertRaisesRegex(publish.PublicationError, 'still pending'):
            self.run_release(execute=True)
        self.fixture.fail = None
        promoted = []
        def site(record, receipt):
            self.assertEqual(self.fixture.tags['latest'], record['version'])
            self.assertEqual(self.fixture.tags['next'], '0.0.1')
            self.assertIsNotNone(receipt)
            promoted.append(True)
        result = self.run_release(execute=True, promote=site, site_observed=lambda *_: bool(promoted))
        self.assertEqual(result['next'], 'complete')
        self.assertEqual(self.fixture.effects.count('upload ' + first), 2)
        self.assertEqual(self.fixture.tags['latest'], '0.1.0')
        self.assertEqual(promoted, [True])
        self.assertEqual(self.run_release(execute=True)['next'], 'promote site')

    def test_newer_channel_cannot_be_regressed_by_replayed_release(self):
        self.use_stable()
        self.fixture.tags['latest'] = '0.2.0'
        with self.assertRaisesRegex(publish.PublicationError, 'newer or equal-precedence'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.tags['latest'], '0.2.0')
        self.assertNotIn('promote channel', self.fixture.effects)

    def test_delayed_registry_completion_is_observed_before_retry(self):
        self.use_stable()
        self.ready_native()
        self.fixture.release['prerelease'] = False
        self.fixture.fail = 'publish npm'
        with self.assertRaisesRegex(publish.PublicationError, 'still pending'):
            self.run_release(execute=True)
        self.fixture.fail = None
        self.fixture.tgz = self.files[self.record['npm']['package']]
        self.fixture.package = {'name': 'twigsmith', 'version': self.record['version'],
            'dist': {'tarball': self.fixture.origin + '/twigsmith/-/' + self.record['npm']['package']}}
        self.fixture.tags['candidate-stable'] = self.record['version']
        result = self.run_release(execute=True)
        self.assertEqual(result['next'], 'promote site')
        self.assertEqual(self.fixture.effects.count('publish npm'), 1)
        self.assertEqual(self.fixture.tags['latest'], self.record['version'])
        self.assertEqual(self.fixture.tags['next'], '0.0.1')

    def test_class_and_channel_mismatches_are_rejected(self):
        for field, value in [('channel', 'latest'), ('prerelease', False), ('release_id', 0),
                             ('staging_channel', 'latest')]:
            bad = copy.deepcopy(self.approval)
            bad['publication'][field] = value
            with self.subTest(field=field), self.assertRaises(publish.PublicationError):
                publish.approved_inputs(self.archive, self.npm, bad)

    def test_default_plan_is_read_only_and_existing_selector_qualifies_source(self):
        with publish.exclusive(self.repo):
            self.assertTrue(publish.source_eligible(self.repo, self.record, self.providers, False))
            result = self.run_release()
        self.assertTrue(result['next'].startswith('upload '))
        self.assertEqual(self.fixture.effects, [])
        approval = self.root / 'approval.json'
        approval.write_text(json.dumps(self.approval))
        import os
        environment = os.environ.copy()
        environment.pop('GH_TOKEN', None)
        cli = subprocess.run(['python3', str(publish.ROOT / 'tools/release_publish.py'), str(self.archive),
            str(self.npm), '--approval', str(approval), '--fixture-api', self.fixture.origin],
            capture_output=True, text=True, env=environment)
        self.assertEqual(cli.returncode, 0, cli.stderr)
        self.assertTrue(json.loads(cli.stdout)['next'].startswith('upload '))
        self.assertEqual(self.fixture.effects, [])
        bad = copy.deepcopy(self.record)
        bad['publication']['source'] = '0'*40
        with self.assertRaisesRegex(ValueError, 'first-parent'):
            publish.source_eligible(self.repo, bad, self.providers, False)

    def test_success_real_publish_install_npx_channel_and_allowlist(self):
        result = self.run_release(execute=True)
        self.assertEqual(result['next'], 'complete')
        self.assertEqual(self.fixture.tags['next'], self.record['version'])
        self.assertEqual(set(self.fixture.assets), set(self.expected))
        self.assertEqual(release_set.digest(self.fixture.tgz), self.record['npm']['sha256'])
        receipt = result['receipt']
        self.assertEqual(receipt['identity'], self.record)
        self.assertEqual(set(receipt['consumption']), {'native', 'npm', 'npx'})
        self.assertTrue(all(v['commands'] == ['twigsmith', 'tgi'] for v in receipt['consumption'].values()))

    def test_auth_rejection_reobserves_missing_asset(self):
        first = sorted(self.expected)[0]
        self.fixture.fail = 'upload ' + first
        with self.assertRaisesRegex(publish.PublicationError, 'still pending: upload'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.assets, {})
        self.assertEqual(self.fixture.effects, ['upload ' + first])

    def test_release_lookup_paginates_and_rejects_wrong_identity_or_duplicate(self):
        self.fixture.other_releases = [
            {'id': i + 100, 'tag_name': 'unrelated-' + str(i), 'draft': False, 'prerelease': True}
            for i in range(100)]
        self.assertTrue(self.run_release()['next'].startswith('upload '))
        self.fixture.release['id'] = 2
        with self.assertRaisesRegex(publish.PublicationError, 'identity'):
            self.run_release(execute=True)
        self.fixture.release['id'] = 1
        self.fixture.release['draft'] = True
        with self.assertRaisesRegex(publish.PublicationError, 'status'):
            self.run_release(execute=True)
        self.fixture.release['draft'] = False
        self.fixture.other_releases.append(dict(self.fixture.release, id=2))
        with self.assertRaisesRegex(publish.PublicationError, 'multiple releases'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.effects, [])

    def test_partial_upload_then_resume_reuses_identical_assets(self):
        first = sorted(self.expected)[0]
        self.fixture.assets[first] = self.files[first]
        self.fixture.fail = 'upload'
        with self.assertRaisesRegex(publish.PublicationError, 'still pending'):
            self.run_release(execute=True)
        self.assertEqual(list(self.fixture.assets), [first])
        self.fixture.fail = None
        result = self.run_release(execute=True)
        self.assertEqual(result['next'], 'complete')
        self.assertNotIn('upload ' + first, self.fixture.effects)

    def test_response_lost_after_success_is_observed_without_duplicate_upload(self):
        first = sorted(self.expected)[0]
        self.fixture.timeout_after = 'upload ' + first
        self.providers.http.timeout = 0.05
        result = self.run_release(execute=True)
        self.assertEqual(result['next'], 'complete')
        self.assertEqual(self.fixture.effects.count('upload ' + first), 1)

    def test_truncated_response_after_effect_is_reconciled_without_duplicate(self):
        first = sorted(self.expected)[0]
        self.fixture.truncated_after = 'upload ' + first
        result = self.run_release(execute=True)
        self.assertEqual(result['next'], 'complete')
        self.assertEqual(self.fixture.effects.count('upload ' + first), 1)
        self.assertEqual(self.fixture.tags['next'], self.record['version'])
        self.assertEqual(self.fixture.tags['latest'], '0.0.0')

    def test_conflicting_bytes_unexpected_asset_and_npm_stop_before_effect(self):
        self.ready_native()
        name = next(iter(self.expected))
        self.fixture.assets[name] = b'conflicting bytes with dishonest metadata digest'
        with self.assertRaisesRegex(publish.PublicationError, 'conflicting release asset'):
            self.run_release(execute=True)
        self.ready_native()
        self.fixture.assets['private-record.txt'] = b'no'
        with self.assertRaisesRegex(publish.PublicationError, 'conflicting'):
            self.run_release(execute=True)
        self.ready_native()
        self.fixture.package = {'name': 'twigsmith', 'version': self.record['version'],
                                'dist': {'tarball': self.fixture.origin + '/twigsmith/-/' + self.record['npm']['package']}}
        self.fixture.tgz = b'conflicting immutable version'
        with self.assertRaisesRegex(publish.PublicationError, 'conflicting npm'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.effects, [])

    def test_unknown_state_stops_and_duplicate_invocation_is_excluded(self):
        self.fixture.unknown = True
        with self.assertRaisesRegex(publish.PublicationError, '503'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.effects, [])
        with publish.exclusive(self.repo):
            with self.assertRaisesRegex(publish.PublicationError, 'holds'):
                with publish.exclusive(self.repo):
                    self.fail('concurrent publication acquired the lock')
            duplicate = subprocess.run(['python3', '-c',
                'import sys; sys.path.insert(0, sys.argv[1]); import release_publish; '
                'release_publish.exclusive(sys.argv[2]).__enter__()', str(publish.ROOT / 'tools'), str(self.repo)],
                capture_output=True, text=True)
            self.assertNotEqual(duplicate.returncode, 0)
            self.assertIn('holds the repository lock', duplicate.stderr)

    def test_failed_npm_channel_and_site_preserve_prior_channel_until_consumption(self):
        self.ready_native()
        self.fixture.fail = 'publish npm'
        with self.assertRaisesRegex(publish.PublicationError, 'still pending'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.tags['next'], '0.0.1')
        self.fixture.fail = 'promote channel'
        with self.assertRaisesRegex(publish.PublicationError, 'still pending'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.tags['next'], '0.0.1')
        self.assertIsNotNone(self.fixture.tgz)
        self.fixture.fail = None
        called = []
        result = self.run_release(execute=True, promote=lambda *args: called.append(args))
        self.assertEqual(result['next'], 'complete')
        self.assertEqual(called, [])
        self.assertEqual(self.fixture.tags['next'], self.record['version'])

    def test_unknown_after_upload_stops_and_retry_observes_existing_bytes(self):
        first = sorted(self.expected)[0]
        self.fixture.unknown_after = 'upload ' + first
        with self.assertRaisesRegex(publish.PublicationError, '503'):
            self.run_release(execute=True)
        self.assertIn(first, self.fixture.assets)
        self.fixture.unknown = False
        self.fixture.unknown_after = None
        self.assertEqual(self.run_release(execute=True)['next'], 'complete')
        self.assertEqual(self.fixture.effects.count('upload ' + first), 1)

    def test_prerelease_never_prepares_site(self):
        called = []
        result = self.run_release(execute=True,
            promote=lambda *args: called.append(args),
            site_observed=lambda *args: called.append(args))
        self.assertEqual(result['next'], 'complete')
        self.assertEqual(called, [])

    def test_input_substitution_and_credential_destinations_rejected(self):
        bad = copy.deepcopy(self.approval)
        bad['native']['sha256'] = '0'*64
        with self.assertRaisesRegex(ValueError, 'archive differs'):
            publish.approved_inputs(self.archive, self.npm, bad)
        bad = copy.deepcopy(self.approval)
        bad['publication']['source'] = '0'*40
        with self.assertRaisesRegex(ValueError, 'source'):
            publish.approved_inputs(self.archive, self.npm, bad)
        for url in ('https://example.com', 'http://localhost:1234', 'http://127.0.0.1:1234/private'):
            with self.subTest(url=url), self.assertRaises(publish.PublicationError):
                publish.HTTP(fixture=url)
        with self.assertRaisesRegex(publish.PublicationError, 'credentials'):
            publish.HTTP(fixture=self.fixture.origin, token='must-not-leak')
        with self.assertRaisesRegex(publish.PublicationError, 'outside'):
            self.providers.http.request('https://example.com/steal', authenticated=True)


class NpmDiagnosticTests(unittest.TestCase):
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


if __name__ == '__main__':
    unittest.main()
