#!/usr/bin/env python3
"""Generate or check reviewed site data from a verified local release candidate.

This is local preparation only: it makes no observation or claim of publication.
"""
import argparse
import base64
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tools'))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import native_package
import npm_single_package
import release_set
import release_model

LEGACY_MANIFEST = 'd688fc0452ceb93b123e8261a4573c1ac7a7106013e6b72d3787d5bd5ccfa972'
LEGACY_REQUIREMENTS = {
    'schema': 1,
    'native': {'platform': {'os': 'linux', 'arch': 'x86_64', 'libc': 'glibc'},
               'requirements': {'system': 'Linux x86_64 with glibc 2.35+ and Git 2.45+',
                                'shell': 'POSIX sh for editors and checks'}},
    'npm': {'coverage': [{'os': 'linux', 'arch': 'x64', 'libc': 'glibc'}],
            'requirements': 'Node.js LTS and npm; Linux x86_64 with glibc 2.35+ and Git 2.45+'}}


def descriptor_for(record, files):
    """Derive a descriptor from an already verified release identity and byte snapshot."""
    if record.get('schema') in (2, 3):
        return _platform_descriptor(record, files)
    version = record['version']
    if not release_model.STABLE.fullmatch(version) or record['tag'] != 'v' + version:
        raise ValueError('site candidate must be a stable version')
    archive = record['native']['archive']
    if not archive.endswith('.tar.gz'):
        raise ValueError('unsupported native candidate format')
    stem = archive[:-7]
    manifest_name, checksums_name = stem + '.manifest.json', stem + '.sha256'
    manifest_data = files[manifest_name]
    manifest_hash = hashlib.sha256(manifest_data).hexdigest()
    if manifest_hash != record['native']['manifest_sha256']:
        raise ValueError('candidate manifest differs from identity')
    manifest = json.loads(manifest_data)
    if manifest['version'] != version:
        raise ValueError('candidate manifest version differs')
    if manifest.get('schema') == 2:
        if version != '0.1.0' or manifest_hash != LEGACY_MANIFEST:
            raise ValueError('unknown historical requirements')
        requirements = LEGACY_REQUIREMENTS
    elif manifest.get('schema') == 3:
        requirements = native_package.validate_requirements(manifest['requirements'])
    else:
        raise ValueError('unsupported candidate requirements')
    assets = {}
    for kind, name in [('archive', archive), ('manifest', manifest_name), ('checksums', checksums_name)]:
        assets[kind] = {'name': name, 'sha256': hashlib.sha256(files[name]).hexdigest()}
    package = files[record['npm']['package']]
    if assets['archive']['sha256'] != record['native']['sha256'] or hashlib.sha256(package).hexdigest() != record['npm']['sha256']:
        raise ValueError('candidate bytes differ from identity')
    checksum = (f"{assets['archive']['sha256']}  {archive}\n"
                f"{manifest_hash}  {manifest_name}\n").encode()
    if files[checksums_name] != checksum:
        raise ValueError('candidate checksum file differs from approved bytes')
    native = dict(requirements['native'])
    native['assets'] = assets
    native['commands'] = release_model._native_commands(assets)
    npm = dict(requirements['npm'])
    npm.update(version=version, url=f'https://www.npmjs.com/package/twigsmith/v/{version}',
               sha256=record['npm']['sha256'],
               integrity='sha512-' + base64.b64encode(hashlib.sha512(package).digest()).decode(),
               commands=release_model._npm_commands(version))
    return {'schema': 1, 'repository': release_set.REPOSITORY, 'version': version,
            'tag': record['tag'], 'native': [native], 'npm': npm}


