"""Stable publication requires reviewed data matching the prepared bytes."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import candidate


class CandidateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / 'descriptor.json'
        self.version = '0.2.0'
        requirements = json.loads((Path(__file__).parent / 'current-requirements.json').read_text())
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
