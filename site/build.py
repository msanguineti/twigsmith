#!/usr/bin/env python3
"""Build a static site preview from verified release inputs.

Preview is the default. A published artifact additionally requires a fresh,
identity-bound consumption receipt from the publication verifier. A receipt records
observations, not publisher authentication or permission to deploy Pages.
"""
import argparse
from datetime import datetime, timezone
import html
from html.parser import HTMLParser
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tarfile
import tempfile
from urllib.parse import quote

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tools'))
import release_set


def validate_receipt(record, receipt, *, now=None):
    """Require recent native, npm install and npx observations of this exact tuple."""
    if record.get('schema') in (2, 3):
        raise ValueError('complete platform sets cannot use legacy publication receipts')
    try:
        if (receipt['identity'] != record
                or receipt['native_sha256'] != record['native']['sha256']
                or receipt['npm_sha256'] != record['npm']['sha256']):
            raise ValueError('consumption receipt differs from release identity')
        observed = datetime.fromisoformat(receipt['observed_at'].replace('Z', '+00:00'))
        if observed.tzinfo is None:
            raise ValueError('consumption receipt timestamp must include timezone')
        age = ((now or datetime.now(timezone.utc)) - observed).total_seconds()
        if age < -60 or age > 900:
            raise ValueError('consumption receipt is not fresh')
        checks = receipt['consumption']
        for name in ('native', 'npm', 'npx'):
            if (checks[name]['version'] != record['version']
                    or checks[name]['commands'] != ['twigsmith', 'tgi']):
                raise ValueError(f'incomplete {name} consumption receipt')
    except (KeyError, TypeError, AttributeError) as error:
        raise ValueError('invalid consumption receipt') from error


def _validate_identity(record, published):
    if record.get('schema') in (2, 3):
        raise ValueError('complete platform sets require a verified bundle preview; full-set publication is unavailable')
    if (record['repository'] != release_set.REPOSITORY
            or not re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?', record['version'])
            or record['tag'] != 'v' + record['version']
            or record['native']['archive'] != f'twigsmith-{record["version"]}-x86_64-unknown-linux-gnu.tar.gz'
            or record['npm']['package'] != f'twigsmith-{record["version"]}.tgz'):
        raise ValueError('invalid site release identity')
    for component in ('native', 'npm'):
        for field in ('sha256', 'manifest_sha256'):
            if not re.fullmatch(r'[a-f0-9]{64}', record[component][field]):
                raise ValueError('invalid site release hash')
    if not re.fullmatch(r'[a-f0-9]{64}', record['native']['payload_sha256']):
        raise ValueError('invalid site payload hash')
    if published:
        publication = record.get('publication', {})
        if (set(publication) != {'source', 'channel', 'staging_channel', 'prerelease', 'release_id'}
                or not re.fullmatch(r'[0-9a-f]{40}', publication['source'])
                or publication['source'] != record['native']['source']['revision']
                or publication['prerelease'] is not False or '-' in record['version']
                or publication['channel'] != 'latest'
                or type(publication['release_id']) is not int or publication['release_id'] <= 0
                or publication['channel'] == publication['staging_channel']
                or not re.fullmatch(r'[a-z][a-z0-9-]{0,80}', publication['staging_channel'])
                or publication['staging_channel'] in ('latest', 'next')):
            raise ValueError('invalid site publication identity')


