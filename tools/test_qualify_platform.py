"""Driver integrity and false-positive boundaries; synthetic fixtures are not native evidence."""
import argparse
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

    def test_rosetta_is_not_native_intel_qualification(self):
        with patch.object(driver.sys, "platform", "darwin"), patch.object(driver.platform, "machine", return_value="x86_64"):
            with patch.object(driver.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)), \
                    patch.object(driver.subprocess, "check_output", return_value="1\n"):
                with self.assertRaisesRegex(ValueError, "Rosetta"):
                    driver.host_target()


if __name__ == "__main__":
    unittest.main()
