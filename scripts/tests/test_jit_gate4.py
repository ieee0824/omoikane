"""Exercise failure handling and coverage of the Gate 4 CI partitions."""

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("gate", Path(__file__).parents[1] / "jit-gate4.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


def acid3_report():
    expected = gate.load_expectation()
    return {mode: {key: expected[key] for key in ("score", "total", "index")} | {
        "log": "\n".join(expected["failed_lines"]), "termination_reason": "completed",
        "script_errors": [], "task_errors": [], "drive_errors": [],
    } for mode in ("faithful", "direct")}


def integration_log(targets):
    chunks = []
    for name in targets:
        chunks.append(f"Running tests/{name}.rs (binary)\n")
        if name == "jit_native_gate":
            chunks.extend([
                "test native_policy_and_seeded_workloads_produce_execution_evidence ... ok\n",
                "test stress::child_failure_preserves_seed_source_code_and_stack_maps ... ok\n",
                "test stress::seed_replay ... ok\n",
                "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; "
                "1 filtered out; finished in 0.1s\n",
            ])
        else:
            chunks.extend(["test ordinary ... ok\n",
                           "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; "
                           "0 filtered out; finished in 0.1s\n"])
    return "".join(chunks)



class PartitioningTests(unittest.TestCase):
    def test_matrix_separates_build_from_two_integration_jobs(self):
        self.assertIn("integration-0", gate.SHARDS)
        self.assertIn("integration-1", gate.SHARDS)
        self.assertIn("build", gate.SHARDS)
        self.assertNotIn("integration", gate.SHARDS)
        for name in ("integration-0", "integration-1"):
            self.assertEqual(gate.STEPS[name], ["integration"])
        self.assertEqual(gate.STEPS["build"], ["examples-and-bins", "doc", "build"])

    def test_integration_partition_is_stable_disjoint_and_complete(self):
        targets = sorted(["jit_native_gate", *[f"target_{i}" for i in range(1000)]])
        partitions = [gate.integration_partition(targets, i)
                      for i in range(gate.INTEGRATION_SHARDS)]
        self.assertTrue(all(partitions))
        flattened = [name for partition in partitions for name in partition]
        self.assertEqual(sorted(flattened), targets)
        self.assertEqual(len(flattened), len(set(flattened)))
        self.assertEqual(flattened.count("jit_native_gate"), 1)
        for index, partition in enumerate(partitions):
            self.assertEqual(partition, gate.integration_partition(targets, index))
            expanded = gate.integration_partition(sorted([*targets, "new_future_target"]), index)
            self.assertEqual([name for name in expanded if name != "new_future_target"], partition)


class GateTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.identity = {"revision": "same-revision", "source_dirty": False,
                         "rustc": "same-rustc", "target": "host", "lock_sha256": "same-lock"}
        (self.root / "inputs").mkdir()
        gate.write_json(self.root / "inputs/identity.json", self.identity)
        targets = sorted(["jit_native_gate", "new_test", *[f"new_test_{i}" for i in range(8)]])
        self.targets = targets
        gate.write_json(self.root / "inputs/integration-targets.json", targets)
        names = [f"module::test_{i}" for i in range(100)]
        for shard in gate.SHARDS:
            folder = self.root / shard
            folder.mkdir()
            steps = [{"name": name, "exit": 0} for name in gate.STEPS[shard]]
            if shard in gate.INTEGRATION_NAMES:
                selected = gate.integration_partition(targets, gate.INTEGRATION_NAMES.index(shard))
                steps[0]["command"] = gate.integration_command(selected)
                gate.write_json(folder / "targets.json", selected)
                (folder / "integration.log").write_text(integration_log(selected))
            if shard == "build":
                steps = [{"name": name, "command": command, "exit": 0}
                         for name, command in gate.build_commands()]
            if shard == "stress":
                steps[0]["command"] = gate.stress_command()
            if shard == "acid3":
                for step in steps:
                    step["test26_ms"] = {
                        mode: {"driver_ms": 2000.0, "fixture_ms": 1999}
                        for mode in ("faithful", "direct")
                    }
            gate.write_json(folder / "shard.json", {
                "shard": shard, "identity": self.identity, "error": None, "passed_tests": 1,
                "steps": steps,
                **({"stress_artifacts": ".artifacts/test-stress"} if shard == "stress" else {}),
            })
            if shard.startswith("unit-"):
                gate.write_json(folder / "all-tests.json", names)
                gate.write_json(folder / "selected-tests.json", gate.unit_partition(names, int(shard[-1])))
        (self.root / "stress/stress.log").write_text(
            "Running tests/jit_deopt.rs (stress)\n"
            "test stress::reproducible_stress_matrix ... "
            "JIT stress: 64 seeds passed in 600.1s; .artifacts/test-stress\n"
            "ok\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; "
            "0 filtered out; finished in 600.1s\n")
        gate.write_json(self.root / "acid3/acid3.json", acid3_report())
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

    def integration_path(self, target, filename):
        name = next(name for index, name in enumerate(gate.INTEGRATION_NAMES)
                    if target in gate.integration_partition(self.targets, index))
        return f"{name}/{filename}"

    def test_complete_gate_passes(self):
        self.assertTrue(self.decide())

    def test_run_each_batch_records_its_exact_command_and_passed_targets(self):
        root = self.root / "execute"
        (root / "inputs").mkdir(parents=True)
        gate.write_json(root / "inputs/identity.json", self.identity)
        gate.write_json(root / "inputs/integration-targets.json", self.targets)
        commands = []

        def execute(command, **kwargs):
            commands.append(command)
            selected = [command[i + 1] for i, value in enumerate(command) if value == "--test"]
            kwargs["stdout"].write(integration_log(selected))
            return SimpleNamespace(returncode=0)

        with patch.object(gate, "identity", return_value=self.identity), \
                patch.object(gate, "integration_targets_from_metadata", return_value=self.targets), \
                patch.object(gate.subprocess, "run", side_effect=execute), \
                contextlib.redirect_stdout(io.StringIO()):
            for name in gate.INTEGRATION_NAMES:
                self.assertTrue(gate.run_shard(name, root))
        rows = {name: gate.read_json(root / name / "shard.json") for name in gate.INTEGRATION_NAMES}
        self.assertTrue(gate.complete_integration_coverage(root, rows))
        self.assertEqual(len(commands), gate.INTEGRATION_SHARDS)
        self.assertEqual(sum(command.count("jit_native_gate") for command in commands), 1)

    def test_missing_integration_batch_rejects_gate(self):
        for name in gate.INTEGRATION_NAMES:
            path = self.root / name / "shard.json"
            original = path.read_text()
            path.unlink()
            self.assertFalse(self.decide())
            path.write_text(original)

    def test_cross_batch_duplicate_or_omission_fails_even_with_matching_command_and_log(self):
        first, second = gate.INTEGRATION_NAMES
        selected = gate.read_json(self.root / first / "targets.json")
        other = gate.read_json(self.root / second / "targets.json")
        for changed in (sorted([*selected, other[0]]), selected[:-1]):
            with self.subTest(changed=changed):
                gate.write_json(self.root / first / "targets.json", changed)
                self.modify(f"{first}/shard.json",
                            lambda row: row["steps"][0].update(command=gate.integration_command(changed)))
                (self.root / first / "integration.log").write_text(integration_log(changed))
                self.assertFalse(self.decide())

    def test_native_policy_target_remains_required_in_the_global_inventory(self):
        targets = [name for name in self.targets if name != "jit_native_gate"]
        gate.write_json(self.root / "inputs/integration-targets.json", targets)
        for index, name in enumerate(gate.INTEGRATION_NAMES):
            selected = gate.integration_partition(targets, index)
            gate.write_json(self.root / name / "targets.json", selected)
            self.modify(f"{name}/shard.json",
                        lambda row: row["steps"][0].update(command=gate.integration_command(selected)))
            (self.root / name / "integration.log").write_text(integration_log(selected))
        self.assertFalse(self.decide())

    def test_build_examples_and_docs_each_require_the_original_command_and_success(self):
        path = self.root / "build/shard.json"
        original = path.read_text()
        for index, (name, _) in enumerate(gate.build_commands()):
            for change in (lambda row: row["steps"].pop(index),
                           lambda row: row["steps"][index].update(exit=101),
                           lambda row: row["steps"][index]["command"].append("--skip")):
                with self.subTest(step=name, change=str(change)):
                    self.modify("build/shard.json", change)
                    self.assertFalse(self.decide())
                    path.write_text(original)

    def test_missing_and_malformed_artifacts_fail(self):
        for name in [f"{s}/shard.json" for s in gate.SHARDS] + [
                "inputs/identity.json", "inputs/integration-targets.json",
                *[f"{name}/targets.json" for name in gate.INTEGRATION_NAMES],
                "acid3/acid3.json", "compatibility/wpt.json",
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
        relative = self.integration_path("jit_native_gate", "shard.json")
        path = self.root / relative
        original = path.read_text()
        self.modify(relative, lambda row: row["steps"][0].update(exit=101))
        self.assertFalse(self.decide())
        path.write_text(original)
        self.modify(relative, lambda row: row["steps"].pop())
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
            {"name": "print_page_wpt", "kind": ["test"]},
        ]}
        self.assertEqual(gate.integration_targets(package), ["native", "new_test"])

    def test_integration_target_or_command_drift_fails(self):
        selected = self.integration_path("jit_native_gate", "targets.json")
        shard = self.integration_path("jit_native_gate", "shard.json")
        changes = [
            ("inputs/integration-targets.json", lambda row: row.append({"invalid": True})),
            (selected, lambda row: row.pop()),
            (selected, lambda row: row.append(row[0])),
            (shard, lambda row: row["steps"][0]["command"].remove("--skip")),
            (shard, lambda row: row["steps"][0]["command"].remove("jit_native_gate")),
            ("stress/shard.json", lambda row: row["steps"][0]["command"].append("--skip")),
        ]
        for name, change in changes:
            path = self.root / name
            original = path.read_text()
            with self.subTest(path=name, change=str(change)):
                self.modify(name, change)
                self.assertFalse(self.decide())
                path.write_text(original)

    def test_integration_log_requires_one_native_skip_and_all_targets(self):
        native = self.integration_path("jit_native_gate", "integration.log")
        ordinary = self.integration_path("new_test", "integration.log")
        changes = [
            (native, lambda text: text.replace("1 filtered out", "0 filtered out")),
            (ordinary, lambda text: text.replace("Running tests/new_test.rs", "Running tests/other.rs")),
            (native, lambda text: text.replace("test native_policy_and_seeded_workloads_produce_execution_evidence ... ok\n", "")),
            (native, lambda text: text.replace("test stress::seed_replay ... ok", "test stress::reproducible_stress_matrix ... ok")),
            (ordinary, lambda text: text.replace("0 filtered out", "1 filtered out")),
        ]
        for name, change in changes:
            path = self.root / name
            original = path.read_text()
            with self.subTest(path=name, change=str(change)):
                path.write_text(change(original))
                self.assertFalse(self.decide())
            path.write_text(original)
        path = self.root / native
        path.unlink()
        self.assertFalse(self.decide())

    def test_stress_log_requires_one_matrix_and_matching_summary(self):
        path = self.root / "stress/stress.log"
        original = path.read_text()
        changes = [
            lambda text: text.replace("test stress::reproducible_stress_matrix ... ", ""),
            lambda text: text.replace("test stress::reproducible_stress_matrix ... ",
                                      "test stress::reproducible_stress_matrix ...\n"
                                      "test stress::reproducible_stress_matrix ... "),
            lambda text: text.replace(".artifacts/test-stress", ".artifacts/other"),
        ]
        for change in changes:
            with self.subTest(change=str(change)):
                path.write_text(change(original))
                self.assertFalse(self.decide())
        path.unlink()
        self.assertFalse(self.decide())

    def test_acid3_test26_timing_retains_driver_and_fixture_values(self):
        report = {
            "faithful": {"test26_step_wall_ms": 2376.25,
                         "log": "Test 26 passed, but took 2310ms (less than 30fps)\n"},
            "direct": {"test26_step_wall_ms": 5012.0,
                       "log": "Test 25 passed, but took 40ms (less than 30fps)\n"},
        }
        self.assertEqual(gate.acid3_test26_timing(report), {
            "faithful": {"driver_ms": 2376.25, "fixture_ms": 2310},
            "direct": {"driver_ms": 5012.0, "fixture_ms": None},
        })

    def test_missing_or_nonfinite_acid3_timing_fails_gate(self):
        original = (self.root / "acid3/shard.json").read_text()
        for value in (None, float("inf"), -1, True):
            with self.subTest(value=value):
                self.modify("acid3/shard.json", lambda row: row["steps"][0]["test26_ms"]["direct"].update(driver_ms=value))
                self.assertFalse(self.decide())
                (self.root / "acid3/shard.json").write_text(original)


if __name__ == "__main__":
    unittest.main()
