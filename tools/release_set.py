#!/usr/bin/env python3
"""Verify locally prepared release bytes and produce a site-ready identity record.

This command does not contact a registry or GitHub, and does not establish that a
source is eligible or that any artifact is published. Supply independently approved
hashes; do not regenerate them from the files immediately before a retry.
"""
import argparse
from copy import deepcopy
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
    native_manifest = archive.with_name(native_package.archive_stem(archive) + '.manifest.json')
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
        if native['target'] != native_package.TARGET:
            raise ValueError('legacy release publication requires the Linux x86_64 GNU target')
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


PLATFORM_SCHEMA = 3
TOPOLOGY = 'single-package-installer'
TARGETS = {'x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc',
           'aarch64-apple-darwin', 'x86_64-apple-darwin'}
PACKAGING_INPUTS = {'tools/npm_single_package.py', 'tools/npm_single_launcher.cjs',
                    'tools/npm_installer.cjs', 'tools/npm_platform_package.py',
                    'tools/npm_package.py', 'tools/native_package.py',
                    'tools/native_formats.py', 'README.md', 'LICENSE'}


def expected_hash(value):
    if not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{64}', value):
        raise ValueError('expected lowercase SHA-256 values')
    return value


def clean_source(value):
    if (not isinstance(value, dict) or set(value) != {'revision', 'tree', 'dirty'}
            or value['dirty'] is not False
            or any(not isinstance(value[key], str) or not re.fullmatch(r'[0-9a-f]{40}', value[key])
                   for key in ('revision', 'tree'))):
        raise ValueError('expected an approved clean source identity')


def artifact_identity(value, name):
    if (not isinstance(value, dict)
            or set(value) != {'run_id', 'run_attempt', 'artifact_id', 'name', 'workflow', 'sha256'}
            or value['name'] != name
            or value['workflow'] not in ('.github/workflows/release.yml',
                                          '.github/workflows/platform-packages.yml')
            or any(not isinstance(value[key], str) or not re.fullmatch(r'[1-9][0-9]*', value[key])
                   for key in ('run_id', 'run_attempt', 'artifact_id'))):
        raise ValueError('invalid approved platform artifact identity')
    expected_hash(value['sha256'])


