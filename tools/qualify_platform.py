#!/usr/bin/env python3
"""Qualify exact native/npm candidate bytes on their actual native host (Python 3.10+)."""

import argparse
import contextlib
import hashlib
import http.server
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
from urllib.parse import unquote

import native_package
from qualify_native import UTILITIES, digest, write_json


# These fixtures launch real CLI processes and independently observe Git state.
# Keep library-only managed-process tests out of artifact acceptance counts.
CASES = {
    "native_journey": [
        "native_start_status_journey",
        "native_selected_save_and_ordered_review_journey",
        "native_direct_candidate_failure_and_retry_journey",
        "bundles::generated_github_and_trunk_settings_drive_split_remote_journeys",
        "bundles::guided_gitflow_repository_correction_updates_roles_and_preserves_custom_route",
    ],
    "cli_oneflow": [
        "configured_direct_release_uses_selected_cutoff_and_publishes_version",
        "hotfix_explicit_tag_routes_to_declared_release_or_development",
        "retry_rejects_changed_settings_and_same_commit_tag_object_substitution",
    ],
    "cli_direct_sequence": [
        "bundles::gitflow_generated_feature_release_and_no_active_hotfix_deliver_expected_refs",
        "bundles::gitflow_active_release_later_conflict_retry_preserves_production_tag_and_develop",
    ],
    "native_checks": ["native_cli_interrupt_contains_tree_and_preserves_retry"],
    "native_check_guards": ["native_skip_and_streamed_redaction_preserve_check_policy"],
    "native_editor": ["native_git_editor_accept_and_cancel_retry", "native_git_editor_failures_and_bypass"],
    "native_prompts": [
        "native_keyboard_cancellations_preserve_config_and_lifecycle_work",
        "native_save_and_finish_submit_real_choices_and_publish_literal_ref",
        "native_bundle_selection_gitflow_roles_and_creation",
    ],
}
# Alternate Node lines exercise the launcher/terminal boundary while the full
# profile retains all real-Git workflow cases on the reference Node line.
LAUNCHER_CASES = {
    "native_journey": ["native_start_status_journey"],
    "native_checks": ["native_cli_interrupt_contains_tree_and_preserves_retry"],
    "native_editor": ["native_git_editor_accept_and_cancel_retry"],
}


def npm_cases(profile):
    if profile not in ("full", "launcher"):
        raise ValueError(f"unknown npm qualification profile: {profile}")
    selected = CASES if profile == "full" else LAUNCHER_CASES
    return {suite: list(names) for suite, names in selected.items()}


EXAMPLES = ["native_check_fixture", "native_editor_fixture", "native_npm_fixture"]
GAPS = [
    "Finite selected real-Git fixtures; not the complete source suite or every shell/provider.",
    "Observed native host only; not minimum desktop OS, browser-download security or signing qualification.",
]


def fixture_output(command, log, env, cwd):
    """Capture UTF-8 fixture diagnostics independently of the host ANSI locale."""
    try:
        output = subprocess.run(command, env=env, cwd=cwd, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, text=True, encoding="utf-8",
                                errors="replace", timeout=300)
    except subprocess.TimeoutExpired as error:
        partial = error.stdout or b""
        if isinstance(partial, bytes):
            partial = partial.decode("utf-8", errors="replace")
        log.write_text(partial + "\nfixture timed out after 300 seconds\n", encoding="utf-8")
        raise RuntimeError(f"fixture timed out: {command[0]}; see {log}") from error
    except OSError as error:
        log.write_text(f"fixture could not start: {error}\n", encoding="utf-8")
        raise
    log.write_text(output.stdout, encoding="utf-8")
    return output


def host_target():
    machine = platform.machine().lower()
    if sys.platform == "linux" and machine in ("x86_64", "amd64"):
        return "x86_64-unknown-linux-gnu"
    if sys.platform == "win32" and machine in ("x86_64", "amd64"):
        return "x86_64-pc-windows-msvc"
    if sys.platform == "darwin":
        if machine == "arm64":
            return "aarch64-apple-darwin"
        if machine == "x86_64":
            translated = subprocess.check_output(["sysctl", "-n", "sysctl.proc_translated"],
                                                  stderr=subprocess.DEVNULL, text=True).strip() \
                if subprocess.run(["sysctl", "-n", "sysctl.proc_translated"],
                                  capture_output=True).returncode == 0 else "0"
            if translated == "1":
                raise ValueError("Rosetta execution cannot qualify a native Intel host")
            return "x86_64-apple-darwin"
    raise ValueError(f"unsupported native qualification host: {sys.platform}/{machine}")


def checked_file(path):
    path = Path(path)
    if not path.is_absolute() or path.is_symlink() or not path.is_file():
        raise ValueError(f"expected absolute regular executable: {path}")
    if os.name != "nt" and not os.access(path, os.X_OK):
        raise ValueError(f"expected executable permissions: {path}")
    return path


