#!/usr/bin/env python3
"""Observe and rehearse full-set publication recovery on disposable providers.

Production effects require the hosted wrapper to revalidate source and approval
before each operation. Channel promotion additionally requires authenticated, fresh
four-platform public qualification. Public observation is read-only by default.
"""
from copy import deepcopy
from datetime import datetime, timezone
from pathlib import Path
import json
import os
import re
import subprocess
import tempfile
from urllib.parse import quote, urlencode

import native_package
import release_providers as shared
import release_set

PublicationError = shared.PublicationError
PACKAGE_NAMES = {'twigsmith'}


def validate_publication(approval):
    """Validate the complete independently approved identity without provider I/O."""
    approval = deepcopy(approval)
    publication = approval.pop('publication', None)
    record = release_set.validate_platform_approval(approval)
    if (not isinstance(publication, dict)
            or set(publication) != {'source', 'channel', 'staging_channel', 'prerelease', 'release_id'}
            or publication['source'] != record['native'][0]['source']['revision']
            or type(publication['release_id']) is not int or publication['release_id'] < 1
            or type(publication['prerelease']) is not bool
            or publication['prerelease'] != (shared.semver_key(record['version'])[3] == 0)
            or publication['channel'] != ('next' if publication['prerelease'] else 'latest')):
        raise PublicationError('approval must bind source, release ID and matching release class/channel')
    staging = publication['staging_channel']
    if (not isinstance(staging, str) or not re.fullmatch(r'[a-z][a-z0-9-]{0,80}', staging)
            or staging in ('latest', 'next')):
        raise PublicationError('staging channel must be explicit and distinct from advertised channels')
    record['publication'] = publication
    return record


def approved_inputs(native_dir, npm_dir, approval):
    """Verify approval and snapshot every approved byte before provider access."""
    record = validate_publication(approval)
    publication = record.pop('publication')
    native_paths, npm_paths = release_set.platform_files(native_dir, npm_dir, record)
    before = {**release_set.read_exact_files(native_dir, native_paths),
              **release_set.read_exact_files(npm_dir, npm_paths)}
    verified = release_set.verify_platform_set(native_dir, npm_dir, record)
    after = {**release_set.read_exact_files(native_dir, native_paths),
             **release_set.read_exact_files(npm_dir, npm_paths)}
    if before != after or verified != record:
        raise PublicationError('release inputs changed while taking immutable snapshot')
    files = {path.name: data for path, data in before.items()}
    if len(files) != len(before):
        raise PublicationError('release input filenames collide')
    expected = {path.name: release_set.digest(before[path]) for path in native_paths}
    # verify_platform_set validates checksum semantics via native_package.verify.
    for item in record['native']:
        stem = native_package.archive_stem(Path(item['archive']))
        checksum = (f"{item['sha256']}  {item['archive']}\n"
                    f"{item['manifest_sha256']}  {stem}.manifest.json\n").encode()
        if files[stem + '.sha256'] != checksum:
            raise PublicationError('checksum sidecar differs from approved archive/manifest')
    record['publication'] = publication
    return record, files, expected


def next_step(expected_assets, expected_packages, observed):
    """Pure decision from complete observations; conflicts anywhere precede effects."""
    if observed.get('exclusive') is not True:
        return 'stop: exclusivity not established'
    if (len(expected_assets) != 12 or set(expected_packages) != PACKAGE_NAMES
            or any(not isinstance(name, str) or not name or '/' in name
                   or not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{64}', value)
                   for name, value in {**expected_assets, **expected_packages}.items())):
        return 'stop: incomplete approved full-set identity'
    assets, packages, tags = (observed.get(key) for key in ('assets', 'packages', 'tags'))
    if (observed.get('release') != 'published' or not isinstance(assets, dict)
            or not isinstance(packages, dict) or set(packages) != PACKAGE_NAMES
            or not isinstance(tags, dict) or set(tags) != PACKAGE_NAMES
            or any(not isinstance(value, dict) or any(not isinstance(k, str) or not isinstance(v, str)
                       for k, v in value.items()) for value in tags.values())
            or observed.get('channel') not in ('expected', 'other')
            or any(value == 'unknown' or value is None for value in packages.values())):
        return 'stop: remote full-set state unknown'
    if any(name not in expected_assets or expected_assets[name] != value for name, value in assets.items()):
        return 'stop: conflicting release asset'
    if any(value != 'absent' and value != expected_packages[name] for name, value in packages.items()):
        return 'stop: conflicting npm version'
    missing = sorted(set(expected_assets) - set(assets))
    if missing:
        return 'upload ' + missing[0]
    for name in sorted(PACKAGE_NAMES):
        if packages[name] == 'absent':
            return 'publish npm ' + name
    if observed.get('consumption_verified') is not True:
        return 'verify public consumption'
    return 'complete' if observed['channel'] == 'expected' else 'promote channel'


