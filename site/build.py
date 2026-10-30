#!/usr/bin/env python3
"""Build an unpublished site preview from locally verified release inputs.

This checks artifact identity only. It does not establish publication or source
eligibility. The emitted release links describe planned locations, not availability.
"""
import argparse
import html
from pathlib import Path
import shutil
import sys
from urllib.parse import quote

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tools'))
import release_set


def build(output, record):
    output = Path(output)
    if output.exists():
        raise ValueError('output directory already exists')
    output.mkdir(parents=True)
    shutil.copytree(ROOT / 'site' / 'assets', output / 'assets')
    shutil.copy2(ROOT / 'site' / 'style.css', output / 'style.css')
    version = html.escape(record['version'])
    tag = quote(record['tag'], safe='')
    archive = quote(record['native']['archive'], safe='')
    repository = record['repository']
    release = f'https://github.com/{repository}/releases/tag/{tag}'
    download = f'https://github.com/{repository}/releases/download/{tag}/{archive}'
    npm = f'https://www.npmjs.com/package/twigsmith/v/{quote(record["version"], safe="")}'
    details = (f'<div class="candidate"><h3>Candidate {version}</h3>'
               '<p>These planned locations may not be available yet. Verify publication before installing.</p>'
               f'<p><a href="{release}">Release page</a> · <a href="{download}">Native archive</a> · '
               f'<a href="{npm}">npm package</a></p>'
               f'<p>Native archive SHA-256: <code>{record["native"]["sha256"]}</code><br>'
               f'npm tarball SHA-256: <code>{record["npm"]["sha256"]}</code></p></div>')
    source = (ROOT / 'site' / 'index.html').read_text(encoding='utf-8')
    (output / 'index.html').write_text(source.replace('<!-- RELEASE-IDENTITY -->', details), encoding='utf-8')


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
    args = parser.parse_args()
    try:
        record = release_set.verify(args.archive, args.npm_dir, args.archive_sha256,
                                    args.payload_sha256, args.package_sha256, args.tag,
                                    args.native_manifest_sha256, args.npm_manifest_sha256)
        build(args.output, record)
    except (ValueError, KeyError, OSError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
