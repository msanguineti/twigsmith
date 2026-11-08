#!/usr/bin/env python3
"""Observe unsigned browser downloads and isolated Windows runtime provisioning.

These finite probes never approve a release, change host policy or publish files.
Unavailable required security observations, UI access or clean runtime remain unmet.
"""
import argparse
import base64
import contextlib
import functools
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import re
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

import native_package
import qualify_platform
import release_set
import release_stage

IMAGE = 'mcr.microsoft.com/windows/servercore:ltsc2025'
REDIST = 'https://aka.ms/vc14/vc_redist.x64.exe'
TIMEOUT = 180
BROWSER_ROWS = ('browser_download', 'outer_security_metadata', 'archive_identity',
                'native_extraction', 'inherited_security_metadata', 'security_policy', 'security_assessment',
                'first_launch_twigsmith', 'first_launch_tgi', 'ordinary_git_journey',
                'original_retained_after_browser_close')
RUNTIME_ROWS = ('isolated_container', 'official_installer', 'runtime_absent',
                'absent_launch_twigsmith', 'absent_launch_tgi', 'runtime_install',
                'installed_launch_twigsmith', 'installed_launch_tgi')


def clean(value):
    """Evidence never contains tokens or signed URLs, including exception output."""
    if isinstance(value, dict):
        return {key: clean(item) for key, item in value.items()}
    if isinstance(value, list):
        return [clean(item) for item in value]
    if isinstance(value, str):
        value = re.sub(r'https?://[^\s\"<>]+', '[URL REDACTED]', value)
        for name in ('GH_TOKEN', 'GITHUB_TOKEN', 'ACTIONS_RUNTIME_TOKEN'):
            if os.environ.get(name):
                value = value.replace(os.environ[name], '[TOKEN REDACTED]')
        return value
    return value


def child_env():
    return {key: value for key, value in os.environ.items()
            if key not in ('GH_TOKEN', 'GITHUB_TOKEN', 'ACTIONS_RUNTIME_TOKEN')}


def output_text(value):
    return value.decode('utf-8', errors='replace') if isinstance(value, bytes) else value or ''


def command(args, *, timeout=TIMEOUT, input=None, max_stdout=24000):
    started = time.monotonic()
    try:
        result = subprocess.run(args, input=input, capture_output=True, text=True,
                                encoding='utf-8', errors='replace', timeout=timeout, env=child_env())
        record = {'exit': result.returncode, 'stdout': output_text(result.stdout),
                  'stderr': output_text(result.stderr), 'timed_out': False}
    except subprocess.TimeoutExpired as error:
        # TimeoutExpired may hold bytes even when subprocess.run used text=True.
        # Preserve partial progress without recording raw command arguments.
        record = {'exit': 124, 'stdout': output_text(error.stdout), 'stderr': output_text(error.stderr),
                  'timed_out': True, 'timeout_seconds': timeout}
    record['seconds'] = round(time.monotonic() - started, 3)
    for name, limit in (('stdout', max_stdout), ('stderr', 12000)):
        record[name + '_characters'] = len(record[name])
        sanitized = clean(record[name])
        record[name + '_truncated'] = limit is not None and len(sanitized) > limit
        record[name] = sanitized if limit is None else sanitized[-limit:]
    return record


def row(report, name, status, observation):
    report['checks'][name] = {'status': status, 'observation': clean(observation)}


def passed(report, required):
    return bool(required) and all(report['checks'].get(name, {}).get('status') == 'passed'
                                  for name in required)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def api_response(path):
    """Authenticate only the canonical GitHub API; never follow its redirects."""
    if (not path.startswith('repos/' + release_set.REPOSITORY + '/actions/')
            or '..' in path or re.search(r'[?#\s]', path)):
        raise ValueError('unexpected canonical Actions API path')
    token = os.environ.get('GH_TOKEN') or os.environ.get('GITHUB_TOKEN')
    if not token:
        raise ValueError('read-only GitHub token is unavailable')
    request = urllib.request.Request('https://api.github.com/' + path, headers={
        'Authorization': 'Bearer ' + token, 'Accept': 'application/vnd.github+json',
        'X-GitHub-Api-Version': '2022-11-28', 'User-Agent': 'twigsmith-installation-probe'})
    opener = urllib.request.build_opener(NoRedirect)
    try:
        response = opener.open(request, timeout=30)
    except urllib.error.HTTPError as error:
        if error.code not in (301, 302, 303, 307, 308):
            raise RuntimeError(f'canonical Actions API returned HTTP {error.code}') from None
        response = error
    with response:
        return response.code, dict(response.headers), response.read(8 * 1024 * 1024)


def api_json(path):
    status, _, body = api_response(path)
    if status != 200:
        raise ValueError('canonical metadata endpoint unexpectedly redirected')
    return json.loads(body)


def signed_redirect(artifact_id):
    artifact_id = release_stage.positive(artifact_id)
    status, headers, _ = api_response(
        f'repos/{release_set.REPOSITORY}/actions/artifacts/{artifact_id}/zip')
    location = next((value for key, value in headers.items() if key.lower() == 'location'), '')
    parsed = urllib.parse.urlsplit(location)
    if (status != 302 or parsed.scheme != 'https' or parsed.username or parsed.password
            or parsed.port not in (None, 443) or not parsed.hostname
            or not any(parsed.hostname.endswith(suffix) for suffix in
                       ('.blob.core.windows.net', '.actions.githubusercontent.com', '.githubusercontent.com'))):
        raise ValueError('expected an HTTPS Actions storage redirect')
    return location


def approved(approval, target, *, public=False):
    if public:
        import qualify_public_release
        approval, _ = qualify_public_release.approved(approval, target)
    else:
        approval = release_set.validate_platform_approval(approval)
    item = next((item for item in approval['native'] if item['target'] == target), None)
    if item is None or target != qualify_platform.host_target() or target.endswith('linux-gnu'):
        raise ValueError('installation probe requires the approved native Windows/macOS host')
    return approval, item


