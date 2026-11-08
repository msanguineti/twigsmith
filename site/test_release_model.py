"""Public release data must agree before it can describe a deployed site."""
from copy import deepcopy
import json
import base64
import hashlib
from pathlib import Path
import unittest

import release_model


class ReleaseModelTests(unittest.TestCase):
    def setUp(self):
        self.descriptor = json.loads((Path(__file__).parent / 'releases/0.1.0.json').read_text())
        self.tarball = b'fixture package bytes'
        self.descriptor['npm']['sha256'] = hashlib.sha256(self.tarball).hexdigest()
        self.descriptor['npm']['integrity'] = 'sha512-' + base64.b64encode(hashlib.sha512(self.tarball).digest()).decode()
        self.github = {
            'id': 429284142, 'draft': False, 'prerelease': False,
            'published_at': '2026-10-30T17:01:40Z', 'tag_name': 'v0.1.0',
            'html_url': 'https://github.com/msanguineti/twigsmith/releases/tag/v0.1.0',
            'assets': []}
        for asset in self.descriptor['native'][0]['assets'].values():
            self.github['assets'].append({
                'name': asset['name'], 'state': 'uploaded', 'size': 100,
                'digest': 'sha256:' + asset['sha256'],
                'browser_download_url': 'https://github.com/msanguineti/twigsmith/releases/download/v0.1.0/' + asset['name']})
        self.npm = {'name': 'twigsmith', 'version': '0.1.0', 'os': ['linux'],
                    'cpu': ['x64'], 'libc': ['glibc'],
                    'dist': {'integrity': self.descriptor['npm']['integrity'], 'tarball': 'https://registry.npmjs.org/twigsmith/-/twigsmith-0.1.0.tgz'}}

    def test_resolves_published_identity_and_explicit_destinations(self):
        model = release_model.resolve(self.descriptor, self.github, self.npm, self.tarball)
        self.assertEqual(model['release_id'], 429284142)
        self.assertEqual(model['npm']['commands']['install'][0], 'npm install --global twigsmith@0.1.0')
        self.assertEqual(model['native'][0]['assets']['archive']['url'],
                         'https://github.com/msanguineti/twigsmith/releases/download/v0.1.0/'
                         'twigsmith-0.1.0-x86_64-unknown-linux-gnu.tar.gz')
        self.assertNotIn('url', self.descriptor['native'][0]['assets']['archive'])

    def test_complete_set_format_refuses_public_resolution_before_provider_fields(self):
        for schema in (2, 3):
            with self.subTest(schema=schema), self.assertRaisesRegex(ValueError, 'full-set publication is unavailable'):
                release_model.resolve({'schema': schema}, None, None, None)

    def test_rejects_provider_disagreement_incomplete_or_changed_assets(self):
        cases = [
            lambda gh, npm: npm.update(version='0.2.0'),
            lambda gh, npm: gh.update(tag_name='v0.2.0'),
            lambda gh, npm: gh.update(prerelease=True),
            lambda gh, npm: gh.update(draft=True),
            lambda gh, npm: gh['assets'].pop(),
            lambda gh, npm: gh['assets'][0].update(state='new'),
            lambda gh, npm: gh['assets'][0].update(digest='sha256:' + '0' * 64),
            lambda gh, npm: gh['assets'][0].update(browser_download_url='https://example.com/other'),
            lambda gh, npm: gh['assets'].append(deepcopy(gh['assets'][0])),
            lambda gh, npm: npm['dist'].update(integrity='sha512-different'),
            lambda gh, npm: npm['dist'].update(tarball='https://example.com/fake'),
            lambda gh, npm: npm.update(cpu=['arm64']),
            lambda gh, npm: npm.pop('dist')]
        for change in cases:
            gh, npm = deepcopy(self.github), deepcopy(self.npm)
            change(gh, npm)
            with self.subTest(change=change), self.assertRaises(ValueError):
                release_model.resolve(self.descriptor, gh, npm, self.tarball)

    def test_rejects_bad_release_or_commands(self):
        for mutate in (lambda d: d.update(version='0.1.0-rc1'),
                       lambda d: d['npm'].update(url='https://example.com'),
                       lambda d: d['npm']['commands'].update(install=['npm install twigsmith@9.0.0']),
                       lambda d: d['npm']['commands']['install'].append('npm install twigsmith@9.0.0'),
                       lambda d: d['npm']['commands'].update(install=['echo twigsmith@0.1.0']),
                       lambda d: d['npm']['commands'].update(npx=['echo twigsmith@0.1.0']),
                       lambda d: d['native'][0]['commands'].__setitem__(0, 'echo ' + d['native'][0]['assets']['checksums']['name']),
                       lambda d: d['native'][0]['commands'].__setitem__(1, 'echo ' + d['native'][0]['assets']['archive']['name']),
                       lambda d: d['native'][0]['commands'].append('curl https://example.com'),
                       lambda d: d['native'][0]['assets']['archive'].update(name='../archive'),
                       lambda d: d['native'][0].update(commands=['echo nothing'])):
            descriptor = deepcopy(self.descriptor)
            mutate(descriptor)
            with self.subTest(mutate=mutate), self.assertRaises(ValueError):
                release_model.resolve(descriptor, self.github, self.npm, self.tarball)

    def test_independent_native_and_npm_platform_coverage(self):
        descriptor, gh = deepcopy(self.descriptor), deepcopy(self.github)
        other = deepcopy(descriptor['native'][0])
        other['platform'] = {'os': 'future', 'arch': 'arm64', 'libc': None}
        other['requirements'] = {'system': 'Example fixture platform'}
        for kind, asset in other['assets'].items():
            asset['name'] = {'archive': 'fixture-platform.tar.gz', 'manifest': 'fixture-manifest.json', 'checksums': 'fixture-checksums.sha256'}[kind]
            gh['assets'].append({'name': asset['name'], 'state': 'uploaded', 'size': 1,
                                 'digest': 'sha256:' + asset['sha256'],
                                 'browser_download_url': 'https://github.com/msanguineti/twigsmith/releases/download/v0.1.0/' + asset['name']})
        other['commands'] = ['sha256sum --check fixture-checksums.sha256', 'tar -xzf fixture-platform.tar.gz',
                             'export PATH="$PWD/fixture-platform:$PATH"', 'twigsmith --version', 'tgi --help']
        descriptor['native'].append(other)
        model = release_model.resolve(descriptor, gh, self.npm, self.tarball)
        self.assertEqual(len(model['native']), 2)
        self.assertEqual(len(model['npm']['coverage']), 1)
        self.assertEqual(model['native'][1]['assets']['archive']['name'], 'fixture-platform.tar.gz')

        # A future platform can specify a different asset format without adopting
        # installation instructions or claiming the platform is qualified today.
        other['assets']['archive']['name'] = 'fixture-platform.zip'
        for asset in gh['assets']:
            if asset['name'] == 'fixture-platform.tar.gz':
                asset['name'] = 'fixture-platform.zip'
                asset['browser_download_url'] = asset['browser_download_url'].replace('fixture-platform.tar.gz', 'fixture-platform.zip')
        other['commands'] = []
        model = release_model.resolve(descriptor, gh, self.npm, self.tarball)
        self.assertEqual(model['native'][1]['commands'], [])
        self.assertEqual(model['native'][1]['assets']['archive']['name'], 'fixture-platform.zip')
        # The qualified Linux download must still have validated instructions.
        descriptor['native'][0]['commands'] = []
        with self.assertRaisesRegex(ValueError, 'invalid native commands'):
            release_model.resolve(descriptor, gh, self.npm, self.tarball)


if __name__ == '__main__':
    unittest.main()
