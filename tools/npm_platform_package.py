#!/usr/bin/env python3
"""Assemble an offline npm platform package set from verified native archives.

This preparation boundary is deliberately separate from the Linux-only release
publisher. It neither publishes packages nor changes release-set contracts.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile

import native_package
from npm_package import package_filename, sha256


PACKAGES = {
    'x86_64-unknown-linux-gnu': 'twigsmith-linux-x64-gnu',
    'x86_64-pc-windows-msvc': 'twigsmith-win32-x64',
    'aarch64-apple-darwin': 'twigsmith-darwin-arm64',
    'x86_64-apple-darwin': 'twigsmith-darwin-x64',
}


def base_metadata(name, version):
    return {'name': name, 'version': version, 'license': 'MIT',
            'repository': {'type': 'git', 'url': 'https://github.com/msanguineti/twigsmith.git'},
            'homepage': 'https://msanguineti.github.io/twigsmith/'}


def top_metadata(version):
    metadata = base_metadata('twigsmith', version)
    metadata.update({'bin': {'twigsmith': 'launcher.cjs', 'tgi': 'launcher.cjs'},
                     'engines': {'node': '>=22.15.0'},
                     'optionalDependencies': {name: version for name in PACKAGES.values()}})
    return metadata


def npm_environment(root):
    root.mkdir(parents=True, exist_ok=True)
    home = root / 'home'
    home.mkdir()
    npmrc = home / 'npmrc'
    npmrc.write_text('')
    # Windows Node/npm require the system root and executable search path.
    env = {key: value for key, value in os.environ.items()
           if key.upper() in ('PATH', 'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'PATHEXT', 'TEMP', 'TMP')}
    env.update({'HOME': str(home), 'USERPROFILE': str(home),
                'npm_config_userconfig': str(npmrc),
                'npm_config_globalconfig': str(home / 'global-npmrc'),
                'npm_config_cache': str(home / 'cache'),
                'npm_config_update_notifier': 'false', 'npm_config_ignore_scripts': 'true',
                'npm_config_offline': 'true'})
    return env


def npm_command(*args):
    # npm.cmd is a batch script, not a CreateProcess executable. Invoke npm's JS
    # entry point directly on Windows, avoiding cmd.exe argument re-parsing.
    if os.name == 'nt':
        import shutil
        executable = shutil.which('npm.cmd')
        if not executable:
            raise ValueError('npm.cmd was not found on PATH')
        cli = Path(executable).parent / 'node_modules/npm/bin/npm-cli.js'
        if not cli.is_file():
            raise ValueError('npm JavaScript entry point was not found beside npm.cmd')
        return ['node', str(cli), *args]
    return ['npm', *args]


def pack(directory, staged, metadata, files, executable, env):
    """Run real offline npm pack, then check the complete tar member boundary."""
    directory.mkdir()
    encoded = (json.dumps(metadata, indent=2) + '\n').encode()
    contents = {'package.json': encoded, **files}
    for name, data in contents.items():
        path = directory / name
        path.write_bytes(data)
        path.chmod(0o755 if name == executable else 0o644)
    result = subprocess.run(npm_command('pack', '--ignore-scripts', '--offline', '--json',
                            '--pack-destination', str(staged), str(directory)),
                            env=env, cwd=directory.parent, check=True, text=True,
                            stdout=subprocess.PIPE)
    tarball = staged / package_filename(json.loads(result.stdout), metadata['name'], metadata['version'])
    expected = {f'package/{name}': data for name, data in contents.items()}
    with tarfile.open(tarball, 'r:gz') as tar:
        entries = tar.getmembers()
        if len(entries) != len(expected):
            raise ValueError('unexpected npm package members')
        seen = set()
        for entry in entries:
            mode = 0o755 if entry.name == f'package/{executable}' else 0o644
            # PE execution does not use Unix mode bits. Windows stat/chmod cannot
            # preserve an executable bit, and this payload package exposes no bin.
            modes = {0o644, 0o755} if (metadata.get('os') == ['win32'] and
                                      entry.name == f'package/{executable}') else {mode}
            if (entry.name not in expected or entry.name in seen or not entry.isfile()
                    or entry.linkname or entry.mode not in modes
                    or tar.extractfile(entry).read() != expected[entry.name]):
                raise ValueError(f'invalid npm package member: {entry.name}')
            seen.add(entry.name)
    return {'name': metadata['name'], 'version': metadata['version'],
            'filename': tarball.name, 'sha256': sha256(tarball)}


def make_package_set(repo, inputs, output, *, partial=False):
    """Return manifest.json in a newly published complete/probe output directory.

    inputs is a list of {archive, archive_sha256, payload_sha256}. Complete sets
    require all four native targets from one source identity/version. Partial
    development probes include the identical top package and available payloads.
    """
    repo, output = Path(repo).absolute(), Path(output).absolute()
    if output.exists() or output.is_symlink():
        raise ValueError('package output directory must not exist')
    if not isinstance(inputs, list) or not inputs:
        raise ValueError('native package inputs must be a nonempty list')
    packaging_source = native_package.identity(repo)
    source_files = ['tools/npm_platform_package.py', 'tools/npm_package.py',
                    'tools/native_package.py', 'tools/native_formats.py',
                    'tools/npm_launcher.cjs', 'README.md', 'LICENSE']
    source_hashes = {name: sha256(repo / name) for name in source_files}
    readme = (repo / 'README.md').read_bytes()
    launcher = (repo / 'tools/npm_launcher.cjs').read_bytes()
    license_bytes = (repo / 'LICENSE').read_bytes()
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.npm-platform-', dir=output.parent) as temp:
        temporary = Path(temp)
        env = npm_environment(temporary)
        node_version = subprocess.check_output(['node', '--version'], env=env, text=True).strip()
        npm_version = subprocess.check_output(npm_command('--version'), env=env, text=True).strip()
        staged = temporary / 'candidate'
        staged.mkdir()
        records, targets, native_inputs = [], set(), []
        version, source = None, None
        for index, item in enumerate(inputs):
            if not isinstance(item, dict) or set(item) != {'archive', 'archive_sha256', 'payload_sha256'}:
                raise ValueError('invalid native package input fields')
            for name in ('archive_sha256', 'payload_sha256'):
                if not isinstance(item[name], str) or not re.fullmatch('[0-9a-f]{64}', item[name]):
                    raise ValueError('expected lowercase native SHA-256 hashes')
            archive = Path(item['archive']).absolute()
            if sha256(archive) != item['archive_sha256']:
                raise ValueError('native archive does not match the expected SHA-256')
            stem = native_package.archive_stem(archive)
            manifest_path = archive.with_name(stem + '.manifest.json')
            manifest_hash = sha256(manifest_path)
            extracted = temporary / f'native-{index}'
            manifest = native_package.verify(archive, extracted)
            target = manifest['target']
            if target not in PACKAGES or target in targets:
                raise ValueError('unsupported or duplicate native target')
            targets.add(target)
            if version is None:
                version, source = manifest['version'], manifest['provenance']['source']
            if version != manifest['version'] or source != manifest['provenance']['source']:
                raise ValueError('native inputs must have the same version and source identity')
            root = extracted / stem
            payload_name = manifest['npm_payload']
            payload = root / payload_name
            if sha256(payload) != item['payload_sha256']:
                raise ValueError('native payload does not match the expected SHA-256')
            current_license = (root / 'LICENSE').read_bytes()
            # Git checkouts may use LF or CRLF. Compare text without changing
            # the verified native archive's original license bytes in its package.
            if current_license.replace(b'\r\n', b'\n') != license_bytes.replace(b'\r\n', b'\n'):
                raise ValueError('native input licenses disagree')
            platform = native_package.target_metadata(target)
            npm_os = {'linux': 'linux', 'windows': 'win32', 'macos': 'darwin'}[platform['os']]
            npm_arch = {'x86_64': 'x64', 'aarch64': 'arm64'}[platform['arch']]
            metadata = base_metadata(PACKAGES[target], version)
            metadata.update({'os': [npm_os], 'cpu': [npm_arch],
                             'twigsmithNative': {'target': target, 'file': payload_name,
                                                 'sha256': item['payload_sha256']}})
            if platform['libc'] is not None:
                metadata['libc'] = [platform['libc']]
            record = pack(temporary / f'package-{index}', staged, metadata,
                          {payload_name: payload.read_bytes(), 'LICENSE': current_license,
                           'README.md': (root / 'README.txt').read_bytes()}, payload_name, env)
            record['native'] = {'target': target, 'archive': archive.name,
                                'archive_sha256': item['archive_sha256'],
                                'manifest_sha256': manifest_hash, 'payload': payload_name,
                                'payload_sha256': item['payload_sha256'],
                                'provenance': manifest['provenance']}
            records.append(record)
            native_inputs.append((archive, item['archive_sha256'], manifest_path, manifest_hash))
        if not partial and targets != set(PACKAGES):
            raise ValueError('complete npm package set requires all four native targets')
        top = pack(temporary / 'top', staged, top_metadata(version),
                   {'launcher.cjs': launcher, 'LICENSE': license_bytes, 'README.md': readme},
                   'launcher.cjs', env)
        records.append(top)
        for archive, expected, manifest_path, expected_manifest in native_inputs:
            if sha256(archive) != expected or sha256(manifest_path) != expected_manifest:
                raise ValueError('native inputs changed during assembly')
        if (native_package.identity(repo) != packaging_source or
                any(sha256(repo / name) != value for name, value in source_hashes.items())):
            raise ValueError('packaging inputs changed during assembly')
        record = {'schema': 2, 'topology': 'optional-platform-packages', 'version': version,
                  'complete': not partial, 'source': source,
                  'packages': sorted(records, key=lambda entry: entry['name']),
                  'packaging': {'source': packaging_source, 'node': node_version,
                                'npm': npm_version, 'inputs_sha256': source_hashes}}
        (staged / 'manifest.json').write_text(json.dumps(record, indent=2, sort_keys=True) + '\n')
        staged.rename(output)
    return output / 'manifest.json'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('spec', type=Path, help='JSON list of native archive paths and expected archive/payload SHA-256 hashes')
    parser.add_argument('output', help='new output directory')
    parser.add_argument('--partial', action='store_true', help='assemble an unpublished per-platform development probe')
    args = parser.parse_args()
    try:
        inputs = json.loads(args.spec.read_text())
        # Relative archive paths belong to the specification location.
        for item in inputs:
            if isinstance(item, dict) and isinstance(item.get('archive'), str):
                item['archive'] = str(args.spec.absolute().parent / item['archive'])
        print(make_package_set(Path(__file__).resolve().parents[1], inputs, args.output, partial=args.partial))
    except (ValueError, KeyError, IndexError, TypeError, AttributeError, OSError,
            tarfile.TarError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
