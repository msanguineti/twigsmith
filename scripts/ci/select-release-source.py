#!/usr/bin/env python3
"""Validate protected-main integrations and their successful source PR runs."""
import json
import os
import subprocess

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
        branch = pr['head'].get('ref')
        repository = (pr['head'].get('repo') or {}).get('full_name')
        if not head or not branch or not repository:
            continue
        for page in pages:
            for run in page.get('workflow_runs', []):
                if (run.get('event') == 'pull_request' and run.get('head_sha') == head
                        and run.get('conclusion') == 'success'
                        and run.get('head_branch') == branch
                        and (run.get('head_repository') or {}).get('full_name') == repository):
                    return number, run['html_url']
    return None


def validate_source(sha, command=None, api=None, repo=None):
    """Validate an explicit native source using the existing protected-main rule."""
    cmd = command or globals()['command']
    fetch = api or globals()['api']
    first_parent = cmd('git', 'rev-list', '--first-parent', 'refs/remotes/origin/main').splitlines()
    if sha not in first_parent or BASELINE not in first_parent or first_parent.index(sha) > first_parent.index(BASELINE):
        raise ValueError('Source must be the verified baseline or a later first-parent main integration')
    if sha == BASELINE:
        print(f'Using verified migration baseline {BASELINE}')
        evidence = {'source': sha, 'baseline': True}
    else:
        repo = repo or os.environ['GITHUB_REPOSITORY']
        pull_pages = fetch(f'repos/{repo}/commits/{sha}/pulls?per_page=100')
        pulls = [pull for page in pull_pages for pull in page]
        matching = [p for p in pulls if p.get('merge_commit_sha') == sha and p.get('merged_at') and p.get('base', {}).get('ref') == 'main']
        if not matching:
            raise ValueError('No merged main pull request identifies this integration')
        # Only successful runs of the source workflow for the merged PR head qualify.
        found = None
        for pr in matching:
            head = pr['head']['sha']
            pages = fetch(f'repos/{repo}/actions/workflows/foundation.yml/runs?head_sha={head}&event=pull_request&status=success&per_page=100')
            found = eligible_pull([pr], pages, sha)
            if found:
                break
        if not found:
            raise ValueError('No successful foundation PR run is available for this integration')
        print(f'PR #{found[0]} has a successful foundation run for its head: {found[1]}')
        print('Protected-main integration supplies the validation enforcement; this run may predate the merge.')
        evidence = {'source': sha, 'pull_request': found[0], 'run_url': found[1]}
    return evidence
