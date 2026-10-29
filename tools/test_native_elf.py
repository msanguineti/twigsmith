"""Real-ELF regression checks for candidate dependency inspection (not release evidence)."""
import importlib.util
import json
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "scripts/ci/inspect-native-elf.py"
spec = importlib.util.spec_from_file_location("inspect_native_elf", SCRIPT)
inspector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inspector)


class NativeElfTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="twigsmith-elf-tests-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.artifact = self.root / "extracted package"
        self.artifact.mkdir()
        self.evidence = self.root / "evidence"
        self.cc = shutil.which("cc")
        self.assertIsNotNone(self.cc, "ELF regressions require the documented C compiler")

    def build(self, source, arguments=()):
        source_path = self.root / "program.c"
        source_path.write_text(source)
        executable = self.artifact / "twigsmith"
        subprocess.run([self.cc, str(source_path), *arguments, "-o", str(executable)],
                       check=True, capture_output=True)
        shutil.copy2(executable, self.artifact / "tgi")
        return executable

    def version_fixture(self, weak):
        # Link a versioned reference against a private build-time libm impostor.
        # The actual userspace libm deliberately does not provide this version.
        source = self.root / "library.c"
        mapping = self.root / "library.map"
        source.write_text("int test_value(void) { return 1; }\n")
        mapping.write_text("TWIGSMITH_TEST_1 { global: test_value; };\n")
        subprocess.run([self.cc, "-shared", "-fPIC", str(source), "-Wl,-soname,libm.so.6",
                        f"-Wl,--version-script={mapping}", "-o", str(self.root / "libm.so.6")],
                       check=True, capture_output=True)
        executable = self.build("extern int test_value(void); int main(void) { return test_value(); }\n",
                                [f"-L{self.root}", "-l:libm.so.6"])
        if weak:
            # VER_FLG_WEAK is metadata on a real ELF verneed entry. GNU ld emits
            # mandatory needs here, so set this flag directly to exercise the
            # distinct weak-version boundary without depending on a Rust build.
            data = bytearray(executable.read_bytes())
            section_offset = struct.unpack_from("<Q", data, 40)[0]
            section_size, section_count = struct.unpack_from("<HH", data, 58)
            headers = [struct.unpack_from("<IIQQQQIIQQ", data, section_offset + i * section_size)
                       for i in range(section_count)]
            changed = False
            for section in headers:
                if section[1] != 0x6ffffffe:  # SHT_GNU_verneed
                    continue
                strings = headers[section[6]][4]
                entry = section[4]
                while True:
                    _, count, _, auxiliary, following = struct.unpack_from("<HHIII", data, entry)
                    need = entry + auxiliary
                    for _ in range(count):
                        _, _, _, name_offset, next_need = struct.unpack_from("<IHHII", data, need)
                        start = strings + name_offset
                        name = bytes(data[start:data.index(0, start)]).decode()
                        if name == "TWIGSMITH_TEST_1":
                            struct.pack_into("<H", data, need + 4, 2)
                            changed = True
                        need += next_need
                    if not following:
                        break
                    entry += following
            self.assertTrue(changed)
            executable.write_bytes(data)
            shutil.copy2(executable, self.artifact / "tgi")

    def report(self):
        return json.loads((self.evidence / "elf.json").read_text())

    def test_real_binary_binds_both_names_and_resolved_library_versions(self):
        self.build('extern int puts(const char *); int main(void) { return puts("fixture") < 0; }\n')
        result = inspector.inspect(self.artifact, self.evidence)
        self.assertTrue(result["passed"])
        self.assertEqual(set(result["executables"]), {"twigsmith", "tgi"})
        for name, executable in result["executables"].items():
            self.assertEqual(executable["sha256"], inspector.digest(self.artifact / name))
            self.assertEqual(executable["interpreters"], [inspector.INTERPRETER])
            self.assertTrue(executable["highest_mandatory_glibc"])
            self.assertTrue(executable["version_needs"])
            self.assertIn("libc.so.6", executable["resolved"])
        self.assertTrue((self.evidence / "tgi-loader.txt").is_file())
        self.assertTrue(result["libraries"]["libc.so.6"]["sha256"])

    def test_alias_is_inspected_independently_and_failure_evidence_survives(self):
        self.build("int main(void) { return 0; }\n")
        (self.artifact / "tgi").write_text("#!/bin/sh\nexit 0\n")
        with self.assertRaisesRegex(ValueError, "ELF64"):
            inspector.inspect(self.artifact, self.evidence)
        self.assertFalse(self.report()["passed"])
        self.assertEqual(len(self.report()["executables"]), 2)
        self.assertIn("error", self.report())

    def test_unavailable_interpreter_is_rejected(self):
        self.build("int main(void) { return 0; }\n", ["-Wl,--dynamic-linker=/absent/loader"])
        with self.assertRaisesRegex(ValueError, "interpreter"):
            inspector.inspect(self.artifact, self.evidence)
        self.assertFalse(self.report()["passed"])

    def test_unexpected_library_is_rejected_even_when_installed(self):
        self.build("int main(void) { return 0; }\n", ["-Wl,--no-as-needed", "-lstdc++"])
        with self.assertRaisesRegex(ValueError, "unexpected ELF dependencies"):
            inspector.inspect(self.artifact, self.evidence)
        self.assertIn("libstdc++.so.6", self.report()["executables"]["twigsmith"]["needed"])

    def test_runtime_search_path_is_rejected(self):
        self.build("int main(void) { return 0; }\n", [f"-Wl,-rpath,{self.root}"])
        with self.assertRaisesRegex(ValueError, "runtime search path"):
            inspector.inspect(self.artifact, self.evidence)

    def test_unresolved_mandatory_version_is_rejected_with_loader_evidence(self):
        self.version_fixture(weak=False)
        with self.assertRaisesRegex(ValueError, "failed or warned|unresolved mandatory version"):
            inspector.inspect(self.artifact, self.evidence)
        report = self.report()
        requirements = report["executables"]["twigsmith"]["version_needs"]
        need = next(item for group in requirements for item in group["versions"]
                    if item["name"] == "TWIGSMITH_TEST_1")
        self.assertFalse(need["weak"])
        self.assertFalse(report["passed"])
        self.assertIn("TWIGSMITH_TEST_1", (self.evidence / "twigsmith-loader.txt").read_text())

    def test_unresolved_weak_version_is_rejected_and_identified_separately(self):
        self.version_fixture(weak=True)
        with self.assertRaisesRegex(ValueError, "failed or warned|unresolved weak version"):
            inspector.inspect(self.artifact, self.evidence)
        report = self.report()
        requirements = report["executables"]["twigsmith"]["version_needs"]
        need = next(item for group in requirements for item in group["versions"]
                    if item["name"] == "TWIGSMITH_TEST_1")
        self.assertTrue(need["weak"])
        self.assertFalse(report["passed"])


if __name__ == "__main__":
    unittest.main()
