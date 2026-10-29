#!/usr/bin/env python3
"""Package a verified Linux native candidate for npm (Python 3.10+)."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile

import native_package


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def make_package(repo, archive, output, archive_sha256, payload_sha256):
    archive = Path(archive).absolute()
    output = Path(output).absolute()
    for value in (archive_sha256, payload_sha256):
        if not re.fullmatch(r'[0-9a-f]{64}', value):
            raise ValueError('expected lowercase SHA-256 hashes for native archive and payload')
    if output.exists() or output.is_symlink():
        raise ValueError('package output directory must not exist')
    if sha256(archive) != archive_sha256:
        raise ValueError('native archive does not match the expected SHA-256')
    packaging_source = native_package.identity(repo)
    tool_hash = sha256(repo / 'tools/npm_package.py')
    verifier_hash = sha256(repo / 'tools/native_package.py')
    readme = (repo / 'README.md').read_bytes()
    native_manifest = archive.with_name(archive.name[:-7] + '.manifest.json')
    native_manifest_hash = sha256(native_manifest)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.npm-package-', dir=output.parent) as temp:
        temporary = Path(temp)
        manifest = native_package.verify(archive, temporary / 'native')
        if sha256(archive) != archive_sha256:
            raise ValueError('native archive changed during verification')
        source = temporary / 'native' / archive.name[:-7]
        payload = source / manifest['npm_payload']
        if sha256(payload) != payload_sha256:
            raise ValueError('native payload does not match the expected SHA-256')
        version = manifest['version']
        metadata = {'name': 'twigsmith', 'version': version, 'license': 'MIT',
                    'os': ['linux'], 'cpu': ['x64'], 'libc': ['glibc'],
                    'bin': {'twigsmith': 'twigsmith', 'tgi': 'twigsmith'}}
        package = temporary / 'package'
        package.mkdir()
        (package / 'package.json').write_text(json.dumps(metadata, indent=2) + '\n')
        (package / 'twigsmith').write_bytes(payload.read_bytes())
        (package / 'twigsmith').chmod(0o755)
        (package / 'LICENSE').write_bytes((source / 'LICENSE').read_bytes())
        (package / 'README.md').write_bytes(readme)
        staged = temporary / 'candidate'
        staged.mkdir()
        # Avoid inherited npm config, caches and lifecycle hooks; pack never needs a registry.
        home = temporary / 'home'
        home.mkdir()
        npmrc = home / 'npmrc'
        npmrc.write_text('')
        env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'HOME': str(home),
               'npm_config_userconfig': str(npmrc), 'npm_config_globalconfig': str(home / 'global-npmrc'),
               'npm_config_cache': str(home / 'cache'), 'npm_config_update_notifier': 'false',
               'npm_config_ignore_scripts': 'true', 'npm_config_offline': 'true'}
        node_version = subprocess.check_output(['node', '--version'], env=env, cwd=temporary, text=True).strip()
        npm_version = subprocess.check_output(['npm', '--version'], env=env, cwd=temporary, text=True).strip()
        result = subprocess.run(['npm', 'pack', '--ignore-scripts', '--offline', '--json',
                                 '--pack-destination', str(staged), str(package)],
                                env=env, cwd=temporary, check=True, text=True, stdout=subprocess.PIPE)
        report = json.loads(result.stdout)
        if len(report) != 1 or report[0]['filename'] != f'twigsmith-{version}.tgz':
            raise ValueError('unexpected npm pack output')
        tarball = staged / report[0]['filename']
        expected = {'package/package.json': (package / 'package.json').read_bytes(),
                    'package/twigsmith': payload.read_bytes(),
                    'package/LICENSE': (package / 'LICENSE').read_bytes(),
                    'package/README.md': (package / 'README.md').read_bytes()}
        with tarfile.open(tarball, 'r:gz') as tar:
            entries = tar.getmembers()
            if len(entries) != len(expected):
                raise ValueError('unexpected npm package members')
            seen = set()
            for entry in entries:
                if (entry.name not in expected or entry.name in seen or not entry.isfile()
                        or entry.linkname or entry.mode != (0o755 if entry.name == 'package/twigsmith' else 0o644)
                        or tar.extractfile(entry).read() != expected[entry.name]):
                    raise ValueError(f'invalid npm package member: {entry.name}')
                seen.add(entry.name)
        if (sha256(native_manifest) != native_manifest_hash
                or native_package.identity(repo) != packaging_source
                or sha256(repo / 'tools/npm_package.py') != tool_hash
                or sha256(repo / 'tools/native_package.py') != verifier_hash
                or (repo / 'README.md').read_bytes() != readme):
            raise ValueError('packaging inputs changed during assembly')
        record = {'schema': 1, 'package': {'name': tarball.name, 'sha256': sha256(tarball)},
                  'native': {'archive': archive.name, 'sha256': archive_sha256,
                             'manifest_sha256': native_manifest_hash,
                             'payload': manifest['npm_payload'], 'payload_sha256': payload_sha256,
                             'provenance': manifest['provenance']},
                  'packaging': {'source': packaging_source, 'node': node_version, 'npm': npm_version,
                                'tool_sha256': tool_hash, 'native_verifier_sha256': verifier_hash,
                                'readme_sha256': native_package.digest(readme)}}
        (staged / 'manifest.json').write_text(json.dumps(record, indent=2, sort_keys=True) + '\n')
        staged.rename(output)
    return output / tarball.name


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', help='verified native archive with adjacent manifest and checksum')
    parser.add_argument('output', help='new output directory')
    parser.add_argument('--archive-sha256', required=True, help='expected native archive SHA-256')
    parser.add_argument('--payload-sha256', required=True, help='expected selected native payload SHA-256')
    args = parser.parse_args()
    try:
        print(make_package(Path(__file__).resolve().parents[1], args.archive, args.output,
                           args.archive_sha256, args.payload_sha256))
    except (ValueError, KeyError, IndexError, TypeError, AttributeError, OSError,
            tarfile.TarError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
