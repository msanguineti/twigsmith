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
import npm_platform_package
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
        kinds = ['native', 'npm_global', 'npx_twigsmith', 'npx_tgi']
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
        for target in npm_platform_package.PACKAGES:
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

    def test_actual_installed_tree_and_node_resolution_bind_common_host_and_native_hash(self):
        manifest, packages = qualify_platform.package_set(self.fixture_root / 'npm')
        expected = {name: public.package_contents(value[2]) for name, value in packages.items()}
        common = self.root / 'node_modules/twigsmith'
        host = self.root / 'node_modules' / npm_platform_package.PACKAGES[self.target]
        for directory, name in ((common, 'twigsmith'), (host, npm_platform_package.PACKAGES[self.target])):
            directory.mkdir(parents=True)
            for member, data in expected[name].items():
                (directory / member.removeprefix('package/')).write_bytes(data)
        node = Path(shutil.which('node')).resolve()
        payload = public.installed_identity(common, node, expected, self.record, self.item,
                                            os.environ, self.root)
        self.assertEqual(qualify_platform.digest(payload), self.item['payload_sha256'])
        original = payload.read_bytes()
        payload.write_bytes(original + b'changed')
        with self.assertRaisesRegex(ValueError, 'tarball bytes'):
            public.installed_identity(common, node, expected, self.record, self.item, os.environ, self.root)
        payload.write_bytes(original)
        (common / 'injected.cjs').write_text('extra')
        with self.assertRaisesRegex(ValueError, 'tarball bytes'):
            public.installed_identity(common, node, expected, self.record, self.item, os.environ, self.root)

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

    def test_windows_npx_observer_quotes_cmd_path_with_spaces_without_implicit_shell(self):
        # Execute the actual observer JavaScript in a VM with Windows path/process
        # mechanics. This proves argument construction; real cmd.exe is a hosted row.
        simulator = r"""
const vm = require('node:vm'), path = require('node:path').win32;
const shim = process.argv[2], captured = {};
const stop = {};
const fake = {platform:'win32', argv:['observer','tgi','observation','execute',shim],
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
            self.assertIn(public.NPX_OBSERVER, command)
            self.assertNotIn('execute', command)
            self.assertIn("if (process.argv[3] !== 'execute') process.exit(0)", public.NPX_OBSERVER)
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

    @unittest.skipIf(os.name == 'nt', 'local synthetic Unix script payload; Windows uses hosted native evidence')
    def test_real_npm_global_and_separate_npx_caches_select_exact_payload_and_run_aliases(self):
        # Real npm/npx/Node transport with an explicitly synthetic shell payload.
        # Child fixture execution is stubbed: this is packaging-boundary evidence,
        # never product real-Git qualification or a public-registry observation.
        manifest, packages = qualify_platform.package_set(self.fixture_root / 'npm')
        packages = deepcopy(packages)
        name = npm_platform_package.PACKAGES[self.target]
        item, metadata, tarball = packages[name]
        payload = f"#!/bin/sh\nprintf 'twigsmith {self.record['version']}\\n'\n".encode()
        payload_hash = native_package.digest(payload)
        self.item['payload_sha256'] = payload_hash
        metadata['twigsmithNative']['sha256'] = payload_hash
        contents = public.package_contents(tarball)
        contents['package/twigsmith'] = payload
        contents['package/package.json'] = json.dumps(metadata).encode()
        replaced = self.root / tarball.name
        with tarfile.open(replaced, 'w:gz') as archive:
            for member, data in contents.items():
                info = tarfile.TarInfo(member)
                info.size = len(data)
                info.mode = 0o755 if member == 'package/twigsmith' else 0o644
                archive.addfile(info, io.BytesIO(data))
        item['sha256'] = qualify_platform.digest(replaced)
        packages[name] = (item, metadata, replaced)
        next(row for row in self.record['npm']['packages'] if row['name'] == name)['sha256'] = item['sha256']
        expected = {selected: public.package_contents(row[2]) for selected, row in packages.items()}
        harness = self.root / 'harness'
        harness.mkdir()
        bridge = harness / 'bridge'
        bridge.write_text('#!/bin/sh\nexec "$TWIGSMITH_NPM_SHIM_DIR/$(basename "$0")" "$@"\n')
        bridge.chmod(0o755)
        prepared = {'executables': {'native_npm_fixture': {'path': 'bridge'}}}
        evidence = self.root / 'evidence'
        evidence.mkdir()
        work = self.root / 'work'
        work.mkdir()
        def synthetic_children(artifact, harness, evidence, report, **kwargs):
            report.update(passed=True, tests=[{'suite': suite, 'name': case, 'passed': True, 'exit': 0}
                for suite, cases in kwargs['selected_cases'].items() for case in cases])
            return True
        with qualify_platform.local_registry(packages) as (registry, requests), \
             patch.object(public, 'REGISTRY', registry), \
             patch.object(qualify_platform, 'run_fixtures', side_effect=synthetic_children):
            env = public.anonymous_environment(self.root / 'host')
            with public.isolated_environment(env):
                reports = {kind: public.npm_check(kind, self.record, self.item, harness, prepared, evidence,
                             work, env, expected) for kind in ('npm_global', 'npx_twigsmith', 'npx_tgi')}
        self.assertTrue(all(report['passed'] for report in reports.values()))
        self.assertEqual(requests.count('tarballs/' + tarball.name), 3)
        for alias in ('twigsmith', 'tgi'):
            detail = json.loads((evidence / ('npx_' + alias) / 'qualification.json').read_text())
            self.assertIn('npx_' + alias, detail['selected_shim']['shim'])
            self.assertEqual(detail['payload_sha256'], payload_hash)


if __name__ == '__main__':
    unittest.main()