def prepare(args):
    """Build harnesses once; the run/npm commands never compile product or tests."""
    repo = Path(__file__).resolve().parents[1]
    directory = args.harness_dir.resolve()
    if directory.exists():
        raise ValueError("harness directory must not exist")
    pin = re.search(r'^channel = "([^"]+)"$', (repo / "rust-toolchain.toml").read_text(), re.M).group(1)
    env = os.environ.copy()
    env.update(CARGO_HOME=str(repo / ".dev/cargo"), RUSTUP_HOME=str(repo / ".dev/rustup"),
               RUSTUP_TOOLCHAIN=pin)
    suffix = ".exe" if os.name == "nt" else ""
    cargo = Path(args.cargo).absolute() if args.cargo else repo / f".dev/cargo/bin/cargo{suffix}"
    if not cargo.is_file() and os.name == "nt" and not args.cargo:
        cargo = Path(shutil.which("cargo") or cargo)
    if not cargo.is_file():
        raise ValueError("pinned Cargo entry point was not found")
    rustc = cargo.with_name(f"rustc{suffix}")
    compiler = subprocess.check_output([str(rustc), "-vV"], cwd=repo, env=env, text=True).strip()
    env["RUSTC"] = str(rustc)
    env["CARGO_BUILD_RUSTC"] = str(rustc)
    for name in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"):
        env[name] = ""
    target = host_target()
    if not compiler.startswith(f"rustc {pin} ") or f"host: {target}" not in compiler.splitlines():
        raise ValueError("fixture compiler differs from pinned native toolchain")
    directory.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".platform-harness-", dir=directory.parent) as temporary:
        staged = Path(temporary) / "harnesses"
        (staged / "deps").mkdir(parents=True)
        (staged / "examples").mkdir()
        outputs = {}
        for operation, selections in [("test", sorted(CASES)), ("build", EXAMPLES)]:
            command = [str(cargo), operation, "--locked", "--target", target, "--message-format=json"]
            if operation == "test":
                command.append("--no-run")
            for name in selections:
                command += ["--test" if operation == "test" else "--example", name]
            build = subprocess.run(command, cwd=repo, env=env, stdout=subprocess.PIPE, text=True)
            if build.returncode:
                raise RuntimeError(f"native fixture {operation} failed: {build.stdout}")
            found = set()
            for line in build.stdout.splitlines():
                item = json.loads(line)
                name = item.get("target", {}).get("name")
                if item.get("reason") == "compiler-artifact" and item.get("executable") and name in selections:
                    relative = Path("deps" if operation == "test" else "examples") / f"{name}{suffix}"
                    shutil.copy2(item["executable"], staged / relative)
                    outputs[name] = {"path": relative.as_posix(), "sha256": digest(staged / relative)}
                    found.add(name)
            if found != set(selections):
                raise RuntimeError(f"missing compiled fixture executables: {set(selections) - found}")
        write_json(staged / "harnesses.json", {
            "schema": 1, "target": target, "toolchain": compiler,
            "source": native_package.identity(repo), "executables": outputs, "cases": CASES,
        })
        staged.rename(directory)
    print(f"Prepared {len(CASES)} native harnesses and {len(EXAMPLES)} helpers: {directory}")
    return 0


def harness_manifest(directory, target):
    manifest = json.loads((directory / "harnesses.json").read_text())
    if manifest.get("target") != target or manifest.get("cases") != CASES:
        raise ValueError("fixture harness target/case selection differs from this qualification")
    for name in [*CASES, *EXAMPLES]:
        item = manifest["executables"][name]
        relative = Path(item["path"])
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("unsafe fixture executable path")
        path = directory / relative
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"expected regular fixture executable: {name}")
        # Actions artifact transfer preserves bytes, but drops executable modes.
        if os.name != "nt":
            path.chmod(0o755)
        path = checked_file(path)
        if digest(path) != item["sha256"]:
            raise ValueError(f"fixture harness bytes changed: {name}")
    return manifest


def fixture_passed(output, name):
    # The outer harness can report one passed test even if isolated --exact ran
    # zero children. Require the opt-in marker emitted inside the real child.
    return (output.returncode == 0 and "1 passed; 0 failed" in output.stdout
            and f"TWIGSMITH_QUALIFICATION_CHILD {name}\n" in output.stdout)


