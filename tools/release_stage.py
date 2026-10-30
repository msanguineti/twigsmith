#!/usr/bin/env python3
"""Inspect immutable Actions inputs and prepare an allowlisted release bundle.

Only reads GitHub. Candidate creation is local; it never publishes or creates tags.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

import native_package
import npm_package
import release_set

ROOT = Path(__file__).resolve().parents[1]
REPO = release_set.REPOSITORY


def api(path):
    return json.loads(subprocess.check_output(['gh', 'api', path], text=True))


def positive(value):
    text = str(value)
    if not re.fullmatch(r'[1-9][0-9]*', text):
        raise ValueError('expected a positive Actions identity')
    return text


def native_identity(value):
    if (not re.fullmatch(r'[a-f0-9]{40}', value['source']['revision'])
            or not re.fullmatch(r'[a-f0-9]{40}', value['source']['tree'])
            or value['source']['dirty'] is not False
            or not re.fullmatch(r'twigsmith-[0-9A-Za-z.+-]+-x86_64-unknown-linux-gnu.tar.gz', value['archive'])):
        raise ValueError('invalid native source or archive identity')
    for field in ('sha256', 'manifest_sha256', 'payload_sha256'):
        if not re.fullmatch(r'[a-f0-9]{64}', value[field]):
            raise ValueError('invalid approved native hash')
    positive(value['ci']['GITHUB_RUN_ID'])
    positive(value['ci']['GITHUB_RUN_ATTEMPT'])
    if value['ci']['GITHUB_REPOSITORY'] != REPO or value['ci']['GITHUB_SHA'] != value['source']['revision']:
        raise ValueError('native CI identity differs from canonical source')


def inspect_request(mode, request, fetch=api, *, publisher=None):
    required = {'publisher_revision', 'artifact_id', 'native'} if mode == 'prepare' else {
        'publisher_revision', 'artifact_id', 'candidate_run', 'approval'}
    if set(request) != required or not re.fullmatch(r'[a-f0-9]{40}', request['publisher_revision']):
        raise ValueError('invalid dispatch request fields or publisher revision')
    if publisher is not None and request['publisher_revision'] != publisher:
        raise ValueError('dispatch differs from approved publisher revision')
    identity = request['native'] if mode == 'prepare' else request['approval']['native']
    native_identity(identity)
    run_id = positive(identity['ci']['GITHUB_RUN_ID'] if mode == 'prepare' else request['candidate_run'])
    artifact_id = positive(request['artifact_id'])
    source = identity['source']['revision'] if mode == 'prepare' else request['approval']['npm']['packaging']['source']['revision']
    path = '.github/workflows/release.yml' if mode == 'prepare' else '.github/workflows/publish.yml'
    name = 'native-candidate-' + source if mode == 'prepare' else 'release-candidate'
    run = fetch(f'repos/{REPO}/actions/runs/{run_id}')
    artifact = fetch(f'repos/{REPO}/actions/artifacts/{artifact_id}')
    if (run['repository']['full_name'] != REPO or run['head_repository']['full_name'] != REPO
            or run['status'] != 'completed' or run['conclusion'] != 'success'
            or run['head_sha'] != source or run['path'] != path
            or run['event'] not in ('workflow_dispatch', 'release')
            or str(run['id']) != run_id):
        raise ValueError('candidate run is not a successful canonical preparation')
    if mode == 'prepare' and str(run['run_attempt']) != str(identity['ci']['GITHUB_RUN_ATTEMPT']):
        raise ValueError('native run attempt differs from approval')
    if (str(artifact['id']) != artifact_id or artifact['name'] != name or artifact['expired'] is not False
            or str(artifact['workflow_run']['id']) != run_id
            or artifact['workflow_run']['head_sha'] != source):
        raise ValueError('artifact differs from approved run/name or has expired')
    return {'run_id': run_id, 'artifact_id': artifact_id, 'source': source}


def check_dispatch(request):
    if (os.environ.get('GITHUB_REPOSITORY') != REPO or os.environ.get('GITHUB_REF') != 'refs/heads/main'
            or os.environ.get('GITHUB_SHA') != request['publisher_revision']):
        raise ValueError('dispatch must use the approved canonical main publisher revision')
    current = subprocess.check_output(['git', 'rev-parse', 'refs/remotes/origin/main'], text=True).strip()
    if current != request['publisher_revision']:
        raise ValueError('main advanced; review and dispatch the current publisher revision')


def prepare(downloaded, output, request):
    value = request['native']
    native_identity(value)
    downloaded, output = Path(downloaded), Path(output)
    if output.exists() or output.is_symlink():
        raise ValueError('prepared bundle output must be new')
    archive = downloaded / value['archive']
    stem = value['archive'][:-7]
    files = [archive, downloaded / (stem + '.manifest.json'), downloaded / (stem + '.sha256')]
    if any(p.is_symlink() or not p.is_file() for p in files):
        raise ValueError('native release inputs must be regular files')
    if npm_package.sha256(files[1]) != value['manifest_sha256'] or npm_package.sha256(archive) != value['sha256']:
        raise ValueError('native inputs differ from approved hashes')
    manifest = native_package.verify(archive)
    if manifest['provenance']['source'] != value['source'] or manifest['provenance']['environment']['ci'] != value['ci']:
        raise ValueError('native manifest provenance differs from approved source/run')
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.release-prepare-', dir=output.parent) as temp:
        staged = Path(temp) / 'bundle'
        native = staged / 'native'
        native.mkdir(parents=True)
        for file in files:
            shutil.copyfile(file, native / file.name)
        package = npm_package.make_package(ROOT, native / archive.name, staged / 'npm', value['sha256'], value['payload_sha256'])
        record = release_set.verify(native / archive.name, staged / 'npm', value['sha256'], value['payload_sha256'],
                                    npm_package.sha256(package), 'v' + manifest['version'], value['manifest_sha256'],
                                    npm_package.sha256(staged / 'npm/manifest.json'))
        (staged / 'identity.json').write_text(json.dumps(record, indent=2) + '\n')
        staged.rename(output)
    return record


def validate_bundle(bundle, request):
    from release_publish import approved_inputs
    bundle = Path(bundle)
    approval = request['approval']
    native_identity(approval['native'])
    if approval['npm']['packaging']['source']['dirty'] is not False:
        raise ValueError('published package must have clean packaging provenance')
    return approved_inputs(bundle / 'native' / approval['native']['archive'], bundle / 'npm', approval)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['inspect-prepare', 'inspect-publish', 'prepare', 'validate'])
    parser.add_argument('request', type=Path)
    parser.add_argument('--input', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    try:
        request = json.loads(args.request.read_text())
        if args.operation.startswith('inspect-'):
            check_dispatch(request)
            result = inspect_request(args.operation.split('-')[1], request, publisher=os.environ['GITHUB_SHA'])
            spec = importlib.util.spec_from_file_location('release_source', ROOT / 'scripts/ci/select-release-source.py')
            selector = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(selector)
            native = request.get('native') or request['approval']['native']
            selector.validate_source(native['source']['revision'], repo=REPO)
            selector.validate_source(request['publisher_revision'], repo=REPO)
            with open(os.environ['GITHUB_OUTPUT'], 'a') as out:
                for key, value in result.items():
                    out.write(f'{key}={value}\n')
        elif args.operation == 'prepare':
            check_dispatch(request)
            result = prepare(args.input, args.output, request)
        else:
            check_dispatch(request)
            record, _, _ = validate_bundle(args.input, request)
            result = record
        print(json.dumps(result, indent=2))
    except (KeyError, TypeError, ValueError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
