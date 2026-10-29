#!/usr/bin/env python3
"""Build and verify the finite Linux native package format (Python 3.10+)."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import stat
import subprocess
import sys
import tarfile
import tempfile

TARGET = "x86_64-unknown-linux-gnu"
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?\Z")
MEMBERS = {"twigsmith": 0o755, "tgi": 0o755, "LICENSE": 0o644, "README.txt": 0o644}
GUIDANCE = """Twigsmith {version} — Linux x86_64 GNU native candidate

Extract this archive with tar -xzf, then add the extracted directory to PATH:
  export PATH="/absolute/path/to/{stem}:$PATH"
Run twigsmith --help or tgi --help; both names use the same native implementation.

Requires Git 2.45 or newer and POSIX sh on PATH for shell execution paths.
Rust, Node and npm are not required. Editors, signing/transport helpers and
project checks are needed only when you select those features.
The initial build baseline is Ubuntu 22.04 (glibc 2.35); qualification evidence
must establish the supported environments for these exact bytes. This archive
alone does not establish every distribution, shared-library or kernel boundary.
Optional completion shells have separate compatibility requirements.

Verify the adjacent .sha256 and .manifest.json with the repository's
./dev verify-native command before using a candidate. SHA-256 establishes
consistency with those files; it does not independently authenticate a publisher.
See the repository docs/USAGE.md for configuration and recovery guidance.
"""


def digest(data):
    return hashlib.sha256(data).hexdigest()


def run(args, cwd=None, env=None):
    return subprocess.check_output(args, cwd=cwd, env=env, text=True).strip()


def git_output(repo, *arguments, text=True):
    # Provenance describes stored commits, never a local replacement view of
    # them. Keep every metadata/object/index read on this same Git boundary.
    output = subprocess.check_output(
        ["git", "--no-replace-objects", *arguments], cwd=repo, text=text
    )
    return output.strip() if text else output


def identity(repo):
    revision = git_output(repo, "rev-parse", "HEAD")
    tree = git_output(repo, "rev-parse", f"{revision}^{{tree}}")
    if git_output(repo, "rev-parse", "--show-object-format") != "sha1":
        raise ValueError("unsupported Git object format for native package provenance")
    dirty = bool(git_output(repo, "status", "--porcelain", "--untracked-files=all"))
    # Git status can deliberately trust the index for assume-unchanged and
    # skip-worktree entries. Read committed objects and actual filesystem bytes
    # independently: those flags must never attest changed input as clean.
    committed = {}
    entries = git_output(repo, "ls-tree", "-rz", "--full-tree", revision, text=False).split(b"\0")
    for entry in filter(None, entries):
        metadata, name = entry.split(b"\t", 1)
        mode, kind, object_id = metadata.split(b" ")
        if kind != b"blob" or mode not in (b"100644", b"100755", b"120000"):
            raise ValueError(f"unsupported committed source entry: {os.fsdecode(name)}")
        committed[name] = mode, object_id.decode("ascii")
    paths = git_output(repo, "ls-files", "--cached", "--others", "--exclude-standard", "-z", text=False).split(b"\0")
    state = hashlib.sha256()
    for name in sorted(set(paths).union(committed) - {b""}):
        path = repo / os.fsdecode(name)
        for parent in path.relative_to(repo).parents:
            ancestor = repo / parent
            if ancestor.is_symlink():
                raise ValueError(f"unsupported symlinked source directory: {ancestor}")
        state.update(name + b"\0")
        expected = committed.get(name)
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            state.update(b"deleted")
            dirty = True
            continue
        if stat.S_ISLNK(metadata.st_mode):
            data = os.fsencode(os.readlink(path))
            state.update(b"symlink\0" + data)
            mode = b"120000"
        elif stat.S_ISREG(metadata.st_mode):
            permissions = stat.S_IMODE(metadata.st_mode)
            data = path.read_bytes()
            state.update(str(permissions).encode() + b"\0")
            state.update(hashlib.sha256(data).digest())
            # Git records executable versus nonexecutable regular files. Be
            # conservative about partial execute masks the tree cannot express.
            executable = permissions & 0o111
            mode = b"100755" if executable == 0o111 else b"100644" if not executable else None
        else:
            raise ValueError(f"unsupported source entry: {path}")
        object_id = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
        if expected != (mode, object_id):
            dirty = True
    return {"revision": revision, "tree": tree, "dirty": dirty,
            "snapshot_sha256": state.hexdigest()}


def package_metadata(repo):
    # This project deliberately keeps its identity in literal Cargo package fields.
    text = (repo / "Cargo.toml").read_text()
    package = text.split("[package]", 1)[1].split("\n[", 1)[0]
    version = re.search(r'^version = "([^"]+)"$', package, re.M).group(1)
    if not VERSION.fullmatch(version):
        raise ValueError("invalid package version")
    lock = (repo / "Cargo.lock").read_text()
    if f'name = "twigsmith"\nversion = "{version}"' not in lock:
        raise ValueError("Cargo.toml and Cargo.lock package versions disagree")
    return version


def validate_elf(data):
    if len(data) < 64 or data[:6] != b"\x7fELF\x02\x01" or data[18:20] != b"\x3e\x00":
        raise ValueError("payload must be a little-endian ELF64 x86_64 executable")
    if int.from_bytes(data[16:18], "little") not in (2, 3):
        raise ValueError("payload ELF type must be executable or position-independent executable")


def write_bundle(output, version, payloads, provenance, epoch):
    """Assemble only validated regular payload bytes; publish after full verification."""
    stem = f"twigsmith-{version}-{TARGET}"
    archive = output / f"{stem}.tar.gz"
    files = dict(payloads)
    files["README.txt"] = GUIDANCE.format(version=version, stem=stem).encode()
    for name in ("twigsmith", "tgi"):
        validate_elf(files[name])
    # Tar/Gzip metadata is stable; compiler/linker byte reproducibility is separate.
    with archive.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch) as zipped:
            with tarfile.open(fileobj=zipped, mode="w", format=tarfile.USTAR_FORMAT) as tar:
                for name, mode in MEMBERS.items():
                    entry = tarfile.TarInfo(f"{stem}/{name}")
                    entry.size = len(files[name])
                    entry.mode = mode
                    entry.mtime = epoch
                    tar.addfile(entry, io.BytesIO(files[name]))
    manifest = {
        "schema": 1, "name": "twigsmith", "version": version, "target": TARGET,
        "archive": {"name": archive.name, "sha256": digest(archive.read_bytes())},
        "files": {name: {"sha256": digest(data), "size": len(data),
                         "mode": format(MEMBERS[name], "04o")} for name, data in files.items()},
        "npm_payload": "twigsmith", "provenance": provenance,
    }
    manifest_path = output / f"{stem}.manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    (output / f"{stem}.sha256").write_text(
        f"{digest(archive.read_bytes())}  {archive.name}\n"
        f"{digest(manifest_path.read_bytes())}  {manifest_path.name}\n"
    )
    verify(archive)
    return archive


def verify(archive, extract_to=None):
    archive = Path(archive).absolute()
    if not archive.name.endswith(".tar.gz") or archive.is_symlink():
        raise ValueError("expected a regular versioned .tar.gz archive")
    stem = archive.name[:-7]
    manifest_path = archive.with_name(stem + ".manifest.json")
    checksum_path = archive.with_name(stem + ".sha256")
    for path in (archive, manifest_path, checksum_path):
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"missing regular package file: {path}")
    archive_data = archive.read_bytes()
    manifest_data = manifest_path.read_bytes()
    expected = (f"{digest(archive_data)}  {archive.name}\n"
                f"{digest(manifest_data)}  {manifest_path.name}\n")
    if checksum_path.read_text() != expected:
        raise ValueError("archive/manifest checksums disagree")
    manifest = json.loads(manifest_data)
    version = manifest["version"]
    if not isinstance(version, str) or not VERSION.fullmatch(version):
        raise ValueError("invalid manifest version")
    if (manifest.get("schema") != 1 or manifest.get("name") != "twigsmith"
            or manifest.get("target") != TARGET
            or stem != f"twigsmith-{version}-{TARGET}"
            or manifest.get("archive") != {"name": archive.name, "sha256": digest(archive_data)}
            or manifest.get("npm_payload") != "twigsmith"
            or set(manifest.get("files", {})) != set(MEMBERS)):
        raise ValueError("inconsistent package name, version, target or manifest entries")
    provenance = manifest["provenance"]
    source = provenance["source"]
    if (not re.fullmatch(r"[a-f0-9]{40}", source["revision"])
            or not re.fullmatch(r"[a-f0-9]{40}", source["tree"])
            or not re.fullmatch(r"[a-f0-9]{64}", source["snapshot_sha256"])
            or not isinstance(source["dirty"], bool)
            or provenance["build"]["target"] != TARGET
            or provenance["build"]["profile"] != "release"
            or not provenance["toolchain"]["rustc"]
            or not provenance["toolchain"]["cargo"]
            or not provenance["environment"]["system"]):
        raise ValueError("incomplete source/build provenance")
    contents = {}
    with tarfile.open(fileobj=io.BytesIO(archive_data), mode="r:gz") as tar:
        entries = tar.getmembers()
        if len(entries) != len(MEMBERS):
            raise ValueError("unexpected or duplicate archive members")
        for entry in entries:
            name = entry.name.removeprefix(stem + "/")
            if (entry.name != f"{stem}/{name}" or name not in MEMBERS
                    or name in contents or not entry.isfile() or entry.mode != MEMBERS[name]
                    or entry.uid != 0 or entry.gid != 0 or entry.linkname
                    or entry.pax_headers):
                raise ValueError(f"invalid archive entry: {entry.name}")
            data = tar.extractfile(entry).read()
            if manifest["files"][name] != {"sha256": digest(data), "size": len(data),
                                           "mode": format(MEMBERS[name], "04o")}:
                raise ValueError(f"payload hash, size or mode mismatch: {name}")
            contents[name] = data
    for name in ("twigsmith", "tgi"):
        validate_elf(contents[name])
    if (contents["README.txt"] != GUIDANCE.format(version=version, stem=stem).encode()
            or b"MIT License" not in contents["LICENSE"]):
        raise ValueError("missing license or package guidance")
    if extract_to is not None:
        destination = Path(extract_to).absolute()
        if destination.exists() or destination.is_symlink():
            raise ValueError("extraction destination must not exist")
        destination.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix=".native-extract-", dir=destination.parent) as temp:
            staged = Path(temp) / "extracted"
            root = staged / stem
            root.mkdir(parents=True)
            for name, data in contents.items():
                path = root / name
                path.write_bytes(data)
                path.chmod(MEMBERS[name])
                if stat.S_IMODE(path.stat().st_mode) != MEMBERS[name]:
                    raise ValueError(f"failed to preserve extracted mode: {name}")
            staged.rename(destination)
    return manifest


def build(repo, output, allow_dirty, target):
    if target != TARGET or platform.system() != "Linux" or platform.machine() != "x86_64":
        raise ValueError(f"native packaging requires Linux x86_64 and target {TARGET}")
    output = Path(output).absolute()
    if output.exists() or output.is_symlink():
        raise ValueError("package output directory must not exist")
    source = identity(repo)
    if source["dirty"] and not allow_dirty:
        raise ValueError("source is dirty; commit it or use --allow-dirty for a labelled probe")
    version = package_metadata(repo)
    for name, value in os.environ.items():
        if value and (name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_BOOTSTRAP", "CC", "CFLAGS", "LDFLAGS")
                      or name.startswith("CARGO_PROFILE_RELEASE_")
                      or (name.startswith("CARGO_TARGET_") and name != "CARGO_TARGET_DIR")
                      or (name.startswith("CARGO_") and name.endswith("RUSTFLAGS"))):
            raise ValueError(f"unsupported build override: {name}")
    tools = repo / ".dev/cargo/bin"
    rustc = run([str(tools / "rustc"), "-vV"], repo)
    cargo = run([str(tools / "cargo"), "--version"], repo)
    pin = re.search(r'^channel = "([^"]+)"$', (repo / "rust-toolchain.toml").read_text(), re.M).group(1)
    if not rustc.startswith(f"rustc {pin} ") or f"host: {TARGET}" not in rustc:
        raise ValueError("compiler differs from the pinned native toolchain")
    epoch = int(git_output(repo, "show", "-s", "--format=%ct", source["revision"]))
    output.parent.mkdir(parents=True, exist_ok=True)
    scratch = repo / ".dev/tmp/distribution-phase-2"
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="native-build-", dir=scratch) as temporary:
        target_dir = Path(temporary) / "target"
        environment = os.environ.copy()
        environment.update(CARGO_TARGET_DIR=str(target_dir), CARGO_BUILD_BUILD_DIR=str(target_dir))
        arguments = [str(tools / "cargo"), "build", "--locked", "--release", "--target", TARGET,
                     "--bin", "twigsmith", "--bin", "tgi"]
        subprocess.run(arguments, cwd=repo, env=environment, check=True)
        payloads = {"LICENSE": (repo / "LICENSE").read_bytes()}
        for name in ("twigsmith", "tgi"):
            path = target_dir / TARGET / "release" / name
            if path.is_symlink() or not path.is_file() or not os.access(path, os.X_OK):
                raise ValueError(f"missing regular executable: {name}")
            result = subprocess.run([str(path), "--version"], capture_output=True, text=True, check=True)
            if result.stdout.strip() != f"twigsmith {version}" or result.stderr:
                raise ValueError(f"unexpected {name} --version output: {result!r}")
            payloads[name] = path.read_bytes()
        if source != identity(repo):
            raise ValueError("source changed during packaging; no candidate published")
        provenance = {
            "source": source,
            "toolchain": {"pin": pin, "rustc": rustc, "cargo": cargo},
            "build": {"target": TARGET, "profile": "release", "locked": True,
                      "arguments": arguments[1:], "rustflags": "", "source_date_epoch": epoch,
                      "compiler": run(["cc", "--version"]).splitlines()[0]},
            "environment": {"system": platform.platform(), "kernel": platform.release(),
                            "libc": platform.libc_ver(), "os_release": Path("/etc/os-release").read_text(),
                            "ci": {name: os.environ[name] for name in
                                   ("GITHUB_SHA", "GITHUB_REPOSITORY", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT",
                                    "ImageOS", "ImageVersion", "RUNNER_OS", "RUNNER_ARCH") if name in os.environ}},
        }
        with tempfile.TemporaryDirectory(prefix=".native-package-", dir=output.parent) as temp:
            staged = Path(temp) / "candidate"
            staged.mkdir()
            archive = write_bundle(staged, version, payloads, provenance, epoch)
            staged.rename(output)
    print(output / archive.name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    package = commands.add_parser("build")
    package.add_argument("output")
    package.add_argument("--target", default=TARGET)
    package.add_argument("--allow-dirty", action="store_true")
    check = commands.add_parser("verify")
    check.add_argument("archive")
    check.add_argument("--extract-to")
    args = parser.parse_args()
    try:
        if args.command == "build":
            build(Path(__file__).resolve().parents[1], args.output, args.allow_dirty, args.target)
        else:
            manifest = verify(args.archive, args.extract_to)
            print(f"verified {manifest['archive']['name']}: source {manifest['provenance']['source']['revision']}"
                  + (" (dirty probe)" if manifest['provenance']['source']['dirty'] else ""))
    except (ValueError, KeyError, IndexError, TypeError, AttributeError, OSError,
            tarfile.TarError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
