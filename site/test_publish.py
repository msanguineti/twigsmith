"""Exercise the read-only site's snapshot and deployment-boundary checks."""
from copy import deepcopy
import json
import hashlib
import io
from types import SimpleNamespace

from test_release_model import public_fixture
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import publish


class PublishTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.bundle = Path(cls.temporary.name) / 'bundle'
        cls.fixture = public_fixture(cls.bundle, b'\r\n')

    def test_fetch_scopes_media_type_and_credentials_to_github_api(self):
        cases = [
            (publish.GITHUB + '/releases/latest', 'application/vnd.github+json', 'Bearer test-token'),
            (publish.REGISTRY + '/latest', '*/*', None),
            (publish.REGISTRY + '/-/twigsmith-0.1.0.tgz', '*/*', None),
            ('https://github.com/msanguineti/twigsmith/releases/download/v0.1.0/native.manifest.json', '*/*', None),
        ]
        payload = b'\x00\xffprovider bytes'
        for url, accept, authorization in cases:
            with self.subTest(url=url):
                def respond(request, timeout):
                    self.assertEqual(request.full_url, url)
                    self.assertEqual(request.get_header('Accept'), accept)
                    self.assertEqual(request.get_header('Authorization'), authorization)
                    self.assertEqual(timeout, 30)
                    return io.BytesIO(payload)
                with mock.patch.dict(publish.os.environ, {'GH_TOKEN': 'test-token'}), mock.patch.object(publish, 'urlopen', side_effect=respond):
                    self.assertEqual(publish.fetch(url, len(payload)), payload)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.output = self.root / 'pages'
        self.snapshot = self.root / 'snapshot.json'
        self.state = {'source': 'a' * 40, 'model': {'version': '0.1.0'},
                      'assets': [['archive', 1, 'sha256:abc', 'https://example.test', 1, 'uploaded']]}

    def build(self, output, model):
        Path(output).mkdir()
        (Path(output) / 'index.html').write_text(model['version'])

    def test_preparation_and_recheck_fail_on_changes(self):
        with mock.patch.object(publish, 'observe', return_value=deepcopy(self.state)), mock.patch.object(publish.build, 'build_resolved', side_effect=self.build):
            publish.prepare(self.output, self.snapshot, '0.1.0')
            publish.recheck(self.snapshot)
        for key, value in [('source', 'b' * 40), ('model', {'version': '0.2.0'}), ('assets', [])]:
            changed = deepcopy(self.state)
            changed[key] = value
            with self.subTest(key=key), mock.patch.object(publish, 'observe', return_value=changed), self.assertRaises(ValueError):
                publish.recheck(self.snapshot)
        (self.output / 'index.html').write_text('modified')
        with mock.patch.object(publish, 'observe', return_value=deepcopy(self.state)), self.assertRaises(ValueError):
            publish.recheck(self.snapshot)

    def provider_fixture(self):
        descriptor, github, npm, tarball = deepcopy(self.fixture)
        directory = self.root / 'site' / 'releases'
        directory.mkdir(parents=True)
        descriptor_path = directory / '1.0.0.json'
        descriptor_path.write_text(json.dumps(descriptor))
        manifests = {path.name: path.read_bytes() for path in (self.bundle / 'native').iterdir()
                     if path.name.endswith(('.manifest.json', '.sha256'))}
        head = 'a' * 40

        def fetch(url, limit):
            if url.endswith('/commits/main'): return json.dumps({'sha': head}).encode()
            if url.endswith('/releases/latest'): return json.dumps(github).encode()
            if url.endswith('/twigsmith/latest'): return json.dumps(npm).encode()
            if url.endswith('.tgz'): return tarball
            return manifests[url.rsplit('/', 1)[-1]]
        return descriptor, github, npm, manifests, descriptor_path, head, fetch

    def test_observation_rechecks_current_main_and_provider_snapshot(self):
        descriptor, github, npm, manifests, path, head, fetch = self.provider_fixture()
        with mock.patch.object(publish, 'ROOT', self.root), mock.patch.object(publish, 'fetch', side_effect=fetch), mock.patch.object(publish.subprocess, 'run', return_value=SimpleNamespace(stdout=head)):
            state = publish.observe()
            self.assertEqual(state['model']['version'], '1.0.0')
            self.assertEqual(len(state['model']['native']), 4)
            publish.build.build_resolved(self.output, state['model'])
            page = (self.output / 'index.html').read_text()
            self.assertIn('macOS ARM64', page)
            self.assertIn('Windows x64', page)
            self.assertIn('twigsmith-install', page)
            for suffix, message in [('/twigsmith/latest', 'npm latest'), ('/commits/main', 'main changed')]:
                calls = []
                def moving(url, limit):
                    if url.endswith(suffix):
                        calls.append(url)
                        if len(calls) == 2:
                            value = {'sha': 'b' * 40} if suffix.endswith('main') else {**npm, 'version': '9.0.0'}
                            return json.dumps(value).encode()
                    return fetch(url, limit)
                with mock.patch.object(publish, 'fetch', side_effect=moving), self.assertRaisesRegex(ValueError, message):
                    publish.observe()

    def test_downloaded_manifest_hash_and_frozen_requirements_are_checked(self):
        descriptor, github, npm, manifests, path, head, fetch = self.provider_fixture()
        row = descriptor['native'][0]
        name = row['assets']['manifest']['name']
        original = manifests[name]
        manifests[name] += b'changed'
        with mock.patch.object(publish, 'ROOT', self.root), mock.patch.object(publish, 'fetch', side_effect=fetch), mock.patch.object(publish.subprocess, 'run', return_value=SimpleNamespace(stdout=head)):
            with self.assertRaisesRegex(ValueError, 'downloaded manifest differs'):
                publish.observe()
            manifest = json.loads(original)
            manifest['requirements']['native']['requirements']['system'] = 'different'
            manifests[name] = json.dumps(manifest).encode()
            digest = hashlib.sha256(manifests[name]).hexdigest()
            row['assets']['manifest']['sha256'] = digest
            identity, = [item for item in descriptor['identity']['native'] if item['target'] == row['target']]
            identity['manifest_sha256'] = digest
            checksum_name = row['assets']['checksums']['name']
            checksum = (f"{identity['sha256']}  {identity['archive']}\n"
                        f"{digest}  {name}\n").encode()
            row['assets']['checksums']['sha256'] = hashlib.sha256(checksum).hexdigest()
            manifests[checksum_name] = checksum
            for asset in github['assets']:
                for kind in ('manifest', 'checksums'):
                    if asset['name'] == row['assets'][kind]['name']:
                        asset['digest'] = 'sha256:' + row['assets'][kind]['sha256']
            path.write_text(json.dumps(descriptor))
            with self.assertRaisesRegex(ValueError, 'requirements differ'):
                publish.observe()

    def test_exact_sidecar_bytes_and_records_are_both_checked(self):
        descriptor, github, npm, files, path, head, fetch = self.provider_fixture()
        row, = [row for row in descriptor['native'] if row['target'] == 'x86_64-pc-windows-msvc']
        name = row['assets']['checksums']['name']
        original = files[name]
        self.assertIn(b'\r\n', original)
        with mock.patch.object(publish, 'ROOT', self.root), mock.patch.object(publish, 'fetch', side_effect=fetch), mock.patch.object(publish.subprocess, 'run', return_value=SimpleNamespace(stdout=head)):
            for altered in (original.replace(b'\r\n', b'\n'), original + b'tampered'):
                files[name] = altered
                with self.subTest(altered=altered), self.assertRaisesRegex(ValueError, 'downloaded checksums differ'):
                    publish.observe()
            # A descriptor/provider digest match does not validate the records:
            # they still have to name the approved archive and manifest hashes.
            files[name] = original.replace(row['assets']['archive']['sha256'].encode(), b'0' * 64)
            digest = hashlib.sha256(files[name]).hexdigest()
            row['assets']['checksums']['sha256'] = digest
            for asset in github['assets']:
                if asset['name'] == name:
                    asset['digest'] = 'sha256:' + digest
            path.write_text(json.dumps(descriptor))
            with self.assertRaisesRegex(ValueError, 'archive/manifest checksums disagree'):
                publish.observe()

    def test_sidecar_identity_changes_during_observation_are_refused(self):
        descriptor, github, npm, files, path, head, fetch = self.provider_fixture()
        name = descriptor['native'][0]['assets']['checksums']['name']
        observations = []

        def moving(url, limit):
            if url.endswith('/releases/latest'):
                observations.append(url)
                if len(observations) == 2:
                    changed = deepcopy(github)
                    for asset in changed['assets']:
                        if asset['name'] == name:
                            asset['id'] += 100
                    return json.dumps(changed).encode()
            return fetch(url, limit)

        with mock.patch.object(publish, 'ROOT', self.root), mock.patch.object(publish, 'fetch', side_effect=moving), mock.patch.object(publish.subprocess, 'run', return_value=SimpleNamespace(stdout=head)):
            with self.assertRaisesRegex(ValueError, 'release changed during provider observation'):
                publish.observe()

    def test_sidecar_bytes_are_downloaded_again_at_deployment_recheck(self):
        descriptor, github, npm, files, path, head, fetch = self.provider_fixture()
        row, = [row for row in descriptor['native'] if row['target'] == 'x86_64-pc-windows-msvc']
        name = row['assets']['checksums']['name']
        with mock.patch.object(publish, 'ROOT', self.root), mock.patch.object(publish, 'fetch', side_effect=fetch), mock.patch.object(publish.subprocess, 'run', return_value=SimpleNamespace(stdout=head)), mock.patch.object(publish.build, 'build_resolved', side_effect=self.build):
            publish.prepare(self.output, self.snapshot, '1.0.0')
            publish.recheck(self.snapshot)
            files[name] = files[name].replace(b'\r\n', b'\n')
            with self.assertRaisesRegex(ValueError, 'downloaded checksums differ'):
                publish.recheck(self.snapshot)

    def test_wrong_published_version_or_provider_error_preserves_output(self):
        self.output.mkdir()
        (self.output / 'index.html').write_text('previous')
        with mock.patch.object(publish, 'observe', return_value=deepcopy(self.state)), self.assertRaises(ValueError):
            publish.prepare(self.output, self.snapshot, '0.2.0')
        with mock.patch.object(publish, 'observe', side_effect=OSError('unavailable')), self.assertRaises(OSError):
            publish.prepare(self.output, self.snapshot)
        self.assertEqual((self.output / 'index.html').read_text(), 'previous')
        self.assertFalse(self.snapshot.exists())


if __name__ == '__main__':
    unittest.main()
