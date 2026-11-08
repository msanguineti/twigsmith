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
import npm_single_package
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
    # Publication is operational approval, not part of the frozen schema-3 byte
    # contract. Never drop another field or drop publication from the report hash.
    approval = validate_publication(approval)
    core = {key: value for key, value in approval.items() if key != 'publication'}
    core = release_set.validate_platform_approval(core)
    if target not in npm_single_package.TARGETS:
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
               npm_config_ignore_scripts='false', npm_config_offline='false',
               npm_config_audit='false', npm_config_fund='false', npm_config_update_notifier='false',
               LC_ALL='C', TZ='UTC')
    return env


def anonymous_browser_environment(root):
    # macOS browser processes require the native login/home/temp mechanics as a
    # group. Keep every other public-consumer credential/configuration exclusion.
    native = {name: os.environ[name] for name in ('HOME', 'USER', 'LOGNAME', 'TMPDIR')
              if name in os.environ} if sys.platform == 'darwin' else {}
    env = anonymous_environment(root)
    env.update(native)
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
        names = {'twigsmith'}
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
    keys = {'native', 'npm_local', 'npm_global', 'npx_twigsmith', 'npx_tgi'}
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
    # npm can emit its own notices on stderr even when the selected command
    # succeeds. Only the command's stdout establishes its version identity.
    try:
        output = subprocess.run(command, env=env, cwd=cwd, capture_output=True,
                                text=True, encoding='utf-8', errors='replace', timeout=300)
    except subprocess.TimeoutExpired as error:
        log.write_text('stdout:\n' + qualify_installation.output_text(error.stdout)
                       + '\nstderr:\n' + qualify_installation.output_text(error.stderr)
                       + '\nversion probe timed out after 300 seconds\n', encoding='utf-8')
        raise RuntimeError('public version probe timed out; see ' + str(log)) from error
    except OSError as error:
        log.write_text(f'version probe could not start: {error}\n', encoding='utf-8')
        raise
    log.write_text('stdout:\n' + output.stdout + '\nstderr:\n' + output.stderr, encoding='utf-8')
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
    return qualify_platform.package_contents(path)


def public_packages(approval, target, directory):
    directory.mkdir()
    packages = {}
    for item in approval['npm']['packages']:
        if item['name'] != 'twigsmith':
            raise ValueError('public qualification accepts only the single installer package')
        filename = item['filename']
        path = directory / filename
        public_download(f"{REGISTRY}twigsmith/-/{filename}", path, item['sha256'], registry=True)
        files = package_contents(path)
        if set(files) != {'package/' + name for name in ('package.json', 'native.json', 'installer.cjs',
                                                        'launcher.cjs', 'README.md', 'LICENSE')}:
            raise ValueError('public installer package must contain exactly six reviewed files')
        metadata = json.loads(files['package/package.json'])
        native = json.loads(files['package/native.json'])
        if (metadata.get('name') != 'twigsmith' or metadata.get('version') != approval['version']
                or metadata.get('private') or metadata.get('dependencies') or metadata.get('optionalDependencies')
                or metadata.get('scripts') != {'postinstall': 'node installer.cjs'}
                or set(native) != {'schema', 'version', 'targets'} or native['schema'] != 1
                or native['version'] != approval['version'] or set(native['targets']) != set(npm_single_package.TARGETS)):
            raise ValueError('approved public single installer metadata differs')
        for record in approval['native']:
            entry = native['targets'][record['target']]
            if (entry.get('url') != release_url(approval, record['archive'])
                    or entry.get('archive') != record['archive'] or entry.get('archive_sha256') != record['sha256']
                    or entry.get('payload_sha256') != record['payload_sha256']
                    or entry.get('payload') != native_package.executable_names(record['target'])[0]):
                raise ValueError('public npm native table differs from approved native release')
        packages['twigsmith'] = files
    if set(packages) != {'twigsmith'}:
        raise ValueError('public qualification lacks the approved single installer')
    return packages


def verify_installed_package(directory, expected):
    return qualify_platform.verify_installed_package(directory, expected)


def installed_identity(common, node, packages, approval, item, env, directory):
    # Inspect the generated state and payload independently; never ask the
    # installed launcher to attest its own package or executable bytes.
    return qualify_platform.installed_payload(common, packages['twigsmith'], item['target'], item['payload_sha256'])


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


