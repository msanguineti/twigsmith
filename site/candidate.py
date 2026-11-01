#!/usr/bin/env python3
"""Generate or check reviewed site data from a verified local release candidate.

This is local preparation only: it makes no observation or claim of publication.
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tools'))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import native_package
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


def verify_descriptor(record, files, descriptor_path):
    descriptor = json.loads(Path(descriptor_path).read_text())
    if descriptor != descriptor_for(record, files):
        raise ValueError('reviewed site descriptor differs from approved candidate')


def from_bundle(bundle):
    """Verify a preparation artifact before producing reviewable site data."""
    bundle = Path(bundle)
    identity = json.loads((bundle / 'identity.json').read_text())
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
    except (ValueError, KeyError, TypeError, OSError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
