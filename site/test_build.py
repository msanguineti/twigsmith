"""Check release copy gating and preservation of the previous Pages artifact."""
from copy import deepcopy
from datetime import datetime, timedelta, timezone
from html.parser import HTMLParser
import importlib.util
import json
import sys
from pathlib import Path
import re
import tempfile
import unittest
from unittest import mock
from urllib.parse import urlsplit

SPEC = importlib.util.spec_from_file_location('twigsmith_site_build', Path(__file__).with_name('build.py'))
build = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(build)


class Links(HTMLParser):
    def __init__(self):
        super().__init__()
        self.ids = []
        self.urls = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if 'id' in attrs:
            self.ids.append(attrs['id'])
        for key in ('href', 'src'):
            if key in attrs:
                self.urls.append(attrs[key])


class SiteTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.output = self.root / 'pages'
        self.record = {
            'repository': 'msanguineti/twigsmith', 'version': '0.1.0',
            'tag': 'v0.1.0',
            'publication': {'source': '4' * 40, 'channel': 'latest',
                            'staging_channel': 'release-candidate', 'prerelease': False, 'release_id': 123},
            'native': {'archive': 'twigsmith-0.1.0-x86_64-unknown-linux-gnu.tar.gz',
                       'sha256': '1' * 64, 'manifest_sha256': '2' * 64,
                       'payload_sha256': '3' * 64, 'source': {'revision': '4' * 40}},
            'npm': {'package': 'twigsmith-0.1.0.tgz',
                    'sha256': '5' * 64, 'manifest_sha256': '6' * 64}}
        self.receipt = {'identity': deepcopy(self.record),
                        'native_sha256': '1' * 64, 'npm_sha256': '5' * 64,
                        'observed_at': datetime.now(timezone.utc).isoformat(),
                        'consumption': {kind: {'version': '0.1.0', 'commands': ['twigsmith', 'tgi']}
                                        for kind in ('native', 'npm', 'npx')}}

    def previous(self):
        self.output.mkdir()
        (self.output / 'index.html').write_text('previous verified site')

    def assert_previous(self):
        self.assertEqual((self.output / 'index.html').read_text(), 'previous verified site')
        self.assertEqual(sorted(p.name for p in self.root.iterdir()), ['pages'])

    def test_preview_default_and_published_exact_installation(self):
        build.build(self.output, self.record)
        preview = (self.output / 'index.html').read_text()
        self.assertIn('not published yet', preview)
        self.assertIn('npm install --global TARBALL', preview)
        self.assertNotIn('npm install --global twigsmith@', preview)
        build.build(self.output, self.record, receipt=self.receipt)
        published = (self.output / 'index.html').read_text()
        self.assertNotIn('not published', published)
        self.assertNotIn('planned locations', published)
        self.assertNotIn('Local npm candidate', published)
        self.assertIn('npm install --global twigsmith@0.1.0', published)
        self.assertIn('npx --yes --package=twigsmith@0.1.0 twigsmith --version', published)
        self.assertIn('npx --yes --package=twigsmith@0.1.0 tgi --help', published)
        download = ('https://github.com/msanguineti/twigsmith/releases/download/v0.1.0/'
                    'twigsmith-0.1.0-x86_64-unknown-linux-gnu')
        for suffix in ('.tar.gz', '.manifest.json', '.sha256'):
            self.assertIn(f'href="{download}{suffix}"', published)
        self.assertIn('https://www.npmjs.com/package/twigsmith/v/0.1.0', published)
        for path in self.output.rglob('*'):
            self.assertFalse(path.is_symlink())
        self.assertNotIn('observed_at', published)
        self.assertEqual(sorted(p.name for p in self.root.iterdir()), ['pages'])

    def test_cli_binds_expected_channel_to_complete_verified_release(self):
        receipt_file = self.root / 'receipt.json'
        receipt_file.write_text(json.dumps(self.receipt))
        verified = deepcopy(self.record)
        verified.pop('publication')
        args = ['build.py', 'native.tar.gz', 'npm', str(self.output),
                '--archive-sha256', '1' * 64, '--payload-sha256', '3' * 64,
                '--package-sha256', '5' * 64, '--native-manifest-sha256', '2' * 64,
                '--npm-manifest-sha256', '6' * 64, '--tag', self.record['tag'],
                '--receipt', str(receipt_file), '--channel', 'latest']
        with mock.patch.object(build.release_set, 'verify', return_value=deepcopy(verified)), mock.patch.object(sys, 'argv', args):
            build.main()
        self.assertTrue(build.promotion_matches(self.output, self.record, self.receipt))
        current = (self.output / 'index.html').read_bytes()
        args[-1] = 'different-channel'
        with mock.patch.object(build.release_set, 'verify', return_value=deepcopy(verified)), mock.patch.object(sys, 'argv', args):
            with self.assertRaises(SystemExit) as error, mock.patch.object(sys, 'stderr'):
                build.main()
            self.assertEqual(error.exception.code, 1)
        self.assertEqual((self.output / 'index.html').read_bytes(), current)

    def test_promotion_observation_compares_entire_artifact(self):
        build.build(self.output, self.record, receipt=self.receipt)
        self.assertTrue(build.promotion_matches(self.output, self.record, self.receipt))
        extra = self.output / 'private-notes.txt'
        extra.write_text('unexpected')
        self.assertFalse(build.promotion_matches(self.output, self.record, self.receipt))
        extra.unlink()
        (self.output / 'assets' / 'mark.svg').write_text('changed')
        self.assertFalse(build.promotion_matches(self.output, self.record, self.receipt))

    def test_prerelease_cannot_replace_published_site(self):
        self.previous()
        record = deepcopy(self.record)
        record['version'] = '0.1.1-rc1'
        record['tag'] = 'v' + record['version']
        record['native']['archive'] = f'twigsmith-{record["version"]}-x86_64-unknown-linux-gnu.tar.gz'
        record['npm']['package'] = f'twigsmith-{record["version"]}.tgz'
        record['publication'].update(prerelease=True, channel='next')
        receipt = deepcopy(self.receipt)
        receipt['identity'] = record
        for observation in receipt['consumption'].values():
            observation['version'] = record['version']
        with self.assertRaisesRegex(ValueError, 'publication identity'):
            build.build(self.output, record, receipt=receipt)
        self.assert_previous()

    def test_receipt_identity_hash_and_consumption_fail_closed(self):
        changes = [lambda receipt: receipt['identity']['publication'].update(channel='next'),
                   lambda receipt: receipt['identity'].update(tag='v0.1.0-beta.2'),
                   lambda receipt: receipt['identity']['native'].update(payload_sha256='0' * 64),
                   lambda receipt: receipt.update(native_sha256='0' * 64),
                   lambda receipt: receipt.update(npm_sha256='0' * 64),
                   lambda receipt: receipt['consumption'].pop('npx'),
                   lambda receipt: receipt['consumption']['npm'].update(commands=['twigsmith']),
                   lambda receipt: receipt['consumption']['native'].update(version='9.0.0'),
                   lambda receipt: receipt.update(observed_at=(datetime.now(timezone.utc) - timedelta(hours=1)).isoformat()),
                   lambda receipt: receipt.update(observed_at=(datetime.now(timezone.utc) + timedelta(hours=1)).isoformat())]
        self.previous()
        for change in changes:
            receipt = deepcopy(self.receipt)
            change(receipt)
            with self.subTest(change=change), self.assertRaises(ValueError):
                build.build(self.output, self.record, receipt=receipt)
            self.assert_previous()
        with self.assertRaises(ValueError):
            build.build(self.root / 'new', self.record, receipt=True)
        self.assertFalse((self.root / 'new').exists())

    def test_failed_staging_and_failed_promotion_preserve_previous_output(self):
        self.previous()
        with mock.patch.object(build.shutil, 'copytree', side_effect=OSError('copy failed')):
            with self.assertRaisesRegex(OSError, 'copy failed'):
                build.build(self.output, self.record, receipt=self.receipt)
        self.assert_previous()
        replace = build.os.replace

        def fail_staged(source, destination):
            if Path(source).name == 'artifact':
                raise OSError('promotion failed')
            return replace(source, destination)

        with mock.patch.object(build.os, 'replace', side_effect=fail_staged):
            with self.assertRaisesRegex(OSError, 'promotion failed'):
                build.build(self.output, self.record, receipt=self.receipt)
        self.assert_previous()

    def test_failed_restore_keeps_recovery_copy(self):
        self.previous()
        replace = build.os.replace

        def fail_install_and_restore(source, destination):
            if Path(source).name in ('artifact', 'previous'):
                raise OSError('destination unavailable')
            return replace(source, destination)

        with mock.patch.object(build.os, 'replace', side_effect=fail_install_and_restore):
            with self.assertRaisesRegex(OSError, 'previous artifact retained'):
                build.build(self.output, self.record, receipt=self.receipt)
        recovered = list(self.root.glob('.pages-*/previous/index.html'))
        self.assertEqual(len(recovered), 1)
        self.assertEqual(recovered[0].read_text(), 'previous verified site')

    def test_template_failure_does_not_create_output(self):
        with mock.patch.object(build.Path, 'read_text', return_value='missing template markers'):
            with self.assertRaisesRegex(ValueError, 'placeholder'):
                build.build(self.output, self.record)
        self.assertFalse(self.output.exists())
        self.assertEqual(list(self.root.iterdir()), [])

    def test_links_resolve_with_repository_base_path_and_manual_anchors(self):
        build.build(self.output, self.record, receipt=self.receipt)
        source = (self.output / 'index.html').read_text()
        page = Links()
        page.feed(source)
        self.assertEqual(len(page.ids), len(set(page.ids)))
        self.assertIn('href="#main"', source)
        self.assertIn('<main id="main">', source)
        self.assertIn('aria-label="Main navigation"', source)
        for url in page.urls:
            with self.subTest(url=url):
                parsed = urlsplit(url)
                if not parsed.scheme and not parsed.netloc:
                    self.assertFalse(parsed.path.startswith('/'))
                    if parsed.path:
                        self.assertTrue((self.output / parsed.path).is_file())
                    if parsed.fragment:
                        self.assertIn(parsed.fragment, page.ids)
                elif '/blob/main/docs/' in parsed.path:
                    manual = build.ROOT / parsed.path.split('/blob/main/', 1)[1]
                    self.assertTrue(manual.is_file())
                    if parsed.fragment:
                        headings = [re.sub(r'[^\w -]', '', h.lower()).replace(' ', '-')
                                    for h in re.findall(r'^#+ (.+)$', manual.read_text(), flags=re.M)]
                        self.assertIn(parsed.fragment, headings)

    def test_links_are_rejected_before_promotion(self):
        self.previous()
        assets = self.root / 'source' / 'site' / 'assets'
        assets.mkdir(parents=True)
        (assets / 'leak').symlink_to(build.ROOT / 'README.md')
        source = assets.parent
        (source / 'index.html').write_text((build.ROOT / 'site' / 'index.html').read_text())
        (source / 'style.css').write_text('body{}')
        with mock.patch.object(build, 'ROOT', source.parent):
            with self.assertRaisesRegex(ValueError, 'ordinary files'):
                build.build(self.output, self.record)
        self.assertEqual((self.output / 'index.html').read_text(), 'previous verified site')


if __name__ == '__main__':
    unittest.main()