def verify_candidate(archive, approval, item):
    archive = Path(archive)
    stem = native_package.archive_stem(archive)
    manifest_path = archive.with_name(stem + '.manifest.json')
    if archive.name != item['archive'] or native_package.digest(archive.read_bytes()) != item['sha256']:
        raise ValueError('native archive differs from approved bytes')
    if native_package.digest(manifest_path.read_bytes()) != item['manifest_sha256']:
        raise ValueError('native manifest differs from approved bytes')
    # Native verification binds each alias separately; the external payload hash
    # identifies only the executable selected for npm, not byte-identical aliases.
    manifest = native_package.verify(archive)
    if (manifest['version'] != approval['version'] or manifest['target'] != item['target']
            or manifest['provenance']['source'] != item['source']
            or manifest['provenance']['environment']['ci'] != item['ci']
            or manifest['requirements'] != approval['requirements'][item['target']]
            or manifest['files'][manifest['npm_payload']]['sha256'] != item['payload_sha256']):
        raise ValueError('native manifest differs from approved source/requirements/payload')
    return manifest


class CDP:
    """Minimal local WebSocket client; no browser package or disabling flags."""
    def __init__(self, url):
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme != 'ws' or parsed.hostname not in ('localhost', '127.0.0.1'):
            raise ValueError('DevTools endpoint is not local')
        self.sock = socket.create_connection((parsed.hostname, parsed.port), timeout=10)
        self.sock.settimeout(10)
        key = base64.b64encode(os.urandom(16)).decode()
        request = (f'GET {parsed.path} HTTP/1.1\r\nHost: {parsed.hostname}:{parsed.port}\r\n'
                   f'Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n'
                   'Sec-WebSocket-Version: 13\r\n\r\n')
        self.sock.sendall(request.encode())
        header = b''
        while not header.endswith(b'\r\n\r\n') and len(header) < 8192:
            header += self.exact(1)
        expected = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest())
        if not header.startswith(b'HTTP/1.1 101 ') or expected.lower() not in header.lower():
            self.sock.close()
            raise ValueError('local DevTools WebSocket handshake failed')
        self.sequence = 0
        self.events = []

    def exact(self, count):
        output = b''
        while len(output) < count:
            chunk = self.sock.recv(count - len(output))
            if not chunk:
                raise RuntimeError('local browser connection closed')
            output += chunk
        return output

    def frame(self, opcode, data):
        mask = os.urandom(4)
        length = len(data)
        header = bytes([0x80 | opcode, 0x80 | (length if length < 126 else 126 if length < 65536 else 127)])
        if length >= 126:
            header += struct.pack('!H' if length < 65536 else '!Q', length)
        self.sock.sendall(header + mask + bytes(byte ^ mask[i % 4] for i, byte in enumerate(data)))

    def call(self, method, params=None):
        self.sequence += 1
        self.frame(1, json.dumps({'id': self.sequence, 'method': method, 'params': params or {}}).encode())
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            first, second = self.exact(2)
            length = second & 127
            if length in (126, 127):
                length = struct.unpack('!H' if length == 126 else '!Q', self.exact(2 if length == 126 else 8))[0]
            if length > 8 * 1024 * 1024 or second & 128 or not first & 128:
                raise ValueError('unexpected local DevTools frame')
            data = self.exact(length)
            if first & 15 == 9:
                self.frame(10, data)
                continue
            if first & 15 != 1:
                raise ValueError('local DevTools closed or sent a non-text message')
            message = json.loads(data)
            if message.get('method') in ('Browser.downloadWillBegin', 'Browser.downloadProgress'):
                fields = ('guid', 'suggestedFilename', 'state', 'receivedBytes', 'totalBytes')
                self.events.append({'event': message['method'], **{key: value for key, value in
                    message.get('params', {}).items() if key in fields}})
            if message.get('id') == self.sequence:
                if 'error' in message:
                    raise RuntimeError('local browser rejected DevTools operation')
                return message.get('result', {})
        raise TimeoutError('local browser command timed out')

    def close(self):
        self.sock.close()


def browser_binary():
    if sys.platform == 'darwin':
        path = Path('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome')
        return path if path.is_file() else None
    if sys.platform == 'win32':
        for variable in ('PROGRAMFILES(X86)', 'PROGRAMFILES', 'LOCALAPPDATA'):
            if os.environ.get(variable):
                path = Path(os.environ[variable]) / 'Microsoft/Edge/Application/msedge.exe'
                if path.is_file():
                    return path
    return None


def download_snapshot(directory, stage):
    files = []
    if not directory.is_dir():
        return {'stage': stage, 'directory_missing': True, 'files': files}
    for path in directory.iterdir():
        try:
            info = path.stat()
            files.append({'name': path.name, 'size': info.st_size, 'regular': path.is_file(),
                          'symlink': path.is_symlink()})
        except FileNotFoundError:
            files.append({'name': path.name, 'disappeared_during_observation': True})
    return {'stage': stage, 'observed_at_utc': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()), 'files': files}


def selected_download_progress(events):
    beginnings = {event['guid']: event for event in events if event.get('event') == 'Browser.downloadWillBegin'
                  and event.get('guid')}
    if len(beginnings) > 1:
        raise RuntimeError('native browser initiated more than one download')
    if not beginnings:
        return {'state': 'pending'}
    guid, beginning = next(iter(beginnings.items()))
    selected = [event for event in events if event.get('event') == 'Browser.downloadProgress'
                and event.get('guid') == guid]
    return {'guid': guid, 'suggestedFilename': beginning.get('suggestedFilename'),
            'state': selected[-1].get('state') if selected else 'pending'}


