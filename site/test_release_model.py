"""Public release data must agree before it can describe a stable site.

Native fixture bytes are synthetic and do not qualify any host.
"""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest

import candidate
import release_model
from test_release_set import platform_fixture


def public_fixture(root):
    """Construct a current-format stable probe, never an actual published release."""
    root = Path(root)
    approval, _ = platform_fixture(root, version='1.0.0')
    (root / 'identity.json').write_text(json.dumps(approval))
    files = {path.name: path.read_bytes()
             for directory in ('native', 'npm') for path in (root / directory).iterdir()}
    descriptor = candidate.descriptor_for(approval, files)
    package, = descriptor['npm']['packages']
    tarball = (root / 'npm' / package['filename']).read_bytes()
    github = {'id': 123, 'draft': False, 'prerelease': False,
              'published_at': '2026-11-08T00:00:00Z', 'tag_name': 'v1.0.0',
              'html_url': 'https://github.com/msanguineti/twigsmith/releases/tag/v1.0.0',
              'assets': []}
    for native in descriptor['native']:
        for asset in native['assets'].values():
            github['assets'].append({'name': asset['name'], 'id': len(github['assets']) + 1,
                'state': 'uploaded', 'size': 100, 'digest': 'sha256:' + asset['sha256'],
                'browser_download_url': 'https://github.com/msanguineti/twigsmith/releases/download/v1.0.0/' + asset['name']})
    npm = {'name': 'twigsmith', 'version': '1.0.0',
           'dist': {'integrity': package['integrity'],
                    'tarball': 'https://registry.npmjs.org/twigsmith/-/twigsmith-1.0.0.tgz'}}
    return descriptor, github, npm, tarball


class ReleaseModelTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.fixture = public_fixture(Path(cls.temporary.name) / 'bundle')

    def setUp(self):
        self.descriptor, self.github, self.npm, self.tarball = deepcopy(self.fixture)

    def test_resolves_exact_stable_single_installer_and_four_platforms(self):
        model = release_model.resolve(self.descriptor, self.github, self.npm, self.tarball)
        self.assertEqual(model['release_id'], 123)
        self.assertEqual(model['availability'], 'published')
        self.assertEqual(len(model['native']), 4)
        self.assertEqual(model['npm']['commands']['install'][0], 'npm install --global twigsmith@1.0.0')
        self.assertIn('--offline', model['npm']['commands']['repair'][1])
        for native in model['native']:
            self.assertNotIn('BUNDLE', '\n'.join(native['commands']))
            self.assertNotIn('python', '\n'.join(native['commands']))
            self.assertTrue(native['assets']['archive']['url'].endswith(native['assets']['archive']['name']))
            self.assertNotIn('url', self.descriptor['native'][0]['assets']['archive'])
            if native['platform']['os'] == 'windows':
                self.assertIn('Get-FileHash', native['commands'][0])
                self.assertIn('Expand-Archive', native['commands'][1])
            elif native['platform']['os'] == 'macos':
                self.assertTrue(native['commands'][0].startswith('shasum -a 256 --check '))
            else:
                self.assertTrue(native['commands'][0].startswith('sha256sum --check '))

    def test_historical_and_optional_package_topologies_are_refused(self):
        for schema in (1, 2):
            with self.subTest(schema=schema), self.assertRaisesRegex(ValueError, 'schema 3 single-package installer'):
                release_model.resolve({'schema': schema}, None, None, None)

    def test_prerelease_is_refused_before_observing_providers(self):
        descriptor = deepcopy(self.descriptor)
        descriptor['identity']['version'] = '1.0.0-rc.1'
        # The reviewed identity has independent internal validation, so use the
        # original valid identity with its version changed only after that check.
        from unittest import mock
        with mock.patch.object(release_model.release_set, 'validate_platform_approval', return_value=descriptor['identity']):
            with self.assertRaisesRegex(ValueError, 'prereleases cannot'):
                release_model.resolve(descriptor, None, None, None)

    def test_provider_disagreement_missing_assets_and_registry_dependencies_are_refused(self):
        cases = [lambda gh, npm: npm.update(version='9.0.0'),
                 lambda gh, npm: gh.update(tag_name='v9.0.0'),
                 lambda gh, npm: gh.update(prerelease=True),
                 lambda gh, npm: gh.update(draft=True),
                 lambda gh, npm: gh['assets'].pop(),
                 lambda gh, npm: gh['assets'][0].update(state='new'),
                 lambda gh, npm: gh['assets'][0].update(digest='sha256:' + '0' * 64),
                 lambda gh, npm: gh['assets'][0].update(browser_download_url='https://example.com/fake'),
                 lambda gh, npm: gh['assets'].append(deepcopy(gh['assets'][0])),
                 lambda gh, npm: npm['dist'].update(integrity='sha512-different'),
                 lambda gh, npm: npm['dist'].update(tarball='https://example.com/fake'),
                 lambda gh, npm: npm.update(os=['linux']),
                 lambda gh, npm: npm.update(optionalDependencies={'obsolete-payload': '1.0.0'}),
                 lambda gh, npm: npm.pop('dist')]
        for change in cases:
            gh, npm = deepcopy(self.github), deepcopy(self.npm)
            change(gh, npm)
            with self.subTest(change=change), self.assertRaises(ValueError):
                release_model.resolve(self.descriptor, gh, npm, self.tarball)
        with self.assertRaises(ValueError):
            release_model.resolve(self.descriptor, self.github, self.npm, self.tarball + b'changed')

    def test_descriptor_cannot_substitute_requirements_identity_or_asset_hashes(self):
        for mutate in (lambda d: d.update(version='9.0.0'),
                       lambda d: d['native'].pop(),
                       lambda d: d['native'][0]['requirements'].update(system='different'),
                       lambda d: d['native'][0]['assets']['archive'].update(name='../archive'),
                       lambda d: d['native'][0]['assets']['archive'].update(sha256='0' * 64),
                       lambda d: d['npm']['packages'][0].update(sha256='0' * 64),
                       lambda d: d['npm']['requirements'][d['native'][0]['target']].update(requirements='different')):
            descriptor = deepcopy(self.descriptor)
            mutate(descriptor)
            with self.subTest(mutate=mutate), self.assertRaises(ValueError):
                release_model.resolve(descriptor, self.github, self.npm, self.tarball)


if __name__ == '__main__':
    unittest.main()
