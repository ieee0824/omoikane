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


def acid3_report():
    expected = gate.load_expectation()
    return {mode: {key: expected[key] for key in ("score", "total", "index")} | {
        "log": "\n".join(expected["failed_lines"]), "termination_reason": "completed",
        "script_errors": [], "task_errors": [], "drive_errors": [],
    } for mode in ("faithful", "direct")}



class GateTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.identity = {"revision": "same-revision", "source_dirty": False,
                         "rustc": "same-rustc", "target": "host", "lock_sha256": "same-lock"}
        (self.root / "inputs").mkdir()
        gate.write_json(self.root / "inputs/identity.json", self.identity)
        targets = ["jit_native_gate", "new_test"]
        gate.write_json(self.root / "inputs/integration-targets.json", targets)
        surface_names = ["js::web_api_surface_tests::" + name for name in (
            "manifest_ids_are_unique_and_well_formed",
            "report_classifies_regressions_and_improvements",
            "committed_document_exposes_navigation_url",
            "navigation_probe_checks_destination_and_history_mode",
            "web_api_surface_does_not_regress",
            "trusted_surface_probe_rejects_missing_input",
        )]
        names = [f"module::test_{i}" for i in range(100)] + surface_names
        for shard in gate.SHARDS:
            folder = self.root / shard
            folder.mkdir()
            steps = [{"name": name, "exit": 0} for name in gate.STEPS[shard]]
            if shard == "integration":
                steps[0]["command"] = gate.integration_command(targets)
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
        gate.write_json(self.root / "integration/targets.json", targets)
        (self.root / "integration/integration.log").write_text(
            "Running tests/jit_native_gate.rs (native)\n"
            "test native_policy_and_seeded_workloads_produce_execution_evidence ... ok\n"
            "test stress::child_failure_preserves_seed_source_code_and_stack_maps ... ok\n"
            "test stress::seed_replay ... ok\n"
            "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; "
            "1 filtered out; finished in 0.1s\n"
            "Running tests/new_test.rs (ordinary)\n"
            "test ordinary ... ok\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; "
            "0 filtered out; finished in 0.1s\n")
        (self.root / "stress/stress.log").write_text(
            "Running tests/jit_deopt.rs (stress)\n"
            "test stress::reproducible_stress_matrix ... "
            "JIT stress: 64 seeds passed in 600.1s; .artifacts/test-stress\n"
            "ok\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; "
            "0 filtered out; finished in 600.1s\n")
        gate.write_json(self.root / "acid3/acid3.json", acid3_report())
        gate.write_json(self.root / "compatibility/wpt.json", {"summary": {"regression": 0}})
        features = json.loads(Path("tests/web_api_surface/manifest.json").read_text())["features"]
        surface_report = {
            "total": len(features), "regressions": [],
            "results": [{key: feature[key] for key in
                         ("id", "area", "description", "baseline_supported")} |
                        {"status": "supported", "error": None} for feature in features],
        }
        for label in ("web-api", "embedded-web-api"):
            gate.write_json(self.root / f"compatibility/{label}.json", surface_report)
            (self.root / f"compatibility/{label}.log").write_text(
                "".join(f"test {name} ... ok\n" for name in surface_names)
                + "test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; "
                "5018 filtered out; finished in 9.32s\n")
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
                "inputs/identity.json", "inputs/integration-targets.json",
                "integration/targets.json", "acid3/acid3.json", "compatibility/wpt.json",
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
            {"name": "print_page_wpt", "kind": ["test"]},
        ]}
        self.assertEqual(gate.integration_targets(package), ["native", "new_test"])

    def test_integration_target_or_command_drift_fails(self):
        changes = [
            ("inputs/integration-targets.json", lambda row: row.append({"invalid": True})),
            ("integration/targets.json", lambda row: row.pop()),
            ("integration/targets.json", lambda row: row.append(row[0])),
            ("integration/shard.json", lambda row: row["steps"][0]["command"].remove("--skip")),
            ("integration/shard.json", lambda row: row["steps"][0]["command"].remove("jit_native_gate")),
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
        path = self.root / "integration/integration.log"
        original = path.read_text()
        changes = [
            lambda text: text.replace("1 filtered out", "0 filtered out"),
            lambda text: text.replace("Running tests/new_test.rs", "Running tests/other.rs"),
            lambda text: text.replace("test native_policy_and_seeded_workloads_produce_execution_evidence ... ok\n", ""),
            lambda text: text.replace("test stress::seed_replay ... ok", "test stress::reproducible_stress_matrix ... ok"),
            lambda text: text.replace("0 filtered out", "1 filtered out"),
        ]
        for change in changes:
            with self.subTest(change=str(change)):
                path.write_text(change(original))
                self.assertFalse(self.decide())
        path.write_text(original)
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


class SurfaceGateTests(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.root=Path(self.tmp.name);(self.root/'unit-0').mkdir();(self.root/'compatibility').mkdir()
  self.names=['js::web_api_surface_tests::'+x for x in ['manifest_ids_are_unique_and_well_formed','report_classifies_regressions_and_improvements','committed_document_exposes_navigation_url','navigation_probe_checks_destination_and_history_mode','web_api_surface_does_not_regress','trusted_surface_probe_rejects_missing_input']]
  gate.write_json(self.root/'unit-0/all-tests.json',self.names+['js::unrelated'])
  features=json.loads(Path('tests/web_api_surface/manifest.json').read_text())['features'];self.report={'total':len(features),'regressions':[],'results':[{k:f[k] for k in ['id','area','description','baseline_supported']}|{'status':'supported','error':None} for f in features]}
  for label in ['web-api','embedded-web-api']:
   gate.write_json(self.root/f'compatibility/{label}.json',self.report);(self.root/f'compatibility/{label}.log').write_text(''.join(f'test {n} ... ok\n' for n in self.names)+'test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 5018 filtered out; finished in 9.32s\n')
 def test_complete(self):self.assertTrue(gate.complete_surface_coverage(self.root))
 def test_missing_second_report(self):
  (self.root/'compatibility/embedded-web-api.json').unlink();self.assertFalse(gate.complete_surface_coverage(self.root))
 def test_missing_case(self):
  (self.root/'compatibility/web-api.log').write_text(''.join(f'test {n} ... ok\n' for n in self.names[:-1]));self.assertFalse(gate.complete_surface_coverage(self.root))
 def test_duplicate_case(self):
  p=self.root/'compatibility/web-api.log';p.write_text(p.read_text()+f'test {self.names[0]} ... ok\n');self.assertFalse(gate.complete_surface_coverage(self.root))
 def test_missing_report_id(self):
  self.report['results'].pop();gate.write_json(self.root/'compatibility/web-api.json',self.report);self.assertFalse(gate.complete_surface_coverage(self.root))
 def test_duplicate_report_id(self):
  self.report['results'][1]=self.report['results'][0];gate.write_json(self.root/'compatibility/web-api.json',self.report);self.assertFalse(gate.complete_surface_coverage(self.root))
 def test_false_empty_regressions(self):
  row=next(x for x in self.report['results'] if x['baseline_supported']);row['status']='error';gate.write_json(self.root/'compatibility/web-api.json',self.report);self.assertFalse(gate.complete_surface_coverage(self.root))
 def test_metadata_mismatch(self):
  self.report['results'][0]['baseline_supported']=not self.report['results'][0]['baseline_supported'];gate.write_json(self.root/'compatibility/web-api.json',self.report);self.assertFalse(gate.complete_surface_coverage(self.root))


class SurfaceLogTests(unittest.TestCase):
    def setUp(self):
        self.raw = (Path(__file__).parent / "fixtures/gate4-surface-nocapture.log").read_text()
        self.names = [
            "js::web_api_surface_tests::" + name for name in (
                "committed_document_exposes_navigation_url",
                "manifest_ids_are_unique_and_well_formed",
                "navigation_probe_checks_destination_and_history_mode",
                "report_classifies_regressions_and_improvements",
                "trusted_surface_probe_rejects_missing_input",
                "web_api_surface_does_not_regress",
            )
        ]

    def test_saved_nocapture_success_log_covers_every_case(self):
        self.assertTrue(gate.surface_log_covers_tests(self.raw, self.names))

    def test_missing_or_failed_multiline_outcome_is_rejected(self):
        for replacement in ("", "FAILED\n", "ignored\n"):
            with self.subTest(replacement=replacement):
                damaged = self.raw.replace("\nok\n", "\n" + replacement)
                self.assertFalse(gate.surface_log_covers_tests(damaged, self.names))

    def test_duplicate_or_unexpected_case_is_rejected(self):
        for name in (self.names[0], "js::unrelated_test"):
            with self.subTest(name=name):
                self.assertFalse(gate.surface_log_covers_tests(
                    self.raw + f"test {name} ... ok\n", self.names))

    def test_missing_or_conflicting_terminal_summary_is_rejected(self):
        for damaged in (
            self.raw.split("test result:")[0],
            self.raw.replace("6 passed", "5 passed"),
            self.raw.replace("0 failed", "1 failed"),
            self.raw.replace("0 ignored", "1 ignored"),
            self.raw + self.raw[self.raw.index("test result:"):],
        ):
            with self.subTest(log=damaged[-150:]):
                self.assertFalse(gate.surface_log_covers_tests(damaged, self.names))

    def test_orphan_outcome_and_unfinished_case_are_rejected(self):
        for damaged in ("ok\n" + self.raw, self.raw + f"test {self.names[0]} ... output\n"):
            with self.subTest(log=damaged[-150:]):
                self.assertFalse(gate.surface_log_covers_tests(damaged, self.names))

    def test_report_body_cannot_supply_an_extra_outcome(self):
        for outcome in ("ok", "FAILED"):
            with self.subTest(outcome=outcome):
                damaged = self.raw.replace("  canvas:", outcome + "\n  canvas:", 1)
                self.assertFalse(gate.surface_log_covers_tests(damaged, self.names))

    def test_interleaved_case_start_before_pending_outcome_is_rejected(self):
        damaged = self.raw.replace(
            "  canvas:", f"test {self.names[0]} ... ok\n  canvas:", 1)
        self.assertFalse(gate.surface_log_covers_tests(damaged, self.names))

    def test_colored_success_log_is_supported(self):
        colored = self.raw.replace("ok", "\x1b[32mok\x1b[0m")
        self.assertTrue(gate.surface_log_covers_tests(colored, self.names))

if __name__ == "__main__":
    unittest.main()
