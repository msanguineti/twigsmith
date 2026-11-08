"""Check the static boundaries of the two website publication entry points.

These checks do not emulate GitHub Actions or prove hosted permissions and effects.
"""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parent.parent
WORKFLOWS = ROOT / '.github' / 'workflows'


def block(source, heading, indent=0):
    """Select one indentation-delimited YAML mapping entry by its exact key."""
    prefix = ' ' * indent
    lines = source.splitlines()
    matches = [i for i, line in enumerate(lines) if line == prefix + heading + ':']
    if len(matches) != 1:
        raise AssertionError(f'expected one {heading} block at indentation {indent}')
    start = matches[0]
    end = start + 1
    while end < len(lines):
        line = lines[end]
        if line.strip() and not line.lstrip().startswith('#') and len(line) - len(line.lstrip()) <= indent:
            break
        end += 1
    return '\n'.join(lines[start:end]) + '\n'


def permissions(source, indent):
    section = block(source, 'permissions', indent)
    values = {}
    for line in section.splitlines()[1:]:
        if not line.strip():
            continue
        match = re.fullmatch(r'\s+([a-z-]+): (read|write|none)', line)
        if not match:
            raise AssertionError(f'unexpected permissions syntax: {line!r}')
        values[match.group(1)] = match.group(2)
    return values


class SiteWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.site = (WORKFLOWS / 'site.yml').read_text()
        cls.publication = (WORKFLOWS / 'publish.yml').read_text()
        cls.site_job = block(block(cls.site, 'jobs'), 'deploy', 2)
        cls.preflight = block(block(cls.publication, 'jobs'), 'platform-preflight', 2)
        cls.publish_job = block(block(cls.publication, 'jobs'), 'platform-stage', 2)
        cls.promote_job = block(block(cls.publication, 'jobs'), 'platform-promote', 2)

    def test_manual_workflow_has_only_website_authority(self):
        triggers = block(self.site, 'on')
        self.assertIn('  workflow_dispatch:\n', triggers)
        self.assertNotIn('  workflow_call:\n', triggers)
        self.assertEqual(permissions(self.site, 0), {'contents': 'read'})
        self.assertEqual(permissions(self.site_job, 4),
                         {'contents': 'read', 'pages': 'write', 'id-token': 'write'})
        self.assertIn("    if: github.ref == 'refs/heads/main'\n", self.site_job)
        self.assertIn('          persist-credentials: false\n', self.site_job)
        self.assertNotIn('environment: release', self.site_job)
        self.assertNotRegex(self.site_job, r'(?m)^\s*(?:contents|packages|actions): write\s*$')
        self.assertNotRegex(self.site_job, r'(?m)^\s*run:.*(?:release_publish|npm publish|dist-tag)')

    def test_shared_site_job_serializes_and_rechecks_before_deploy(self):
        self.assertIn('  group: twigsmith-release\n', block(self.publication, 'concurrency'))
        self.assertIn('      group: twigsmith-pages\n', block(self.site_job, 'concurrency', 4))
        self.assertIn('      cancel-in-progress: false\n', block(self.site_job, 'concurrency', 4))
        self.assertIn('          ref: ${{ github.sha }}\n', self.site_job)
        self.assertIn('      name: github-pages\n', self.site_job)
        steps = ('site/publish.py prepare', 'actions/upload-pages-artifact@',
                 'site/publish.py recheck', 'actions/deploy-pages@')
        offsets = [self.site_job.index(step) for step in steps]
        self.assertEqual(offsets, sorted(offsets))
        self.assertEqual(len(set(offsets)), len(offsets))
        self.assertNotIn('continue-on-error:', self.site_job)

    def test_release_creation_cannot_trigger_preparation_or_publication(self):
        release = (WORKFLOWS / 'release.yml').read_text()
        for workflow in (release, self.publication):
            triggers = block(workflow, 'on')
            self.assertIn('  workflow_dispatch:\n', triggers)
            self.assertNotIn('  workflow_run:', triggers)
            self.assertNotIn('  release:', triggers)
        self.assertIn('default: platform-packages', release)
        self.assertNotIn('linux-release', release)
        self.assertNotIn('qualify-candidate.sh', release)
        self.assertNotIn('twigsmith-release-request', self.publication)
        self.assertIn('options: [publish-platforms, promote-platforms]', self.publication)
        for removed in ('prepare', 'preflight', 'publish', 'deploy-pages'):
            self.assertNotIn('  ' + removed + ':\n', block(self.publication, 'jobs'))
        self.assertNotIn('uses: ./.github/workflows/site.yml', self.publication)
        self.assertNotIn('pages: write', self.publication)

    def test_publication_authority_is_separate_from_four_host_promotion_gate(self):
        self.assertIn("github.ref == 'refs/heads/main'", self.preflight)
        self.assertEqual(permissions(self.preflight, 4),
                         {'contents': 'read', 'actions': 'read', 'pull-requests': 'read'})
        self.assertIn('release_platform_dispatch.py acquire', self.preflight)
        self.assertIn('    needs: platform-preflight\n', self.publish_job)
        self.assertIn("    if: inputs.operation == 'publish-platforms'\n", self.publish_job)
        self.assertIn('    environment: release\n', self.publish_job)
        self.assertEqual(permissions(self.publish_job, 4),
                         {'contents': 'write', 'actions': 'read',
                          'pull-requests': 'read', 'id-token': 'write'})
        self.assertIn('release_platform_dispatch.py stage', self.publish_job)
        self.assertIn('    needs: [platform-preflight, platform-consume]\n', self.promote_job)
        self.assertIn("inputs.operation == 'promote-platforms'", self.promote_job)
        self.assertIn("needs.platform-consume.result == 'success'", self.promote_job)
        self.assertIn('    environment: release\n', self.promote_job)
        self.assertEqual(permissions(self.promote_job, 4),
                         {'contents': 'read', 'actions': 'read',
                          'pull-requests': 'read', 'id-token': 'write'})
        self.assertIn('release_platform_dispatch.py promote', self.promote_job)
        consume = block(block(self.publication, 'jobs'), 'platform-consume', 2)
        self.assertEqual(permissions(consume, 4), {'contents': 'read', 'actions': 'read'})
        for host in ('ubuntu-22.04', 'windows-2025', 'macos-15', 'macos-15-intel'):
            self.assertIn('os: ' + host + '\n', consume)
        self.assertIn("needs.platform-stage.result == 'success'", consume)
        self.assertIn("needs.platform-stage.result == 'skipped'", consume)

    def test_installation_dispatch_is_read_only_and_separate_from_release_preparation(self):
        release = (WORKFLOWS / 'release.yml').read_text()
        installation = (WORKFLOWS / 'platform-installation.yml').read_text()
        jobs = block(release, 'jobs')
        caller = block(jobs, 'platform-installation', 2)
        packages = block(jobs, 'platform-packages', 2)
        self.assertIn("inputs.qualification == 'platform-installation'", caller)
        self.assertIn("inputs.qualification == 'platform-packages'", packages)
        self.assertEqual(permissions(caller, 4), {'contents': 'read', 'actions': 'read'})
        self.assertEqual(permissions(installation, 0), {'contents': 'read', 'actions': 'read'})
        self.assertNotRegex(installation, r'(?m)^\s*[a-z-]+: write\s*$')
        self.assertNotIn('windows-11', installation)
        for host in ('windows-2025', 'macos-15', 'macos-15-intel'):
            self.assertIn('os: ' + host + '\n', installation)
        self.assertIn('release_stage.inspect_platform_request(approval)', installation)
        self.assertIn('APPROVAL_JSON: ${{ inputs.approval }}', installation)
        self.assertNotIn("json.loads('${{", installation)
        self.assertNotRegex(installation, r'npm publish|release_publish.py|deploy-pages@')


if __name__ == '__main__':
    unittest.main()
