"""Real npm consumer fixtures; synthetic archives do not qualify native hosts."""
import copy
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import platform
import zipfile
import stat
import threading
import time
import warnings
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch

import npm_single_package as package
import native_package
from test_native_package import pe_fixture, macho_fixture

REPO = Path(__file__).resolve().parents[1]
TARGET = 'x86_64-unknown-linux-gnu'
VERSION = '0.1.0-fixture.1'


def candidate(root, target, payload=None, version='0.1.0-beta.1', revision='1' * 40, license_bytes=None):
    platform = native_package.target_metadata(target)
    if payload is None:
        if platform['os'] == 'windows':
            payload = pe_fixture()
        elif platform['os'] == 'macos':
            payload = macho_fixture(platform['arch'])
        else:
            data = bytearray(64)
            data[:6] = b'\x7fELF\x02\x01'
            data[16:20] = b'\x03\x00\x3e\x00'
            payload = bytes(data)
    provenance = {'kind': 'development-probe',
                  'source': {'revision': revision, 'tree': '2' * 40, 'dirty': False},
                  'build': {'target': target, 'profile': 'release', 'locked': True, 'compiler': 'fixture'},
                  'toolchain': {'pin': 'fixture', 'rustc': 'fixture\nhost: ' + target, 'cargo': 'fixture'},
                  'environment': {'system': 'fixture, not product qualification', 'machine': platform['arch']}}
    directory = root / target
    directory.mkdir(parents=True)
    archive = native_package.write_bundle(directory, version,
        {name: payload for name in platform['executables']}
        | {'LICENSE': (REPO / 'LICENSE').read_bytes() if license_bytes is None else license_bytes},
        provenance, 1, target=target)
    return {'archive': str(archive), 'archive_sha256': package.sha256(archive),
            'payload_sha256': native_package.digest(payload)}


def contents(tarball):
    with tarfile.open(tarball) as tar:
        return {entry.name: tar.extractfile(entry).read() for entry in tar.getmembers()}


class BuilderTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def test_pack_report_client_shapes_and_rejections(self):
        entry = {'filename': 'twigsmith-0.1.0-beta.1.tgz'}
        for report in ([entry], {'twigsmith': entry}):
            self.assertEqual(package.package_filename(report, 'twigsmith', '0.1.0-beta.1'), entry['filename'])
        for report in ({'other': entry}, [entry, entry], {}, [], 'invalid',
                       {'twigsmith': {'filename': '../outside.tgz'}}, {'twigsmith': None}):
            with self.subTest(report=report), self.assertRaises(ValueError):
                package.package_filename(report, 'twigsmith', '0.1.0-beta.1')

    def test_every_current_packaging_input_is_hashed_and_rechecked(self):
        expected = {'tools/npm_single_package.py', 'tools/npm_common.py',
                    'tools/npm_single_launcher.cjs', 'tools/npm_installer.cjs',
                    'tools/native_package.py', 'tools/native_formats.py', 'README.md', 'LICENSE'}
        repo = self.root / 'source'
        for name in expected:
            destination = repo / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(REPO / name, destination)
        item = candidate(self.root / 'native', TARGET)
        original_pack = package.pack
        source = {'revision': '3' * 40, 'tree': '4' * 40, 'dirty': False}
        with patch.object(native_package, 'identity', return_value=source):
            complete = self.root / 'probe'
            manifest = json.loads(package.make_package_set(repo, [item], complete, partial=True).read_text())
            self.assertEqual(set(manifest['packaging']['inputs_sha256']), expected)
            self.assertEqual(manifest['packaging']['inputs_sha256'],
                             {name: package.sha256(repo / name) for name in expected})
            for index, name in enumerate(sorted(expected)):
                path = repo / name
                original = path.read_bytes()
                def mutate(*args, **kwargs):
                    result = original_pack(*args, **kwargs)
                    path.write_bytes(original + b'changed')
                    return result
                output = self.root / f'changed-{index}'
                with self.subTest(input=name), patch.object(package, 'pack', side_effect=mutate):
                    with self.assertRaisesRegex(ValueError, 'packaging inputs changed'):
                        package.make_package_set(repo, [item], output, partial=True)
                self.assertFalse(output.exists())
                path.write_bytes(original)

    def test_native_license_normalization_and_distinct_license_refusal(self):
        canonical = (REPO / 'LICENSE').read_bytes()
        lf = canonical.replace(b'\r\n', b'\n')
        item = candidate(self.root / 'native', 'x86_64-pc-windows-msvc',
                         license_bytes=lf.replace(b'\n', b'\r\n'))
        output = self.root / 'probe'
        manifest = json.loads(package.make_package_set(REPO, [item], output, partial=True).read_text())
        self.assertEqual(contents(output / manifest['packages'][0]['filename'])['package/LICENSE'], canonical)
        other = candidate(self.root / 'other', TARGET, license_bytes=canonical + b'additional condition')
        with self.assertRaisesRegex(ValueError, 'licenses disagree'):
            package.make_package_set(REPO, [other], self.root / 'bad', partial=True)
        self.assertFalse((self.root / 'bad').exists())

    def test_bad_native_hash_and_failed_pack_are_atomic(self):
        item = candidate(self.root / 'native', TARGET)
        bad = dict(item, archive_sha256='0' * 64)
        with self.assertRaisesRegex(ValueError, 'archive does not match'):
            package.make_package_set(REPO, [bad], self.root / 'bad', partial=True)
        original = package.subprocess.run
        def fail_pack(args, **kwargs):
            if 'pack' in args:
                raise subprocess.CalledProcessError(1, args)
            return original(args, **kwargs)
        with patch.object(package.subprocess, 'run', side_effect=fail_pack):
            with self.assertRaises(subprocess.CalledProcessError):
                package.make_package_set(REPO, [item], self.root / 'failed', partial=True)
        self.assertFalse((self.root / 'bad').exists())
        self.assertFalse((self.root / 'failed').exists())


class SinglePackageTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.requests = []
        self.responses = {}
        self.slow_paths = set()
        owner = self
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                owner.requests.append(self.path)
                status, data, headers = owner.responses.get(self.path, (404, b'', {}))
                self.send_response(status)
                for key, value in headers.items():
                    self.send_header(key, value)
                self.end_headers()
                if self.path in owner.slow_paths:
                    try:
                        for _ in range(400):
                            self.wfile.write(b'x'); self.wfile.flush(); time.sleep(0.2)
                    except (BrokenPipeError, ConnectionResetError):
                        pass
                else:
                    self.wfile.write(data)
                self.close_connection = True
            def log_message(self, *_):
                pass
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.origin = f'http://127.0.0.1:{server.server_port}'
        self.env = dict(os.environ, npm_config_cache=str(self.root / 'cache'),
                        npm_config_update_notifier='false', npm_config_audit='false',
                        npm_config_fund='false')
        self.env['npm_config_userconfig'] = str(self.root / 'empty-npmrc')
        self.env['npm_config_globalconfig'] = str(self.root / 'empty-global-npmrc')
        self.npm = package.npm_command()
        self.npm_major = int(subprocess.check_output(
            [*self.npm, '--version'], env=self.env, text=True).split('.')[0])

    def run_command(self, args, **kwargs):
        return subprocess.run(args, env=self.env, text=True, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, timeout=90, **kwargs)

    def fixture(self, payload=b'#!/bin/sh\nprintf "fixture:%s\\n" "$@"\n', targets=(TARGET,), mutate=None):
        """Explicit test-only packer: never callable from the production builder.

        Loopback routing is literal package metadata, not an environment bypass.
        Archive/member digests are computed independently in this fixture.
        """
        records = []
        for target in targets:
            stem = f'twigsmith-{VERSION}-{target}'
            names = ['twigsmith', 'tgi', 'LICENSE', 'README.txt']
            members = {}
            raw = io.BytesIO()
            with tarfile.open(fileobj=raw, mode='w:gz', format=tarfile.USTAR_FORMAT) as tar:
                for name in names:
                    data = payload if name in ('twigsmith', 'tgi') else b'fixture\n'
                    member = tarfile.TarInfo(f'{stem}/{name}')
                    member.size, member.mode = len(data), 0o755 if name in ('twigsmith', 'tgi') else 0o644
                    tar.addfile(member, io.BytesIO(data))
                    members[member.name] = {'size': len(data), 'mode': member.mode,
                                            'sha256': package.native_package.digest(data)}
            archive = raw.getvalue()
            name = stem + '.tar.gz'
            self.responses['/' + name] = (200, archive, {'Content-Length': str(len(archive))})
            records.append({'target': target, 'archive': name, 'archive_sha256': package.native_package.digest(archive),
                            'payload': 'twigsmith', 'payload_sha256': package.native_package.digest(payload),
                            'members': members})
        metadata = package.metadata_for_native(VERSION, records)
        metadata.update(fixture='loopback-test-only', fixture_origin=self.origin)
        for entry in metadata['targets'].values():
            entry['url'] = self.origin + '/' + entry['archive']
        if mutate:
            mutate(metadata)
        staged = self.root / f'packed-{len(list(self.root.glob("packed-*")))}'
        staged.mkdir()
        files = {'native.json': (json.dumps(metadata) + '\n').encode(),
                 'launcher.cjs': (REPO / 'tools/npm_single_launcher.cjs').read_bytes(),
                 'installer.cjs': (REPO / 'tools/npm_installer.cjs').read_bytes()}
        record = package.pack(staged / 'package', staged, package.top_metadata(VERSION), files, self.env)
        return staged / record['filename'], metadata

    def install(self, tarball, *flags):
        consumer = self.root / f'consumer-{len(list(self.root.glob("consumer-*")))}'
        consumer.mkdir()
        (consumer / 'package.json').write_text(json.dumps({'name': 'consumer', 'version': '1.0.0',
            'allowScripts': {str(tarball): True}}))
        result = self.run_command([*self.npm, 'install', '--offline', '--foreground-scripts', *flags, str(tarball)], cwd=consumer)
        return consumer, result

    @unittest.skipUnless(platform.system() == 'Linux' and platform.machine() == 'x86_64', 'representative Linux executable fixture')
    def test_first_real_npm_install_auto_explicit_execute_and_failure(self):
        tarball, metadata = self.fixture()
        consumer, result = self.install(tarball)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        package_dir = consumer / 'node_modules/twigsmith'
        state = json.loads((package_dir / '.native/installed.json').read_text())
        self.assertEqual(state['target'], TARGET)
        self.assertEqual(len(self.requests), 1)
        for binary in ('twigsmith', 'tgi'):
            run = self.run_command([str(consumer / 'node_modules/.bin' / binary), 'literal with spaces', '--os=invalid'])
            self.assertEqual((run.returncode, run.stdout), (0, 'fixture:literal with spaces\nfixture:--os=invalid\n'), run.stderr)
        consumer2, result = self.install(tarball, '--os=linux', '--cpu=x64', '--libc=glibc')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        run = self.run_command(['node', str(package_dir / 'installer.cjs'), '--target', TARGET])
        self.assertEqual(run.returncode, 0, run.stderr)
        bad = self.run_command(['node', str(package_dir / 'installer.cjs'), '--target', 'wrong'])
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn('unsupported target', bad.stderr)
        entry = metadata['targets'][TARGET]
        self.responses['/' + entry['archive']] = (200, b'wrong bytes', {})
        bad = self.run_command(['node', str(package_dir / 'installer.cjs')])
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn('SHA-256 differs', bad.stderr)
        self.responses['/' + entry['archive']] = (200, b'truncated', {'Content-Length': '1000'})
        bad = self.run_command(['node', str(package_dir / 'installer.cjs')])
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn('interrupted', bad.stderr)
        self.assertEqual(state, json.loads((package_dir / '.native/installed.json').read_text()))

    def node(self, expression, value=None):
        module = json.dumps(str(REPO / 'tools/npm_installer.cjs'))
        return self.run_command(['node', '-e', f"const i=require({module}); const v=JSON.parse(process.argv[1]); {expression}", json.dumps(value)])

    def test_target_precedence_and_libc_floor(self):
        cases = [({'os':'linux','cpu':'x64','glibc':'2.35'}, {}, TARGET),
                 ({'os':'linux','cpu':'x64','glibc':'2.34'}, {}, None),
                 ({'os':'linux','cpu':'x64','glibc':None}, {}, None),
                 ({'os':'darwin','cpu':'arm64'}, {}, 'aarch64-apple-darwin'),
                 ({'os':'linux','cpu':'arm64','glibc':'2.39'}, {}, None),
                 ({'os':'linux','cpu':'x64','glibc':'2.39'}, {'libc':'musl'}, None),
                 ({'os':'linux','cpu':'x64','glibc':'2.39'}, {'target':'aarch64-apple-darwin'}, 'aarch64-apple-darwin'),
                 ({'os':'linux','cpu':'x64','glibc':'2.39'}, {'target':TARGET,'os':'linux'}, None)]
        for host, options, expected in cases:
            run = self.node("console.log(i.detectTarget(v.options,v.host))", {'options':options,'host':host})
            if expected:
                self.assertEqual((run.returncode,run.stdout.strip()),(0,expected),run.stderr)
            else:
                self.assertNotEqual(run.returncode,0)
        run = self.node("console.log(JSON.stringify(i.parseOptions(v.args,v.env)))", {
            'args':['--os','darwin','--cpu=arm64'],'env':{'npm_config_os':'linux','npm_config_cpu':'x64'}})
        self.assertEqual(json.loads(run.stdout),{'os':'darwin','cpu':'arm64'})
        for args in (['--bad'],['--os'],['--os=linux','--os=darwin']):
            self.assertNotEqual(self.node('i.parseOptions(v,{})',args).returncode,0)

    def test_builder_verified_four_archives_one_package_no_binaries(self):
        inputs = [candidate(self.root / 'native', target) for target in package.TARGETS]
        output = self.root / 'complete'
        record = json.loads(package.make_package_set(REPO,inputs,output).read_text())
        self.assertEqual((record['schema'],record['topology'],len(record['packages'])),(3,'single-package-installer',1))
        item = record['packages'][0]
        files = contents(output / item['filename'])
        self.assertEqual(set(files),{'package/package.json','package/launcher.cjs','package/installer.cjs',
                                    'package/native.json','package/LICENSE','package/README.md'})
        metadata = json.loads(files['package/package.json'])
        self.assertNotIn('dependencies',metadata)
        self.assertNotIn('optionalDependencies',metadata)
        self.assertEqual(metadata['scripts'],{'postinstall':'node installer.cjs'})
        native = json.loads(files['package/native.json'])
        self.assertEqual(set(native['targets']),set(package.TARGETS))
        self.assertNotIn('fixture',native)
        for item in inputs:
            archive = Path(item['archive'])
            target = next(t for t in package.TARGETS if t in archive.name)
            self.assertEqual(native['targets'][target]['archive_sha256'],item['archive_sha256'])
        bad = copy.deepcopy(inputs)
        bad[0]['payload_sha256'] = '0' * 64
        with self.assertRaisesRegex(ValueError,'payload does not match'):
            package.make_package_set(REPO,bad,self.root/'bad')
        self.assertFalse((self.root/'bad').exists())
        with self.assertRaisesRegex(ValueError,'same version and source'):
            changed = candidate(self.root/'changed',package.TARGETS[0],revision='9'*40)
            package.make_package_set(REPO,[changed,*inputs[1:]],self.root/'mixed')
        with self.assertRaisesRegex(ValueError,'all four'):
            package.make_package_set(REPO,inputs[:1],self.root/'missing')
        with self.assertRaisesRegex(ValueError,'duplicate'):
            package.make_package_set(REPO,[*inputs,inputs[0]],self.root/'duplicate')

    @unittest.skipUnless(platform.system() == 'Linux' and platform.machine() == 'x86_64', 'representative Linux executable fixture')
    def test_scripts_blocked_default_policy_and_explicit_repair_no_launch_network(self):
        tarball, metadata = self.fixture()
        consumer,result = self.install(tarball,'--ignore-scripts')
        self.assertEqual(result.returncode,0,result.stderr)
        binary = consumer/'node_modules/.bin/twigsmith'
        run = self.run_command([str(binary),'--version'])
        self.assertNotEqual(run.returncode,0)
        self.assertIn('twigsmith-install',run.stderr)
        self.assertEqual(self.requests,[])
        run = self.run_command([*self.npm,'exec','--offline','--ignore-scripts','--','twigsmith-install','--target',TARGET],cwd=consumer)
        self.assertEqual(run.returncode,0,run.stderr)
        self.assertEqual(len(self.requests),1)
        # npm 12 default is a denied lifecycle, even though install exits zero.
        consumer2=self.root/'default';consumer2.mkdir()
        (consumer2/'package.json').write_text('{"name":"default-consumer","version":"1.0.0"}')
        result=self.run_command([*self.npm,'install','--offline',str(tarball)],cwd=consumer2)
        self.assertEqual(result.returncode,0,result.stderr)
        if self.npm_major >= 12:
            self.assertFalse((consumer2/'node_modules/twigsmith/.native').exists())
            self.assertEqual(len(self.requests),1)
        else:
            self.assertTrue((consumer2/'node_modules/twigsmith/.native/installed.json').is_file())
            self.assertEqual(len(self.requests),2)

    @unittest.skipUnless(platform.system() == 'Linux' and platform.machine() == 'x86_64', 'representative Linux executable fixture')
    def test_download_origin_http_errors_tamper_redirect_and_retry(self):
        tarball,metadata=self.fixture()
        consumer,result=self.install(tarball,'--ignore-scripts')
        package_dir=consumer/'node_modules/twigsmith'
        entry=metadata['targets'][TARGET]
        route='/'+entry['archive']
        good=self.responses[route]
        for status,data,headers,expected in [
            (403,b'',{},'HTTP 403'),
            (200,b'bad',{},'SHA-256 differs'),
            (200,b'',{'Content-Length':str(129*1024*1024)},'size limit'),
            (302,b'',{'Location':'http://example.com/payload'},'loopback origin'),
            (302,b'',{'Location':self.origin+'/redirect'},'redirect/time limit')]:
            self.responses[route]=(status,data,headers)
            self.responses['/redirect']=(302,b'',{'Location':self.origin+'/redirect'})
            run=self.run_command(['node',str(package_dir/'installer.cjs')])
            self.assertNotEqual(run.returncode,0)
            self.assertIn(expected,run.stderr)
            self.assertFalse((package_dir/'.native/installed.json').exists())
            self.assertEqual(list((package_dir/'.native').iterdir()),[])
        self.responses[route]=good
        run=self.run_command(['node',str(package_dir/'installer.cjs')])
        self.assertEqual(run.returncode,0,run.stderr)
        state=json.loads((package_dir/'.native/installed.json').read_text())
        exe=package_dir/'.native'/state['directory']/'twigsmith'
        exe.write_bytes(b'tampered')
        run=self.run_command([str(consumer/'node_modules/.bin/twigsmith')])
        self.assertNotEqual(run.returncode,0)
        self.assertIn('SHA-256 differs',run.stderr)
        repair=self.run_command(['node',str(package_dir/'installer.cjs')])
        self.assertEqual(repair.returncode,0,repair.stderr)
        self.assertEqual(package.sha256(exe),entry['payload_sha256'])
        self.assertEqual(len(list((package_dir/'.native').iterdir())),2)

    def unpack_archive(self, data, entry):
        archive=self.root/'unpack-input'
        archive.write_bytes(data)
        return self.node("i.unpack(require('node:fs').readFileSync(v.archive),v.entry)",{'archive':str(archive),'entry':entry})

    def test_untrusted_tar_and_zip_boundaries(self):
        for target in package.TARGETS:
            item=candidate(self.root/'archives',target)
            archive=Path(item['archive'])
            stem=package.native_package.archive_stem(archive)
            manifest=json.loads(archive.with_name(stem+'.manifest.json').read_text())
            entry={'archive':archive.name,'archive_sha256':item['archive_sha256'],
                   'payload':manifest['npm_payload'],'payload_sha256':item['payload_sha256'],
                   'members':{f'{stem}/{name}':{**value,'mode':int(value['mode'],8)} for name,value in manifest['files'].items()}}
            data=archive.read_bytes()
            run=self.unpack_archive(data,entry)
            self.assertEqual(run.returncode,0,run.stderr)
            wrong=copy.deepcopy(entry);wrong['payload_sha256']='0'*64
            self.assertNotEqual(self.unpack_archive(data,wrong).returncode,0)
            if archive.name.endswith('.zip'):
                with zipfile.ZipFile(io.BytesIO(data)) as zipped:
                    originals=[(z.filename,zipped.read(z)) for z in zipped.infolist()]
                for change in ('traversal','link','duplicate','unexpected','directory'):
                    raw=io.BytesIO()
                    with zipfile.ZipFile(raw,'w',compression=zipfile.ZIP_DEFLATED) as zipped:
                        for index,(name,content) in enumerate(originals):
                            if index==0 and change in ('traversal','unexpected'):name='../twigsmith.exe' if change=='traversal' else stem+'/extra'
                            z=zipfile.ZipInfo(name);z.create_system=3
                            z.external_attr=((stat.S_IFLNK if change=='link' and index==0 else stat.S_IFREG)|0o755)<<16
                            if change=='directory' and index==0:z.external_attr=(stat.S_IFDIR|0o755)<<16
                            zipped.writestr(z,content,compress_type=zipfile.ZIP_DEFLATED)
                            if change=='duplicate' and index==0:
                                with warnings.catch_warnings():
                                    warnings.simplefilter('ignore',UserWarning);zipped.writestr(z,content)
                    corrupted=raw.getvalue();bad=copy.deepcopy(entry);bad['archive_sha256']=package.native_package.digest(corrupted)
                    self.assertNotEqual(self.unpack_archive(corrupted,bad).returncode,0,change)
            else:
                with tarfile.open(fileobj=io.BytesIO(data)) as tar:
                    originals=[(member,tar.extractfile(member).read()) for member in tar]
                for change in ('traversal','link','hardlink','duplicate','unexpected','directory','mode'):
                    raw=io.BytesIO()
                    with tarfile.open(fileobj=raw,mode='w:gz',format=tarfile.USTAR_FORMAT) as tar:
                        for index,(member,content) in enumerate(originals):
                            member=copy.copy(member)
                            if index==0:
                                if change in ('traversal','unexpected'):member.name='../twigsmith' if change=='traversal' else stem+'/extra'
                                if change in ('link','hardlink','directory'):
                                    member.type={'link':tarfile.SYMTYPE,'hardlink':tarfile.LNKTYPE,'directory':tarfile.DIRTYPE}[change]
                                    member.linkname='../outside';member.size=0
                                if change=='mode':member.mode=0o777
                            tar.addfile(member,io.BytesIO(content))
                            if change=='duplicate' and index==0:tar.addfile(member,io.BytesIO(content))
                    corrupted=raw.getvalue();bad=copy.deepcopy(entry);bad['archive_sha256']=package.native_package.digest(corrupted)
                    self.assertNotEqual(self.unpack_archive(corrupted,bad).returncode,0,change)

    @unittest.skipUnless(platform.system() == 'Linux' and platform.machine() == 'x86_64', 'representative Linux executable fixture')
    def test_parallel_installs_atomic_state_and_foreign_selection(self):
        tarball,metadata=self.fixture(targets=(TARGET,'aarch64-apple-darwin'))
        consumer,result=self.install(tarball,'--ignore-scripts')
        directory=consumer/'node_modules/twigsmith'
        processes=[subprocess.Popen(['node',str(directory/'installer.cjs')],env=self.env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True) for _ in range(4)]
        for process in processes:
            stdout,stderr=process.communicate(timeout=30)
            self.assertEqual(process.returncode,0,stdout+stderr)
        state=json.loads((directory/'.native/installed.json').read_text())
        self.assertEqual(len(list((directory/'.native').iterdir())),2)
        run=self.run_command(['node',str(directory/'installer.cjs'),'--target','aarch64-apple-darwin'])
        self.assertEqual(run.returncode,0,run.stderr)
        run=self.run_command([str(consumer/'node_modules/.bin/twigsmith')])
        self.assertNotEqual(run.returncode,0)
        self.assertIn('incompatible',run.stderr)
        self.assertEqual(json.loads((directory/'.native/installed.json').read_text())['target'],'aarch64-apple-darwin')

    def test_production_origins_exact_identity_and_no_env_bypass(self):
        tarball,metadata=self.fixture()
        production=copy.deepcopy(metadata)
        production.pop('fixture');production.pop('fixture_origin')
        entry=production['targets'][TARGET]
        # Origin/metadata validators require the exact release identity; test
        # their policy without making a public network request.
        entry['url']=f'https://github.com/msanguineti/twigsmith/releases/download/v{VERSION}/'+entry['archive']
        for url in [entry['url'],'https://release-assets.githubusercontent.com/github-production-release-asset/a?signature=opaque',
                    'https://objects.githubusercontent.com/github-production-release-asset/a']:
            run=self.node("i.allowedURL(new URL(v.url),v.metadata,false,v.entry)",{'url':url,'metadata':production,'entry':entry})
            self.assertEqual(run.returncode,0,run.stderr)
        for url in ['http://github.com/asset','https://github.com/other/asset','https://github.com.evil.test/asset',
                    'https://release-assets.githubusercontent.com.evil.test/a','https://user:pass@objects.githubusercontent.com/a',
                    'https://objects.githubusercontent.com:444/a','http://127.0.0.1:1234/a']:
            run=self.node("i.allowedURL(new URL(v.url),v.metadata,false,v.entry)",{'url':url,'metadata':production,'entry':entry})
            self.assertNotEqual(run.returncode,0,url)
        for key,value in [('url',self.origin+'/'+entry['archive']),('archive','twigsmith-other.tar.gz')]:
            bad=copy.deepcopy(metadata);bad['targets'][TARGET][key]=value
            if key=='url':bad['fixture_origin']='http://127.0.0.1:1'
            self.assertNotEqual(self.node('i.validateMetadata(v,v.version)',bad).returncode,0)

    @unittest.skipUnless(platform.system() == 'Linux' and platform.machine() == 'x86_64', 'representative Linux executable fixture')
    def test_native_state_symlinks_permissions_and_explicit_policy_denial(self):
        tarball,metadata=self.fixture()
        consumer,result=self.install(tarball)
        self.assertEqual(result.returncode,0,result.stderr)
        directory=consumer/'node_modules/twigsmith';root=directory/'.native'
        state=root/'installed.json';saved=state.read_bytes()
        elsewhere=self.root/'outside-state';elsewhere.write_bytes(saved)
        state.unlink();state.symlink_to(elsewhere)
        before=len(self.requests)
        run=self.run_command([str(consumer/'node_modules/.bin/twigsmith')])
        self.assertNotEqual(run.returncode,0)
        self.assertEqual(len(self.requests),before)
        state.unlink();state.write_bytes(saved)
        if os.name!='nt' and os.geteuid()!=0:
            root.chmod(0o555)
            try:
                run=self.run_command(['node',str(directory/'installer.cjs')])
                self.assertNotEqual(run.returncode,0)
                self.assertEqual(state.read_bytes(),saved)
            finally:root.chmod(0o755)
        # Exact resolved tarball spec denial suppresses lifecycle in npm12.
        denied=self.root/'denied';denied.mkdir()
        (denied/'package.json').write_text(json.dumps({'name':'denied','version':'1.0.0',
            'allowScripts':{str(tarball):False}}))
        run=self.run_command([*self.npm,'install','--offline',str(tarball)],cwd=denied)
        self.assertEqual(run.returncode,0,run.stderr)
        self.assertFalse((denied/'node_modules/twigsmith/.native').exists())

    def test_download_total_deadline_bounds_an_active_slow_stream(self):
        tarball,metadata=self.fixture()
        entry=metadata['targets'][TARGET];route='/'+entry['archive']
        self.responses[route]=(200,b'',{'Content-Length':'100000'})
        self.slow_paths.add(route)
        started=time.monotonic()
        run=self.node('i.download(v.entry,v.metadata).catch(e=>{console.error(e.message);process.exitCode=1})',
                      {'entry':entry,'metadata':metadata})
        elapsed=time.monotonic()-started
        self.assertNotEqual(run.returncode,0)
        self.assertIn('timed out',run.stderr)
        self.assertLess(elapsed,75)
        self.assertGreater(elapsed,55)

    @unittest.skipUnless(platform.system() == 'Linux' and platform.machine() == 'x86_64', 'representative Linux executable fixture')
    def test_killed_install_stays_unready_and_retry_cleans_owned_staging(self):
        tarball,metadata=self.fixture()
        consumer,result=self.install(tarball,'--ignore-scripts')
        directory=consumer/'node_modules/twigsmith'
        entry=metadata['targets'][TARGET];route='/'+entry['archive']
        original=self.responses[route]
        self.responses[route]=(200,b'',{'Content-Length':'100000'})
        self.slow_paths.add(route)
        process=subprocess.Popen(['node',str(directory/'installer.cjs')],env=self.env,
                                 stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        try:
            deadline=time.monotonic()+5
            while not self.requests and time.monotonic()<deadline:time.sleep(0.02)
            self.assertTrue(self.requests)
            process.kill();process.communicate(timeout=10)
        finally:
            if process.poll() is None:process.kill();process.communicate(timeout=10)
        root=directory/'.native'
        self.assertFalse((root/'installed.json').exists())
        self.assertTrue(any(p.name.startswith('.install-') for p in root.iterdir()))
        self.slow_paths.remove(route);self.responses[route]=original
        run=self.run_command(['node',str(directory/'installer.cjs')])
        self.assertEqual(run.returncode,0,run.stderr)
        self.assertEqual(len(list(root.iterdir())),2)


if __name__ == '__main__':
    unittest.main()
