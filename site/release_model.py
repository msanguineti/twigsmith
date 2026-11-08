"""Resolve a reviewed four-platform installer descriptor against public stable data.

Provider observations bind the displayed release to exact approved bytes. They do
not authorize publication or deployment. Historical package topologies are refused.
"""
from copy import deepcopy
import base64
import hashlib
import json
from pathlib import Path
import re
import sys
from urllib.parse import quote

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'tools'))
import native_package
import npm_single_package
import release_set

REPOSITORY = release_set.REPOSITORY
STABLE = re.compile(r'(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\Z')


def _npm_commands(version):
    return {
        'install': [f'npm install --global twigsmith@{version}', 'twigsmith --version', 'tgi --help'],
        'npx': [f'npx --yes --package=twigsmith@{version} -- twigsmith --version',
                f'npx --yes --package=twigsmith@{version} -- tgi --help'],
        'repair': [f'npx --yes --ignore-scripts --package=twigsmith@{version} -- twigsmith-install',
                   f'npx --yes --offline --package=twigsmith@{version} -- tgi --version'],
    }


def _native_commands(target, assets):
    """Installation from downloaded files; no source checkout is required."""
    archive = assets['archive']['name']
    stem = native_package.archive_stem(archive)
    digest = assets['archive']['sha256']
    if target == 'x86_64-pc-windows-msvc':
        return [f'if ((Get-FileHash "{archive}" -Algorithm SHA256).Hash.ToLowerInvariant() -ne "{digest}") {{ throw "Archive checksum mismatch" }}',
                f'Expand-Archive "{archive}" -DestinationPath .',
                f'$env:PATH = "$PWD\\{stem};" + $env:PATH', 'twigsmith --version', 'tgi --help']
    check = 'sha256sum' if target == 'x86_64-unknown-linux-gnu' else 'shasum -a 256'
    return [f"{check} --check {assets['checksums']['name']}",
            f'tar -xzf {archive}', f'export PATH="$PWD/{stem}:$PATH"',
            'twigsmith --version', 'tgi --help']