def _platform_descriptor(record, files):
    """Keep complete-set preview data behind the offline verification boundary."""
    release_set.validate_platform_approval(record)
    native_names = []
    for native in record['native']:
        stem = native_package.archive_stem(native['archive'])
        native_names.extend([native['archive'], stem + '.manifest.json', stem + '.sha256'])
    npm_names = ['manifest.json'] + [item['filename'] for item in record['npm']['packages']]
    if set(files) != set(native_names + npm_names):
        raise ValueError('candidate snapshot must contain the complete native and npm set')
    # Verify this exact in-memory snapshot, rather than validating paths and later
    # reading potentially different bytes for the descriptor.
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        for directory, names in [('native', native_names), ('npm', npm_names)]:
            (root / directory).mkdir()
            for name in names:
                (root / directory / name).write_bytes(files[name])
        verified = release_set.verify_platform_set(root / 'native', root / 'npm', record)
    native_rows = []
    for native in verified['native']:
        target, archive = native['target'], native['archive']
        stem = native_package.archive_stem(archive)
        snapshot = verified['requirements'][target]
        row = deepcopy(snapshot['native'])
        row.update(target=target, payload_sha256=native['payload_sha256'])
        row['assets'] = {kind: {'name': name, 'sha256': hashlib.sha256(files[name]).hexdigest()}
                         for kind, name in [('archive', archive), ('manifest', stem + '.manifest.json'),
                                            ('checksums', stem + '.sha256')]}
        row['commands'] = _candidate_native_commands(target, archive)
        native_rows.append(row)
    npm = deepcopy(verified['npm'])
    for package in npm['packages']:
        package['integrity'] = 'sha512-' + base64.b64encode(
            hashlib.sha512(files[package['filename']]).digest()).decode()
    npm['requirements'] = {target: deepcopy(snapshot['npm'])
                           for target, snapshot in verified['requirements'].items()}
    npm['topology'] = verified['topology']
    npm['coverage'] = []
    for target in npm_single_package.TARGETS:
        platform = native_package.target_metadata(target)
        npm['coverage'].append({'target': target, 'package': 'twigsmith',
                                'os': {'linux': 'linux', 'windows': 'win32', 'macos': 'darwin'}[platform['os']],
                                'arch': {'x86_64': 'x64', 'aarch64': 'arm64'}[platform['arch']],
                                'libc': platform['libc']})
    return {'schema': 3, 'topology': verified['topology'], 'repository': verified['repository'],
            'version': verified['version'], 'tag': verified['tag'], 'availability': 'unpublished',
            'identity': deepcopy(verified), 'native': native_rows, 'npm': npm}


def _candidate_native_commands(target, archive):
    stem = native_package.archive_stem(archive)
    if native_package.target_metadata(target)['os'] == 'windows':
        return [f'python tools/native_package.py verify "BUNDLE\\native\\{archive}" --extract-to DESTINATION',
                f'$env:PATH = "$PWD\\DESTINATION\\{stem};" + $env:PATH',
                'twigsmith --version', 'tgi --help']
    return [f'./dev verify-native "BUNDLE/native/{archive}" --extract-to DESTINATION',
            f'export PATH="$PWD/DESTINATION/{stem}:$PATH"',
            'twigsmith --version', 'tgi --help']


def verify_descriptor(record, files, descriptor_path):
    descriptor = json.loads(Path(descriptor_path).read_text())
    if descriptor != descriptor_for(record, files):
        raise ValueError('reviewed site descriptor differs from approved candidate')


def from_bundle(bundle):
    """Verify a preparation artifact before producing reviewable site data."""
    bundle = Path(bundle)
    if bundle.is_symlink() or (bundle / 'identity.json').is_symlink():
        raise ValueError('candidate bundle must contain ordinary files')
    identity = json.loads((bundle / 'identity.json').read_text())
    if identity.get('schema') in (2, 3):
        if {path.name for path in bundle.iterdir()} != {'identity.json', 'native', 'npm'}:
            raise ValueError('candidate bundle must contain exactly identity.json, native and npm')
        verified = release_set.verify_platform_set(bundle / 'native', bundle / 'npm', identity)
        files = {}
        for directory in ('native', 'npm'):
            for path in (bundle / directory).iterdir():
                files[path.name] = path.read_bytes()
        return descriptor_for(verified, files)
    native, npm = identity['native'], identity['npm']
    archive = bundle / 'native' / native['archive']
    verified = release_set.verify(archive, bundle / 'npm', native['sha256'], native['payload_sha256'],
                                  npm['sha256'], identity['tag'], native['manifest_sha256'], npm['manifest_sha256'])
    if identity != verified:
        raise ValueError('candidate identity differs from verified bytes')
    stem = archive.name[:-7]
    names = [archive.name, stem + '.manifest.json', stem + '.sha256']
    files = {name: (bundle / 'native' / name).read_bytes() for name in names}
    files[npm['package']] = (bundle / 'npm' / npm['package']).read_bytes()
    return descriptor_for(verified, files)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('bundle', type=Path, help='downloaded prepared release-candidate directory')
    args = parser.parse_args()
    try:
        print(json.dumps(from_bundle(args.bundle), indent=2))
    except (ValueError, KeyError, TypeError, OSError, tarfile.TarError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
