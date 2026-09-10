"""Reject incomplete, stale or weakened engine compatibility comparisons."""

import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "engine_test262", Path(__file__).parents[1] / "engine-test262.py")
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)


class ComparisonTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.artifacts = self.root / "artifacts"
        gate.write(self.root / "engine/boa-origin.json", {"tree": "b" * 40})
        source = self.root / "engine/boa"
        source.mkdir()
        (source / "Cargo.lock").write_text("original dependencies\n")
        (source / "test262_config.toml").write_text('commit = "' + "c" * 40 + '"\n')
        for target in gate.TARGETS:
            for variant in ("reference", "current"):
                folder = self.artifacts / target / variant
                gate.write(folder / "execution.json", {
                    "target": target, "variant": variant, "revision": "a" * 40,
                    "status": "completed", "origin_tree": "b" * 40,
                    "test262_revision": "c" * 40, "toolchain_sha256": "d" * 64,
                    "lock_sha256": hashlib.sha256((source / "Cargo.lock").read_bytes()).hexdigest(),
                    "case_count": 2, "stats": {"O": 1, "F": 1}})
                gate.write(folder / "cases.json", {
                    "test/passing": {"status": "O", "ecma_version": 6},
                    "test/known-failure": {"status": "F", "ecma_version": 6}})
        self.folder = self.artifacts / gate.TARGETS[0] / "current"

    def compare(self):
        with patch.object(gate, "ROOT", self.root), patch.object(
                gate.subprocess, "check_output", return_value="a" * 40 + "\n"), contextlib.redirect_stdout(io.StringIO()):
            return gate.compare(self.artifacts)

    def change_case(self, name, status):
        cases = gate.read(self.folder / "cases.json")
        if status is None:
            del cases[name]
        else:
            cases[name] = {"status": status, "ecma_version": 6}
        gate.write(self.folder / "cases.json", cases)
        report = gate.read(self.folder / "execution.json")
        report.update(case_count=len(cases), stats=dict(gate.Counter(x["status"] for x in cases.values())))
        gate.write(self.folder / "execution.json", report)

    def test_identical_cases_pass_with_known_failures_preserved(self):
        self.assertTrue(self.compare())

    def test_resolved_failure_passes_and_is_recorded(self):
        self.change_case("test/known-failure", "O")
        self.assertTrue(self.compare())
        result = gate.read(self.artifacts / "comparison.json")
        self.assertEqual(result["targets"][gate.TARGETS[0]]["changes"][0]["case"], "test/known-failure")

    def test_removed_case_fails_even_when_counts_match_new_inventory(self):
        self.change_case("test/passing", None)
        self.assertFalse(self.compare())

    def test_regression_skip_panic_and_invalid_status_fail(self):
        for status in ("F", "I", "P", "unknown"):
            with self.subTest(status=status):
                self.change_case("test/passing", status)
                self.assertFalse(self.compare())

    def test_existing_failure_cannot_be_hidden_by_ignoring_it(self):
        self.change_case("test/known-failure", "I")
        self.assertFalse(self.compare())

    def test_new_failing_case_fails(self):
        self.change_case("test/new-failure", "F")
        self.assertFalse(self.compare())

    def test_stale_source_wrong_inputs_and_incomplete_execution_fail(self):
        original = gate.read(self.folder / "execution.json")
        for field, value in (("revision", "e" * 40), ("test262_revision", "e" * 40),
                             ("origin_tree", "e" * 40), ("lock_sha256", "e" * 64),
                             ("toolchain_sha256", "e" * 64), ("status", "running"),
                             ("case_count", 0), ("stats", {"O": 2})):
            with self.subTest(field=field):
                gate.write(self.folder / "execution.json", dict(original, **{field: value}))
                self.assertFalse(self.compare())

    def test_missing_target_does_not_pass(self):
        (self.folder / "execution.json").unlink()
        self.assertFalse(self.compare())

    def test_duplicate_case_names_are_rejected(self):
        with self.assertRaises(AssertionError):
            gate.case_index({"n": "test", "t": [{"n": "same", "r": "O"}, {"n": "same", "r": "F"}]})


class RetainedSourceTests(unittest.TestCase):
    def test_original_git_tree_restores_exact_bytes_and_executable_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(["git", "init", "--quiet", str(root)], check=True)
            files = {}
            for name, mode in (("ordinary.rs", 0o644), ("script.sh", 0o755)):
                data = name.encode() + b"\n"
                path = root / name
                path.write_bytes(data)
                path.chmod(mode)
                files[name] = {"mode": oct(0o100000 | mode)[2:], "sha256": hashlib.sha256(data).hexdigest()}
            subprocess.run(["git", "add", "."], cwd=root, check=True)
            tree = subprocess.check_output(["git", "write-tree"], cwd=root, text=True).strip()
            origin = {"tree": tree, "files": files, "file_count": len(files)}
            with patch.object(gate, "ROOT", root):
                gate.restore_reference(root / "restored", origin)
            for name in files:
                self.assertEqual((root / "restored" / name).read_bytes(), (root / name).read_bytes())
                self.assertEqual((root / "restored" / name).stat().st_mode & 0o777,
                                 (root / name).stat().st_mode & 0o777)


if __name__ == "__main__":
    unittest.main()
