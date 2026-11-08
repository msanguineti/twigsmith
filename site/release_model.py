"""Resolve reviewed release data against public GitHub and npm observations.

Provider metadata is an observation, not a publishing permission. The descriptor is
reviewed source data for one release; old release requirements must not be replaced
with the requirements of a later development checkout.
"""
from copy import deepcopy
import itertools
import hashlib
import base64
import re
from urllib.parse import quote

REPOSITORY = 'msanguineti/twigsmith'
STABLE = re.compile(r'(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\Z')
HASH = re.compile(r'[0-9a-f]{64}\Z')
NAME = re.compile(r'[A-Za-z0-9][A-Za-z0-9._+-]*\Z')


def _text(value):
    return isinstance(value, str) and bool(value.strip()) and not any(ord(c) < 32 for c in value)


def _platform(value):
    if (not isinstance(value, dict) or set(value) != {'os', 'arch', 'libc'}
            or not _text(value['os']) or not _text(value['arch'])
            or (value['libc'] is not None and not _text(value['libc']))):
        raise ValueError('invalid platform')
    return value['os'], value['arch'], value['libc']


def _npm_commands(version):
    return {
        'install': [f'npm install --global twigsmith@{version}', 'twigsmith --version', 'tgi --help'],
        'npx': [f'npx --yes --package=twigsmith@{version} twigsmith --version',
                f'npx --yes --package=twigsmith@{version} tgi --help'],
    }


def _native_commands(assets):
    # The presently supported native instructions describe tar.gz archives. Other
    # archive/installation formats need their own explicit rendering semantics.
    archive = assets['archive']['name']
    if not archive.endswith('.tar.gz') or len(archive) == len('.tar.gz'):
        raise ValueError('unsupported native installation format')
    stem = archive[:-len('.tar.gz')]
    return [f"sha256sum --check {assets['checksums']['name']}",
            f'tar -xzf {archive}', f'export PATH="$PWD/{stem}:$PATH"',
            'twigsmith --version', 'tgi --help']


def resolve(descriptor, github_latest, npm_latest, npm_tarball):
    """Return an isolated resolved model or fail closed on incomplete observations.

    `github_latest` is the response from GitHub's /releases/latest endpoint;
    `npm_latest` is the registry's /twigsmith/latest response. Callers own network
    acquisition, snapshot freshness and any deployment authorization.
    """
    try:
        if descriptor.get('schema') in (2, 3):
            raise ValueError('complete platform sets support unpublished previews only; full-set publication is unavailable')
        model = deepcopy(descriptor)
        version = model['version']
        tag = model['tag']
        if (model['schema'] != 1 or model['repository'] != REPOSITORY
                or not isinstance(version, str) or not STABLE.fullmatch(version)
                or tag != 'v' + version):
            raise ValueError('invalid release descriptor identity')
        base = f'https://github.com/{REPOSITORY}/releases'
        release_url = f'{base}/tag/{quote(tag, safe="")}'
        if (github_latest['draft'] is not False or github_latest['prerelease'] is not False
                or not _text(github_latest['published_at'])
                or type(github_latest['id']) is not int or github_latest['id'] <= 0
                or github_latest['tag_name'] != tag or github_latest['html_url'] != release_url):
            raise ValueError('GitHub latest release differs from descriptor')
        npm = model['npm']
        npm_url = f'https://www.npmjs.com/package/twigsmith/v/{quote(version, safe="")}'
        if (npm['version'] != version or npm['url'] != npm_url
                or npm_latest['name'] != 'twigsmith' or npm_latest['version'] != version
                or not isinstance(npm['sha256'], str) or not HASH.fullmatch(npm['sha256'])
                or not _text(npm['integrity']) or not npm['integrity'].startswith('sha512-')
                or npm_latest['dist']['integrity'] != npm['integrity']
                or npm_latest['dist']['tarball'] != f'https://registry.npmjs.org/twigsmith/-/twigsmith-{version}.tgz'
                or not isinstance(npm_tarball, bytes)
                or hashlib.sha256(npm_tarball).hexdigest() != npm['sha256']
                or 'sha512-' + base64.b64encode(hashlib.sha512(npm_tarball).digest()).decode() != npm['integrity']):
            raise ValueError('npm latest differs from descriptor')
        coverage = [_platform(p) for p in npm['coverage']]
        observed_coverage = list(itertools.product(npm_latest['os'], npm_latest['cpu'], npm_latest.get('libc', [None])))
        if not coverage or len(set(coverage)) != len(coverage) or set(coverage) != set(observed_coverage):
            raise ValueError('npm platform coverage differs from descriptor')
        if not _text(npm['requirements']):
            raise ValueError('missing npm requirements')
        if npm['commands'] != _npm_commands(version):
            raise ValueError('invalid npm commands')
        observed = github_latest['assets']
        if not isinstance(observed, list):
            raise ValueError('invalid GitHub assets')
        by_name = {}
        for asset in observed:
            name = asset['name']
            if name in by_name:
                raise ValueError('duplicate GitHub asset')
            by_name[name] = asset
        platforms = []
        if not isinstance(model['native'], list) or not model['native']:
            raise ValueError('missing native platforms')
        all_names = set()
        for native in model['native']:
            platforms.append(_platform(native['platform']))
            requirements = native['requirements']
            if not isinstance(requirements, dict) or not requirements or not all(_text(k) and _text(v) for k, v in requirements.items()):
                raise ValueError('missing native requirements')
            assets = native['assets']
            if set(assets) != {'archive', 'manifest', 'checksums'}:
                raise ValueError('missing native asset')
            for entry in assets.values():
                name, digest = entry['name'], entry['sha256']
                if (not isinstance(name, str) or not NAME.fullmatch(name) or name in all_names
                        or not isinstance(digest, str) or not HASH.fullmatch(digest)):
                    raise ValueError('invalid native asset descriptor')
                all_names.add(name)
                url = f'{base}/download/{quote(tag, safe="")}/{quote(name, safe="")}'
                actual = by_name[name]
                if (actual['state'] != 'uploaded' or actual['browser_download_url'] != url
                        or actual['digest'] != 'sha256:' + digest or type(actual['size']) is not int or actual['size'] <= 0):
                    raise ValueError('GitHub asset differs from descriptor')
                entry['url'] = url
            # Future platforms may describe availability without choosing their
            # eventual archive format or installation instructions.
            commands = native['commands']
            qualified_linux = native['platform'] == {'os': 'linux', 'arch': 'x86_64', 'libc': 'glibc'}
            if not isinstance(commands, list) or (qualified_linux or commands) and commands != _native_commands(assets):
                raise ValueError('invalid native commands')
        if len(set(platforms)) != len(platforms):
            raise ValueError('duplicate native platform')
        model['release_url'] = release_url
        model['release_id'] = github_latest['id']
        return model
    except (KeyError, TypeError, AttributeError) as error:
        raise ValueError('incomplete release data or provider observation') from error