@contextlib.contextmanager
def browser_download(url, directory, profile, expected_hash, observation=None):
    binary = browser_binary()
    if binary is None:
        raise RuntimeError('installed native headed browser is unavailable')
    flags = ['--user-data-dir=' + str(profile), '--remote-debugging-address=127.0.0.1',
             '--remote-debugging-port=0', '--no-first-run', '--no-default-browser-check', 'about:blank']
    directory = directory.resolve()
    profile = profile.resolve()
    process = subprocess.Popen([str(binary), *flags], stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, env=child_env())
    client = None
    observation = {} if observation is None else observation
    observation['lifecycle'] = [download_snapshot(directory, 'before_browser_navigation')]
    try:
        active = profile / 'DevToolsActivePort'
        deadline = time.monotonic() + 40
        while not active.is_file():
            if process.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError('headed browser or local UI session could not start')
            time.sleep(.2)
        lines = active.read_text().splitlines()
        client = CDP('ws://127.0.0.1:' + str(int(lines[0])) + lines[1])
        version = client.call('Browser.getVersion')
        observation.update(binary=str(binary), flags=flags, version=version, headed=True,
                           download_events=client.events)
        client.call('Browser.setDownloadBehavior', {'behavior': 'allow', 'downloadPath': str(directory),
                                                     'eventsEnabled': True})
        target = client.call('Target.createTarget', {'url': 'about:blank'})
        connection = http.client.HTTPConnection('127.0.0.1', int(lines[0]), timeout=10)
        connection.request('GET', '/json/list')
        targets = json.loads(connection.getresponse().read())
        connection.close()
        page = next(item for item in targets if item.get('id') == target['targetId'])
        page_client = CDP(page['webSocketDebuggerUrl'])
        try:
            page_client.call('Page.navigate', {'url': url})
        finally:
            page_client.close()
        deadline = time.monotonic() + TIMEOUT
        while time.monotonic() < deadline:
            # Drain browser-level progress messages without persisting their signed URL.
            client.call('Browser.getVersion')
            progress = selected_download_progress(client.events)
            completed = progress['state'] == 'completed'
            if progress['state'] == 'canceled':
                raise RuntimeError('native browser canceled the download; security decision or cancellation remains unqualified')
            files = list(directory.iterdir())
            complete = [path for path in files if path.is_file() and not path.is_symlink()
                        and path.suffix not in ('.crdownload', '.tmp')]
            if completed and len(complete) == 1 and len(files) == 1:
                if complete[0].name != progress['suggestedFilename']:
                    raise ValueError('native browser completion filename differs from the original file')
                if native_package.digest(complete[0].read_bytes()) != expected_hash:
                    raise ValueError('browser-downloaded original ZIP hash mismatch')
                observation.update(binary=str(binary), flags=flags, version=version, headed=True,
                                   download_sha256=expected_hash, download_events=client.events)
                observation['lifecycle'].append(download_snapshot(directory, 'verified_original_before_consumer'))
                yield complete[0], observation
                return
            time.sleep(.5)
        raise RuntimeError('native browser did not complete the exact download; security UI may require a person')
    finally:
        observation['lifecycle'].append(download_snapshot(directory, 'before_browser_close'))
        if client:
            try:
                client.call('Browser.close')
            except (OSError, ValueError, RuntimeError, TimeoutError):
                pass
            client.close()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)
        observation['lifecycle'].append(download_snapshot(directory, 'after_browser_close'))


def ps_literal(value):
    return "'" + str(value).replace("'", "''") + "'"


@functools.lru_cache(maxsize=1)
def powershell_host():
    selected = shutil.which('pwsh')
    if not selected:
        raise RuntimeError('the selected maintained PowerShell 7.6 host is unavailable')
    selected = str(Path(selected).resolve())
    script = ("$modules=@{}; foreach ($name in @('Expand-Archive','Get-AuthenticodeSignature')) {"
              "try { $c=Get-Command $name -ErrorAction Stop; $modules[$name]=@{status='observed';"
              "name=$c.ModuleName;version=$c.Module.Version.ToString();path=$c.Module.Path} }"
              "catch { $modules[$name]=@{status='unavailable';error=$_.Exception.Message} } }; "
              '[pscustomobject]@{executable=[System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName;'
              'version=$PSVersionTable.PSVersion.ToString();edition=$PSVersionTable.PSEdition;'
              'home=$PSHOME;module_search_path=$env:PSModulePath;modules=$modules}'
              ' | ConvertTo-Json -Depth 5 -Compress')
    result = command([selected, '-NoLogo', '-NoProfile', '-NonInteractive', '-Command', script], timeout=30, max_stdout=None)
    if result['exit']:
        raise RuntimeError('could not inspect selected PowerShell 7.6 host: ' + json.dumps(result))
    identity = json.loads(result['stdout'])
    if (identity.get('edition') != 'Core' or not re.fullmatch(r'7\.6\.[0-9]+', identity.get('version', ''))
            or Path(identity.get('executable', '')).resolve() != Path(selected)):
        raise ValueError('host helpers require the selected native PowerShell Core 7.6 executable')
    modules = identity.get('modules')
    expected = {'Expand-Archive': 'Microsoft.PowerShell.Archive',
                'Get-AuthenticodeSignature': 'Microsoft.PowerShell.Security'}
    if (not isinstance(modules, dict) or set(modules) != set(expected)
            or any(not isinstance(modules[name], dict) or modules[name].get('status') != 'observed'
                   or modules[name].get('name') != module or not modules[name].get('version')
                   or not modules[name].get('path') for name, module in expected.items())):
        raise ValueError('selected PowerShell module identities are unavailable: ' + json.dumps(clean(identity)))
    return identity


def powershell(script, **kwargs):
    host = powershell_host()
    kwargs.setdefault('max_stdout', None)
    result = command([host['executable'], '-NoLogo', '-NoProfile', '-NonInteractive', '-Command', script], **kwargs)
    return {**result, 'host_powershell': host}


def security_metadata(path):
    if sys.platform == 'darwin':
        result = command(['/usr/bin/xattr', '-p', 'com.apple.quarantine', str(path)])
        return {'present': result['exit'] == 0 and bool(result['stdout'].strip()), 'probe': result}
    script = ("$ErrorActionPreference='Stop'; $p=" + ps_literal(path) +
              "; if (!(Test-Path -LiteralPath $p -PathType Leaf)) { throw 'original file is unavailable' }; " +
              "$readable=[System.IO.File]::OpenRead($p); $readable.Dispose(); " +
              "$streams=@(Get-Item -LiteralPath $p -Stream * | Select-Object Stream,Length); " +
              "$zone=@($streams | Where-Object Stream -eq 'Zone.Identifier'); " +
              "$content=if ($zone.Count) { Get-Content -LiteralPath $p -Stream Zone.Identifier -Raw } else { '' }; " +
              "[pscustomobject]@{streams=$streams;zone_identifier=$content;zone_stream_present=($zone.Count -gt 0)}" +
              ' | ConvertTo-Json -Depth 5 -Compress')
    result = powershell(script)
    if result['exit']:
        return {'state': 'unavailable', 'present': False, 'probe': result}
    try:
        observation = json.loads(result['stdout'])
        zone = observation['zone_stream_present']
        if type(zone) is not bool or not isinstance(observation['streams'], list):
            raise ValueError('unexpected stream observation')
        matched = re.search(r'(?m)^ZoneId=([0-9]+)\s*$', observation['zone_identifier'])
        zone_id = int(matched[1]) if matched else None
        marked = zone and zone_id in (3, 4)
        return {'state': 'observed_present' if zone else 'observed_absent', 'present': marked, 'zone_id': zone_id,
                'streams': observation['streams'], 'zone_identifier': observation['zone_identifier'], 'probe': result}
    except (ValueError, KeyError, TypeError):
        return {'state': 'unavailable', 'present': False, 'probe': result,
                'reason': 'stream observation is unreadable or incomplete'}


