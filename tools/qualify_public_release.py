#!/usr/bin/env python3
"""Consume exact approved public release bytes without publication credentials.

Run on each native host after publication and before advertised-channel promotion.
Prepared, source-bound child fixtures are required; this tool never compiles them.
"""
import argparse
import contextlib
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import urllib.parse
import urllib.request

import native_package
import npm_platform_package
import qualify_installation
import qualify_platform
import release_set
from release_platform_publish import validate_publication


JOURNEY = {'native_journey': ['native_start_status_journey']}
REGISTRY = 'https://registry.npmjs.org/'
ALLOWED_ENV = {'PATH', 'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'PATHEXT', 'TEMP', 'TMP',
               'PROGRAMFILES(X86)', 'PROGRAMFILES', 'LOCALAPPDATA',
               'DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY', 'XDG_RUNTIME_DIR',
               '__CF_USER_TEXT_ENCODING'}


def canonical_hash(approval):
    return hashlib.sha256(json.dumps(approval, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def approved(approval, target):
    # Publication is operational approval, not part of the frozen schema-2 byte
    # contract. Never drop another field or drop publication from the report hash.
    approval = validate_publication(approval)
    core = {key: value for key, value in approval.items() if key != 'publication'}
    core = release_set.validate_platform_approval(core)
    if target not in npm_platform_package.PACKAGES:
        raise ValueError('unsupported public qualification target')
    return core, next(item for item in core['native'] if item['target'] == target)


def anonymous_environment(root):
    """Allowlist host mechanics; create fresh homes and all npm configuration.

    No GitHub/npm/OIDC/Actions runtime credentials, credential helpers, user
    npmrc, proxies, Python startup configuration or Git configuration are inherited.
    """
    root.mkdir(parents=True, exist_ok=True)
    home = root / 'home'
    home.mkdir()
    npmrc = home / 'npmrc'
    npmrc.write_text('', encoding='utf-8')
    globalrc = home / 'global-npmrc'
    globalrc.write_text('', encoding='utf-8')
    env = {key: value for key, value in os.environ.items() if key.upper() in ALLOWED_ENV}
    env.update(HOME=str(home), USERPROFILE=str(home), XDG_CONFIG_HOME=str(home / 'config'),
               XDG_CACHE_HOME=str(home / 'cache'), GIT_CONFIG_NOSYSTEM='1',
               GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT='0',
               npm_config_userconfig=str(npmrc), npm_config_globalconfig=str(globalrc),
               npm_config_cache=str(home / 'npm-cache'), npm_config_registry=REGISTRY,
               npm_config_ignore_scripts='true', npm_config_offline='false',
               npm_config_audit='false', npm_config_fund='false', npm_config_update_notifier='false',
               LC_ALL='C', TZ='UTC')
    return env


@contextlib.contextmanager
def isolated_environment(env):
    previous = dict(os.environ)
    os.environ.clear()
    os.environ.update(env)
    try:
        yield
    finally:
        os.environ.clear()
        os.environ.update(previous)


def release_url(approval, filename):
    if Path(filename).name != filename or filename in ('', '.', '..'):
        raise ValueError('unsafe public release filename')
    return (f"https://github.com/{release_set.REPOSITORY}/releases/download/"
            f"{urllib.parse.quote(approval['tag'], safe='')}/{urllib.parse.quote(filename, safe='')}")


def validate_download_url(url, hosts):
    parsed = urllib.parse.urlsplit(url)
    if (parsed.scheme != 'https' or parsed.hostname not in hosts or parsed.username
            or parsed.password or parsed.port not in (None, 443) or parsed.fragment):
        raise ValueError('public download left its allowed HTTPS origin')


class PublicRedirect(urllib.request.HTTPRedirectHandler):
    def __init__(self, hosts):
        self.hosts = hosts

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        validate_download_url(newurl, self.hosts)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def public_download(url, destination, expected=None, *, registry=False):
    hosts = {'registry.npmjs.org'} if registry else {
        'github.com', 'release-assets.githubusercontent.com', 'objects.githubusercontent.com'}
    validate_download_url(url, hosts)
    # Ignore host proxy/auth configuration. Neither an auth handler nor a cookie
    # jar is installed, and requests contain no publication identity.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), PublicRedirect(hosts))
    request = urllib.request.Request(url, headers={'User-Agent': 'twigsmith-public-consumption'})
    temporary = destination.with_name(destination.name + '.partial')
    try:
        with opener.open(request, timeout=120) as response, temporary.open('xb') as output:
            validate_download_url(response.geturl(), hosts)
            if response.status != 200:
                raise ValueError('public download did not return HTTP 200')
            shutil.copyfileobj(response, output)
        if expected is not None and qualify_platform.digest(temporary) != expected:
            raise ValueError('public download hash differs from approval')
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)


def expected_identity(approval, target, kind):
    _, item = approved(approval, target)
    identity = {'version': approval['version'], 'payload_sha256': item['payload_sha256']}
    if kind in ('native', 'browser'):
        identity.update(archive_sha256=item['sha256'], manifest_sha256=item['manifest_sha256'])
    else:
        names = {'twigsmith', npm_platform_package.PACKAGES[target]}
        identity['package_sha256'] = {row['name']: row['sha256'] for row in approval['npm']['packages']
                                      if row['name'] in names}
    return identity


def expected_cases(kind):
    return qualify_platform.npm_cases('launcher') if kind == 'npm_global' else JOURNEY


def require_cases(report, selected):
    expected = {(suite, name) for suite, names in selected.items() for name in names}
    rows = report.get('tests')
    if (not expected or not isinstance(rows, list) or len(rows) != len(expected)
            or any(not isinstance(row, dict) for row in rows)):
        raise ValueError('public qualification lacks required actual child cases')
    actual = [(row.get('suite'), row.get('name')) for row in rows]
    if (set(actual) != expected or len(set(actual)) != len(actual)
            or any(row.get('passed') is not True or type(row.get('exit')) is not int or row['exit'] != 0
                   for row in rows)):
        raise ValueError('public qualification child cases failed, skipped or differ from selection')
    return [{key: row[key] for key in ('suite', 'name', 'passed', 'exit')} for row in rows]


def validate_report(report, approval, target):
    """Validate the compact promotion boundary independently of aggregate flags.

    Hosted delivery must additionally authenticate successful same-run artifacts
    and enforce producer/freshness; this pure check cannot authenticate its input.
    """
    approved(approval, target)
    keys = {'native', 'npm_global', 'npx_twigsmith', 'npx_tgi'}
    if not target.endswith('linux-gnu'):
        keys.add('browser')
    if (not isinstance(report, dict) or set(report) != {'schema', 'approval_sha256', 'target',
            'observed_at', 'producer', 'checks', 'passed'} or type(report['schema']) is not int
            or report['schema'] != 1 or report['approval_sha256'] != canonical_hash(approval)
            or report['target'] != target or report['passed'] is not True
            or not isinstance(report['checks'], dict) or set(report['checks']) != keys
            or not isinstance(report['producer'], dict)
            or set(report['producer']) != {'publisher_revision', 'run_id', 'run_attempt'}):
        raise ValueError('invalid or incomplete public consumption report')
    for kind, check in report['checks'].items():
        fields = {'passed', 'report', 'cases', 'identity', 'aliases'}
        if kind.startswith('npx_'):
            fields |= {'cold_cache', 'invoked_alias'}
        path = f'{kind}/' + ('installation.json' if kind == 'browser' else 'qualification.json')
        if (not isinstance(check, dict) or set(check) != fields or check['passed'] is not True
                or check['report'] != path or check['identity'] != expected_identity(approval, target, kind)):
            raise ValueError('public consumption check identity or evidence differs')
        require_cases({'tests': check['cases']}, expected_cases(kind))
        aliases = check['aliases']
        if (not isinstance(aliases, dict) or set(aliases) != {'twigsmith', 'tgi'}
                or any(not isinstance(value, dict) or set(value) != {'passed', 'exit', 'version'}
                       or value['passed'] is not True or type(value['exit']) is not int
                       or value['exit'] != 0 or value['version'] != approval['version']
                       for value in aliases.values())):
            raise ValueError('both public aliases must execute the approved version')
        if kind.startswith('npx_') and (check['cold_cache'] is not True or check['invoked_alias'] != kind[4:]):
            raise ValueError('npx requires a separate cold cache and exact alias invocation')
    return report


def check_record(kind, approval, target, report, aliases):
    if report.get('passed') is not True:
        raise ValueError(f'{kind} public qualification failed')
    record = {'passed': True,
              'report': f'{kind}/' + ('installation.json' if kind == 'browser' else 'qualification.json'),
              'cases': require_cases(report, expected_cases(kind)),
              'identity': expected_identity(approval, target, kind), 'aliases': aliases}
    if kind.startswith('npx_'):
        record.update(cold_cache=True, invoked_alias=kind[4:])
    return record


def version_probe(command, version, log, env, cwd):
    output = qualify_platform.fixture_output(command, log, env, cwd)
    if output.returncode or output.stdout.strip() != 'twigsmith ' + version:
        raise ValueError('public alias did not execute the approved version')
    return {'passed': True, 'exit': output.returncode, 'version': version}


def native_check(approval, item, harness, evidence, env):
    directory = evidence / 'native'
    directory.mkdir()
    stem = native_package.archive_stem(item['archive'])
    for filename, expected in ((item['archive'], item['sha256']),
                               (stem + '.manifest.json', item['manifest_sha256']),
                               (stem + '.sha256', None)):
        public_download(release_url(approval, filename), directory / filename, expected)
    archive = directory / item['archive']
    manifest = qualify_installation.verify_candidate(archive, approval, item)
    native_package.verify(archive, directory / 'extracted')
    artifact = directory / 'extracted' / stem
    aliases = {name.removesuffix('.exe'): version_probe([str(artifact / name), '--version'],
               approval['version'], directory / f'{name}-version.log', env, directory)
               for name in native_package.executable_names(item['target'])}
    report = {'kind': 'public-native', 'manifest': manifest, 'download_url': release_url(approval, item['archive']),
              'aliases': aliases}
    qualify_platform.run_fixtures(artifact, harness, directory, report, env=env, selected_cases=JOURNEY)
    qualify_platform.write_json(directory / 'qualification.json', report)
    return check_record('native', approval, item['target'], report, aliases)


def package_contents(path):
    """Read a verified tarball without permitting links or path traversal."""
    result = {}
    with tarfile.open(path, 'r:gz') as archive:
        for member in archive.getmembers():
            parts = Path(member.name).parts
            if (not member.isfile() or not parts or parts[0] != 'package' or len(parts) < 2
                    or '..' in parts or member.name in result):
                raise ValueError('unsafe or duplicate public npm package member')
            result[member.name] = archive.extractfile(member).read()
    if not result or 'package/package.json' not in result:
        raise ValueError('public npm package metadata is absent')
    return result


def public_packages(approval, target, directory):
    directory.mkdir()
    selected = {'twigsmith', npm_platform_package.PACKAGES[target]}
    packages = {}
    for item in approval['npm']['packages']:
        if item['name'] not in selected:
            continue
        filename = item['filename']
        url = f"{REGISTRY}{item['name']}/-/{filename}"
        path = directory / filename
        public_download(url, path, item['sha256'], registry=True)
        files = package_contents(path)
        metadata = json.loads(files['package/package.json'])
        if metadata.get('name') != item['name'] or metadata.get('version') != approval['version']:
            raise ValueError('approved public npm tarball metadata differs')
        packages[item['name']] = files
    if set(packages) != selected:
        raise ValueError('public qualification lacks approved common/host packages')
    return packages


def verify_installed_package(directory, expected):
    # npm's cache may include metadata; the installed package itself must contain
    # all and only the approved tar members, byte for byte. Optional dependencies
    # can be nested under common/node_modules, which is checked separately.
    files = {}
    for path in directory.rglob('*'):
        relative = path.relative_to(directory)
        if relative.parts[0] == 'node_modules':
            continue
        if path.is_symlink():
            raise ValueError('installed npm package contains an unapproved link')
        if path.is_file():
            files['package/' + relative.as_posix()] = path.read_bytes()
    if files != expected:
        raise ValueError('installed npm package differs from approved public tarball bytes')


def installed_identity(common, node, packages, approval, item, env, directory):
    expected = npm_platform_package.PACKAGES[item['target']]
    common = common.resolve(strict=True)
    verify_installed_package(common, packages['twigsmith'])
    output = qualify_platform.fixture_output([str(node), '-e',
        "process.stdout.write(require.resolve(process.argv[1]+'/package.json',{paths:[process.argv[2]]}))",
        expected, str(common)], directory / 'resolve-payload.log', env, directory)
    if output.returncode:
        raise ValueError('public npm selected payload resolution failed')
    metadata_path = Path(output.stdout.strip())
    if not metadata_path.is_absolute() or metadata_path.is_symlink() or metadata_path.name != 'package.json':
        raise ValueError('public npm resolved an invalid payload metadata path')
    verify_installed_package(metadata_path.parent, packages[expected])
    metadata = json.loads(metadata_path.read_text(encoding='utf-8'))
    filename = native_package.executable_names(item['target'])[0]
    payload = metadata_path.parent / filename
    if (metadata.get('name') != expected or metadata.get('version') != approval['version']
            or metadata.get('twigsmithNative') != {'target': item['target'], 'file': filename,
                                                   'sha256': item['payload_sha256']}
            or payload.is_symlink() or qualify_platform.digest(payload) != item['payload_sha256']):
        raise ValueError('public npm selected payload identity differs from approval')
    return payload


def bridge_for(harness, prepared, directory):
    bridge = directory / 'fixture-bridge'
    bridge.mkdir()
    source = harness / prepared['executables']['native_npm_fixture']['path']
    for name in ('twigsmith', 'tgi'):
        shutil.copy2(source, bridge / (name + ('.exe' if os.name == 'nt' else '')))
    return bridge


def npm_install(command, log, env, cwd):
    output = qualify_platform.fixture_output(command, log, env, cwd)
    if output.returncode:
        raise ValueError('exact public npm installation failed')


def npm_failures(common, node, expected, version, directory, env):
    """Controlled copies of the public launcher exercise refusal without policy changes."""
    source = directory / 'refusal'
    source.mkdir()
    shutil.copy2(common / 'launcher.cjs', source / 'launcher.cjs')
    shutil.copy2(common / 'package.json', source / 'package.json')
    probes = []
    output = qualify_platform.fixture_output([str(node), str(source / 'launcher.cjs'), '--version'],
        directory / 'missing-dependency.log', env, source)
    probes.append({'name': 'missing-dependency', 'passed': output.returncode != 0
                   and f'missing {expected}@{version}' in output.stdout})
    output = qualify_platform.fixture_output([str(node), '-e',
        "try { require(process.argv[1]).selectPlatform('unsupported','unsupported',null); process.exit(0); } "
        "catch(error) { console.error(error.message); process.exit(1); }", str(source / 'launcher.cjs')],
        directory / 'unsupported-platform.log', env, source)
    probes.append({'name': 'unsupported-platform', 'passed': output.returncode != 0
                   and 'unsupported platform unsupported-unsupported' in output.stdout})
    if not all(probe['passed'] for probe in probes):
        raise ValueError('public launcher refusal probes failed')
    return probes


# Running this small observer through npx first records its PATH-selected shim
# without executing product code. After byte verification, a second, offline
# invocation asserts that same shim and runs the selected alias.
# Windows batch invocation quotes the npm-generated private-cache path explicitly.
# Only the fixed --version argument enters cmd; no user command text is accepted.
NPX_OBSERVER = r"""
const fs = require('node:fs'), path = require('node:path'), cp = require('node:child_process');
const alias = process.argv[1], out = process.argv[2];
if (!['twigsmith','tgi'].includes(alias)) throw new Error('unexpected npx alias');
const suffix = process.platform === 'win32' ? '.cmd' : '';
const shim = process.env.PATH.split(path.delimiter).map(p => path.join(p,alias+suffix)).find(p => fs.existsSync(p));
if (!shim) throw new Error('npx did not provide its alias');
fs.writeFileSync(out, JSON.stringify({shim:path.resolve(shim)}));
if (process.argv[3] !== 'execute') process.exit(0);
if (path.resolve(shim) !== process.argv[4]) throw new Error('npx selected another shim after verification');
let result;
if (process.platform === 'win32') {
  if (/["%!\r\n]/.test(shim)) throw new Error('npx shim path contains cmd expansion characters');
  const cmd = process.env.ComSpec || process.env.COMSPEC;
  if (!cmd || !path.isAbsolute(cmd)) throw new Error('absolute Windows command processor is unavailable');
  result = cp.spawnSync(cmd,['/d','/s','/c','""'+shim+'" --version"'],
                        {encoding:'utf8',shell:false,windowsVerbatimArguments:true});
} else {
  result = cp.spawnSync(shim,['--version'],{encoding:'utf8',shell:false});
}
if (result.error) throw result.error;
process.stdout.write(result.stdout||''); process.stderr.write(result.stderr||'');
process.exit(result.status===null?1:result.status);
"""


def npx_common(cache, observation, alias):
    row = json.loads(observation.read_text(encoding='utf-8'))
    if not isinstance(row, dict) or set(row) != {'shim'}:
        raise ValueError('npx selected-shim observation is invalid')
    shim = Path(row['shim'])
    if not shim.is_absolute():
        raise ValueError('npx selected an invalid shim')
    try:
        relative = shim.relative_to(cache)
    except ValueError:
        raise ValueError('npx selected an alias outside its independent cold cache') from None
    suffix = '.cmd' if os.name == 'nt' else ''
    if (len(relative.parts) != 5 or relative.parts[0] != '_npx'
            or relative.parts[2:] != ('node_modules', '.bin', alias + suffix)):
        raise ValueError('npx did not select its exact cached package shim')
    common = shim.parent.parent / 'twigsmith'
    if os.name != 'nt' and shim.resolve(strict=True) != (common / 'launcher.cjs').resolve(strict=True):
        raise ValueError('npx selected shim differs from the installed launcher')
    return common, shim.parent


def npm_check(kind, approval, item, harness, prepared, evidence, temporary, base_env, packages):
    directory = evidence / kind
    directory.mkdir()
    work = temporary / kind
    env = anonymous_environment(work)
    # anonymous_environment reads only host mechanics from the already isolated
    # environment; preserve the same pinned tool selection for all routes.
    env['PATH'] = base_env['PATH']
    node = qualify_platform.selected_npm_node(env)
    npm_args = SimpleNamespace(npm=None)
    npm = qualify_platform.npm_invocation(npm_args)
    report = {'kind': kind, 'registry': REGISTRY, 'version': approval['version'],
              'node': qualify_platform.fixture_output([str(node), '--version'], directory / 'node.log', env, work).stdout.strip(),
              'npm': qualify_platform.fixture_output(npm + ['--version'], directory / 'npm.log', env, work).stdout.strip()}
    npx_alias = kind[4:] if kind.startswith('npx_') else None
    if npx_alias is None:
        prefix = work / 'global'
        npm_install(npm + ['install', '--global', '--ignore-scripts', '--no-audit', '--no-fund',
                    '--registry', REGISTRY, '--prefix', str(prefix), 'twigsmith@' + approval['version']],
                    directory / 'install.log', env, work)
        common = prefix / ('node_modules' if os.name == 'nt' else 'lib/node_modules') / 'twigsmith'
        shim_dir = prefix if os.name == 'nt' else prefix / 'bin'
    else:
        cache = Path(env['npm_config_cache'])
        if cache.exists():
            raise ValueError('npx qualification requires an absent independent cache')
        observation = work / 'selected-shim.json'
        command = qualify_platform.npm_invocation(npm_args, npx=True) + [
            '--yes', '--ignore-scripts', '--registry', REGISTRY, '--package=twigsmith@' + approval['version'],
            '--', str(node), '-e', NPX_OBSERVER, npx_alias, str(observation)]
        npm_install(command, directory / 'npx-acquire.log', env, work)
        common, shim_dir = npx_common(cache, observation, npx_alias)
        report.update(cold_cache=True, invoked_alias=npx_alias, selected_shim=json.loads(observation.read_text()))
    payload = installed_identity(common, node, packages, approval, item, env, directory)
    if npx_alias is not None:
        # No network or package substitution after verifying the acquired cache.
        env['npm_config_offline'] = 'true'
        command.insert(len(qualify_platform.npm_invocation(npm_args, npx=True)), '--offline')
        selected_shim = json.loads(observation.read_text(encoding='utf-8'))['shim']
        version_probe(command + ['execute', selected_shim], approval['version'],
                      directory / 'npx.log', env, work)
    report.update(selected_payload=str(payload), payload_sha256=qualify_platform.digest(payload))
    bridge = bridge_for(harness, prepared, work)
    env.update(TWIGSMITH_NPM_NODE=str(node), TWIGSMITH_NPM_SHIM_DIR=str(shim_dir), TWIGSMITH_NPM_SHIM_KIND='cmd')
    aliases = {name: version_probe([str(bridge / (name + ('.exe' if os.name == 'nt' else ''))), '--version'],
               approval['version'], directory / f'{name}-version.log', env, work) for name in ('twigsmith', 'tgi')}
    selected = expected_cases(kind)
    qualify_platform.run_fixtures(bridge, harness, directory, report, kind='npm', env=env, selected_cases=selected)
    require_cases(report, selected)
    if kind == 'npm_global':
        report['refusals'] = npm_failures(common, node, npm_platform_package.PACKAGES[item['target']],
                                          approval['version'], directory, env)
        if os.name == 'nt':
            report['powershell'] = qualify_platform.powershell_shim_probes(shim_dir, approval['version'], directory, env)
            if not all(probe['passed'] for probe in report['powershell']['probes']):
                raise ValueError('public npm PowerShell shim probes failed')
    installed_identity(common, node, packages, approval, item, env, directory)
    report['aliases'] = aliases
    qualify_platform.write_json(directory / 'qualification.json', report)
    return check_record(kind, approval, item['target'], report, aliases)


def browser_check(approval_path, approval, item, harness, evidence):
    directory = evidence / 'browser'
    args = SimpleNamespace(operation='browser', approval=approval_path, target=item['target'],
                           harness_dir=harness, evidence_dir=directory, public_release=True)
    if qualify_installation.run(args):
        raise ValueError('direct public browser security/launch qualification failed')
    report = json.loads((directory / 'installation.json').read_text(encoding='utf-8'))
    journey = report['checks']['ordinary_git_journey']['observation']
    aliases = {}
    for name in ('twigsmith', 'tgi'):
        check = report['checks']['first_launch_' + name]
        observation = check['observation']
        if check['status'] != 'passed' or observation['exit'] != 0:
            raise ValueError('public browser alias failed its first launch')
        aliases[name] = {'passed': True, 'exit': observation['exit'], 'version': approval['version']}
    return check_record('browser', approval, item['target'],
                        {'passed': report['passed'], 'tests': journey['tests']}, aliases)


def run(args):
    approval = json.loads(args.approval.read_text(encoding='utf-8'))
    _, item = approved(approval, args.target)
    if qualify_platform.host_target() != args.target:
        raise ValueError('public consumption target differs from actual native host')
    harness = args.harness_dir.resolve(strict=True)
    prepared = qualify_platform.harness_manifest(harness, args.target)
    if prepared['source'] != item['source']:
        raise ValueError('public fixtures differ from approved native source')
    evidence = args.evidence_dir.resolve()
    if evidence.exists() or evidence.is_symlink():
        raise ValueError('public consumption evidence directory must be new')
    evidence.mkdir(parents=True)
    report = {'schema': 1, 'approval_sha256': canonical_hash(approval), 'target': args.target,
              'observed_at': datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z'),
              'producer': {field: os.environ.get(variable, '') for field, variable in (
                  ('publisher_revision', 'GITHUB_SHA'), ('run_id', 'GITHUB_RUN_ID'), ('run_attempt', 'GITHUB_RUN_ATTEMPT'))},
              'checks': {}, 'passed': False}
    try:
        with tempfile.TemporaryDirectory(prefix='twigsmith-public-consumption-') as temporary:
            temporary = Path(temporary)
            env = anonymous_environment(temporary / 'anonymous')
            with isolated_environment(env):
                qualify_platform.write_json(evidence / 'host.json', qualify_platform.environment())
                report['checks']['native'] = native_check(approval, item, harness, evidence, env)
                packages = public_packages(approval, args.target, evidence / 'public-npm-bytes')
                for kind in ('npm_global', 'npx_twigsmith', 'npx_tgi'):
                    report['checks'][kind] = npm_check(kind, approval, item, harness, prepared, evidence,
                                                      temporary, env, packages)
                if not args.target.endswith('linux-gnu'):
                    report['checks']['browser'] = browser_check(args.approval.resolve(), approval, item, harness, evidence)
        report['passed'] = True
        validate_report(report, approval, args.target)
        return 0
    except (OSError, ValueError, RuntimeError, KeyError, subprocess.SubprocessError) as error:
        report['passed'] = False
        # Preserve the strict summary schema even on failure; details are separate.
        qualify_platform.write_json(evidence / 'failure.json', {'error': qualify_installation.clean(str(error))})
        return 1
    finally:
        report['observed_at'] = datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z')
        qualify_platform.write_json(evidence / 'public-consumption.json', report)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--approval', required=True, type=Path)
    parser.add_argument('--target', required=True, choices=list(npm_platform_package.PACKAGES))
    parser.add_argument('--harness-dir', required=True, type=Path)
    parser.add_argument('--evidence-dir', required=True, type=Path)
    args = parser.parse_args()
    try:
        return run(args)
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        parser.exit(1, f'public consumption failed: {qualify_installation.clean(str(error))}\n')


if __name__ == '__main__':
    sys.exit(main())
