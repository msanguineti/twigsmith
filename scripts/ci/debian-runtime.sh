#!/bin/sh
# Called inside the recorded Debian image, with /work and /candidate read-only.
set -eu
apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
    build-essential ca-certificates curl xz-utils python3 binutils perl \
    libcurl4-openssl-dev libssl-dev zlib1g-dev libexpat1-dev
sh scripts/ci/provision-git.sh 2.56.0 /tmp/qualification-git /runtime/git-source
export PATH="/tmp/qualification-git/bin:$PATH"
sh scripts/ci/qualify-candidate.sh /candidate /runtime/run representative \
    /tmp/qualification-git/bin/git