def _render(record, published):
    version = record['version']
    archive = record['native']['archive']
    stem = archive[:-7]
    base = f'https://github.com/{record["repository"]}/releases'
    tag = quote(record['tag'], safe='')
    release = f'{base}/tag/{tag}'
    downloads = f'{base}/download/{tag}'
    download = f'{downloads}/{quote(archive, safe="")}'
    npm = f'https://www.npmjs.com/package/twigsmith/v/{quote(version, safe="")}'
    label = 'Release' if published else 'Candidate'
    details = (f'<div class="candidate"><h3>{label} {html.escape(version)}</h3>'
               + ('' if published else '<p>These planned locations may not be available yet. Verify publication before installing.</p>')
               + f'<p><a href="{release}">Release page</a> · <a href="{download}">Native archive</a> · '
               f'<a href="{npm}">npm package</a></p>'
               f'<details><summary>SHA-256 checksums</summary><p>Native archive: <code>{record["native"]["sha256"]}</code><br>'
               f'npm tarball: <code>{record["npm"]["sha256"]}</code></p></details></div>')
    if published:
        notice = ''
        instructions = f'''<h3>Install with npm</h3><pre tabindex="0"><code>npm install --global twigsmith@{version}
 twigsmith --version
 tgi --help</code></pre>
    <h3>Or run with npx</h3><pre tabindex="0"><code>npx --yes --package=twigsmith@{version} twigsmith --version
 npx --yes --package=twigsmith@{version} tgi --help</code></pre>
    <h3>Or install the native archive</h3><p>Download the <a href="{download}">archive</a>, <a href="{downloads}/{stem}.manifest.json">manifest</a> and <a href="{downloads}/{stem}.sha256">checksums</a> to the same directory:</p>
    <pre tabindex="0"><code>sha256sum --check {stem}.sha256
 tar -xzf {archive}
 export PATH="$PWD/{stem}:$PATH"
 twigsmith --version
 tgi --help</code></pre>
'''
        instructions = instructions.replace('\n ', '\n')
    else:
        notice = ('<div class="notice"><strong>Candidate availability</strong><p>Release downloads and npm registry installation are not published yet. The instructions below describe the current local candidate.</p></div>')
        instructions = '''<h3>Verify a native candidate</h3><p>Keep the archive with its manifest and checksum. From a source checkout, verify the archive and extract it to a new directory:</p>
    <pre tabindex="0"><code>./dev verify-native ARCHIVE --extract-to DESTINATION
export PATH="DESTINATION/ARCHIVE_DIRECTORY:$PATH"
twigsmith --version
tgi --help</code></pre>
    <p>Checksums detect mismatches with the accompanying manifest; they do not independently authenticate the publisher. See the <a href="https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md#native-archive-candidates">native candidate guide</a> for the full instructions and compatibility limits.</p>
    <h3>Local npm candidate</h3><p>Install a candidate tarball with <code>npm install TARBALL</code>, or use <code>npm install --global TARBALL</code>. The package targets Linux x86_64 GNU; confirm the glibc baseline yourself.</p>'''
    source = (ROOT / 'site' / 'index.html').read_text(encoding='utf-8')
    # This legacy candidate view does not assert requirements for a different
    # published version. The resolved release view supplies version-bound details.
    requirements = '<h3>Requirements</h3><p>See the release-specific requirements before installing.</p>'
    for marker, content in [('AVAILABILITY-NOTICE', notice), ('RELEASE-IDENTITY', details),
                            ('REQUIREMENTS', requirements),
                            ('INSTALLATION-INSTRUCTIONS', instructions)]:
        placeholder = f'<!-- {marker} -->'
        if source.count(placeholder) != 1:
            raise ValueError(f'site template requires exactly one {marker} placeholder')
        source = source.replace(placeholder, content)
    return source


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
    """Render an already provider-validated, release-bound model."""
    if model.get('schema') in (2, 3):
        raise ValueError('complete platform sets support unpublished previews only; full-set publication is unavailable')
    escape = html.escape
    expected = {}

    def text_field(key, value, tag='span'):
        expected[key] = {'text': value, 'href': None}
        return f'<{tag} data-release-field="{key}">{escape(value)}</{tag}>'

    def link(key, value, label):
        expected[key] = {'text': label, 'href': value}
        return f'<a data-release-field="{key}" href="{escape(value, quote=True)}">{escape(label)}</a>'

    version = model['version']
    npm = model['npm']
    details = '<div class="candidate"><h3>Release ' + text_field('version', version) + '</h3><p>'
    details += link('release-link', model['release_url'], 'Release page')
    for index, native in enumerate(model['native']):
        label = 'Native archive' if len(model['native']) == 1 else 'Native archive ' + str(index + 1)
        details += ' · ' + link(f'archive-link-{index}', native['assets']['archive']['url'], label)
    details += ' · ' + link('npm-link', npm['url'], 'npm package') + '</p>'
    details += '<details><summary>SHA-256 checksums</summary><p>'
    for index, native in enumerate(model['native']):
        details += 'Native archive: <code>' + text_field(f'archive-hash-{index}', native['assets']['archive']['sha256']) + '</code><br>'
    details += 'npm tarball: <code>' + text_field('npm-hash', npm['sha256']) + '</code></p></details></div>'

    requirements = '<h3>Requirements</h3><dl class="requirements">'
    for index, native in enumerate(model['native']):
        for item, (key, value) in enumerate(native['requirements'].items()):
            label = {'system': 'System', 'shell': 'Editors & checks'}.get(key, key.capitalize())
            requirements += '<div><dt>' + escape(label) + '</dt><dd>' + text_field(f'requirement-{index}-{item}', value) + '</dd></div>'
    coverage = '; '.join(' / '.join(str(platform[key]) for key in ('os', 'arch', 'libc') if platform.get(key))
                         for platform in npm['coverage'])
    requirements += '<div><dt>npm &amp; npx</dt><dd>' + text_field('npm-requirements', npm['requirements'])
    requirements += ' (' + text_field('npm-coverage', coverage) + ')</dd></div></dl>'

    instructions = ''
    for method, heading in [('install', 'Install with npm'), ('npx', 'Or run with npx')]:
        instructions += '<h3>' + heading + '</h3><pre tabindex="0"><code>'
        instructions += text_field('npm-command-' + method, '\n'.join(npm['commands'][method])) + '</code></pre>'
    for index, native in enumerate(model['native']):
        platform = native['platform']
        label = ' / '.join(str(platform[key]) for key in ('os', 'arch', 'libc') if platform.get(key))
        heading = 'Or install the native archive' if native['commands'] else 'Native downloads'
        instructions += '<h3>' + heading + '</h3><p>' + text_field(f'platform-{index}', label) + ': Download the '
        for position, (kind, title) in enumerate([('archive', 'archive'), ('manifest', 'manifest'), ('checksums', 'checksums')]):
            if position:
                instructions += ', ' if position == 1 else ' and '
            instructions += link(f'{kind}-download-{index}', native['assets'][kind]['url'], title)
        if native['commands']:
            instructions += ' to the same directory:</p><pre tabindex="0"><code>'
            instructions += text_field(f'native-command-{index}', '\n'.join(native['commands'])) + '</code></pre>'
        else:
            instructions += '.</p>'

    source = (ROOT / 'site' / 'index.html').read_text(encoding='utf-8')
    for marker, content in [('AVAILABILITY-NOTICE', ''), ('RELEASE-IDENTITY', details),
                            ('REQUIREMENTS', requirements), ('INSTALLATION-INSTRUCTIONS', instructions)]:
        placeholder = f'<!-- {marker} -->'
        if source.count(placeholder) != 1:
            raise ValueError(f'site template requires exactly one {marker} placeholder')
        source = source.replace(placeholder, content)
    _validate_rendered(source, expected, version)
    return source


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


