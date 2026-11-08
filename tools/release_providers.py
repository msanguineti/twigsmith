#!/usr/bin/env python3
"""Shared provider boundaries for the four-native, single-npm release pipeline.

HTTP origin and credential restrictions, isolated npm commands, source eligibility
and repository locking are shared by preparation and hosted publication. Publication
observation, recovery decisions and effects belong to release_platform_publish.
"""
from contextlib import contextmanager, redirect_stdout
from http.client import HTTPException
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

import release_set

REPO = release_set.REPOSITORY
ROOT = Path(__file__).resolve().parents[1]


class PublicationError(ValueError):
    pass


def semver_key(version):
    """Return a comparable SemVer precedence key; reject ambiguous registry values."""
    match = re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?', version)
    if not match:
        raise PublicationError('npm channel version is not valid SemVer')
    prerelease = match.group(4)
    identifiers = []
    if prerelease is not None:
        for part in prerelease.split('.'):
            if not part or (part.isdigit() and len(part) > 1 and part.startswith('0')):
                raise PublicationError('npm channel version is not valid SemVer')
            identifiers.append((0, int(part)) if part.isdigit() else (1, part))
    return (int(match.group(1)), int(match.group(2)), int(match.group(3)),
            1 if prerelease is None else 0, tuple(identifiers))


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def fixture_origin(value):
    parsed = urlsplit(value)
    if (parsed.scheme != 'http' or parsed.hostname not in ('127.0.0.1', '[::1]', '::1')
            or not parsed.port or parsed.username or parsed.password
            or parsed.path not in ('', '/') or parsed.query or parsed.fragment):
        raise PublicationError('fixture endpoint must be an explicit loopback HTTP origin')
    return value.rstrip('/')


class HTTP:
    def __init__(self, token=None, fixture=None, timeout=30):
        self.fixture = fixture_origin(fixture) if fixture else None
        if self.fixture and token:
            raise PublicationError('live credentials are forbidden with fixture endpoints')
        self.token, self.timeout = token, timeout
        self.opener = build_opener(NoRedirect)

    def request(self, url, *, method='GET', data=None, authenticated=False,
                binary=False, absent=False, redirects=0):
        p = urlsplit(url)
        if self.fixture:
            allowed = url.startswith(self.fixture + '/')
        else:
            allowed = (p.scheme == 'https' and not p.username and not p.password
                       and p.port in (None, 443) and p.hostname in
                       ('api.github.com', 'uploads.github.com', 'github.com',
                        'release-assets.githubusercontent.com', 'registry.npmjs.org'))
        if not allowed or p.fragment:
            raise PublicationError('provider URL is outside the allowed origins')
        if authenticated and not self.fixture and p.hostname not in ('api.github.com', 'uploads.github.com'):
            raise PublicationError('GitHub credentials cannot be sent to this origin')
        headers = {'Accept': 'application/octet-stream' if binary else 'application/json',
                   'User-Agent': 'twigsmith-release'}
        if authenticated and self.token:
            headers['Authorization'] = 'Bearer ' + self.token
        if data is not None:
            headers['Content-Type'] = 'application/octet-stream' if isinstance(data, bytes) else 'application/json'
            if not isinstance(data, bytes):
                data = json.dumps(data).encode()
        try:
            with self.opener.open(Request(url, data=data, method=method, headers=headers), timeout=self.timeout) as response:
                body = response.read()
                return body if binary else (json.loads(body) if body else None)
        except HTTPError as error:
            if error.code == 404 and absent:
                return None
            # Redirects are only allowed for downloads, with credentials removed.
            if error.code in (301, 302, 303, 307, 308) and binary and method == 'GET' and redirects < 3:
                target = error.headers.get('Location', '')
                if self.fixture or urlsplit(target).hostname == 'release-assets.githubusercontent.com':
                    return self.request(target, binary=True, redirects=redirects + 1)
            raise PublicationError(f'provider request rejected (HTTP {error.code})') from None
        except (URLError, TimeoutError, OSError, HTTPException, json.JSONDecodeError):
            raise PublicationError('provider observation or request unavailable') from None


