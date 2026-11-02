"""Offline artifact/consumer tests; synthetic payloads do not qualify product hosts."""
from copy import deepcopy
import json
import os
from pathlib import Path
import signal
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import native_package
import npm_platform_package as package
from test_native_package import pe_fixture, macho_fixture


REPO = Path(__file__).resolve().parents[1]
VERSION = '0.1.0-beta.1'


def candidate(root, target, payload=None, version=VERSION, revision='1' * 40):
    platform = native_package.target_metadata(target)
    if payload is None:
        if platform['os'] == 'windows':
            payload = pe_fixture()
        elif platform['os'] == 'macos':
            payload = macho_fixture(platform['arch'])
        else:
            data = bytearray(64)
            data[:6] = b'\x7fELF\x02\x01'
            data[16:20] = b'\x03\x00\x3e\x00'
            payload = bytes(data)
    provenance = {'kind': 'development-probe',
                  'source': {'revision': revision, 'tree': '2' * 40, 'dirty': False},
                  'build': {'target': target, 'profile': 'release', 'locked': True, 'compiler': 'fixture'},
                  'toolchain': {'pin': 'fixture', 'rustc': 'fixture\nhost: ' + target, 'cargo': 'fixture'},
                  'environment': {'system': 'fixture, not product qualification', 'machine': platform['arch']}}
    directory = root / target
    directory.mkdir(parents=True)
    archive = native_package.write_bundle(directory, version,
        {name: payload for name in platform['executables']} | {'LICENSE': (REPO / 'LICENSE').read_bytes()},
        provenance, 1, target=target)
    return {'archive': str(archive), 'archive_sha256': package.sha256(archive),
            'payload_sha256': native_package.digest(payload)}


def contents(tarball):
    with tarfile.open(tarball) as tar:
        return {entry.name: tar.extractfile(entry).read() for entry in tar.getmembers()}


class PlatformPackageTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def test_complete_set_exact_metadata_hashes_and_identical_probe_top(self):
        inputs = [candidate(self.root, target) for target in package.PACKAGES]
        output = self.root / 'complete'
        manifest_path = package.make_package_set(REPO, inputs, output)
        record = json.loads(manifest_path.read_text())
        self.assertEqual((record['schema'], record['complete'], record['version']), (2, True, VERSION))
        self.assertEqual(len(record['packages']), 5)
        expected = {
            'twigsmith-linux-x64-gnu': (['linux'], ['x64'], ['glibc']),
            'twigsmith-win32-x64': (['win32'], ['x64'], None),
            'twigsmith-darwin-arm64': (['darwin'], ['arm64'], None),
            'twigsmith-darwin-x64': (['darwin'], ['x64'], None),
        }
        for item in record['packages']:
            tarball = output / item['filename']
            self.assertEqual(package.sha256(tarball), item['sha256'])
            files = contents(tarball)
            metadata = json.loads(files['package/package.json'])
            self.assertEqual(metadata['version'], VERSION)
            self.assertNotIn('scripts', metadata)
            self.assertNotIn('dependencies', metadata)
            if item['name'] == 'twigsmith':
                self.assertEqual(set(files), {'package/package.json', 'package/launcher.cjs',
                                             'package/README.md', 'package/LICENSE'})
                self.assertEqual(metadata['bin'], {'twigsmith': 'launcher.cjs', 'tgi': 'launcher.cjs'})
                self.assertEqual(metadata['optionalDependencies'], {name: VERSION for name in expected})
                self.assertNotIn('os', metadata)
                top = tarball
            else:
                self.assertEqual((metadata['os'], metadata['cpu'], metadata.get('libc')), expected[item['name']])
                self.assertNotIn('bin', metadata)
                self.assertNotIn('optionalDependencies', metadata)
                native = item['native']
                self.assertEqual(native_package.digest(files['package/' + native['payload']]), native['payload_sha256'])
                self.assertEqual(metadata['twigsmithNative']['sha256'], native['payload_sha256'])
        probe = self.root / 'probe'
        package.make_package_set(REPO, inputs[:1], probe, partial=True)
        self.assertEqual(top.read_bytes(), (probe / top.name).read_bytes())
        self.assertFalse(json.loads((probe / 'manifest.json').read_text())['complete'])
        with self.assertRaisesRegex(ValueError, 'must not exist'):
            package.make_package_set(REPO, inputs, output)

    def test_missing_duplicate_version_and_source_are_rejected_atomically(self):
        first = candidate(self.root / 'first', native_package.TARGET)
        cases = [([first], 'all four'), ([first, first], 'duplicate')]
        for key, options, message in (
                ('version', {'version': '0.2.0'}, 'same version'),
                ('source', {'revision': '3' * 40}, 'same version')):
            other = candidate(self.root / key, 'aarch64-apple-darwin', **options)
            cases.append(([first, other], message))
        for index, (inputs, message) in enumerate(cases):
            output = self.root / f'rejected-{index}'
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                package.make_package_set(REPO, inputs, output)
            self.assertFalse(output.exists())

    def test_wrong_archive_payload_and_corruption_rejected(self):
        original = candidate(self.root, native_package.TARGET)
        for key in ('archive_sha256', 'payload_sha256'):
            item = deepcopy(original)
            item[key] = '0' * 64
            output = self.root / key
            with self.assertRaisesRegex(ValueError, 'expected SHA-256'):
                package.make_package_set(REPO, [item], output, partial=True)
            self.assertFalse(output.exists())
        archive = Path(original['archive'])
        archive.write_bytes(archive.read_bytes() + b'corruption')
        original['archive_sha256'] = package.sha256(archive)
        with self.assertRaisesRegex(ValueError, 'checksums'):
            package.make_package_set(REPO, [original], self.root / 'corrupt', partial=True)
        self.assertFalse((self.root / 'corrupt').exists())

    def test_failed_pack_does_not_publish(self):
        item = candidate(self.root, native_package.TARGET)
        original = package.subprocess.run
        def fail_pack(args, **kwargs):
            if 'pack' in args:
                raise subprocess.CalledProcessError(1, args)
            return original(args, **kwargs)
        with patch.object(package.subprocess, 'run', side_effect=fail_pack):
            with self.assertRaises(subprocess.CalledProcessError):
                package.make_package_set(REPO, [item], self.root / 'failed', partial=True)
        self.assertFalse((self.root / 'failed').exists())

    def test_platform_pairing_and_libc_resolver(self):
        script = r'''
          const assert = require('node:assert/strict');
          const {selectPlatform} = require(process.argv[1]);
          for (const [os, arch, name] of [
            ['linux','x64','twigsmith-linux-x64-gnu'], ['win32','x64','twigsmith-win32-x64'],
            ['darwin','arm64','twigsmith-darwin-arm64'], ['darwin','x64','twigsmith-darwin-x64']]) {
              assert.equal(selectPlatform(os,arch,{header:{glibcVersionRuntime:'2.35'}})[0],name);
          }
          for (const [os,arch,report] of [['linux','x64',{}], ['linux','arm64',{header:{glibcVersionRuntime:'2.35'}}],
            ['win32','arm64',null], ['freebsd','x64',null], ['darwin','ia32',null]]) {
              assert.throws(() => selectPlatform(os,arch,report), /unsupported platform/);
          }
        '''
        subprocess.run(['node', '-e', script, str(REPO / 'tools/npm_launcher.cjs')], check=True)


@unittest.skipUnless(os.name == 'posix' and Path('/bin/sh').is_file()
                     and os.uname().sysname == 'Linux' and os.uname().machine == 'x86_64',
                     'real shell-payload consumer fixture requires Linux x64')
class InstalledLauncherTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.root = Path(cls.temporary.name)
        # /bin/sh is a real native ELF fixture, not a product qualification result.
        item = candidate(cls.root, native_package.TARGET, Path('/bin/sh').read_bytes())
        output = cls.root / 'packages'
        package.make_package_set(REPO, [item], output, partial=True)
        cls.env = package.npm_environment(cls.root / 'consumer-env')
        cls.consumer = cls.root / 'consumer with spaces'
        cls.consumer.mkdir()
        cls.top = output / f'twigsmith-{VERSION}.tgz'
        cls.payload = output / f'twigsmith-linux-x64-gnu-{VERSION}.tgz'
        cls.install(cls.consumer)

    @classmethod
    def install(cls, prefix, *options):
        subprocess.run(package.npm_command('install', '--ignore-scripts', '--offline', '--no-audit',
                         '--no-fund', '--prefix', str(prefix), *options, str(cls.top), str(cls.payload)),
                       env=cls.env, cwd=prefix, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    def test_actual_both_npm_shims_literal_args_directory_environment_io_exit(self):
        arguments = ['', 'a b', '"quoted"', "single'quote", '$HOME', '`uname`', '; touch should-not-exist',
                     'a&b', 'path\\tail\\', 'Ελληνικά']
        script = 'printf "%s\\0" "$PWD" "$FIXTURE_ENV" "$@"; cat; printf fixture-stderr >&2; exit 37'
        for command in ('twigsmith', 'tgi'):
            result = subprocess.run([str(self.consumer / 'node_modules/.bin' / command), '-c', script,
                                     'fixture', *arguments], cwd=self.consumer,
                                    env=dict(self.env, FIXTURE_ENV='inherited value'), input=b'fixture-stdin',
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.assertEqual(result.returncode, 37)
            self.assertEqual(result.stdout.split(b'\0'),
                             [str(self.consumer).encode(), b'inherited value',
                              *(arg.encode() for arg in arguments), b'fixture-stdin'])
            self.assertEqual(result.stderr, b'fixture-stderr')
            self.assertFalse((self.consumer / 'should-not-exist').exists())

    def test_actual_explicit_version_npx_both_commands_offline(self):
        for command in ('twigsmith', 'tgi'):
            result = subprocess.run(['npx', '--offline', '--yes', f'--package=twigsmith@{VERSION}',
                                     command, '-c', 'printf npx-fixture'], env=self.env,
                                    cwd=self.consumer, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.assertEqual(result.stdout, b'npx-fixture')

    def test_actual_global_install_both_shims(self):
        prefix = self.root / 'global with spaces'
        prefix.mkdir()
        self.install(prefix, '--global')
        for command in ('twigsmith', 'tgi'):
            result = subprocess.run([str(prefix / 'bin' / command), '-c', 'exit 23'], env=self.env)
            self.assertEqual(result.returncode, 23)

    def test_missing_omitted_wrong_version_and_wrong_platform_dependency(self):
        dependency = self.consumer / 'node_modules/twigsmith-linux-x64-gnu'
        metadata_path = dependency / 'package.json'
        original = metadata_path.read_bytes()
        for field, value in (('version', '9.9.9'), ('os', ['win32']), ('cpu', ['arm64']), ('libc', ['musl'])):
            metadata = json.loads(original)
            metadata[field] = value
            metadata_path.write_text(json.dumps(metadata))
            try:
                result = subprocess.run([str(self.consumer / 'node_modules/.bin/twigsmith')],
                                         env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(b'expected version', result.stderr)
            finally:
                metadata_path.write_bytes(original)
        moved = dependency.with_name('held-payload')
        dependency.rename(moved)
        try:
            result = subprocess.run([str(self.consumer / 'node_modules/.bin/tgi')],
                                     env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.assertIn(b'optional dependencies enabled', result.stderr)
        finally:
            moved.rename(dependency)
        omitted = self.root / 'omitted'
        omitted.mkdir()
        subprocess.run(package.npm_command('install', '--offline', '--ignore-scripts', '--omit=optional',
                         '--no-audit', '--no-fund', '--prefix', str(omitted), str(self.top)),
                       env=self.env, cwd=omitted, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        result = subprocess.run([str(omitted / 'node_modules/.bin/twigsmith')], env=self.env,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b'--omit=optional', result.stderr)

    def test_native_signal_identity_and_parent_directed_termination(self):
        shim = str(self.consumer / 'node_modules/.bin/twigsmith')
        result = subprocess.run([shim, '-c', 'kill -TERM $$'], env=self.env)
        self.assertEqual(result.returncode, -signal.SIGTERM)
        child = subprocess.Popen([shim, '-c', 'printf ready; exec sleep 60'], env=self.env,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            self.assertEqual(child.stdout.read(5), b'ready')
            child.send_signal(signal.SIGTERM)
            self.assertEqual(child.wait(timeout=5), -signal.SIGTERM)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
            child.stdout.close()
            child.stderr.close()


if __name__ == '__main__':
    unittest.main()
