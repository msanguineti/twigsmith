#!/usr/bin/env python3
"""Select a protected-main integration with an associated successful PR run."""
import json
import os
from pathlib import Path
import subprocess
import sys

BASELINE = '82ee0a7ad240a0fdd595b4258b35e2f702aac0a8'


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def api(path):
    return json.loads(command('gh', 'api', '--paginate', '--slurp', path))


def eligible_pull(pulls, pages, sha):
    """Require a successful foundation run for the head of the merged PR."""
    for pr in pulls:
        if (pr.get('merge_commit_sha') != sha or not pr.get('merged_at')
                or pr.get('base', {}).get('ref') != 'main'):
            continue
        number, head = pr['number'], pr['head']['sha']
        for page in pages:
            for run in page.get('workflow_runs', []):
                if (run.get('event') == 'pull_request' and run.get('head_sha') == head
                        and run.get('conclusion') == 'success'
                        and any(p.get('number') == number for p in run.get('pull_requests', []))):
                    return number, run['html_url']
    return None


def select():
    event = os.environ['EVENT_NAME']
    if event == 'workflow_dispatch':
        if os.environ['EVENT_REF'] != 'refs/heads/main':
            raise ValueError('Manual qualification must be dispatched on main')
        sha = os.environ['GITHUB_SHA']
    elif event == 'release':
        tag = os.environ.get('RELEASE_TAG', '')
        if not tag:
            raise ValueError('Release event has no tag')
        sha = command('git', 'rev-parse', '--verify', f'refs/tags/{tag}^{{commit}}')
        if sha != os.environ['GITHUB_SHA']:
            raise ValueError('Release tag does not match the event source commit')
    else:
        raise ValueError('Unsupported event')
    first_parent = command('git', 'rev-list', '--first-parent', 'refs/remotes/origin/main').splitlines()
    if sha not in first_parent or BASELINE not in first_parent or first_parent.index(sha) > first_parent.index(BASELINE):
        raise ValueError('Source must be the verified baseline or a later first-parent main integration')
    if sha == BASELINE:
        print(f'Using verified migration baseline {BASELINE}')
    else:
        repo = os.environ['GITHUB_REPOSITORY']
        pull_pages = api(f'repos/{repo}/commits/{sha}/pulls?per_page=100')
        pulls = [pull for page in pull_pages for pull in page]
        matching = [p for p in pulls if p.get('merge_commit_sha') == sha and p.get('merged_at') and p.get('base', {}).get('ref') == 'main']
        if not matching:
            raise ValueError('No merged main pull request identifies this integration')
        # Only successful runs of the source workflow for the merged PR head qualify.
        found = None
        for pr in matching:
            head = pr['head']['sha']
            pages = api(f'repos/{repo}/actions/workflows/foundation.yml/runs?head_sha={head}&event=pull_request&status=success&per_page=100')
            found = eligible_pull([pr], pages, sha)
            if found:
                break
        if not found:
            raise ValueError('No successful foundation PR run is available for this integration')
        print(f'PR #{found[0]} has a successful foundation run for its head: {found[1]}')
        print('Protected-main integration supplies the validation enforcement; this run may predate the merge.')
    Path(os.environ['RUNNER_TEMP'], 'candidate-sha').write_text(sha + '\n')
    with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
        output.write(f'sha={sha}\n')


if __name__ == '__main__':
    try:
        select()
    except (ValueError, subprocess.CalledProcessError, KeyError, json.JSONDecodeError) as error:
        print(f'::error::Release source is not eligible: {error}', file=sys.stderr)
        sys.exit(1)
