"""Full-set recovery with disposable HTTP services and real npm publication.

Synthetic PE/Mach-O/ELF bytes exercise immutable identity only. Fixture receipts
exercise the gate and deliberately cannot establish public host qualification.
"""
import base64
from copy import deepcopy
from datetime import datetime, timezone, timedelta
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import tempfile
import threading
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

import native_package
import release_platform_publish as publish
import release_providers as shared
import release_set
from test_release_set import platform_fixture


class Fixture:
    def __init__(self, record):
        self.record = record
        self.assets, self.packages, self.tarballs = {}, {}, {}
        self.tags = {name: {'latest': '0.0.0', 'next': '0.0.1'} for name in publish.PACKAGE_NAMES}
        self.effects, self.reads = [], []
        self.fail = self.lost = self.unknown_after = self.mutate_after = None
        self.unknown = False
        self.duplicate_asset = False
        self.public_missing = self.public_conflict = False
        self.tag_source = record['publication']['source']
        self.release = {'id': 1, 'tag_name': record['tag'], 'draft': False, 'prerelease': record['publication']['prerelease']}
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def reply(self, data, code=200):
                body = data if isinstance(data, bytes) else json.dumps(data).encode()
                self.send_response(code)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):
                path = urlsplit(self.path).path
                fixture.reads.append(path)
                if fixture.unknown:
                    return self.reply({}, 503)
                if '/git/ref/tags/' in path:
                    return self.reply({'object': {'type': 'commit', 'sha': fixture.tag_source}})
                if path.endswith('/releases'):
                    return self.reply([fixture.release])
                if path.endswith('/releases/1/assets'):
                    entries = [{'id': index + 10, 'name': name, 'state': 'uploaded'} for index, name in enumerate(fixture.assets)]
                    if fixture.duplicate_asset and entries:
                        entries.append(entries[0])
                    return self.reply(entries)
                if '/releases/download/' in path:
                    if fixture.public_missing:
                        return self.reply({}, 404)
                    name = path.rsplit('/', 1)[1]
                    return self.reply(b'wrong public bytes' if fixture.public_conflict else fixture.assets[name])
                if '/releases/assets/' in path:
                    name = list(fixture.assets)[int(path.rsplit('/', 1)[1]) - 10]
                    return self.reply(fixture.assets[name])
                if path.startswith('/-/package/'):
                    return self.reply(fixture.tags.get(path.split('/')[3], {}))
                parts = path.strip('/').split('/')
                name = parts[0]
                if len(parts) == 3 and parts[1] == '-':
                    return self.reply(fixture.tarballs[name]) if name in fixture.tarballs else self.reply({}, 404)
                if len(parts) == 2:
                    return self.reply(fixture.packages[name]) if name in fixture.packages else self.reply({}, 404)
                if len(parts) == 1 and name in fixture.packages:
                    return self.reply({'name': name, 'dist-tags': fixture.tags[name], 'versions': {fixture.record['version']: fixture.packages[name]}})
                return self.reply({}, 404)

            def mutate(self):
                path = urlsplit(self.path).path
                body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
                if path.endswith('/assets'):
                    label = 'upload ' + parse_qs(urlsplit(self.path).query)['name'][0]
                elif path.startswith('/-/package/'):
                    label = 'promote channel'
                else:
                    label = 'publish npm ' + path.strip('/')
                fixture.effects.append(label)
                if fixture.fail == label:
                    return self.reply({}, 403)
                if label.startswith('upload '):
                    fixture.assets[label[7:]] = body
                elif label.startswith('publish npm '):
                    name = label.removeprefix('publish npm ')
                    data = json.loads(body)
                    version, package = next(iter(data['versions'].items()))
                    fixture.packages[name] = package
                    fixture.packages[name]['dist']['tarball'] = fixture.origin + '/' + name + '/-/' + name + '-' + version + '.tgz'
                    fixture.tarballs[name] = base64.b64decode(next(iter(data['_attachments'].values()))['data'])
                    fixture.tags[name].update(data['dist-tags'])
                else:
                    fixture.tags['twigsmith'][path.rsplit('/', 1)[1]] = json.loads(body)
                if fixture.mutate_after:
                    name, tag, value = fixture.mutate_after
                    fixture.tags[name][tag] = value
                    fixture.mutate_after = None
                if fixture.unknown_after == label:
                    fixture.unknown = True
                if fixture.lost == label:
                    fixture.lost = None
                    self.connection.shutdown(socket.SHUT_RDWR)
                    self.connection.close()
                    return
                return self.reply({'ok': True}, 201)

            do_POST = do_PUT = mutate

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.origin = 'http://127.0.0.1:' + str(self.server.server_port)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


