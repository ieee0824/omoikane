"""Verify rerun artifacts are selected per logical Gate 4/5 job."""

import importlib.util
from pathlib import Path
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location(
    "select_gate_artifacts",
    Path(__file__).parents[1] / "select-gate-artifacts.py",
)
selector = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(selector)


class ArtifactSelectionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "downloaded"
        self.destination = self.root / "selected"
        self.source.mkdir()

    def artifact(self, name: str, relative: str, contents: str) -> None:
        path = self.source / name / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(contents)

    def test_gate4_combines_latest_rerun_with_earlier_successful_shard(self):
        prefix = "jit-gate4-partition-"
        self.artifact(f"{prefix}acid3-attempt-1", "gate4/acid3/result.json", "failed")
        self.artifact(f"{prefix}acid3-attempt-3", "gate4/acid3/result.json", "passed")
        self.artifact(f"{prefix}unit-0-attempt-1", "gate4/unit-0/result.json", "passed")

        selected = selector.select_and_merge(self.source, self.destination, prefix)

        self.assertEqual(selected, {"acid3": 3, "unit-0": 1})
        self.assertEqual((self.destination / "gate4/acid3/result.json").read_text(), "passed")
        self.assertEqual((self.destination / "gate4/unit-0/result.json").read_text(), "passed")

    def test_gate5_selects_attempt_independently_for_each_target_and_kind(self):
        prefix = "jit-gate5-"
        suite = "x86_64-unknown-linux-gnu-suite"
        package = "x86_64-unknown-linux-gnu-package"
        self.artifact(f"{prefix}{suite}-attempt-1", "x86/suite/result.json", "failed")
        self.artifact(f"{prefix}{suite}-attempt-2", "x86/suite/result.json", "passed")
        self.artifact(f"{prefix}{package}-attempt-1", "x86/package/result.json", "passed")
        (self.destination / "inputs").mkdir(parents=True)
        (self.destination / "inputs/identity.json").write_text("preserved")

        selected = selector.select_and_merge(self.source, self.destination, prefix)

        self.assertEqual(selected, {package: 1, suite: 2})
        self.assertEqual((self.destination / "x86/suite/result.json").read_text(), "passed")
        self.assertEqual((self.destination / "x86/package/result.json").read_text(), "passed")
        self.assertEqual((self.destination / "inputs/identity.json").read_text(), "preserved")

    def test_conflicting_paths_from_selected_artifacts_fail(self):
        prefix = "jit-gate4-partition-"
        self.artifact(f"{prefix}acid3-attempt-2", "gate4/shared.json", "acid3")
        self.artifact(f"{prefix}unit-0-attempt-1", "gate4/shared.json", "unit")

        with self.assertRaisesRegex(ValueError, "conflicting artifact path"):
            selector.select_and_merge(self.source, self.destination, prefix)

    def test_missing_or_malformed_attempt_artifacts_fail(self):
        with self.assertRaisesRegex(ValueError, "no attempted artifacts"):
            selector.select_and_merge(self.source, self.destination, "jit-gate5-")
        (self.source / "jit-gate5-x86-suite-attempt-invalid").mkdir()
        with self.assertRaisesRegex(ValueError, "invalid attempted artifact name"):
            selector.select_and_merge(self.source, self.destination, "jit-gate5-")


if __name__ == "__main__":
    unittest.main()
