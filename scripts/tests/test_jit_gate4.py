"""Exercise failure handling and coverage of the Gate 4 CI partitions."""

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("gate", Path(__file__).parents[1] / "jit-gate4.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class GateTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.identity = {"revision": "same-revision", "source_dirty": False,
                         "rustc": "same-rustc", "target": "host", "lock_sha256": "same-lock"}
        (self.root / "inputs").mkdir()
        gate.write_json(self.root / "inputs/identity.json", self.identity)
        names = [f"module::test_{i}" for i in range(100)]
        for shard in gate.SHARDS:
            folder = self.root / shard
            folder.mkdir()
            gate.write_json(folder / "shard.json", {
                "shard": shard, "identity": self.identity, "error": None, "passed_tests": 1,
                "steps": [{"name": name, "exit": 0} for name in gate.STEPS[shard]],
            })
            if shard.startswith("unit-"):
                gate.write_json(folder / "all-tests.json", names)
                gate.write_json(folder / "selected-tests.json", gate.unit_partition(names, int(shard[-1])))
        gate.write_json(self.root / "acid3/acid3.json", {
            mode: {"score": 100, "total": 100} for mode in ("faithful", "direct")})
        gate.write_json(self.root / "compatibility/wpt.json", {"summary": {"regression": 0}})
        gate.write_json(self.root / "compatibility/web-api.json", {"regressions": []})
        gate.write_json(self.root / "stress/stress.json", {
            "gc_profile": True, "elapsed_seconds": 600,
            "seeds": [{"seed": i, "success": True} for i in range(64)],
        })

    def decide(self, success=True):
        with patch.object(gate, "output", return_value="same-revision"), contextlib.redirect_stdout(io.StringIO()):
            return gate.aggregate(self.root, success)

    def modify(self, path, change):
        path = self.root / path
        value = json.loads(path.read_text())
        change(value)
        gate.write_json(path, value)

    def test_complete_gate_passes(self):
        self.assertTrue(self.decide())

    def test_missing_and_malformed_artifacts_fail(self):
        for name in [f"{s}/shard.json" for s in gate.SHARDS] + [
                "inputs/identity.json", "acid3/acid3.json", "compatibility/wpt.json",
                "compatibility/web-api.json", "stress/stress.json"]:
            path = self.root / name
            original = path.read_text()
            with self.subTest(path=name):
                path.unlink()
                self.assertFalse(self.decide())
                path.write_text("{truncated")
                self.assertFalse(self.decide())
                path.write_text(original)

    def test_failed_or_incomplete_job_fails(self):
        path = self.root / "integration/shard.json"
        original = path.read_text()
        self.modify("integration/shard.json", lambda row: row["steps"][0].update(exit=101))
        self.assertFalse(self.decide())
        path.write_text(original)
        self.modify("integration/shard.json", lambda row: row["steps"].pop())
        self.assertFalse(self.decide())
        path.write_text(original)
        self.assertFalse(self.decide(success=False))

    def test_mixed_revision_compiler_lock_or_target_fails(self):
        path = self.root / "stress/shard.json"
        original = path.read_text()
        for key in ["revision", "rustc", "target", "lock_sha256"]:
            with self.subTest(key=key):
                self.modify("stress/shard.json", lambda row: row["identity"].update({key: "different"}))
                self.assertFalse(self.decide())
                path.write_text(original)

    def test_dirty_or_stale_source_fails(self):
        self.modify("inputs/identity.json", lambda row: row.update(source_dirty=True))
        self.assertFalse(self.decide())
        self.modify("inputs/identity.json", lambda row: row.update(source_dirty=False, revision="old"))
        self.assertFalse(self.decide())

    def test_missing_duplicated_or_changed_test_list_fails(self):
        path = self.root / "unit-1/selected-tests.json"
        original = path.read_text()
        self.modify("unit-1/selected-tests.json", lambda names: names.pop())
        self.assertFalse(self.decide())
        path.write_text(original)
        self.modify("unit-1/selected-tests.json", lambda names: names.append(names[0]))
        self.assertFalse(self.decide())
        path.write_text(original)
        self.modify("unit-1/all-tests.json", lambda names: names.append("new::test"))
        self.assertFalse(self.decide())

    def test_acid3_and_regressions_remain_required(self):
        changes = [
            ("acid3/acid3.json", lambda row: row["faithful"].update(score=26)),
            ("compatibility/wpt.json", lambda row: row["summary"].update(regression=1)),
            ("compatibility/web-api.json", lambda row: row["regressions"].append("missing API")),
        ]
        for name, change in changes:
            path = self.root / name
            original = path.read_text()
            with self.subTest(path=name):
                self.modify(name, change)
                self.assertFalse(self.decide())
                path.write_text(original)

    def test_short_unprofiled_or_failed_stress_fails(self):
        path = self.root / "stress/stress.json"
        original = path.read_text()
        for change in [lambda row: row.update(elapsed_seconds=599),
                       lambda row: row.update(gc_profile=False),
                       lambda row: row["seeds"].pop(),
                       lambda row: row["seeds"][0].update(success=False)]:
            self.modify("stress/stress.json", change)
            self.assertFalse(self.decide())
            path.write_text(original)

    def test_new_integration_targets_are_included(self):
        package = {"features": {"jit-stress": ["baseline-jit"], "baseline-jit": []}, "targets": [
            {"name": "new_test", "kind": ["test"]},
            {"name": "native", "kind": ["test"], "required-features": ["baseline-jit"]},
            {"name": "gui_only", "kind": ["test"], "required-features": ["gui"]},
            {"name": "acid3_harness", "kind": ["test"]},
        ]}
        self.assertEqual(gate.integration_targets(package), ["native", "new_test"])


if __name__ == "__main__":
    unittest.main()
