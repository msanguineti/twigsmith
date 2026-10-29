#!/usr/bin/env python3
"""Inspect the trusted extracted candidate against the current GNU userspace."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


INTERPRETER = "/lib64/ld-linux-x86-64.so.2"
LIBRARIES = {"libc.so.6", "libm.so.6", "libgcc_s.so.1", "ld-linux-x86-64.so.2"}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inspect(artifact, evidence):
    artifact = Path(artifact).resolve()
    evidence = Path(evidence).resolve()
    evidence.mkdir(parents=True, exist_ok=False)
    report_path = evidence / "elf.json"
    report = {"schema": 1, "passed": False, "executables": {},
              "allowed_interpreter": INTERPRETER, "allowed_needed": sorted(LIBRARIES),
              "scope": "ELF dependencies and warning-free GNU loader resolution on this host; no old-kernel guarantee"}
    clean_env = {"PATH": os.defpath, "LC_ALL": "C"}

    def command(argv, filename):
        result = subprocess.run(argv, env=clean_env, text=True, capture_output=True, timeout=30)
        (evidence / filename).write_text(
            "argv: " + json.dumps(argv) + f"\nstatus: {result.returncode}\nstdout:\n"
            + result.stdout + "\nstderr:\n" + result.stderr)
        if result.returncode or result.stderr:
            raise ValueError(f"ELF inspection command failed or warned: {argv}; see {filename}")
        return result.stdout

    try:
        readelf = shutil.which("readelf", path=os.defpath)
        if not readelf:
            raise ValueError("readelf from binutils is required for native qualification")
        report["readelf"] = command([readelf, "--version"], "readelf-version.txt").splitlines()[0]
        for name in ("twigsmith", "tgi"):
            path = artifact / name
            if path.is_symlink() or not path.is_file() or not os.access(path, os.X_OK):
                raise ValueError(f"expected regular executable candidate: {path}")
            report["executables"][name] = {"path": str(path), "sha256": digest(path)}
        for name, binary in report["executables"].items():
            path = artifact / name
            header = path.read_bytes()[:20]
            if header[:6] != b"\x7fELF\x02\x01" or header[18:20] != b"\x3e\x00":
                raise ValueError(f"expected little-endian ELF64 x86_64: {name}")
            headers = command([readelf, "--wide", "--program-headers", str(path)], f"{name}-headers.txt")
            dynamic = command([readelf, "--wide", "--dynamic", str(path)], f"{name}-dynamic.txt")
            interpreters = re.findall(r"\[Requesting program interpreter: ([^\]]+)\]", headers)
            needed = re.findall(r"\(NEEDED\)\s+Shared library: \[([^\]]+)\]", dynamic)
            binary.update({"interpreters": interpreters, "needed": needed})
            if interpreters != [INTERPRETER] or not Path(INTERPRETER).is_file():
                raise ValueError(f"unexpected or unavailable ELF interpreter for {name}: {interpreters}")
            if (not needed or len(needed) != len(set(needed)) or set(needed) - LIBRARIES
                    or re.search(r"\((?:RPATH|RUNPATH)\)", dynamic)):
                raise ValueError(f"unexpected ELF dependencies or runtime search path for {name}: {needed}")

            # Invoke the reviewed GNU interpreter directly with --list: resolve
            # dependencies without invoking application main, and retain warnings.
            listing = command([INTERPRETER, "--list", str(path)], f"{name}-loader.txt")
            resolved = set(re.findall(r"^\s*(\S+) =>", listing, re.MULTILINE))
            if re.search(r"^\s*" + re.escape(INTERPRETER) + r"\s+\(", listing, re.MULTILINE):
                resolved.add(Path(INTERPRETER).name)
            if set(needed) - resolved or resolved - LIBRARIES or "not found" in listing:
                raise ValueError(f"unresolved or unexpected loader dependency for {name}")
        report["passed"] = True
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-dir", required=True, type=Path)
    parser.add_argument("--evidence-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        inspect(args.artifact_dir, args.evidence_dir)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(f"error: {error}")
    print(f"ELF inspection passed for both executables; evidence: {args.evidence_dir}")


if __name__ == "__main__":
    main()
