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
from datetime import datetime

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
            or run['event'] not in (('workflow_dispatch', 'release') if mode == 'prepare' else ('workflow_dispatch',))
            or str(run['id']) != run_id):
        raise ValueError('candidate run is not a successful canonical preparation')
    if mode == 'prepare' and str(run['run_attempt']) != str(identity['ci']['GITHUB_RUN_ATTEMPT']):
        raise ValueError('native run attempt differs from approval')
    if (str(artifact['id']) != artifact_id or artifact['name'] != name or artifact['expired'] is not False
            or str(artifact['workflow_run']['id']) != run_id
            or artifact['workflow_run']['head_sha'] != source):
        raise ValueError('artifact differs from approved run/name or has expired')
    return {'run_id': run_id, 'artifact_id': artifact_id, 'source': source}


def inspect_event_run(run_id, attempt, fetch=api):
    """Bind the continuation to one successful canonical release-event run."""
    run_id, attempt = positive(run_id), positive(attempt)
    run = fetch(f'repos/{REPO}/actions/runs/{run_id}')
    if (str(run['id']) != run_id or str(run['run_attempt']) != attempt
            or run['repository']['full_name'] != REPO or run['head_repository']['full_name'] != REPO
            or run['event'] != 'release' or run['path'] != '.github/workflows/release.yml'
            or run['status'] != 'completed' or run['conclusion'] != 'success'
            or not re.fullmatch(r'[a-f0-9]{40}', run['head_sha'])):
        raise ValueError('release continuation requires a successful canonical release-event run')
    artifacts = []
    for page in range(1, 101):
        entries = fetch(f'repos/{REPO}/actions/runs/{run_id}/artifacts?per_page=100&page={page}')['artifacts']
        artifacts.extend(entries)
        if len(entries) < 100:
            break
    else:
        raise ValueError('release artifact pagination exceeded limit')
    name = f'release-event-{run_id}-{attempt}'
    matches = [a for a in artifacts if a['name'] == name and a['expired'] is False]
    if len(matches) != 1:
        raise ValueError('release event artifact is missing or ambiguous')
    artifact = matches[0]
    if (str(artifact['workflow_run']['id']) != run_id
            or artifact['workflow_run']['head_sha'] != run['head_sha']):
        raise ValueError('release event artifact differs from its run')
    return {'run_id': run_id, 'artifact_id': positive(artifact['id']), 'source': run['head_sha'], 'attempt': attempt}


def request_from_event(event, event_run, publisher, fetch=api):
    """Bind a release-body candidate request to the original and current release."""
    if (event['action'] != 'published' or event['repository']['full_name'] != REPO
            or event['source'] != event_run['source'] or str(event['run_id']) != event_run['run_id']
            or str(event['run_attempt']) != event_run['attempt']):
        raise ValueError('release event metadata differs from its successful run')
    original = event['release']
    release_id = positive(original['id'])
    current = fetch(f'repos/{REPO}/releases/{release_id}')
    for field in ('id', 'tag_name', 'body', 'prerelease', 'published_at'):
        if current[field] != original[field]:
            raise ValueError('release identity changed since publication')
    if current['draft'] is not False or not current['published_at']:
        raise ValueError('release is not published')
    body = current['body'] or ''
    matches = re.findall(r'<!-- twigsmith-release-request\s*\n(.*?)\n-->', body, re.DOTALL)
    if len(matches) != 1:
        raise ValueError('release requires exactly one candidate request')
    request = json.loads(matches[0])
    if set(request) != {'candidate_run', 'artifact_id', 'approval'}:
        raise ValueError('release candidate request fields differ')
    approval = request['approval']
    publication = approval['publication']
    if 'release_id' in publication:
        raise ValueError('release ID comes from the event, not the request')
    publication['release_id'] = int(release_id)
    if (approval['tag'] != current['tag_name'] or approval['native']['source']['revision'] != event_run['source']
            or publication['source'] != event_run['source'] or publication['prerelease'] is not current['prerelease']):
        raise ValueError('release tag, class or source differs from candidate')
    request['publisher_revision'] = publisher
    result = inspect_request('publish', request, fetch, publisher=publisher)
    prep = fetch(f"repos/{REPO}/actions/runs/{result['run_id']}")
    artifact = fetch(f"repos/{REPO}/actions/artifacts/{result['artifact_id']}")
    published = datetime.fromisoformat(current['published_at'].replace('Z', '+00:00'))
    for timestamp in (prep['updated_at'], artifact['created_at']):
        if datetime.fromisoformat(timestamp.replace('Z', '+00:00')) > published:
            raise ValueError('candidate was not prepared before release publication')
    return request


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
    parser.add_argument('operation', choices=['inspect-prepare', 'inspect-publish', 'inspect-event', 'event-request', 'prepare', 'validate'])
    parser.add_argument('request', type=Path)
    parser.add_argument('--input', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    try:
        if args.operation == 'inspect-event':
            result = inspect_event_run(os.environ['TRIGGER_RUN'], os.environ['TRIGGER_ATTEMPT'])
            with open(os.environ['GITHUB_OUTPUT'], 'a') as out:
                for key, value in result.items():
                    out.write(f'{key}={value}\n')
        elif args.operation == 'event-request':
            event_run = inspect_event_run(os.environ['TRIGGER_RUN'], os.environ['TRIGGER_ATTEMPT'])
            request = request_from_event(json.loads(args.request.read_text()), event_run, os.environ['GITHUB_SHA'])
            check_dispatch(request)
            from release_publish import HTTP, Providers
            if Providers(HTTP(token=os.environ['GH_TOKEN'])).tag_source(request['approval']['tag']) != event_run['source']:
                raise ValueError('release tag changed or is absent')
            spec = importlib.util.spec_from_file_location('release_source', ROOT / 'scripts/ci/select-release-source.py')
            selector = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(selector)
            selector.validate_source(event_run['source'], repo=REPO)
            selector.validate_source(request['publisher_revision'], repo=REPO)
            if not args.output:
                raise ValueError('event request needs an output path')
            args.output.write_text(json.dumps(request))
            result = {'release_id': request['approval']['publication']['release_id']}
        else:
            request = json.loads(args.request.read_text())
        if args.operation in ('inspect-prepare', 'inspect-publish'):
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
        elif args.operation == 'validate':
            check_dispatch(request)
            record, _, _ = validate_bundle(args.input, request)
            result = record
        print(json.dumps(result, indent=2))
    except (KeyError, TypeError, ValueError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
