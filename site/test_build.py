"""Check release rendering and preservation of the previous Pages artifact."""
from copy import deepcopy
from html.parser import HTMLParser
import importlib.util
from pathlib import Path
import re
import tempfile
import unittest
from unittest import mock
from urllib.parse import urlsplit

from test_release_model import public_fixture
import release_model

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
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.fixture = public_fixture(Path(cls.temporary.name) / 'bundle')

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.output = self.root / 'pages'

    def previous(self):
        self.output.mkdir()
        (self.output / 'index.html').write_text('previous verified site')

    def assert_previous(self):
        self.assertEqual((self.output / 'index.html').read_text(), 'previous verified site')
        self.assertEqual(sorted(p.name for p in self.root.iterdir()), ['pages'])

    def test_current_stable_release_renders_four_downloads_and_installer_recovery(self):
        build.build_resolved(self.output, self.resolved_model())
        published = (self.output / 'index.html').read_text()
        self.assertNotIn('not published', published)
        self.assertIn('npm install --global twigsmith@1.0.0', published)
        self.assertIn('npx --yes --package=twigsmith@1.0.0 -- tgi --help', published)
        self.assertIn('twigsmith-install', published)
        for platform in ('Linux x64', 'Windows x64', 'macOS ARM64', 'macOS Intel'):
            self.assertIn(platform, published)
        self.assertEqual(sorted(p.name for p in self.root.iterdir()), ['pages'])

    def test_historical_descriptor_is_refused_before_changing_output(self):
        self.previous()
        descriptor = {'schema': 1, 'version': '0.1.0'}
        with self.assertRaisesRegex(ValueError, 'schema 3 installer release'):
            build.build_resolved(self.output, descriptor)
        self.assert_previous()

    def test_failed_staging_and_failed_promotion_preserve_previous_output(self):
        self.previous()
        with mock.patch.object(build.shutil, 'copytree', side_effect=OSError('copy failed')):
            with self.assertRaisesRegex(OSError, 'copy failed'):
                build.build_resolved(self.output, self.resolved_model())
        self.assert_previous()
        replace = build.os.replace

        def fail_staged(source, destination):
            if Path(source).name == 'artifact':
                raise OSError('promotion failed')
            return replace(source, destination)

        with mock.patch.object(build.os, 'replace', side_effect=fail_staged):
            with self.assertRaisesRegex(OSError, 'promotion failed'):
                build.build_resolved(self.output, self.resolved_model())
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
                build.build_resolved(self.output, self.resolved_model())
        recovered = list(self.root.glob('.pages-*/previous/index.html'))
        self.assertEqual(len(recovered), 1)
        self.assertEqual(recovered[0].read_text(), 'previous verified site')

    def test_template_failure_does_not_create_output(self):
        with mock.patch.object(build.Path, 'read_text', return_value='missing template markers'):
            with self.assertRaisesRegex(ValueError, 'placeholder'):
                build.build_resolved(self.output, self.resolved_model())
        self.assertFalse(self.output.exists())
        self.assertEqual(list(self.root.iterdir()), [])

    def test_links_resolve_with_repository_base_path_and_manual_anchors(self):
        build.build_resolved(self.output, self.resolved_model())
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
                build.build_resolved(self.output, self.resolved_model())
        self.assertEqual((self.output / 'index.html').read_text(), 'previous verified site')

    def resolved_model(self):
        return release_model.resolve(*deepcopy(self.fixture))

    def test_resolved_release_fields_survive_moved_component_and_style_changes(self):
        model = self.resolved_model()
        template = (build.ROOT / 'site/index.html').read_text()
        template = template.replace('    <!-- RELEASE-IDENTITY -->\n', '')
        template = template.replace('    <!-- REQUIREMENTS -->',
                                    '    <!-- REQUIREMENTS -->\n    <!-- RELEASE-IDENTITY -->')
        template = template.replace('class="section-body"', 'class="changed-layout"')
        original = build.Path.read_text

        def read(path, *args, **kwargs):
            if path == build.ROOT / 'site/index.html':
                return template
            return original(path, *args, **kwargs)

        with mock.patch.object(build.Path, 'read_text', read):
            build.build_resolved(self.output, model)
        result = (self.output / 'index.html').read_text()
        self.assertLess(result.index('<h3>Requirements'), result.index('Release <span'))
        self.assertIn('class="changed-layout"', result)
        self.assertIn(model['native'][0]['assets']['archive']['url'], result)
        self.assertIn(model['native'][0]['requirements']['system'], result)
        self.assertIn('npm install --global twigsmith@1.0.0', result)

    def test_rendered_release_mismatch_is_rejected(self):
        model = self.resolved_model()
        source = build._render_resolved(model)
        page = build._ReleaseFields()
        page.feed(source)
        for changed in [source.replace('twigsmith@1.0.0', 'twigsmith@9.0.0', 1),
                        source.replace(model['release_url'], model['release_url'] + '-bad', 1),
                        source.replace('data-release-field="npm-requirement-0"', '', 1),
                        source.replace('glibc 2.35+', 'glibc 2.40+', 1),
                        source.replace('</main>', '<p>Current release: 9.0.0</p></main>'),
                        source.replace('</main>', '<p>Latest version: 9.0.0</p></main>')]:
            with self.subTest(changed=changed[:30]), self.assertRaises(ValueError):
                build._validate_rendered(changed, page.fields, '1.0.0')

if __name__ == '__main__':
    unittest.main()
