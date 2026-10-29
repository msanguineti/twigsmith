#!/usr/bin/env python3
"""Run existing real-Git fixtures against extracted native artifacts, without Cargo."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import time


# Explicit test names prevent library-only coverage from being counted as native
# artifact qualification. The fixtures independently inspect Git refs and state.
REPRESENTATIVE = {
    "cli_smoke": ["cli_runs_outside_git_without_node_or_rust_tools"],
    "cli_lifecycle": ["public_cli_project_and_fork_lifecycles_preserve_both_tracking_styles"],
    "cli_oneflow": [
        "ordinary_lifecycle_executable_and_alias_wiring",
        "configured_direct_release_uses_selected_cutoff_and_publishes_version",
        "hotfix_explicit_tag_routes_to_declared_release_or_development",
    ],
    "cli_hosted_oneflow": [
        "hosted_executable_alias_and_shared_remote_wiring",
        "hosted_release_project_fork_and_exact_version_history",
        "hosted_hotfix_routes_to_declared_active_release_or_development",
    ],
}
RECOVERY = {
    "cli_direct_sequence": [
        "native_rebase_conflict_continuation_checks_actual_prepared_source_once",
        "later_push_rejection_retries_exact_result_before_any_cleanup",
    ],
    "cli_hosted_recovery": [
        "hosted_partial_multiple_destination_retry_preserves_confirmed_effects",
        "hosted_retry_refuses_changed_instances_tag_objects_settings_and_endpoints",
    ],
    "cli_oneflow": [
        "partial_multidestination_release_delivery_and_source_deleted_cleanup_resume",
        "retry_rejects_changed_settings_and_same_commit_tag_object_substitution",
    ],
}
BROAD = {
    "cli_lifecycle": [
        "guided_setup_checks_prepared_work_before_successful_publication",
        "ordinary::staged_save_retains_residual_work_then_commit_resume_and_squash_local_cleanup",
        "ordinary::stash_switch_resume_restore_index_save_again_and_merge_both_cleanup",
        "ordinary::default_config_wip_switch_resume_native_reword_and_both_cleanup",
        "ordinary::published_default_config_stopped_reword_fresh_process_resolves_method_without_reediting",
    ],
    "cli_finish": [
        "finish_grammar_and_help_without_git",
        "finish_capable_navigation_review_and_integration_delivery_cancel_and_failure",
        "integration_git_editor_suggestion_multiline_failure_and_context_guard",
        "integration_cancelled_editor_after_preparation_retains_work_and_retries",
        "integration_missing_path_shell_retains_work_and_reports_editor_failure",
    ],
    "finish_checks": [
        "real_checks_use_literal_arguments_root_environment_and_order",
        "cancellation_stops_check_group_retains_preparation_and_blocks_publication",
        "check_output_without_newline_is_live_and_fragmented_credentials_are_redacted",
    ],
}
UTILITIES = "sh cat cp mv rm mkdir touch sleep sed grep awk head tail sort tr cut dirname basename chmod ln env true false printf test wc find xargs readlink expr uname".split()
GAPS = [
    "Finite selected fixture coverage; this is not the complete Cargo suite.",
    "No Node/npm/npx, shell-completion, Windows, macOS, signing-helper or transport-provider qualification.",
    "Signal cleanup covers managed configured-check process groups; arbitrary external editor/transport descendants are not covered.",
    "Userspace/container execution does not establish old-kernel compatibility.",
]


def cases(profile):
    result = {suite: list(names) for suite, names in REPRESENTATIVE.items()}
    for extra in ([RECOVERY] if profile == "floor" else [RECOVERY, BROAD] if profile == "broad" else []):
        for suite, names in extra.items():
            result.setdefault(suite, []).extend(names)
    return result


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def checked_file(path):
    path = Path(path)
    if not path.is_absolute() or path.is_symlink() or not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError(f"expected absolute regular executable: {path}")
    return path


def prepare(args):
    directory = args.harness_dir.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    suites = sorted(cases("broad"))
    root = Path(__file__).resolve().parents[1]
    command = [str(root / "dev"), "cargo", "test", "--locked", "--no-run", "--message-format=json"]
    for suite in suites:
        command += ["--test", suite]
    build = subprocess.run(command, cwd=root, text=True, stdout=subprocess.PIPE)
    if build.returncode:
        for line in build.stdout.splitlines():
            try:
                message = json.loads(line).get("message", {})
            except ValueError:
                continue
            if message.get("rendered"):
                print(message["rendered"], file=sys.stderr, end="")
        raise RuntimeError("fixture harness compilation failed")
    executables = {}
    for line in build.stdout.splitlines():
        item = json.loads(line)
        name = item.get("target", {}).get("name")
        if item.get("reason") == "compiler-artifact" and item.get("executable") and name in suites:
            target = directory / name
            shutil.copy2(item["executable"], target)
            executables[name] = digest(target)
    if set(executables) != set(suites):
        raise RuntimeError(f"missing compiled harnesses: {set(suites) - set(executables)}")
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    dirty = subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True)
    write_json(directory / "harnesses.json", {
        "schema": 1, "source_revision": revision, "source_dirty": bool(dirty),
        "driver_sha256": digest(__file__), "executables": executables,
    })
    print(f"Prepared {len(executables)} fixture harnesses in {directory}")


def run(args):
    evidence = args.evidence_dir.resolve()
    evidence.mkdir(parents=True, exist_ok=False)
    write_json(evidence / "qualification.json", {
        "schema": 1, "profile": args.profile, "passed": False,
        "artifact_dir": str(args.artifact_dir.resolve()), "tests": [], "gaps": GAPS,
    })
    try:
        return qualify(args, evidence)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        path = evidence / "qualification.json"
        report = json.loads(path.read_text())
        report.update(passed=False, error=str(error))
        write_json(path, report)
        raise


def qualify(args, evidence):
    artifact = args.artifact_dir.resolve()
    harness = args.harness_dir.resolve()
    manifest = json.loads((harness / "harnesses.json").read_text())
    if manifest["driver_sha256"] != digest(__file__):
        raise ValueError("harness selection driver differs from prepared source")
    binaries = {name: checked_file(artifact / name) for name in ["twigsmith", "tgi"]}
    hashes = {name: digest(path) for name, path in binaries.items()}
    selected = cases(args.profile)
    for suite in selected:
        path = checked_file(harness / suite)
        if digest(path) != manifest["executables"][suite]:
            raise ValueError(f"changed harness: {suite}")
    git = Path(args.git).resolve()
    checked_file(git)
    report = {
        "schema": 1, "profile": args.profile, "artifact_sha256": hashes,
        "harness_manifest": manifest, "platform": platform.platform(),
        "uname": list(platform.uname()), "libc": os.confstr("CS_GNU_LIBC_VERSION"),
        "os_release": Path("/etc/os-release").read_text(),
        "git": str(git), "git_version": subprocess.check_output([str(git), "--version"], text=True).strip(),
        "git_exec_path": subprocess.check_output([str(git), "--exec-path"], text=True).strip(),
        "tests": [], "gaps": list(GAPS),
    }
    if args.profile != "broad":
        report["gaps"].append("Broad native PTY/editor/streaming/signal/missing-shell coverage is assigned to the baseline reference-Git cell.")
    report["passed"] = False
    write_json(evidence / "qualification.json", report)
    with tempfile.TemporaryDirectory(prefix="twigsmith-native-qualification-") as temporary:
        temporary = Path(temporary)
        runtime = temporary / "runtime-bin"
        runtime.mkdir()
        tools = {}
        for name in UTILITIES:
            source = shutil.which(name, path=os.defpath)
            if source:
                (runtime / name).symlink_to(source)
                tools[name] = source
        (runtime / "git").symlink_to(git)
        report["runtime_tools"] = tools
        env = {"PATH": str(runtime), "TWIGSMITH_ARTIFACT_DIR": str(artifact),
               "HOME": str(temporary), "TMPDIR": str(temporary), "LC_ALL": "C", "TZ": "UTC",
               "GIT_EXEC_PATH": report["git_exec_path"]}
        for name in ["rustc", "cargo", "node", "npm", "npx"]:
            if shutil.which(name, path=env["PATH"]):
                raise ValueError(f"unexpected development tool on runtime PATH: {name}")
        # Direct native exit/diagnostic probes supplement the workflow fixtures.
        for name, binary in binaries.items():
            for label, argv, expected, needle in [
                ("version", ["--version"], 0, ""),
                ("missing-git", ["status"], 1, "Git is installed and available on PATH"),
                ("usage", ["start"], 2, "use --help"),
            ]:
                probe_env = dict(env, PATH="")
                output = subprocess.run([str(binary), *argv], cwd=temporary, env=probe_env,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                passed = output.returncode == expected and needle in output.stderr
                if label == "version":
                    passed = passed and not output.stderr
                (evidence / f"{name}-{label}.log").write_text(output.stdout + output.stderr)
                report["tests"].append({"suite": name, "name": label, "passed": passed, "exit": output.returncode})
        for suite, names in selected.items():
            executable = str(harness / suite)
            listing = subprocess.check_output([executable, "--list"], env=env, cwd=temporary, text=True)
            for name in names:
                if f"{name}: test\n" not in listing:
                    raise ValueError(f"selected fixture absent: {suite}::{name}")
                started = time.monotonic()
                output = subprocess.run([executable, "--exact", name, "--include-ignored", "--nocapture"],
                                        env=env, cwd=temporary, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                        text=True, timeout=300)
                (evidence / f"{suite}--{name.replace('::', '--')}.log").write_text(output.stdout)
                passed = output.returncode == 0 and "1 passed; 0 failed" in output.stdout
                report["tests"].append({"suite": suite, "name": name, "passed": passed,
                                        "exit": output.returncode, "seconds": round(time.monotonic() - started, 3)})
                print(f"{'PASS' if passed else 'FAIL'} {suite}::{name}", flush=True)
                write_json(evidence / "qualification.json", report)
    if hashes != {name: digest(path) for name, path in binaries.items()}:
        raise ValueError("artifact bytes changed during qualification")
    report["passed"] = all(test["passed"] for test in report["tests"])
    write_json(evidence / "qualification.json", report)
    return 0 if report["passed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build = commands.add_parser("prepare", help="compile and stage fixture harnesses once on the baseline builder")
    build.add_argument("--harness-dir", required=True, type=Path)
    qualify = commands.add_parser("run", help="qualify extracted bytes using staged harnesses; never builds")
    qualify.add_argument("--artifact-dir", required=True, type=Path)
    qualify.add_argument("--harness-dir", required=True, type=Path)
    qualify.add_argument("--evidence-dir", required=True, type=Path)
    qualify.add_argument("--profile", choices=["broad", "floor", "representative"], required=True)
    qualify.add_argument("--git", required=True, type=Path)
    args = parser.parse_args()
    try:
        return prepare(args) if args.command == "prepare" else run(args)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"native qualification failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