def resolve(descriptor, github_latest, npm_latest, npm_tarball):
    """Resolve only the current single-installer topology and a public stable release.

    Callers acquire /releases/latest and /twigsmith/latest, check freshness, verify
    downloaded native manifests, and retain deployment authority separately.
    """
    try:
        if (descriptor.get('schema') != 3
                or descriptor.get('topology') != release_set.TOPOLOGY):
            raise ValueError('site requires a schema 3 single-package installer descriptor')
        approval = release_set.validate_platform_approval(descriptor['identity'])
        version, tag = approval['version'], approval['tag']
        if not STABLE.fullmatch(version):
            raise ValueError('prereleases cannot describe the stable website')
        if any(descriptor[key] != approval[key] for key in ('repository', 'version', 'tag', 'topology')):
            raise ValueError('site descriptor differs from approved release identity')
        base = f'https://github.com/{REPOSITORY}/releases'
        release_url = f'{base}/tag/{quote(tag, safe="")}'
        if (github_latest['draft'] is not False or github_latest['prerelease'] is not False
                or not isinstance(github_latest['published_at'], str) or not github_latest['published_at']
                or type(github_latest['id']) is not int or github_latest['id'] <= 0
                or github_latest['tag_name'] != tag or github_latest['html_url'] != release_url):
            raise ValueError('GitHub latest stable release differs from descriptor')
        package, = descriptor['npm']['packages']
        expected_package, = approval['npm']['packages']
        if {key: package[key] for key in expected_package} != expected_package:
            raise ValueError('npm package differs from approved identity')
        integrity = 'sha512-' + base64.b64encode(hashlib.sha512(npm_tarball).digest()).decode()
        if (hashlib.sha256(npm_tarball).hexdigest() != package['sha256']
                or package['integrity'] != integrity
                or npm_latest['name'] != 'twigsmith' or npm_latest['version'] != version
                or npm_latest['dist']['integrity'] != integrity
                or npm_latest['dist']['tarball'] != f'https://registry.npmjs.org/twigsmith/-/twigsmith-{version}.tgz'):
            raise ValueError('npm latest differs from approved installer bytes')
        if any(key in npm_latest for key in ('os', 'cpu', 'libc', 'dependencies', 'optionalDependencies')):
            raise ValueError('npm registry metadata restricts or adds installer dependencies')
        contents = release_set.npm_contents(npm_tarball)
        metadata = json.loads(contents['package.json'])
        if metadata != release_set.installer_metadata(version):
            raise ValueError('npm metadata differs from single-package installer contract')
        for file, input_name in (('launcher.cjs', 'tools/npm_single_launcher.cjs'),
                                 ('installer.cjs', 'tools/npm_installer.cjs'),
                                 ('README.md', 'README.md'), ('LICENSE', 'LICENSE')):
            if hashlib.sha256(contents[file]).hexdigest() != approval['npm']['packaging']['inputs_sha256'][input_name]:
                raise ValueError('npm installer differs from approved source inputs')
        table = json.loads(contents['native.json'])
        if (set(table) != {'schema', 'version', 'targets'} or table['schema'] != 1
                or table['version'] != version or set(table['targets']) != set(npm_single_package.TARGETS)):
            raise ValueError('npm native target table differs from approved release')
        assets = github_latest['assets']
        by_name = {asset['name']: asset for asset in assets}
        if len(by_name) != len(assets):
            raise ValueError('duplicate GitHub release asset')
        model = deepcopy(descriptor)
        approved = {row['target']: row for row in approval['native']}
        if (len(model['native']) != 4
                or {row['target'] for row in model['native']} != set(approved)):
            raise ValueError('site requires all four approved native targets')
        for native in model['native']:
            target = native['target']
            identity = approved[target]
            snapshot = approval['requirements'][target]
            if (native['platform'] != snapshot['native']['platform']
                    or native['requirements'] != snapshot['native']['requirements']
                    or native['payload_sha256'] != identity['payload_sha256']
                    or descriptor['npm']['requirements'][target] != snapshot['npm']):
                raise ValueError('site platform requirements differ from approved identity')
            stem = native_package.archive_stem(identity['archive'])
            # The reviewed descriptor binds the verified candidate's raw sidecar.
            # Native Windows production uses CRLF; reconstructing records here
            # would replace that immutable byte identity with a different hash.
            checksum_digest = native['assets']['checksums']['sha256']
            release_set.expected_hash(checksum_digest)
            expected = {'archive': (identity['archive'], identity['sha256']),
                        'manifest': (stem + '.manifest.json', identity['manifest_sha256']),
                        'checksums': (stem + '.sha256', checksum_digest)}
            if set(native['assets']) != set(expected):
                raise ValueError('missing native release asset')
            for kind, (name, digest) in expected.items():
                entry = native['assets'][kind]
                if entry != {'name': name, 'sha256': digest}:
                    raise ValueError('native asset differs from approved identity')
                url = f'{base}/download/{quote(tag, safe="")}/{quote(name, safe="")}'
                actual = by_name[name]
                if (actual['state'] != 'uploaded' or actual['browser_download_url'] != url
                        or actual['digest'] != 'sha256:' + digest
                        or type(actual['size']) is not int or actual['size'] <= 0):
                    raise ValueError('GitHub release asset differs from approved identity')
                entry['url'] = url
            download = table['targets'][target]
            if (download['archive'] != identity['archive']
                    or download['archive_sha256'] != identity['sha256']
                    or download['payload_sha256'] != identity['payload_sha256']
                    or download['url'] != native['assets']['archive']['url']):
                raise ValueError('npm native download differs from approved identity')
            native['commands'] = _native_commands(target, native['assets'])
        model.update(availability='published', release_url=release_url, release_id=github_latest['id'])
        model['npm'].update(url=f'https://www.npmjs.com/package/twigsmith/v/{version}',
                            commands=_npm_commands(version))
        return model
    except (KeyError, TypeError, AttributeError) as error:
        raise ValueError('incomplete release data or provider observation') from error
