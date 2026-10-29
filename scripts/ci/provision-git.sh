#!/bin/sh
# Build a qualification Git in a private prefix; never replace the system Git.
set -eu

if [ "$#" -ne 3 ]; then
    echo 'Usage: scripts/ci/provision-git.sh VERSION PREFIX EVIDENCE_DIR' >&2
    exit 2
fi
version=$1
prefix=$2
evidence=$3
case "$version" in
    2.45.4) expected=93f82ed1cab772acf3167db3be2bd2e54876e0ab53c6a9af744940752b961d88 ;;
    2.56.0) expected=26c56c296b38c0695b26fa95f475f1d01704d2d38e73465ca30b0b2f5dc789d3 ;;
    *) echo "error: unqualified Git source version: $version" >&2; exit 2 ;;
esac
case "$prefix" in /*) ;; *) echo 'error: PREFIX must be absolute' >&2; exit 2 ;; esac
if [ -e "$prefix" ] || [ -e "$evidence" ]; then
    echo 'error: Git prefix and evidence directory must not exist' >&2
    exit 1
fi
mkdir -p "$evidence"
evidence=$(CDPATH='' cd -- "$evidence" && pwd -P)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
archive="git-$version.tar.xz"
source_url="https://cdn.kernel.org/pub/software/scm/git/$archive"
checksum_url=https://www.kernel.org/pub/software/scm/git/sha256sums.asc

# Pins were read independently from this upstream checksum document on 2026-10-29.
# Fetch the document separately from the source download and retain it. We check
# its exact release record and the pinned digest, not an unauthenticated local
# hash alone. This is HTTPS/upstream-checksum verification, not PGP verification.
curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
    --retry 3 "$checksum_url" -o "$evidence/upstream-sha256sums.asc"
printf '%s  %s\n' "$expected" "$archive" > "$evidence/selected-source.sha256"
if ! grep -Fxq "$expected  $archive" "$evidence/upstream-sha256sums.asc"; then
    echo 'error: upstream checksum record does not match the reviewed Git pin' >&2
    exit 1
fi
curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
    --retry 3 "$source_url" -o "$work/$archive"
(cd "$work" && sha256sum --check "$evidence/selected-source.sha256")
tar -xJf "$work/$archive" -C "$work"
# Git 2.56 supports optional Rust components. These C builds keep provisioning
# independent of the application's Rust toolchain; the flags are recorded below.
make -C "$work/git-$version" -j2 prefix="$prefix" \
    NO_RUST=YesPlease NO_GETTEXT=YesPlease NO_TCLTK=YesPlease \
    > "$evidence/build.log" 2>&1 || { cat "$evidence/build.log" >&2; exit 1; }
make -C "$work/git-$version" prefix="$prefix" \
    NO_RUST=YesPlease NO_GETTEXT=YesPlease NO_TCLTK=YesPlease install \
    > "$evidence/install.log" 2>&1 || { cat "$evidence/install.log" >&2; exit 1; }
test "$("$prefix/bin/git" --version)" = "git version $version"
"$prefix/bin/git" --no-lazy-fetch --version
python3 - "$evidence" "$version" "$prefix" "$expected" "$source_url" "$checksum_url" <<'PY'
import hashlib
import json
import pathlib
import subprocess
import sys

evidence, version, prefix, digest, source_url, checksum_url = sys.argv[1:]
git = pathlib.Path(prefix) / "bin/git"
record = {
    "version": version,
    "source_url": source_url,
    "source_sha256": digest,
    "checksum_url": checksum_url,
    "verification": "HTTPS upstream checksum record and reviewed SHA-256 pin; PGP not verified",
    "pin_checked_on": "2026-10-29",
    "build_flags": ["NO_RUST=YesPlease", "NO_GETTEXT=YesPlease", "NO_TCLTK=YesPlease"],
    "prefix": prefix,
    "executable_sha256": hashlib.sha256(git.read_bytes()).hexdigest(),
    "build_options": subprocess.check_output([str(git), "version", "--build-options"], text=True),
}
(pathlib.Path(evidence) / "git-source.json").write_text(json.dumps(record, indent=2) + "\n")
PY
