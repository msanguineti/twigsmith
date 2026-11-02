"""Package-format boundary tests: synthetic ELF bytes never qualify a runtime."""
from copy import deepcopy
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
import struct
import zipfile
import warnings
from unittest.mock import patch

import native_package as package


class CurrentRequirementsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        (self.repo / "site").mkdir()
        self.path = self.repo / "site/current-requirements.json"
        self.current = json.loads((Path(__file__).resolve().parents[1]
                                   / "site/current-requirements.json").read_text())
        self.path.write_text(json.dumps(self.current))

    def test_selects_exact_snapshot_without_envelope_or_mutation(self):
        self.assertEqual(self.current["schema"], 2)
        self.assertEqual(set(self.current["targets"]), set(package.TARGETS))
        for target, snapshot in self.current["targets"].items():
            with self.subTest(target=target):
                selected = package.load_requirements(self.repo, target)
                self.assertEqual(selected, snapshot)
                self.assertEqual(selected["schema"], 1)
                selected["native"]["requirements"]["system"] = "caller mutation"
                self.assertEqual(package.load_requirements(self.repo, target), snapshot)

    def test_rejects_incomplete_extra_misassigned_or_wrong_npm_targets(self):
        windows = "x86_64-pc-windows-msvc"
        for mutation in (
            lambda value: value["targets"].pop(windows),
            lambda value: value["targets"].update(unknown=value["targets"][package.TARGET]),
            lambda value: value["targets"][windows]["native"]["platform"].update(os="linux"),
            lambda value: value["targets"][windows]["npm"].update(
                coverage=value["targets"][package.TARGET]["npm"]["coverage"]),
            lambda value: value["targets"][windows]["npm"]["coverage"].append(
                {"os": "darwin", "arch": "arm64", "libc": None}),
            lambda value: value.update(schema=True),
        ):
            changed = deepcopy(self.current)
            mutation(changed)
            self.path.write_text(json.dumps(changed))
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                # Validate the entire current policy even when another target is selected.
                package.load_requirements(self.repo)

    def test_legacy_current_file_remains_linux_only(self):
        snapshot = self.current["targets"][package.TARGET]
        self.path.write_text(json.dumps(snapshot))
        self.assertEqual(package.load_requirements(self.repo), snapshot)
        with self.assertRaisesRegex(ValueError, "only the Linux target"):
            package.load_requirements(self.repo, "x86_64-pc-windows-msvc")


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
            if os.name != "nt":
                self.assertEqual(stat.S_IMODE(binary.stat().st_mode), 0o755)
        with self.assertRaisesRegex(ValueError, "must not exist"):
            package.verify(self.archive, destination)

    def test_frozen_requirements_survive_current_source_changes(self):
        before = package.verify(self.archive)
        changed = deepcopy(before["requirements"])
        changed["native"]["requirements"]["system"] = "A later supported system"
        changed["npm"]["requirements"] = "A later npm runtime"
        # Model an actual later edit to the current source after the archive exists.
        current = self.root / "current-requirements.json"
        current.write_text(json.dumps(before["requirements"]))
        current.write_text(json.dumps(changed))
        with patch.object(package, "load_requirements", side_effect=lambda _: json.loads(current.read_text())):
            after = package.verify(self.archive)
        self.assertEqual(before["requirements"], after["requirements"])
        descriptor = {"version": before["version"],
                      "native": [deepcopy(before["requirements"]["native"])],
                      "npm": deepcopy(before["requirements"]["npm"])}
        self.assertTrue(package.requirements_match_descriptor(before, descriptor))
        descriptor["native"][0]["requirements"]["system"] = "different"
        self.assertFalse(package.requirements_match_descriptor(before, descriptor))

    def test_descriptor_matching_allows_other_platform_snapshot_without_changing_builder(self):
        manifest = package.verify(self.archive)
        manifest["requirements"]["native"] = {
            "platform": {"os": "future", "arch": "arm64", "libc": None},
            "requirements": {"system": "Example fixture platform", "runtime": "Example runtime"}}
        descriptor = {"version": manifest["version"],
                      "native": [deepcopy(manifest["requirements"]["native"])],
                      "npm": deepcopy(manifest["requirements"]["npm"])}
        self.assertTrue(package.requirements_match_descriptor(manifest, descriptor))
        descriptor["native"][0]["requirements"]["system"] = "different"
        self.assertFalse(package.requirements_match_descriptor(manifest, descriptor))
        with self.assertRaisesRegex(ValueError, "native builder"):
            package.validate_requirements(manifest["requirements"], native_builder=True)

    def test_legacy_manifest_and_guidance_stay_verifiable(self):
        root = self.root / "legacy"
        root.mkdir()
        with patch.object(package, "guidance", side_effect=lambda version, stem, _: package.GUIDANCE.format(version=version, stem=stem)):
            archive = package.write_bundle(root, "0.1.0-beta.1",
                {"twigsmith": self.payload, "tgi": self.payload,
                 "LICENSE": Path(__file__).resolve().parents[1].joinpath("LICENSE").read_bytes()}, self.provenance, 1)
        self.archive = archive
        def legacy(manifest):
            manifest["schema"] = 2
            manifest.pop("requirements")
        self.rebind(legacy)
        with patch.object(package, "load_requirements", side_effect=AssertionError("current requirements read")):
            manifest = package.verify(archive)
        self.assertEqual(manifest["schema"], 2)
        self.assertFalse(package.requirements_match_descriptor(manifest, {}))

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

    @patch.object(package.platform, "machine", return_value="x86_64")
    @patch.object(package.platform, "system", return_value="Linux")
    def test_wrong_target_and_existing_output_reject_before_build(self, *_):
        repo = Path(__file__).resolve().parents[1]
        with self.assertRaisesRegex(ValueError, "target"):
            package.build(repo, self.root / "unused", False, "aarch64-unknown-linux-gnu")
        with self.assertRaisesRegex(ValueError, "must not exist"):
            package.build(repo, self.output, False, package.TARGET)