class ChecksumSnapshotTests(unittest.TestCase):
    def test_windows_checksum_keeps_exact_bytes_and_rejects_corrupt_records(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / 'candidate'
            approval, _ = platform_fixture(root, version='1.0.0-rc.1')
            approval['publication'] = {'source': approval['native'][0]['source']['revision'],
                'release_id': 1, 'channel': 'next', 'prerelease': True,
                'staging_channel': 'candidate-fixture'}
            windows = next(item for item in approval['native'] if item['target'] == 'x86_64-pc-windows-msvc')
            path = root / 'native' / (native_package.archive_stem(windows['archive']) + '.sha256')
            lf = path.read_bytes().replace(b'\r\n', b'\n')
            crlf = lf.replace(b'\n', b'\r\n')
            path.write_bytes(crlf)
            record, files, expected = publish.approved_inputs(root / 'native', root / 'npm', approval)
            self.assertEqual(path.read_bytes(), crlf)
            self.assertEqual(files[path.name], crlf)
            self.assertEqual(expected[path.name], release_set.digest(crlf))
            self.assertNotEqual(expected[path.name], release_set.digest(lf))
            fixture = Fixture(record)
            self.addCleanup(fixture.close)
            providers = publish.Providers(shared.HTTP(fixture=fixture.origin, timeout=2))
            result = publish.resume(record, files, expected, providers, execute=True, stage_only=True)
            self.assertEqual(result['next'], 'verify public consumption')
            self.assertEqual(fixture.assets[path.name], crlf)
            self.assertEqual(len(fixture.effects), 13)
            self.assertEqual(fixture.tags['twigsmith']['next'], '0.0.1')
            # An equivalent-text rewrite during observation must still fail
            # before the next attempted effect: raw approved bytes own identity.
            fixture.assets.clear()
            observe = providers.observe
            def change_after_snapshot(record, expected):
                state = observe(record, expected)
                files[path.name] = lf
                return state
            with patch.object(providers, 'observe', side_effect=change_after_snapshot), \
                 patch.object(providers, 'effect') as effect:
                with self.assertRaisesRegex(publish.PublicationError, 'native snapshot'):
                    publish.resume(record, files, expected, providers, execute=True, stage_only=True)
                effect.assert_not_called()
            files[path.name] = crlf
            corrupt = bytes([ord('1') if crlf[0] == ord('0') else ord('0')]) + crlf[1:]
            for data in (corrupt, crlf + crlf, crlf[:-2]):
                path.write_bytes(data)
                with self.subTest(data=data), self.assertRaisesRegex(ValueError, 'checksums disagree'):
                    publish.approved_inputs(root / 'native', root / 'npm', approval)
                with self.subTest(snapshot=data), self.assertRaisesRegex(ValueError, 'checksums disagree'):
                    publish.validate_snapshot(record, dict(files, **{path.name: data}),
                                              dict(expected, **{path.name: release_set.digest(data)}))


class PlatformPublicationTests(unittest.TestCase):
    VERSION = '1.0.0-rc.1'
    CHANNEL = 'next'
    PROTECTED = 'latest'
    PRERELEASE = True

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.root = Path(cls.tmp.name)
        cls.approval, _ = platform_fixture(cls.root, version=cls.VERSION)
        cls.approval['publication'] = {'source': cls.approval['native'][0]['source']['revision'],
            'release_id': 1, 'channel': cls.CHANNEL, 'prerelease': cls.PRERELEASE, 'staging_channel': 'candidate-fixture'}
        cls.base = publish.approved_inputs(cls.root / 'native', cls.root / 'npm', cls.approval)

    def setUp(self):
        self.record, self.files, self.expected = deepcopy(self.base)
        self.fixture = Fixture(self.record)
        self.original_channel = self.fixture.tags['twigsmith'][self.CHANNEL]
        self.original_protected = self.fixture.tags['twigsmith'][self.PROTECTED]
        self.addCleanup(self.fixture.close)
        self.providers = publish.Providers(shared.HTTP(fixture=self.fixture.origin, timeout=2))

    def run_release(self, **kwargs):
        return publish.resume(self.record, self.files, self.expected, self.providers, **kwargs)

    def ready_native(self):
        self.fixture.assets = {name: self.files[name] for name in self.expected}

    def ready_packages(self, omit=()):
        for item in self.record['npm']['packages']:
            if item['name'] in omit:
                continue
            name = item['name']
            self.fixture.tarballs[name] = self.files[item['filename']]
            self.fixture.packages[name] = {'name': name, 'version': self.record['version'],
                'dist': {'tarball': self.fixture.origin + '/' + name + '/-/' + item['filename']}}

    def qualification(self, record, expected, state):
        self.assertEqual(set(state['packages']), publish.PACKAGE_NAMES)
        self.assertTrue(all(value != 'absent' for value in state['packages'].values()))
        return {'fixture': True, 'identity': record, 'observed_at': datetime.now(timezone.utc).isoformat(),
                'targets': {item['target']: {'native_sha256': expected[item['archive']],
                    'payload_sha256': item['payload_sha256'],
                    'routes': {kind: ['twigsmith', 'tgi'] for kind in ('native', 'npm', 'npx')}}
                    for item in record['native']}}

    def test_default_observes_every_package_before_any_write(self):
        result = self.run_release()
        self.assertTrue(result['next'].startswith('upload '))
        self.assertEqual(self.fixture.effects, [])
        for name in publish.PACKAGE_NAMES:
            self.assertIn('/' + name + '/' + self.record['version'], self.fixture.reads)

    def test_real_npm_stages_single_package_after_twelve_public_assets_then_gate_promotes(self):
        result = self.run_release(execute=True, stage_only=True, qualify=self.qualification)
        self.assertEqual(result['next'], 'verify public consumption')
        self.assertEqual(len(self.fixture.effects), 13)
        self.assertTrue(all(step.startswith('upload ') for step in self.fixture.effects[:12]))
        self.assertEqual(self.fixture.effects[12], 'publish npm twigsmith')
        public_reads = [path for path in self.fixture.reads if '/releases/download/' in path]
        self.assertEqual({path.rsplit('/', 1)[1] for path in public_reads}, set(self.expected))
        self.assertEqual(self.fixture.tags['twigsmith'][self.CHANNEL], self.original_channel)
        self.assertEqual(self.fixture.tags['twigsmith']['candidate-fixture'], self.record['version'])
        result = self.run_release(execute=True, qualify=self.qualification)
        self.assertEqual(result['next'], 'complete')
        self.assertEqual(self.fixture.tags['twigsmith'][self.CHANNEL], self.record['version'])
        self.assertEqual(self.fixture.tags['twigsmith'][self.PROTECTED], self.original_protected)

    def test_existing_conflicting_package_blocks_missing_assets(self):
        self.ready_packages()
        self.fixture.tarballs['twigsmith'] = b'conflicting immutable bytes'
        with self.assertRaisesRegex(publish.PublicationError, 'conflicting npm'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.effects, [])

    def test_existing_exact_package_repairs_only_missing_native_assets_then_qualifies(self):
        self.ready_packages()
        retained = sorted(self.expected)[0]
        self.fixture.assets[retained] = self.files[retained]
        result = self.run_release(execute=True, qualify=self.qualification)
        self.assertEqual(result['next'], 'complete')
        self.assertNotIn('upload ' + retained, self.fixture.effects)
        self.assertNotIn('publish npm twigsmith', self.fixture.effects)
        self.assertEqual(self.fixture.effects[-1], 'promote channel')

    def test_api_assets_without_exact_public_availability_block_npm_effect(self):
        self.ready_native()
        for attribute in ('public_missing', 'public_conflict'):
            setattr(self.fixture, attribute, True)
            with self.subTest(attribute=attribute), self.assertRaises(publish.PublicationError):
                self.run_release(execute=True)
            setattr(self.fixture, attribute, False)
        self.assertEqual(self.fixture.effects, [])

    def test_lost_publish_response_observes_bytes_without_duplicate(self):
        self.ready_native()
        label = 'publish npm twigsmith'
        self.fixture.lost = label
        self.assertEqual(self.run_release(execute=True)['next'], 'verify public consumption')
        self.assertEqual(self.fixture.effects.count(label), 1)

    def test_partial_native_assets_retained_and_unknown_after_effect_stops(self):
        first = sorted(self.expected)[0]
        self.fixture.assets[first] = self.files[first]
        second = sorted(self.expected)[1]
        self.fixture.unknown_after = 'upload ' + second
        with self.assertRaisesRegex(publish.PublicationError, '503'):
            self.run_release(execute=True)
        self.assertNotIn('upload ' + first, self.fixture.effects)
        self.fixture.unknown = False
        self.fixture.unknown_after = None
        self.ready_packages()
        self.assertEqual(self.run_release(execute=True)['next'], 'verify public consumption')
        self.assertEqual(self.fixture.effects.count('upload ' + second), 1)

    def test_refused_publish_stops_without_blind_retry(self):
        self.ready_native()
        self.fixture.fail = 'publish npm twigsmith'
        with self.assertRaisesRegex(publish.PublicationError, 'still pending'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.effects, [self.fixture.fail])

    def test_other_channel_protected_during_native_and_npm_effects(self):
        self.fixture.mutate_after = ('twigsmith', self.PROTECTED, '99.0.0')
        with self.assertRaisesRegex(publish.PublicationError, 'protected advertised'):
            self.run_release(execute=True)
        self.fixture.tags['twigsmith'][self.PROTECTED] = self.original_protected
        self.ready_native()
        self.fixture.mutate_after = ('twigsmith', self.PROTECTED, '99.0.0')
        with self.assertRaisesRegex(publish.PublicationError, 'protected advertised'):
            self.run_release(execute=True)

    def test_newer_channel_blocks_before_any_effect(self):
        self.fixture.tags['twigsmith'][self.CHANNEL] = '99.0.0'
        with self.assertRaisesRegex(publish.PublicationError, 'newer or equal'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.effects, [])

    def test_bad_release_tag_duplicate_asset_and_metadata_refused(self):
        self.fixture.tag_source = 'f' * 40
        with self.assertRaisesRegex(publish.PublicationError, 'tag'):
            self.run_release(execute=True)
        self.fixture.tag_source = self.record['publication']['source']
        self.fixture.release['draft'] = True
        with self.assertRaisesRegex(publish.PublicationError, 'status'):
            self.run_release(execute=True)
        self.fixture.release['draft'] = False
        self.ready_native()
        self.fixture.duplicate_asset = True
        with self.assertRaisesRegex(publish.PublicationError, 'conflicting'):
            self.run_release(execute=True)
        self.fixture.duplicate_asset = False
        self.ready_packages()
        self.fixture.packages['twigsmith']['dist']['tarball'] = 'https://example.com/steal'
        with self.assertRaisesRegex(publish.PublicationError, 'identity'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.effects, [])

    def test_qualification_requires_fresh_complete_rows(self):
        self.ready_native()
        self.ready_packages()
        for mutation in ('missing', 'stale', 'failed', 'identity'):
            def bad(record, expected, state):
                receipt = self.qualification(record, expected, state)
                target = next(iter(receipt['targets']))
                if mutation == 'missing':
                    del receipt['targets'][target]
                elif mutation == 'stale':
                    receipt['observed_at'] = (datetime.now(timezone.utc) - timedelta(days=1)).isoformat()
                elif mutation == 'failed':
                    receipt['targets'][target]['routes']['npx'] = []
                else:
                    receipt['identity'] = {}
                return receipt
            with self.subTest(mutation=mutation), self.assertRaises(publish.PublicationError):
                self.run_release(execute=True, qualify=bad)
        self.assertEqual(self.fixture.effects, [])

    def test_approval_refuses_bad_source_class_channels_and_incomplete_set(self):
        for field, value in [('source', '0' * 40), ('release_id', True), ('prerelease', not self.PRERELEASE), ('channel', self.PROTECTED), ('staging_channel', 'next')]:
            approval = deepcopy(self.approval)
            approval['publication'][field] = value
            with self.subTest(field=field), self.assertRaises(publish.PublicationError):
                publish.approved_inputs(self.root / 'native', self.root / 'npm', approval)
        approval = deepcopy(self.approval)
        approval['npm']['packages'].pop()
        with self.assertRaises(ValueError):
            publish.validate_publication(approval)

    def test_changed_snapshot_refused_before_provider_access(self):
        name = next(iter(self.expected))
        self.files[name] = b'changed after verification'
        with self.assertRaisesRegex(publish.PublicationError, 'snapshot'):
            self.run_release(execute=True)
        self.assertEqual(self.fixture.reads, [])
        self.assertEqual(self.fixture.effects, [])

    def test_channel_change_during_qualification_blocks_promotion(self):
        self.ready_native()
        self.ready_packages()
        def concurrent(record, expected, state):
            receipt = self.qualification(record, expected, state)
            self.fixture.tags['twigsmith'][self.CHANNEL] = '0.0.2'
            return receipt
        with self.assertRaisesRegex(publish.PublicationError, 'outside approved promotion'):
            self.run_release(execute=True, qualify=concurrent)
        self.assertEqual(self.fixture.effects, [])

    def test_failed_promotion_preserves_bytes_and_retry_requalifies(self):
        self.ready_native()
        self.ready_packages()
        self.fixture.fail = 'promote channel'
        with self.assertRaisesRegex(publish.PublicationError, 'still pending'):
            self.run_release(execute=True, qualify=self.qualification)
        self.assertEqual(self.fixture.tags['twigsmith'][self.CHANNEL], self.original_channel)
        self.fixture.fail = None
        self.assertEqual(self.run_release(execute=True)['next'], 'verify public consumption')
        self.assertEqual(self.run_release(execute=True, qualify=self.qualification)['next'], 'complete')
        self.assertEqual(self.fixture.effects, ['promote channel', 'promote channel'])

    def test_live_execution_cannot_use_fixture_receipt_or_naked_effect(self):
        provider = publish.Providers(shared.HTTP())
        with self.assertRaisesRegex(publish.PublicationError, 'hosted revalidation'):
            publish.resume(self.record, self.files, self.expected, provider, execute=True, qualify=self.qualification)
        with self.assertRaisesRegex(publish.PublicationError, 'hosted revalidation'):
            provider.effect('promote channel', self.record, self.files)
        receipt = self.qualification(self.record, self.expected, {'packages': {name: 'x' for name in publish.PACKAGE_NAMES}})
        with self.assertRaisesRegex(publish.PublicationError, 'authenticated'):
            publish.public_qualification(self.record, receipt)


class StablePublicationTests(PlatformPublicationTests):
    VERSION = '1.0.0'
    CHANNEL = 'latest'
    PROTECTED = 'next'
    PRERELEASE = False

    def test_unrelated_tag_creation_change_and_removal_stop_before_next_effect(self):
        for value in ('0.0.2', None):
            self.fixture.tags['twigsmith']['legacy'] = '0.0.1'
            observe = self.providers.observe
            calls = 0
            def concurrent(record, expected):
                nonlocal calls
                state = observe(record, expected)
                calls += 1
                if calls == 1:
                    if value is None:
                        self.fixture.tags['twigsmith'].pop('legacy')
                    else:
                        self.fixture.tags['twigsmith']['legacy'] = value
                return state
            with patch.object(self.providers, 'observe', side_effect=concurrent):
                with self.subTest(value=value), self.assertRaisesRegex(publish.PublicationError, 'protected advertised'):
                    self.run_release(execute=True)
            self.fixture.assets.clear()
        self.fixture.mutate_after = ('twigsmith', 'new-tag', '0.0.1')
        with self.assertRaisesRegex(publish.PublicationError, 'protected advertised'):
            self.run_release(execute=True)

    def test_staging_tag_can_change_only_after_approved_package_publication(self):
        staging = self.record['publication']['staging_channel']
        self.fixture.mutate_after = ('twigsmith', staging, self.VERSION)
        with self.assertRaisesRegex(publish.PublicationError, 'staging npm channel changed'):
            self.run_release(execute=True)

    def test_lost_latest_response_requalifies_without_republishing_or_repromoting(self):
        self.ready_native()
        self.ready_packages()
        self.fixture.lost = 'promote channel'
        self.assertEqual(self.run_release(execute=True, qualify=self.qualification)['next'], 'complete')
        self.assertEqual(self.fixture.effects, ['promote channel'])
        self.assertEqual(self.fixture.tags['twigsmith']['next'], self.original_protected)
        self.assertEqual(self.run_release(execute=True)['next'], 'verify public consumption')
        with patch.object(self, 'qualification', wraps=self.qualification) as qualify:
            self.assertEqual(self.run_release(execute=True, qualify=qualify)['next'], 'complete')
            qualify.assert_called_once()
        self.assertEqual(self.fixture.effects, ['promote channel'])


class PublicGateTests(unittest.TestCase):
    """Structural receipt checks do not authenticate report origins by themselves."""
    VERSION = '1.0.0-rc.1'
    CHANNEL = 'next'
    PRERELEASE = True

    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.approved, _ = platform_fixture(Path(cls.temporary.name), version=cls.VERSION)
        cls.approved['publication'] = {'source': cls.approved['native'][0]['source']['revision'],
            'release_id': 1, 'channel': cls.CHANNEL, 'prerelease': cls.PRERELEASE, 'staging_channel': 'candidate-fixture'}

    def setUp(self):
        from qualify_public_release import expected_cases, expected_identity
        self.record = deepcopy(self.approved)
        self.producer = {'publisher_revision': 'a' * 40, 'run_id': '10', 'run_attempt': '2'}
        self.environment = {'GITHUB_SHA': 'a' * 40, 'GITHUB_RUN_ID': '10', 'GITHUB_RUN_ATTEMPT': '2'}
        self.receipt = {'identity': self.record, 'targets': {}}
        digest = release_set.digest(json.dumps(self.record, sort_keys=True, separators=(',', ':')).encode())
        for target in release_set.TARGETS:
            kinds = ['native', 'npm_local', 'npm_global', 'npx_twigsmith', 'npx_tgi']
            if native_package.target_metadata(target)['os'] != 'linux':
                kinds.append('browser')
            self.receipt['targets'][target] = {'schema': 1, 'approval_sha256': digest,
                'target': target, 'observed_at': datetime.now(timezone.utc).isoformat(),
                'producer': self.producer, 'passed': True,
                'checks': {kind: {'passed': True,
                    'report': kind + '/' + ('installation.json' if kind == 'browser' else 'qualification.json'),
                    'cases': [{'suite': suite, 'name': name, 'passed': True, 'exit': 0}
                              for suite, names in expected_cases(kind).items() for name in names],
                    'identity': expected_identity(self.record, target, kind),
                    'aliases': {name: {'passed': True, 'exit': 0, 'version': self.record['version']} for name in ('twigsmith', 'tgi')},
                    **({'cold_cache': True, 'invoked_alias': kind[4:]} if kind.startswith('npx_') else {})}
                    for kind in kinds}}

    def check(self, receipt=None):
        with patch.dict(os.environ, self.environment):
            return publish.public_qualification(self.record, self.receipt if receipt is None else receipt)

    def test_exact_current_run_identity_and_all_rows_required(self):
        self.assertEqual(self.check(), self.receipt)
        target = next(iter(self.receipt['targets']))
        for mutation in ('missing', 'skipped', 'stale', 'producer', 'approval', 'unsafe_report', 'missing_npx'):
            receipt = deepcopy(self.receipt)
            row = receipt['targets'][target]
            if mutation == 'missing':
                del receipt['targets'][target]
            elif mutation == 'skipped':
                row['checks']['native']['passed'] = False
            elif mutation == 'stale':
                row['observed_at'] = (datetime.now(timezone.utc) - timedelta(hours=2)).isoformat()
            elif mutation == 'producer':
                row['producer']['run_attempt'] = '1'
            elif mutation == 'approval':
                row['approval_sha256'] = '0' * 64
            elif mutation == 'unsafe_report':
                row['checks']['native']['report'] = '../other.json'
            else:
                del row['checks']['npx_tgi']
            with self.subTest(mutation=mutation), self.assertRaises(publish.PublicationError):
                self.check(receipt)


class StablePublicGateTests(PublicGateTests):
    VERSION = '1.0.0'
    CHANNEL = 'latest'
    PRERELEASE = False


if __name__ == '__main__':
    unittest.main()
