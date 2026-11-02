"""Descriptor integrity with synthetic candidates; these fixtures qualify no hosts."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest import mock

import candidate
import build as site_build
from test_release_set import platform_fixture


class CandidateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / 'descriptor.json'
        self.version = '0.2.0'
        requirements = candidate.native_package.load_requirements(Path(__file__).resolve().parents[1])
        manifest = json.dumps({'schema': 3, 'version': self.version, 'requirements': requirements}).encode()
        self.files = {'twigsmith-0.2.0-x86_64-unknown-linux-gnu.tar.gz': b'archive',
                      'twigsmith-0.2.0-x86_64-unknown-linux-gnu.manifest.json': manifest,
                      'twigsmith-0.2.0-x86_64-unknown-linux-gnu.sha256': b'checksum',
                      'twigsmith-0.2.0.tgz': b'npm tarball'}
        sha = lambda b: hashlib.sha256(b).hexdigest()
        self.files['twigsmith-0.2.0-x86_64-unknown-linux-gnu.sha256'] = (
            f"{sha(b'archive')}  twigsmith-0.2.0-x86_64-unknown-linux-gnu.tar.gz\n"
            f"{sha(manifest)}  twigsmith-0.2.0-x86_64-unknown-linux-gnu.manifest.json\n").encode()
        self.record = {'version': self.version, 'tag': 'v' + self.version,
                       'native': {'archive': 'twigsmith-0.2.0-x86_64-unknown-linux-gnu.tar.gz',
                                  'sha256': sha(b'archive'), 'manifest_sha256': sha(manifest)},
                       'npm': {'package': 'twigsmith-0.2.0.tgz', 'sha256': sha(b'npm tarball')}}

    def test_reviewed_descriptor_binds_candidate_and_requirements(self):
        descriptor = candidate.descriptor_for(self.record, self.files)
        self.path.write_text(json.dumps(descriptor))
        candidate.verify_descriptor(self.record, self.files, self.path)
        for mutate in (lambda d: d['native'][0]['requirements'].update(system='different'),
                       lambda d: d['npm'].update(sha256='0' * 64),
                       lambda d: d['native'][0]['assets']['checksums'].update(name='other.sha256'),
                       lambda d: d['npm']['commands'].update(install=['echo twigsmith@0.2.0'])):
            changed = deepcopy(descriptor)
            mutate(changed)
            self.path.write_text(json.dumps(changed))
            with self.subTest(mutate=mutate), self.assertRaisesRegex(ValueError, 'differs'):
                candidate.verify_descriptor(self.record, self.files, self.path)
        self.path.unlink()
        with self.assertRaises(FileNotFoundError):
            candidate.verify_descriptor(self.record, self.files, self.path)

    def test_changed_candidate_or_unknown_historical_manifest_is_rejected(self):
        self.files['twigsmith-0.2.0.tgz'] = b'other'
        with self.assertRaisesRegex(ValueError, 'bytes differ'):
            candidate.descriptor_for(self.record, self.files)
        self.files['twigsmith-0.2.0.tgz'] = b'npm tarball'
        name = 'twigsmith-0.2.0-x86_64-unknown-linux-gnu.manifest.json'
        self.files[name] = json.dumps({'schema': 2, 'version': self.version}).encode()
        self.record['native']['manifest_sha256'] = hashlib.sha256(self.files[name]).hexdigest()
        with self.assertRaisesRegex(ValueError, 'historical'):
            candidate.descriptor_for(self.record, self.files)


class PlatformCandidateTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.base = Path(cls.temporary.name) / 'fixture'
        cls.approval, _ = platform_fixture(cls.base)
        (cls.base / 'identity.json').write_text(json.dumps(cls.approval))

    def setUp(self):
        self.temporary_case = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary_case.cleanup)
        self.root = Path(self.temporary_case.name)
        self.bundle = self.root / 'bundle'
        self.bundle.mkdir()
        for directory in ('native', 'npm'):
            shutil.copytree(self.base / directory, self.bundle / directory)
        shutil.copy2(self.base / 'identity.json', self.bundle / 'identity.json')
        self.files = {path.name: path.read_bytes()
                      for directory in ('native', 'npm') for path in (self.bundle / directory).iterdir()}

    def test_complete_descriptor_preserves_identity_requirements_and_all_integrities(self):
        descriptor = candidate.from_bundle(self.bundle)
        self.assertEqual(descriptor, candidate.descriptor_for(self.approval, self.files))
        self.assertEqual(descriptor['schema'], 2)
        self.assertEqual(descriptor['identity'], self.approval)
        self.assertEqual(descriptor['availability'], 'unpublished')
        self.assertEqual(len(descriptor['native']), 4)
        self.assertEqual(len(descriptor['npm']['packages']), 5)
        self.assertEqual(len(descriptor['npm']['coverage']), 4)
        for native in descriptor['native']:
            snapshot = self.approval['requirements'][native['target']]
            self.assertEqual(native['requirements'], snapshot['native']['requirements'])
            self.assertEqual(descriptor['npm']['requirements'][native['target']], snapshot['npm'])
            self.assertEqual(len(native['assets']), 3)
        for package in descriptor['npm']['packages']:
            data = self.files[package['filename']]
            integrity = 'sha512-' + candidate.base64.b64encode(hashlib.sha512(data).digest()).decode()
            self.assertEqual(package['integrity'], integrity)
            self.assertEqual(package['sha256'], hashlib.sha256(data).hexdigest())
            self.assertEqual(package['version'], self.approval['version'])
        self.assertNotIn('url', descriptor['npm'])
        self.assertNotIn('commands', descriptor['npm'])

    def test_missing_changed_and_unreviewed_snapshots_refused(self):
        changed = dict(self.files)
        changed.pop(self.approval['npm']['packages'][0]['filename'])
        with self.assertRaisesRegex(ValueError, 'complete'):
            candidate.descriptor_for(self.approval, changed)
        for filename in [self.approval['native'][0]['archive'], self.approval['npm']['packages'][0]['filename'],
                         'manifest.json']:
            changed = dict(self.files)
            changed[filename] += b'tampered'
            with self.subTest(filename=filename), self.assertRaises(ValueError):
                candidate.descriptor_for(self.approval, changed)
        path = self.root / 'reviewed.json'
        descriptor = candidate.from_bundle(self.bundle)
        descriptor['npm']['packages'][1]['integrity'] = 'sha512-changed'
        path.write_text(json.dumps(descriptor))
        with self.assertRaisesRegex(ValueError, 'differs'):
            candidate.verify_descriptor(self.approval, self.files, path)
        extra = self.bundle / 'unexpected.txt'
        extra.write_text('extra input')
        with self.assertRaisesRegex(ValueError, 'exactly'):
            candidate.from_bundle(self.bundle)

    def test_preview_has_local_commands_and_no_public_availability(self):
        output = self.root / 'preview'
        site_build.build_candidate(output, self.bundle)
        page = (output / 'index.html').read_text()
        self.assertIn(self.approval['version'], page)  # The fixture is a prerelease.
        self.assertIn('not published yet', page)
        self.assertIn('PowerShell', page)
        self.assertIn('python tools/native_package.py verify', page)
        self.assertIn('$env:PATH', page)
        self.assertIn('export PATH=', page)
        self.assertNotIn('sha256sum --check', page)
        self.assertNotIn('npm install --global twigsmith@', page)
        self.assertNotIn('https://github.com/msanguineti/twigsmith/releases/', page)
        self.assertNotIn('https://www.npmjs.com/package/twigsmith/v/', page)
        self.assertNotIn('Gatekeeper', page)
        self.assertNotIn('SmartScreen', page)
        for native in self.approval['native']:
            self.assertIn(native['archive'], page)
            self.assertIn(native['sha256'], page)
        for package in self.approval['npm']['packages']:
            self.assertIn(package['filename'], page)
            self.assertIn(package['sha256'], page)

    def test_new_sets_refuse_legacy_publication_before_changing_output(self):
        output = self.root / 'preview'
        output.mkdir()
        (output / 'index.html').write_text('previous')
        descriptor = candidate.from_bundle(self.bundle)
        for operation in [lambda: site_build.build(output, self.approval),
                          lambda: site_build.build(output, self.approval, receipt={}),
                          lambda: site_build.build_resolved(output, descriptor),
                          lambda: candidate.release_model.resolve(descriptor, {}, {}, b'')]:
            with self.subTest(operation=operation), self.assertRaisesRegex(ValueError, 'Phase 5'):
                operation()
            self.assertEqual((output / 'index.html').read_text(), 'previous')

    def test_cli_bundle_mode_refuses_publication_options_and_bad_inputs_preserve_output(self):
        output = self.root / 'preview'
        argv = ['build.py', '--bundle', str(self.bundle), str(output)]
        with mock.patch.object(sys, 'argv', argv):
            site_build.main()
        previous = (output / 'index.html').read_bytes()
        for options in (['--channel', 'latest'], ['--receipt', str(self.root / 'receipt.json')],
                        ['--package-sha256', '0' * 64]):
            with mock.patch.object(sys, 'argv', argv + options):
                with mock.patch.object(sys, 'stderr'), self.assertRaises(SystemExit):
                    site_build.main()
        archive = self.bundle / 'native' / self.approval['native'][0]['archive']
        archive.write_bytes(b'changed')
        with self.assertRaises(ValueError):
            site_build.build_candidate(output, self.bundle)
        self.assertEqual((output / 'index.html').read_bytes(), previous)

    def test_preview_output_cannot_replace_or_extend_the_input_bundle(self):
        identity = (self.bundle / 'identity.json').read_bytes()
        for output in (self.bundle, self.bundle / 'preview', self.root):
            with self.subTest(output=output), self.assertRaisesRegex(ValueError, 'separate'):
                site_build.build_candidate(output, self.bundle)
            self.assertEqual((self.bundle / 'identity.json').read_bytes(), identity)
