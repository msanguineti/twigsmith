#!/usr/bin/env python3
"""Build and verify the finite native candidate package formats (Python 3.10+)."""
import argparse
from copy import deepcopy
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
import shutil
import zipfile

from native_formats import inspect_pe, inspect_macho

TARGET = "x86_64-unknown-linux-gnu"
TARGETS = {
    TARGET: {"os": "linux", "arch": "x86_64", "libc": "glibc", "extension": ".tar.gz", "executables": ("twigsmith", "tgi")},
    "x86_64-pc-windows-msvc": {"os": "windows", "arch": "x86_64", "libc": None, "extension": ".zip", "executables": ("twigsmith.exe", "tgi.exe")},
    "aarch64-apple-darwin": {"os": "macos", "arch": "aarch64", "libc": None, "extension": ".tar.gz", "executables": ("twigsmith", "tgi")},
    "x86_64-apple-darwin": {"os": "macos", "arch": "x86_64", "libc": None, "extension": ".tar.gz", "executables": ("twigsmith", "tgi")},
}


def target_metadata(target):
    try:
        return deepcopy(TARGETS[target])
    except KeyError as error:
        raise ValueError(f"unsupported native target: {target}") from error


def executable_names(target):
    return target_metadata(target)["executables"]


def archive_stem(path):
    name = Path(path).name
    for extension in (".tar.gz", ".zip"):
        if name.endswith(extension):
            return name[:-len(extension)]
    raise ValueError("expected a regular versioned .tar.gz or .zip archive")


def members_for(target):
    return dict.fromkeys(executable_names(target), 0o755) | {"LICENSE": 0o644, "README.txt": 0o644}