def npm_failures(common, node, target, version, directory, env):
    """Controlled package copies must refuse absent/corrupt state without repair."""
    source = directory / 'refusal'
    shutil.copytree(common, source)
    state = source / '.native/installed.json'
    original = state.read_bytes()
    installed = json.loads(original)
    metadata = json.loads((source / 'native.json').read_text())
    payload = source / '.native' / installed['directory'] / metadata['targets'][target]['payload']
    original_payload = payload.read_bytes()
    guard = directory / 'refusal-network-guard.cjs'
    guard.write_text("""'use strict';
const refuse = () => { console.error('TWIGSMITH_NETWORK_ATTEMPT'); throw new Error('network forbidden in refusal probe'); };
for (const [name, methods] of [['node:http',['request','get']],['node:https',['request','get']],
                              ['node:net',['connect','createConnection']],['node:tls',['connect']]]) {
  const transport = require(name); for (const method of methods) transport[method] = refuse;
}
globalThis.fetch = refuse;
""", encoding='utf-8')
    probes = []
    for label in ('missing-state', 'corrupt-state', 'corrupt-payload'):
        state.write_bytes(original)
        payload.write_bytes(original_payload)
        if label == 'missing-state':
            state.unlink()
        elif label == 'corrupt-state':
            state.write_text(json.dumps({'target': 'unsupported', 'directory': 'unsupported'}))
        else:
            payload.write_bytes(original_payload + b'corrupted')
        # A local request recorder also detects accidental npm/network repair.
        # The launcher must fail before any application execution.
        with qualify_platform.local_registry({}) as (registry, requests):
            offline = dict(env, npm_config_offline='true', npm_config_registry=registry)
            output = qualify_platform.fixture_output([str(node), '--require', str(guard), str(source / 'launcher.cjs'), '--version'],
                directory / (label + '.log'), offline, source)
            probes.append({'name': label, 'passed': output.returncode != 0
                           and 'twigsmith-install' in output.stdout and not requests
                           and 'TWIGSMITH_NETWORK_ATTEMPT' not in output.stdout})
    if not all(probe['passed'] for probe in probes):
        raise ValueError('public launcher refusal probes failed')
    return probes


def blocked_repair(approval, item, packages, node, npm_args, directory, work, env):
    """Real script-disabled local install, offline npm repair dispatch, then verify."""
    consumer = work / 'script-blocked'
    consumer.mkdir()
    npm = qualify_platform.npm_invocation(npm_args)
    npm_install(npm + ['install', '--ignore-scripts', '--no-audit', '--no-fund', '--prefix', str(consumer),
                      '--registry', REGISTRY, 'twigsmith@' + approval['version']],
                directory / 'blocked-install.log', env, consumer)
    common = consumer / 'node_modules/twigsmith'
    verify_installed_package(common, packages['twigsmith'])
    if (common / '.native').exists():
        raise ValueError('script-disabled public install unexpectedly created native state')
    for alias in ('twigsmith', 'tgi'):
        output = qualify_platform.fixture_output([str(node), str(common / 'launcher.cjs'), '--version'],
            directory / ('blocked-' + alias + '.log'), env, consumer)
        if output.returncode == 0 or 'twigsmith-install' not in output.stdout or (common / '.native').exists():
            raise ValueError('script-disabled public launcher did not refuse missing executable')
    # npm resolves only the installed command offline; its explicit installer is
    # intentionally online to acquire the exact approved native release asset.
    npm_install(npm + ['exec', '--offline', '--ignore-scripts', '--', 'twigsmith-install'],
                directory / 'blocked-explicit-repair.log', env, consumer)
    installed_identity(common, node, packages, approval, item, env, directory)
    version_probe([str(node), str(common / 'launcher.cjs'), '--version'], approval['version'],
                  directory / 'blocked-repaired-version.log', env, consumer)
    return {'passed': True, 'scripts_blocked': True, 'explicit_repair': True}


