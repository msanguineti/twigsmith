#!/usr/bin/env python3
"""Record actual CI identity and userspace; a container shares its host kernel."""
import json
import os
from pathlib import Path
import platform
import subprocess
import sys


def observe(argv):
    try:
        result = subprocess.run(argv, text=True, capture_output=True, check=False)
        return {"argv": argv, "status": result.returncode,
                "stdout": result.stdout, "stderr": result.stderr}
    except OSError as error:
        return {"argv": argv, "error": str(error)}


if len(sys.argv) != 2:
    sys.exit("Usage: scripts/ci/record-environment.py OUTPUT.json")
output = Path(sys.argv[1])
output.parent.mkdir(parents=True, exist_ok=True)
record = {
    "platform": platform.platform(),
    "architecture": platform.machine(),
    "kernel": platform.release(),
    "os_release": Path("/etc/os-release").read_text(),
    "context": {key: os.environ.get(key) for key in (
        "GITHUB_SHA", "GITHUB_REF", "GITHUB_HEAD_REF", "GITHUB_BASE_REF",
        "GITHUB_REPOSITORY", "GITHUB_WORKFLOW", "GITHUB_WORKFLOW_REF", "GITHUB_WORKFLOW_SHA",
        "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_JOB", "RUNNER_OS",
        "RUNNER_ARCH", "RUNNER_NAME", "ImageOS", "ImageVersion", "ImageRelease",
        "TWIGSMITH_CONTAINER_DIGEST")},
    "observations": [observe(argv) for argv in (
        ["uname", "-a"], ["ldd", "--version"], ["readlink", "-f", "/bin/sh"],
        ["dpkg-query", "-W", "libc6", "libgcc-s1", "libstdc++6", "dash", "bash"],
        ["bash", "--version"], ["cc", "--version"], ["git", "--version"],
    )],
}
output.write_text(json.dumps(record, indent=2) + "\n")
