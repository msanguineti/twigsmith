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
import npm_common
import release_set


def platform_fixture(root, version='0.1.0-beta.1'):
    """Real offline npm packing and synthetic native formats, never host qualification."""
    import npm_single_package
    from test_npm_single_package import candidate
    root = Path(root)
    native = root / 'native'
    native.mkdir(parents=True)
    approvals, inputs, requirements = [], [], {}
    for index, target in enumerate(npm_single_package.TARGETS):
        item = candidate(root / 'build', target, version=version)
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
                            f"{npm_common.sha256(manifest_path)}  {manifest_path.name}\n")
        for path in (archive, manifest_path, checksum):
            shutil.copyfile(path, native / path.name)
        item['archive'] = str(native / archive.name)
        inputs.append(item)
        requirements[target] = manifest['requirements']
        approvals.append({'target': target, 'archive': archive.name, 'sha256': item['archive_sha256'],
                          'manifest_sha256': npm_common.sha256(manifest_path),
                          'payload_sha256': item['payload_sha256'], 'source': manifest['provenance']['source'],
                          'ci': ci, 'artifact': {'run_id': '10', 'run_attempt': '1',
                           'artifact_id': str(100 + index), 'name': 'platform-archive-' + target,
                           'workflow': '.github/workflows/release.yml', 'sha256': '0' * 64}})
    packaging_source = {'revision': '3' * 40, 'tree': '4' * 40, 'dirty': False}
    npm_dir = root / 'npm'
    with patch.object(native_package, 'identity', return_value=packaging_source):
        npm_single_package.make_package_set(Path(__file__).resolve().parents[1], inputs, npm_dir)
    npm = json.loads((npm_dir / 'manifest.json').read_text())
    approval = {'schema': 3, 'topology': release_set.TOPOLOGY, 'repository': release_set.REPOSITORY,
                'tag': 'v' + npm['version'], 'version': npm['version'], 'requirements': requirements,
                'native': approvals, 'npm': {'manifest_sha256': npm_common.sha256(npm_dir / 'manifest.json'),
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
        item['sha256'] = npm_common.sha256(path)
        manifest_path = self.root / 'npm/manifest.json'
        record = json.loads(manifest_path.read_text())
        next(record for record in record['packages'] if record['name'] == name)['sha256'] = item['sha256']
        manifest_path.write_text(json.dumps(record))
        self.approval['npm']['manifest_sha256'] = npm_common.sha256(manifest_path)

    def test_complete_roundtrip_preserves_approved_native_and_distinct_packaging_identity(self):
        self.assertEqual(self.check(), self.approval)
        self.assertNotEqual(self.approval['native'][0]['source'], self.approval['npm']['packaging']['source'])

    def test_exact_four_targets_one_installer_and_requirements(self):
        original = deepcopy(self.approval)
        mutations = [lambda x: x['native'].pop(),
                     lambda x: x['native'].append(deepcopy(x['native'][0])),
                     lambda x: x['native'].__setitem__(1, deepcopy(x['native'][0])),
                     lambda x: x['npm']['packages'].pop(),
                     lambda x: x['npm']['packages'].append(deepcopy(x['npm']['packages'][0])),
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

    def test_schema_two_five_package_approvals_are_not_migrated(self):
        self.approval['schema'] = 2
        self.approval['topology'] = 'optional-platform-packages'
        for name in ('twigsmith-linux-x64-gnu', 'twigsmith-win32-x64',
                     'twigsmith-darwin-arm64', 'twigsmith-darwin-x64'):
            self.approval['npm']['packages'].append({'name': name,
                'version': self.approval['version'], 'filename': f"{name}-{self.approval['version']}.tgz",
                'sha256': '0' * 64})
        with self.assertRaisesRegex(ValueError, 'invalid integrated release approval'):
            self.check()

    def test_exact_current_packaging_inputs_reject_missing_extra_and_substituted_hashes(self):
        expected = {'tools/npm_single_package.py', 'tools/npm_common.py',
                    'tools/npm_single_launcher.cjs', 'tools/npm_installer.cjs',
                    'tools/native_package.py', 'tools/native_formats.py', 'README.md', 'LICENSE'}
        self.assertEqual(set(self.approval['npm']['packaging']['inputs_sha256']), expected)
        original = deepcopy(self.approval)
        for name in expected:
            self.approval = deepcopy(original)
            self.approval['npm']['packaging']['inputs_sha256'].pop(name)
            with self.subTest(missing=name), self.assertRaisesRegex(ValueError, 'packaging inputs'):
                self.check()
            self.approval = deepcopy(original)
            self.approval['npm']['packaging']['inputs_sha256'][name] = '0' * 64
            with self.subTest(substituted=name), self.assertRaisesRegex(ValueError, 'manifest differs'):
                self.check()
        self.approval = deepcopy(original)
        self.approval['npm']['packaging']['inputs_sha256']['tools/npm_package.py'] = '0' * 64
        with self.assertRaisesRegex(ValueError, 'packaging inputs'):
            self.check()

    def test_native_table_is_reconstructed_from_approved_archives(self):
        mutations = [lambda table: table['targets'].pop(next(iter(table['targets']))),
                     lambda table: table.update(version='9.9.9'),
                     lambda table: table.update(fixture=True)]
        for field, value in (('url', 'http://127.0.0.1:1/native'),
                             ('archive', 'foreign.tar.gz'), ('archive_sha256', '0' * 64),
                             ('payload', 'foreign'), ('payload_sha256', '0' * 64),
                             ('members', {})):
            mutations.append(lambda table, field=field, value=value:
                next(iter(table['targets'].values())).__setitem__(field, value))
        for mutation in mutations:
            shutil.rmtree(self.root / 'npm')
            shutil.copytree(self.base / 'npm', self.root / 'npm')
            self.approval = deepcopy(self.approved)
            def alter(entry, data):
                if entry.name == 'package/native.json':
                    table = json.loads(data)
                    mutation(table)
                    return (json.dumps(table, indent=2, sort_keys=True) + '\n').encode()
                return data
            self.rebind_package('twigsmith', alter)
            with self.subTest(mutation=mutation), self.assertRaisesRegex(ValueError, 'native table'):
                self.check()

    def test_rebound_manifest_native_records_cannot_authorize_a_changed_table(self):
        path = self.root / 'npm/manifest.json'
        manifest = json.loads(path.read_text())
        manifest['native'][0]['members'] = {}
        path.write_text(json.dumps(manifest))
        self.approval['npm']['manifest_sha256'] = npm_common.sha256(path)
        with self.assertRaisesRegex(ValueError, 'native records'):
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
                            {'bin': {'twigsmith': 'launcher.cjs'}}, {'engines': {'node': '>=18'}}):
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
            return data + b'tampered' if entry.name == 'package/installer.cjs' else data
        self.rebind_package('twigsmith', alter)
        with self.assertRaisesRegex(ValueError, 'packaging inputs'):
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

    def test_installer_tar_refuses_wrong_mode_and_duplicate_member(self):
        for mutation in ('mode', 'duplicate'):
            shutil.rmtree(self.root / 'npm')
            shutil.copytree(self.base / 'npm', self.root / 'npm')
            self.approval = deepcopy(self.approved)
            def alter(entry, data):
                if mutation == 'mode' and entry.name == 'package/installer.cjs':
                    entry.mode = 0o644
                elif mutation == 'duplicate' and entry.name == 'package/LICENSE':
                    entry.name = 'package/README.md'
                return data
            self.rebind_package('twigsmith', alter)
            with self.subTest(mutation=mutation), self.assertRaisesRegex(ValueError, 'npm member|duplicate'):
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