class Providers(shared.Providers):
    """Use existing origin restrictions, anonymous npm isolation and diagnostics."""
    def observe(self, record, expected):
        if self.tag_source(record['tag']) != record['publication']['source']:
            raise PublicationError('approved remote tag is absent or changed')
        pages = self.pages('repos/' + shared.REPO + '/releases?per_page=100')
        if any(not isinstance(page, list) for page in pages):
            raise PublicationError('release listing is not a complete provider observation')
        matches = [item for page in pages for item in page if item['tag_name'] == record['tag']]
        if len(matches) != 1:
            raise PublicationError('approved published release is absent or duplicated')
        release = matches[0]
        if (release['id'] != record['publication']['release_id'] or release['draft'] is not False
                or release['prerelease'] is not record['publication']['prerelease']):
            raise PublicationError('release identity or published status conflicts')
        self.release_id = release['id']
        assets = {}
        for page in range(1, 101):
            entries = self.http.request(self.api + f'/releases/{self.release_id}/assets?per_page=100&page={page}', authenticated=True)
            if not isinstance(entries, list):
                raise PublicationError('asset observation is unavailable')
            for asset in entries:
                name = asset['name']
                if name in assets or name not in expected or asset.get('state') != 'uploaded':
                    raise PublicationError('conflicting or incomplete release asset')
                data = self.http.request(self.api + '/releases/assets/' + str(int(asset['id'])), authenticated=True, binary=True)
                assets[name] = release_set.digest(data)
            if len(entries) < 100:
                break
        else:
            raise PublicationError('asset pagination exceeded bounded limit')
        # The installer refers to anonymous release URLs. API visibility alone
        # cannot establish that consumers can fetch these exact bytes.
        if set(assets) == set(expected):
            origin = self.http.fixture or 'https://github.com'
            for name in sorted(expected):
                url = origin + '/' + shared.REPO + '/releases/download/' + quote(record['tag'], safe='') + '/' + quote(name, safe='')
                public = self.http.request(url, binary=True)
                if release_set.digest(public) != expected[name]:
                    raise PublicationError('public release asset differs from approved bytes')
        packages, tags = {}, {}
        for item in sorted(record['npm']['packages'], key=lambda item: item['name']):
            name = item['name']
            package = self.http.request(self.registry + '/' + name + '/' + quote(record['version'], safe=''), absent=True)
            packages[name] = 'absent'
            if package is not None:
                tarball = self.registry + '/' + name + '/-/' + item['filename']
                if (package.get('name') != name or package.get('version') != record['version']
                        or package.get('dist', {}).get('tarball') != tarball):
                    raise PublicationError('registry immutable package identity conflicts')
                packages[name] = release_set.digest(self.http.request(tarball, binary=True))
            channels = self.http.request(self.registry + '/-/package/' + name + '/dist-tags', absent=True)
            if channels is None and package is None:
                channels = {}
            if (not isinstance(channels, dict)
                    or any(not isinstance(k, str) or not isinstance(v, str) for k, v in channels.items())):
                raise PublicationError('npm channel observation unavailable or invalid')
            tags[name] = channels
        return {'exclusive': bool(self.http.fixture) or getattr(self, '_exclusive', False), 'release': 'published', 'assets': assets,
                'packages': packages, 'tags': tags,
                'channel': 'expected' if tags['twigsmith'].get(record['publication']['channel']) == record['version'] else 'other',
                'consumption_verified': False}

    def effect(self, step, record, files):
        if not self.http.fixture and not getattr(self, '_effect_guard', False):
            raise PublicationError('production effects require hosted revalidation')
        if step.startswith('publish npm '):
            name = step.removeprefix('publish npm ')
            item = next((item for item in record['npm']['packages'] if item['name'] == name), None)
            if item is None:
                raise PublicationError('unapproved npm package effect')
            with tempfile.TemporaryDirectory(prefix='twigsmith-platform-publish-') as temp:
                path = Path(temp, item['filename'])
                path.write_bytes(files[item['filename']])
                self.npm_command(['publish', str(path), '--ignore-scripts', '--access=public',
                                  '--tag=' + record['publication']['staging_channel'],
                                  '--provenance=false' if self.http.fixture else '--provenance'])
        elif step.startswith('upload '):
            name = step[7:]
            if name not in files:
                raise PublicationError('unapproved native asset effect')
            origin = self.http.fixture or 'https://uploads.github.com'
            self.http.request(origin + '/repos/' + shared.REPO + f'/releases/{self.release_id}/assets?'
                              + urlencode({'name': name}), method='POST', authenticated=True,
                              data=files[name])
        elif step == 'promote channel':
            self.npm_command(['dist-tag', 'add', 'twigsmith@' + record['version'], record['publication']['channel']])
        else:
            raise PublicationError('unsupported full-set effect')