# Running this small observer through npx first records its PATH-selected shim
# without executing product code. After byte verification, a second, offline
# invocation asserts that same shim and runs the selected alias.
# Windows batch invocation quotes the npm-generated private-cache path explicitly.
# Only the fixed --version argument enters cmd; no user command text is accepted.
NPX_OBSERVER = r"""
const fs = require('node:fs'), path = require('node:path'), cp = require('node:child_process');
const alias = process.argv[2], out = process.argv[3];
if (!['twigsmith','tgi'].includes(alias)) throw new Error('unexpected npx alias');
const suffix = process.platform === 'win32' ? '.cmd' : '';
const shim = process.env.PATH.split(path.delimiter).map(p => path.join(p,alias+suffix)).find(p => fs.existsSync(p));
if (!shim) throw new Error('npx did not provide its alias');
fs.writeFileSync(out, JSON.stringify({shim:path.resolve(shim)}));
if (process.argv[4] !== 'execute') process.exit(0);
if (path.resolve(shim) !== process.argv[5]) throw new Error('npx selected another shim after verification');
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


def npx_observer_command(node, directory, alias, observation):
    """Pass only a private script filename through npm's platform shell.

    npm exec reconstructs a shell command on Windows; literal multiline `-e`
    source cannot be carried safely through that command's newline boundary.
    The fixed observer file uses Node script argument positions on every host.
    """
    if alias not in ('twigsmith', 'tgi'):
        raise ValueError('unexpected npx observer alias')
    script = directory.resolve(strict=True) / 'npx-observer.cjs'
    data = NPX_OBSERVER.encode('utf-8')
    if script.exists() or script.is_symlink():
        if script.is_symlink() or not script.is_file() or script.read_bytes() != data:
            raise ValueError('private npx observer file changed')
    else:
        with script.open('xb') as output:
            output.write(data)
    return [str(node), str(script), alias, str(observation.absolute())]


def npx_blocked_repair(npm_args, version, packages, target, payload_hash, node, directory, work, env):
    """Qualify the documented exact-version npx repair in one independent cache."""
    env = dict(env, npm_config_cache=str(work / 'npx-blocked-cache'), npm_config_offline='false')
    cache = Path(env['npm_config_cache'])
    if cache.exists():
        raise ValueError('blocked npx repair needs an independent cold cache')
    observation = work / 'npx-blocked-shim.json'
    npx = qualify_platform.npm_invocation(npm_args, npx=True)
    acquire = npx + ['--yes', '--ignore-scripts', '--package=twigsmith@' + version,
                     '--', *npx_observer_command(node, directory, 'twigsmith', observation)]
    npm_install(acquire, directory / 'npx-blocked-acquire.log', env, work)
    common, _ = npx_common(cache, observation, 'twigsmith')
    verify_installed_package(common, packages['twigsmith'])
    if (common / '.native').exists():
        raise ValueError('script-blocked npx unexpectedly installed native bytes')
    output = qualify_platform.fixture_output([str(node), str(common / 'launcher.cjs'), '--version'],
                                             directory / 'npx-blocked-refusal.log', env, work)
    if output.returncode == 0 or 'twigsmith-install' not in output.stdout:
        raise ValueError('script-blocked npx launcher did not refuse missing state')
    repair = npx + ['--yes', '--offline', '--ignore-scripts', '--package=twigsmith@' + version,
                    '--', 'twigsmith-install']
    npm_install(repair, directory / 'npx-blocked-repair.log', env, work)
    qualify_platform.installed_payload(common, packages['twigsmith'], target, payload_hash)
    shim = json.loads(observation.read_text())['shim']
    version_probe(npx + ['--offline', *acquire[len(npx):], 'execute', shim], version,
                  directory / 'npx-blocked-repaired-version.log', dict(env, npm_config_offline='true'), work)
    qualify_platform.installed_payload(common, packages['twigsmith'], target, payload_hash)
    return {'passed': True, 'cold_cache': True, 'scripts_blocked': True, 'explicit_repair': True}


def npx_common(cache, observation, alias):
    if observation.is_symlink() or not observation.is_file():
        raise ValueError('npx observer did not write its selected-shim observation')
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
        local = kind == 'npm_local'
        prefix = work / ('local' if local else 'global')
        prefix.mkdir()
        flags = qualify_platform.script_policy(report['npm'], 'twigsmith', prefix if local else None)
        npm_install(npm + ['install', *([] if local else ['--global']), '--no-audit', '--no-fund',
                    *flags, '--registry', REGISTRY, '--prefix', str(prefix), 'twigsmith@' + approval['version']],
                    directory / 'install.log', env, prefix)
        common = prefix / ('node_modules' if local or os.name == 'nt' else 'lib/node_modules') / 'twigsmith'
        shim_dir = prefix / 'node_modules/.bin' if local else prefix if os.name == 'nt' else prefix / 'bin'
    else:
        cache = Path(env['npm_config_cache'])
        if cache.exists():
            raise ValueError('npx qualification requires an absent independent cache')
        observation = work / 'selected-shim.json'
        command = qualify_platform.npm_invocation(npm_args, npx=True) + [
            '--yes', *qualify_platform.script_policy(report['npm'], 'twigsmith'), '--registry', REGISTRY, '--package=twigsmith@' + approval['version'],
            '--', *npx_observer_command(node, directory, npx_alias, observation)]
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
        report['refusals'] = npm_failures(common, node, item['target'],
                                          approval['version'], directory, env)
        report['script_blocked_repair'] = blocked_repair(approval, item, packages, node, npm_args, directory, work, env)
        report['npx_blocked_repair'] = npx_blocked_repair(npm_args, approval['version'], packages, item['target'],
                                                       item['payload_sha256'], node, directory, work, env)
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
                for kind in ('npm_local', 'npm_global', 'npx_twigsmith', 'npx_tgi'):
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
    parser.add_argument('--target', required=True, choices=list(npm_single_package.TARGETS))
    parser.add_argument('--harness-dir', required=True, type=Path)
    parser.add_argument('--evidence-dir', required=True, type=Path)
    args = parser.parse_args()
    try:
        return run(args)
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        parser.exit(1, f'public consumption failed: {qualify_installation.clean(str(error))}\n')


if __name__ == '__main__':
    sys.exit(main())