def validate_platform_approval(approval):
    """Validate the independently reviewed contract, without reading candidate bytes.

    requirements maps each target to its exact frozen native manifest snapshot.
    Artifact IDs and run/attempts are review inputs, never inferred approvals.
    """
    if (not isinstance(approval, dict)
            or set(approval) != {'schema', 'topology', 'repository', 'tag', 'version',
                                 'requirements', 'native', 'npm'}
            or type(approval['schema']) is not int or approval['schema'] != PLATFORM_SCHEMA
            or approval['topology'] != TOPOLOGY or approval['repository'] != REPOSITORY
            or not isinstance(approval['version'], str)
            or not native_package.VERSION.fullmatch(approval['version'])
            or approval['tag'] != 'v' + approval['version']):
        raise ValueError('invalid integrated release approval')
    targets = TARGETS
    if (not isinstance(approval['requirements'], dict) or set(approval['requirements']) != targets
            or not isinstance(approval['native'], list) or len(approval['native']) != len(targets)):
        raise ValueError('integrated approval requires exactly four native targets and requirements')
    seen, artifacts, source = set(), set(), None
    for item in approval['native']:
        if (not isinstance(item, dict)
                or set(item) != {'target', 'archive', 'sha256', 'manifest_sha256',
                                 'payload_sha256', 'source', 'ci', 'artifact'}
                or item['target'] not in targets or item['target'] in seen):
            raise ValueError('unsupported or duplicate native target in approval')
        target = item['target']
        seen.add(target)
        platform = native_package.target_metadata(target)
        if item['archive'] != f"twigsmith-{approval['version']}-{target}{platform['extension']}":
            raise ValueError('approved native archive name differs from version/target')
        for key in ('sha256', 'manifest_sha256', 'payload_sha256'):
            expected_hash(item[key])
        clean_source(item['source'])
        if source is not None and source != item['source']:
            raise ValueError('native candidates have mixed source identities')
        source = item['source']
        artifact_identity(item['artifact'], 'platform-archive-' + target)
        if item['artifact']['artifact_id'] in artifacts:
            raise ValueError('duplicate native artifact identity')
        artifacts.add(item['artifact']['artifact_id'])
        ci = item['ci']
        if (not isinstance(ci, dict) or ci.get('GITHUB_REPOSITORY') != REPOSITORY
                or ci.get('GITHUB_SHA') != source['revision']
                or ci.get('GITHUB_RUN_ID') != item['artifact']['run_id']
                or ci.get('GITHUB_RUN_ATTEMPT') != item['artifact']['run_attempt']):
            raise ValueError('native approval differs from canonical source/run/attempt')
        native_package.validate_target_requirements(approval['requirements'][target], target)
    npm = approval['npm']
    if (not isinstance(npm, dict)
            or set(npm) != {'manifest_sha256', 'packaging', 'artifact', 'packages'}):
        raise ValueError('invalid npm set approval')
    expected_hash(npm['manifest_sha256'])
    artifact_identity(npm['artifact'], 'platform-npm-set')
    if npm['artifact']['artifact_id'] in artifacts:
        raise ValueError('duplicate npm artifact identity')
    packaging = npm['packaging']
    if (not isinstance(packaging, dict)
            or set(packaging) != {'source', 'node', 'npm', 'inputs_sha256'}
            or not isinstance(packaging['node'], str) or not packaging['node']
            or not isinstance(packaging['npm'], str) or not packaging['npm']
            or not isinstance(packaging['inputs_sha256'], dict)
            or set(packaging['inputs_sha256']) != PACKAGING_INPUTS):
        raise ValueError('invalid approved packaging inputs')
    clean_source(packaging['source'])
    for value in packaging['inputs_sha256'].values():
        expected_hash(value)
    names = {'twigsmith'}
    if not isinstance(npm['packages'], list) or len(npm['packages']) != len(names):
        raise ValueError('integrated approval requires exactly one npm installer package')
    seen = set()
    for item in npm['packages']:
        if (not isinstance(item, dict) or set(item) != {'name', 'version', 'filename', 'sha256'}
                or item['name'] not in names or item['name'] in seen
                or item['version'] != approval['version']
                or item['filename'] != f"{item['name']}-{approval['version']}.tgz"):
            raise ValueError('unsupported, duplicate or mixed npm package identity')
        seen.add(item['name'])
        expected_hash(item['sha256'])
    return deepcopy(approval)


def platform_files(native_dir, npm_dir, approval):
    """Return the exact flat native/npm input allowlists."""
    native_dir, npm_dir = Path(native_dir), Path(npm_dir)
    native = []
    for item in approval['native']:
        archive = native_dir / item['archive']
        stem = native_package.archive_stem(archive)
        native.extend((archive, native_dir / (stem + '.manifest.json'), native_dir / (stem + '.sha256')))
    return native, [npm_dir / 'manifest.json', *[npm_dir / item['filename'] for item in approval['npm']['packages']]]


def read_exact_files(directory, files):
    directory = Path(directory)
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError('candidate input must be a regular directory')
    if {path.name for path in directory.iterdir()} != {path.name for path in files}:
        raise ValueError('candidate directory contains missing or extra files')
    result = {}
    for path in files:
        if path.is_symlink() or not path.is_file():
            raise ValueError('candidate input must be a regular file')
        result[path] = path.read_bytes()
    return result


