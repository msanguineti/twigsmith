"""Exercise release identity using real npm packing and synthetic native bytes."""
import io
import json
import tarfile
from pathlib import Path
import tempfile
import unittest

import native_package
import npm_package
import release_set


class ReleaseSetTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.repo = Path(__file__).resolve().parents[1]
        native = self.root / 'native'
        native.mkdir()
        payload = bytearray(64)
        payload[:6] = b'\x7fELF\x02\x01'
        payload[16:20] = b'\x03\x00\x3e\x00'
        self.payload_hash = native_package.digest(payload)
        provenance = {'kind': 'ci-candidate',
                      'source': {'revision': '1' * 40, 'tree': '2' * 40, 'dirty': False},
                      'build': {'target': native_package.TARGET, 'profile': 'release'},
                      'toolchain': {'rustc': 'fixture', 'cargo': 'fixture'},
                      'environment': {'system': 'fixture', 'ci': {'GITHUB_REPOSITORY': 'msanguineti/twigsmith', 'GITHUB_SHA': '1' * 40, 'GITHUB_RUN_ID': '1', 'GITHUB_RUN_ATTEMPT': '1'}}}
        self.archive = native_package.write_bundle(native, '0.1.0-beta.1',
            {'twigsmith': bytes(payload), 'tgi': bytes(payload), 'LICENSE': (self.repo / 'LICENSE').read_bytes()}, provenance, 1)
        self.archive_hash = npm_package.sha256(self.archive)
        self.npm = self.root / 'npm'
        package = npm_package.make_package(self.repo, self.archive, self.npm, self.archive_hash, self.payload_hash)
        self.package_hash = npm_package.sha256(package)
        self.native_manifest = self.archive.with_name(self.archive.name[:-7] + '.manifest.json')
        self.native_manifest_hash = npm_package.sha256(self.native_manifest)
        self.npm_manifest_hash = npm_package.sha256(self.npm / 'manifest.json')

    def check(self, **overrides):
        args = dict(archive=self.archive, npm_dir=self.npm, archive_hash=self.archive_hash,
                    payload_hash=self.payload_hash, package_hash=self.package_hash, tag='v0.1.0-beta.1',
                    native_manifest_hash=self.native_manifest_hash, npm_manifest_hash=self.npm_manifest_hash)
        args.update(overrides)
        return release_set.verify(**args)

    def test_identity_and_mismatch_rejection(self):
        result = self.check()
        self.assertEqual(result['repository'], 'msanguineti/twigsmith')
        self.assertEqual(result['native']['sha256'], self.archive_hash)
        self.assertEqual(result['npm']['sha256'], self.package_hash)
        for override in ({'tag': 'v0.1.0-beta.2'}, {'archive_hash': '0' * 64},
                         {'package_hash': '0' * 64}, {'payload_hash': '0' * 64}):
            with self.subTest(override=override), self.assertRaises(ValueError):
                self.check(**override)

    def test_rebound_package_with_wrong_repository_is_rejected(self):
        package = self.npm / 'twigsmith-0.1.0-beta.1.tgz'
        with tarfile.open(package, 'r:gz') as tar:
            entries = [(entry, tar.extractfile(entry).read()) for entry in tar.getmembers()]
        with tarfile.open(package, 'w:gz') as tar:
            for entry, data in entries:
                if entry.name == 'package/package.json':
                    metadata = json.loads(data)
                    metadata['repository']['url'] = 'https://example.com/other.git'
                    data = json.dumps(metadata).encode()
                    entry.size = len(data)
                tar.addfile(entry, io.BytesIO(data))
        changed_hash = npm_package.sha256(package)
        record_path = self.npm / 'manifest.json'
        record = json.loads(record_path.read_text())
        record['package']['sha256'] = changed_hash
        record_path.write_text(json.dumps(record))
        with self.assertRaisesRegex(ValueError, 'metadata'):
            self.check(package_hash=changed_hash, npm_manifest_hash=npm_package.sha256(record_path))

    def test_coordinated_sidecar_substitution_is_rejected(self):
        native = json.loads(self.native_manifest.read_text())
        native['provenance']['environment']['ci']['GITHUB_RUN_ID'] = '999'
        self.native_manifest.write_text(json.dumps(native, indent=2) + '\n')
        checksum = self.archive.with_name(self.archive.name[:-7] + '.sha256')
        checksum.write_text(f'{self.archive_hash}  {self.archive.name}\n'
                            f'{npm_package.sha256(self.native_manifest)}  {self.native_manifest.name}\n')
        record_path = self.npm / 'manifest.json'
        record = json.loads(record_path.read_text())
        record['native']['manifest_sha256'] = npm_package.sha256(self.native_manifest)
        record['native']['provenance'] = native['provenance']
        record_path.write_text(json.dumps(record))
        with self.assertRaisesRegex(ValueError, 'native manifest differs'):
            self.check()
        with self.assertRaisesRegex(ValueError, 'npm manifest differs'):
            self.check(native_manifest_hash=npm_package.sha256(self.native_manifest))

    def test_changed_package_and_record_rejected(self):
        record_path = self.npm / 'manifest.json'
        record = json.loads(record_path.read_text())
        record['native']['provenance']['source']['revision'] = '3' * 40
        record_path.write_text(json.dumps(record))
        with self.assertRaisesRegex(ValueError, 'npm manifest differs'):
            self.check()


if __name__ == '__main__':
    unittest.main()