class Providers:
    def __init__(self, http, npm='npm'):
        self.http, self.npm = http, npm
        self.api = (http.fixture or 'https://api.github.com') + '/repos/' + REPO
        self.registry = http.fixture or 'https://registry.npmjs.org'
        self.release_id = None

    def pages(self, path):
        results = []
        for page in range(1, 101):
            sep = '&' if '?' in path else '?'
            value = self.http.request((self.http.fixture or 'https://api.github.com') + '/' + path
                                      + sep + 'page=' + str(page), authenticated=True)
            results.append(value)
            items = value.get('workflow_runs', []) if isinstance(value, dict) else value
            if len(items) < 100:
                return results
        raise PublicationError('provider pagination exceeded the bounded limit')

    def tag_source(self, tag):
        obj = self.http.request(self.api + '/git/ref/tags/' + quote(tag, safe=''), authenticated=True, absent=True)
        if obj is None:
            return None
        obj = obj['object']
        for _ in range(5):
            if obj['type'] == 'commit':
                return obj['sha']
            if obj['type'] != 'tag' or not re.fullmatch(r'[0-9a-f]{40}', obj['sha']):
                break
            obj = self.http.request(self.api + '/git/tags/' + obj['sha'], authenticated=True)['object']
        raise PublicationError('tag does not resolve to a bounded commit identity')

    def npm_command(self, arguments, *, cwd=None, anonymous=False, cache=None):
        environment = os.environ.copy()
        # Configuration is isolated from project/global npmrc, credentials stay in env.
        with tempfile.TemporaryDirectory(prefix='twigsmith-npm-') as temp:
            config = Path(temp, 'npmrc')
            config.write_text('')
            global_config = Path(temp, 'global-npmrc')
            global_config.write_text('')
            environment = {k: v for k, v in environment.items() if not k.lower().startswith('npm_config_')}
            environment.update(NPM_CONFIG_USERCONFIG=str(config), NPM_CONFIG_GLOBALCONFIG=str(global_config),
                               NPM_CONFIG_CACHE=str(cache or Path(temp, 'cache')), NPM_CONFIG_REGISTRY=self.registry,
                               NPM_CONFIG_LOGLEVEL='verbose', NPM_CONFIG_LOGS_MAX='0',
                               NPM_CONFIG_FETCH_RETRIES='0', NPM_CONFIG_FETCH_TIMEOUT='30000')
            if self.http.fixture:
                environment.pop('NODE_AUTH_TOKEN', None)
                environment.pop('NPM_TOKEN', None)
                config.write_text('//' + urlsplit(self.registry).netloc + '/:_authToken=disposable-fixture\n')
            elif not anonymous and environment.get('NODE_AUTH_TOKEN'):
                config.write_text('//registry.npmjs.org/:_authToken=${NODE_AUTH_TOKEN}\n')
            if anonymous:
                for key in list(environment):
                    if key in ('GH_TOKEN', 'GITHUB_TOKEN', 'NODE_AUTH_TOKEN', 'NPM_TOKEN') or key.startswith('ACTIONS_ID_TOKEN_'):
                        environment.pop(key, None)
            try:
                result = subprocess.run([self.npm, '--registry=' + self.registry, *arguments],
                                        cwd=cwd or temp, env=environment, capture_output=True, text=True, timeout=180)
            except (OSError, subprocess.TimeoutExpired):
                raise PublicationError('npm operation unavailable or timed out; remote state must be observed') from None
            if result.returncode:
                # npm's OIDC exchange failures are verbose-only. Report fixed
                # classifications, never its raw output (which can contain URLs,
                # credentials or provider-controlled error text).
                details = ['exit ' + str(result.returncode)]
                codes = sorted(set(re.findall(r'^npm (?:error|ERR!) code (E[A-Z0-9_]+)\s*$', result.stderr, re.M)))
                if codes:
                    known = {'E400', 'E401', 'E403', 'E404', 'E409', 'E422', 'E429', 'E500', 'E502', 'E503', 'E504',
                             'EAI_AGAIN', 'ECONNREFUSED', 'ECONNRESET', 'ENEEDAUTH', 'ENOTFOUND', 'EOTP',
                             'EPUBLISHCONFLICT', 'ESOCKETTIMEDOUT', 'ETIMEDOUT', 'EUSAGE'}
                    labels = sorted({code if code in known else 'unrecognized' for code in codes})
                    details.append('code ' + ','.join(labels))
                statuses = sorted(set(re.findall(r'^npm http fetch (?:GET|POST|PUT|DELETE) ([1-5][0-9]{2})\b', result.stderr, re.M)))
                if statuses:
                    details.append('HTTP ' + ','.join(statuses))
                for marker, label in (
                    ('Successfully retrieved and set token', 'OIDC token acquired'),
                    ('Failed token exchange request', 'OIDC exchange rejected'),
                    ('Failed to fetch id_token', 'OIDC identity request failed'),
                    ('Skipped because incorrect permissions for id-token', 'OIDC identity permission absent'),
                    ('Signed provenance statement', 'provenance signed'),
                ):
                    if marker in result.stderr:
                        details.append(label)
                words = ('provenance', 'repository', 'workflow', 'environment', 'permission', 'expired', 'sigstore', 'fulcio')
                mentioned = [word for word in words if word in result.stderr.lower()]
                if mentioned:
                    details.append('mentions ' + ','.join(mentioned))
                raise PublicationError('npm operation failed (' + '; '.join(details) + '); remote state must be observed')
            return result.stdout


@contextmanager
def exclusive(repo):
    # Hosted effects execute on Linux. Import the Unix lock implementation only
    # when acquiring the lock; read-only consumers also run on Windows/macOS.
    import fcntl
    git_dir = subprocess.check_output(['git', '-C', str(repo), 'rev-parse', '--git-common-dir'], text=True).strip()
    lock = Path(repo, git_dir, 'twigsmith-publication.lock')
    with lock.open('a') as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise PublicationError('another publication invocation holds the repository lock') from None
        yield


def source_eligible(repo, record, providers, execute):
    spec = importlib.util.spec_from_file_location('release_source', ROOT / 'scripts/ci/select-release-source.py')
    selector = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(selector)
    def git(*args):
        return subprocess.check_output([args[0], '-C', str(repo), *args[1:]], text=True).strip()
    with redirect_stdout(sys.stderr):
        selector.validate_source(record['publication']['source'], command=git, api=providers.pages, repo=REPO)
    local = subprocess.run(['git', '-C', str(repo), 'rev-parse', '--verify',
                            'refs/tags/' + record['tag'] + '^{commit}'], capture_output=True, text=True)
    if local.returncode == 0 and local.stdout.strip() != record['publication']['source']:
        raise PublicationError('local tag differs from approved native source')
    remote_source = providers.tag_source(record['tag'])
    if remote_source is not None and remote_source != record['publication']['source']:
        raise PublicationError('remote tag differs from approved publication source')
    if execute and remote_source is None:
        raise PublicationError('approved tag must already exist; publication never creates a tag')
    return remote_source is not None
