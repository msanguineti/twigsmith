#!/usr/bin/env python3
"""Observe and resume an approved immutable prerelease (read/plan by default).

Live effects require --execute and the protected publication workflow. A fixture
endpoint is restricted to loopback and never receives live credentials. This is
not a cross-provider transaction: every attempted effect is followed by observation.
"""
import argparse
from contextlib import contextmanager, redirect_stdout
from datetime import datetime, timezone
import fcntl
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
from urllib.parse import quote, urlencode, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

import native_package
import release_recovery
import release_set

REPO = release_set.REPOSITORY
ROOT = Path(__file__).resolve().parents[1]


class PublicationError(ValueError):
    pass


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

    def observe(self, record, expected):
        tag_source = self.tag_source(record['tag'])
        if tag_source is not None and tag_source != record['publication']['source']:
            raise PublicationError('remote tag differs from approved native source')
        # Tag lookup only finds published releases. The authenticated listing
        # also includes drafts, including interrupted preparation of this tag.
        pages = self.pages('repos/' + REPO + '/releases?per_page=100')
        if any(not isinstance(page, list) for page in pages):
            raise PublicationError('release listing is not a complete provider observation')
        matches = [release for page in pages for release in page if release['tag_name'] == record['tag']]
        if len(matches) > 1:
            raise PublicationError('multiple releases conflict with the approved tag')
        release = matches[0] if matches else None
        assets = {}
        status = 'absent'
        self.release_id = None
        if release is not None:
            if release['tag_name'] != record['tag'] or release['prerelease'] is not True:
                raise PublicationError('release identity or prerelease status conflicts')
            self.release_id = int(release['id'])
            status = 'draft' if release['draft'] else 'published'
            page = 1
            while True:
                entries = self.http.request(self.api + f'/releases/{self.release_id}/assets?per_page=100&page={page}', authenticated=True)
                for asset in entries:
                    name = asset['name']
                    if name in assets or name not in expected or asset.get('state') != 'uploaded':
                        raise PublicationError('conflicting or incomplete release asset')
                    data = self.http.request(self.api + '/releases/assets/' + str(int(asset['id'])), authenticated=True, binary=True)
                    assets[name] = release_set.digest(data)
                if len(entries) < 100:
                    break
                page += 1
                if page > 100:
                    raise PublicationError('asset pagination exceeded the bounded limit')
        package = self.http.request(self.registry + '/twigsmith/' + quote(record['version'], safe=''), absent=True)
        npm = 'absent'
        if package is not None:
            if package['name'] != 'twigsmith' or package['version'] != record['version']:
                raise PublicationError('registry version identity conflicts')
            tarball = package['dist']['tarball']
            allowed = self.registry + '/twigsmith/-/' + record['npm']['package']
            if tarball != allowed:
                raise PublicationError('registry tarball URL conflicts')
            npm = release_set.digest(self.http.request(tarball, binary=True))
        tags = self.http.request(self.registry + '/-/package/twigsmith/dist-tags', absent=True) or {}
        return {'exclusive': True, 'release': status, 'assets': assets, 'npm': npm,
                'channel': 'expected' if tags.get(record['publication']['channel']) == record['version'] else 'other',
                'consumption_verified': False, 'site_promoted': False}

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
                for key in ('NODE_AUTH_TOKEN', 'NPM_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_URL'):
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

    def effect(self, step, record, files):
        if step == 'create draft':
            self.http.request(self.api + '/releases', method='POST', authenticated=True,
                              data={'tag_name': record['tag'], 'name': record['tag'], 'draft': True,
                                    'prerelease': True, 'make_latest': 'false'})
        elif step.startswith('upload '):
            name = step[7:]
            origin = self.http.fixture or 'https://uploads.github.com'
            self.http.request(origin + '/repos/' + REPO + f'/releases/{self.release_id}/assets?'
                              + urlencode({'name': name}), method='POST', authenticated=True,
                              data=files[name])
        elif step == 'publish release':
            self.http.request(self.api + f'/releases/{self.release_id}', method='PATCH', authenticated=True,
                              data={'draft': False, 'make_latest': 'false'})
        elif step == 'publish npm':
            with tempfile.TemporaryDirectory(prefix='twigsmith-publish-') as temp:
                tgz = Path(temp, record['npm']['package'])
                tgz.write_bytes(files[record['npm']['package']])
                self.npm_command(['publish', str(tgz), '--ignore-scripts', '--access=public',
                                  '--tag=' + record['publication']['staging_channel'], '--provenance'
                                  if not self.http.fixture else '--provenance=false'])
        elif step == 'promote channel':
            self.npm_command(['dist-tag', 'add', 'twigsmith@' + record['version'], record['publication']['channel']])
        else:
            raise PublicationError('unsupported publication step')

    def consume(self, record, expected):
        """Download anonymously, then execute both native/install/npm-exec names."""
        with tempfile.TemporaryDirectory(prefix='twigsmith-consume-') as temp:
            root = Path(temp)
            for name, digest in expected.items():
                url = ((self.http.fixture or 'https://github.com') + '/' + REPO + '/releases/download/'
                       + quote(record['tag'], safe='') + '/' + quote(name, safe=''))
                data = self.http.request(url, binary=True)
                if release_set.digest(data) != digest:
                    raise PublicationError('anonymous native download differs from approved bytes')
                (root / name).write_bytes(data)
            native_package.verify(root / record['native']['archive'], root / 'native')
            package = self.http.request(self.registry + '/twigsmith/-/' + record['npm']['package'], binary=True)
            if release_set.digest(package) != record['npm']['sha256']:
                raise PublicationError('anonymous npm download differs from approved bytes')
            tgz = root / record['npm']['package']
            tgz.write_bytes(package)
            prefix = root / 'install'
            self.npm_command(['install', '--prefix', str(prefix), '--ignore-scripts', '--no-audit', '--no-fund',
                              '--package-lock=false', str(tgz)], anonymous=True)
            # Fetch via real npm into a fresh cache, verify its downloaded tarball
            # before npm-exec can execute it, then prohibit further network fetches.
            npx_cache = root / 'npx-cache'
            packed = root / 'registry-package'
            packed.mkdir()
            self.npm_command(['pack', 'twigsmith@' + record['version'], '--ignore-scripts',
                              '--pack-destination', str(packed)], anonymous=True, cache=npx_cache)
            if release_set.digest((packed / record['npm']['package']).read_bytes()) != record['npm']['sha256']:
                raise PublicationError('npm registry acquisition differs from approved package')
            for name in ('twigsmith', 'tgi'):
                for binary in (root / 'native' / record['native']['archive'][:-7] / name, prefix / 'node_modules' / '.bin' / name):
                    result = subprocess.run([str(binary), '--version'], capture_output=True, text=True, timeout=20)
                    if result.returncode or result.stdout.strip() != 'twigsmith ' + record['version']:
                        raise PublicationError('downloaded or npm-installed command failed version observation')
                output = self.npm_command(['exec', '--offline', '--yes', '--ignore-scripts', '--package=' + str(packed / record['npm']['package']),
                                           '--', name, '--version'], anonymous=True, cache=npx_cache)
                if output.strip() != 'twigsmith ' + record['version']:
                    raise PublicationError('registry npm-exec command failed version observation')
        return {'identity': record, 'native_sha256': record['native']['sha256'], 'npm_sha256': record['npm']['sha256'],
                'observed_at': datetime.now(timezone.utc).isoformat(),
                'consumption': {kind: {'version': record['version'], 'commands': ['twigsmith', 'tgi']}
                                for kind in ('native', 'npm', 'npx')}}


