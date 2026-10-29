"""Tests for the npm artifact boundary using synthetic native input."""
import json
import subprocess
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import native_package
import npm_package


class NpmPackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.repo = Path(__file__).resolve().parents[1]
        source = self.root / 'native'
        source.mkdir()
        payload = bytearray(64)
        payload[:6] = b'\x7fELF\x02\x01'
        payload[16:20] = b'\x03\x00\x3e\x00'
        self.payload = bytes(payload)
        provenance = {'kind': 'development-probe',
                      'source': {'revision': '1' * 40, 'tree': '2' * 40, 'dirty': False},
                      'build': {'target': native_package.TARGET, 'profile': 'release'},
                      'toolchain': {'rustc': 'fixture', 'cargo': 'fixture'},
                      'environment': {'system': 'fixture'}}
        self.archive = native_package.write_bundle(source, '0.1.0-beta.1',
                        {'twigsmith': self.payload, 'tgi': self.payload,
                         'LICENSE': (self.repo / 'LICENSE').read_bytes()}, provenance, 1)
        self.archive_hash = npm_package.sha256(self.archive)
        self.payload_hash = native_package.digest(self.payload)
        self.output = self.root / 'result'

    def build(self):
        return npm_package.make_package(self.repo, self.archive, self.output,
                                        self.archive_hash, self.payload_hash)

    def test_real_npm_pack_has_only_expected_bytes_and_metadata(self):
        tarball = self.build()
        with tarfile.open(tarball) as tar:
            self.assertEqual(set(tar.getnames()), {'package/package.json', 'package/twigsmith',
                                                   'package/LICENSE', 'package/README.md'})
            metadata = json.load(tar.extractfile('package/package.json'))
            self.assertEqual(metadata['bin'], {'twigsmith': 'twigsmith', 'tgi': 'twigsmith'})
            self.assertEqual((metadata['os'], metadata['cpu'], metadata['libc']),
                             (['linux'], ['x64'], ['glibc']))
            self.assertFalse(any(k in metadata for k in ('scripts', 'dependencies', 'optionalDependencies')))
            self.assertEqual(tar.extractfile('package/twigsmith').read(), self.payload)
            self.assertEqual(tar.getmember('package/twigsmith').mode, 0o755)
        record = json.loads((self.output / 'manifest.json').read_text())
        self.assertEqual(record['package']['sha256'], npm_package.sha256(tarball))
        self.assertEqual(record['native']['payload_sha256'], self.payload_hash)
        self.assertEqual(record['packaging']['node'], subprocess.check_output(['node', '--version'], text=True).strip())
        self.assertEqual(record['packaging']['npm'], subprocess.check_output(['npm', '--version'], text=True).strip())
        with self.assertRaisesRegex(ValueError, 'must not exist'):
            self.build()

    def test_corrupt_input_and_wrong_expected_hashes_never_publish(self):
        self.archive.write_bytes(self.archive.read_bytes() + b'corrupt')
        with self.assertRaisesRegex(ValueError, 'expected SHA-256'):
            self.build()
        self.assertFalse(self.output.exists())
        # Even rebinding the caller's archive hash cannot bypass native checksums.
        self.archive_hash = npm_package.sha256(self.archive)
        with self.assertRaisesRegex(ValueError, 'checksums'):
            self.build()
        self.assertFalse(self.output.exists())

    def test_npm_failure_never_publishes(self):
        original_run = subprocess.run
        def fail_pack(args, **kwargs):
            if args[:2] == ['npm', 'pack']:
                raise subprocess.CalledProcessError(1, args)
            return original_run(args, **kwargs)
        with patch.object(npm_package.subprocess, 'run', side_effect=fail_pack):
            with self.assertRaises(subprocess.CalledProcessError):
                self.build()
        self.assertFalse(self.output.exists())

    def test_payload_mismatch_never_publishes(self):
        self.payload_hash = '0' * 64
        with self.assertRaisesRegex(ValueError, 'payload'):
            self.build()
        self.assertFalse(self.output.exists())


if __name__ == '__main__':
    unittest.main()
