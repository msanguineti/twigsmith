#!/usr/bin/env python3
"""Build a static site preview from verified release inputs.

Preview is the default. A published artifact additionally requires a fresh,
identity-bound consumption receipt from the publication verifier. A receipt records
observations, not publisher authentication or permission to deploy Pages.
"""
import argparse
from datetime import datetime, timezone
import html
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile
from urllib.parse import quote

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tools'))
import release_set


def validate_receipt(record, receipt, *, now=None):
    """Require recent native, npm install and npx observations of this exact tuple."""
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
        if (set(publication) != {'source', 'channel', 'staging_channel', 'prerelease'}
                or not re.fullmatch(r'[0-9a-f]{40}', publication['source'])
                or publication['source'] != record['native']['source']['revision']
                or publication['prerelease'] is not True or '-' not in record['version']
                or publication['channel'] == publication['staging_channel']
                or any(not re.fullmatch(r'[a-z][a-z0-9-]{0,80}', publication[name])
                       or publication[name] == 'latest' for name in ('channel', 'staging_channel'))):
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
               f'<p>Native archive SHA-256: <code>{record["native"]["sha256"]}</code><br>'
               f'npm tarball SHA-256: <code>{record["npm"]["sha256"]}</code></p></div>')
    if published:
        notice = (f'<div class="notice"><strong>Version {html.escape(version)} · {html.escape(record["publication"]["channel"])} channel</strong>'
                  '<p>Download the Linux native archive or install the same version with npm.</p></div>')
        instructions = f'''<h3>Native installation</h3><p>Download the <a href="{download}">archive</a>,
    <a href="{downloads}/{stem}.manifest.json">manifest</a> and
    <a href="{downloads}/{stem}.sha256">checksums</a> into the same directory. Verify before extracting:</p>
    <pre tabindex="0"><code>sha256sum --check {stem}.sha256
 tar -xzf {archive}
 export PATH="$PWD/{stem}:$PATH"
 twigsmith --version
 tgi --help</code></pre>
    <p>Compare the archive hash with the release identity above. Checksums detect mismatches; they do not independently authenticate the publisher. The <a href="https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md#native-archive-candidates">native archive guide</a> also covers verification with a source checkout and compatibility limits.</p>
    <h3>npm installation</h3><pre tabindex="0"><code>npm install --global twigsmith@{version}
 twigsmith --version
 tgi --help</code></pre>
    <h3>Run with npx</h3><pre tabindex="0"><code>npx --yes --package=twigsmith@{version} twigsmith --version
 npx --yes --package=twigsmith@{version} tgi --help</code></pre>
    <p>The npm package targets Linux x86_64 GNU; confirm the glibc baseline before installing.</p>'''
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
    for marker, content in [('AVAILABILITY-NOTICE', notice), ('RELEASE-IDENTITY', details),
                            ('INSTALLATION-INSTRUCTIONS', instructions)]:
        placeholder = f'<!-- {marker} -->'
        if source.count(placeholder) != 1:
            raise ValueError(f'site template requires exactly one {marker} placeholder')
        source = source.replace(placeholder, content)
    return source


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
        if published:
            validate_receipt(record, receipt)
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
    parser.add_argument('archive')
    parser.add_argument('npm_dir')
    parser.add_argument('output')
    parser.add_argument('--archive-sha256', required=True)
    parser.add_argument('--payload-sha256', required=True)
    parser.add_argument('--package-sha256', required=True)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--native-manifest-sha256', required=True)
    parser.add_argument('--npm-manifest-sha256', required=True)
    parser.add_argument('--channel')
    parser.add_argument('--receipt', type=Path, help='fresh consumption receipt from the publication verifier')
    args = parser.parse_args()
    try:
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
    except (ValueError, KeyError, TypeError, OSError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
