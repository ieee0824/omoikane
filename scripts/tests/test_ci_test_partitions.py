"""Default test coverage must survive splitting the normal CI's linking work."""

import copy
import hashlib
import importlib.util
import json
import os
import re
import sys
from pathlib import Path
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "ci-test-partitions.py"
SPEC = importlib.util.spec_from_file_location("ci_test_partitions", SCRIPT)
ci = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ci)


def target(name, kind, *, test=True, doctest=False, required=()):
    directory = {"lib": "src", "bin": "src/bin", "test": "tests", "example": "examples"}[kind]
    return {"name": name, "kind": [kind], "test": test, "doctest": doctest,
            "required-features": list(required), "src_path": f"/repo/{directory}/{name}.rs"}


def package():
    targets = [target("library", "lib", doctest=True), target("tool", "bin"),
               target("gui_tool", "bin", required=["gui"]),
               target("compile_only", "example", test=False),
               target("tested_example", "example"),
               target("jit_probe", "example", test=False, required=["jit"])]
    targets += [target(f"case_{index}", "test") for index in range(20)]
    targets += [target("jit_differential", "test")]
    targets += [target("feature_test", "test", required=["gui"]),
                target("disabled", "test", test=False)]
    return {"packages": [{"name": "omoikane", "features": {"gui": [], "jit": [], "jit-differential": []},
                           "targets": targets}]}


def fixture_identity():
    return {"revision": "fixture", "rustc": "fixture rustc",
            "lock_sha256": hashlib.sha256(b"fixture lockfile").hexdigest()}


def write_source_inputs(directory, root):
    browser = directory / "browser"
    browser.mkdir(parents=True)
    identity = fixture_identity()
    (browser / "revision.txt").write_text(identity["revision"])
    (browser / "toolchain.txt").write_text(identity["rustc"])
    (browser / "Cargo.lock").write_bytes(b"fixture lockfile")
    metadata = package()
    for target in metadata["packages"][0]["targets"]:
        target["src_path"] = str(root / Path(target["src_path"]).relative_to("/repo"))
    (browser / "dependencies.json").write_text(json.dumps(metadata))


class InventoryTests(unittest.TestCase):
    def test_default_scope_records_required_feature_exclusions(self):
        inventory = ci.inventory_from_metadata(package(), Path("/repo"))
        self.assertEqual({t["name"] for t in inventory["excluded"]},
                         {"gui_tool", "jit_probe", "feature_test"})
        self.assertIn("compile_only", {t["name"] for t in inventory["targets"]})

    def test_default_feature_closure_and_cycles_include_newly_enabled_targets(self):
        metadata = package()
        metadata["packages"][0]["features"].update({
            "default": ["nested", "dep:optional", "dependency/feature"],
            "nested": ["gui"], "gui": ["nested"],
        })
        inventory = ci.inventory_from_metadata(metadata, Path("/repo"))
        self.assertEqual(inventory["features"], ["default", "gui", "nested"])
        self.assertEqual({t["name"] for t in inventory["excluded"]}, {"jit_probe"})

    def test_metadata_order_does_not_affect_prepared_inventory(self):
        metadata = package()
        reversed_metadata = copy.deepcopy(metadata)
        reversed_metadata["packages"][0]["targets"].reverse()
        self.assertEqual(ci.inventory_from_metadata(metadata, Path("/repo")),
                         ci.inventory_from_metadata(reversed_metadata, Path("/repo")))

    def test_duplicates_and_escaping_source_are_rejected(self):
        metadata = package()
        metadata["packages"][0]["targets"].append(copy.deepcopy(metadata["packages"][0]["targets"][0]))
        with self.assertRaisesRegex(ValueError, "duplicate"):
            ci.inventory_from_metadata(metadata, Path("/repo"))
        metadata = package()
        metadata["packages"][0]["targets"][0]["src_path"] = "/repo/../outside.rs"
        with self.assertRaisesRegex(ValueError, "escapes"):
            ci.inventory_from_metadata(metadata, Path("/repo"))

    def test_new_benchmark_tests_cannot_silently_fall_out_of_coverage(self):
        metadata = package()
        benchmark = target("new_benchmark", "test")
        benchmark["kind"] = ["bench"]
        metadata["packages"][0]["targets"].append(benchmark)
        with self.assertRaisesRegex(ValueError, "explicit normal CI plan"):
            ci.inventory_from_metadata(metadata, Path("/repo"))


