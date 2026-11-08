#!/usr/bin/env python3
"""Build one npm installer package from independently hash-approved native archives."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile

import native_package
from npm_common import sha256, package_filename, base_metadata, npm_environment, npm_command

TARGETS = ('x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc',
           'aarch64-apple-darwin', 'x86_64-apple-darwin')
TOPOLOGY = 'single-package-installer'


def top_metadata(version):
    result = base_metadata('twigsmith', version)
    result.update({'bin': {'twigsmith': 'launcher.cjs', 'tgi': 'launcher.cjs',
                           'twigsmith-install': 'installer.cjs'},
                   'engines': {'node': '^22.21.0 || >=24.5.0'},
                   'scripts': {'postinstall': 'node installer.cjs'}})
    return result


def metadata_for_native(version, records):
    """Deterministic production metadata; records come from verified native manifests.

    A record contains target, archive, archive_sha256, payload, payload_sha256,
    and members (full archive names -> size, SHA-256 and integer mode).
    """
    targets = {}
    for record in records:
        target = record['target']
        if target not in TARGETS or target in targets:
            raise ValueError('unsupported or duplicate native target')
        archive = record['archive']
        targets[target] = {key: record[key] for key in
                           ('archive', 'archive_sha256', 'payload', 'payload_sha256', 'members')}
        targets[target]['url'] = (f'https://github.com/msanguineti/twigsmith/releases/download/'
                                  f'v{version}/{archive}')
    return {'schema': 1, 'version': version, 'targets': dict(sorted(targets.items()))}


def pack(directory, staged, metadata, files, env):
    directory.mkdir()
    contents = {'package.json': (json.dumps(metadata, indent=2) + '\n').encode(), **files}
    executables = {'installer.cjs', 'launcher.cjs'}
    for name, data in contents.items():
        destination = directory / name
        destination.write_bytes(data)
        destination.chmod(0o755 if name in executables else 0o644)
    result = subprocess.run(npm_command('pack', '--ignore-scripts', '--offline', '--json',
                            '--pack-destination', str(staged), str(directory)), env=env,
                            cwd=directory.parent, check=True, text=True, stdout=subprocess.PIPE)
    tarball = staged / package_filename(json.loads(result.stdout), 'twigsmith', metadata['version'])
    with tarfile.open(tarball, 'r:gz') as tar:
        entries = tar.getmembers()
        expected = {f'package/{name}': data for name, data in contents.items()}
        if len(entries) != len(expected) or len({entry.name for entry in entries}) != len(entries):
            raise ValueError('unexpected or duplicate npm members')
        for entry in entries:
            mode = 0o755 if entry.name.removeprefix('package/') in executables else 0o644
            if (entry.name not in expected or not entry.isfile() or entry.linkname or
                    entry.mode != mode or tar.extractfile(entry).read() != expected[entry.name]):
                raise ValueError(f'invalid npm member: {entry.name}')
    return {'name': 'twigsmith', 'version': metadata['version'],
            'filename': tarball.name, 'sha256': sha256(tarball)}


def make_package_set(repo, inputs, output, *, partial=False):
    repo, output = Path(repo).absolute(), Path(output).absolute()
    if output.exists() or output.is_symlink():
        raise ValueError('package output directory must not exist')
    if not isinstance(inputs, list) or not inputs:
        raise ValueError('native package inputs must be a nonempty list')
    source = native_package.identity(repo)
    source_files = ['tools/npm_single_package.py', 'tools/npm_single_launcher.cjs',
                    'tools/npm_installer.cjs', 'tools/npm_common.py',
                    'tools/native_package.py', 'tools/native_formats.py',
                    'README.md', 'LICENSE']
    source_hashes = {name: sha256(repo / name) for name in source_files}
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.npm-single-', dir=output.parent) as temp:
        temporary = Path(temp)
        env = npm_environment(temporary)
        node_version = subprocess.check_output(['node', '--version'], env=env, text=True).strip()
        npm_version = subprocess.check_output(npm_command('--version'), env=env, text=True).strip()
        staged = temporary / 'candidate'
        staged.mkdir()
        records, native_inputs = [], []
        version = native_source = None
        for index, item in enumerate(inputs):
            if not isinstance(item, dict) or set(item) != {'archive', 'archive_sha256', 'payload_sha256'}:
                raise ValueError('invalid native package input fields')
            if any(not isinstance(item[key], str) or not re.fullmatch('[0-9a-f]{64}', item[key])
                   for key in ('archive_sha256', 'payload_sha256')):
                raise ValueError('expected lowercase native SHA-256 hashes')
            archive = Path(item['archive']).absolute()
            if sha256(archive) != item['archive_sha256']:
                raise ValueError('native archive does not match the expected SHA-256')
            stem = native_package.archive_stem(archive)
            manifest_path = archive.with_name(stem + '.manifest.json')
            manifest_hash = sha256(manifest_path)
            extracted = temporary / f'native-{index}'
            manifest = native_package.verify(archive, extracted)
            if version is None:
                version, native_source = manifest['version'], manifest['provenance']['source']
            if version != manifest['version'] or native_source != manifest['provenance']['source']:
                raise ValueError('native inputs must have the same version and source identity')
            payload = manifest['npm_payload']
            if sha256(extracted / stem / payload) != item['payload_sha256']:
                raise ValueError('native payload does not match the expected SHA-256')
            if ((extracted / stem / 'LICENSE').read_bytes().replace(b'\r\n', b'\n') !=
                    (repo / 'LICENSE').read_bytes().replace(b'\r\n', b'\n')):
                raise ValueError('native input licenses disagree')
            members = {f'{stem}/{name}': {**details, 'mode': int(details['mode'], 8)}
                       for name, details in manifest['files'].items()}
            records.append({'target': manifest['target'], 'archive': archive.name,
                            'archive_sha256': item['archive_sha256'], 'manifest_sha256': manifest_hash,
                            'payload': payload, 'payload_sha256': item['payload_sha256'],
                            'members': members, 'provenance': manifest['provenance']})
            native_inputs.append((archive, item['archive_sha256'], manifest_path, manifest_hash))
        metadata = metadata_for_native(version, records)
        if not partial and set(metadata['targets']) != set(TARGETS):
            raise ValueError('complete npm package requires all four native targets')
        # Partial probes remain production-policy packages; only the test-only
        # fixture packer can emit loopback metadata. They are not publishable.
        files = {'launcher.cjs': (repo / 'tools/npm_single_launcher.cjs').read_bytes(),
                 'installer.cjs': (repo / 'tools/npm_installer.cjs').read_bytes(),
                 'native.json': (json.dumps(metadata, indent=2, sort_keys=True) + '\n').encode(),
                 'LICENSE': (repo / 'LICENSE').read_bytes(), 'README.md': (repo / 'README.md').read_bytes()}
        package = pack(temporary / 'package', staged, top_metadata(version), files, env)
        for archive, expected, manifest_path, expected_manifest in native_inputs:
            if sha256(archive) != expected or sha256(manifest_path) != expected_manifest:
                raise ValueError('native inputs changed during assembly')
        if (native_package.identity(repo) != source or
                any(sha256(repo / name) != digest for name, digest in source_hashes.items())):
            raise ValueError('packaging inputs changed during assembly')
        record = {'schema': 3, 'topology': TOPOLOGY, 'version': version, 'complete': not partial,
                  'source': native_source, 'native': sorted(records, key=lambda x: x['target']),
                  'packages': [package], 'packaging': {'source': source, 'node': node_version,
                  'npm': npm_version, 'inputs_sha256': source_hashes}}
        (staged / 'manifest.json').write_text(json.dumps(record, indent=2, sort_keys=True) + '\n')
        staged.rename(output)
    return output / 'manifest.json'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('spec', type=Path)
    parser.add_argument('output')
    parser.add_argument('--partial', action='store_true', help='unpublished incomplete preparation probe')
    args = parser.parse_args()
    try:
        inputs = json.loads(args.spec.read_text())
        for item in inputs:
            if isinstance(item, dict) and isinstance(item.get('archive'), str):
                item['archive'] = str(args.spec.absolute().parent / item['archive'])
        print(make_package_set(Path(__file__).resolve().parents[1], inputs, args.output, partial=args.partial))
    except (ValueError, KeyError, TypeError, OSError, tarfile.TarError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