def fixture_qualification(record, expected, receipt, started):
    """Validate fixture evidence only; never an authorization for production writes.

    Hosted integration must acquire and independently verify actual public runner
    evidence. Accepting a local JSON report cannot supply that capability.
    """
    if not isinstance(receipt, dict) or set(receipt) != {'fixture', 'identity', 'observed_at', 'targets'}:
        raise PublicationError('complete four-platform fixture qualification required')
    if receipt['fixture'] is not True or receipt['identity'] != record:
        raise PublicationError('qualification differs from approved fixture identity')
    try:
        observed = datetime.fromisoformat(receipt['observed_at'])
        now = datetime.now(timezone.utc)
        fresh = started <= observed <= now and (now - observed).total_seconds() <= 3600
    except (TypeError, ValueError):
        fresh = False
    if not fresh:
        raise PublicationError('fresh qualification observation required')
    rows = receipt['targets']
    if not isinstance(rows, dict) or set(rows) != release_set.TARGETS:
        raise PublicationError('all four qualification targets required')
    for item in record['native']:
        row = rows[item['target']]
        if (not isinstance(row, dict) or set(row) != {'native_sha256', 'payload_sha256', 'routes'}
                or row['native_sha256'] != expected[item['archive']]
                or row['payload_sha256'] != item['payload_sha256']
                or row['routes'] != {route: ['twigsmith', 'tgi'] for route in ('native', 'npm', 'npx')}):
            raise PublicationError('each target requires exact native/npm/npx fixture commands and hashes')
    return deepcopy(receipt)


def public_qualification(record, receipt):
    """Check reports already authenticated by the hosted same-run artifact adapter.

    This structural check alone cannot authenticate a JSON file. Production resume
    requires its caller's qualification callback and per-effect authority check.
    """
    if (not isinstance(receipt, dict) or set(receipt) != {'identity', 'targets'}
            or receipt['identity'] != record or not isinstance(receipt['targets'], dict)
            or set(receipt['targets']) != release_set.TARGETS):
        raise PublicationError('authenticated four-platform public qualification required')
    digest = release_set.digest(json.dumps(record, sort_keys=True, separators=(',', ':')).encode())
    producer = {'publisher_revision': os.environ.get('GITHUB_SHA'),
                'run_id': os.environ.get('GITHUB_RUN_ID'),
                'run_attempt': os.environ.get('GITHUB_RUN_ATTEMPT')}
    if not all(producer.values()):
        raise PublicationError('hosted qualification producer identity unavailable')
    from qualify_public_release import validate_report
    for target, row in receipt['targets'].items():
        try:
            validate_report(row, record, target)
        except (ValueError, KeyError, TypeError) as error:
            raise PublicationError('public qualification child evidence differs: ' + str(error)) from None
        if row['producer'] != producer or row['approval_sha256'] != digest:
            raise PublicationError('public qualification producer or approval identity differs')
        try:
            observed = datetime.fromisoformat(row['observed_at'].replace('Z', '+00:00'))
            age = (datetime.now(timezone.utc) - observed).total_seconds()
        except (AttributeError, TypeError, ValueError):
            age = -1
        if not 0 <= age <= 3600:
            raise PublicationError('fresh public qualification observation required')
    return deepcopy(receipt)