def pe_fixture():
    """Independent minimal PE32+ image with one imported Windows system DLL."""
    data = bytearray(1024)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 60, 128)
    data[128:132] = b"PE\0\0"
    struct.pack_into("<HH", data, 132, 0x8664, 1)
    struct.pack_into("<HH", data, 148, 240, 2)
    struct.pack_into("<H", data, 152, 0x20b)
    struct.pack_into("<HH", data, 200, 6, 0)
    struct.pack_into("<H", data, 220, 3)
    struct.pack_into("<I", data, 260, 16)
    struct.pack_into("<II", data, 272, 4096, 40)
    data[392:400] = b".rdata\0\0"
    struct.pack_into("<IIII", data, 400, 512, 4096, 512, 512)
    struct.pack_into("<I", data, 524, 4160)
    data[576:589] = b"KERNEL32.dll\0"
    return bytes(data)


def macho_fixture(arch):
    """Independent MH_EXECUTE plus dylib and LC_BUILD_VERSION commands."""
    name = b"/usr/lib/libSystem.B.dylib\0"
    width = (24 + len(name) + 7) // 8 * 8
    library = struct.pack("<IIIIII", 0xc, width, 24, 0, 0, 0) + name
    library += b"\0" * (width - len(library))
    deployment = struct.pack("<IIIIII", 0x32, 24, 1, 0x0b0000, 0x0f0000, 0)
    loader = struct.pack("<III", 0xe, 32, 12) + b"/usr/lib/dyld\0" + b"\0" * 6
    cpu = 0x100000c if arch == "aarch64" else 0x1000007
    return struct.pack("<IIIIIIII", 0xfeedfacf, cpu, 0, 2, 3, width + 24 + 32, 0, 0) + library + deployment + loader


class NewPlatformTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def candidate(self, target):
        metadata = package.target_metadata(target)
        data = pe_fixture() if metadata["os"] == "windows" else macho_fixture(metadata["arch"])
        provenance = {"kind": "development-probe",
                      "source": {"revision": "1" * 40, "tree": "2" * 40, "dirty": False},
                      "build": {"target": target, "profile": "release", "locked": True, "compiler": "fixture"},
                      "toolchain": {"pin": "fixture", "rustc": "rustc fixture\nhost: " + target, "cargo": "cargo fixture"},
                      "environment": {"system": "fixture only", "machine": metadata["arch"]}}
        output = self.root / target
        output.mkdir()
        archive = package.write_bundle(output, "0.1.0", {name: data for name in metadata["executables"]}
                                       | {"LICENSE": b"MIT License\nfixture"}, provenance, 1, target=target)
        return archive

    def rebind(self, archive, edit=None):
        stem = package.archive_stem(archive)
        path = archive.with_name(stem + ".manifest.json")
        manifest = json.loads(path.read_text())
        manifest["archive"]["sha256"] = package.digest(archive.read_bytes())
        if edit:
            edit(manifest)
        path.write_text(json.dumps(manifest))
        archive.with_name(stem + ".sha256").write_text(
            f"{package.digest(archive.read_bytes())}  {archive.name}\n"
            f"{package.digest(path.read_bytes())}  {path.name}\n")

    def test_all_new_targets_roundtrip_with_target_npm_coverage(self):
        coverage = {
            "x86_64-pc-windows-msvc": {"os": "win32", "arch": "x64", "libc": None},
            "aarch64-apple-darwin": {"os": "darwin", "arch": "arm64", "libc": None},
            "x86_64-apple-darwin": {"os": "darwin", "arch": "x64", "libc": None},
        }
        for target in ("x86_64-pc-windows-msvc", "aarch64-apple-darwin", "x86_64-apple-darwin"):
            with self.subTest(target=target):
                archive = self.candidate(target)
                destination = self.root / (target + " extracted with spaces")
                manifest = package.verify(archive, destination)
                self.assertEqual(manifest["schema"], 4)
                self.assertEqual(manifest["target"], target)
                self.assertEqual(set(manifest["files"]), set(package.members_for(target)))
                self.assertEqual(manifest["requirements"]["npm"]["coverage"],
                                 [coverage[target]])
                root = destination / package.archive_stem(archive)
                readme = (root / "README.txt").read_text()
                for value in manifest["requirements"]["native"]["requirements"].values():
                    self.assertIn(value, readme)
                self.assertIn("Selected support targets are policy", readme)
                self.assertNotIn("OS support floor is unqualified", readme)
                for name in package.executable_names(target):
                    self.assertEqual(package.inspect_payload((root / name).read_bytes(), target),
                                     manifest["inspection"][name])
                if target.endswith("windows-msvc"):
                    self.assertIn("Windows Server 2025 CI", readme)
                    self.assertIn("do not represent tests on a Windows 11 desktop", readme)
                    self.assertEqual(manifest["inspection"]["twigsmith.exe"]["imports"], ["KERNEL32.dll"])
                    self.assertEqual(manifest["npm_payload"], "twigsmith.exe")
                else:
                    self.assertEqual(manifest["inspection"]["twigsmith"]["deployment"],
                                     {"minimum": "11.0.0", "sdk": "15.0.0"})

    def test_historical_unqualified_platform_snapshots_and_readme_still_verify(self):
        for target in ("x86_64-pc-windows-msvc", "aarch64-apple-darwin", "x86_64-apple-darwin"):
            frozen = package.load_requirements(Path(__file__).resolve().parents[1], target)
            frozen["native"]["requirements"] = {
                "system": "Candidate platform; OS support floor is unqualified",
                "shell": "Git 2.45 or newer; native execution needs no Node/npm or Rust"}
            frozen["npm"] = {
                "coverage": [{"os": "linux", "arch": "x64", "libc": "glibc"}],
                "requirements": "Node.js LTS and npm; Linux x86_64 with glibc 2.35+ and Git 2.45+"}
            with patch.object(package, "load_requirements", return_value=frozen), \
                 patch.object(package, "candidate_guidance", side_effect=package.legacy_platform_guidance):
                archive = self.candidate(target)
            with patch.object(package, "load_requirements", side_effect=AssertionError("current policy read")):
                manifest = package.verify(archive)
            self.assertEqual(manifest["requirements"], frozen)

    def test_windows_archive_rejects_unsafe_forms_before_extraction(self):
        archive = self.candidate("x86_64-pc-windows-msvc")
        original = archive.read_bytes()
        mutations = [lambda entries: setattr(entries[0][0], "filename", "../escape.exe"),
                     lambda entries: setattr(entries[0][0], "filename", entries[0][0].filename.replace("/", "\\")),
                     lambda entries: setattr(entries[0][0], "external_attr", (stat.S_IFLNK | 0o777) << 16),
                     lambda entries: setattr(entries[0][0], "external_attr", ((stat.S_IFREG | 0o755) << 16) | 0x400),
                     lambda entries: entries.append(entries[0]),
                     lambda entries: entries.pop()]
        for mutation in mutations:
            with self.subTest(mutation=mutation):
                archive.write_bytes(original)
                with zipfile.ZipFile(archive) as zipped:
                    entries = [(entry, zipped.read(entry)) for entry in zipped.infolist()]
                mutation(entries)
                with warnings.catch_warnings(), zipfile.ZipFile(archive, "w") as zipped:
                    warnings.simplefilter("ignore", UserWarning)
                    for entry, data in entries:
                        zipped.writestr(entry, data)
                self.rebind(archive)
                destination = self.root / "never extracted"
                with self.assertRaises(ValueError):
                    package.verify(archive, destination)
                self.assertFalse(destination.exists())

    def test_inspection_and_platform_provenance_are_bound_to_payload(self):
        archive = self.candidate("aarch64-apple-darwin")
        for edit in (lambda value: value["inspection"]["twigsmith"]["deployment"].update(minimum="1.0.0"),
                     lambda value: value["requirements"]["native"]["platform"].update(arch="x86_64"),
                     lambda value: value["provenance"]["environment"].update(machine="x86_64")):
            original = archive.with_name(package.archive_stem(archive) + ".manifest.json").read_bytes()
            self.rebind(archive, edit)
            with self.assertRaises(ValueError):
                package.verify(archive)
            archive.with_name(package.archive_stem(archive) + ".manifest.json").write_bytes(original)
            self.rebind(archive)

    def test_executable_architecture_kind_and_dependency_structures_are_inspected(self):
        data = bytearray(pe_fixture())
        for offset, value in ((132, 0xaa64), (148, 1), (150, 0x2002), (152, 0x10b)):
            changed = bytearray(data)
            struct.pack_into("<H", changed, offset, value)
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                package.inspect_payload(bytes(changed), "x86_64-pc-windows-msvc")
        changed = bytearray(data)
        struct.pack_into("<I", changed, 524, 0x9000)
        with self.assertRaisesRegex(ValueError, "RVA"):
            package.inspect_payload(bytes(changed), "x86_64-pc-windows-msvc")
        with self.assertRaisesRegex(ValueError, "Mach-O"):
            package.inspect_payload(macho_fixture("aarch64"), "x86_64-apple-darwin")
        for changed in (macho_fixture("aarch64")[:-1],
                        macho_fixture("aarch64").replace(b"/usr/lib/", b"/tmp/lib/")):
            with self.assertRaises(ValueError):
                package.inspect_payload(changed, "aarch64-apple-darwin")

    def test_windows_msvc_runtime_and_delay_import_evidence(self):
        data = bytearray(pe_fixture())
        struct.pack_into("<II", data, 368, 4192, 64)
        struct.pack_into("<II", data, 608, 1, 4288)
        runtime = b"VCRUNTIME140.dll\0"
        data[704:704 + len(runtime)] = runtime
        report = package.inspect_payload(bytes(data), "x86_64-pc-windows-msvc")
        self.assertEqual(report["delay_imports"], ["VCRUNTIME140.dll"])
        self.assertEqual(report["msvc_runtime_imports"], ["VCRUNTIME140.dll"])
        struct.pack_into("<I", data, 608, 0)
        with self.assertRaisesRegex(ValueError, "VA-based"):
            package.inspect_payload(bytes(data), "x86_64-pc-windows-msvc")

    @patch.object(package.platform, "machine", return_value="x86_64")
    @patch.object(package.platform, "system", return_value="Linux")
    def test_new_targets_refuse_cross_build_before_running_tools(self, *_):
        with patch.object(subprocess, "run", side_effect=AssertionError("must not build")):
            for target in ("x86_64-pc-windows-msvc", "aarch64-apple-darwin", "x86_64-apple-darwin"):
                with self.subTest(target=target), self.assertRaisesRegex(ValueError, "matching host"):
                    package.build(self.root, self.root / "unused", False, target)


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

    @patch.object(package.platform, "machine", return_value="x86_64")
    @patch.object(package.platform, "system", return_value="Linux")
    def test_ordinary_git_metadata_records_clean_and_dirty_source(self, *_):
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

    @patch.object(package.platform, "machine", return_value="x86_64")
    @patch.object(package.platform, "system", return_value="Linux")
    def test_compiler_overrides_cleared_and_failed_build_publishes_no_output(self, *_):
        (self.repo / "site").mkdir()
        (self.repo / "site/current-requirements.json").write_text((Path(__file__).resolve().parents[1] / "site/current-requirements.json").read_text())
        (self.repo / "Cargo.toml").write_text('[package]\nname = "twigsmith"\nversion = "0.1.0"\n')
        (self.repo / "Cargo.lock").write_text('name = "twigsmith"\nversion = "0.1.0"\n')
        (self.repo / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.99.0"\n')
        (self.repo / ".gitignore").write_text('.dev/\n')
        self.git("add", ".")
        self.git("commit", "-m", "build fixture")
        output = self.repo / "candidate"
        source = package.identity(self.repo)
        overrides = {"GITHUB_ACTIONS": "false", "RUSTC": "uninspected-rustc",
                     "CARGO_BUILD_RUSTC": "another-uninspected-rustc",
                     "RUSTC_WRAPPER": "untrusted-wrapper", "RUSTC_WORKSPACE_WRAPPER": "workspace-wrapper",
                     "CARGO_BUILD_RUSTC_WRAPPER": "config-wrapper",
                     "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER": "config-workspace-wrapper"}
        with patch.dict(os.environ, overrides), \
             patch.object(package, "identity", return_value=source), \
             patch.object(package, "git_output", return_value="1"), \
             patch.object(package, "run", side_effect=["rustc 1.99.0 fixture\nhost: " + package.TARGET,
                                                        "cargo fixture"]) as inspected_tools, \
             patch.object(subprocess, "run", side_effect=subprocess.CalledProcessError(1, "cargo")) as cargo_build:
            with self.assertRaises(subprocess.CalledProcessError):
                package.build(self.repo, output, False, package.TARGET)
        environment = cargo_build.call_args.kwargs["env"]
        inspected_rustc = inspected_tools.call_args_list[0].args[0][0]
        self.assertTrue(Path(inspected_rustc).is_absolute())
        self.assertEqual(environment["RUSTC"], inspected_rustc)
        self.assertEqual(environment["CARGO_BUILD_RUSTC"], inspected_rustc)
        for name in overrides:
            if "WRAPPER" in name:
                self.assertEqual(environment[name], "")
        self.assertFalse(output.exists())
        self.assertFalse((self.repo / ".dev/tmp/distribution-phase-2").exists())
        self.assertEqual(list((self.repo / ".dev/tmp/native-build").iterdir()), [])

    @patch.object(package.platform, "machine", return_value="x86_64")
    @patch.object(package.platform, "system", return_value="Linux")
    def test_target_compiler_and_sdk_overrides_fail_before_build(self, *_):
        source = package.identity(self.repo)
        requirements = package.load_requirements(Path(__file__).resolve().parents[1])
        with patch.object(package, "identity", return_value=source), \
             patch.object(package, "package_metadata", return_value="0.1.0"), \
             patch.object(package, "load_requirements", return_value=requirements), \
             patch.object(package, "run", side_effect=AssertionError("must reject before tool invocation")):
            for name in ("CC_x86_64_unknown_linux_gnu", "CFLAGS_x86_64-unknown-linux-gnu", "TARGET_CC",
                         "SDKROOT", "MACOSX_DEPLOYMENT_TARGET"):
                with self.subTest(name=name), patch.dict(os.environ, {name: "unrecorded-override"}):
                    with self.assertRaisesRegex(ValueError, "unsupported build override"):
                        package.build(self.repo, self.repo / "unused", False, package.TARGET)


if __name__ == "__main__":
    unittest.main()
