"""Package-format boundary tests: synthetic ELF bytes never qualify a runtime."""
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import native_package as package


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.output = self.root / "candidate"
        self.output.mkdir()
        # Independently encode ELF64, little endian, ET_DYN, EM_X86_64.
        executable = bytearray(64)
        executable[:6] = bytes([127, 69, 76, 70, 2, 1])
        executable[16:20] = bytes([3, 0, 62, 0])
        self.payload = bytes(executable)
        self.provenance = {
            "kind": "development-probe",
            "source": {"revision": "1" * 40, "tree": "2" * 40, "dirty": False},
            "build": {"target": "x86_64-unknown-linux-gnu", "profile": "release"},
            "toolchain": {"rustc": "rustc fixture", "cargo": "cargo fixture"},
            "environment": {"system": "fixture, no runtime qualification"},
        }
        self.archive = package.write_bundle(
            self.output, "0.1.0-beta.1",
            {"twigsmith": self.payload, "tgi": self.payload,
             "LICENSE": Path(__file__).resolve().parents[1].joinpath("LICENSE").read_bytes()},
            self.provenance, 1,
        )

    def rebind(self, edit_manifest=None):
        manifest_path = self.archive.with_name(self.archive.name[:-7] + ".manifest.json")
        checksum_path = self.archive.with_name(self.archive.name[:-7] + ".sha256")
        manifest = json.loads(manifest_path.read_text())
        manifest["archive"]["sha256"] = hashlib.sha256(self.archive.read_bytes()).hexdigest()
        if edit_manifest:
            edit_manifest(manifest)
        manifest_path.write_text(json.dumps(manifest))
        checksum_path.write_text(
            f"{hashlib.sha256(self.archive.read_bytes()).hexdigest()}  {self.archive.name}\n"
            f"{hashlib.sha256(manifest_path.read_bytes()).hexdigest()}  {manifest_path.name}\n"
        )

    def mutate_archive(self, mutation):
        with tarfile.open(self.archive) as archive:
            entries = [(entry, archive.extractfile(entry).read()) for entry in archive.getmembers()]
        mutation(entries)
        with tarfile.open(self.archive, "w:gz", format=tarfile.USTAR_FORMAT) as archive:
            for entry, data in entries:
                archive.addfile(entry, io.BytesIO(data) if entry.isfile() else None)
        self.rebind()

    def test_round_trip_space_path_regular_executables_and_preserved_modes(self):
        destination = self.root / "outside checkout with spaces"
        manifest = package.verify(self.archive, destination)
        self.assertEqual(manifest["version"], "0.1.0-beta.1")
        extracted = destination / self.archive.name[:-7]
        self.assertEqual(set(path.name for path in extracted.iterdir()),
                         {"twigsmith", "tgi", "LICENSE", "README.txt"})
        for name in ("twigsmith", "tgi"):
            binary = extracted / name
            self.assertFalse(binary.is_symlink())
            self.assertEqual(binary.read_bytes(), self.payload)
            self.assertEqual(stat.S_IMODE(binary.stat().st_mode), 0o755)
        with self.assertRaisesRegex(ValueError, "must not exist"):
            package.verify(self.archive, destination)

    def test_corruption_and_manifest_tampering_fail_before_extraction(self):
        self.archive.write_bytes(self.archive.read_bytes() + b"corruption")
        destination = self.root / "unpublished"
        with self.assertRaisesRegex(ValueError, "checksums"):
            package.verify(self.archive, destination)
        self.assertFalse(destination.exists())

    def test_inconsistent_target_version_and_source_identity_rejected(self):
        for mutation in (
            lambda manifest: manifest.update(target="aarch64-unknown-linux-gnu"),
            lambda manifest: manifest.update(version="0.2.0"),
            lambda manifest: manifest["provenance"]["source"].update(revision="unknown"),
        ):
            with self.subTest(mutation=mutation):
                original = list(self.output.glob("*.manifest.json"))[0].read_bytes()
                self.rebind(mutation)
                with self.assertRaises(ValueError):
                    package.verify(self.archive)
                list(self.output.glob("*.manifest.json"))[0].write_bytes(original)
                self.rebind()

    def test_ci_manifest_binds_clean_source_to_selected_run(self):
        ci = {"GITHUB_SHA": "1" * 40, "GITHUB_REPOSITORY": "owner/repository",
              "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1"}
        def make_candidate(manifest):
            manifest["provenance"].update(kind="ci-candidate")
            manifest["provenance"]["environment"]["ci"] = ci.copy()
        self.rebind(make_candidate)
        self.assertEqual(package.verify(self.archive)["provenance"]["kind"], "ci-candidate")
        self.rebind(lambda manifest: manifest["provenance"]["source"].update(dirty=True))
        with self.assertRaisesRegex(ValueError, "CI candidate"):
            package.verify(self.archive)
        self.rebind(lambda manifest: manifest["provenance"]["source"].update(dirty=False))
        self.rebind(lambda manifest: manifest["provenance"]["environment"]["ci"].update(GITHUB_SHA="2" * 40))
        with self.assertRaisesRegex(ValueError, "CI candidate"):
            package.verify(self.archive)

    def test_links_traversal_duplicates_and_nonexecutable_modes_rejected(self):
        original = self.archive.read_bytes()
        def symlink(entries):
            entries[0][0].type = tarfile.SYMTYPE
            entries[0][0].linkname = "/bin/true"
            entries[0][0].size = 0
        mutations = [symlink,
                     lambda entries: setattr(entries[0][0], "name", "../escaped"),
                     lambda entries: entries.append(entries[0]),
                     lambda entries: setattr(entries[0][0], "mode", 0o644)]
        for mutation in mutations:
            with self.subTest(mutation=mutation):
                self.archive.write_bytes(original)
                self.mutate_archive(mutation)
                with self.assertRaises(ValueError):
                    package.verify(self.archive, self.root / "never extracted")
                self.assertFalse((self.root / "never extracted").exists())

    def test_same_length_payload_change_rejected_even_with_rebound_archive_hash(self):
        def change(entries):
            entry, data = entries[0]
            entries[0] = entry, data[:-1] + b"x"
        self.mutate_archive(change)
        with self.assertRaisesRegex(ValueError, "payload hash"):
            package.verify(self.archive)

    def test_alias_must_be_native_target_elf(self):
        for data in (b"#!/bin/sh\nexec twigsmith\n", self.payload[:18] + b"\xb7\x00" + self.payload[20:]):
            with self.assertRaisesRegex(ValueError, "ELF64 x86_64"):
                package.validate_elf(data)

    def test_wrong_target_and_existing_output_reject_before_build(self):
        repo = Path(__file__).resolve().parents[1]
        with self.assertRaisesRegex(ValueError, "target"):
            package.build(repo, self.root / "unused", False, "aarch64-unknown-linux-gnu")
        with self.assertRaisesRegex(ValueError, "must not exist"):
            package.build(repo, self.output, False, package.TARGET)


class SourceIdentityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        isolated = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        isolated.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GITHUB_ACTIONS="false")
        environment = patch.dict(os.environ, isolated, clear=True)
        environment.start()
        self.addCleanup(environment.stop)
        self.git("init", "--initial-branch=main")
        self.git("config", "user.name", "Package fixture")
        self.git("config", "user.email", "package@example.invalid")
        self.source = self.repo / "source.rs"
        self.source.write_text("committed source\n")
        self.source.chmod(0o644)
        self.git("add", "source.rs")
        self.git("commit", "-m", "fixture")

    def git(self, *arguments):
        return subprocess.check_output(["git", *arguments], cwd=self.repo, text=True).strip()

    def test_ordinary_git_metadata_records_clean_and_dirty_source(self):
        original = package.identity(self.repo)
        self.assertEqual(original, {"revision": self.git("rev-parse", "HEAD"),
                                    "tree": self.git("rev-parse", "HEAD^{tree}"),
                                    "dirty": False})
        self.source.write_text("modified source\n")
        self.assertTrue(package.identity(self.repo)["dirty"])
        output = self.repo / "candidate"
        with self.assertRaisesRegex(ValueError, "source is dirty"):
            package.build(self.repo, output, False, package.TARGET)
        self.assertFalse(output.exists())
        with patch.object(package, "package_metadata", side_effect=RuntimeError("probe permitted")):
            with self.assertRaisesRegex(RuntimeError, "probe permitted"):
                package.build(self.repo, output, True, package.TARGET)

    def test_local_builds_are_development_probes_even_when_clean(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false"}):
            self.assertEqual(package.provenance_kind(package.identity(self.repo)), "development-probe")

    def test_ci_candidate_requires_selected_clean_source_and_run_identity(self):
        source = package.identity(self.repo)
        ci = {"GITHUB_ACTIONS": "true", "GITHUB_SHA": source["revision"],
              "GITHUB_REPOSITORY": "owner/repository", "GITHUB_RUN_ID": "123",
              "GITHUB_RUN_ATTEMPT": "1"}
        with patch.dict(os.environ, ci):
            self.assertEqual(package.provenance_kind(source), "ci-candidate")
            for overrides, message in (({"dirty": True}, "clean checkout"),
                                       ({"revision": "1" * 40}, "GITHUB_SHA")):
                with self.subTest(overrides=overrides):
                    with self.assertRaisesRegex(ValueError, message):
                        package.provenance_kind(source | overrides)
            with patch.dict(os.environ, {"GITHUB_RUN_ID": ""}):
                with self.assertRaisesRegex(ValueError, "run identity"):
                    package.provenance_kind(source)

    def test_failed_build_publishes_no_output(self):
        (self.repo / "Cargo.toml").write_text('[package]\nname = "twigsmith"\nversion = "0.1.0"\n')
        (self.repo / "Cargo.lock").write_text('name = "twigsmith"\nversion = "0.1.0"\n')
        (self.repo / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.99.0"\n')
        (self.repo / ".gitignore").write_text('.dev/\n')
        self.git("add", ".")
        self.git("commit", "-m", "build fixture")
        output = self.repo / "candidate"
        source = package.identity(self.repo)
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false"}), \
             patch.object(package, "identity", return_value=source), \
             patch.object(package, "git_output", return_value="1"), \
             patch.object(package, "run", side_effect=["rustc 1.99.0 fixture\nhost: " + package.TARGET,
                                                        "cargo fixture"]), \
             patch.object(subprocess, "run", side_effect=subprocess.CalledProcessError(1, "cargo")):
            with self.assertRaises(subprocess.CalledProcessError):
                package.build(self.repo, output, False, package.TARGET)
        self.assertFalse(output.exists())
        self.assertFalse((self.repo / ".dev/tmp/distribution-phase-2").exists())
        self.assertEqual(list((self.repo / ".dev/tmp/native-build").iterdir()), [])


if __name__ == "__main__":
    unittest.main()
