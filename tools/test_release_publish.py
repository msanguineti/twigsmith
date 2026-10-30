"""Disposable HTTP providers + real npm exercise publication effects and recovery.

No endpoint here is a live GitHub/npm service. C compiles an inert version fixture;
real native qualification remains separate from these publication-boundary tests.
"""
import base64
import copy
import importlib.util
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import socket
import time
import subprocess
import tempfile
import threading
import unittest
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
        self.tags = {'beta': '0.0.1'}
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
        cls.approval['publication'] = {'source': cls.source, 'channel': 'beta',
                                     'staging_channel': 'candidate-beta-1', 'prerelease': True}

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def setUp(self):
        self.fixture = Fixture(self.source)
        self.addCleanup(self.fixture.close)
        self.providers = publish.Providers(publish.HTTP(fixture=self.fixture.origin, timeout=1))
        self.record, self.files, self.expected = publish.approved_inputs(self.archive, self.npm, self.approval)

    def run_release(self, **kwargs):
        return publish.resume(self.record, self.files, self.expected, self.providers, **kwargs)

    def ready_native(self):
        self.fixture.release = {'id': 1, 'tag_name': self.record['tag'], 'draft': False, 'prerelease': True}
        self.fixture.assets = {name: self.files[name] for name in self.expected}

    def test_default_plan_is_read_only_and_existing_selector_qualifies_source(self):
        with publish.exclusive(self.repo):
            self.assertTrue(publish.source_eligible(self.repo, self.record, self.providers, False))
            result = self.run_release()
        self.assertEqual(result['next'], 'create draft')
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
        self.assertEqual(json.loads(cli.stdout)['next'], 'create draft')
        self.assertEqual(self.fixture.effects, [])
        bad = copy.deepcopy(self.record)
        bad['publication']['source'] = '0'*40
        with self.assertRaisesRegex(ValueError, 'first-parent'):
            publish.source_eligible(self.repo, bad, self.providers, False)

    def test_success_real_publish_install_npx_channel_and_allowlist(self):
        result = self.run_release(execute=True)
        self.assertEqual(result['next'], 'promote site')
        self.assertEqual(self.fixture.tags['beta'], self.record['version'])
        self.assertEqual(set(self.fixture.assets), set(self.expected))
        self.assertEqual(release_set.digest(self.fixture.tgz), self.record['npm']['sha256'])
        receipt = result['receipt']
        self.assertEqual(receipt['identity'], self.record)
        self.assertEqual(set(receipt['consumption']), {'native', 'npm', 'npx'})
        self.assertTrue(all(v['commands'] == ['twigsmith', 'tgi'] for v in receipt['consumption'].values()))

    def test_auth_rejection_reobserves_and_retains_remote_absence(self):
        self.fixture.fail = 'create draft'
        with self.assertRaisesRegex(publish.PublicationError, 'still pending'):
            self.run_release(execute=True)
        self.assertIsNone(self.fixture.release)
        self.assertEqual(self.fixture.effects, ['create draft'])

    def test_draft_lookup_paginates_listing_and_rejects_duplicate_tag(self):
        self.fixture.release = {'id': 1, 'tag_name': self.record['tag'], 'draft': True, 'prerelease': True}
        self.fixture.other_releases = [
            {'id': i + 100, 'tag_name': 'unrelated-' + str(i), 'draft': False, 'prerelease': True}
            for i in range(100)]
        # The published-tag endpoint intentionally cannot expose this draft.
        self.assertIsNone(self.providers.http.request(
            self.providers.api + '/releases/tags/' + self.record['tag'], absent=True))
        self.assertTrue(self.run_release()['next'].startswith('upload '))
        self.assertEqual(self.providers.release_id, 1)
        self.assertEqual(self.fixture.effects, [])
        self.fixture.other_releases.append(dict(self.fixture.release, id=2))
        with self.assertRaisesRegex(publish.PublicationError, 'multiple releases'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.effects, [])

    def test_partial_upload_then_resume_reuses_identical_assets(self):
        self.fixture.release = {'id': 1, 'tag_name': self.record['tag'], 'draft': True, 'prerelease': True}
        first = sorted(self.expected)[0]
        self.fixture.assets[first] = self.files[first]
        self.fixture.fail = 'upload'
        with self.assertRaisesRegex(publish.PublicationError, 'still pending'):
            self.run_release(execute=True)
        self.assertEqual(list(self.fixture.assets), [first])
        self.fixture.fail = None
        result = self.run_release(execute=True)
        self.assertEqual(result['next'], 'promote site')
        self.assertNotIn('upload ' + first, self.fixture.effects)

    def test_response_lost_after_success_is_observed_without_duplicate_upload(self):
        first = sorted(self.expected)[0]
        self.fixture.timeout_after = 'upload ' + first
        self.providers.http.timeout = 0.05
        result = self.run_release(execute=True)
        self.assertEqual(result['next'], 'promote site')
        self.assertEqual(self.fixture.effects.count('upload ' + first), 1)

    def test_truncated_response_after_effect_is_reconciled_without_duplicate(self):
        self.fixture.truncated_after = 'create draft'
        result = self.run_release(execute=True)
        self.assertEqual(result['next'], 'promote site')
        self.assertEqual(self.fixture.effects.count('create draft'), 1)
        self.assertEqual(self.fixture.release['draft'], False)
        self.assertEqual(self.fixture.tags['beta'], self.record['version'])

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
        self.assertEqual(self.fixture.tags['beta'], '0.0.1')
        self.fixture.fail = 'promote channel'
        with self.assertRaisesRegex(publish.PublicationError, 'still pending'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.tags['beta'], '0.0.1')
        self.assertIsNotNone(self.fixture.tgz)
        self.fixture.fail = None
        def fail_site(record, receipt):
            raise OSError('fixture promotion failed')
        with self.assertRaisesRegex(publish.PublicationError, 'still pending: promote site'):
            self.run_release(execute=True, promote=fail_site)
        self.assertEqual(self.fixture.tags['beta'], self.record['version'])

    def test_unknown_after_effect_stops_and_retry_reobserves_existing_draft(self):
        self.fixture.unknown_after = 'create draft'
        with self.assertRaisesRegex(publish.PublicationError, '503'):
            self.run_release(execute=True)
        self.assertIsNotNone(self.fixture.release)
        self.assertEqual(self.fixture.effects, ['create draft'])
        self.fixture.unknown = False
        self.fixture.unknown_after = None
        self.assertTrue(self.run_release()['next'].startswith('upload '))

    def test_matching_site_artifact_is_prepared_only_after_fresh_consumption(self):
        module_spec = importlib.util.spec_from_file_location('publication_site', publish.ROOT / 'site/build.py')
        site = importlib.util.module_from_spec(module_spec)
        module_spec.loader.exec_module(site)
        output = self.root / 'published-site'
        result = self.run_release(execute=True,
            promote=lambda record, receipt: site.build(output, record, receipt=receipt),
            site_observed=lambda record, receipt: site.promotion_matches(output, record, receipt))
        self.assertEqual(result['next'], 'complete')
        self.assertTrue(site.promotion_matches(output, self.record, result['receipt']))
        self.assertIn('Version 0.1.0-beta.1', (output / 'index.html').read_text())

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


if __name__ == '__main__':
    unittest.main()