def browser_origin_observed(metadata):
    return (metadata.get('state') == 'observed_present' and metadata.get('zone_id') == 3
            if sys.platform == 'win32' else metadata.get('present') is True)


def extraction_metadata_observed(marks):
    if sys.platform == 'win32':
        return browser_origin_observed(marks['outer']) and all(
            item.get('state') in ('observed_present', 'observed_absent') for item in marks.values())
    return all(item.get('present') is True for item in marks.values())


WINDOWS_POLICY_SCRIPT = r'''
$ErrorActionPreference='Stop'
function Observe($action) {
  try { @{status='observed';value=(& $action)} }
  catch { @{status='unavailable';error=$_.Exception.Message} }
}
function RegistryValue($path,$name) {
  if (!(Test-Path -LiteralPath $path)) { return @{state='not_configured'} }
  $item=Get-ItemProperty -LiteralPath $path
  if ($null -eq $item.PSObject.Properties[$name]) { return @{state='not_configured'} }
  return @{state='configured';value=$item.$name}
}
$checks=@{}
$checks.defender=Observe { Get-MpComputerStatus | Select-Object AMServiceEnabled,AntivirusEnabled,RealTimeProtectionEnabled,BehaviorMonitorEnabled,IoavProtectionEnabled,AMRunningMode,AMProductVersion }
$checks.execution_policy=Observe { @(Get-ExecutionPolicy -List | Select-Object @{n='scope';e={$_.Scope.ToString()}},@{n='policy';e={$_.ExecutionPolicy.ToString()}}) }
foreach ($name in @('EnableSmartScreen','ShellSmartScreenLevel')) {
  $checks["explorer_machine_$name"]=Observe { RegistryValue 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\System' $name }
}
foreach ($hive in @('HKLM','HKCU')) {
  $checks["explorer_$hive"]=Observe { RegistryValue "${hive}:\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer" 'SmartScreenEnabled' }
  foreach ($name in @('SmartScreenEnabled','PreventSmartScreenPromptOverride','PreventSmartScreenPromptOverrideForFiles','SmartScreenForTrustedDownloadsEnabled')) {
    $checks["edge_${hive}_$name"]=Observe { RegistryValue "${hive}:\SOFTWARE\Policies\Microsoft\Edge" $name }
  }
}
$checks.uac=Observe { RegistryValue 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System' 'EnableLUA' }
@{checks=$checks} | ConvertTo-Json -Depth 8 -Compress
'''


def windows_policy():
    result = powershell(WINDOWS_POLICY_SCRIPT)
    if result['exit']:
        return {'available': False, 'probe': result}
    try:
        observation = json.loads(result['stdout'])
        checks = observation['checks']
        if not isinstance(checks, dict) or any(not isinstance(value, dict) for value in checks.values()):
            raise ValueError('unexpected policy result shape')
        expected = {'defender', 'execution_policy', 'uac', 'explorer_HKLM', 'explorer_HKCU'}
        expected.update('explorer_machine_' + name for name in ('EnableSmartScreen', 'ShellSmartScreenLevel'))
        expected.update('edge_' + hive + '_' + name for hive in ('HKLM', 'HKCU') for name in
                        ('SmartScreenEnabled', 'PreventSmartScreenPromptOverride',
                         'PreventSmartScreenPromptOverrideForFiles', 'SmartScreenForTrustedDownloadsEnabled'))
        available = set(checks) == expected and all(value.get('status') == 'observed' for value in checks.values())
        return {'available': available, 'host_powershell': result.get('host_powershell'), **observation}
    except (ValueError, KeyError, TypeError):
        return {'available': False, 'probe': result, 'reason': 'policy observation is unreadable or incomplete'}


def os_extract(archive, destination, expected_files=None):
    destination.mkdir(parents=True, exist_ok=False)
    if sys.platform == 'win32':
        result = powershell("$ErrorActionPreference='Stop'; Expand-Archive -LiteralPath " +
                            ps_literal(archive) + ' -DestinationPath ' + ps_literal(destination))
    elif archive.name.endswith('.zip'):
        result = command(['/usr/bin/ditto', '-x', '-k', str(archive), str(destination)])
    else:
        # Invoke the system GUI extractor on the verified original tar.gz. Do not
        # modify its preferences or replace quarantine inheritance with copies.
        if not expected_files:
            raise ValueError('Archive Utility requires the verified native member hashes')
        produced = archive.with_name(native_package.archive_stem(archive))
        if produced.exists() or produced.is_symlink():
            raise ValueError('Archive Utility output already exists')
        result = command(['/usr/bin/open', '-a',
                          '/System/Library/CoreServices/Applications/Archive Utility.app', str(archive)])
        if not result['exit']:
            deadline = time.monotonic() + TIMEOUT
            while time.monotonic() < deadline:
                if produced.is_symlink():
                    raise ValueError('Archive Utility produced a symlink')
                if produced.is_dir() and {path.name for path in produced.iterdir()} == set(expected_files):
                    if all(not (produced / name).is_symlink() and (produced / name).is_file()
                           and native_package.digest((produced / name).read_bytes()) == item['sha256']
                           for name, item in expected_files.items()):
                        # Same-filesystem rename preserves the actual inherited attributes.
                        produced.rename(destination / produced.name)
                        break
                time.sleep(.5)
            else:
                raise RuntimeError('Archive Utility did not produce the exact candidate; native UI may require a person')
    if result['exit']:
        raise RuntimeError('native OS extractor failed: ' + json.dumps(result))
    return result