def inspect_payload(data, target):
    metadata = target_metadata(target)
    if metadata["os"] == "linux":
        validate_elf(data)
        return {"format": "ELF64", "arch": "x86_64"}
    if metadata["os"] == "windows":
        return inspect_pe(data)
    return inspect_macho(data, metadata["arch"])


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
The manifest labels CI candidates and local development probes and records the
checkout, toolchain and available CI run. It does not attest arbitrary local
source bytes or establish reproducible compiler output.
See the repository docs/USAGE.md for configuration and recovery guidance.
"""


def digest(data):
    return hashlib.sha256(data).hexdigest()


def run(args, cwd=None, env=None):
    return subprocess.check_output(args, cwd=cwd, env=env, text=True, encoding="utf-8").strip()


def git_output(repo, *arguments):
    return run(["git", *arguments], repo)


def identity(repo):
    # CI supplies a controlled checkout. Ordinary Git metadata records that
    # source; local status is useful context, not filesystem attestation.
    return {"revision": git_output(repo, "rev-parse", "HEAD"),
            "tree": git_output(repo, "rev-parse", "HEAD^{tree}"),
            "dirty": bool(git_output(repo, "status", "--porcelain", "--untracked-files=all"))}


def provenance_kind(source):
    if os.environ.get("GITHUB_ACTIONS") != "true":
        return "development-probe"
    if source["dirty"]:
        raise ValueError("CI candidate requires a clean checkout")
    if source["revision"] != os.environ.get("GITHUB_SHA"):
        raise ValueError("CI candidate HEAD differs from selected GITHUB_SHA")
    if not all(os.environ.get(name) for name in
               ("GITHUB_REPOSITORY", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")):
        raise ValueError("CI candidate requires repository and run identity")
    return "ci-candidate"


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


def validate_requirements(requirements, *, native_builder=False):
    """Validate the frozen supported-runtime data independently of build provenance."""
    def text(value):
        return isinstance(value, str) and bool(value.strip()) and not any(ord(c) < 32 for c in value)
    def platform_data(value):
        return (isinstance(value, dict) and set(value) == {"os", "arch", "libc"}
                and text(value["os"]) and text(value["arch"])
                and (value["libc"] is None or text(value["libc"])))
    try:
        if (not isinstance(requirements, dict) or set(requirements) != {"schema", "native", "npm"}
                or type(requirements["schema"]) is not int or requirements["schema"] != 1):
            raise ValueError("invalid requirements schema")
        native, npm = requirements["native"], requirements["npm"]
        if (set(native) != {"platform", "requirements"}
                or not platform_data(native["platform"])
                or not isinstance(native["requirements"], dict) or not native["requirements"]
                or not all(text(k) and text(v) for k, v in native["requirements"].items())
                or set(npm) != {"coverage", "requirements"} or not text(npm["requirements"])
                or not isinstance(npm["coverage"], list) or not npm["coverage"]
                or not all(platform_data(v) for v in npm["coverage"])):
            raise ValueError("invalid supported requirements")
        if native_builder and (native["platform"] != {"os": "linux", "arch": "x86_64", "libc": "glibc"}
                               or set(native["requirements"]) != {"system", "shell"}):
            raise ValueError("requirements do not match the native builder")
    except (KeyError, TypeError) as error:
        raise ValueError("invalid supported requirements") from error
    return deepcopy(requirements)


def load_requirements(repo, target=TARGET):
    current = validate_requirements(json.loads((repo / "site/current-requirements.json").read_text()), native_builder=True)
    if target == TARGET:
        return current
    metadata = target_metadata(target)
    # The public requirements remain Linux-only until qualification and approval.
    current["native"] = {"platform": {key: metadata[key] for key in ("os", "arch", "libc")},
                         "requirements": {"system": "Candidate platform; OS support floor is unqualified",
                                          "shell": "Git 2.45 or newer; native execution needs no Node/npm or Rust"}}
    return current


def validate_target_requirements(requirements, target):
    result = validate_requirements(requirements, native_builder=target == TARGET)
    metadata = target_metadata(target)
    if result["native"]["platform"] != {key: metadata[key] for key in ("os", "arch", "libc")}:
        raise ValueError("requirements do not match native target")
    return result


def requirements_match_descriptor(manifest, descriptor):
    """Compare a release descriptor with the requirements frozen in its manifest."""
    try:
        snapshot = validate_requirements(manifest["requirements"])
        native = [entry for entry in descriptor["native"] if entry["platform"] == snapshot["native"]["platform"]]
        return (manifest["version"] == descriptor["version"] and len(native) == 1
                and native[0]["requirements"] == snapshot["native"]["requirements"]
                and descriptor["npm"]["requirements"] == snapshot["npm"]["requirements"]
                and descriptor["npm"]["coverage"] == snapshot["npm"]["coverage"])
    except (KeyError, TypeError, ValueError):
        return False


def guidance(version, stem, requirements):
    native = requirements["native"]["requirements"]
    return f"""Twigsmith {version} — Linux x86_64 GNU native candidate

Extract this archive with tar -xzf, then add the extracted directory to PATH:
  export PATH="/absolute/path/to/{stem}:$PATH"
Run twigsmith --help or tgi --help; both names use the same native implementation.

Requirements: {native['system']}. {native['shell']}.
Rust, Node and npm are not required for the native archive. Editors,
signing/transport helpers and project checks are needed only when selected.
Qualification evidence applies to the observed environments and exact bytes;
it does not establish every distribution, shared-library or kernel boundary.
Optional completion shells have separate compatibility requirements.

