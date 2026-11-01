#!/usr/bin/env python3
"""Prepare and recheck a site using public release observations, without product writes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from urllib.request import Request, urlopen

import build
import release_model

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tools'))
import native_package
GITHUB = 'https://api.github.com/repos/msanguineti/twigsmith'
REGISTRY = 'https://registry.npmjs.org/twigsmith'


def fetch(url, limit):
    headers = {'Accept': 'application/vnd.github+json', 'User-Agent': 'twigsmith-site'}
    if url.startswith(GITHUB + '/') and os.environ.get('GH_TOKEN'):
        headers['Authorization'] = 'Bearer ' + os.environ['GH_TOKEN']
    with urlopen(Request(url, headers=headers), timeout=30) as response:
        data = response.read(limit + 1)
    if len(data) > limit:
        raise ValueError('provider response exceeds size limit')
    return data


def observe():
    main = json.loads(fetch(GITHUB + '/commits/main', 2_000_000))['sha']
    checked_out = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, check=True,
                                 capture_output=True, text=True).stdout.strip()
    if main != checked_out:
        raise ValueError('checkout is not current main')
    github = json.loads(fetch(GITHUB + '/releases/latest', 2_000_000))
    npm = json.loads(fetch(REGISTRY + '/latest', 2_000_000))
    version = npm['version']
    if not isinstance(version, str) or not release_model.STABLE.fullmatch(version):
        raise ValueError('invalid stable npm version')
    descriptor = json.loads((ROOT / 'site' / 'releases' / (version + '.json')).read_text())
    tarball_url = f'https://registry.npmjs.org/twigsmith/-/twigsmith-{version}.tgz'
    if npm['dist']['tarball'] != tarball_url:
        raise ValueError('unexpected npm tarball location')
    tarball = fetch(tarball_url, 100_000_000)
    model = release_model.resolve(descriptor, github, npm, tarball)
    for platform in model['native']:
        asset = platform['assets']['manifest']
        manifest_bytes = fetch(asset['url'], 2_000_000)
        if hashlib.sha256(manifest_bytes).hexdigest() != asset['sha256']:
            raise ValueError('downloaded manifest differs from release asset')
        manifest = json.loads(manifest_bytes)
        if manifest.get('schema') == 2:
            # This released artifact predates frozen requirements. Its exact
            # bytes and reviewed descriptor bootstrap the historical release.
            if (version != '0.1.0' or asset['sha256'] !=
                    'd688fc0452ceb93b123e8261a4573c1ac7a7106013e6b72d3787d5bd5ccfa972'):
                raise ValueError('unknown historical requirements')
        elif (manifest.get('schema') != 3
              or manifest.get('requirements', {}).get('native', {}).get('platform') != platform['platform']
              or not native_package.requirements_match_descriptor(manifest, model)):
            raise ValueError('release requirements differ from native manifest')
    expected_names = {asset['name'] for platform in model['native'] for asset in platform['assets'].values()}
    assets = sorted([a['name'], a['id'], a['digest'], a['browser_download_url'], a['size'], a['state']]
                    for a in github['assets'] if a['name'] in expected_names)
    fresh_github = json.loads(fetch(GITHUB + '/releases/latest', 2_000_000))
    fresh_npm = json.loads(fetch(REGISTRY + '/latest', 2_000_000))
    fresh_model = release_model.resolve(descriptor, fresh_github, fresh_npm, tarball)
    fresh_assets = sorted([a['name'], a['id'], a['digest'], a['browser_download_url'], a['size'], a['state']]
                          for a in fresh_github['assets'] if a['name'] in expected_names)
    if fresh_model != model or fresh_assets != assets:
        raise ValueError('release changed during provider observation')
    if json.loads(fetch(GITHUB + '/commits/main', 2_000_000))['sha'] != main:
        raise ValueError('main changed during provider observation')
    return {'source': main, 'model': model, 'assets': assets}


def artifact_hash(output):
    output = Path(output)
    if output.is_symlink() or not output.is_dir():
        raise ValueError('invalid site artifact')
    digest = hashlib.sha256()
    for path in sorted(output.rglob('*')):
        if path.is_symlink() or not (path.is_file() or path.is_dir()):
            raise ValueError('invalid site artifact entry')
        if path.is_file():
            relative = path.relative_to(output).as_posix().encode()
            content = path.read_bytes()
            digest.update(len(relative).to_bytes(8, 'big') + relative)
            digest.update(len(content).to_bytes(8, 'big') + content)
    return digest.hexdigest()


def prepare(output, snapshot, expected_version=None):
    state = observe()
    if expected_version and state['model']['version'] != expected_version:
        raise ValueError('latest release differs from published version')
    build.build_resolved(output, state['model'])
    state['artifact_sha256'] = artifact_hash(output)
    state['output'] = str(Path(output).absolute())
    Path(snapshot).write_text(json.dumps(state, sort_keys=True) + '\n')


def recheck(snapshot):
    previous = json.loads(Path(snapshot).read_text())
    current = observe()
    if any(current[key] != previous[key] for key in ('source', 'model', 'assets')):
        raise ValueError('site source or release observations changed')
    if artifact_hash(previous['output']) != previous['artifact_sha256']:
        raise ValueError('site artifact changed after preparation')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    p = commands.add_parser('prepare')
    p.add_argument('--output', required=True)
    p.add_argument('--snapshot', required=True)
    p.add_argument('--expected-version')
    p = commands.add_parser('recheck')
    p.add_argument('--snapshot', required=True)
    args = parser.parse_args()
    try:
        if args.command == 'prepare':
            prepare(args.output, args.snapshot, args.expected_version)
        else:
            recheck(args.snapshot)
    except (ValueError, KeyError, TypeError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