def _render_candidate(descriptor):
    """Render a complete offline set without suggesting public availability."""
    escape = html.escape
    notice = ('<div class="notice"><strong>Unpublished candidate</strong>'
              '<p>This preview describes verified local files. Release downloads and npm registry '
              'installation are not published yet.</p></div>')
    details = '<div class="candidate"><h3>Candidate ' + escape(descriptor['version']) + '</h3>'
    details += '<p>Four native archives and one npm installer package.</p><details><summary>SHA-256 checksums</summary><dl>'
    for native in descriptor['native']:
        for asset in native['assets'].values():
            details += '<dt>' + escape(asset['name']) + '</dt><dd><code>' + asset['sha256'] + '</code></dd>'
    details += '<dt>npm/manifest.json</dt><dd><code>' + descriptor['npm']['manifest_sha256'] + '</code></dd>'
    for package in descriptor['npm']['packages']:
        details += '<dt>' + escape(package['filename']) + '</dt><dd><code>' + package['sha256'] + '</code></dd>'
    details += '</dl></details></div>'
    requirements = '<h3>Requirements frozen in this candidate</h3><dl class="requirements">'
    instructions = ('<p>From a source checkout, replace BUNDLE with the local candidate directory '
                    'and DESTINATION with a new extraction directory. The verification helper needs '
                    'Python; the extracted native commands need neither Node/npm nor Rust.</p>')
    for native in descriptor['native']:
        target = native['target']
        requirements += '<div><dt>' + escape(target) + '</dt><dd>'
        requirements += '; '.join(escape(value) for value in native['requirements'].values()) + '</dd></div>'
        npm_requirements = descriptor['npm']['requirements'][target]
        coverage = '; '.join(' / '.join(str(platform[key]) for key in ('os', 'arch', 'libc') if platform.get(key))
                             for platform in npm_requirements['coverage'])
        requirements += '<div><dt>Frozen npm requirements for ' + escape(target) + '</dt><dd>'
        requirements += escape(npm_requirements['requirements']) + ' (' + escape(coverage) + ')</dd></div>'
        shell = 'PowerShell' if native['platform']['os'] == 'windows' else 'Bash or Zsh'
        instructions += '<h3>Local native candidate: ' + escape(target) + '</h3><p>' + shell + ':</p>'
        instructions += '<pre tabindex="0"><code>' + escape('\n'.join(native['commands'])) + '</code></pre>'
    requirements += '</dl><p>Frozen candidate metadata does not establish additional supported OS or shell versions.</p>'
    instructions += ('<h3>npm installer candidate</h3><p>The single package contains installer code and '
                     'approved native hashes. With lifecycle scripts approved, installation downloads '
                     'only the selected target from its exact GitHub release. These native assets must '
                     'be public before registry installation can work. Local qualification uses an '
                     'explicitly marked loopback fixture; it does not establish public availability.</p>'
                     '<p>When scripts are blocked, use the installed twigsmith-install command to repair '
                     'installation. For deliberate target selection, pass --target and one of the '
                     'triples below to that installer. Ordinary twigsmith and tgi commands never download '
                     'a missing executable.</p><ul>')
    for coverage in descriptor['npm']['coverage']:
        label = ' / '.join(str(coverage[key]) for key in ('os', 'arch', 'libc') if coverage.get(key))
        instructions += '<li>' + escape(label + ': ' + coverage['target']) + '</li>'
    instructions += '</ul><dl>'
    for package in descriptor['npm']['packages']:
        instructions += '<dt>' + escape(package['name'] + '@' + package['version']) + '</dt><dd>'
        instructions += escape(package['filename']) + '<br><code>' + escape(package['integrity']) + '</code></dd>'
    instructions += '</dl>'
    source = (ROOT / 'site' / 'index.html').read_text(encoding='utf-8')
    for marker, content in [('AVAILABILITY-NOTICE', notice), ('RELEASE-IDENTITY', details),
                            ('REQUIREMENTS', requirements), ('INSTALLATION-INSTRUCTIONS', instructions)]:
        placeholder = f'<!-- {marker} -->'
        if source.count(placeholder) != 1:
            raise ValueError(f'site template requires exactly one {marker} placeholder')
        source = source.replace(placeholder, content)
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