class CacheTelemetryTests(unittest.TestCase):
    def test_existing_key_is_not_reported_as_a_restored_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "missing"),
                                              "OMOIKANE_CI_CACHE_HIT": "true"}):
                ci.snapshot(root / "report", "probe")
            cache = json.loads((root / "report/probe.json").read_text())["cache"]
            self.assertTrue(cache["lookup_only"])
            self.assertFalse(cache["restored"])
            self.assertEqual(cache["lookup_exact_hit"], "true")


class PlanTests(unittest.TestCase):
    def setUp(self):
        self.inventory = ci.inventory_from_metadata(package(), Path("/repo"))

    def test_every_default_integration_executes_once_without_disabled_targets(self):
        parts = [ci.integration_partition(self.inventory, index) for index in range(ci.INTEGRATION_PARTITIONS)]
        names = [t["name"] for part in parts for t in part]
        self.assertEqual(sorted(names), sorted([f"case_{index}" for index in range(20)] + ["jit_differential"]))
        self.assertEqual(len(names), len(set(names)))
        for index, targets in enumerate(parts):
            steps = ci.partition_plan(self.inventory, f"integration-{index}")
            self.assertEqual(len(steps), 1)
            command = steps[0]["command"]
            selected = [command[i + 1] for i, arg in enumerate(command) if arg == "--test"]
            self.assertEqual(selected, [t["name"] for t in targets])

    def test_unrelated_new_target_does_not_move_existing_targets_between_runners(self):
        previous = [{t["name"] for t in ci.integration_partition(self.inventory, i)} for i in range(ci.INTEGRATION_PARTITIONS)]
        modified = copy.deepcopy(self.inventory)
        modified["targets"].append({"name": "new_case", "kind": "test", "test": True,
                                    "doctest": False, "source": "tests/new_case.rs"})
        for index in range(ci.INTEGRATION_PARTITIONS):
            current = {t["name"] for t in ci.integration_partition(modified, index)}
            self.assertEqual(current - {"new_case"}, previous[index])

    def test_unit_docs_compile_only_examples_gate2_and_build_are_all_retained(self):
        steps = ci.partition_plan(self.inventory, "unit-build")
        self.assertEqual([step["name"] for step in steps], ["unit", "examples", "doc", "gate2", "build"])
        self.assertEqual(steps[0]["runs"], ["src/bin/tool.rs", "examples/tested_example.rs", "src/library.rs"])
        self.assertEqual(steps[1]["builds"], ["examples/compile_only.rs"])
        # test=false examples must compile in the test profile without a test harness.
        self.assertEqual(steps[1]["command"][:6], ["cargo", "build", "--locked", "--profile", "test", "--example"])
        self.assertNotIn("--no-run", steps[1]["command"])
        self.assertEqual(steps[2]["command"][:4], ["cargo", "test", "--locked", "--doc"])
        self.assertEqual(steps[3]["command"], ["cargo", "test", "--locked", "--features", "jit-differential",
                                               "--test", "jit_differential"])
        self.assertEqual(steps[4]["command"], ["cargo", "build", "--locked"])

    def test_ignored_tests_still_run_with_only_the_existing_twelve_manual_utilities_skipped(self):
        expected = {
            "refresh_acid2_baseline_png", "refresh_fullscreen_top_layer_baseline_png",
            "refresh_popover_top_layer_baseline_png",
            "profile_inline_style_subtree_invalidation_against_full_cache_clear",
            "profile_hidden_subtree_geometry_retention_against_full_rebuild",
            "profile_static_document_shadow_frames", "profile_repeated_and_distinct_frame_shadow_masks",
            "profile_offcanvas_outer_shadow_fixed_fixture", "profile_fixed_transform_property_lookup_hot_paths",
            "manual_image_scaling_reference_ab", "static_transition_sampling_microbenchmark",
            "compound_scanline_microbenchmark",
        }
        for partition in ci.PARTITIONS:
            for step in ci.partition_plan(self.inventory, partition):
                if step["name"] not in {"unit", "integration", "doc"}:
                    continue
                command = step["command"]
                self.assertIn("--include-ignored", command)
                skipped = [command[i + 1] for i, arg in enumerate(command) if arg == "--skip"]
                self.assertEqual(set(skipped), expected)
                self.assertEqual(len(skipped), 12)

    def test_invalid_and_empty_partitions_fail_instead_of_green_empty_work(self):
        for name in [f"integration-{ci.INTEGRATION_PARTITIONS}", "unknown"]:
            with self.assertRaises(ValueError):
                ci.partition_plan(self.inventory, name)
        self.inventory["targets"] = []
        with self.assertRaisesRegex(ValueError, "empty"):
            ci.partition_plan(self.inventory, "integration-0")


class ExecutionEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.step = {"name": "integration", "command": ["cargo", "test", "--test", "one", "--test", "two"],
                     "runs": ["tests/one.rs", "tests/two.rs"]}
        self.result = {"name": "integration", "command": self.step["command"], "exit": 0}
        self.log = ("Running tests/one.rs (target/debug/deps/one-abc)\n"
                    "test result: ok. 1 passed; 0 failed; 0 ignored;\n"
                    "Running tests/two.rs (target/debug/deps/two-def)\n"
                    "test result: ok. 0 passed; 0 failed; 0 ignored;\n")

    def complete(self, log=None, step=None, result=None):
        return ci.complete_step(step or self.step, result or self.result,
                                self.log if log is None else log, Path("/repo"))

    def test_real_execution_markers_and_each_harness_result_are_required(self):
        self.assertTrue(self.complete())
        self.assertFalse(self.complete(""))
        self.assertFalse(self.complete(self.log.replace("Running tests/two.rs (target/debug/deps/two-def)\n", "")))
        self.assertFalse(self.complete(self.log.replace("test result: ok. 0 passed; 0 failed; 0 ignored;\n", "")))

    def test_duplicate_missing_extra_and_failed_targets_cannot_pass(self):
        self.assertFalse(self.complete(self.log.replace("tests/two.rs", "tests/one.rs")))
        self.assertFalse(self.complete(self.log + self.log))
        self.assertFalse(self.complete(self.log.replace("tests/two.rs", "tests/other.rs")))
        self.assertFalse(self.complete(self.log.replace("ok. 1 passed; 0 failed", "FAILED. 0 passed; 1 failed")))

    def test_success_log_cannot_mask_nonzero_exit_or_changed_command(self):
        failed = dict(self.result, exit=75)
        self.assertFalse(self.complete(result=failed))
        changed = dict(self.result, command=["cargo", "test", "--test", "one"])
        self.assertFalse(self.complete(result=changed))

    def test_ansi_and_zero_test_feature_gated_targets_are_preserved(self):
        self.assertTrue(self.complete("\x1b[1m" + self.log + "\x1b[0m"))

    def test_ignored_or_missing_ignored_count_cannot_certify_execution(self):
        for summary in ["0 passed; 0 failed; 1 ignored;", "0 passed; 0 failed;"]:
            with self.subTest(summary=summary):
                changed = self.log.replace("0 passed; 0 failed; 0 ignored;", summary)
                self.assertFalse(self.complete(changed))

    def test_compile_only_example_requires_non_harness_compiler_artifact(self):
        step = {"name": "examples", "command": ["cargo", "build"], "builds": ["examples/one.rs"]}
        result = {"name": "examples", "command": step["command"], "exit": 0}
        event = {"reason": "compiler-artifact", "target": {"kind": ["example"],
                 "src_path": "/repo/examples/one.rs"}, "profile": {"test": False}}
        finished = json.dumps({"reason": "build-finished", "success": True})
        valid = json.dumps(event) + "\n" + finished
        self.assertTrue(self.complete(valid, step, result))
        self.assertFalse(self.complete(finished, step, result))
        event["profile"]["test"] = True
        self.assertFalse(self.complete(json.dumps(event) + "\n" + finished, step, result))
        event["profile"]["test"] = False
        event["target"]["src_path"] = "/other/examples/one.rs"
        self.assertFalse(self.complete(json.dumps(event) + "\n" + finished, step, result))

    def test_doc_scope_requires_doc_marker_and_success_summary(self):
        step = {"name": "doc", "command": ["cargo", "test", "--doc"], "doctest": "omoikane"}
        result = {"name": "doc", "command": step["command"], "exit": 0}
        valid = "Doc-tests omoikane\ntest result: ok. 0 passed; 0 failed; 0 ignored;\n"
        self.assertTrue(self.complete(valid, step, result))
        self.assertFalse(self.complete(valid.replace("Doc-tests omoikane\n", ""), step, result))

    def test_complete_partition_is_not_proved_by_uploaded_success_booleans(self):
        inventory = ci.inventory_from_metadata(package(), Path("/repo"))
        plan = ci.partition_plan(inventory, "integration-0")
        prepared = {"identity": fixture_identity(), "inventory": inventory}
        result = {"partition": "integration-0", "identity": prepared["identity"], "complete": True,
                  "source_unchanged": True,
                  "steps": [{"name": "integration", "command": plan[0]["command"], "exit": 0, "coverage": True}]}
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.assertFalse(ci.complete_partition(prepared, "integration-0", result, directory, Path("/repo")))
            write_source_inputs(directory, Path("/repo"))
            log = "".join(f"Running {source} (target/debug/deps/a)\n"
                          "test result: ok. 1 passed; 0 failed; 0 ignored;\n"
                          for source in plan[0]["runs"])
            (directory / "integration.log").write_text(log)
            self.assertTrue(ci.complete_partition(prepared, "integration-0", result, directory, Path("/repo")))
            result["identity"] = {"revision": "different"}
            self.assertFalse(ci.complete_partition(prepared, "integration-0", result, directory, Path("/repo")))


class AggregateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.previous = Path.cwd()
        os.chdir(self.temporary.name)
        self.addCleanup(os.chdir, self.previous)
        self.root = Path.cwd()
        self.inventory = ci.inventory_from_metadata(package(), Path("/repo"))
        self.identity = fixture_identity()
        self.prepared = {"identity": self.identity, "inventory": self.inventory,
                         "maximum_target_gib": 10,
                         "plans": {name: ci.partition_plan(self.inventory, name) for name in ci.PARTITIONS}}
        ci.write_json(Path("inputs/inputs.json"), self.prepared)
        for partition in ci.PARTITIONS:
            directory = Path("parts") / partition
            directory.mkdir(parents=True)
            write_source_inputs(directory, self.root)
            steps = []
            for planned in self.prepared["plans"][partition]:
                if "runs" in planned:
                    log = "".join(f"Running {source} (target/debug/deps/a)\n"
                                  "test result: ok. 1 passed; 0 failed; 0 ignored;\n"
                                  for source in planned["runs"])
                elif "doctest" in planned:
                    log = "Doc-tests omoikane\ntest result: ok. 1 passed; 0 failed; 0 ignored;\n"
                elif "builds" in planned:
                    events = [{"reason": "compiler-artifact", "target": {"kind": ["example"],
                               "src_path": str(self.root / source)}, "profile": {"test": False}}
                              for source in planned["builds"]]
                    events.append({"reason": "build-finished", "success": True})
                    log = "\n".join(json.dumps(event) for event in events)
                else:
                    log = "Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.0s\n"
                (directory / (planned["name"] + ".log")).write_text(log)
                steps.append({"name": planned["name"], "command": planned["command"], "exit": 0})
            ci.write_json(directory / "result.json", {"partition": partition, "identity": self.identity,
                                                       "complete": True, "source_unchanged": True, "steps": steps})
        ci.write_json(Path("parts/integration-0/js-benchmark/report.json"), {"fixture": True})

    def aggregate(self):
        with mock.patch.object(ci, "current_inventory", return_value=self.inventory), \
                mock.patch.object(ci, "identity", return_value=self.identity):
            status = ci.aggregate(Path("inputs"), Path("parts"), Path("aggregate"))
        return status, json.loads(Path("aggregate/gate.json").read_text())

    def test_complete_full_scope_and_unique_benchmark_are_required(self):
        status, result = self.aggregate()
        self.assertEqual(status, 0)
        self.assertTrue(result["full_suite"])
        self.assertEqual(result["integration_targets"], 21)
        self.assertEqual(json.loads(Path(".artifacts/js-benchmark/report.json").read_text()), {"fixture": True})

    def test_missing_docs_cannot_be_hidden_by_complete_flags(self):
        Path("parts/unit-build/doc.log").unlink()
        status, result = self.aggregate()
        self.assertEqual(status, 1)
        self.assertFalse(result["checks"]["unit-build"])

    def test_ignored_or_missing_ignored_count_cannot_make_aggregate_green(self):
        for relative in ["integration-0/integration.log", "unit-build/doc.log", "unit-build/gate2.log"]:
            path = Path("parts") / relative
            original = path.read_text()
            for summary in ["0 passed; 0 failed; 1 ignored;", "0 passed; 0 failed;"]:
                with self.subTest(log=relative, summary=summary):
                    path.write_text(original.replace("1 passed; 0 failed; 0 ignored;", summary, 1))
                    status, report = self.aggregate()
                    self.assertEqual(status, 1)
                    self.assertFalse(report["full_suite"])
            path.write_text(original)

    def test_nonzero_exit_and_incomplete_partition_are_not_success(self):
        path = Path("parts/integration-0/result.json")
        result = json.loads(path.read_text())
        result["steps"][0]["exit"] = 75
        ci.write_json(path, result)
        self.assertEqual(self.aggregate()[0], 1)

    def test_missing_build_inputs_and_source_mutation_cannot_be_accepted(self):
        Path("parts/unit-build/browser/Cargo.lock").unlink()
        self.assertEqual(self.aggregate()[0], 1)
        path = Path("parts/integration-0/result.json")
        result = json.loads(path.read_text())
        result["source_unchanged"] = False
        ci.write_json(path, result)
        self.assertFalse(self.aggregate()[1]["checks"]["integration-0"])

    def test_missing_and_duplicate_benchmark_both_fail(self):
        path = Path("parts/integration-0/js-benchmark/report.json")
        path.unlink()
        self.assertEqual(self.aggregate()[0], 1)
        ci.write_json(path, {"fixture": True})
        ci.write_json(Path("parts/unit-build/js-benchmark/report.json"), {"fixture": True})
        self.assertEqual(self.aggregate()[0], 1)

    def test_changed_plan_and_mixed_source_are_rejected(self):
        self.prepared["plans"]["unit-build"].pop()
        ci.write_json(Path("inputs/inputs.json"), self.prepared)
        with self.assertRaisesRegex(ValueError, "current source plan"):
            self.aggregate()

    def test_capacity_limit_cannot_be_increased_or_disabled(self):
        for value in [0, -1, float("nan"), float("inf"), 11, True, "10"]:
            with self.assertRaises(ValueError):
                ci.capacity_limit(value)
        self.assertEqual(ci.capacity_limit(0.1), 0.1)

    def test_failed_retry_without_new_artifact_cannot_reuse_old_green_evidence(self):
        with mock.patch.object(ci, "current_inventory", return_value=self.inventory), \
                mock.patch.object(ci, "identity", return_value=self.identity):
            status = ci.aggregate(Path("inputs"), Path("parts"), Path("aggregate"), "failure")
        result = json.loads(Path("aggregate/gate.json").read_text())
        self.assertEqual(status, 1)
        self.assertFalse(result["checks"]["partition_jobs"])
        self.assertFalse(result["full_suite"])


class CacheBindingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        previous = Path.cwd()
        os.chdir(self.temporary.name)
        self.addCleanup(os.chdir, previous)
        self.root = Path.cwd()
        self.target = self.root / "target"
        self.target.mkdir()
        patch = mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": str(self.target)})
        patch.start()
        self.addCleanup(patch.stop)
        self.binding = {"workspace": str(self.root), "uid": os.geteuid(), "gid": os.getegid()}
        self.path = self.target / ".cargo-space-guard-worktree.json"
        self.backup = self.root / "cache-evidence/target-binding.json"
        ci.write_json(self.path, self.binding)

    def test_cache_cleanup_may_remove_only_the_metadata_we_saved(self):
        ci.save_cache_binding(self.backup)
        self.path.unlink()
        (self.target / "kept-artifact").write_text("preserve")
        ci.restore_cache_binding(self.backup)
        self.assertEqual(json.loads(self.path.read_text()), self.binding)
        self.assertEqual((self.target / "kept-artifact").read_text(), "preserve")

    def test_existing_foreign_binding_is_not_overwritten(self):
        ci.save_cache_binding(self.backup)
        foreign = dict(self.binding, workspace="/different-worktree")
        ci.write_json(self.path, foreign)
        with self.assertRaisesRegex(ValueError, "refusing to overwrite"):
            ci.restore_cache_binding(self.backup)
        self.assertEqual(json.loads(self.path.read_text()), foreign)

    def test_wrong_target_user_and_workspace_backups_are_rejected(self):
        ci.save_cache_binding(self.backup)
        original = json.loads(self.backup.read_text())
        for field, value in [("workspace", "/other"), ("uid", os.geteuid() + 1)]:
            changed = copy.deepcopy(original)
            changed["binding"][field] = value
            ci.write_json(self.backup, changed)
            with self.assertRaises(ValueError):
                ci.restore_cache_binding(self.backup)
        changed = copy.deepcopy(original)
        changed["target"] = str(self.root / "other-target")
        ci.write_json(self.backup, changed)
        with self.assertRaises(ValueError):
            ci.restore_cache_binding(self.backup)

    def test_symlink_binding_cannot_redirect_metadata_writes(self):
        ci.save_cache_binding(self.backup)
        self.path.unlink()
        destination = self.root / "unrelated-file"
        destination.write_text("preserve")
        self.path.symlink_to(destination)
        with self.assertRaises(ValueError):
            ci.restore_cache_binding(self.backup)
        self.assertEqual(destination.read_text(), "preserve")