def npm_contents(data):
    members = {'package/' + name for name in ('package.json', 'LICENSE', 'README.md',
               'launcher.cjs', 'installer.cjs', 'native.json')}
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as tar:
        entries = tar.getmembers()
        if len(entries) != len(members) or {entry.name for entry in entries} != members:
            raise ValueError('npm package contains missing, duplicate or extra members')
        result = {}
        for entry in entries:
            mode = 0o755 if entry.name in ('package/launcher.cjs', 'package/installer.cjs') else 0o644
            if not entry.isfile() or entry.linkname or entry.mode != mode or entry.pax_headers:
                raise ValueError('invalid npm member type or mode')
            result[entry.name.removeprefix('package/')] = tar.extractfile(entry).read()
        return result


def installer_metadata(version):
    # Deliberately independent of the builder's metadata implementation.
    return {'name': 'twigsmith', 'version': version, 'license': 'MIT',
            'repository': {'type': 'git', 'url': 'https://github.com/msanguineti/twigsmith.git'},
            'homepage': 'https://msanguineti.github.io/twigsmith/',
            'bin': {'twigsmith': 'launcher.cjs', 'tgi': 'launcher.cjs',
                    'twigsmith-install': 'installer.cjs'},
            'engines': {'node': '^22.21.0 || >=24.5.0'},
            'scripts': {'postinstall': 'node installer.cjs'}}