def run_fixtures(artifact, harness, evidence, report, *, kind="native", env=None, selected_cases=None):
    selected = {suite: list(names) for suite, names in (CASES if selected_cases is None else selected_cases).items()}
    if (not selected or any(not names or suite not in CASES or len(names) != len(set(names))
                            or any(name not in CASES[suite] for name in names)
                            for suite, names in selected.items())):
        raise ValueError("invalid selected qualification fixtures")
    target = host_target()
    manifest = harness_manifest(harness, target)
    report.update(harnesses=manifest, selected_cases=selected, tests=[], passed=False)
    report.setdefault("profile", "full")
    env = dict(os.environ if env is None else env)
    env.update(TWIGSMITH_ARTIFACT_DIR=str(artifact), TWIGSMITH_ARTIFACT_KIND=kind,
               TWIGSMITH_QUALIFICATION_CHILD="1", LC_ALL="C", TZ="UTC")
    # Never let the driver accidentally enter a fixture's internal child path.
    env.pop("TWIGSMITH_ISOLATED_TEST", None)
    env.pop("TWIGSMITH_TEST_ROOT", None)
    with tempfile.TemporaryDirectory(prefix="twigsmith-platform-runtime-") as temporary:
        if kind == "native":
            env = native_environment(Path(temporary), env)
            report["runtime_path"] = env["PATH"]
        for suite, names in selected.items():
            executable = str(harness / manifest["executables"][suite]["path"])
            listing = fixture_output([executable, "--list"], evidence / f"{suite}--list.log", env, temporary)
            if listing.returncode:
                raise RuntimeError(f"fixture listing failed: {suite}")
            for name in names:
                if f"{name}: test\n" not in listing.stdout:
                    raise ValueError(f"selected fixture absent: {suite}::{name}")
                start = time.monotonic()
                output = fixture_output([executable, "--exact", name, "--include-ignored", "--nocapture"],
                    evidence / f"{suite}--{name.replace('::', '--')}.log", env, temporary)
                passed = fixture_passed(output, name)
                report["tests"].append({"suite": suite, "name": name, "passed": passed,
                    "exit": output.returncode, "seconds": round(time.monotonic() - start, 3)})
                write_json(evidence / "qualification.json", report)
                print(f"{'PASS' if passed else 'FAIL'} {suite}::{name}", flush=True)
    # Prepared fixture helpers are also part of the exact execution boundary.
    harness_manifest(harness, target)
    report["passed"] = bool(report["tests"]) and all(case["passed"] for case in report["tests"])
    return report["passed"]


def native_environment(temporary, env):
    """Keep build and npm runtimes off the native product's runtime PATH."""
    env = dict(env)
    git = checked_file(Path(shutil.which("git", path=env.get("PATH")) or "").absolute())
    env["GIT_EXEC_PATH"] = subprocess.check_output([str(git), "--exec-path"], text=True).strip()
    if os.name == "nt":
        directories = [git.parent, Path(env.get("SystemRoot") or env["SYSTEMROOT"]) / "System32"]
        env["PATH"] = os.pathsep.join(map(str, directories))
    else:
        runtime = temporary / "runtime-bin"
        runtime.mkdir()
        for name in [*UTILITIES, "ps", "kill"]:
            source = shutil.which(name, path=env.get("PATH"))
            if source:
                (runtime / name).symlink_to(Path(source).resolve())
        (runtime / "git").symlink_to(git)
        env["PATH"] = str(runtime)
    for name in ("cargo", "rustc", "node", "npm", "npx"):
        if shutil.which(name, path=env["PATH"]):
            raise ValueError(f"development runtime unexpectedly on native PATH: {name}")
    return env


def environment():
    git = shutil.which("git")
    if not git:
        raise ValueError("Git is required for artifact qualification")
    return {"system": platform.platform(), "architecture": platform.machine(), "target": host_target(),
            "git": subprocess.check_output([git, "--version"], text=True).strip(),
            "ci": {name: os.environ[name] for name in
                ("GITHUB_SHA", "GITHUB_REPOSITORY", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT",
                 "ImageOS", "ImageVersion", "RUNNER_OS", "RUNNER_ARCH") if name in os.environ}}


def supported_shells(target):
    return ("powershell", "cmd", "git-bash") if target.endswith("windows-msvc") else ("bash", "zsh", "fish")


def shell_executable(name, env):
    """Select real host shells, avoiding the Windows WSL bash launcher."""
    override = env.get("TWIGSMITH_QUALIFY_SHELL_" + name.upper().replace("-", "_"))
    if override:
        return str(checked_file(Path(override)))
    if name == "cmd":
        return env.get("COMSPEC") or shutil.which("cmd", path=env.get("PATH"))
    if name == "git-bash":
        git = shutil.which("git", path=env.get("PATH"))
        if git:
            for root in (Path(git).resolve().parent.parent, Path(git).resolve().parent):
                for relative in ("bin/bash.exe", "usr/bin/bash.exe"):
                    candidate = root / relative
                    if candidate.is_file():
                        return str(candidate)
        return None
    return shutil.which("pwsh" if name == "powershell" else name, path=env.get("PATH"))


def shell_command(name, executable, script):
    if name in ("bash", "git-bash"):
        return [executable, "--noprofile", "--norc", script.as_posix()]
    if name == "zsh":
        return [executable, "-f", str(script)]
    if name == "fish":
        return [executable, "--no-config", str(script)]
    if name == "powershell":
        return [executable, "-NoLogo", "-NoProfile", "-File", str(script)]
    if name == "cmd":
        return [executable, "/d", "/c", str(script)]
    raise ValueError(f"unknown qualification shell: {name}")


