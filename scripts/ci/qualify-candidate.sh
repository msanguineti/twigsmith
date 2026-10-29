#!/bin/sh
# Consume the transferred release and fixture bytes; this path never compiles.
set -eu
if [ "$#" -ne 4 ]; then
    echo 'Usage: scripts/ci/qualify-candidate.sh BUNDLE_DIR RUN_DIR PROFILE GIT' >&2
    exit 2
fi
bundle=$(CDPATH='' cd -- "$1" && pwd -P)
run=$2
profile=$3
selected_git=$4
case "$run" in /*) ;; *) echo 'error: RUN_DIR must be absolute' >&2; exit 2 ;; esac
case "$profile" in broad|floor|representative) ;; *) echo 'error: unknown profile' >&2; exit 2 ;; esac
if [ -e "$run" ]; then
    echo 'error: RUN_DIR must not exist' >&2
    exit 1
fi
mkdir -p "$run/evidence"
python3 scripts/ci/record-environment.py "$run/evidence/runner.json"
cp "$bundle"/*.manifest.json "$bundle"/*.sha256 "$run/evidence/"
set -- "$bundle"/twigsmith-*-x86_64-unknown-linux-gnu.tar.gz
if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
    echo 'error: expected exactly one native candidate archive' >&2
    exit 1
fi
archive=$1
./dev verify-native "$archive" --extract-to "$run/extracted package"
(cd "$bundle" && sha256sum --check harness.tar.gz.sha256)
mkdir "$run/harness"
tar -xzf "$bundle/harness.tar.gz" -C "$run/harness"
python3 - "$archive" "$run/harness/harnesses.json" <<'PY'
import json
import os
from pathlib import Path
import sys

archive, harness_path = map(Path, sys.argv[1:])
manifest = json.loads(archive.with_name(archive.name[:-7] + ".manifest.json").read_text())
source = manifest["provenance"]["source"]
harness = json.loads(harness_path.read_text())
if source["revision"] != harness["source_revision"]:
    sys.exit("error: candidate and fixture harness source revisions differ")
if os.environ.get("GITHUB_SHA") and (
    source["revision"] != os.environ["GITHUB_SHA"]
    or source["dirty"] or harness["source_dirty"]
):
    sys.exit("error: CI requires clean candidate and fixture source at GITHUB_SHA")
PY
set -- "$run/extracted package"/*/twigsmith
if [ "$#" -ne 1 ] || [ ! -x "$1" ]; then
    echo 'error: expected one extracted executable package' >&2
    exit 1
fi
artifact_dir=$(dirname -- "$1")
python3 scripts/ci/inspect-native-elf.py --artifact-dir "$artifact_dir" \
    --evidence-dir "$run/evidence/elf"
./dev native-qualify run --artifact-dir "$artifact_dir" \
    --harness-dir "$run/harness" --profile "$profile" \
    --evidence-dir "$run/evidence/qualification" --git "$selected_git" \
    > "$run/evidence/driver.log" 2>&1 \
    || { cat "$run/evidence/driver.log" >&2; exit 1; }
cat "$run/evidence/driver.log"