def browser_probe(args, approval, item, report):
    evidence = args.evidence_dir
    public = getattr(args, 'public_release', False)
    if public:
        import qualify_public_release
        url = qualify_public_release.release_url(approval, item['archive'])
        expected_hash = item['sha256']
    else:
        release_stage.inspect_platform_request(approval, api_json)
        url = signed_redirect(item['artifact']['artifact_id'])
        expected_hash = item['artifact']['sha256']
    download = evidence / 'browser-download'
    download.mkdir()
    if sys.platform == 'win32':
        policy = windows_policy()
        report['windows_policy_before_download'] = policy
        row(report, 'security_policy', 'passed' if policy['available'] else 'unmet', policy)
    # Keep the original browser/profile alive until the original file is consumed.
    # Chromium may still finalize a download/reputation decision after bytes appear.
    observation, outer = {}, None
    try:
        with tempfile.TemporaryDirectory(prefix='twigsmith-browser-profile-') as profile:
            with browser_download(url, download, Path(profile), expected_hash, observation) as (outer, observation):
                inspect_browser_download(args, approval, item, report, outer, observation)
    finally:
        observation.setdefault('lifecycle', []).append(download_snapshot(download, 'after_profile_cleanup'))
        report['browser_lifecycle'] = clean(observation)
        if outer is not None:
            retained = outer.is_file() and not outer.is_symlink()
            retained = retained and native_package.digest(outer.read_bytes()) == expected_hash
            metadata = security_metadata(outer)
            row(report, 'original_retained_after_browser_close',
                'passed' if retained and browser_origin_observed(metadata) else 'unmet',
                {'same_original_hash': bool(retained), 'security_metadata': metadata})
        if sys.platform == 'win32':
            report['windows_policy_after_download'] = windows_policy()


def inspect_browser_download(args, approval, item, report, outer, observation):
    evidence = args.evidence_dir
    observation['lifecycle'].append(download_snapshot(outer.parent, 'before_original_metadata'))
    row(report, 'browser_download', 'passed', observation)
    marks = {'outer': security_metadata(outer)}
    row(report, 'outer_security_metadata', 'passed' if browser_origin_observed(marks['outer']) else 'unmet', marks['outer'])
    stem = native_package.archive_stem(item['archive'])
    if getattr(args, 'public_release', False):
        import qualify_public_release
        if outer.name != item['archive']:
            raise ValueError('browser original name differs from approved public archive')
        # Sidecars are anonymously fetched; the browser-original native archive
        # remains untouched, including its real security metadata.
        for filename, expected in ((stem + '.manifest.json', item['manifest_sha256']),
                                   (stem + '.sha256', None)):
            qualify_public_release.public_download(qualify_public_release.release_url(approval, filename),
                                                  outer.parent / filename, expected)
        archive = outer
        outer_result = {'route': 'direct-public-native-archive'}
    else:
        # Existing exact ZIP verifier supplies safe allowlists BEFORE invoking the OS.
        names = [item['archive'], stem + '.manifest.json', stem + '.sha256']
        with tempfile.TemporaryDirectory(prefix='twigsmith-zip-inspection-') as temporary:
            release_stage.extract_artifact(outer.read_bytes(), item['artifact'], Path(temporary) / 'checked', names)
        extracted = evidence / 'os-outer-extraction'
        outer_result = os_extract(outer, extracted)
        observation['lifecycle'].append(download_snapshot(outer.parent, 'after_outer_extraction'))
        archive = extracted / item['archive']
    manifest = verify_candidate(archive, approval, item)
    marks['archive'] = security_metadata(archive)
    row(report, 'archive_identity', 'passed', {'manifest': manifest, 'archive_sha256': item['sha256']})
    payload_root = evidence / 'os-native-extraction'
    extraction = os_extract(archive, payload_root, manifest['files'])
    artifact = payload_root / stem
    binaries = {name: artifact / name for name in native_package.executable_names(item['target'])}
    if (not artifact.is_dir() or {path.name for path in artifact.iterdir()} != set(manifest['files'])
            or any(path.is_symlink() or not path.is_file()
                   or native_package.digest(path.read_bytes()) != manifest['files'][name]['sha256']
                   for name, path in ((path.name, path) for path in artifact.iterdir()))):
        raise ValueError('OS-extracted candidate differs from verified original archive')
    row(report, 'native_extraction', 'passed', {'outer': outer_result, 'native': extraction})
    for name, binary in binaries.items():
        marks[name] = security_metadata(binary)
    row(report, 'inherited_security_metadata',
        'passed' if extraction_metadata_observed(marks) else 'unmet', marks)
    if sys.platform == 'win32' and any(marks[name].get('state') == 'observed_absent' for name in binaries):
        report.setdefault('limits', []).append(
            'Extracted Windows payloads lack Zone.Identifier; terminal launch did not exercise payload-MOTW-triggered security or SmartScreen UI.')
    if sys.platform == 'darwin':
        policy = {'gatekeeper': command(['/usr/sbin/spctl', '--status']),
                  'system_integrity': command(['/usr/bin/csrutil', 'status'])}
        assessments = {name: {'gatekeeper': command(['/usr/sbin/spctl', '--assess', '--type', 'execute', '-vv', str(path)]),
                              'signature': command(['/usr/bin/codesign', '-dv', '--verbose=4', str(path)])}
                       for name, path in binaries.items()}
    else:
        assessments = {name: powershell('Get-AuthenticodeSignature -LiteralPath ' + ps_literal(path) +
                       ' | Select-Object Status,StatusMessage | ConvertTo-Json -Compress')
                       for name, path in binaries.items()}
    if sys.platform == 'darwin':
        policy_observed = (policy['gatekeeper']['exit'] == 0
                           and 'assessments enabled' in policy['gatekeeper']['stdout'].lower()
                           and policy['system_integrity']['exit'] == 0)
        assessment_observed = all(result['gatekeeper']['exit'] in (0, 3) for result in assessments.values())
    else:
        policy = windows_policy()
        policy_observed = policy['available']
        defender = policy.get('checks', {}).get('defender', {}).get('value', {})
        disabled = [name for name in ('RealTimeProtectionEnabled', 'BehaviorMonitorEnabled', 'IoavProtectionEnabled')
                    if defender.get(name) is False]
        if disabled:
            report.setdefault('limits', []).append('Hosted Windows reports disabled Defender components: ' + ', '.join(disabled))
        assessment_observed = all(result['exit'] == 0 for result in assessments.values())
    row(report, 'security_policy', 'passed' if policy_observed else 'unmet', policy)
    row(report, 'security_assessment', 'passed' if assessment_observed else 'unmet', assessments)
    runtime_env = child_env()
    runtime_env['PATH'] = str(artifact) + os.pathsep + runtime_env.get('PATH', '')
    for name, path in binaries.items():
        # Use the PATH route while retaining the exact downloaded payload identity.
        actual = shutil.which(name, path=runtime_env['PATH'])
        if not actual or Path(actual).resolve() != path.resolve():
            raise ValueError('installed alias PATH resolved to another payload')
        result = command([actual, '--version'])
        success = (result['exit'] == 0 and result['stdout'].strip() == 'twigsmith ' + approval['version']
                   and not result['stderr'])
        row(report, 'first_launch_' + name.removesuffix('.exe'), 'passed' if success else 'failed', result)
    if args.harness_dir is None:
        return
    fixture_report = {'kind': 'browser-original-native', 'profile': 'ordinary-git'}
    harness = args.harness_dir.resolve()
    harness_identity = qualify_platform.harness_manifest(harness, item['target'])
    if harness_identity['source'] != item['source']:
        raise ValueError('native harness differs from approved source')
    selected = {'native_journey': ['native_start_status_journey']}
    fixture_ok = qualify_platform.run_fixtures(artifact, harness, evidence, fixture_report,
        env=runtime_env, selected_cases=selected)
    row(report, 'ordinary_git_journey', 'passed' if fixture_ok and fixture_report['tests'] else 'failed', fixture_report)
    if not getattr(args, 'public_release', False):
        release_stage.inspect_platform_request(approval, api_json)


class MicrosoftRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        validate_microsoft_url(newurl)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def validate_microsoft_url(url):
    parsed = urllib.parse.urlsplit(url)
    if (parsed.scheme != 'https' or parsed.username or parsed.password
            or parsed.port not in (None, 443)
            or parsed.hostname not in ('aka.ms', 'download.visualstudio.microsoft.com', 'download.microsoft.com')):
        raise ValueError('redistributable redirect is outside official Microsoft download hosts')


def redist_download(path, expected):
    release_set.expected_hash(expected)
    opener = urllib.request.build_opener(MicrosoftRedirect)
    with opener.open(REDIST, timeout=60) as response, path.open('xb') as destination:
        validate_microsoft_url(response.url)
        total = 0
        while chunk := response.read(1024 * 1024):
            total += len(chunk)
            if total > 100 * 1024 * 1024:
                raise ValueError('official redistributable exceeds finite download limit')
            destination.write(chunk)
    if native_package.digest(path.read_bytes()) != expected:
        raise ValueError('official redistributable differs from externally approved SHA-256')
    signature = powershell("$ErrorActionPreference='Stop'; $f=" + ps_literal(path) +
        "; $s=Get-AuthenticodeSignature -LiteralPath $f; [pscustomobject]@{" +
        "status=$s.Status.ToString(); subject=$s.SignerCertificate.Subject; " +
        "thumbprint=$s.SignerCertificate.Thumbprint; version=(Get-Item -LiteralPath $f).VersionInfo.FileVersion}" +
        ' | ConvertTo-Json -Compress')
    if signature['exit']:
        raise ValueError('could not verify official redistributable signature/version')
    record = json.loads(signature['stdout'])
    if record['status'] != 'Valid' or not re.search(r'(^|,\s*)O=Microsoft Corporation(,|$)', record['subject']):
        raise ValueError('official redistributable lacks a valid Microsoft Authenticode signature')
    return {'sha256': expected, 'version': record['version'], 'signature': record['status'],
            'signer_thumbprint': record['thumbprint']}


# All writes occur in the disposable process-isolated container. Only the two
# approved aliases and the verified official installer are mounted read-only.
CONTAINER_SCRIPT = r'''
$ErrorActionPreference='Stop'
function Dlls {
  @(Get-ChildItem C:\Windows\System32\vcruntime*.dll,C:\Windows\System32\msvcp140*.dll -ErrorAction SilentlyContinue |
    ForEach-Object { [pscustomobject]@{name=$_.Name;version=$_.VersionInfo.FileVersion;path=$_.FullName} })
}
function Launch($name) {
  try {
    $s=New-Object System.Diagnostics.ProcessStartInfo
    $s.FileName="C:\candidate\$name.exe"; $s.Arguments='--version'
    $s.UseShellExecute=$false; $s.RedirectStandardOutput=$true; $s.RedirectStandardError=$true
    $p=New-Object System.Diagnostics.Process; $p.StartInfo=$s; [void]$p.Start()
    if (!$p.WaitForExit(20000)) { $p.Kill(); return @{timeout=$true} }
    return @{exit=$p.ExitCode;stdout=$p.StandardOutput.ReadToEnd();stderr=$p.StandardError.ReadToEnd()}
  } catch { return @{error=$_.Exception.Message} }
}
$required=@('VCRUNTIME140.dll')
$before=@(Dlls); $requiredBefore=@($before | Where-Object { $required -contains $_.name })
$absent=@{}; $after=@{}; $installed=@{}; $code=$null; $log=''
foreach ($name in @('twigsmith','tgi')) { $absent[$name]=Launch $name }
if ($requiredBefore.Count -eq 0 -and $absent.twigsmith.exit -eq -1073741515 -and $absent.tgi.exit -eq -1073741515) {
  $p=Start-Process -FilePath C:\candidate\vc_redist.x64.exe -ArgumentList '/install /quiet /norestart /log C:\runtime-install.log' -PassThru
  if (!$p.WaitForExit(180000)) { $p.Kill(); $code='timeout' } else { $code=$p.ExitCode }
  $after=@(Dlls)
  if (Test-Path C:\runtime-install.log) { $log=(Get-Content C:\runtime-install.log -Tail 120 | Out-String) }
  foreach ($name in @('twigsmith','tgi')) { $installed[$name]=Launch $name }
}
$shell=@{version=$PSVersionTable.PSVersion.ToString();edition=$PSVersionTable.PSEdition;process_bits=([IntPtr]::Size*8);executable=[System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName}
[pscustomobject]@{powershell=$shell;required_dlls=$required;before=$before;required_before=$requiredBefore;absent=$absent;installer_exit=$code;after=$after;installed=$installed;installer_log=$log} | ConvertTo-Json -Depth 8 -Compress
'''