def shell_script(name, completion=False):
    """Adapter invocation smoke only: no line-editor/actual-Tab claim."""
    if completion:
        scripts = {
            "bash": '''source "$TWIGSMITH_ADAPTER" || exit 1
complete -p "$TWIGSMITH_ALIAS" >/dev/null || exit 1
COMP_LINE="$TWIGSMITH_ALIAS st"; COMP_POINT=${#COMP_LINE}
_twigsmith "$TWIGSMITH_ALIAS"
[[ "$(printf '%s\\n' "${COMPREPLY[@]}")" == $'start\\nstatus' ]] || exit 1
printf 'SHELL_PROBE_OK\\n'
''',
            "zsh": '''autoload -Uz compinit
compinit -u -D
source "$TWIGSMITH_ADAPTER" || exit 1
[[ ${_comps[$TWIGSMITH_ALIAS]} == _twigsmith ]] || exit 1
compadd() { shift; printf '%s\\n' "$@"; }
words=("$TWIGSMITH_ALIAS" st); CURRENT=2; PREFIX=st
[[ "$(_twigsmith)" == $'start\\nstatus' ]] || exit 1
printf 'SHELL_PROBE_OK\\n'
''',
            "fish": '''source "$TWIGSMITH_ADAPTER"; or exit 1
set -l actual (complete -C "$TWIGSMITH_ALIAS st" | string split -f 1 \\t)
test (count $actual) -eq 2; or exit 1
test "$actual[1]" = start; and test "$actual[2]" = status; or exit 1
printf 'SHELL_PROBE_OK\\n'
''',
            "powershell": '''$ErrorActionPreference = 'Stop'
. $env:TWIGSMITH_ADAPTER
$line = "$env:TWIGSMITH_ALIAS st"
$matches = @((TabExpansion2 $line $line.Length).CompletionMatches)
if ($matches.Count -ne 2 -or $matches[0].ListItemText -cne 'start' -or $matches[1].ListItemText -cne 'status') { throw 'unexpected adapter candidates' }
Write-Output 'SHELL_PROBE_OK'
''',
        }
        return scripts[name]
    if name == "powershell":
        return '''$ErrorActionPreference = 'Stop'
$actual = & $env:TWIGSMITH_ALIAS --version
if ($LASTEXITCODE -ne 0 -or $actual -cne $env:TWIGSMITH_EXPECTED_VERSION) { throw 'unexpected version' }
Write-Output 'SHELL_PROBE_OK'
'''
    if name == "cmd":
        # The driver checks exact output and status. No policy/profile changes.
        return '@echo off\n%TWIGSMITH_ALIAS% --version\nexit /b %errorlevel%\n'
    if name == "fish":
        return '''set -l actual ($TWIGSMITH_ALIAS --version); or exit 1
test "$actual" = "$TWIGSMITH_EXPECTED_VERSION"; or exit 1
printf 'SHELL_PROBE_OK\\n'
'''
    return '''actual=$("$TWIGSMITH_ALIAS" --version) || exit 1
[ "$actual" = "$TWIGSMITH_EXPECTED_VERSION" ] || exit 1
printf 'SHELL_PROBE_OK\\n'
'''


def shell_routes(artifact, version, evidence, *, require=False):
    """Bind selected launch/adapter routes to the final archive's actual bytes."""
    target = host_target()
    report = {"required": require, "selected": list(supported_shells(target)), "routes": [],
              "scope": "Final-artifact launch and generated-adapter invocation smoke for both aliases; "
                       "not actual Tab, terminal/editor/cancellation, profile installation or arbitrary shell versions. "
                       "Prior source-level actual-Tab evidence is separate and does not qualify these archive bytes."}
    directory = evidence / "shell-routes"
    directory.mkdir()
    with tempfile.TemporaryDirectory(prefix="twigsmith-shell-routes-") as temporary:
        temporary = Path(temporary)
        env = native_environment(temporary, os.environ)
        env["PATH"] = str(artifact) + os.pathsep + env["PATH"]
        env["TWIGSMITH_EXPECTED_VERSION"] = f"twigsmith {version}"
        for name in report["selected"]:
            executable = shell_executable(name, os.environ)
            route = {"name": name, "executable": executable, "available": bool(executable), "probes": [], "passed": False}
            report["routes"].append(route)
            if not executable:
                route["gap"] = "required host shell unavailable"
                continue
            version_command = ([executable, "-NoLogo", "-NoProfile", "-Command", "$PSVersionTable.PSVersion.ToString()"]
                               if name == "powershell" else [executable, "/d", "/c", "ver"]
                               if name == "cmd" else [executable, "--version"])
            output = fixture_output(version_command, directory / f"{name}-version.log", env, temporary)
            route.update(version=output.stdout.strip(), version_command=version_command, version_exit=output.returncode)
            if output.returncode or not output.stdout.strip():
                route["gap"] = "shell version observation failed"
                continue
            if name == "powershell" and not re.match(r"^7\.6\.", output.stdout.strip()):
                route["gap"] = "approved PowerShell 7.6 line required"
                continue
            adapter = directory / (f"adapter-{name}.ps1" if name == "powershell" else f"adapter-{name}.txt")
            completion = name not in ("cmd", "git-bash")
            if completion:
                binary = artifact / ("twigsmith.exe" if target.endswith("windows-msvc") else "twigsmith")
                command = [str(binary), "completion", name]
                output = fixture_output(command, directory / f"{name}-generate.log", env, temporary)
                route["adapter_generation"] = {"command": command, "exit": output.returncode}
                if output.returncode or not output.stdout.strip():
                    route["gap"] = "artifact adapter generation failed"
                    continue
                adapter.write_text(output.stdout, encoding="utf-8")
                route["adapter_sha256"] = digest(adapter)
                env["TWIGSMITH_ADAPTER"] = str(adapter)
            for action in (["launch", "completion"] if completion else ["launch"]):
                suffix = ".ps1" if name == "powershell" else ".cmd" if name == "cmd" else ".sh"
                script = directory / f"{name}-{action}{suffix}"
                script.write_text(shell_script(name, completion=action == "completion"), encoding="utf-8")
                for alias in ("twigsmith", "tgi"):
                    env["TWIGSMITH_ALIAS"] = alias
                    command = shell_command(name, executable, script)
                    output = fixture_output(command, directory / f"{name}-{action}-{alias}.log", env, temporary)
                    expected = f"twigsmith {version}" if name == "cmd" else "SHELL_PROBE_OK"
                    route["probes"].append({"name": f"{action}-{alias}", "command": command, "alias": alias,
                                            "exit": output.returncode, "passed": output.returncode == 0
                                            and output.stdout.strip() == expected})
            route["passed"] = all(probe["passed"] for probe in route["probes"])
    report["coverage_complete"] = all(route["passed"] for route in report["routes"])
    # Missing optional runtimes remain explicit gaps; failures of available routes
    # always fail, and acceptance mode requires every approved host shell.
    report["passed"] = all(route["passed"] for route in report["routes"] if route["available"]) and (
        report["coverage_complete"] or not require)
    write_json(directory / "shell-routes.json", report)
    return report


