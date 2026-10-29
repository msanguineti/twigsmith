"""Qualification-driver failure boundaries; synthetic scripts are not runtime evidence."""
import argparse
import contextlib
import io
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

import qualify_native as driver


class QualificationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        self.artifact = root / "artifact"
        self.harness = root / "harness"
        self.artifact.mkdir()
        self.harness.mkdir()
        for name in ["twigsmith", "tgi"]:
            path = self.artifact / name
            path.write_text("#!/bin/sh\ncase \"$1\" in\n--version) echo fixture;;\nstatus) echo 'Git is installed and available on PATH' >&2; exit 1;;\n*) echo 'use --help' >&2; exit 2;;\nesac\n")
            path.chmod(0o755)
        self.fake = self.harness / "fake"
        self.set_harness("fixture: test", "1 passed; 0 failed")
        self.args = argparse.Namespace(artifact_dir=self.artifact, harness_dir=self.harness,
            evidence_dir=root / "evidence", profile="representative", git=Path(shutil.which("git")))
        self.selection = patch.object(driver, "cases", return_value={"fake": ["fixture"]})
        self.selection.start()
        self.addCleanup(self.selection.stop)

    def set_harness(self, listing, result):
        self.fake.write_text(f"#!/bin/sh\nif [ \"$1\" = --list ]; then echo '{listing}'; else echo '{result}'; fi\n")
        self.fake.chmod(0o755)
        driver.write_json(self.harness / "harnesses.json", {
            "driver_sha256": driver.digest(driver.__file__), "executables": {"fake": driver.digest(self.fake)}})

    def run_driver(self):
        with contextlib.redirect_stdout(io.StringIO()):
            return driver.run(self.args)

    def test_success_binds_both_names_and_reports_runtime_limits(self):
        self.assertEqual(self.run_driver(), 0)
        report = json.loads((self.args.evidence_dir / "qualification.json").read_text())
        self.assertTrue(report["passed"])
        self.assertEqual(set(report["artifact_sha256"]), {"twigsmith", "tgi"})
        self.assertEqual(len(report["tests"]), 7)
        self.assertTrue(report["gaps"])
        self.assertNotIn("cargo", report["runtime_tools"])

    def test_missing_or_linked_alias_never_falls_back(self):
        alias = self.artifact / "tgi"
        alias.unlink()
        with self.assertRaisesRegex(ValueError, "regular executable"):
            self.run_driver()
        report = json.loads((self.args.evidence_dir / "qualification.json").read_text())
        self.assertFalse(report["passed"])
        self.assertIn("error", report)

    def test_loader_startup_failure_retains_failed_evidence(self):
        (self.artifact / "twigsmith").write_text("#!/absent-loader\n")
        with self.assertRaises(OSError):
            self.run_driver()
        report = json.loads((self.args.evidence_dir / "qualification.json").read_text())
        self.assertFalse(report["passed"])
        self.assertIn("error", report)

    def test_changed_harness_cannot_inherit_preparation(self):
        self.fake.write_text("#!/bin/sh\nexit 0\n")
        with self.assertRaisesRegex(ValueError, "changed harness"):
            self.run_driver()

    def test_absent_selected_fixture_is_an_error(self):
        self.set_harness("other: test", "1 passed; 0 failed")
        with self.assertRaisesRegex(ValueError, "selected fixture absent"):
            self.run_driver()

    def test_successful_empty_harness_is_a_failed_check(self):
        self.set_harness("fixture: test", "0 passed; 0 failed")
        self.assertEqual(self.run_driver(), 1)
        self.assertFalse(json.loads((self.args.evidence_dir / "qualification.json").read_text())["passed"])


if __name__ == "__main__":
    unittest.main()