Verify the adjacent .sha256 and .manifest.json with the repository's
./dev verify-native command before using a candidate. SHA-256 establishes
consistency with those files; it does not independently authenticate a publisher.
The manifest labels CI candidates and local development probes and records the
checkout, toolchain and available CI run. It does not attest arbitrary local
source bytes or establish reproducible compiler output.
See the repository docs/USAGE.md for configuration and recovery guidance.
"""


def candidate_guidance(version, stem, requirements, target):
    if target == TARGET:
        return guidance(version, stem, requirements)
    metadata = target_metadata(target)
    if metadata["os"] == "windows":
        install = f'''Extract with PowerShell:
  Expand-Archive -LiteralPath "{stem}.zip" -DestinationPath .
Add the extracted directory to this session's PATH:
  $env:PATH = "$PWD\\{stem};" + $env:PATH
Run twigsmith.exe --help or tgi.exe --help.
Windows downloads are unsigned. Browser/SmartScreen policy depends on the host;
review the source and checksums and use your organization's approved execution path.'''
    else:
        install = f'''Extract with tar -xzf "{stem}.tar.gz", then add the directory to PATH:
  export PATH="/absolute/path/to/{stem}:$PATH"
Run twigsmith --help or tgi --help.
macOS downloads are unsigned and not notarized. Gatekeeper/quarantine policy
depends on download method and host; use your approved local execution procedure.'''
    return f'''Twigsmith {version} — {target} native candidate

{install}
Both names use the same native implementation. Git 2.45 or newer is required.
Rust, Node and npm are not required for native execution. Git's configured shell,
editors, signing/transport helpers and checks are needed when selected.
This candidate establishes no Windows/macOS minimum OS support claim.
Dependency/deployment metadata is inspection evidence, not native runtime qualification.
Verify the adjacent .sha256 and .manifest.json with tools/native_package.py verify.
SHA-256 checks consistency; it does not independently authenticate a publisher.
See docs/USAGE.md for configuration and recovery guidance.
'''


def write_bundle(output, version, payloads, provenance, epoch, requirements=None, target=TARGET):
    """Assemble only validated regular payload bytes; publish after full verification."""
    requirements = validate_target_requirements(requirements, target) if requirements is not None else load_requirements(Path(__file__).resolve().parents[1], target)
    metadata = target_metadata(target)
    members = members_for(target)
    stem = f"twigsmith-{version}-{target}"
    archive = output / f"{stem}{metadata['extension']}"
    files = dict(payloads)
    files["README.txt"] = candidate_guidance(version, stem, requirements, target).encode()
    if set(files) != set(members):
        raise ValueError("unexpected or missing payload files")
    inspection = {name: inspect_payload(files[name], target) for name in executable_names(target)}
    # Stable metadata describes packaging, not compiler output reproducibility.
    if metadata["extension"] == ".zip":
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as zipped:
            for name, data in files.items():
                entry = zipfile.ZipInfo(f"{stem}/{name}", (1980, 1, 1, 0, 0, 0))
                entry.create_system = 3
                entry.external_attr = (stat.S_IFREG | members[name]) << 16
                zipped.writestr(entry, data, compress_type=zipfile.ZIP_DEFLATED)
    else:
        with archive.open("wb") as raw:
            with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch) as zipped:
                with tarfile.open(fileobj=zipped, mode="w", format=tarfile.USTAR_FORMAT) as tar:
                    for name, mode in members.items():
                        entry = tarfile.TarInfo(f"{stem}/{name}")
                        entry.size = len(files[name])
                        entry.mode = mode
                        entry.mtime = epoch
                        tar.addfile(entry, io.BytesIO(files[name]))
    manifest = {
        "schema": 3 if target == TARGET else 4, "name": "twigsmith", "version": version, "target": target,
        "requirements": requirements,
        "archive": {"name": archive.name, "sha256": digest(archive.read_bytes())},
        "files": {name: {"sha256": digest(data), "size": len(data),
                         "mode": format(members[name], "04o")} for name, data in files.items()},
        "npm_payload": executable_names(target)[0], "provenance": provenance,
    }
    if target != TARGET:
        manifest["inspection"] = inspection
    manifest_path = output / f"{stem}.manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    (output / f"{stem}.sha256").write_text(
        f"{digest(archive.read_bytes())}  {archive.name}\n"
        f"{digest(manifest_path.read_bytes())}  {manifest_path.name}\n", encoding="utf-8"
    )
    verify(archive)
    return archive


def verify(archive, extract_to=None):
    archive = Path(archive).absolute()
    stem = archive_stem(archive)
    manifest_path = archive.with_name(stem + ".manifest.json")
    checksum_path = archive.with_name(stem + ".sha256")
    for path in (archive, manifest_path, checksum_path):
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"missing regular package file: {path}")
    archive_data = archive.read_bytes()
    manifest_data = manifest_path.read_bytes()
    expected = (f"{digest(archive_data)}  {archive.name}\n"
                f"{digest(manifest_data)}  {manifest_path.name}\n")
    if checksum_path.read_text(encoding="utf-8") != expected:
        raise ValueError("archive/manifest checksums disagree")
    manifest = json.loads(manifest_data)
    version = manifest["version"]
    target = manifest.get("target")
    metadata = target_metadata(target)
    members = members_for(target)
    if not isinstance(version, str) or not VERSION.fullmatch(version):
        raise ValueError("invalid manifest version")
    if (manifest.get("schema") not in ((2, 3) if target == TARGET else (4,)) or manifest.get("name") != "twigsmith"
            or archive.name != f"twigsmith-{version}-{target}{metadata['extension']}"
            or manifest.get("archive") != {"name": archive.name, "sha256": digest(archive_data)}
            or manifest.get("npm_payload") != executable_names(target)[0]
            or set(manifest.get("files", {})) != set(members)):
        raise ValueError("inconsistent package name, version, target or manifest entries")
    if manifest["schema"] >= 3:
        validate_target_requirements(manifest["requirements"], target)
    provenance = manifest["provenance"]
    source = provenance["source"]
    if (not re.fullmatch(r"[a-f0-9]{40}", source["revision"])
            or not re.fullmatch(r"[a-f0-9]{40}", source["tree"])
            or not isinstance(source["dirty"], bool)
            or provenance["kind"] not in ("ci-candidate", "development-probe")
            or provenance["build"]["target"] != target
            or provenance["build"]["profile"] != "release"
            or not provenance["toolchain"]["rustc"]
            or not provenance["toolchain"]["cargo"]
            or not provenance["environment"]["system"]):
        raise ValueError("incomplete source/build provenance")
    if provenance["kind"] == "ci-candidate":
        ci = provenance["environment"]["ci"]
        if (source["dirty"] or ci.get("GITHUB_SHA") != source["revision"]
                or not all(ci.get(name) for name in
                           ("GITHUB_REPOSITORY", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"))):
            raise ValueError("incomplete CI candidate source/run provenance")
    contents = {}
    def add(name, data):
        if manifest["files"][name] != {"sha256": digest(data), "size": len(data),
                                       "mode": format(members[name], "04o")}:
            raise ValueError(f"payload hash, size or mode mismatch: {name}")
        contents[name] = data
    if metadata["extension"] == ".zip":
        with zipfile.ZipFile(io.BytesIO(archive_data)) as zipped:
            entries = zipped.infolist()
            if len(entries) != len(members):
                raise ValueError("unexpected or duplicate archive members")
            for entry in entries:
                name = entry.filename.removeprefix(stem + "/")
                mode = entry.external_attr >> 16
                # ZIP metadata must explicitly identify regular files. Windows
                # does not have Unix executable mode semantics at extraction.
                if (name not in members or entry.filename != f"{stem}/{name}"
                        or name in contents or entry.orig_filename != entry.filename
                        or entry.create_system != 3 or not stat.S_ISREG(mode)
                        or entry.external_attr & 0xffff or entry.extra or entry.comment
                        or entry.flag_bits & 1 or entry.is_dir()):
                    raise ValueError(f"invalid archive entry: {entry.filename}")
                add(name, zipped.read(entry))
    else:
        with tarfile.open(fileobj=io.BytesIO(archive_data), mode="r:gz") as tar:
            entries = tar.getmembers()
            if len(entries) != len(members):
                raise ValueError("unexpected or duplicate archive members")
            for entry in entries:
                name = entry.name.removeprefix(stem + "/")
                if (entry.name != f"{stem}/{name}" or name not in members
                        or name in contents or not entry.isfile() or entry.mode != members[name]
                        or entry.uid != 0 or entry.gid != 0 or entry.linkname
                        or entry.pax_headers):
                    raise ValueError(f"invalid archive entry: {entry.name}")
                add(name, tar.extractfile(entry).read())
    inspection = {name: inspect_payload(contents[name], target) for name in executable_names(target)}
    if target != TARGET:
        if manifest.get("inspection") != inspection:
            raise ValueError("executable inspection differs from manifest")
        if (provenance["toolchain"].get("pin") is None
                or f"host: {target}" not in provenance["toolchain"]["rustc"]
                or provenance["build"].get("locked") is not True
                or not provenance["build"].get("compiler")
                or provenance["environment"].get("machine") != metadata["arch"]):
            raise ValueError("incomplete platform toolchain/build provenance")
    expected_guidance = (GUIDANCE.format(version=version, stem=stem) if manifest["schema"] == 2
                         else candidate_guidance(version, stem, manifest["requirements"], target))
    if (contents["README.txt"] != expected_guidance.encode()
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
                path.chmod(members[name])
                if os.name != "nt" and stat.S_IMODE(path.stat().st_mode) != members[name]:
                    raise ValueError(f"failed to preserve extracted mode: {name}")
            staged.rename(destination)
    return manifest


def compiler_identity(metadata):
    if metadata["os"] != "windows":
        return run(["cc", "--version"]).splitlines()[0]
    # Rust locates MSVC through Visual Studio discovery even when link.exe is
    # absent from PATH. Record installed/default toolset evidence separately
    # from a claim about the linker selected internally by rustc.
    vswhere = Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Microsoft Visual Studio/Installer/vswhere.exe"
    installations = json.loads(run([str(vswhere), "-latest", "-products", "*", "-requires",
                                    "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-format", "json", "-utf8"]))
    if len(installations) != 1:
        raise ValueError("could not identify installed MSVC build tools")
    installation = installations[0]
    version_path = Path(installation["installationPath"]) / "VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt"
    return {"installed_visual_studio": installation["installationVersion"],
            "default_msvc_toolset": version_path.read_text(encoding="utf-8").strip(),
            "selected_environment": {key: os.environ[key] for key in
                                     ("VCINSTALLDIR", "VCToolsInstallDir", "WindowsSdkDir", "WindowsSDKVersion")
                                     if key in os.environ}}


def build(repo, output, allow_dirty, target):
    metadata = target_metadata(target)
    systems = {"linux": "Linux", "windows": "Windows", "macos": "Darwin"}
    machines = {"x86_64": ("x86_64", "amd64"), "aarch64": ("arm64", "aarch64")}
    if platform.system() != systems[metadata["os"]] or platform.machine().lower() not in machines[metadata["arch"]]:
        raise ValueError(f"native packaging requires matching host for target {target}")
    if metadata["os"] == "macos":
        translated = subprocess.run(["sysctl", "-in", "sysctl.proc_translated"],
                                    capture_output=True, text=True, check=False)
        if translated.stdout.strip() == "1":
            raise ValueError("native packaging requires a host without Rosetta translation")
    output = Path(output).absolute()
    if output.exists() or output.is_symlink():
        raise ValueError("package output directory must not exist")
    source = identity(repo)
    if source["dirty"] and not allow_dirty:
        raise ValueError("source is dirty; commit it or use --allow-dirty for a labelled probe")
    kind = provenance_kind(source)
    version = package_metadata(repo)
    requirements = load_requirements(repo, target)
    for name, value in os.environ.items():
        if value and (name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_BOOTSTRAP",
                                    "CC", "CXX", "CFLAGS", "CXXFLAGS", "AR", "ARFLAGS", "LDFLAGS",
                                    "SDKROOT", "MACOSX_DEPLOYMENT_TARGET")
                      or name.startswith(("CC_", "CXX_", "CFLAGS_", "CXXFLAGS_", "AR_", "ARFLAGS_", "LDFLAGS_"))
                      or (name.startswith(("HOST_", "TARGET_")) and name.split("_", 1)[1] in
                          ("CC", "CXX", "CFLAGS", "CXXFLAGS", "AR", "ARFLAGS", "LDFLAGS"))
                      or name.startswith("CARGO_PROFILE_RELEASE_")
                      or (name.startswith("CARGO_TARGET_") and name != "CARGO_TARGET_DIR")
                      or (name.startswith("CARGO_") and name.endswith("RUSTFLAGS"))):
            raise ValueError(f"unsupported build override: {name}")
    tools = repo / ".dev/cargo/bin"
    suffix = ".exe" if metadata["os"] == "windows" else ""
    rustc_path = tools / ("rustc" + suffix)
    cargo_path = tools / ("cargo" + suffix)
    if not rustc_path.is_file() or not cargo_path.is_file():
        rustc_path = shutil.which("rustc") or str(rustc_path)
        cargo_path = shutil.which("cargo") or str(cargo_path)
    # Keep rustup shim names intact: resolving their symlink to rustup would
    # change argv[0] dispatch. Inspect and invoke the same absolute command.
    rustc_path = Path(rustc_path).absolute()
    cargo_path = Path(cargo_path).absolute()
    rustc = run([str(rustc_path), "-vV"], repo)
    cargo = run([str(cargo_path), "--version"], repo)
    pin = re.search(r'^channel = "([^"]+)"$', (repo / "rust-toolchain.toml").read_text(), re.M).group(1)
    if not rustc.startswith(f"rustc {pin} ") or f"host: {target}" not in rustc:
        raise ValueError("compiler differs from the pinned native toolchain")
    epoch = int(git_output(repo, "show", "-s", "--format=%ct", source["revision"]))
    output.parent.mkdir(parents=True, exist_ok=True)
    scratch = repo / ".dev/tmp/native-build"
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="native-build-", dir=scratch) as temporary:
        target_dir = Path(temporary) / "target"
        environment = os.environ.copy()
        controlled_environment = {
            "RUSTC": str(rustc_path), "CARGO_BUILD_RUSTC": str(rustc_path),
            "RUSTC_WRAPPER": "", "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER": "",
        }
        # Cargo's documented empty wrapper values override personal config too.
        # Direct Python invocation must have the same compiler boundary as ./dev.
        environment.update(controlled_environment,
                           CARGO_TARGET_DIR=str(target_dir), CARGO_BUILD_BUILD_DIR=str(target_dir))
        arguments = [str(cargo_path), "build", "--locked", "--release", "--target", target,
                     "--bin", "twigsmith", "--bin", "tgi"]
        subprocess.run(arguments, cwd=repo, env=environment, check=True)
        payloads = {"LICENSE": (repo / "LICENSE").read_bytes()}
        for name in executable_names(target):
            path = target_dir / target / "release" / name
            if path.is_symlink() or not path.is_file() or not os.access(path, os.X_OK):
                raise ValueError(f"missing regular executable: {name}")
            result = subprocess.run([str(path), "--version"], capture_output=True, text=True, check=True)
            if result.stdout.strip() != f"twigsmith {version}" or result.stderr:
                raise ValueError(f"unexpected {name} --version output: {result!r}")
            payloads[name] = path.read_bytes()
        if requirements != load_requirements(repo, target):
            raise ValueError("requirements changed during packaging; no candidate published")
        if source != identity(repo):
            raise ValueError("source changed during packaging; no candidate published")
        provenance = {
            "kind": kind, "source": source,
            "toolchain": {"pin": pin, "rustc": rustc, "cargo": cargo},
            "build": {"target": target, "profile": "release", "locked": True,
                      "arguments": arguments[1:], "rustflags": "", "source_date_epoch": epoch,
                      "controlled_environment": controlled_environment,
                      "compiler": compiler_identity(metadata)},
            "environment": {"system": platform.platform(), "kernel": platform.release(),
                            "machine": metadata["arch"],
                            "libc": platform.libc_ver(), "os_release": Path("/etc/os-release").read_text() if metadata["os"] == "linux" else None,
                            "ci": {name: os.environ[name] for name in
                                   ("GITHUB_SHA", "GITHUB_REPOSITORY", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT",
                                    "ImageOS", "ImageVersion", "RUNNER_OS", "RUNNER_ARCH") if name in os.environ}},
        }
        with tempfile.TemporaryDirectory(prefix=".native-package-", dir=output.parent) as temp:
            staged = Path(temp) / "candidate"
            staged.mkdir()
            archive = write_bundle(staged, version, payloads, provenance, epoch, requirements, target=target)
            staged.rename(output)
    print(output / archive.name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    package = commands.add_parser("build")
    package.add_argument("output")
    package.add_argument("--target", default=TARGET)
    package.add_argument("--allow-dirty", action="store_true",
                         help="allow uncommitted source for a local development probe")
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
                  + f" ({manifest['provenance']['kind']})")
    except (ValueError, KeyError, IndexError, TypeError, AttributeError, OSError,
            tarfile.TarError, zipfile.BadZipFile, UnicodeError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