def verify_platform_set(native_dir, npm_dir, approval):
    """Verify four native archives and one npm installer tarball offline.

    The returned schema-3 identity equals the supplied independent approval. It is
    not source eligibility, qualification, publication authority or provider state.
    Native and packaging sources are deliberately separate approved identities.
    """
    approval = validate_platform_approval(approval)
    native_dir, npm_dir = Path(native_dir), Path(npm_dir)
    native_files, npm_files = platform_files(native_dir, npm_dir, approval)
    before = {**read_exact_files(native_dir, native_files), **read_exact_files(npm_dir, npm_files)}
    manifest_path = npm_dir / 'manifest.json'
    if digest(before[manifest_path]) != approval['npm']['manifest_sha256']:
        raise ValueError('npm manifest differs from approved hash')
    npm = json.loads(before[manifest_path])
    source = approval['native'][0]['source']
    if (set(npm) != {'schema', 'topology', 'version', 'complete', 'source', 'native', 'packages', 'packaging'}
            or type(npm['schema']) is not int or npm['schema'] != PLATFORM_SCHEMA or npm['topology'] != TOPOLOGY
            or npm['complete'] is not True or npm['version'] != approval['version']
            or npm['source'] != source or npm['packaging'] != approval['npm']['packaging']
            or not isinstance(npm['packages'], list) or len(npm['packages']) != 1):
        raise ValueError('npm set manifest differs from complete approved set')
    records = {}
    for record in npm['packages']:
        if not isinstance(record, dict) or record.get('name') in records:
            raise ValueError('duplicate or invalid npm package record')
        records[record.get('name')] = record
    if set(records) != {item['name'] for item in approval['npm']['packages']}:
        raise ValueError('npm set contains missing or extra package identities')
    with tempfile.TemporaryDirectory() as temp:
        native_records, targets = [], {}
        for index, item in enumerate(approval['native']):
            archive = native_dir / item['archive']
            manifest_path = native_dir / (native_package.archive_stem(archive) + '.manifest.json')
            if digest(before[archive]) != item['sha256'] or digest(before[manifest_path]) != item['manifest_sha256']:
                raise ValueError('native archive/manifest differs from approved hashes')
            extracted = Path(temp) / str(index)
            native = native_package.verify(archive, extracted)
            if (native['target'] != item['target'] or native['version'] != approval['version']
                    or native['provenance']['kind'] != 'ci-candidate'
                    or native['provenance']['source'] != item['source']
                    or native['provenance']['environment']['ci'] != item['ci']
                    or native.get('requirements') != approval['requirements'][item['target']]
                    or native['files'][native['npm_payload']]['sha256'] != item['payload_sha256']):
                raise ValueError('native candidate differs from approved source/run/requirements/payload')
            stem = native_package.archive_stem(archive)
            members = {f'{stem}/{name}': {**details, 'mode': int(details['mode'], 8)}
                       for name, details in native['files'].items()}
            fields = {'archive': item['archive'], 'archive_sha256': item['sha256'],
                      'payload': native['npm_payload'], 'payload_sha256': item['payload_sha256'],
                      'members': members}
            native_records.append({'target': item['target'], **fields,
                                   'manifest_sha256': item['manifest_sha256'],
                                   'provenance': native['provenance']})
            targets[item['target']] = {**fields, 'url':
                f"https://github.com/{REPOSITORY}/releases/download/{approval['tag']}/{item['archive']}"}
        if npm['native'] != sorted(native_records, key=lambda item: item['target']):
            raise ValueError('npm native records differ from approved verified native set')
        expected_table = {'schema': 1, 'version': approval['version'], 'targets': targets}
        for item in approval['npm']['packages']:
            name = item['name']
            record = records[name]
            fields = {'name', 'version', 'filename', 'sha256'}
            if set(record) != fields:
                raise ValueError('invalid npm package record fields')
            if {key: record[key] for key in fields} != item:
                raise ValueError('npm package record differs from approved identity')
            data = before[npm_dir / item['filename']]
            if digest(data) != item['sha256']:
                raise ValueError('npm package differs from approved hash')
            contents = npm_contents(data)
            for file, input_name in (('launcher.cjs', 'tools/npm_single_launcher.cjs'),
                                      ('installer.cjs', 'tools/npm_installer.cjs'),
                                      ('README.md', 'README.md'), ('LICENSE', 'LICENSE')):
                if digest(contents[file]) != npm['packaging']['inputs_sha256'][input_name]:
                    raise ValueError('npm installer differs from approved packaging inputs')
            # Exact canonical bytes reject extra/duplicate fields and fixture URLs.
            if contents['native.json'] != (json.dumps(expected_table, indent=2, sort_keys=True) + '\n').encode():
                raise ValueError('npm native table differs from approved verified native archives')
            if json.loads(contents['package.json']) != installer_metadata(approval['version']):
                raise ValueError('npm metadata does not match the single-package installer contract')
    after = {**read_exact_files(native_dir, native_files), **read_exact_files(npm_dir, npm_files)}
    if after != before:
        raise ValueError('release inputs changed during verification')
    return approval


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive')
    parser.add_argument('npm_dir')
    parser.add_argument('--approval', type=Path, help='independently reviewed schema-3 complete-set approval; archive argument is native directory')
    parser.add_argument('--archive-sha256')
    parser.add_argument('--payload-sha256')
    parser.add_argument('--package-sha256')
    parser.add_argument('--tag')
    parser.add_argument('--native-manifest-sha256')
    parser.add_argument('--npm-manifest-sha256')
    args = parser.parse_args()
    try:
        if args.approval:
            legacy = (args.archive_sha256, args.payload_sha256, args.package_sha256, args.tag,
                      args.native_manifest_sha256, args.npm_manifest_sha256)
            if any(value is not None for value in legacy):
                raise ValueError('complete-set approval cannot be combined with legacy hash flags')
            print(json.dumps(verify_platform_set(args.archive, args.npm_dir, json.loads(args.approval.read_text())), indent=2))
            return
        if any(value is None for value in (args.archive_sha256, args.payload_sha256, args.package_sha256,
                args.tag, args.native_manifest_sha256, args.npm_manifest_sha256)):
            raise ValueError('legacy verification requires all hash flags and --tag')
        print(json.dumps(verify(args.archive, args.npm_dir, args.archive_sha256,
                                args.payload_sha256, args.package_sha256, args.tag,
                                args.native_manifest_sha256, args.npm_manifest_sha256), indent=2))
    except (ValueError, KeyError, TypeError, OSError, tarfile.TarError, json.JSONDecodeError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
