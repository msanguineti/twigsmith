#!/usr/bin/env python3
"""Generate reviewable site data from a verified four-platform installer candidate.

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


def descriptor_for(record, files):
    """Derive current installer site data from a verified identity and byte snapshot."""
    return _platform_descriptor(record, files)


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


def from_bundle(bundle):
    """Verify a preparation artifact before producing reviewable site data."""
    bundle = Path(bundle)
    if bundle.is_symlink() or (bundle / 'identity.json').is_symlink():
        raise ValueError('candidate bundle must contain ordinary files')
    identity = json.loads((bundle / 'identity.json').read_text())
    release_set.validate_platform_approval(identity)
    if {path.name for path in bundle.iterdir()} != {'identity.json', 'native', 'npm'}:
        raise ValueError('candidate bundle must contain exactly identity.json, native and npm')
    verified = release_set.verify_platform_set(bundle / 'native', bundle / 'npm', identity)
    files = {path.name: path.read_bytes()
             for directory in ('native', 'npm') for path in (bundle / directory).iterdir()}
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