def native_run(args, evidence, report):
    manifest = native_package.verify(args.archive)
    target = host_target()
    if manifest["target"] != target:
        raise ValueError("archive target does not match the actual native host")
    harnesses = harness_manifest(args.harness_dir.resolve(), target)
    if harnesses["source"] != manifest["provenance"]["source"]:
        raise ValueError("fixture harness and native artifact source identities differ")
    extracted = evidence / "extracted"
    native_package.verify(args.archive, extracted)
    artifact = extracted / native_package.archive_stem(args.archive)
    binaries = {name: checked_file(artifact / name) for name in native_package.executable_names(target)}
    hashes = {name: digest(path) for name, path in binaries.items()}
    report.update(kind="native", profile="full", manifest=manifest, archive_sha256=digest(args.archive), payload_sha256=hashes)
    if native_package.target_metadata(target)["os"] == "linux":
        inspector = Path(__file__).resolve().parents[1] / "scripts/ci/inspect-native-elf.py"
        inspection = evidence / "elf"
        subprocess.run([sys.executable, str(inspector), "--artifact-dir", str(artifact),
                        "--evidence-dir", str(inspection)], check=True)
        report["elf"] = json.loads((inspection / "elf.json").read_text())
        if not report["elf"]["passed"]:
            raise ValueError("native ELF runtime inspection failed")
    for name, binary in binaries.items():
        output = subprocess.run([str(binary), "--version"], capture_output=True, text=True)
        (evidence / f"{name}--version.log").write_text(output.stdout + output.stderr)
        if output.returncode or output.stdout.strip() != f"twigsmith {manifest['version']}" or output.stderr:
            raise ValueError(f"unexpected archived {name} --version result")
    passed = run_fixtures(artifact, args.harness_dir.resolve(), evidence, report)
    report["shell_routes"] = shell_routes(artifact, manifest["version"], evidence,
                                          require=getattr(args, "require_shells", False))
    passed = passed and report["shell_routes"]["passed"]
    report["passed"] = passed
    if hashes != {name: digest(path) for name, path in binaries.items()}:
        raise ValueError("artifact bytes changed during qualification")
    return 0 if passed else 1


def package_set(directory):
    manifest = json.loads((directory / "manifest.json").read_text())
    if manifest.get("schema") != 2 or manifest.get("topology") != "optional-platform-packages":
        raise ValueError("expected an npm platform package-set manifest")
    packages = {}
    for item in manifest["packages"]:
        filename = item["filename"]
        if Path(filename).name != filename or filename in ("", ".", ".."):
            raise ValueError("unsafe npm package filename")
        path = directory / filename
        if path.is_symlink() or not path.is_file() or digest(path) != item["sha256"]:
            raise ValueError(f"npm package hash mismatch: {filename}")
        with tarfile.open(path, "r:gz") as archive:
            entries = [entry for entry in archive.getmembers() if entry.name == "package/package.json"]
            if len(entries) != 1 or not entries[0].isfile():
                raise ValueError("missing or duplicate npm package metadata")
            metadata = json.load(archive.extractfile(entries[0]))
        if metadata["name"] != item["name"] or metadata["version"] != manifest["version"]:
            raise ValueError("npm package metadata differs from package set")
        if item["name"] in packages:
            raise ValueError("duplicate npm package name")
        packages[item["name"]] = (item, metadata, path)
    if "twigsmith" not in packages:
        raise ValueError("npm package set has no top-level launcher")
    return manifest, packages


def registry_response(path, packages, base):
    if path in packages:
        item, metadata, tarball = packages[path]
        version = dict(metadata, dist={"tarball": base + "tarballs/" + tarball.name,
                                       "shasum": hashlib.sha1(tarball.read_bytes()).hexdigest()})
        return json.dumps({"name": item["name"], "dist-tags": {"latest": item["version"]},
                           "versions": {item["version"]: version}}).encode(), "application/json"
    found = [value[2] for value in packages.values() if path == "tarballs/" + value[2].name]
    if found:
        return found[0].read_bytes(), "application/octet-stream"
    raise ValueError("package resource is outside the exact local set")


