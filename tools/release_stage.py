#!/usr/bin/env python3
"""Inspect immutable Actions inputs and prepare an allowlisted release bundle.

Only reads GitHub. Candidate creation is local; it never publishes or creates tags.
"""
import argparse
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import zipfile

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


def check_dispatch(request):
    if (os.environ.get('GITHUB_REPOSITORY') != REPO or os.environ.get('GITHUB_REF') != 'refs/heads/main'
            or os.environ.get('GITHUB_SHA') != request['publisher_revision']):
        raise ValueError('dispatch must use the approved canonical main publisher revision')
    current = subprocess.check_output(['git', 'rev-parse', 'refs/remotes/origin/main'], text=True).strip()
    if current != request['publisher_revision']:
        raise ValueError('main advanced; review and dispatch the current publisher revision')


def inspect_platform_request(approval, fetch=api):
    """Read only: require exact successful canonical run/attempt/artifact metadata.

    The external approval supplies artifact ZIP hashes and member hashes. No run
    search, latest-artifact selection, package generation or provider write occurs.
    """
    approval = release_set.validate_platform_approval(approval)
    results = []
    items = [(item['artifact'], item['source']['revision']) for item in approval['native']]
    items.append((approval['npm']['artifact'], approval['npm']['packaging']['source']['revision']))
    for identity, source in items:
        run_id, attempt, artifact_id = (identity[key] for key in ('run_id', 'run_attempt', 'artifact_id'))
        run = fetch(f'repos/{REPO}/actions/runs/{run_id}')
        artifact = fetch(f'repos/{REPO}/actions/artifacts/{artifact_id}')
        if (str(run['id']) != run_id or str(run['run_attempt']) != attempt
                or run['repository']['full_name'] != REPO or run['head_repository']['full_name'] != REPO
                or run['status'] != 'completed' or run['conclusion'] != 'success'
                or run['event'] != 'workflow_dispatch'
                or run['path'] != identity['workflow'] or run['head_sha'] != source):
            raise ValueError('platform run differs from approved canonical source/workflow/attempt')
        if (str(artifact['id']) != artifact_id or artifact['name'] != identity['name']
                or artifact['expired'] is not False
                or str(artifact['workflow_run']['id']) != run_id
                or artifact['workflow_run']['head_sha'] != source
                or artifact.get('digest') != 'sha256:' + identity['sha256']):
            raise ValueError('platform artifact differs from approved identity/digest or has expired')
        results.append({'run_id': run_id, 'run_attempt': attempt, 'artifact_id': artifact_id,
                        'source': source, 'name': identity['name'], 'sha256': identity['sha256']})
    return results


def prepare_platform_set(downloaded, output, approval):
    """Atomically snapshot an already assembled complete set; never regenerate bytes.

    downloaded contains flat native/ and npm/ directories. Verification before and
    after copying prevents partial or changed input from becoming an accepted set.
    """
    approval = release_set.validate_platform_approval(approval)
    downloaded, output = Path(downloaded), Path(output)
    if downloaded.is_symlink() or not downloaded.is_dir():
        raise ValueError('downloaded bundle must be a regular directory')
    if output.exists() or output.is_symlink():
        raise ValueError('prepared bundle output must be new')
    record = release_set.verify_platform_set(downloaded / 'native', downloaded / 'npm', approval)
    native_files, npm_files = release_set.platform_files(downloaded / 'native', downloaded / 'npm', record)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.release-platform-', dir=output.parent) as temp:
        staged = Path(temp) / 'bundle'
        for name, files in (('native', native_files), ('npm', npm_files)):
            directory = staged / name
            directory.mkdir(parents=True)
            for file in files:
                if file.is_symlink() or not file.is_file():
                    raise ValueError('release inputs changed during staging')
                shutil.copyfile(file, directory / file.name)
        record = release_set.verify_platform_set(staged / 'native', staged / 'npm', record)
        release_set.verify_platform_set(downloaded / 'native', downloaded / 'npm', record)
        (staged / 'identity.json').write_text(json.dumps(record, indent=2) + '\n')
        if output.exists() or output.is_symlink():
            raise ValueError('prepared bundle output appeared during staging')
        staged.rename(output)
    return record