def encoded_powershell_arguments(script):
    return ['-EncodedCommand', base64.b64encode(script.encode('utf-16-le')).decode('ascii')]


def container_script_transport():
    payload = CONTAINER_SCRIPT.encode('utf-16-le')
    return {'argument': '-EncodedCommand', 'encoding': 'utf-16-le',
            'decoded_script_sha256': hashlib.sha256(payload).hexdigest(),
            'decoded_script_bytes': len(payload), 'stdin_required': False}


def run_container(docker, image, candidate, name):
    try:
        return command([docker, 'run', '--rm', '--name', name,
                        '--isolation=process', '--network=none',
                        '--mount', f'type=bind,source={candidate},target=C:\\candidate,readonly',
                        image, 'powershell.exe', '-NoLogo', '-NoProfile', '-NonInteractive',
                        *encoded_powershell_arguments(CONTAINER_SCRIPT)], timeout=300, max_stdout=None)
    finally:
        command([docker, 'rm', '--force', name], timeout=30)


def runtime_host_state():
    return powershell("$ErrorActionPreference='Stop'; [pscustomobject]@{" +
        "services=@(Get-CimInstance Win32_Service -Filter \"Name='docker' OR Name='com.docker.service' OR Name='containerd'\" | " +
        "Select-Object Name,State,StartMode); " +
        "dockerd=(Get-Command dockerd.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source); " +
        "docker_host=$env:DOCKER_HOST; docker_context=$env:DOCKER_CONTEXT}" +
        ' | ConvertTo-Json -Depth 5 -Compress')


def required_runtime_dlls(manifest):
    # The current inspected MSVC payloads require this exact VC DLL. API-set
    # imports are OS contracts, not filenames to search for in System32.
    for alias in native_package.executable_names(manifest['target']):
        imports = manifest['inspection'][alias]['msvc_runtime_imports']
        vc = {name.casefold() for name in imports if name.casefold().startswith(('vcruntime', 'msvcp', 'msvcr'))}
        if vc != {'vcruntime140.dll'}:
            raise ValueError('runtime probe requires the inspected VCRUNTIME140.dll dependency on both aliases')
    return ['VCRUNTIME140.dll']


def runtime_probe(args, approval, item, report):
    if sys.platform != 'win32':
        raise ValueError('isolated MSVC runtime qualification requires native Windows')
    manifest = verify_candidate(args.archive, approval, item)
    required_dlls = required_runtime_dlls(manifest)
    report['required_runtime_dlls'] = required_dlls
    if not args.redist_sha256:
        raise ValueError('runtime probe requires an externally approved official installer SHA-256')
    docker = shutil.which('docker')
    if not docker:
        report['runtime_host_state'] = runtime_host_state()
        return
    info = command([docker, 'info', '--format', '{{json .}}'], timeout=30, max_stdout=None)
    if info['exit']:
        report['runtime_host_state'] = runtime_host_state()
        row(report, 'isolated_container', 'unmet', info)
        return
    engine = json.loads(info['stdout'])
    if engine.get('OSType') != 'windows':
        row(report, 'isolated_container', 'unmet', {'reason': 'existing daemon is not a Windows container engine'})
        return
    report['container_engine'] = {key: engine.get(key) for key in
                                  ('ServerVersion', 'OSType', 'OSVersion', 'Architecture',
                                   'OperatingSystem', 'Isolation')}
    report['container_storage'] = []
    for location in dict.fromkeys(filter(None, (engine.get('DockerRootDir'), tempfile.gettempdir()))):
        try:
            usage = shutil.disk_usage(location)
            report['container_storage'].append({'path': location, 'total_bytes': usage.total, 'free_bytes': usage.free})
        except OSError as error:
            report['container_storage'].append({'path': location, 'unavailable': clean(str(error))})
    # The Windows 2025 runner deliberately omits cached base images. This finite
    # cold-pull diagnostic budget is separate from the 300-second container probe.
    image = command([docker, 'pull', IMAGE], timeout=900)
    report['container_pull'] = image
    if image['exit'] and not image.get('timed_out'):
        row(report, 'isolated_container', 'unmet', image)
        return
    # A killed pull client can race with daemon completion. Inspect the actual
    # local image separately; never reinterpret a timeout as command success.
    inspection = command([docker, 'image', 'inspect', IMAGE], timeout=30, max_stdout=None)
    report['container_image_inspection'] = inspection
    if inspection['exit']:
        row(report, 'isolated_container', 'unmet', {'pull': image, 'image_inspection': inspection})
        return
    identities = json.loads(inspection['stdout'])
    if (len(identities) != 1 or not re.fullmatch(r'sha256:[0-9a-f]{64}', identities[0].get('Id', ''))
            or not any(re.fullmatch(r'mcr\.microsoft\.com/windows/servercore@sha256:[0-9a-f]{64}', value)
                       for value in identities[0].get('RepoDigests', []))):
        raise ValueError('official container image has no immutable content identity')
    identity = identities[0]
    with tempfile.TemporaryDirectory(prefix='twigsmith-runtime-') as temporary:
        temporary = Path(temporary)
        extracted = temporary / 'checked'
        native_package.verify(args.archive, extracted)
        candidate = temporary / 'candidate'
        candidate.mkdir()
        for name in native_package.executable_names(item['target']):
            shutil.copyfile(extracted / native_package.archive_stem(args.archive) / name, candidate / name)
        installer = redist_download(candidate / 'vc_redist.x64.exe', args.redist_sha256)
        row(report, 'official_installer', 'passed', installer)
        container_name = 'twigsmith-runtime-' + os.urandom(8).hex()
        report['container_script_transport'] = container_script_transport()
        result = run_container(docker, identity['Id'], candidate, container_name)
    report['container'] = {'image': IMAGE, 'id': identity['Id'], 'repo_digests': identity['RepoDigests'],
                           'os_version': identity.get('OsVersion'), 'engine_version': engine.get('ServerVersion'),
                           'isolation': 'process', 'network': 'none'}
    if result['exit']:
        row(report, 'isolated_container', 'unmet', result)
        return
    try:
        observation = json.loads(result['stdout'])
    except ValueError:
        row(report, 'isolated_container', 'unmet', result)
        return
    row(report, 'isolated_container', 'passed', report['container'])
    report['runtime'] = observation
    record_runtime_observation(report, observation, manifest['version'], required_dlls)