@contextlib.contextmanager
def local_registry(packages):
    """Serve only exact verified local bytes; publication and public registry are unused."""
    requests = []
    class Registry(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            path = unquote(self.path.split("?", 1)[0]).lstrip("/")
            requests.append(path)
            try:
                body, content_type = registry_response(path, packages,
                    f"http://127.0.0.1:{self.server.server_port}/")
            except ValueError:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Registry)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/", requests
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def npm_invocation(args, *, npx=False):
    npm = str(args.npm) if args.npm else shutil.which("npm")
    if not npm:
        raise ValueError("npm was not found on PATH")
    if os.name == "nt":
        cli = Path(npm).parent / "node_modules/npm/bin" / ("npx-cli.js" if npx else "npm-cli.js")
        if not cli.is_file():
            raise ValueError("npm JavaScript entry point was not found beside npm.cmd")
        return [shutil.which("node") or "node", str(cli)]
    return [str(Path(npm).with_name("npx")) if npx else npm]


def powershell_shim_probes(shim_dir, version, evidence, env):
    """Observe installed .ps1 launch/diagnostics under the host's default policy.

    This probe alone does not establish interactive/cancellation behavior;
    the separate powershell_terminal report records those installed-shim fixtures.
    """
    pwsh = shutil.which("pwsh", path=env.get("PATH"))
    if not pwsh:
        raise ValueError("PowerShell is required for Windows npm shim qualification")
    output = fixture_output([pwsh, "-NoLogo", "-NoProfile", "-Command", "$PSVersionTable.PSVersion.ToString()"],
                            evidence / "npm-powershell-version.log", env, evidence)
    if output.returncode or not output.stdout.strip():
        raise ValueError("PowerShell version observation failed")
    report = {"executable": pwsh, "version": output.stdout.strip(), "shim_sha256": {},
              "scope": "Noninteractive installed .ps1 invocation with default execution policy; "
                       "terminal/cancellation results are recorded separately under powershell_terminal.",
              "probes": []}
    for name in ("twigsmith", "tgi"):
        shim = shim_dir / f"{name}.ps1"
        if shim.is_symlink() or not shim.is_file():
            raise ValueError(f"expected installed regular PowerShell shim: {shim}")
        report["shim_sha256"][name] = digest(shim)
        for label, arguments, expected in [("version", ["--version"], 0),
                                            ("help", ["--help"], 0), ("usage", ["start"], 2)]:
            output = fixture_output([pwsh, "-NoLogo", "-NoProfile", "-File", str(shim), *arguments],
                                    evidence / f"npm-powershell-{name}-{label}.log", env, evidence)
            passed = output.returncode == expected
            if label == "version":
                passed = passed and output.stdout.strip() == f"twigsmith {version}"
            elif label == "help":
                passed = passed and "twigsmith" in output.stdout.lower() and "Usage:" in output.stdout
            else:
                passed = passed and "use --help" in output.stdout
            report["probes"].append({"name": f"powershell-{name}-{label}", "passed": passed,
                                     "exit": output.returncode})
        if digest(shim) != report["shim_sha256"][name]:
            raise ValueError("installed PowerShell shim changed during qualification")
    return report


def powershell_terminal_fixtures(bridge, harness, evidence, env):
    """Keep .cmd evidence intact while exercising actual installed .ps1 shims."""
    directory = evidence / "powershell-terminal"
    directory.mkdir()
    executable = shutil.which("pwsh", path=env.get("PATH"))
    if not executable:
        raise ValueError("PowerShell is required for Windows npm terminal qualification")
    pwsh = checked_file(Path(executable).resolve(strict=True))
    if not pwsh.stat().st_size:
        raise ValueError("selected npm PowerShell must be nonempty")
    trace = directory / "shim-invocations.jsonl"
    trace.touch(exist_ok=False)
    selected_env = dict(env, TWIGSMITH_NPM_SHIM_KIND="ps1", TWIGSMITH_NPM_POWERSHELL=str(pwsh),
                        TWIGSMITH_NPM_SHIM_TRACE=str(trace))
    report = {"profile": "launcher", "shim_kind": "ps1",
              "powershell_executable": str(pwsh),
              "invocation": "qualification bridge -> pwsh -NoLogo -NoProfile -File installed .ps1 -> Node launcher -> native payload",
              "scope": "Three existing launcher/terminal fixtures; no execution-policy override."}
    run_fixtures(bridge, harness, directory, report, kind="npm", env=selected_env,
                 selected_cases=npm_cases("launcher"))
    report["shim_trace"] = powershell_trace_report(trace, Path(env["TWIGSMITH_NPM_SHIM_DIR"]), pwsh)
    report["passed"] = report["passed"] and report["shim_trace"]["passed"]
    write_json(directory / "qualification.json", report)
    return report


