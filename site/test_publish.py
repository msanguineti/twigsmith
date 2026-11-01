"""Exercise the read-only site's snapshot and deployment-boundary checks."""
from copy import deepcopy
import json
import hashlib
import io
from types import SimpleNamespace

from test_release_model import ReleaseModelTests
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import publish


class PublishTests(unittest.TestCase):
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

    def test_observation_binds_downloaded_manifest_requirements(self):
        fixture = ReleaseModelTests()
        fixture.setUp()
        descriptor, github, npm = fixture.descriptor, fixture.github, fixture.npm
        requirements = json.loads((Path(__file__).parent / 'current-requirements.json').read_text())
        manifest = {'schema': 3, 'version': '0.1.0', 'requirements': requirements}
        data = json.dumps(manifest).encode()
        digest = hashlib.sha256(data).hexdigest()
        descriptor['native'][0]['assets']['manifest']['sha256'] = digest
        for i, asset in enumerate(github['assets']):
            asset['id'] = i + 1
            if asset['name'] == descriptor['native'][0]['assets']['manifest']['name']:
                asset['digest'] = 'sha256:' + digest
        directory = self.root / 'site' / 'releases'
        directory.mkdir(parents=True)
        (directory / '0.1.0.json').write_text(json.dumps(descriptor))
        head = 'a' * 40
        def fetch(url, limit):
            if url.endswith('/commits/main'): return json.dumps({'sha': head}).encode()
            if url.endswith('/releases/latest'): return json.dumps(github).encode()
            if url.endswith('/twigsmith/latest'): return json.dumps(npm).encode()
            if url.endswith('.tgz'): return fixture.tarball
            if url.endswith('.manifest.json'): return data
            raise AssertionError(url)
        with mock.patch.object(publish, 'ROOT', self.root), mock.patch.object(publish, 'fetch', side_effect=fetch), mock.patch.object(publish.subprocess, 'run', return_value=SimpleNamespace(stdout=head)):
            self.assertEqual(publish.observe()['model']['version'], '0.1.0')
            npm_reads = []
            def moving_npm(url, limit):
                if url.endswith('/twigsmith/latest'):
                    npm_reads.append(url)
                    if len(npm_reads) == 2:
                        changed = deepcopy(npm)
                        changed['version'] = '0.2.0'
                        return json.dumps(changed).encode()
                return fetch(url, limit)
            with mock.patch.object(publish, 'fetch', side_effect=moving_npm), self.assertRaises(ValueError):
                publish.observe()
            original_fetch = fetch
            calls = []
            def moving_main(url, limit):
                if url.endswith('/commits/main'):
                    calls.append(url)
                    if len(calls) == 2:
                        return json.dumps({'sha': 'b' * 40}).encode()
                return original_fetch(url, limit)
            with mock.patch.object(publish, 'fetch', side_effect=moving_main), self.assertRaisesRegex(ValueError, 'main changed'):
                publish.observe()
            manifest['requirements']['native']['requirements']['system'] = 'different'
            data = json.dumps(manifest).encode()
            digest = hashlib.sha256(data).hexdigest()
            descriptor['native'][0]['assets']['manifest']['sha256'] = digest
            for asset in github['assets']:
                if asset['name'] == descriptor['native'][0]['assets']['manifest']['name']:
                    asset['digest'] = 'sha256:' + digest
            (directory / '0.1.0.json').write_text(json.dumps(descriptor))
            with self.assertRaisesRegex(ValueError, 'requirements differ'):
                publish.observe()

    def test_additional_native_platform_through_provider_and_build(self):
        fixture = ReleaseModelTests()
        fixture.setUp()
        descriptor, github, npm = fixture.descriptor, fixture.github, fixture.npm
        other = deepcopy(descriptor['native'][0])
        other['platform'] = {'os': 'fixture-os', 'arch': 'arm64', 'libc': None}
        other['requirements'] = {'system': 'Fixture system', 'shell': 'Fixture shell'}
        for kind, asset in other['assets'].items():
            asset['name'] = {'archive': 'fixture.zip', 'manifest': 'fixture.manifest.json', 'checksums': 'fixture.sha256'}[kind]
        other['commands'] = []
        descriptor['native'].append(other)
        base_requirements = json.loads((Path(__file__).parent / 'current-requirements.json').read_text())
        manifests = {}
        for native in descriptor['native']:
            req = deepcopy(base_requirements)
            req['native'] = {'platform': native['platform'], 'requirements': native['requirements']}
            data = json.dumps({'schema': 3, 'version': '0.1.0', 'requirements': req}).encode()
            native['assets']['manifest']['sha256'] = hashlib.sha256(data).hexdigest()
            manifests[native['assets']['manifest']['name']] = data
        github['assets'] = []
        for native in descriptor['native']:
            for asset in native['assets'].values():
                github['assets'].append({'name': asset['name'], 'id': len(github['assets']) + 1,
                    'state': 'uploaded', 'size': 1, 'digest': 'sha256:' + asset['sha256'],
                    'browser_download_url': 'https://github.com/msanguineti/twigsmith/releases/download/v0.1.0/' + asset['name']})
        directory = self.root / 'site' / 'releases'
        directory.mkdir(parents=True)
        (directory / '0.1.0.json').write_text(json.dumps(descriptor))
        head = 'a' * 40
        def fetch(url, limit):
            if url.endswith('/commits/main'): return json.dumps({'sha': head}).encode()
            if url.endswith('/releases/latest'): return json.dumps(github).encode()
            if url.endswith('/twigsmith/latest'): return json.dumps(npm).encode()
            if url.endswith('.tgz'): return fixture.tarball
            return manifests[url.rsplit('/', 1)[-1]]
        with mock.patch.object(publish, 'ROOT', self.root), mock.patch.object(publish, 'fetch', side_effect=fetch), mock.patch.object(publish.subprocess, 'run', return_value=SimpleNamespace(stdout=head)):
            state = publish.observe()
        self.assertEqual(len(state['model']['native']), 2)
        publish.build.build_resolved(self.output, state['model'])
        self.assertIn('fixture-os / arm64', (self.output / 'index.html').read_text())
        self.assertEqual(len(state['model']['npm']['coverage']), 1)

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
