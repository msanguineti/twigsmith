"""Exercise the production HTTPS agent through an entirely local CONNECT proxy.

The package keeps exact github.com release URLs and production metadata. Only the
standard proxy and additional-CA settings route these requests to local fixtures;
there is no installer origin override and no external network operation.
"""
import base64
import json
import os
from pathlib import Path
import select
import shutil
import socket
import ssl
import subprocess
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import npm_single_package as package
from test_npm_single_package import candidate

REPO = Path(__file__).resolve().parents[1]
TARGET = 'x86_64-unknown-linux-gnu'
VERSION = '0.1.0-beta.1'
PROXY_USER = 'fixture-user'
PROXY_PASSWORD = 'fixture-password-only-proxy'
PROXY_AUTH = 'Basic ' + base64.b64encode(f'{PROXY_USER}:{PROXY_PASSWORD}'.encode()).decode()


@unittest.skipUnless(shutil.which('openssl') and shutil.which('node'),
                     'TLS qualification gap: openssl and Node are required')
class InstallerTLSTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.asset_requests = []
        self.proxy_requests = []
        self.redirect = False
        self._make_certificates()
        self.metadata, self.archives = self._native_metadata()
        self.entry = self.metadata['targets'][TARGET]
        owner = self

        class AssetHandler(BaseHTTPRequestHandler):
            def do_GET(self):
                owner.asset_requests.append((self.path, {key.lower(): value for key, value in self.headers.items()}))
                if owner.redirect:
                    self.send_response(302)
                    self.send_header('Location', 'https://unapproved.invalid/payload')
                    self.end_headers()
                    return
                data = owner.archives.get(self.path)
                self.send_response(200 if data is not None else 404)
                self.send_header('Content-Length', str(len(data or b'')))
                self.end_headers()
                self.wfile.write(data or b'')

            def log_message(self, *_):
                pass

        self.tls_server = ThreadingHTTPServer(('127.0.0.1', 0), AssetHandler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(self.root / 'server.pem', self.root / 'server.key')
        self.tls_server.socket = context.wrap_socket(self.tls_server.socket, server_side=True)
        self._start_server(self.tls_server)

        class ProxyHandler(BaseHTTPRequestHandler):
            def do_CONNECT(self):
                owner.proxy_requests.append((self.path, {key.lower(): value for key, value in self.headers.items()}))
                # This fixture can reach only its local TLS server. It cannot
                # relay an accidental public request, including a redirect.
                if self.path != 'github.com:443' or self.headers.get('Proxy-Authorization') != PROXY_AUTH:
                    self.send_error(403)
                    return
                upstream = socket.create_connection(('127.0.0.1', owner.tls_server.server_port), timeout=5)
                self.send_response(200, 'Connection established')
                self.end_headers()
                upstream.settimeout(None)
                sockets = (self.connection, upstream)
                deadline = time.monotonic() + 15
                try:
                    while time.monotonic() < deadline:
                        readable, _, _ = select.select(sockets, [], [], 0.25)
                        for source in readable:
                            data = source.recv(65536)
                            if not data:
                                return
                            destination = upstream if source is self.connection else self.connection
                            destination.sendall(data)
                except (OSError, ConnectionError):
                    pass
                finally:
                    upstream.close()
                    self.close_connection = True

            def log_message(self, *_):
                pass

        self.proxy = ThreadingHTTPServer(('127.0.0.1', 0), ProxyHandler)
        self._start_server(self.proxy)

    def _start_server(self, server):
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        threading.Thread(target=server.serve_forever, daemon=True).start()

    def _openssl(self, *args):
        subprocess.run(['openssl', *args], cwd=self.root, check=True,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20)

    def _make_certificates(self):
        self._openssl('req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
                      '-subj', '/CN=Twigsmith local test CA', '-keyout', 'ca.key', '-out', 'ca.pem',
                      '-addext', 'basicConstraints=critical,CA:TRUE',
                      '-addext', 'keyUsage=critical,keyCertSign,cRLSign')
        self._openssl('req', '-new', '-newkey', 'rsa:2048', '-nodes',
                      '-subj', '/CN=github.com', '-keyout', 'server.key', '-out', 'server.csr')
        (self.root / 'extensions.cnf').write_text(
            'subjectAltName=DNS:github.com\n'
            'basicConstraints=critical,CA:FALSE\n'
            'keyUsage=critical,digitalSignature,keyEncipherment\n'
            'extendedKeyUsage=serverAuth\n')
        self._openssl('x509', '-req', '-in', 'server.csr', '-CA', 'ca.pem', '-CAkey', 'ca.key',
                      '-CAcreateserial', '-days', '1', '-out', 'server.pem', '-extfile', 'extensions.cnf')

    def _native_metadata(self):
        records, archives = [], {}
        for target in package.TARGETS:
            native = candidate(self.root / 'native', target)
            archive = Path(native['archive'])
            stem = package.native_package.archive_stem(archive)
            manifest = json.loads(archive.with_name(stem + '.manifest.json').read_text())
            members = {f'{stem}/{name}': {**value, 'mode': int(value['mode'], 8)}
                       for name, value in manifest['files'].items()}
            records.append({'target': target, 'archive': archive.name,
                            'archive_sha256': native['archive_sha256'],
                            'payload': manifest['npm_payload'], 'payload_sha256': native['payload_sha256'],
                            'members': members})
            archives[f'/msanguineti/twigsmith/releases/download/v{VERSION}/{archive.name}'] = archive.read_bytes()
        return package.metadata_for_native(VERSION, records), archives

    def _install(self, name, *, trust_ca):
        directory = self.root / name
        directory.mkdir()
        for source, destination in [('tools/npm_installer.cjs', 'installer.cjs'),
                                    ('tools/npm_single_launcher.cjs', 'launcher.cjs')]:
            (directory / destination).write_bytes((REPO / source).read_bytes())
        (directory / 'package.json').write_text(json.dumps(package.top_metadata(VERSION)))
        (directory / 'native.json').write_text(json.dumps(self.metadata))
        env = {key: value for key, value in os.environ.items()
               if key.upper() in ('PATH', 'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'PATHEXT', 'TEMP', 'TMP')}
        env['HTTPS_PROXY'] = (f'http://{PROXY_USER}:{PROXY_PASSWORD}'
                              f'@127.0.0.1:{self.proxy.server_port}')
        if trust_ca:
            env['NODE_EXTRA_CA_CERTS'] = str(self.root / 'ca.pem')
        result = subprocess.run(['node', str(directory / 'installer.cjs'), '--target', TARGET],
                                env=env, text=True, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=25)
        for secret in (PROXY_USER, PROXY_PASSWORD, PROXY_AUTH):
            self.assertNotIn(secret, result.stdout + result.stderr)
        return directory, result

    def test_production_https_proxy_ca_validation_and_credential_boundary(self):
        self.assertNotIn('fixture', self.metadata)
        self.assertEqual(set(self.metadata['targets']), set(package.TARGETS))
        good, result = self._install('trusted', trust_ca=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        state = json.loads((good / '.native/installed.json').read_text())
        payload = good / '.native' / state['directory'] / self.entry['payload']
        self.assertEqual(package.sha256(payload), self.entry['payload_sha256'])
        self.assertEqual(state['target'], TARGET)
        self.assertEqual(len(self.asset_requests), 1)
        self.assertEqual(len(self.proxy_requests), 1)
        self.assertEqual(self.proxy_requests[0][0], 'github.com:443')
        self.assertEqual(self.proxy_requests[0][1]['proxy-authorization'], PROXY_AUTH)

        bad, result = self._install('untrusted', trust_ca=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Node CA trust', result.stderr)
        self.assertFalse((bad / '.native/installed.json').exists())
        self.assertEqual(len(self.asset_requests), 1, 'untrusted TLS must fail before the asset request')
        self.assertEqual(len(self.proxy_requests), 2)

        self.redirect = True
        redirected, result = self._install('redirected', trust_ca=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('unapproved origin', result.stderr)
        self.assertFalse((redirected / '.native/installed.json').exists())
        self.assertEqual(len(self.proxy_requests), 3, 'a forbidden redirect must never reach the proxy')
        self.assertEqual(len(self.asset_requests), 2)
        for requested, headers in self.asset_requests:
            self.assertEqual(requested, '/msanguineti/twigsmith/releases/download/v' + VERSION + '/' + self.entry['archive'])
            self.assertEqual(headers['host'], 'github.com')
            self.assertNotIn('proxy-authorization', headers)
            self.assertNotIn('authorization', headers)
            encoded = json.dumps(headers)
            self.assertNotIn(PROXY_USER, encoded)
            self.assertNotIn(PROXY_PASSWORD, encoded)
            self.assertNotIn(PROXY_AUTH, encoded)
        self.assertTrue(all(requested == 'github.com:443' for requested, _ in self.proxy_requests))


if __name__ == '__main__':
    unittest.main()