def powershell_trace_report(trace, shim_dir, pwsh):
    """A passing fixture is insufficient unless the isolated bridge used .ps1."""
    expected = {name for names in LAUNCHER_CASES.values() for name in names}
    report = {"path": str(trace), "entries": [], "errors": [], "passed": False}
    try:
        if trace.is_symlink() or not trace.is_file():
            raise ValueError("expected regular qualification shim trace")
        report["entries"] = [json.loads(line) for line in trace.read_text(encoding="utf-8").splitlines()]
        if not report["entries"]:
            raise ValueError("no actual bridge invocations recorded")
        cases, aliases = set(), set()
        for row in report["entries"]:
            if not isinstance(row, dict) or set(row) != {"kind", "shim", "pwsh", "isolated_test", "alias"}:
                raise ValueError("invalid shim invocation trace fields")
            alias = row["alias"]
            if row["kind"] != "ps1" or alias not in ("twigsmith", "tgi") or row["isolated_test"] not in expected:
                raise ValueError("bridge used wrong shim kind, alias or isolated test")
            for field, selected in (("pwsh", pwsh), ("shim", shim_dir / f"{alias}.ps1")):
                actual = Path(row[field])
                if not actual.is_absolute() or not actual.samefile(selected):
                    raise ValueError(f"bridge used unexpected {field}")
            cases.add(row["isolated_test"])
            aliases.add(alias)
        if cases != expected or aliases != {"twigsmith", "tgi"}:
            raise ValueError("shim invocation trace lacks a selected fixture or alias")
        report["passed"] = True
    except (OSError, ValueError, TypeError) as error:
        report["errors"].append(str(error))
    return report


def selected_npm_node(env):
    executable = shutil.which("node", path=env.get("PATH"))
    if not executable:
        raise ValueError("selected npm Node executable was not found")
    node = checked_file(Path(executable).resolve(strict=True))
    if not node.stat().st_size:
        raise ValueError("selected npm Node executable must be nonempty")
    return node