def approved_inputs(archive, npm_dir, approval):
    publication = approval['publication']
    if (set(publication) != {'source', 'channel', 'staging_channel', 'prerelease'}
            or not re.fullmatch(r'[0-9a-f]{40}', publication['source'])
            or publication['source'] != approval['native']['source']['revision']
            or publication['prerelease'] is not True or '-' not in approval['version']):
        raise PublicationError('approval must bind a source and prerelease target')
    for channel in (publication['channel'], publication['staging_channel']):
        if not re.fullmatch(r'[a-z][a-z0-9-]{0,80}', channel) or channel == 'latest':
            raise PublicationError('approval must select explicit non-latest prerelease channels')
    if publication['channel'] == publication['staging_channel']:
        raise PublicationError('staging and advertised channels must differ')
    record = release_set.verify(archive, npm_dir, approval['native']['sha256'],
        approval['native']['payload_sha256'], approval['npm']['sha256'], approval['tag'],
        approval['native']['manifest_sha256'], approval['npm']['manifest_sha256'])
    if record != {k: v for k, v in approval.items() if k != 'publication'}:
        raise PublicationError('approval differs from verified release identity')
    record['publication'] = publication
    archive = Path(archive)
    stem = archive.name[:-7]
    paths = [archive, archive.with_name(stem + '.manifest.json'), archive.with_name(stem + '.sha256')]
    files = {}
    for path in paths + [Path(npm_dir, record['npm']['package'])]:
        if path.is_symlink() or not path.is_file():
            raise PublicationError('release input must be a regular file')
        files[path.name] = path.read_bytes()
    expected = {path.name: release_set.digest(files[path.name]) for path in paths}
    if (expected[archive.name] != record['native']['sha256']
            or expected[stem + '.manifest.json'] != record['native']['manifest_sha256']
            or release_set.digest(files[record['npm']['package']]) != record['npm']['sha256']):
        raise PublicationError('release inputs changed while taking immutable snapshot')
    checksum = (f"{expected[archive.name]}  {archive.name}\n"
                f"{expected[stem + '.manifest.json']}  {stem}.manifest.json\n").encode()
    if files[stem + '.sha256'] != checksum:
        raise PublicationError('checksum asset is not the exact approved archive/manifest sidecar')
    return record, files, expected