def validate_snapshot(record, files, expected):
    """Check immutable in-memory inputs again before every attempted effect."""
    validate_publication(record)
    hashes = {}
    for item in record['native']:
        stem = native_package.archive_stem(Path(item['archive']))
        hashes[item['archive']] = item['sha256']
        hashes[stem + '.manifest.json'] = item['manifest_sha256']
        checksum = (f"{item['sha256']}  {item['archive']}\n"
                    f"{item['manifest_sha256']}  {stem}.manifest.json\n").encode()
        hashes[stem + '.sha256'] = release_set.digest(checksum)
    if expected != hashes:
        raise PublicationError('native snapshot differs from approved identity')
    hashes['manifest.json'] = record['npm']['manifest_sha256']
    hashes.update({item['filename']: item['sha256'] for item in record['npm']['packages']})
    if set(files) != set(hashes) or any(not isinstance(files[name], bytes)
            or release_set.digest(files[name]) != digest for name, digest in hashes.items()):
        raise PublicationError('release snapshot differs from approved bytes')


def resume(record, files, expected, providers, *, execute=False, qualify=None,
           stage_only=False, revalidate=None):
    """Recover staged publication; advertised promotion needs public qualification.

    The production wrapper owns source eligibility, exclusivity and authenticating
    same-run public evidence. It must revalidate authority before each effect and
    supply qualify only after authenticating complete successful runner artifacts.
    No standalone JSON file or stored success flag authorizes promotion.
    """
    validate_snapshot(record, files, expected)
    production = not providers.http.fixture
    if execute and production and revalidate is None:
        raise PublicationError('production full-set execution requires hosted revalidation')
    expected_packages = {item['name']: item['sha256'] for item in record['npm']['packages']}
    receipt, attempted, protected = None, [], None
    started = datetime.now(timezone.utc)
    common_channel = None
    channel_initialized = False

    def observe(*, allow_promotion=False):
        nonlocal protected, common_channel, channel_initialized
        state = providers.observe(record, expected)
        if production and revalidate is not None:
            revalidate(record, deepcopy(state))
            state['exclusive'] = True
        current = {(name, tag): channels.get(tag) for name, channels in state['tags'].items()
                   for tag in ('latest', 'next')
                   if name != 'twigsmith' or tag != record['publication']['channel']}
        if protected is None:
            protected = current
        elif protected != current:
            raise PublicationError('protected advertised npm channel changed during publication')
        state['consumption_verified'] = receipt is not None
        step = next_step(expected, expected_packages, state)
        if step.startswith('stop:'):
            raise PublicationError(step)
        channel = state['tags']['twigsmith'].get(record['publication']['channel'])
        if channel_initialized and channel != common_channel:
            if not allow_promotion or channel != record['version']:
                raise PublicationError('advertised npm channel changed outside approved promotion')
        common_channel, channel_initialized = channel, True
        if (channel is not None and channel != record['version']
                and shared.semver_key(channel) >= shared.semver_key(record['version'])):
            raise PublicationError('advertised npm channel points to newer or equal-precedence version')
        return state, step

    state, step = observe()
    for _ in range(40):
        if not execute or step == 'complete' or (step == 'verify public consumption' and (stage_only or qualify is None)):
            return {'next': step, 'observed': state, 'attempted': attempted, 'receipt': receipt}
        if step == 'verify public consumption':
            evidence = qualify(record, expected, deepcopy(state))
            receipt = public_qualification(record, evidence) if production else fixture_qualification(record, expected, evidence, started)
            state, step = observe()
            continue
        validate_snapshot(record, files, expected)
        if production:
            revalidate(record, deepcopy(state))
        before_channel = state['tags']['twigsmith'].get(record['publication']['channel'])
        try:
            providers._effect_guard = True
            providers.effect(step, record, files)
            error = None
        except (PublicationError, OSError, subprocess.SubprocessError) as failure:
            error = failure
        finally:
            providers._effect_guard = False
        attempted.append(step)
        state, after_step = observe(allow_promotion=step == 'promote channel')
        after_channel = state['tags']['twigsmith'].get(record['publication']['channel'])
        allowed = (before_channel, record['version']) if step == 'promote channel' else (before_channel,)
        if after_channel not in allowed:
            raise PublicationError('advertised npm channel changed before or outside approved promotion')
        if after_step == step:
            detail = ': ' + str(error) if isinstance(error, PublicationError) else ''
            raise PublicationError('operation failed or made no observable progress; still pending: ' + step + detail)
        state, step = observe()
    raise PublicationError('full-set publication did not converge within bounded operations')
