#!/usr/bin/env python3
"""Build an unpublished four-platform installer preview from a verified local bundle.

Public stable site preparation separately resolves the same descriptor against
current GitHub and npm observations. This command performs no provider writes.
"""
import argparse
import html
from html.parser import HTMLParser
import os
from pathlib import Path
import re
import shutil
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent


class _ReleaseFields(HTMLParser):
    """Collect actual rendered fields independently of their layout and style."""
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.fields = {}
        self.stack = []
        self.release_links = []
        self.text = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        field = attrs.get('data-release-field')
        if field:
            if field in self.fields:
                raise ValueError('duplicate release field')
            self.fields[field] = {'text': '', 'href': attrs.get('href')}
        if tag not in {'area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'param', 'source', 'track', 'wbr'}:
            self.stack.append(field)
        href = attrs.get('href', '')
        if ('github.com/msanguineti/twigsmith/releases/' in href
                or 'www.npmjs.com/package/twigsmith/v/' in href):
            self.release_links.append(href)

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)
        if tag not in {'area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'param', 'source', 'track', 'wbr'}:
            self.handle_endtag(tag)

    def handle_endtag(self, tag):
        if self.stack:
            self.stack.pop()

    def handle_data(self, text):
        self.text.append(text)
        for field in self.stack:
            if field:
                self.fields[field]['text'] += text


def _render_resolved(model):
    """Render a current public stable model returned by release_model.resolve."""
    if model.get('availability') != 'published' or model.get('schema') != 3:
        raise ValueError('published site requires a resolved schema 3 installer release')
    return _render_candidate(model, published=True)


def _validate_rendered(source, expected, version):
    page = _ReleaseFields()
    page.feed(source)
    page.close()
    if page.fields != expected:
        raise ValueError('rendered release fields differ from resolved model')
    expected_links = [value['href'] for value in expected.values() if value['href']]
    if sorted(page.release_links) != sorted(expected_links):
        raise ValueError('rendered release links differ from resolved model')
    # A redesign can introduce a separate release claim outside generated fields.
    # Scope this to release-labelled versions so dependency versions remain valid.
    visible_text = ' '.join(page.text)
    for found in re.findall(r'\b(?:release(?:\s+version)?|(?:current|latest|twigsmith)\s+version)\s*[:#]?\s*v?([0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?)', visible_text, flags=re.IGNORECASE):
        if found != version:
            raise ValueError('conflicting rendered release version')
    # Reject separately embedded exact-version npm commands that could contradict
    # the generated instructions, including those outside generated components.
    for found in re.findall(r'twigsmith@([0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?)', ''.join(page.text)):
        if found != version:
            raise ValueError('conflicting rendered release version')


def build_resolved(output, model):
    """Build from a model returned by release_model.resolve, without publishing."""
    source = _render_resolved(model)
    _build_source(output, source)


def _render_candidate(descriptor, *, published=False):
    """Share version-bound installer/platform content between preview and stable views."""
    escape = html.escape
    expected = {}

    def field(key, value):
        expected[key] = {'text': value, 'href': None}
        return f'<span data-release-field="{key}">{escape(value)}</span>'

    def link(key, url, label):
        expected[key] = {'text': label, 'href': url}
        return f'<a data-release-field="{key}" href="{escape(url, quote=True)}">{escape(label)}</a>'

    version = descriptor['version']
    notice = '' if published else (
        '<div class="notice"><strong>Unpublished candidate preview</strong>'
        '<p>This page describes verified local files. Release downloads and npm registry '
        'installation are not published yet. Local verification does not establish public availability '
        'or qualification on a particular host.</p></div>')
    details = '<div class="candidate"><h3>' + ('Release ' if published else 'Candidate ') + field('version', version) + '</h3>'
    details += '<p>Four native archives and one npm installer package. Both <code>twigsmith</code> and <code>tgi</code> run the same executable.</p>'
    if published:
        details += '<p>' + link('release-link', descriptor['release_url'], 'Release page') + ' · '
        details += link('npm-link', descriptor['npm']['url'], 'npm package') + '</p>'
    details += '<p><a href="#native-installation">Native archives</a> · <a href="#npm-installation">npm installer</a> · <a href="#installer-recovery">Installation recovery</a></p>'
    details += '<details><summary>SHA-256 checksums</summary><dl class="hashes">'
    for index, native in enumerate(descriptor['native']):
        for kind, asset in native['assets'].items():
            details += '<dt>' + escape(asset['name']) + '</dt><dd><code>' + field(f'hash-{index}-{kind}', asset['sha256']) + '</code></dd>'
    for package in descriptor['npm']['packages']:
        details += '<dt>' + escape(package['filename']) + '</dt><dd><code>' + field('npm-hash', package['sha256']) + '</code></dd>'
    details += '</dl></details></div>'
    requirements = '<h3>Requirements</h3><p>Choose the native archive for your system. Native commands need Git and the platform runtime; npm adds its Node.js requirements.</p><dl class="requirements platforms">'
    instructions = '<h3 id="native-installation">Native archives</h3><p>No Rust or Node.js is needed to run the extracted commands. Keep each archive with its manifest and checksum file.</p>'
    if not published:
        instructions += '<p>From a source checkout, replace BUNDLE with the local candidate directory and DESTINATION with a new extraction directory. The verification helper needs Python.</p>'
    for index, native in enumerate(descriptor['native']):
        target = native['target']
        platform_name = {'x86_64-unknown-linux-gnu': 'Linux x64',
                         'x86_64-pc-windows-msvc': 'Windows x64',
                         'aarch64-apple-darwin': 'macOS ARM64',
                         'x86_64-apple-darwin': 'macOS Intel'}[target]
        requirements += '<div><dt>' + escape(platform_name) + '</dt><dd>'
        for item, value in enumerate(native['requirements'].values()):
            requirements += ('; ' if item else '') + field(f'requirement-{index}-{item}', value)
        npm_req = descriptor['npm']['requirements'][target]['requirements']
        requirements += '<br><strong>npm:</strong> ' + field(f'npm-requirement-{index}', npm_req) + '</dd></div>'
        shell = 'PowerShell' if native['platform']['os'] == 'windows' else 'Bash or Zsh'
        instructions += '<details class="platform-install"><summary>' + escape(platform_name) + ' — ' + escape(target) + '</summary><p>' + shell + ':</p>'
        if published:
            instructions += '<p>Download the '
            for position, kind in enumerate(('archive', 'manifest', 'checksums')):
                instructions += (', ' if position else '') + link(f'{kind}-download-{index}', native['assets'][kind]['url'], kind)
            instructions += ' to the same directory, then verify and extract:</p>'
        instructions += '<pre tabindex="0"><code>' + field(f'native-command-{index}', '\n'.join(native['commands'])) + '</code></pre></details>'
    requirements += '</dl><p>Requirements are frozen for this exact version. Windows/macOS archives are unsigned; host security settings may require approval. The manual explains runtime setup and observed qualification limits.</p>'
    instructions += '<h3 id="npm-installation">One npm installer</h3><p>The package detects the host and downloads only its matching archive from the exact GitHub release, checking the archive and executable hashes before making it ready. It contains installer code and metadata, with no bundled native executable or platform npm dependencies.</p>'
    if published:
        for method, heading in [('install', 'Install globally'), ('npx', 'Run with npx')]:
            instructions += '<h4>' + heading + '</h4><pre tabindex="0"><code>' + field('npm-command-' + method, '\n'.join(descriptor['npm']['commands'][method])) + '</code></pre>'
    else:
        instructions += '<p>The native release assets must be public before this installer candidate can download them. Local qualification uses an explicitly marked loopback fixture; it does not establish public installation. This preview supplies no registry installation command.</p>'
    instructions += ('<h3 id="installer-recovery">Script policy and recovery</h3>'
                     '<p>npm 12.2.1 blocks unapproved installation scripts by default. A successful npm install can therefore leave the executable missing. '
                     '<code>--ignore-scripts</code> also skips installation. Approve the script with your npm client, or explicitly run the installer after acquiring the package:</p>'
                     '<p>After global installation:</p><pre tabindex="0"><code>twigsmith-install</code></pre>'
                     '<p>From a project with a local installation:</p><pre tabindex="0"><code>npm exec --offline --ignore-scripts -- twigsmith-install</code></pre>'
                     '<p>The offline flag prevents npm package acquisition; the installer itself still downloads the native archive. Ordinary <code>twigsmith</code> and <code>tgi</code> commands never download a missing executable.</p>')
    if published:
        instructions += '<p>For npx, repair and run the same exact version from its cache:</p><pre tabindex="0"><code>' + field('npm-command-repair', '\n'.join(descriptor['npm']['commands']['repair'])) + '</code></pre>'
    instructions += '<h4>Choose a target explicitly</h4><pre tabindex="0"><code>' + escape('\n'.join('twigsmith-install --target ' + native['target'] for native in descriptor['native'])) + '</code></pre>'
    instructions += ('<p>Explicit selection takes precedence over detection. It verifies the selected bytes and does not make a foreign executable compatible with this host. Run <code>twigsmith-install</code> without overrides to restore host detection.</p>'
                     '<p>Failed downloads retain the previous verified selection. Correct network or file permissions and retry the installer; it repairs damaged executables with newly verified bytes. Downloaded files belong to the installed package’s <code>.native/</code> directory and are removed with that package or npx cache.</p>'
                     '<p>Downloads require HTTPS access to GitHub releases and asset delivery hosts. <code>HTTPS_PROXY</code>, <code>HTTP_PROXY</code> and <code>NO_PROXY</code> apply. Set <code>NODE_EXTRA_CA_CERTS</code> before starting npm or the installer for an organization CA. Restricted networks can use a separately verified native archive. Embedded hashes bind consistency to the npm package; they do not independently authenticate its publisher.</p>')
    source = (ROOT / 'site' / 'index.html').read_text(encoding='utf-8')
    for marker, content in [('AVAILABILITY-NOTICE', notice), ('RELEASE-IDENTITY', details),
                            ('REQUIREMENTS', requirements), ('INSTALLATION-INSTRUCTIONS', instructions)]:
        placeholder = f'<!-- {marker} -->'
        if source.count(placeholder) != 1:
            raise ValueError(f'site template requires exactly one {marker} placeholder')
        source = source.replace(placeholder, content)
    _validate_rendered(source, expected, version)
    return source


def build_candidate(output, bundle):
    """Verify and build an unpublished complete-set preview using only local files."""
    import candidate
    destination, source = Path(output).resolve(), Path(bundle).resolve()
    if destination.is_relative_to(source) or source.is_relative_to(destination):
        raise ValueError('candidate preview output must be separate from its input bundle')
    descriptor = candidate.from_bundle(bundle)
    if descriptor.get('schema') != 3:
        raise ValueError('complete-set preview requires a schema 3 installer candidate bundle')
    _build_source(output, _render_candidate(descriptor))


def _promote(staged, output):
    """Retain the old directory if replacing a staged artifact fails."""
    if output.is_symlink() or (output.exists() and not output.is_dir()):
        raise ValueError('site output must be an ordinary directory')
    if not output.exists():
        os.replace(staged, output)
        return
    backup = staged.parent / 'previous'
    os.replace(output, backup)
    try:
        os.replace(staged, output)
    except BaseException:
        try:
            os.replace(backup, output)
        except OSError as error:
            raise OSError(f'site restore failed; previous artifact retained at {backup}') from error
        raise


def _build_source(output, source):
    output = Path(output).absolute()
    output.parent.mkdir(parents=True, exist_ok=True)
    temp = Path(tempfile.mkdtemp(prefix=f'.{output.name}-', dir=output.parent))
    try:
        staged = temp / 'artifact'
        staged.mkdir()
        shutil.copytree(ROOT / 'site' / 'assets', staged / 'assets', symlinks=True)
        shutil.copy2(ROOT / 'site' / 'style.css', staged / 'style.css')
        (staged / 'index.html').write_text(source, encoding='utf-8')
        for path in staged.rglob('*'):
            if path.is_symlink() or not (path.is_file() or path.is_dir()):
                raise ValueError('site artifact must contain ordinary files and directories')
        _promote(staged, output)
    except BaseException:
        # A failed restore must leave the previous artifact available for recovery.
        if not (temp / 'previous').exists():
            shutil.rmtree(temp, ignore_errors=True)
        raise
    else:
        shutil.rmtree(temp, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', required=True, type=Path, help='verified complete four-platform installer candidate directory')
    parser.add_argument('output', type=Path, help='local preview directory, separate from the bundle')
    args = parser.parse_args()
    try:
        build_candidate(args.output, args.bundle)
    except (ValueError, KeyError, TypeError, OSError, tarfile.TarError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