class RunnerCleanupTests(unittest.TestCase):
    def test_only_documented_hosted_linux_sdk_environment_is_accepted(self):
        environment = {"RUNNER_ENVIRONMENT": "github-hosted", "RUNNER_OS": "Linux",
                       "ANDROID_HOME": str(ci.ANDROID_SDK), "ANDROID_SDK_ROOT": str(ci.ANDROID_SDK)}
        ci.validate_runner_sdk_environment(environment)
        for name, value in [("RUNNER_ENVIRONMENT", "self-hosted"), ("RUNNER_OS", "macOS"),
                            ("ANDROID_HOME", "/tmp/sdk"), ("ANDROID_SDK_ROOT", "/")]:
            with self.subTest(name=name):
                changed = dict(environment, **{name: value})
                with self.assertRaises(ValueError):
                    ci.validate_runner_sdk_environment(changed)
        for name in environment:
            with self.subTest(missing=name):
                changed = dict(environment)
                del changed[name]
                with self.assertRaises(ValueError):
                    ci.validate_runner_sdk_environment(changed)

    def test_cleanup_command_is_fixed_and_retains_capacity_evidence(self):
        environment = {"RUNNER_ENVIRONMENT": "github-hosted", "RUNNER_OS": "Linux",
                       "ANDROID_HOME": str(ci.ANDROID_SDK), "ANDROID_SDK_ROOT": str(ci.ANDROID_SDK)}
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with mock.patch.dict(os.environ, environment, clear=True), \
                    mock.patch.object(Path, "resolve", return_value=ci.ANDROID_SDK), \
                    mock.patch.object(Path, "is_symlink", return_value=False), \
                    mock.patch.object(Path, "exists", side_effect=[True, False]), \
                    mock.patch.object(ci, "snapshot") as snapshot, \
                    mock.patch.object(ci.subprocess, "run", return_value=mock.Mock(stdout="123 sdk")) as run:
                ci.clean_runner_sdk(output)
            self.assertEqual(run.call_args_list[-1],
                             mock.call(["sudo", "rm", "-rf", "--", "/usr/local/lib/android/sdk"], check=True))
            self.assertEqual(snapshot.call_args_list,
                             [mock.call(output, "before-sdk-cleanup"), mock.call(output, "after-sdk-cleanup")])
            self.assertTrue(json.loads((output / "sdk-cleanup.json").read_text())["complete"])

    def test_invalid_runner_never_invokes_cleanup(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(ci.subprocess, "run") as run:
            with self.assertRaises(ValueError):
                ci.clean_runner_sdk(Path("unused"))
            run.assert_not_called()


class WorkflowTests(unittest.TestCase):
    def setUp(self):
        workflow = SCRIPT.parents[1] / ".github/workflows/ci.yml"
        source = workflow.read_text()
        self.normal = source[source.index("  normal-test-prepare:"):source.index("  gate4-prepare:")]
        self.aggregate = self.normal[self.normal.index("  test:\n"):]

    def test_existing_required_test_check_waits_for_all_partitions(self):
        self.assertIn('test "$(rustc --version --verbose | awk \'/^host:/ {print $2}\')" = x86_64-unknown-linux-gnu', self.normal)
        self.assertIn("needs: [normal-test-prepare, normal-test-partition]", self.aggregate)
        self.assertIn("if: always() && needs.normal-test-prepare.result != 'skipped'", self.aggregate)
        self.assertIn("ci-test-partitions.py aggregate", self.aggregate)
        self.assertIn("--partition-job-result ${{ needs.normal-test-partition.result }}", self.aggregate)
        self.assertNotIn("continue-on-error", self.normal)
        self.assertIn("fail-fast: false", self.normal)

    def test_every_partition_uses_guarded_execution_and_preserves_all_artifacts(self):
        self.assertIn("partition: [unit-build, integration-0, integration-1, integration-2, integration-3]", self.normal)
        self.assertIn("CARGO_TARGET_DIR: ${{ github.workspace }}/../normal-test-target", self.normal)
        self.assertIn('CARGO_INCREMENTAL: "0"', self.normal)
        self.assertIn("ci-test-partitions.py run ${{ matrix.partition }}", self.normal)
        self.assertIn("name: normal-test-part-${{ matrix.partition }}-attempt-${{ github.run_attempt }}", self.normal)
        self.assertIn("pattern: normal-test-part-*", self.aggregate)
        self.assertNotIn("merge-multiple: true", self.aggregate)
        self.assertIn("--prefix normal-test-part-", self.aggregate)
        self.assertIn("pattern: normal-test-inputs-attempt-*", self.aggregate)
        self.assertIn("name: browser-x86_64-unknown-linux-gnu-interpreter-attempt-${{ github.run_attempt }}", self.aggregate)
        self.assertIn("name: js-benchmark-report-attempt-${{ github.run_attempt }}", self.aggregate)

    def test_workflow_target_is_guard_valid_and_matches_cache_target(self):
        spec = importlib.util.spec_from_file_location("workflow_guard", SCRIPT.with_name("cargo-space-guard.py"))
        guard = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = guard
        spec.loader.exec_module(guard)
        source = Path("/home/runner/work/omoikane/omoikane")
        expression = re.search(r"CARGO_TARGET_DIR: (.+)", self.normal).group(1)
        selected = Path(expression.replace("${{ github.workspace }}", str(source)))
        cache_relative = re.search(r'workspaces: "\. -> ([^"]+)"', self.normal).group(1)
        with mock.patch.object(guard, "worktree_root", return_value=source):
            with self.assertRaisesRegex(ValueError, "separate"):
                guard.validate_run_target(source / "target", source)
            validated = guard.validate_run_target(selected, source)
        self.assertEqual(validated, (source / cache_relative).resolve())

    def test_cache_has_only_the_main_unit_writer_and_capacity_evidence(self):
        self.assertIn("lookup-only: true", self.normal)
        self.assertIn("github.ref == 'refs/heads/main' && matrix.partition == 'unit-build'", self.normal)
        self.assertLess(self.normal.index("Bind the empty target"), self.normal.index("uses: Swatinem/rust-cache@v2"))
        self.assertIn("cargo-space-guard.py run --required-free-gib 2 --maximum-target-gib 10 -- true", self.normal)
        self.assertIn("save-cache-binding", self.normal)
        self.assertIn("restore-cache-binding", self.normal)
        for stage in ["before-cache", "after-cache", "after-fonts", "after-partition"]:
            self.assertIn(stage, self.normal)


if __name__ == "__main__":
    unittest.main()