def npm_run(args, evidence, report):
    import npm_platform_package
    directory = args.package_set_dir.resolve()
    manifest, packages = package_set(directory)
    target = host_target()
    expected = npm_platform_package.PACKAGES[target]
    if expected not in packages or packages[expected][0].get("native", {}).get("target") != target:
        raise ValueError("npm package set lacks the exact native host payload")
    native = packages[expected][0]["native"]
    version = manifest["version"]
    profile = getattr(args, "profile", "full")
    selected = npm_cases(profile)
    harness = args.harness_dir.resolve()
    prepared = harness_manifest(harness, target)
    if prepared["source"] != manifest["source"]:
        raise ValueError("fixture harness and npm artifact source identities differ")
    node = selected_npm_node(os.environ)
    report.update(kind="npm", profile=profile, package_set=manifest, node_executable=str(node),
                  node=subprocess.check_output([str(node), "--version"], text=True).strip(),
                  npm=subprocess.check_output(npm_invocation(args) + ["--version"], text=True).strip(),
                  invocation="qualification bridge -> actual npm shim -> Node launcher -> exact native payload")
    with tempfile.TemporaryDirectory(prefix="twigsmith-npm-qualification-") as temporary:
        temporary = Path(temporary)
        env = npm_platform_package.npm_environment(temporary)
        env["TWIGSMITH_NPM_NODE"] = str(node)
        env["npm_config_offline"] = "false"
        env["npm_config_audit"] = "false"
        env["npm_config_fund"] = "false"
        # User proxy settings/credentials are intentionally absent. The only
        # registry serves the reviewed local tarballs on the loopback interface.
        with local_registry(packages) as (registry, requests):
            env["npm_config_registry"] = registry
            prefix = temporary / "global"
            command = npm_invocation(args) + ["install", "--global", "--ignore-scripts", "--prefix", str(prefix),
                                             "--no-audit", "--no-fund", str(packages["twigsmith"][2])]
            output = fixture_output(command, evidence / "npm-install.log", env, temporary)
            if output.returncode:
                raise RuntimeError("local npm global installation failed")
            modules = prefix / ("node_modules" if os.name == "nt" else "lib/node_modules")
            resolved_metadata = Path(subprocess.check_output([
                str(node), "-e",
                "process.stdout.write(require.resolve(process.argv[1]+'/package.json',{paths:[process.argv[2]]}))",
                expected, str(modules / "twigsmith")], env=env, text=True))
            payload = resolved_metadata.parent / native["payload"]
            if digest(payload) != native["payload_sha256"]:
                raise ValueError("installed npm payload differs from native candidate")
            for name in ("twigsmith", expected):
                metadata_path = modules / "twigsmith/package.json" if name == "twigsmith" else resolved_metadata
                installed = json.loads(metadata_path.read_text())
                if installed["version"] != version:
                    raise ValueError("installed npm package version differs from candidate")
            shim_dir = prefix if os.name == "nt" else prefix / "bin"
            bridge_dir = temporary / "fixture-bridge"
            bridge_dir.mkdir()
            bridge = harness / prepared["executables"]["native_npm_fixture"]["path"]
            for name in ("twigsmith", "tgi"):
                shutil.copy2(bridge, bridge_dir / (name + (".exe" if os.name == "nt" else "")))
            env["TWIGSMITH_NPM_SHIM_DIR"] = str(shim_dir)
            env["TWIGSMITH_NPM_SHIM_KIND"] = "cmd"
            # Both commands go through the real installed npm shim even under
            # native PTY/ConPTY fixtures; the fixture bridge adds no product logic.
            for name in ("twigsmith", "tgi"):
                command = [str(bridge_dir / (name + (".exe" if os.name == "nt" else ""))), "--version"]
                output = fixture_output(command, evidence / f"npm-{name}--version.log", env, temporary)
                if output.returncode or output.stdout.strip() != f"twigsmith {version}":
                    raise ValueError(f"installed npm {name} shim failed")
            passed = run_fixtures(bridge_dir, harness, evidence, report, kind="npm", env=env, selected_cases=selected)
            report["probes"] = []
            if os.name == "nt":
                report["powershell"] = powershell_shim_probes(shim_dir, version, evidence, env)
                report["probes"].extend(report["powershell"]["probes"])
                report["powershell_terminal"] = powershell_terminal_fixtures(bridge_dir, harness, evidence, env)
                passed = passed and report["powershell_terminal"]["passed"]
            original_metadata = resolved_metadata.read_bytes()
            try:
                for label, field, value in [("wrong-version-refusal", "version", "0.0.0-invalid"),
                                            ("wrong-platform-refusal", "os", ["invalid-host"])]:
                    changed = json.loads(original_metadata)
                    changed[field] = value
                    write_json(resolved_metadata, changed)
                    output = fixture_output([str(node), str(modules / "twigsmith/launcher.cjs"), "--version"],
                                            evidence / f"npm-{label}.log", env, temporary)
                    report["probes"].append({"name": label, "passed": output.returncode != 0
                                            and f"invalid {expected}" in output.stdout})
            finally:
                resolved_metadata.write_bytes(original_metadata)
            # npx is npm's own maintained npx entry point, using an empty cache.
            # The explicit version also covers prerelease versions unchanged.
            env["npm_config_cache"] = str(temporary / "npx-cache")
            for name in ("twigsmith", "tgi"):
                command = npm_invocation(args, npx=True) + ["--yes", "--ignore-scripts", f"--package=twigsmith@{version}",
                                                          "--", name, "--version"]
                output = fixture_output(command, evidence / f"npx-{name}.log", env, temporary)
                success = output.returncode == 0 and output.stdout.strip() == f"twigsmith {version}"
                report["probes"].append({"name": f"explicit-version-npx-{name}", "passed": success})
            omitted = temporary / "omitted"
            command = npm_invocation(args) + ["install", "--ignore-scripts", "--omit=optional", "--prefix", str(omitted),
                                             "--no-audit", "--no-fund", str(packages["twigsmith"][2])]
            output = fixture_output(command, evidence / "npm-omit-install.log", env, temporary)
            if output.returncode:
                raise RuntimeError("omitted-optional npm installation failed")
            launcher = omitted / "node_modules/twigsmith/launcher.cjs"
            output = fixture_output([str(node), str(launcher), "--version"],
                                    evidence / "npm-omit-launch.log", env, temporary)
            report["probes"].append({"name": "omitted-optional-refusal", "passed": output.returncode != 0
                                      and f"missing {expected}@{version}" in output.stdout})
            report["registry_requests"] = list(requests)
            if digest(payload) != native["payload_sha256"]:
                raise ValueError("installed npm payload changed during qualification")
            package_set(directory)
            report["passed"] = passed and all(probe["passed"] for probe in report["probes"])
    return 0 if report["passed"] else 1


def run(args):
    evidence = args.evidence_dir.resolve()
    evidence.mkdir(parents=True, exist_ok=False)
    report = {"schema": 1, "passed": False, "gaps": list(GAPS), "tests": []}
    write_json(evidence / "qualification.json", report)
    try:
        report["environment"] = environment()
        return native_run(args, evidence, report) if args.command == "run" else npm_run(args, evidence, report)
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        report.update(passed=False, error=str(error))
        raise
    finally:
        write_json(evidence / "qualification.json", report)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prepare_parser = commands.add_parser("prepare", help="compile pinned native fixtures and helpers once")
    prepare_parser.add_argument("--harness-dir", required=True, type=Path)
    prepare_parser.add_argument("--cargo", type=Path)
    native = commands.add_parser("run", help="verify/extract and qualify exact native archive; never compiles")
    native.add_argument("--archive", required=True, type=Path)
    native.add_argument("--require-shells", action="store_true",
                        help="require every approved host shell launch/completion route; missing shells fail")
    npm = commands.add_parser("npm", help="qualify unpublished exact packages with local npm install/npx")
    npm.add_argument("--package-set-dir", required=True, type=Path)
    npm.add_argument("--npm", type=Path)
    npm.add_argument("--profile", choices=["full", "launcher"], default="full",
                     help="full workflow coverage (default), or bounded launcher/terminal fixtures; all npm probes run")
    for command in (native, npm):
        command.add_argument("--harness-dir", required=True, type=Path)
        command.add_argument("--evidence-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        return prepare(args) if args.command == "prepare" else run(args)
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        parser.exit(1, f"platform qualification failed: {error}\n")


if __name__ == "__main__":
    sys.exit(main())
