#!/usr/bin/env python3
"""Run the reviewed full-set path in the canonical publication workflow.

Publication and channel promotion are separate operations. The intervening native
consumer jobs have read-only permissions; their exact current-run evidence is
required for promotion. This command never creates releases or tags.
"""
import argparse
import importlib.util
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from urllib.error import HTTPError, URLError
from urllib.request import Request, build_opener

import release_publish
import release_set
import release_stage

ROOT = Path(__file__).resolve().parents[1]
REPO = release_set.REPOSITORY
TARGETS = {'x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc',
           'aarch64-apple-darwin', 'x86_64-apple-darwin'}


def approval_digest(approval):
    return release_set.digest(json.dumps(approval, sort_keys=True, separators=(',', ':')).encode())


def producer(environment=os.environ):
    return {'publisher_revision': environment.get('GITHUB_SHA'),
            'run_id': environment.get('GITHUB_RUN_ID'), 'run_attempt': environment.get('GITHUB_RUN_ATTEMPT')}


def site_fingerprint():
    # Fixed public Pages origin; no authentication, redirects or caller-selected URL.
    url = 'https://msanguineti.github.io/twigsmith/'
    try:
        with build_opener(release_publish.NoRedirect).open(
                Request(url, headers={'User-Agent': 'twigsmith-release', 'Cache-Control': 'no-cache'}), timeout=30) as response:
            return release_set.digest(response.read())
    except (HTTPError, URLError, TimeoutError, OSError):
        raise ValueError('stable Pages index observation unavailable') from None


def advertised_channels(state):
    return {name: {tag: channels.get(tag) for tag in ('latest', 'next')}
            for name, channels in state['tags'].items()}


def validate_baseline(baseline, request, state, operation, environment=os.environ):
    names = {'twigsmith'}
    if (not isinstance(baseline, dict)
            or set(baseline) != {'schema', 'approval_sha256', 'producer', 'channels', 'site_sha256'}
            or type(baseline['schema']) is not int or baseline['schema'] != 1
            or baseline['approval_sha256'] != approval_digest(request['approval'])
            or baseline['producer'] != producer(environment)
            or not isinstance(baseline['site_sha256'], str)
            or not re.fullmatch(r'[0-9a-f]{64}', baseline['site_sha256'])
            or not isinstance(baseline['channels'], dict) or set(baseline['channels']) != names):
        raise ValueError('provider baseline differs from the exact approved workflow attempt')
    current = advertised_channels(state)
    if set(current) != names:
        raise ValueError('incomplete advertised channel observation')
    for name, channels in baseline['channels'].items():
        if (not isinstance(channels, dict) or set(channels) != {'latest', 'next'}
                or any(value is not None and not isinstance(value, str) for value in channels.values())):
            raise ValueError('invalid baseline advertised channels')
        for tag, version in channels.items():
            allowed = (version,)
            if operation == 'promote' and name == 'twigsmith' and tag == 'next':
                allowed += (request['approval']['version'],)
            if current[name][tag] not in allowed:
                raise ValueError('advertised npm channel changed since this publication attempt began')


def inspect_request(request, fetch=release_stage.api):
    if (not isinstance(request, dict) or set(request) != {'publisher_revision', 'approval'}
            or not isinstance(request['publisher_revision'], str)
            or not re.fullmatch(r'[0-9a-f]{40}', request['publisher_revision'])):
        raise ValueError('platform publication request must bind the exact publisher and approval')
    approval = request['approval']
    # Keep the offline contract unchanged; publication effects have a separate binding.
    release_set.validate_platform_approval({key: value for key, value in approval.items() if key != 'publication'})
    from release_platform_publish import validate_publication
    validate_publication(approval)
    if (approval['publication']['prerelease'] is not True
            or not re.fullmatch(r'1\.0\.0-rc\.[1-9][0-9]*', approval['version'])):
        raise ValueError('full-set publication is limited to the approved 1.0.0-rc.x phase')
    main = fetch(f'repos/{REPO}/branches/main')
    if main['commit']['sha'] != request['publisher_revision'] or main.get('protected') is not True:
        raise ValueError('approved publisher is no longer protected current main')
    return release_stage.inspect_platform_request(
        {key: value for key, value in approval.items() if key != 'publication'}, fetch)


