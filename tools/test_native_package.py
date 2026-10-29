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
            "source": {"revision": "1" * 40, "tree": "2" * 40,
                       "dirty": False, "snapshot_sha256": "3" * 64},
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

    def test_wrong_target_dirty_tree_and_existing_output_reject_before_build(self):
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
        isolated.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
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

    def test_hidden_content_changes_are_dirty_and_default_build_rejects(self):
        for flag in ("--assume-unchanged", "--skip-worktree"):
            with self.subTest(flag=flag):
                self.source.write_text("committed source\n")
                self.git("update-index", "--no-assume-unchanged", "--no-skip-worktree", "source.rs")
                clean = package.identity(self.repo)
                self.assertFalse(clean["dirty"])
                self.git("update-index", flag, "source.rs")
                self.source.write_text("modified source!\n")
                self.assertEqual(self.git("status", "--porcelain"), "")
                changed = package.identity(self.repo)
                self.assertTrue(changed["dirty"])
                self.assertNotEqual(changed["snapshot_sha256"], clean["snapshot_sha256"])
                self.assertEqual(changed["revision"], clean["revision"])
                output = self.repo / "candidate"
                with self.assertRaisesRegex(ValueError, "source is dirty"):
                    package.build(self.repo, output, False, package.TARGET)
                self.assertFalse(output.exists())
                # Dirty probes explicitly bypass only the clean-source gate.
                with patch.object(package, "package_metadata", side_effect=RuntimeError("dirty probe permitted")):
                    with self.assertRaisesRegex(RuntimeError, "dirty probe permitted"):
                        package.build(self.repo, output, True, package.TARGET)

    def test_hidden_type_and_executable_changes_are_dirty(self):
        for flag in ("--assume-unchanged", "--skip-worktree"):
            for change in ("executable", "symlink", "deleted"):
                with self.subTest(flag=flag, change=change):
                    if self.source.is_symlink() or self.source.exists():
                        self.source.unlink()
                    self.source.write_text("committed source\n")
                    self.source.chmod(0o644)
                    self.git("update-index", "--no-assume-unchanged", "--no-skip-worktree", "source.rs")
                    self.git("update-index", flag, "source.rs")
                    if change == "executable":
                        self.source.chmod(0o755)
                    else:
                        self.source.unlink()
                        if change == "symlink":
                            self.source.symlink_to("outside.rs")
                    self.assertEqual(self.git("status", "--porcelain"), "")
                    self.assertTrue(package.identity(self.repo)["dirty"])

    def test_replace_refs_cannot_attest_replacement_bytes_as_original_source(self):
        original = self.git("rev-parse", "HEAD")
        original_tree = self.git("rev-parse", "HEAD^{tree}")
        original_epoch = self.git("show", "-s", "--format=%ct", original)
        original_identity = package.identity(self.repo)
        self.source.write_text("replacement source\n")
        self.git("add", "source.rs")
        replacement_tree = self.git("write-tree")
        with patch.dict(os.environ, {"GIT_AUTHOR_DATE": "@946684800 +0000",
                                     "GIT_COMMITTER_DATE": "@946684800 +0000"}):
            replacement = self.git("commit-tree", replacement_tree, "-p", original, "-m", "replacement")
        self.git("replace", original, replacement)
        self.assertEqual(self.git("rev-parse", "HEAD"), original)
        self.assertEqual(self.git("rev-parse", "HEAD^{tree}"), replacement_tree)
        self.assertEqual(self.git("status", "--porcelain"), "")
        actual = package.identity(self.repo)
        self.assertEqual(actual["revision"], original)
        self.assertEqual(actual["tree"], original_tree)
        self.assertEqual(package.git_output(self.repo, "show", "-s", "--format=%ct", original), original_epoch)
        self.assertTrue(actual["dirty"])
        self.assertNotEqual(actual["snapshot_sha256"], original_identity["snapshot_sha256"])
        with self.assertRaisesRegex(ValueError, "source is dirty"):
            package.build(self.repo, self.repo / "candidate", False, package.TARGET)
        # Restoring the actual original tree is clean even while replacement
        # refs remain active; index/object reads must use the same boundary.
        self.git("--no-replace-objects", "read-tree", original)
        self.source.write_text("committed source\n")
        restored = package.identity(self.repo)
        self.assertEqual(restored, original_identity)

    def test_unsupported_source_types_fail_closed_even_for_dirty_probes(self):
        self.git("update-index", "--skip-worktree", "source.rs")
        self.source.unlink()
        self.source.mkdir()
        with self.assertRaisesRegex(ValueError, "unsupported source entry"):
            package.identity(self.repo)
        with self.assertRaisesRegex(ValueError, "unsupported source entry"):
            package.build(self.repo, self.repo / "candidate", True, package.TARGET)


if __name__ == "__main__":
    unittest.main()