@contextmanager
def exclusive(repo):
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


def resume(record, files, expected, providers, *, execute=False, promote=None, site_observed=None):
    receipt = None
    attempted = []
    for _ in range(20):
        state = providers.observe(record, expected)
        state['consumption_verified'] = receipt is not None
        state['site_promoted'] = bool(receipt and site_observed and site_observed(record, receipt))
        step = release_recovery.next_step(expected, record['npm']['sha256'], state)
        if execute and providers.tag_source(record['tag']) != record['publication']['source']:
            raise PublicationError('approved remote tag is absent or changed')
        if step.startswith('stop:'):
            raise PublicationError(step)
        if step == 'complete' or not execute:
            return {'next': step, 'observed': state, 'attempted': attempted, 'receipt': receipt}
        if step == 'verify public consumption':
            receipt = providers.consume(record, expected)
            continue
        if step == 'promote site' and promote is None:
            return {'next': step, 'observed': state, 'attempted': attempted, 'receipt': receipt}
        # Re-observe even after a rejection/timeout. If no progress occurred, stop;
        # another invocation may retry only from its fresh observations.
        try:
            if step == 'promote site':
                promote(record, receipt)
            else:
                providers.effect(step, record, files)
            error = None
        except (PublicationError, OSError, subprocess.SubprocessError) as failure:
            error = failure
        attempted.append(step)
        after = providers.observe(record, expected)
        if error:
            after['consumption_verified'] = receipt is not None
            after['site_promoted'] = bool(receipt and site_observed and site_observed(record, receipt))
            if release_recovery.next_step(expected, record['npm']['sha256'], after) == step:
                detail = ': ' + str(error) if isinstance(error, PublicationError) else ''
                raise PublicationError('operation failed or timed out; fresh observation confirms it is still pending: ' + step + detail) from None
        if len(attempted) > 1 and attempted[-1] == attempted[-2]:
            raise PublicationError('operation did not make observable progress')
    raise PublicationError('publication did not converge within bounded operations')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive')
    parser.add_argument('npm_dir')
    parser.add_argument('--approval', required=True)
    parser.add_argument('--repo', default=str(ROOT))
    parser.add_argument('--execute', action='store_true')
    parser.add_argument('--fixture-api', help='disposable loopback fixture only; no live credentials')
    parser.add_argument('--site-directory')
    parser.add_argument('--receipt-output')
    args = parser.parse_args()
    try:
        approval = json.loads(Path(args.approval).read_text())
        record, files, expected = approved_inputs(args.archive, args.npm_dir, approval)
        http = HTTP(token=os.environ.get('GH_TOKEN'), fixture=args.fixture_api)
        providers = Providers(http)
        if args.execute and not http.fixture and not os.environ.get('GH_TOKEN'):
            raise PublicationError('live execution requires scoped GitHub credentials')
        if args.execute and not http.fixture and (os.environ.get('GITHUB_ACTIONS') != 'true'
                or os.environ.get('GITHUB_REPOSITORY') != REPO
                or os.environ.get('GITHUB_WORKFLOW') != 'Publish approved prerelease'):
            raise PublicationError('live execution requires the protected Publish approved prerelease workflow')
        with exclusive(args.repo):
            tag_exists = source_eligible(args.repo, record, providers, args.execute)
            promote = observed = None
            if args.site_directory:
                spec = importlib.util.spec_from_file_location('release_site', ROOT / 'site/build.py')
                site = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(site)
                promote = lambda identity, receipt: site.build(Path(args.site_directory), identity, receipt=receipt)
                observed = lambda identity, receipt: site.promotion_matches(Path(args.site_directory), identity, receipt)
            result = resume(record, files, expected, providers, execute=args.execute,
                            promote=promote, site_observed=observed)
            result['tag_exists'] = tag_exists
            result['site_artifact_prepared'] = result['observed'].pop('site_promoted')
            if result['next'] == 'complete':
                result['next'] = 'site artifact prepared; Pages deployment remains separate'
            if args.receipt_output and result['receipt']:
                Path(args.receipt_output).write_text(json.dumps(result['receipt'], indent=2) + '\n')
            print(json.dumps(result, indent=2))
    except subprocess.SubprocessError:
        parser.exit(1, 'error: required local command failed\n')
    except (ValueError, KeyError, TypeError, OSError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
