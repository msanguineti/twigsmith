"""Driver integrity and false-positive boundaries; synthetic fixtures are not native evidence."""
import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import qualify_platform as driver


class DriverTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def test_successful_outer_harness_requires_real_matching_isolated_child(self):
        for output in ["1 passed; 0 failed\n", "0 passed; 0 failed\n",
                       "TWIGSMITH_QUALIFICATION_CHILD other\n1 passed; 0 failed\n"]:
            self.assertFalse(driver.fixture_passed(subprocess.CompletedProcess([], 0, output), "wanted"))
        output = "TWIGSMITH_QUALIFICATION_CHILD wanted\n1 passed; 0 failed\n"
        self.assertTrue(driver.fixture_passed(subprocess.CompletedProcess([], 0, output), "wanted"))
        self.assertFalse(driver.fixture_passed(subprocess.CompletedProcess([], 1, output), "wanted"))

    def harnesses(self):
        manifest = {"target": "fixture-target", "cases": driver.CASES, "executables": {}}
        for name in [*driver.CASES, *driver.EXAMPLES]:
            path = self.root / name
            path.write_bytes(b"fixture bytes")
            path.chmod(0o644)
            manifest["executables"][name] = {"path": name, "sha256": driver.digest(path)}
        driver.write_json(self.root / "harnesses.json", manifest)
        return manifest

    def test_npm_full_profile_preserves_complete_catalog_and_prepared_manifest(self):
        selected = driver.npm_cases("full")
        self.assertEqual(selected, driver.CASES)
        self.assertEqual(sum(map(len, selected.values())), 17)
        selected["native_journey"].clear()
        self.assertTrue(driver.CASES["native_journey"])
        manifest = self.harnesses()
        self.assertEqual(driver.harness_manifest(self.root, "fixture-target"), manifest)

    def test_npm_launcher_profile_selects_alias_journey_cancellation_and_editor_handoff(self):
        selected = driver.npm_cases("launcher")
        self.assertEqual(selected, {
            "native_journey": ["native_start_status_journey"],
            "native_checks": ["native_cli_interrupt_contains_tree_and_preserves_retry"],
            "native_editor": ["native_git_editor_accept_and_cancel_retry"],
        })
        for suite, names in selected.items():
            self.assertTrue(set(names).issubset(driver.CASES[suite]))
        with self.assertRaisesRegex(ValueError, "unknown npm qualification profile"):
            driver.npm_cases("empty")

    def test_selected_profile_runs_only_listed_cases_and_records_exact_selection(self):
        self.harnesses()
        selected = driver.npm_cases("launcher")
        report = {"profile": "launcher"}
        def output(command, *_):
            if command[-1] == "--list":
                suite = Path(command[0]).name
                return subprocess.CompletedProcess(command, 0, "".join(f"{name}: test\n" for name in selected[suite]))
            name = command[2]
            return subprocess.CompletedProcess(command, 0, f"TWIGSMITH_QUALIFICATION_CHILD {name}\n1 passed; 0 failed\n")
        with patch.object(driver, "host_target", return_value="fixture-target"), \
                patch.object(driver, "fixture_output", side_effect=output) as execute, \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertTrue(driver.run_fixtures(self.root, self.root, self.root, report, kind="npm",
                                               selected_cases=selected))
        self.assertEqual(execute.call_count, 6)
        self.assertEqual(report["profile"], "launcher")
        self.assertEqual(report["selected_cases"], selected)
        self.assertEqual([(case["suite"], case["name"]) for case in report["tests"]],
                         [(suite, name) for suite, names in selected.items() for name in names])

    def test_selected_fixture_missing_from_prepared_harness_listing_refuses(self):
        self.harnesses()
        selected = {"native_journey": ["native_start_status_journey"]}
        with patch.object(driver, "host_target", return_value="fixture-target"), \
                patch.object(driver, "fixture_output", return_value=subprocess.CompletedProcess([], 0, "other: test\n")):
            with self.assertRaisesRegex(ValueError, "selected fixture absent"):
                driver.run_fixtures(self.root, self.root, self.root, {}, kind="npm", selected_cases=selected)

    def test_empty_or_unknown_selected_cases_cannot_qualify(self):
        for selected in ({}, {"native_journey": []}, {"unknown_suite": ["fixture"]},
                         {"native_journey": ["unknown_fixture"]}):
            with self.assertRaisesRegex(ValueError, "invalid selected qualification fixtures"):
                driver.run_fixtures(self.root, self.root, self.root, {}, selected_cases=selected)

    def test_downloaded_harness_modes_are_restored_without_changing_bytes(self):
        manifest = self.harnesses()
        self.assertEqual(driver.harness_manifest(self.root, "fixture-target"), manifest)
        if os.name != "nt":
            self.assertEqual((self.root / "native_journey").stat().st_mode & 0o777, 0o755)

    def test_harness_drift_never_silently_uses_another_binary(self):
        self.harnesses()
        (self.root / "native_journey").write_bytes(b"substituted executable")
        with self.assertRaisesRegex(ValueError, "bytes changed"):
            driver.harness_manifest(self.root, "fixture-target")

    def test_harness_target_and_cases_are_bound(self):
        self.harnesses()
        with self.assertRaisesRegex(ValueError, "target/case selection"):
            driver.harness_manifest(self.root, "other-target")
        with patch.object(driver, "CASES", {}):
            with self.assertRaisesRegex(ValueError, "target/case selection"):
                driver.harness_manifest(self.root, "fixture-target")

    def test_harness_paths_cannot_escape_or_link(self):
        manifest = self.harnesses()
        manifest["executables"]["native_journey"]["path"] = "../native_journey"
        driver.write_json(self.root / "harnesses.json", manifest)
        with self.assertRaisesRegex(ValueError, "unsafe fixture"):
            driver.harness_manifest(self.root, "fixture-target")
        if os.name != "nt":
            manifest = self.harnesses()
            path = self.root / "native_journey"
            path.unlink()
            path.symlink_to(self.root / "native_editor")
            with self.assertRaisesRegex(ValueError, "regular fixture"):
                driver.harness_manifest(self.root, "fixture-target")

    def test_wrong_host_archive_refuses_before_extraction_or_execution(self):
        args = argparse.Namespace(archive=self.root / "archive", harness_dir=self.root)
        with patch.object(driver.native_package, "verify", return_value={"target": "other"}) as verify:
            with patch.object(driver, "host_target", return_value="actual"):
                with self.assertRaisesRegex(ValueError, "actual native host"):
                    driver.native_run(args, self.root, {})
        verify.assert_called_once_with(args.archive)

    def test_failure_writes_unpassed_evidence(self):
        args = argparse.Namespace(command="run", evidence_dir=self.root / "evidence")
        with patch.object(driver, "environment", return_value={}):
            with patch.object(driver, "native_run", side_effect=ValueError("boundary failure")):
                with self.assertRaisesRegex(ValueError, "boundary failure"):
                    driver.run(args)
        report = json.loads((args.evidence_dir / "qualification.json").read_text())
        self.assertFalse(report["passed"])
        self.assertEqual(report["error"], "boundary failure")

    def package(self):
        metadata = {"name": "twigsmith", "version": "1.0.0-rc.7"}
        path = self.root / "twigsmith-1.0.0-rc.7.tgz"
        with tarfile.open(path, "w:gz") as archive:
            entry = tarfile.TarInfo("package/package.json")
            contents = json.dumps(metadata).encode()
            entry.size = len(contents)
            archive.addfile(entry, io.BytesIO(contents))
        item = dict(metadata, filename=path.name, sha256=driver.digest(path))
        manifest = {"schema": 2, "topology": "optional-platform-packages", "version": metadata["version"],
                    "packages": [item]}
        driver.write_json(self.root / "manifest.json", manifest)
        return manifest, metadata, path

    def test_package_bytes_and_prerelease_identity_are_bound(self):
        manifest, metadata, path = self.package()
        loaded, packages = driver.package_set(self.root)
        self.assertEqual(loaded, manifest)
        self.assertEqual(packages["twigsmith"][1], metadata)
        path.write_bytes(b"substituted archive")
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            driver.package_set(self.root)

    def test_package_metadata_cannot_substitute_another_version(self):
        manifest, _, _ = self.package()
        manifest["version"] = "1.0.0"
        driver.write_json(self.root / "manifest.json", manifest)
        with self.assertRaisesRegex(ValueError, "differs from package set"):
            driver.package_set(self.root)

    def test_package_paths_cannot_escape(self):
        manifest, _, _ = self.package()
        manifest["packages"][0]["filename"] = "../outside.tgz"
        driver.write_json(self.root / "manifest.json", manifest)
        with self.assertRaisesRegex(ValueError, "unsafe npm"):
            driver.package_set(self.root)

    def test_local_registry_serves_only_verified_packages_and_exact_versions(self):
        _, _, path = self.package()
        _, packages = driver.package_set(self.root)
        body, kind = driver.registry_response("twigsmith", packages, "http://127.0.0.1:1234/")
        metadata = json.loads(body)
        self.assertEqual(kind, "application/json")
        self.assertEqual(list(metadata["versions"]), ["1.0.0-rc.7"])
        self.assertEqual(metadata["versions"]["1.0.0-rc.7"]["dist"]["tarball"],
                         "http://127.0.0.1:1234/tarballs/" + path.name)
        body, kind = driver.registry_response("tarballs/" + path.name, packages, "unused")
        self.assertEqual(body, path.read_bytes())
        self.assertEqual(kind, "application/octet-stream")
        with self.assertRaisesRegex(ValueError, "outside the exact local set"):
            driver.registry_response("../../etc/passwd", packages, "unused")

    def test_selected_npm_node_is_absolute_verified_file_from_runtime_path(self):
        node = self.root / "node.exe"
        node.write_bytes(b"fixture Node executable")
        node.chmod(0o755)
        with patch.object(driver.shutil, "which", return_value=str(node)) as lookup:
            self.assertEqual(driver.selected_npm_node({"PATH": "selected runtime"}), node.resolve())
        lookup.assert_called_once_with("node", path="selected runtime")

    def test_selected_npm_node_refuses_missing_or_empty_runtime(self):
        with patch.object(driver.shutil, "which", return_value=None):
            with self.assertRaisesRegex(ValueError, "was not found"):
                driver.selected_npm_node({})
        node = self.root / "node.exe"
        node.touch()
        node.chmod(0o755)
        with patch.object(driver.shutil, "which", return_value=str(node)):
            with self.assertRaisesRegex(ValueError, "must be nonempty"):
                driver.selected_npm_node({})

    def test_real_unicode_child_capture_and_log_ignore_host_ansi_locale(self):
        log = self.root / "unicode.log"
        text = "ύ 世界\n"
        command = [sys.executable, "-c", f"import sys; sys.stdout.buffer.write({text.encode('utf-8')!r})"]
        # Python's default subprocess text decoding otherwise uses this ANSI
        # codepage; the UTF-8 byte 0x8d in ύ cannot be decoded as cp1252.
        with patch("locale.getencoding", return_value="cp1252", create=True), \
                patch("locale.getpreferredencoding", return_value="cp1252"):
            output = driver.fixture_output(command, log, dict(os.environ), self.root)
        self.assertEqual(output.returncode, 0)
        self.assertEqual(output.stdout, text)
        self.assertEqual(log.read_bytes(), text.encode("utf-8"))

    def test_timed_out_unicode_child_retains_utf8_diagnostics(self):
        log = self.root / "timeout.log"
        partial = "ύ 世界\n".encode("utf-8") + b"invalid byte: \xff"
        failure = subprocess.TimeoutExpired(["fixture"], 300, output=partial)
        with patch.object(driver.subprocess, "run", side_effect=failure):
            with self.assertRaisesRegex(RuntimeError, "fixture timed out"):
                driver.fixture_output(["fixture"], log, {}, self.root)
        self.assertEqual(log.read_text(encoding="utf-8"),
                         partial.decode("utf-8", errors="replace") + "\nfixture timed out after 300 seconds\n")

    def test_start_failure_retains_utf8_diagnostics(self):
        log = self.root / "startup.log"
        with patch.object(driver.subprocess, "run", side_effect=OSError("absent fixture ύ")):
            with self.assertRaisesRegex(OSError, "absent fixture"):
                driver.fixture_output(["fixture"], log, {}, self.root)
        self.assertEqual(log.read_bytes(), "fixture could not start: absent fixture ύ\n".encode("utf-8"))

    def powershell_shims(self):
        directory = self.root / "installed shims 'quotes'"
        directory.mkdir()
        for name in ("twigsmith", "tgi"):
            (directory / f"{name}.ps1").write_text("# actual installed shim fixture " + name)
        return directory

    @staticmethod
    def powershell_output(command, *_):
        argument = command[-1]
        if argument == "$PSVersionTable.PSVersion.ToString()":
            return subprocess.CompletedProcess(command, 0, "7.6.7\n")
        if argument == "--version":
            return subprocess.CompletedProcess(command, 0, "twigsmith 1.0.0-rc.7\n")
        if argument == "--help":
            return subprocess.CompletedProcess(command, 0, "Twigsmith\nUsage: twigsmith <COMMAND>\n")
        if argument == "start":
            return subprocess.CompletedProcess(command, 2, "start requires a workstream; use --help\n")
        raise AssertionError(f"unexpected PowerShell probe: {command}")

    def test_powershell_probes_launch_both_actual_ps1_paths_under_default_policy(self):
        shims = self.powershell_shims()
        with patch.object(driver.shutil, "which", return_value="C:/PowerShell/pwsh.exe"), \
                patch.object(driver, "fixture_output", side_effect=self.powershell_output) as run:
            report = driver.powershell_shim_probes(shims, "1.0.0-rc.7", self.root, {"PATH": "runtime"})
        self.assertEqual(report["version"], "7.6.7")
        self.assertEqual(len(report["probes"]), 6)
        self.assertTrue(all(probe["passed"] for probe in report["probes"]))
        self.assertEqual(report["shim_sha256"], {name: driver.digest(shims / f"{name}.ps1")
                                               for name in ("twigsmith", "tgi")})
        commands = [call.args[0] for call in run.call_args_list]
        self.assertEqual(commands[1:], [["C:/PowerShell/pwsh.exe", "-NoLogo", "-NoProfile", "-File",
                                        str(shims / f"{name}.ps1"), argument]
                                       for name in ("twigsmith", "tgi")
                                       for argument in ("--version", "--help", "start")])
        self.assertTrue(all("-ExecutionPolicy" not in command for command in commands))

    def test_powershell_usage_status_cannot_pass_when_native_exit_is_lost(self):
        shims = self.powershell_shims()
        def lost_status(command, *args):
            output = self.powershell_output(command, *args)
            output.returncode = 0
            return output
        with patch.object(driver.shutil, "which", return_value="pwsh"), \
                patch.object(driver, "fixture_output", side_effect=lost_status):
            report = driver.powershell_shim_probes(shims, "1.0.0-rc.7", self.root, {})
        self.assertEqual([probe["name"] for probe in report["probes"] if not probe["passed"]],
                         ["powershell-twigsmith-usage", "powershell-tgi-usage"])

    def test_powershell_unavailable_is_an_explicit_qualification_gap(self):
        with patch.object(driver.shutil, "which", return_value=None):
            with self.assertRaisesRegex(ValueError, "PowerShell is required"):
                driver.powershell_shim_probes(self.root, "1.0.0-rc.7", self.root, {})

    def test_powershell_terminal_cases_keep_original_report_and_environment(self):
        original = self.root / "qualification.json"
        original.write_text('original cmd evidence')
        pwsh = self.root / "selected-pwsh"
        pwsh.write_bytes(b"selected PowerShell executable")
        pwsh.chmod(0o755)
        env = {"TWIGSMITH_NPM_SHIM_KIND": "cmd", "PATH": "selected runtime",
               "TWIGSMITH_NPM_SHIM_DIR": str(self.powershell_shims())}
        def fixtures(bridge, harness, evidence, report, **options):
            self.assertEqual(evidence, self.root / "powershell-terminal")
            self.assertEqual(options["selected_cases"], driver.LAUNCHER_CASES)
            self.assertEqual(options["env"]["TWIGSMITH_NPM_SHIM_KIND"], "ps1")
            self.assertEqual(options["env"]["TWIGSMITH_NPM_POWERSHELL"], str(pwsh.resolve()))
            self.assertEqual(options["env"]["TWIGSMITH_NPM_SHIM_TRACE"], str(evidence / "shim-invocations.jsonl"))
            self.assertEqual(options["kind"], "npm")
            report.update(passed=False, tests=[{"name": "failed terminal fixture", "passed": False}])
            return False
        with patch.object(driver, "run_fixtures", side_effect=fixtures), \
                patch.object(driver.shutil, "which", return_value=str(pwsh)):
            report = driver.powershell_terminal_fixtures(self.root, self.root, self.root, env)
        self.assertFalse(report["passed"])
        self.assertEqual(env["TWIGSMITH_NPM_SHIM_KIND"], "cmd")
        self.assertEqual(original.read_text(), 'original cmd evidence')
        self.assertEqual(json.loads((self.root / "powershell-terminal/qualification.json").read_text()), report)

    def shim_trace(self):
        shims = self.powershell_shims()
        pwsh = self.root / "selected-pwsh"
        pwsh.write_bytes(b"selected PowerShell executable")
        pwsh.chmod(0o755)
        entries = [{"kind": "ps1", "shim": str(shims / f"{alias}.ps1"), "pwsh": str(pwsh),
                    "isolated_test": name, "alias": alias}
                   for alias, name in zip(("twigsmith", "tgi", "twigsmith"),
                                          (name for names in driver.LAUNCHER_CASES.values() for name in names))]
        return shims, pwsh, entries

    def test_trace_requires_actual_ps1_invocations_from_every_case_and_both_aliases(self):
        shims, pwsh, entries = self.shim_trace()
        trace = self.root / "trace.jsonl"
        trace.write_text("".join(json.dumps(entry) + "\n" for entry in entries))
        report = driver.powershell_trace_report(trace, shims, pwsh)
        self.assertTrue(report["passed"])
        self.assertEqual(report["entries"], entries)

    def test_trace_refuses_dropped_selector_trace_runtime_or_selected_case(self):
        shims, pwsh, entries = self.shim_trace()
        trace = self.root / "trace.jsonl"
        wrong = self.root / "wrong-pwsh"
        wrong.write_bytes(b"different executable")
        variants = [[], entries[:-1], [dict(entry, kind="cmd") for entry in entries],
                    [dict(entry, pwsh=None) for entry in entries],
                    [dict(entry, pwsh=str(wrong)) for entry in entries],
                    [dict(entry, shim=str(shims / "tgi.ps1")) for entry in entries],
                    [dict(entry, isolated_test="unselected-case") for entry in entries]]
        for changed in variants:
            with self.subTest(entries=changed):
                trace.write_text("".join(json.dumps(entry) + "\n" for entry in changed))
                report = driver.powershell_trace_report(trace, shims, pwsh)
                self.assertFalse(report["passed"])
                self.assertTrue(report["errors"])
        trace.unlink()
        self.assertFalse(driver.powershell_trace_report(trace, shims, pwsh)["passed"])

    def test_passing_terminal_fixtures_without_bridge_trace_are_rejected(self):
        shims, pwsh, _ = self.shim_trace()
        def fixtures(bridge, harness, evidence, report, **options):
            report.update(passed=True, tests=[{"passed": True}])
            return True
        with patch.object(driver, "run_fixtures", side_effect=fixtures), \
                patch.object(driver.shutil, "which", return_value=str(pwsh)):
            report = driver.powershell_terminal_fixtures(self.root, self.root, self.root,
                                                       {"TWIGSMITH_NPM_SHIM_DIR": str(shims)})
        self.assertFalse(report["passed"])
        self.assertFalse(report["shim_trace"]["passed"])

    def test_shell_matrix_has_only_approved_completion_and_launch_routes(self):
        for target in ("x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-apple-darwin"):
            self.assertEqual(driver.supported_shells(target), ("bash", "zsh", "fish"))
        self.assertEqual(driver.supported_shells("x86_64-pc-windows-msvc"), ("powershell", "cmd", "git-bash"))

    def test_windows_git_bash_does_not_select_the_wsl_bash_launcher(self):
        git = self.root / "Git/cmd/git.exe"
        git.parent.mkdir(parents=True)
        git.touch()
        bash = self.root / "Git/bin/bash.exe"
        bash.parent.mkdir()
        bash.touch()
        with patch.object(driver.shutil, "which", return_value=str(git)) as lookup:
            self.assertEqual(driver.shell_executable("git-bash", {}), str(bash))
        lookup.assert_called_once_with("git", path=None)
        bash.unlink()
        with patch.object(driver.shutil, "which", return_value=str(git)):
            self.assertIsNone(driver.shell_executable("git-bash", {}))

    def shell_routes(self, outputs, *, required=True, target="x86_64-unknown-linux-gnu", missing=()):
        def selected(name, _):
            return None if name in missing else "/host/" + name
        with patch.object(driver, "host_target", return_value=target), \
                patch.object(driver, "native_environment", return_value={"PATH": "runtime"}), \
                patch.object(driver, "shell_executable", side_effect=selected), \
                patch.object(driver, "fixture_output", side_effect=outputs):
            return driver.shell_routes(self.root, "1.0.0-rc.7", self.root, require=required)

    @staticmethod
    def shell_output(command, *_):
        if command[-1] == "--version":
            return subprocess.CompletedProcess(command, 0, "host shell 1.2.3\n")
        if command[-1] == "$PSVersionTable.PSVersion.ToString()":
            return subprocess.CompletedProcess(command, 0, "7.6.7\n")
        if "completion" in command:
            return subprocess.CompletedProcess(command, 0, "# generated adapter bytes\n")
        return subprocess.CompletedProcess(command, 0, "SHELL_PROBE_OK\n")

    def test_required_missing_shells_cannot_produce_successful_acceptance(self):
        report = self.shell_routes(self.shell_output, missing=("zsh", "fish"))
        self.assertFalse(report["passed"])
        self.assertFalse(report["coverage_complete"])
        self.assertTrue(report["routes"][0]["passed"])
        self.assertEqual([route["name"] for route in report["routes"] if not route["available"]], ["zsh", "fish"])
        self.assertEqual(json.loads((self.root / "shell-routes/shell-routes.json").read_text()), report)

    def test_optional_missing_shells_remain_incomplete_and_do_not_imply_actual_tab(self):
        report = self.shell_routes(self.shell_output, required=False, missing=("zsh", "fish"))
        self.assertTrue(report["passed"])
        self.assertFalse(report["coverage_complete"])
        self.assertIn("not actual Tab", report["scope"])

    def test_each_shell_loads_actual_generated_adapter_and_checks_both_aliases(self):
        calls = []
        def outputs(command, log, env, cwd):
            calls.append((command, dict(env)))
            return self.shell_output(command, log, env, cwd)
        report = self.shell_routes(outputs)
        self.assertTrue(report["coverage_complete"])
        for route in report["routes"]:
            self.assertEqual([probe["name"] for probe in route["probes"]],
                             ["launch-twigsmith", "launch-tgi", "completion-twigsmith", "completion-tgi"])
            adapter = self.root / f"shell-routes/adapter-{route['name']}.txt"
            self.assertEqual(adapter.read_text(), "# generated adapter bytes\n")
            self.assertEqual(route["adapter_sha256"], driver.digest(adapter))
            self.assertEqual(route["adapter_generation"]["command"],
                             [str(self.root / "twigsmith"), "completion", route["name"]])
        self.assertTrue(all("-ExecutionPolicy" not in command for command, _ in calls))

    def test_zero_status_without_probe_marker_fails_even_in_optional_mode(self):
        def outputs(command, *args):
            result = self.shell_output(command, *args)
            if "bash-launch" in command[-1]:
                result.stdout = ""
            return result
        report = self.shell_routes(outputs, required=False)
        self.assertFalse(report["passed"])
        self.assertFalse(report["routes"][0]["probes"][0]["passed"])

    def test_powershell_other_line_refuses_before_adapter_or_launch(self):
        def outputs(command, *args):
            return subprocess.CompletedProcess(command, 0, "7.5.4\n")
        report = self.shell_routes(outputs, target="x86_64-pc-windows-msvc", missing=("cmd", "git-bash"))
        self.assertFalse(report["passed"])
        self.assertIn("7.6", report["routes"][0]["gap"])
        self.assertEqual(report["routes"][0]["probes"], [])

    def test_windows_requires_powershell_adapter_but_cmd_and_git_bash_launch_only(self):
        def outputs(command, *args):
            if command[-1].endswith("cmd-launch.cmd"):
                return subprocess.CompletedProcess(command, 0, "twigsmith 1.0.0-rc.7\n")
            return self.shell_output(command, *args)
        report = self.shell_routes(outputs, target="x86_64-pc-windows-msvc")
        self.assertTrue(report["coverage_complete"])
        self.assertEqual([len(route["probes"]) for route in report["routes"]], [4, 2, 2])
        self.assertTrue((self.root / "shell-routes/adapter-powershell.ps1").is_file())
        for route in report["routes"]:
            for probe in route["probes"]:
                self.assertNotIn("-ExecutionPolicy", probe["command"])
                if route["name"] == "powershell":
                    self.assertEqual(probe["command"][1:4], ["-NoLogo", "-NoProfile", "-File"])

    def test_rosetta_is_not_native_intel_qualification(self):
        with patch.object(driver.sys, "platform", "darwin"), patch.object(driver.platform, "machine", return_value="x86_64"):
            with patch.object(driver.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)), \
                    patch.object(driver.subprocess, "check_output", return_value="1\n"):
                with self.assertRaisesRegex(ValueError, "Rosetta"):
                    driver.host_target()


if __name__ == "__main__":
    unittest.main()