def download_artifact(artifact_id):
    """Acquire one immutable artifact using only GitHub's read endpoint."""
    return subprocess.check_output(['gh', 'api', f'repos/{REPO}/actions/artifacts/{positive(artifact_id)}/zip'])


def extract_artifact(data, identity, destination, names):
    """Extract a flat, exact regular-file allowlist after approved ZIP hash binding."""
    if release_set.digest(data) != identity['sha256']:
        raise ValueError('downloaded artifact ZIP differs from approved hash')
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        entries = archive.infolist()
        if len(entries) != len(names) or {entry.filename for entry in entries} != set(names):
            raise ValueError('artifact ZIP contains missing, duplicate or extra files')
        contents = {}
        for entry in entries:
            mode = entry.external_attr >> 16
            if (entry.filename != entry.orig_filename or entry.is_dir() or entry.flag_bits & 1
                    or (stat.S_IFMT(mode) not in (0, stat.S_IFREG))):
                raise ValueError('artifact ZIP contains a nonregular or unsafe member')
            contents[entry.filename] = archive.read(entry)
    destination.mkdir(parents=True, exist_ok=True)
    for name, data in contents.items():
        path = destination / name
        if path.exists() or path.is_symlink():
            raise ValueError('artifact ZIP conflicts with another input')
        path.write_bytes(data)


def validate_platform_sources(approval):
    """Apply the same protected-main source eligibility rule as release preparation."""
    spec = importlib.util.spec_from_file_location('release_source', ROOT / 'scripts/ci/select-release-source.py')
    selector = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(selector)
    sources = {item['source']['revision'] for item in approval['native']}
    sources.add(approval['npm']['packaging']['source']['revision'])
    for source in sorted(sources):
        selector.validate_source(source, repo=REPO)


def acquire_platform_set(approval, output, fetch=api, download=download_artifact,
                         validate_sources=validate_platform_sources):
    """Read approved Actions artifacts and publish one verified local directory.

    Source eligibility and exact successful run/artifact metadata precede download.
    A second metadata read catches expiry, attempt or identity changes during it.
    Failure leaves no accepted output and mutates neither inputs nor providers.
    """
    approval = release_set.validate_platform_approval(approval)
    output = Path(output)
    if output.exists() or output.is_symlink():
        raise ValueError('prepared bundle output must be new')
    inspected = inspect_platform_request(approval, fetch)
    validate_sources(approval)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.release-acquire-', dir=output.parent) as temp:
        root = Path(temp)
        native, npm = release_set.platform_files(root / 'native', root / 'npm', approval)
        for index, item in enumerate(approval['native']):
            identity = item['artifact']
            names = [path.name for path in native[index * 3:index * 3 + 3]]
            extract_artifact(download(identity['artifact_id']), identity, root / 'native', names)
        identity = approval['npm']['artifact']
        extract_artifact(download(identity['artifact_id']), identity, root / 'npm', [path.name for path in npm])
        if inspect_platform_request(approval, fetch) != inspected:
            raise ValueError('platform artifact identities changed during acquisition')
        return prepare_platform_set(root, output, approval)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['inspect-platforms', 'prepare-platforms', 'acquire-platforms'])
    parser.add_argument('request', type=Path)
    parser.add_argument('--input', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    try:
        approval = json.loads(args.request.read_text())
        if args.operation == 'inspect-platforms':
            result = inspect_platform_request(approval)
        elif args.operation == 'prepare-platforms':
            if not args.input or not args.output:
                raise ValueError('platform staging requires --input and --output')
            result = prepare_platform_set(args.input, args.output, approval)
        else:
            if not args.output:
                raise ValueError('platform acquisition requires --output')
            result = acquire_platform_set(approval, args.output)
        print(json.dumps(result, indent=2))
    except (KeyError, TypeError, ValueError, OSError, subprocess.SubprocessError, zipfile.BadZipFile, tarfile.TarError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