def record_runtime_observation(report, observation, version, required_dlls):
    required = {name.casefold() for name in required_dlls}
    if required != {'vcruntime140.dll'}:
        raise ValueError('unbound runtime dependency selection')
    before = {item['name'].casefold() for item in observation['before']}
    after = {item['name'].casefold() for item in observation.get('after', [])}
    row(report, 'runtime_absent', 'passed' if required.isdisjoint(before) else 'unmet',
        {'required_dlls': required_dlls, 'family_inventory': observation['before']})
    for name in ('twigsmith', 'tgi'):
        child = observation['absent'].get(name, {})
        missing = child.get('exit') in (-1073741515, 3221225781)
        row(report, 'absent_launch_' + name, 'passed' if missing else 'unmet', child)
    installed = observation.get('installer_exit') in (0, 3010) and required.issubset(after)
    row(report, 'runtime_install', 'passed' if installed else 'unmet', {
        'exit': observation.get('installer_exit'), 'dlls': observation.get('after'),
        'log': observation.get('installer_log'), 'source': 'verified official Microsoft x64 v14 installer'})
    for name in ('twigsmith', 'tgi'):
        child = observation.get('installed', {}).get(name, {})
        success = (installed and child.get('exit') == 0
                   and child.get('stdout', '').strip() == 'twigsmith ' + version
                   and not child.get('stderr'))
        row(report, 'installed_launch_' + name, 'passed' if success else 'unmet', child)


def run(args):
    evidence = args.evidence_dir.resolve()
    if evidence.exists() or evidence.is_symlink():
        raise ValueError('evidence directory must be new')
    evidence.mkdir(parents=True)
    required = BROWSER_ROWS if args.operation == 'browser' else RUNTIME_ROWS
    report = {'schema': 1, 'kind': 'native-installation-observation', 'operation': args.operation,
              'passed': False, 'checks': {}, 'required_checks': list(required),
              'host': {'system': platform.platform(), 'architecture': platform.machine(),
                       'ci': {name: os.environ[name] for name in
                              ('GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT', 'GITHUB_SHA', 'ImageOS',
                               'ImageVersion', 'RUNNER_OS', 'RUNNER_ARCH') if name in os.environ}},
              'limits': ['Observed CI host only; Windows Server is not a Windows 11 desktop.',
                         'Read-only Actions artifact maintainer route; direct published RC downloads require later observation.',
                         'Headed browser download, OS extraction and terminal launch; native warning UI approval is not automated.',
                         'Security marks are observed before first launch and never fabricated or stripped.']}
    for name in required:
        row(report, name, 'unmet', 'not observed')
    try:
        with contextlib.ExitStack() as stack:
            public = getattr(args, 'public_release', False)
            if public:
                import qualify_public_release
                if args.operation != 'browser' or args.harness_dir is None:
                    raise ValueError('direct public browser mode requires native child fixtures')
                temporary = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix='twigsmith-public-browser-')))
                stack.enter_context(qualify_public_release.isolated_environment(
                    qualify_public_release.anonymous_environment(temporary)))
                report['limits'][1] = 'Direct public release browser route; actual unsigned security behavior is host-specific.'
                approval, item = approved(json.loads(args.approval.read_text()), args.target, public=True)
            else:
                approval, item = approved(json.loads(args.approval.read_text()), args.target)
            if sys.platform == 'win32':
                report['host']['powershell'] = powershell_host()
            report.update(target=args.target, approved_native=item,
                          approval_sha256=native_package.digest(args.approval.read_bytes()),
                          download_route='public-release' if public else 'actions-artifact')
            if args.operation == 'browser':
                browser_probe(args, approval, item, report)
            else:
                runtime_probe(args, approval, item, report)
    except (OSError, ValueError, RuntimeError, TimeoutError, subprocess.SubprocessError, KeyError) as error:
        report['error'] = clean(str(error))
    finally:
        # Existing fixture helpers also write evidence; sanitize their text logs.
        for path in evidence.rglob('*.log'):
            if path.is_file() and not path.is_symlink():
                path.write_text(clean(path.read_text(encoding='utf-8', errors='replace')), encoding='utf-8')
        report['passed'] = 'error' not in report and passed(report, required)
        (evidence / 'installation.json').write_text(json.dumps(clean(report), indent=2) + '\n', encoding='utf-8')
    print(f"{args.operation}: {'passed' if report['passed'] else 'unmet or failed'}; {evidence / 'installation.json'}")
    return 0 if report['passed'] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='operation', required=True)
    for operation in ('browser', 'runtime'):
        command_parser = commands.add_parser(operation)
        command_parser.add_argument('--approval', required=True, type=Path)
        command_parser.add_argument('--target', required=True)
        command_parser.add_argument('--evidence-dir', required=True, type=Path)
        if operation == 'browser':
            command_parser.add_argument('--harness-dir', type=Path)
            command_parser.add_argument('--public-release', action='store_true',
                                        help='observe exact native archive from the public release anonymously')
        else:
            command_parser.add_argument('--archive', required=True, type=Path)
            command_parser.add_argument('--redist-sha256', required=True)
    return run(parser.parse_args())


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError) as error:
        print(clean(str(error)), file=sys.stderr)
        sys.exit(1)
