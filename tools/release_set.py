#!/usr/bin/env python3
"""Verify locally prepared release bytes and produce a site-ready identity record.

This command does not contact a registry or GitHub, and does not establish that a
source is eligible or that any artifact is published. Supply independently approved
hashes; do not regenerate them from the files immediately before a retry.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import re
import tarfile
import tempfile

import native_package

REPOSITORY = 'msanguineti/twigsmith'
MEMBERS = {'package/package.json', 'package/twigsmith', 'package/LICENSE', 'package/README.md'}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def verify(archive, npm_dir, archive_hash, payload_hash, package_hash, tag,
           native_manifest_hash, npm_manifest_hash):
    archive, npm_dir = Path(archive), Path(npm_dir)
    for value in (archive_hash, payload_hash, package_hash, native_manifest_hash, npm_manifest_hash):
        if not re.fullmatch(r'[0-9a-f]{64}', value):
            raise ValueError('expected lowercase SHA-256 values')
    if digest(archive.read_bytes()) != archive_hash:
        raise ValueError('native archive differs from approved hash')
    native_manifest = archive.with_name(archive.name[:-7] + '.manifest.json')
    npm_manifest = npm_dir / 'manifest.json'
    if digest(native_manifest.read_bytes()) != native_manifest_hash:
        raise ValueError('native manifest differs from approved hash')
    if npm_manifest.is_symlink() or not npm_manifest.is_file():
        raise ValueError('npm manifest must be a regular file')
    npm_manifest_bytes = npm_manifest.read_bytes()
    if digest(npm_manifest_bytes) != npm_manifest_hash:
        raise ValueError('npm manifest differs from approved hash')
    with tempfile.TemporaryDirectory() as temp:
        native = native_package.verify(archive, Path(temp) / 'native')
        version = native['version']
        if tag != 'v' + version:
            raise ValueError('tag must exactly match native version')
        if (native['provenance'].get('kind') != 'ci-candidate'
                or native['provenance'].get('environment', {}).get('ci', {}).get('GITHUB_REPOSITORY') != REPOSITORY):
            raise ValueError('native candidate is not from canonical repository')
        record = json.loads(npm_manifest_bytes)
        n = record['native']
        if (n['archive'] != archive.name or n['sha256'] != archive_hash
                or n['manifest_sha256'] != native_manifest_hash
                or n['payload'] != native['npm_payload'] or n['payload_sha256'] != payload_hash
                or n['provenance'] != native['provenance']):
            raise ValueError('npm record does not match native candidate')
        if native['files'][native['npm_payload']]['sha256'] != payload_hash:
            raise ValueError('native payload differs from approved hash')
        name = f'twigsmith-{version}.tgz'
        if record['package'] != {'name': name, 'sha256': package_hash}:
            raise ValueError('npm record differs from approved package')
        package = npm_dir / name
        if package.is_symlink() or not package.is_file():
            raise ValueError('npm package must be a regular file')
        package_bytes = package.read_bytes()
        if digest(package_bytes) != package_hash:
            raise ValueError('npm package differs from approved hash')
        with tarfile.open(fileobj=io.BytesIO(package_bytes), mode='r:gz') as tar:
            entries = tar.getmembers()
            if len(entries) != 4 or {e.name for e in entries} != MEMBERS:
                raise ValueError('npm package must contain exactly four expected files')
            for entry in entries:
                if not entry.isfile() or entry.linkname or entry.mode != (0o755 if entry.name == 'package/twigsmith' else 0o644):
                    raise ValueError('invalid npm member type or mode')
            metadata = json.load(tar.extractfile('package/package.json'))
            if (metadata.get('name') != 'twigsmith' or metadata.get('version') != version
                    or metadata.get('repository') != {'type': 'git', 'url': 'https://github.com/msanguineti/twigsmith.git'}
                    or metadata.get('bin') != {'twigsmith': 'twigsmith', 'tgi': 'twigsmith'}
                    or metadata.get('os') != ['linux'] or metadata.get('cpu') != ['x64']
                    or metadata.get('libc') != ['glibc']
                    or any(k in metadata for k in ('scripts', 'dependencies', 'optionalDependencies'))):
                raise ValueError('npm metadata does not match the release contract')
            if digest(tar.extractfile('package/twigsmith').read()) != payload_hash:
                raise ValueError('npm payload differs from approved native payload')
            if digest(tar.extractfile('package/LICENSE').read()) != native['files']['LICENSE']['sha256']:
                raise ValueError('npm license differs from native candidate')
            if digest(tar.extractfile('package/README.md').read()) != record['packaging']['readme_sha256']:
                raise ValueError('npm README differs from packaging record')
        if (digest(archive.read_bytes()) != archive_hash
                or digest(native_manifest.read_bytes()) != native_manifest_hash
                or digest(npm_manifest.read_bytes()) != npm_manifest_hash):
            raise ValueError('release inputs changed during verification')
        return {'repository': REPOSITORY, 'tag': tag, 'version': version,
                'native': {'archive': archive.name, 'sha256': archive_hash,
                           'manifest_sha256': native_manifest_hash, 'payload_sha256': payload_hash,
                           'source': native['provenance']['source'],
                           'ci': native['provenance']['environment']['ci']},
                'npm': {'package': name, 'sha256': package_hash,
                        'manifest_sha256': npm_manifest_hash, 'packaging': record['packaging']}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive')
    parser.add_argument('npm_dir')
    parser.add_argument('--archive-sha256', required=True)
    parser.add_argument('--payload-sha256', required=True)
    parser.add_argument('--package-sha256', required=True)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--native-manifest-sha256', required=True)
    parser.add_argument('--npm-manifest-sha256', required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(verify(args.archive, args.npm_dir, args.archive_sha256,
                                args.payload_sha256, args.package_sha256, args.tag,
                                args.native_manifest_sha256, args.npm_manifest_sha256), indent=2))
    except (ValueError, KeyError, TypeError, OSError, tarfile.TarError, json.JSONDecodeError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
