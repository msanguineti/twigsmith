"""Anonymous consumer boundaries; synthetic fixtures never qualify a product host."""
from copy import deepcopy
from datetime import datetime, timezone
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import native_package
import npm_single_package
import qualify_platform
import qualify_public_release as public
from test_release_set import platform_fixture


class VersionProbeTests(unittest.TestCase):
    def test_correct_stdout_accepts_stderr_notices_and_retains_diagnostics(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            log = root / 'version.log'
            result = public.version_probe([sys.executable, '-c',
                "import sys; print('twigsmith 1.0.0-rc.1'); print('npm notice run npx', file=sys.stderr)"],
                '1.0.0-rc.1', log, os.environ.copy(), root)
            self.assertEqual(result, {'passed': True, 'exit': 0, 'version': '1.0.0-rc.1'})
            self.assertIn('twigsmith 1.0.0-rc.1', log.read_text())
            self.assertIn('npm notice run npx', log.read_text())

    def test_wrong_stdout_or_failed_command_cannot_pass_from_stderr_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for stdout, exit_code in [('twigsmith 0.1.0', 0), ('', 0), ('twigsmith 1.0.0-rc.1', 1)]:
                script = (f"import sys; print({stdout!r}); "
                          "print('twigsmith 1.0.0-rc.1', file=sys.stderr); "
                          f"sys.exit({exit_code})")
                with self.subTest(stdout=stdout, exit=exit_code), self.assertRaisesRegex(
                        ValueError, 'did not execute the approved version'):
                    public.version_probe([sys.executable, '-c', script], '1.0.0-rc.1',
                                         root / 'version.log', os.environ.copy(), root)


class PublicConsumptionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fixture = tempfile.TemporaryDirectory()
        cls.fixture_root = Path(cls.fixture.name)
        cls.approval, _ = platform_fixture(cls.fixture_root)
        cls.approval['publication'] = {'source': cls.approval['native'][0]['source']['revision'],
            'channel': 'next', 'staging_channel': 'public-fixture', 'prerelease': True, 'release_id': 1}

    @classmethod
    def tearDownClass(cls):
        cls.fixture.cleanup()

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.record = deepcopy(self.approval)
        self.target = 'x86_64-unknown-linux-gnu'
        self.item = next(row for row in self.record['native'] if row['target'] == self.target)

    def passing_report(self, target=None):
        target = target or self.target
        kinds = ['native', 'npm_local', 'npm_global', 'npx_twigsmith', 'npx_tgi']
        if not target.endswith('linux-gnu'):
            kinds.append('browser')
        result = {'schema': 1, 'approval_sha256': public.canonical_hash(self.record), 'target': target,
                  'observed_at': datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z'),
                  'producer': {'publisher_revision': 'f' * 40, 'run_id': '999', 'run_attempt': '2'},
                  'checks': {}, 'passed': True}
        for kind in kinds:
            detail = {'passed': True, 'tests': [{'suite': suite, 'name': name, 'passed': True, 'exit': 0}
                       for suite, names in public.expected_cases(kind).items() for name in names]}
            aliases = {name: {'passed': True, 'exit': 0, 'version': self.record['version']}
                       for name in ('twigsmith', 'tgi')}
            result['checks'][kind] = public.check_record(kind, self.record, target, detail, aliases)
        return result

    def test_summary_binds_whole_approval_and_exact_case_alias_hash_evidence(self):
        report = self.passing_report()
        self.assertEqual(public.validate_report(report, self.record, self.target), report)
        changed = deepcopy(self.record)
        changed['publication']['release_id'] += 1
        self.assertNotEqual(public.canonical_hash(changed), report['approval_sha256'])
        with self.assertRaises(ValueError):
            public.validate_report(report, changed, self.target)
        changes = [lambda row: row['checks'].pop('npx_tgi'),
                   lambda row: row['checks']['native'].update(cases=[]),
                   lambda row: row['checks']['native']['cases'][0].update(passed='skipped'),
                   lambda row: row['checks']['native']['cases'][0].update(exit=True),
                   lambda row: row['checks']['native']['identity'].update(payload_sha256='0' * 64),
                   lambda row: row['checks']['npm_global']['identity']['package_sha256'].pop('twigsmith'),
                   lambda row: row['checks']['npm_global']['cases'].pop(),
                   lambda row: row['checks']['npx_tgi'].update(cold_cache=False),
                   lambda row: row['checks']['npx_tgi'].update(invoked_alias='twigsmith'),
                   lambda row: row['checks']['native']['aliases'].pop('tgi'),
                   lambda row: row['checks']['native']['aliases']['twigsmith'].update(passed=1),
                   lambda row: row['checks']['native']['aliases']['tgi'].update(exit=False),
                   lambda row: row['checks']['native'].update(report='../native/qualification.json')]
        for mutate in changes:
            candidate = deepcopy(report)
            mutate(candidate)
            with self.subTest(mutation=changes.index(mutate)), self.assertRaises(ValueError):
                public.validate_report(candidate, self.record, self.target)

    def test_browser_required_on_each_actual_windows_mac_target(self):
        for target in npm_single_package.TARGETS:
            report = self.passing_report(target)
            public.validate_report(report, self.record, target)
            if target != self.target:
                report['checks'].pop('browser')
                with self.assertRaises(ValueError):
                    public.validate_report(report, self.record, target)

    def test_refuses_incomplete_approval_and_drops_only_publication_for_set_validator(self):
        core, item = public.approved(self.record, self.target)
        self.assertNotIn('publication', core)
        self.assertEqual(item, self.item)
        for key in ('publication', 'npm'):
            candidate = deepcopy(self.record)
            candidate.pop(key)
            with self.assertRaises(ValueError):
                public.approved(candidate, self.target)
        candidate = deepcopy(self.record)
        candidate['extra'] = 'must not be silently removed'
        with self.assertRaises(ValueError):
            public.approved(candidate, self.target)

    def test_child_case_selection_refuses_zero_duplicate_missing_failed_and_outer_only(self):
        wanted = public.expected_cases('npm_global')
        rows = self.passing_report()['checks']['npm_global']['cases']
        for tests in ([], rows[:1], rows + rows[:1], [dict(row, passed=False) for row in rows]):
            with self.subTest(tests=tests), self.assertRaises(ValueError):
                public.require_cases({'tests': tests}, wanted)
        self.assertFalse(qualify_platform.fixture_passed(subprocess.CompletedProcess([], 0,
            '1 passed; 0 failed\n'), 'native_start_status_journey'))
        self.assertEqual(public.require_cases({'tests': rows}, wanted), rows)

    def test_allowlist_removes_credential_configuration_and_startup_paths_from_actual_child(self):
        secrets = {name: 'private-' + name for name in ('GH_TOKEN', 'GITHUB_TOKEN', 'NPM_TOKEN',
            'NODE_AUTH_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_URL',
            'ACTIONS_RUNTIME_TOKEN', 'ACTIONS_RUNTIME_URL', 'GIT_CONFIG_COUNT', 'NODE_OPTIONS',
            'npm_config_userconfig', 'npm_config_registry', 'HTTPS_PROXY')}
        with patch.dict(os.environ, secrets):
            env = public.anonymous_environment(self.root / 'isolation')
            for name in secrets:
                self.assertNotEqual(env.get(name), secrets[name])
            self.assertEqual(Path(env['npm_config_userconfig']).read_text(), '')
            self.assertEqual(Path(env['npm_config_globalconfig']).read_text(), '')
            with public.isolated_environment(env):
                child = subprocess.check_output([sys.executable, '-c',
                    'import os,json; print(json.dumps(dict(os.environ)))'], text=True)
            self.assertEqual(os.environ['GH_TOKEN'], secrets['GH_TOKEN'])
            for value in secrets.values():
                self.assertNotIn(value, child)

    def test_anonymous_windows_browser_retains_each_nonsecret_edge_installation_root(self):
        for variable in ('PROGRAMFILES(X86)', 'PROGRAMFILES', 'LOCALAPPDATA'):
            with self.subTest(variable=variable):
                installation = self.root / variable
                edge = installation / 'Microsoft/Edge/Application/msedge.exe'
                edge.parent.mkdir(parents=True)
                edge.write_bytes(b'fake installed Edge; discovery only')
                with patch.dict(os.environ, {variable: str(installation), 'GITHUB_TOKEN': 'private-token'}, clear=True):
                    env = public.anonymous_environment(self.root / ('home-' + variable))
                    self.assertEqual(env[variable], str(installation))
                    self.assertNotIn('GITHUB_TOKEN', env)
                    with public.isolated_environment(env), \
                         patch.object(public.qualify_installation.sys, 'platform', 'win32'):
                        self.assertEqual(public.qualify_installation.browser_binary(), edge)

    def test_download_anonymous_proxy_free_hash_verified_and_redirect_origins_restricted(self):
        seen = []
        class Response(io.BytesIO):
            status = 200
            def geturl(self):
                return 'https://release-assets.githubusercontent.com/fixture?sig=ephemeral'
        class Opener:
            def open(self, request, timeout):
                seen.append(request)
                return Response(b'approved bytes')
        destination = self.root / 'download'
        with patch.dict(os.environ, {'GITHUB_TOKEN': 'never-send'}), \
             patch.object(public.urllib.request, 'build_opener', return_value=Opener()) as build:
            public.public_download('https://github.com/msanguineti/twigsmith/releases/download/v1/test',
                                   destination, native_package.digest(b'approved bytes'))
            self.assertEqual(destination.read_bytes(), b'approved bytes')
            self.assertEqual(build.call_args.args[0].proxies, {})
            self.assertNotIn('Authorization', dict(seen[0].header_items()))
            with self.assertRaisesRegex(ValueError, 'hash'):
                public.public_download('https://github.com/fixture', self.root / 'wrong', '0' * 64)
            self.assertFalse((self.root / 'wrong').exists())
            self.assertFalse((self.root / 'wrong.partial').exists())
        for url in ('http://github.com/file', 'https://user:password@github.com/file',
                    'https://github.com:444/file', 'https://evil.invalid/file',
                    'https://registry.npmjs.org@evil.invalid/file'):
            with self.subTest(url=url), self.assertRaises(ValueError):
                public.PublicRedirect({'github.com'}).redirect_request(None, None, 302, '', {}, url)

    def installed_fixture(self):
        manifest, packages = qualify_platform.package_set(self.fixture_root / 'npm')
        expected = {'twigsmith': public.package_contents(packages['twigsmith'][2])}
        common = self.root / 'node_modules/twigsmith'
        common.mkdir(parents=True)
        for member, data in expected['twigsmith'].items():
            (common / member.removeprefix('package/')).write_bytes(data)
        native = json.loads(expected['twigsmith']['package/native.json'])['targets'][self.target]
        directory = self.target + '-' + self.item['payload_sha256']
        payload = common / '.native' / directory / native['payload']
        payload.parent.mkdir(parents=True)
        # The shared fixture uses synthetic native archive bytes, never host qualification.
        archive = self.fixture_root / 'native' / self.item['archive']
        with tarfile.open(archive) as tar:
            payload.write_bytes(tar.extractfile(next(m for m in tar.getmembers()
                if m.name.endswith('/' + native['payload']))).read())
        payload.chmod(0o755)
        state = common / '.native/installed.json'
        state.write_text(json.dumps({'target': self.target, 'directory': directory}))
        return common, payload, expected

    def test_actual_installed_tree_binds_immutable_package_state_target_and_native_hash(self):
        common, payload, expected = self.installed_fixture()
        node = Path(shutil.which('node')).resolve()
        self.assertEqual(public.installed_identity(common, node, expected, self.record, self.item,
                                                  os.environ, self.root), payload)
        original = payload.read_bytes()
        payload.write_bytes(original + b'changed')
        with self.assertRaisesRegex(ValueError, 'payload/tree'):
            public.installed_identity(common, node, expected, self.record, self.item, os.environ, self.root)
        payload.write_bytes(original)
        (common / 'injected.cjs').write_text('extra')
        with self.assertRaisesRegex(ValueError, 'tarball bytes'):
            public.installed_identity(common, node, expected, self.record, self.item, os.environ, self.root)

    def test_generated_state_rejects_extra_payloads_wrong_target_stale_hash_and_links(self):
        common, payload, expected = self.installed_fixture()
        node = Path(shutil.which('node')).resolve()
        state = common / '.native/installed.json'
        original = state.read_bytes()
        for changed in ({'target': self.target, 'directory': '../escape'},
                        {'target': 'unsupported', 'directory': payload.parent.name},
                        {'target': self.target, 'directory': payload.parent.name, 'extra': True}):
            state.write_text(json.dumps(changed))
            with self.subTest(state=changed), self.assertRaises(ValueError):
                public.installed_identity(common, node, expected, self.record, self.item, os.environ, self.root)
        state.write_bytes(original)
        extra = payload.with_name('unexpected')
        extra.write_bytes(b'extra')
        with self.assertRaisesRegex(ValueError, 'payload/tree'):
            public.installed_identity(common, node, expected, self.record, self.item, os.environ, self.root)
        extra.unlink()
        if os.name != 'nt':
            saved = payload.read_bytes()
            payload.unlink()
            payload.symlink_to(state)
            with self.assertRaisesRegex(ValueError, 'link|member'):
                public.installed_identity(common, node, expected, self.record, self.item, os.environ, self.root)
            payload.unlink()
            payload.write_bytes(saved)

    def test_refusal_probe_fails_any_network_attempt_even_when_launcher_refuses(self):
        common, payload, expected = self.installed_fixture()
        (common / 'launcher.cjs').write_text("try { require('node:https').request('https://github.com/'); } "
            "catch(error) {} console.error('run twigsmith-install'); process.exit(1);\n")
        node = Path(shutil.which('node')).resolve()
        with self.assertRaisesRegex(ValueError, 'refusal probes failed'):
            public.npm_failures(common, node, self.target, self.record['version'], self.root, os.environ)
        for name in ('missing-state', 'corrupt-state', 'corrupt-payload'):
            self.assertIn('TWIGSMITH_NETWORK_ATTEMPT', (self.root / (name + '.log')).read_text())

    def test_loopback_repack_is_nonpublishable_and_preserves_reviewed_code_and_scripts(self):
        manifest, production = qualify_platform.package_set(self.fixture_root / 'npm')
        import npm_single_package
        original = npm_single_package.pack
        def corrupt(directory, staged, metadata, files, env):
            changed = dict(files, **{'installer.cjs': b'unreviewed installer'})
            return original(directory, staged, metadata, changed, env)
        env = public.anonymous_environment(self.root / 'host')
        with patch.object(npm_single_package, 'pack', side_effect=corrupt):
            with self.assertRaisesRegex(ValueError, 'reviewed installer/launcher'):
                qualify_platform.fixture_package(production, 'http://127.0.0.1:1234/', self.root / 'packed', env)
        # Approval of a tarball alone does not permit fixture metadata or a changed
        # target table to masquerade as the reviewed production table.
        manifest['native'][0]['payload_sha256'] = '0' * 64
        with self.assertRaisesRegex(ValueError, 'production npm native metadata'):
            qualify_platform.verified_native_resources(manifest, production, self.fixture_root / 'native')

    def test_native_fixture_reobserves_archive_bytes_before_transport(self):
        manifest, production = qualify_platform.package_set(self.fixture_root / 'npm')
        native = self.root / 'native'
        shutil.copytree(self.fixture_root / 'native', native)
        path = native / manifest['native'][0]['archive']
        path.write_bytes(path.read_bytes() + b'changed')
        with self.assertRaisesRegex(ValueError, 'native fixture archive'):
            qualify_platform.verified_native_resources(manifest, production, native)

    def test_npx_rejects_global_alias_other_cache_and_wrong_alias(self):
        cache = self.root / 'cache'
        common = cache / '_npx/hash/node_modules/twigsmith'
        common.mkdir(parents=True)
        (common / 'launcher.cjs').write_text('fixture')
        shim_dir = common.parent / '.bin'
        shim_dir.mkdir()
        suffix = '.cmd' if os.name == 'nt' else ''
        shim = shim_dir / ('tgi' + suffix)
        if os.name == 'nt':
            shim.write_text('fixture')
        else:
            shim.symlink_to(common / 'launcher.cjs')
        observation = self.root / 'selected.json'
        observation.write_text(json.dumps({'shim': str(shim)}))
        self.assertEqual(public.npx_common(cache, observation, 'tgi'), (common, shim_dir))
        for selected in (str(self.root / 'global/tgi'), str(shim.with_name('twigsmith' + suffix))):
            observation.write_text(json.dumps({'shim': selected}))
            with self.assertRaises(ValueError):
                public.npx_common(cache, observation, 'tgi')

    def test_file_observer_runs_under_real_node_without_launching_selected_alias(self):
        directory = self.root / 'private observer with spaces'
        directory.mkdir()
        bins = self.root / 'selected shims with spaces'
        bins.mkdir()
        node = qualify_platform.selected_npm_node(os.environ)
        env = dict(os.environ, PATH=str(bins) + os.pathsep + os.environ['PATH'])
        for alias in ('twigsmith', 'tgi'):
            shim = bins / (alias + ('.cmd' if os.name == 'nt' else ''))
            # A nonexecutable file can be observed; acquisition must never try to
            # invoke it. The existing real npm test exercises later execution.
            shim.write_text('not executable application code')
            observation = directory / (alias + ' observed shim.json')
            command = public.npx_observer_command(node, directory, alias, observation)
            self.assertEqual(len(command), 4)
            self.assertNotIn('-e', command)
            self.assertTrue(all('\n' not in arg and '\r' not in arg for arg in command))
            self.assertEqual(Path(command[1]).read_bytes(), public.NPX_OBSERVER.encode())
            output = subprocess.run(command, env=env, capture_output=True, text=True)
            self.assertEqual(output.returncode, 0, output.stderr)
            self.assertEqual(output.stdout + output.stderr, '')
            self.assertEqual(json.loads(observation.read_text()), {'shim': str(shim.resolve())})
            refused = subprocess.run(command + ['execute', str(shim.with_name('other-shim'))],
                                     env=env, capture_output=True, text=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn('another shim after verification', refused.stderr)
        Path(command[1]).write_text('substituted observer')
        with self.assertRaisesRegex(ValueError, 'observer file changed'):
            public.npx_observer_command(node, directory, 'tgi', observation)

    def test_blocked_npx_acquisition_also_passes_actual_observer_filename(self):
        evidence = self.root / 'evidence'
        evidence.mkdir()
        def acquire(command, log, env, cwd):
            self.assertNotIn('-e', command)
            self.assertNotIn(public.NPX_OBSERVER, command)
            self.assertNotIn('execute', command)
            script = Path(command[command.index('--') + 2])
            self.assertTrue(script.is_absolute())
            self.assertEqual(script.read_text(), public.NPX_OBSERVER)
            raise RuntimeError('stopped before fixture package verification')
        with patch.object(public, 'npm_install', side_effect=acquire):
            with self.assertRaisesRegex(RuntimeError, 'stopped before fixture package verification'):
                public.npx_blocked_repair(SimpleNamespace(npm=None), self.record['version'], {}, self.target,
                    self.item['payload_sha256'], qualify_platform.selected_npm_node(os.environ),
                    evidence, self.root, os.environ)
        with self.assertRaisesRegex(ValueError, 'observer did not write'):
            public.npx_common(self.root / 'cache', self.root / 'absent-observation.json', 'tgi')

    def test_windows_npx_observer_quotes_cmd_path_with_spaces_without_implicit_shell(self):
        # Execute the actual observer JavaScript in a VM with Windows path/process
        # mechanics. This proves argument construction; real cmd.exe is a hosted row.
        simulator = r"""
const vm = require('node:vm'), path = require('node:path').win32;
const shim = process.argv[2], captured = {};
const stop = {};
const fake = {platform:'win32', argv:['node','observer.cjs','tgi','observation','execute',shim],
  env:{PATH:path.dirname(shim),ComSpec:'C:\\Windows\\System32\\cmd.exe'},
  stdout:{write:s=>{captured.stdout=s;}},stderr:{write:s=>{captured.stderr=s;}},
  exit:code=>{captured.exit=code;throw stop;}};
const modules = {'node:path':{...path,delimiter:';'},
  'node:fs':{existsSync:p=>p===shim,writeFileSync:(p,data)=>{captured.observation=JSON.parse(data);}},
  'node:child_process':{spawnSync:(command,args,options)=>{
    captured.invocation={command,args,options};return {status:0,stdout:'twigsmith fixture\n',stderr:''};}}};
try {vm.runInNewContext(process.argv[1],{process:fake,require:name=>modules[name]});}
catch(error) {if(error!==stop) captured.error=error.message;}
process.stdout.write(JSON.stringify(captured));
"""
        node = shutil.which('node')
        shim = r'C:\private cache with spaces\node_modules\.bin\tgi.cmd'
        observed = json.loads(subprocess.check_output([node, '-e', simulator, public.NPX_OBSERVER, shim], text=True))
        self.assertNotIn('error', observed)
        invocation = observed['invocation']
        self.assertEqual(invocation['command'], r'C:\Windows\System32\cmd.exe')
        self.assertEqual(invocation['args'], ['/d', '/s', '/c', '""' + shim + '" --version"'])
        self.assertEqual(invocation['options'], {'encoding': 'utf8', 'shell': False, 'windowsVerbatimArguments': True})
        self.assertEqual(observed['exit'], 0)
        for unsafe in (shim.replace('private cache', '%PRIVATE% cache'), shim.replace('private cache', 'private! cache')):
            observed = json.loads(subprocess.check_output([node, '-e', simulator, public.NPX_OBSERVER, unsafe], text=True))
            self.assertIn('expansion characters', observed['error'])
            self.assertNotIn('invocation', observed)

    def test_native_archive_hash_refusal_precedes_any_alias_or_child_execution(self):
        def download(url, destination, expected=None):
            source = self.fixture_root / 'native' / destination.name
            shutil.copy2(source, destination)
            if destination.name == self.item['archive']:
                destination.write_bytes(destination.read_bytes() + b'tampered-public-response')
        with patch.object(public, 'public_download', side_effect=download), \
             patch.object(public, 'version_probe') as version, \
             patch.object(qualify_platform, 'run_fixtures') as child:
            with self.assertRaisesRegex(ValueError, 'approved bytes'):
                public.native_check(self.record, self.item, self.root, self.root, {})
        version.assert_not_called()
        child.assert_not_called()

    def test_run_records_partial_failure_and_never_marks_remaining_routes_passed(self):
        approval = self.root / 'approval.json'
        approval.write_text(json.dumps(self.record))
        args = SimpleNamespace(approval=approval, target=self.target, harness_dir=self.root,
                               evidence_dir=self.root / 'evidence')
        with patch.object(qualify_platform, 'host_target', return_value=self.target), \
             patch.object(qualify_platform, 'harness_manifest', return_value={'source': self.item['source']}), \
             patch.object(qualify_platform, 'environment', return_value={}), \
             patch.object(public, 'native_check', side_effect=ValueError('actual child failed')):
            self.assertEqual(public.run(args), 1)
        summary = json.loads((args.evidence_dir / 'public-consumption.json').read_text())
        self.assertFalse(summary['passed'])
        self.assertEqual(summary['checks'], {})
        self.assertEqual(set(summary), {'schema', 'approval_sha256', 'target', 'observed_at', 'producer', 'checks', 'passed'})
        self.assertIn('actual child failed', (args.evidence_dir / 'failure.json').read_text())

    def test_npx_acquisition_never_launches_product_before_cache_byte_verification(self):
        evidence = self.root / 'evidence'
        evidence.mkdir()
        def acquire(command, log, env, cwd):
            self.assertNotIn('-e', command)
            self.assertNotIn(public.NPX_OBSERVER, command)
            self.assertNotIn('execute', command)
            script = Path(command[command.index('--') + 2])
            self.assertTrue(script.is_absolute())
            self.assertEqual(script.read_text(), public.NPX_OBSERVER)
            self.assertIn("if (process.argv[4] !== 'execute') process.exit(0)", script.read_text())
            shim = Path(env['npm_config_cache']) / '_npx/hash/node_modules/.bin/tgi'
            (cwd / 'selected-shim.json').write_text(json.dumps({'shim': str(shim)}))
        with patch.object(public, 'npm_install', side_effect=acquire) as acquired, \
             patch.object(public, 'npx_common', return_value=(self.root / 'tampered-common', self.root / '.bin')), \
             patch.object(public, 'installed_identity', side_effect=ValueError('unapproved cache bytes')), \
             patch.object(public, 'version_probe') as product, \
             patch.object(public, 'bridge_for') as bridge:
            with self.assertRaisesRegex(ValueError, 'unapproved cache bytes'):
                public.npm_check('npx_tgi', self.record, self.item, self.root, {}, evidence,
                                 self.root, os.environ, {})
        acquired.assert_called_once()
        product.assert_not_called()
        bridge.assert_not_called()

    @unittest.skipIf(os.name == 'nt', 'synthetic shell payload is Unix only; Windows requires hosted native row')
    def test_real_npm_global_policy_and_cold_npx_blocked_repair(self):
        import test_npm_single_package as fixtures
        # Explicit synthetic shell fixture: transport, policy and repair behavior
        # only; these processes do not establish product/native real-Git acceptance.
        fixture = fixtures.SinglePackageTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        payload = ("#!/bin/sh\nprintf 'twigsmith " + fixtures.VERSION + "\\n'\n").encode()
        tarball, native = fixture.fixture(payload=payload)
        files = public.package_contents(tarball)
        metadata = json.loads(files['package/package.json'])
        item = {'name': 'twigsmith', 'version': fixtures.VERSION, 'filename': tarball.name,
                'sha256': qualify_platform.digest(tarball)}
        available = [Path(shutil.which('npm'))]
        packages = {'twigsmith': (item, metadata, tarball)}
        with qualify_platform.local_registry(packages) as (registry, requests):
            for index, npm in enumerate(available):
                with self.subTest(npm=str(npm)):
                    root = self.root / str(index)
                    root.mkdir()
                    env = public.anonymous_environment(root / 'host')
                    env['npm_config_registry'] = registry
                    node = qualify_platform.selected_npm_node(env)
                    args = SimpleNamespace(npm=npm)
                    command = qualify_platform.npm_invocation(args)
                    version = subprocess.check_output(command + ['--version'], env=env, text=True).strip()
                    default = root / 'default-policy'
                    default.mkdir()
                    before = len(fixture.requests)
                    public.npm_install(command + ['install', '--prefix', str(default), '--no-audit', '--no-fund', str(tarball)],
                                       root / 'default.log', env, default)
                    default_common = default / 'node_modules/twigsmith'
                    if int(version.split('.')[0]) >= 12:
                        self.assertEqual(len(fixture.requests), before)
                        self.assertFalse((default_common / '.native').exists())
                        result = subprocess.run([str(node), str(default_common / 'launcher.cjs'), '--version'],
                                                env=env, capture_output=True, text=True)
                        self.assertNotEqual(result.returncode, 0)
                        self.assertIn('twigsmith-install', result.stderr)
                        self.assertEqual(len(fixture.requests), before)
                    else:
                        qualify_platform.installed_payload(default_common, files, fixtures.TARGET,
                                                           native['targets'][fixtures.TARGET]['payload_sha256'])
                    prefix = root / 'global'
                    flags = qualify_platform.script_policy(version, str(tarball))
                    public.npm_install(command + ['install', '--global', '--prefix', str(prefix), *flags,
                                                 '--no-audit', '--no-fund', str(tarball)],
                                       root / 'global.log', env, root)
                    common = prefix / 'lib/node_modules/twigsmith'
                    qualify_platform.installed_payload(common, files, fixtures.TARGET,
                                                        native['targets'][fixtures.TARGET]['payload_sha256'])
                    public.version_probe([str(prefix / 'bin/tgi'), '--version'], fixtures.VERSION,
                                         root / 'version.log', env, root)
                    result = public.npx_blocked_repair(args, fixtures.VERSION, {'twigsmith': files}, fixtures.TARGET,
                        native['targets'][fixtures.TARGET]['payload_sha256'], node, root, root, env)
                    self.assertTrue(result['passed'])
                    self.assertTrue(result['cold_cache'])
                    local = root / 'local'
                    local.mkdir()
                    local_flags = qualify_platform.script_policy(version, 'twigsmith', local)
                    public.npm_install(command + ['install', '--prefix', str(local), *local_flags,
                                                 '--no-audit', '--no-fund', 'twigsmith@' + fixtures.VERSION],
                                       root / 'local.log', env, local)
                    qualify_platform.installed_payload(local / 'node_modules/twigsmith', files, fixtures.TARGET,
                                                        native['targets'][fixtures.TARGET]['payload_sha256'])
                    with patch.object(public, 'REGISTRY', registry):
                        self.assertTrue(public.blocked_repair({'version': fixtures.VERSION},
                            {'target': fixtures.TARGET, 'payload_sha256': native['targets'][fixtures.TARGET]['payload_sha256']},
                            {'twigsmith': files}, node, args, root, root, env)['passed'])
                    for alias in ('twigsmith', 'tgi'):
                        npx_env = dict(env, npm_config_cache=str(root / ('approved-cache-' + alias)))
                        observation = root / ('approved-' + alias + '.json')
                        npx = qualify_platform.npm_invocation(args, npx=True)
                        acquire = npx + ['--yes', *qualify_platform.script_policy(version, 'twigsmith'),
                            '--package=twigsmith@' + fixtures.VERSION, '--',
                            *public.npx_observer_command(node, root, alias, observation)]
                        public.npm_install(acquire, root / (alias + '-acquire.log'), npx_env, root)
                        cached, _ = public.npx_common(Path(npx_env['npm_config_cache']), observation, alias)
                        qualify_platform.installed_payload(cached, files, fixtures.TARGET,
                                                           native['targets'][fixtures.TARGET]['payload_sha256'])
                        shim = json.loads(observation.read_text())['shim']
                        public.version_probe(npx + ['--offline', *acquire[len(npx):], 'execute', shim],
                            fixtures.VERSION, root / (alias + '-offline-version.log'),
                            dict(npx_env, npm_config_offline='true'), root)

    @unittest.skipIf(os.name == 'nt', 'real Unix/npm transport; Windows uses hosted native evidence')
    def test_real_npm_global_local_and_independent_npx_verify_before_execution(self):
        # Real npm install/npx/installer acquisition with shared synthetic native
        # archives. Execution boundary is deliberately stopped after byte checks:
        # these are synthetic fixture bytes and must never count as real-Git proof.
        manifest, production = qualify_platform.package_set(self.fixture_root / 'npm')
        resources = qualify_platform.verified_native_resources(manifest, production, self.fixture_root / 'native')
        packages = {}
        evidence = self.root / 'evidence'
        evidence.mkdir()
        work = self.root / 'work'
        work.mkdir()
        with qualify_platform.local_registry(packages, resources) as (registry, requests):
            env = public.anonymous_environment(self.root / 'host')
            packages['twigsmith'] = qualify_platform.fixture_package(production, registry, self.root / 'packed', env)
            expected = {'twigsmith': public.package_contents(packages['twigsmith'][2])}
            real_identity = public.installed_identity
            def inspect(*args, **kwargs):
                result = real_identity(*args, **kwargs)
                self.assertEqual(qualify_platform.digest(result), self.item['payload_sha256'])
                raise RuntimeError('verified synthetic bytes; execution intentionally stopped')
            with patch.object(public, 'REGISTRY', registry), public.isolated_environment(env), \
                 patch.object(public, 'installed_identity', side_effect=inspect), \
                 patch.object(public, 'version_probe') as execution:
                for kind in ('npm_local', 'npm_global', 'npx_twigsmith', 'npx_tgi'):
                    with self.subTest(kind=kind), self.assertRaisesRegex(RuntimeError, 'verified synthetic bytes'):
                        public.npm_check(kind, self.record, self.item, self.root, {}, evidence, work, env, expected)
                execution.assert_not_called()
            self.assertEqual(requests.count('native/' + self.item['archive']), 4)
            self.assertEqual(requests.count('tarballs/' + packages['twigsmith'][2].name), 4)



if __name__ == '__main__':
    unittest.main()
