"""Exercise release identity using real npm packing and synthetic native bytes."""
from copy import deepcopy
import io
import json
import shutil
import tarfile
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

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



def platform_fixture(root):
    """Real offline npm packing and synthetic native formats, never host qualification."""
    import npm_platform_package
    from test_npm_platform_package import candidate
    root = Path(root)
    native = root / 'native'
    native.mkdir(parents=True)
    approvals, inputs, requirements = [], [], {}
    for index, target in enumerate(npm_platform_package.PACKAGES):
        item = candidate(root / 'build', target)
        archive = Path(item['archive'])
        stem = native_package.archive_stem(archive)
        manifest_path = archive.with_name(stem + '.manifest.json')
        manifest = json.loads(manifest_path.read_text())
        ci = {'GITHUB_REPOSITORY': release_set.REPOSITORY, 'GITHUB_SHA': '1' * 40,
              'GITHUB_RUN_ID': '10', 'GITHUB_RUN_ATTEMPT': '1'}
        manifest['provenance']['kind'] = 'ci-candidate'
        manifest['provenance']['environment']['ci'] = ci
        manifest_path.write_text(json.dumps(manifest) + '\n')
        checksum = archive.with_name(stem + '.sha256')
        checksum.write_text(f"{item['archive_sha256']}  {archive.name}\n"
                            f"{npm_package.sha256(manifest_path)}  {manifest_path.name}\n")
        for path in (archive, manifest_path, checksum):
            shutil.copyfile(path, native / path.name)
        item['archive'] = str(native / archive.name)
        inputs.append(item)
        requirements[target] = manifest['requirements']
        approvals.append({'target': target, 'archive': archive.name, 'sha256': item['archive_sha256'],
                          'manifest_sha256': npm_package.sha256(manifest_path),
                          'payload_sha256': item['payload_sha256'], 'source': manifest['provenance']['source'],
                          'ci': ci, 'artifact': {'run_id': '10', 'run_attempt': '1',
                           'artifact_id': str(100 + index), 'name': 'platform-archive-' + target,
                           'workflow': '.github/workflows/release.yml', 'sha256': '0' * 64}})
    packaging_source = {'revision': '3' * 40, 'tree': '4' * 40, 'dirty': False}
    npm_dir = root / 'npm'
    with patch.object(native_package, 'identity', return_value=packaging_source):
        npm_platform_package.make_package_set(Path(__file__).resolve().parents[1], inputs, npm_dir)
    npm = json.loads((npm_dir / 'manifest.json').read_text())
    approval = {'schema': 2, 'topology': release_set.TOPOLOGY, 'repository': release_set.REPOSITORY,
                'tag': 'v' + npm['version'], 'version': npm['version'], 'requirements': requirements,
                'native': approvals, 'npm': {'manifest_sha256': npm_package.sha256(npm_dir / 'manifest.json'),
                'packaging': npm['packaging'], 'packages': [{key: record[key] for key in
                    ('name', 'version', 'filename', 'sha256')} for record in npm['packages']],
                'artifact': {'run_id': '20', 'run_attempt': '1', 'artifact_id': '200',
                             'name': 'platform-npm-set', 'workflow': '.github/workflows/release.yml',
                             'sha256': '0' * 64}}}
    artifacts = {}
    native_files, npm_files = release_set.platform_files(native, npm_dir, approval)
    for index, identity in enumerate([item['artifact'] for item in approvals] + [approval['npm']['artifact']]):
        paths = native_files[index * 3:index * 3 + 3] if index < 4 else npm_files
        data = io.BytesIO()
        with zipfile.ZipFile(data, 'w') as zipped:
            for path in paths:
                zipped.writestr(path.name, path.read_bytes())
        artifacts[identity['artifact_id']] = data.getvalue()
        identity['sha256'] = native_package.digest(data.getvalue())
    return approval, artifacts


class PlatformSetTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.base_tmp = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.base_tmp.cleanup)
        cls.base = Path(cls.base_tmp.name) / 'fixture'
        cls.approved, cls.artifacts = platform_fixture(cls.base)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / 'input'
        shutil.copytree(self.base, self.root)
        self.approval = deepcopy(self.approved)

    def check(self):
        return release_set.verify_platform_set(self.root / 'native', self.root / 'npm', self.approval)

    def rebind_package(self, name, transform):
        item = next(item for item in self.approval['npm']['packages'] if item['name'] == name)
        path = self.root / 'npm' / item['filename']
        with tarfile.open(path) as tar:
            entries = [(entry, tar.extractfile(entry).read()) for entry in tar.getmembers()]
        with tarfile.open(path, 'w:gz') as tar:
            for entry, data in entries:
                data = transform(entry, data)
                entry.size = len(data)
                tar.addfile(entry, io.BytesIO(data))
        item['sha256'] = npm_package.sha256(path)
        manifest_path = self.root / 'npm/manifest.json'
        record = json.loads(manifest_path.read_text())
        next(record for record in record['packages'] if record['name'] == name)['sha256'] = item['sha256']
        manifest_path.write_text(json.dumps(record))
        self.approval['npm']['manifest_sha256'] = npm_package.sha256(manifest_path)

    def test_complete_roundtrip_preserves_approved_native_and_distinct_packaging_identity(self):
        self.assertEqual(self.check(), self.approval)
        self.assertNotEqual(self.approval['native'][0]['source'], self.approval['npm']['packaging']['source'])

    def test_exact_four_targets_five_packages_and_requirements(self):
        original = deepcopy(self.approval)
        mutations = [lambda x: x['native'].pop(),
                     lambda x: x['native'].append(deepcopy(x['native'][0])),
                     lambda x: x['native'].__setitem__(1, deepcopy(x['native'][0])),
                     lambda x: x['npm']['packages'].pop(),
                     lambda x: x['npm']['packages'].__setitem__(1, deepcopy(x['npm']['packages'][0])),
                     lambda x: x['requirements'].pop(next(iter(x['requirements']))),
                     lambda x: x.update(tag='v9.9.9'),
                     lambda x: x['native'][0]['source'].update(tree='f' * 40),
                     lambda x: x['native'][0]['artifact'].update(run_attempt='2'),
                     lambda x: x['npm']['packages'][0].update(version='9.9.9')]
        for mutate in mutations:
            self.approval = deepcopy(original)
            mutate(self.approval)
            with self.subTest(mutation=mutate), self.assertRaises(ValueError):
                self.check()

    def test_hash_tampering_stale_requirements_and_wrong_provenance_refused(self):
        original = deepcopy(self.approval)
        for field in ('sha256', 'manifest_sha256', 'payload_sha256'):
            self.approval = deepcopy(original)
            self.approval['native'][0][field] = '0' * 64
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.check()
        self.approval = deepcopy(original)
        target = self.approval['native'][0]['target']
        self.approval['requirements'][target]['native']['requirements']['system'] += ' changed'
        with self.assertRaisesRegex(ValueError, 'requirements'):
            self.check()
        self.approval = deepcopy(original)
        self.approval['native'][0]['ci']['RUNNER_OS'] = 'invented'
        with self.assertRaisesRegex(ValueError, 'source/run'):
            self.check()
        self.approval = deepcopy(original)
        self.approval['npm']['packaging']['source']['tree'] = '5' * 40
        with self.assertRaisesRegex(ValueError, 'manifest differs'):
            self.check()

    def test_missing_extra_and_symlinked_inputs_refused(self):
        for name in ('unexpected.zip', 'twigsmith-extra.tgz'):
            path = self.root / 'native' / name
            path.write_bytes(b'foreign')
            with self.assertRaisesRegex(ValueError, 'missing or extra'):
                self.check()
            path.unlink()
        package = self.root / 'npm' / self.approval['npm']['packages'][0]['filename']
        saved = self.root / 'saved'
        package.rename(saved)
        with self.assertRaisesRegex(ValueError, 'missing or extra'):
            self.check()
        package.symlink_to(saved)
        with self.assertRaisesRegex(ValueError, 'regular file'):
            self.check()

    def test_rebound_common_metadata_cannot_omit_or_add_dependencies_or_scripts(self):
        for replacement in ({'optionalDependencies': {}}, {'scripts': {'install': 'curl elsewhere'}},
                            {'optionalDependencies': {'twigsmith-linux-x64-gnu': '9.9.9'}},
                            {'bin': {'twigsmith': 'launcher.cjs'}}):
            def alter(entry, data):
                if entry.name == 'package/package.json':
                    metadata = json.loads(data)
                    metadata.update(replacement)
                    return json.dumps(metadata).encode()
                return data
            self.rebind_package('twigsmith', alter)
            with self.subTest(replacement=replacement), self.assertRaisesRegex(ValueError, 'metadata'):
                self.check()

    def test_rebound_payload_bytes_and_extra_tar_members_refused(self):
        def alter(entry, data):
            return data + b'tampered' if entry.name == 'package/twigsmith.exe' else data
        self.rebind_package('twigsmith-win32-x64', alter)
        with self.assertRaisesRegex(ValueError, 'bytes differ'):
            self.check()
        self.approval = deepcopy(self.approved)
        shutil.rmtree(self.root / 'npm')
        shutil.copytree(self.base / 'npm', self.root / 'npm')
        def rename(entry, data):
            if entry.name == 'package/LICENSE':
                entry.name = 'package/../LICENSE'
            return data
        self.rebind_package('twigsmith', rename)
        with self.assertRaisesRegex(ValueError, 'missing, duplicate or extra'):
            self.check()

    def test_checksum_sidecars_are_derived_exactly_and_input_races_refused(self):
        archive = self.root / 'native' / self.approval['native'][0]['archive']
        checksum = archive.with_name(native_package.archive_stem(archive) + '.sha256')
        before = checksum.read_bytes()
        checksum.write_bytes(before + b'junk')
        with self.assertRaisesRegex(ValueError, 'checksums'):
            self.check()
        checksum.write_bytes(before)
        original = native_package.verify
        def change(*args, **kwargs):
            result = original(*args, **kwargs)
            path = self.root / 'npm' / self.approval['npm']['packages'][0]['filename']
            path.write_bytes(path.read_bytes() + b'changed')
            return result
        with patch.object(native_package, 'verify', side_effect=change):
            with self.assertRaisesRegex(ValueError, 'changed during verification'):
                self.check()


if __name__ == '__main__':
    unittest.main()