def build(output, record, *, receipt=None):
    """Stage and replace an artifact; only verified consumption enables release copy."""
    published = receipt is not None
    _validate_identity(record, published)
    if published:
        validate_receipt(record, receipt)
    source = _render(record, published)
    _build_source(output, source, lambda: validate_receipt(record, receipt) if published else None)


def _build_source(output, source, before_promote=lambda: None):
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
        before_promote()
        _promote(staged, output)
    except BaseException:
        # A failed restore must leave the previous artifact available for recovery.
        if not (temp / 'previous').exists():
            shutil.rmtree(temp, ignore_errors=True)
        raise
    else:
        shutil.rmtree(temp, ignore_errors=True)


def promotion_matches(output, record, receipt):
    """Observe the complete local artifact; this says nothing about hosted Pages."""
    try:
        _validate_identity(record, True)
        validate_receipt(record, receipt)
        output = Path(output)
        if output.is_symlink() or not output.is_dir():
            return False
        expected = {'index.html': _render(record, True).encode('utf-8'),
                    'style.css': (ROOT / 'site' / 'style.css').read_bytes()}
        assets = ROOT / 'site' / 'assets'
        for path in assets.rglob('*'):
            if path.is_symlink() or not (path.is_file() or path.is_dir()):
                return False
            if path.is_file():
                expected['assets/' + path.relative_to(assets).as_posix()] = path.read_bytes()
        observed = {}
        for path in output.rglob('*'):
            if path.is_symlink() or not (path.is_file() or path.is_dir()):
                return False
            if path.is_file():
                observed[path.relative_to(output).as_posix()] = path.read_bytes()
        return observed == expected
    except (ValueError, KeyError, TypeError, OSError):
        return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', nargs='?', help='native archive; with --bundle, preview output directory')
    parser.add_argument('npm_dir', nargs='?')
    parser.add_argument('output', nargs='?')
    parser.add_argument('--bundle', type=Path, help='verified complete-set candidate directory; use --bundle BUNDLE OUTPUT')
    parser.add_argument('--archive-sha256')
    parser.add_argument('--payload-sha256')
    parser.add_argument('--package-sha256')
    parser.add_argument('--tag')
    parser.add_argument('--native-manifest-sha256')
    parser.add_argument('--npm-manifest-sha256')
    parser.add_argument('--channel')
    parser.add_argument('--receipt', type=Path, help='fresh consumption receipt from the publication verifier')
    args = parser.parse_args()
    try:
        hashes = [args.archive_sha256, args.payload_sha256, args.package_sha256,
                  args.native_manifest_sha256, args.npm_manifest_sha256, args.tag]
        if args.bundle:
            if not args.archive or args.npm_dir or args.output or any(hashes) or args.receipt or args.channel:
                raise ValueError('use --bundle BUNDLE OUTPUT without publication or legacy candidate options')
            build_candidate(args.archive, args.bundle)
            return
        if not all([args.archive, args.npm_dir, args.output, *hashes]):
            raise ValueError('legacy preview requires archive, npm_dir, output, tag and all five hashes')
        record = release_set.verify(args.archive, args.npm_dir, args.archive_sha256,
                                    args.payload_sha256, args.package_sha256, args.tag,
                                    args.native_manifest_sha256, args.npm_manifest_sha256)
        receipt = json.loads(args.receipt.read_text(encoding='utf-8')) if args.receipt else None
        if receipt is not None:
            record['publication'] = receipt['identity']['publication']
            if not args.channel or args.channel != record['publication']['channel']:
                raise ValueError('requested channel differs from consumption receipt')
        elif args.channel:
            raise ValueError('--channel requires a consumption receipt')
        build(args.output, record, receipt=receipt)
    except (ValueError, KeyError, TypeError, OSError, tarfile.TarError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