def workflow_context(request, environment=os.environ):
    required = {'GITHUB_ACTIONS': 'true', 'GITHUB_REPOSITORY': REPO,
                'GITHUB_REF': 'refs/heads/main', 'GITHUB_SHA': request['publisher_revision'],
                'GITHUB_WORKFLOW': 'Publish approved prerelease', 'GITHUB_EVENT_NAME': 'workflow_dispatch'}
    if any(environment.get(key) != value for key, value in required.items()):
        raise ValueError('full-set effects require the reviewed canonical main publication workflow')
    for key in ('GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT'):
        release_stage.positive(environment.get(key, ''))


def consumption_evidence(directory, request, fetch=release_stage.api, environment=os.environ):
    """Accept only the complete successful current-attempt read-only matrix.

    The workflow downloads these artifacts from its own run into a new directory;
    user-supplied paths/receipts cannot choose another producer run or attempt.
    """
    workflow_context(request, environment)
    run_id = release_stage.positive(environment['GITHUB_RUN_ID'])
    attempt = release_stage.positive(environment['GITHUB_RUN_ATTEMPT'])
    run = fetch(f'repos/{REPO}/actions/runs/{run_id}')
    if (str(run['id']) != run_id or str(run['run_attempt']) != attempt
            or run['repository']['full_name'] != REPO or run['head_repository']['full_name'] != REPO
            or run['head_sha'] != request['publisher_revision'] or run['head_branch'] != 'main'
            or run['event'] != 'workflow_dispatch' or run['path'] != '.github/workflows/publish.yml'):
        raise ValueError('public consumption producer differs from this canonical publication attempt')
    jobs = []
    for page in range(1, 101):
        value = fetch(f'repos/{REPO}/actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100&page={page}')
        jobs.extend(value['jobs'])
        if len(value['jobs']) < 100:
            break
    else:
        raise ValueError('public consumption job pagination exceeded limit')
    expected_jobs = {f'Public installation ({target})' for target in TARGETS}
    for name in expected_jobs:
        matches = [job for job in jobs if job['name'] == name]
        if len(matches) != 1 or matches[0]['status'] != 'completed' or matches[0]['conclusion'] != 'success':
            raise ValueError('required public installation job is absent, skipped or unsuccessful: ' + name)
    directory = Path(directory)
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError('public consumption evidence must be a regular directory')
    expected_names = {'public-consumption-' + target + '-' + attempt for target in TARGETS}
    if {path.name for path in directory.iterdir()} != expected_names:
        raise ValueError('public consumption evidence must contain exactly four target artifacts')
    reports = []
    for target in sorted(TARGETS):
        path = directory / ('public-consumption-' + target + '-' + attempt) / 'public-consumption.json'
        if path.parent.is_symlink() or path.is_symlink() or not path.is_file():
            raise ValueError('public consumption report must be a regular file')
        report = json.loads(path.read_text())
        if (report.get('schema') != 1 or report.get('target') != target or report.get('passed') is not True
                or report.get('approval_sha256') != approval_digest(request['approval'])
                or report.get('producer') != {'publisher_revision': request['publisher_revision'],
                                               'run_id': run_id, 'run_attempt': attempt}):
            raise ValueError('public consumption report differs from approved identity/producer/target')
        observed = datetime.fromisoformat(report['observed_at'].replace('Z', '+00:00'))
        if observed.tzinfo is None or not 0 <= (datetime.now(timezone.utc) - observed).total_seconds() <= 3600:
            raise ValueError('public consumption report is stale or future dated')
        checks = report.get('checks', {})
        required = {'native', 'npm_local', 'npm_global', 'npx_twigsmith', 'npx_tgi'}
        if target != 'x86_64-unknown-linux-gnu':
            required.add('browser')
        if not required.issubset(checks) or any(checks[name].get('passed') is not True for name in required):
            raise ValueError('public consumption report lacks required successful checks')
        from qualify_public_release import validate_report
        validate_report(report, request['approval'], target)
        reports.append(report)
    return reports


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['plan', 'inspect', 'acquire', 'stage', 'promote'])
    parser.add_argument('request', type=Path)
    parser.add_argument('--bundle', type=Path)
    parser.add_argument('--evidence', type=Path)
    args = parser.parse_args()
    try:
        request = json.loads(args.request.read_text())
        if args.operation != 'plan':
            workflow_context(request)
            release_stage.check_dispatch(request)
        inspect_request(request)
        base = {key: value for key, value in request['approval'].items() if key != 'publication'}
        release_stage.validate_platform_sources(base)
        # Publisher provenance is independently eligible from native/packaging source.
        spec = importlib.util.spec_from_file_location('platform_release_source', ROOT / 'scripts/ci/select-release-source.py')
        selector = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(selector)
        selector.validate_source(request['publisher_revision'], repo=REPO)
        if args.operation == 'inspect':
            print(json.dumps({'approval_sha256': approval_digest(request['approval'])}))
            return
        if args.operation == 'acquire':
            if not args.bundle:
                raise ValueError('acquisition requires --bundle')
            release_stage.acquire_platform_set(base, args.bundle)
            import release_platform_publish as platform_publish
            record, _, expected = platform_publish.approved_inputs(args.bundle / 'native', args.bundle / 'npm', request['approval'])
            providers = platform_publish.Providers(release_publish.HTTP(token=os.environ.get('GH_TOKEN')))
            state = providers.observe(record, expected)
            baseline = {'schema': 1, 'approval_sha256': approval_digest(record), 'producer': producer(),
                        'channels': advertised_channels(state), 'site_sha256': site_fingerprint()}
            validate_baseline(baseline, request, state, 'stage')
            (args.bundle / 'publication-baseline.json').write_text(json.dumps(baseline, indent=2) + '\n')
            return
        if not args.bundle:
            raise ValueError('publication requires --bundle')
        import release_platform_publish as platform_publish
        record, files, expected = platform_publish.approved_inputs(args.bundle / 'native', args.bundle / 'npm', request['approval'])
        http = release_publish.HTTP(token=os.environ.get('GH_TOKEN'))
        if args.operation != 'plan' and not http.token:
            raise ValueError('publication requires scoped GitHub credentials')
        class Providers(platform_publish.Providers):
            def effect(self, step, record, files):
                if args.operation == 'promote' and step != 'promote channel':
                    raise ValueError('promotion cannot repair missing assets or packages; rerun staged publication')
                return super().effect(step, record, files)
        providers = Providers(http)
        baseline = None
        if args.operation != 'plan':
            path = args.bundle / 'publication-baseline.json'
            if path.is_symlink() or not path.is_file():
                raise ValueError("publication requires this attempt's immutable preflight baseline")
            baseline = json.loads(path.read_text())
            if site_fingerprint() != baseline['site_sha256']:
                raise ValueError('stable Pages index changed since preflight')
        evidence = None
        if args.operation == 'promote':
            if not args.evidence:
                raise ValueError('promotion requires the complete public installation artifacts')
            reports = consumption_evidence(args.evidence, request)
            evidence = {'identity': request['approval'], 'targets': {report['target']: report for report in reports}}
        def revalidate(record, state):
            workflow_context(request)
            validate_baseline(baseline, request, state, args.operation)
            main = release_stage.api(f'repos/{REPO}/branches/main')
            if main['commit']['sha'] != request['publisher_revision'] or main.get('protected') is not True:
                raise ValueError('approved publisher is no longer protected current main')
        with release_publish.exclusive(ROOT):
            release_publish.source_eligible(ROOT, record, providers, args.operation != 'plan')
            if args.operation == 'plan':
                providers._exclusive = True
                print(json.dumps(platform_publish.resume(record, files, expected, providers), indent=2))
                return
            result = platform_publish.resume(record, files, expected, providers, execute=True,
                stage_only=args.operation == 'stage', qualify=(lambda *_: evidence) if evidence else None,
                revalidate=revalidate)
        result['stable_site_index_sha256'] = site_fingerprint()
        if result['stable_site_index_sha256'] != baseline['site_sha256']:
            raise ValueError('stable Pages index changed during publication')
        print(json.dumps(result, indent=2))
        required = 'verify public consumption' if args.operation == 'stage' else 'complete'
        if result['next'] not in (required, 'complete'):
            raise ValueError('full-set publication stopped before its required boundary')
    except (KeyError, TypeError, ValueError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
