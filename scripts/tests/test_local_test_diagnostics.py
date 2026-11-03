"""Check selection integrity before supplemental isolation starts test bodies."""

from contextlib import redirect_stderr, redirect_stdout
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

from scripts.local_test_diagnostics import (
    Artifact, Case, Inventory, Profile, SensitiveTest, Target, audit_doctests, audit_output, build_plan,
    load_sensitive_tests, parse_artifacts, parse_listing, root_package_id,
)


LIB = Target("omoikane", "lib", "omoikane")
MEDIA = Target("omoikane", "test", "p1_media")
SENSITIVE = (
    SensitiveTest("slow", "lib", None, "js::slow"),
    SensitiveTest("media", "test", "p1_media", "media_case"),
)


def listing(names):
    return "".join(f"{name}: test\n" for name in names)


def inventory(target, names, ignored=()):
    return Inventory.from_outputs(target, listing(names), listing(ignored))


def fixtures():
    return (inventory(LIB, ("js::slow", "js::ordinary"), ("js::ordinary",)),
            inventory(MEDIA, ("quick_case", "media_case")))


class ListingTests(unittest.TestCase):
    def test_empty_listing_is_a_valid_zero_test_target(self):
        self.assertEqual(parse_listing(""), ())

    def test_listing_is_sorted_and_retains_unicode_names(self):
        self.assertEqual(parse_listing("z: test\nα::β: test\na: test\n"),
                         ("a", "z", "α::β"))

    def test_duplicate_listing_cannot_hide_duplicate_execution(self):
        with self.assertRaisesRegex(ValueError, "duplicate libtest"):
            parse_listing("same: test\nsame: test\n")

    def test_unknown_or_benchmark_lines_are_not_silently_dropped(self):
        for output in ("compiler error\n", "measure: benchmark\n", ": test\n"):
            with self.subTest(output=output), self.assertRaises(ValueError):
                parse_listing(output)

    def test_ignored_listing_must_belong_to_same_target_inventory(self):
        with self.assertRaisesRegex(ValueError, "unknown tests"):
            inventory(LIB, ("present",), ("absent",))

    def test_duplicate_ignored_listing_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "duplicate libtest"):
            inventory(LIB, ("present",), ("present", "present"))


class PlanTests(unittest.TestCase):
    def test_partition_covers_each_target_case_once(self):
        plan = build_plan(fixtures(), SENSITIVE, Profile())
        selected = [case for group in plan.groups for case in group.cases]
        expected = {Case(inv.target, name) for inv in fixtures() for name in inv.names}
        self.assertEqual(set(selected), expected)
        self.assertEqual(len(selected), len(expected))
        self.assertEqual(len(plan.groups), 4)

    def test_ignored_policy_changes_execution_without_changing_coverage(self):
        regular = build_plan(fixtures(), SENSITIVE, Profile())
        included = build_plan(fixtures(), SENSITIVE, Profile(include_ignored=True))
        self.assertEqual(sum(len(g.cases) for g in regular.groups), 4)
        self.assertEqual(sum(len(g.runnable) for g in regular.groups), 3)
        self.assertEqual(sum(len(g.runnable) for g in included.groups), 4)
        self.assertTrue(all("--include-ignored" in g.arguments for g in included.groups))
        self.assertTrue(all("--include-ignored" not in g.arguments for g in regular.groups))

    def test_sensitive_ignored_case_remains_explicitly_ignored(self):
        invs = (inventory(LIB, ("js::slow",), ("js::slow",)), fixtures()[1])
        plan = build_plan(invs, SENSITIVE, Profile())
        group = next(g for g in plan.groups if g.label == "sensitive-slow")
        self.assertEqual(group.cases, (Case(LIB, "js::slow"),))
        self.assertEqual(group.ignored, frozenset(group.cases))
        self.assertFalse(group.runnable)

    def test_parallel_does_not_inherit_serial_exact_or_thread_count(self):
        plan = build_plan(fixtures(), SENSITIVE, Profile())
        for group in plan.groups[:2]:
            self.assertNotIn("--exact", group.arguments)
            self.assertNotIn("--test-threads=1", group.arguments)
            self.assertEqual(group.arguments.count("--skip"), 2)
        for group in plan.groups[2:]:
            self.assertIn("--exact", group.arguments)
            self.assertIn("--test-threads=1", group.arguments)
            self.assertNotIn("--skip", group.arguments)

    def test_features_are_explicit_and_never_implicitly_enable_jit(self):
        self.assertEqual(build_plan(fixtures(), SENSITIVE, Profile()).profile.features, ())
        profile = Profile(features=("gc-profile",))
        self.assertEqual(build_plan(fixtures(), SENSITIVE, profile).profile, profile)

    def test_missing_exact_case_is_rejected_before_any_execution(self):
        invs = (inventory(LIB, ("js::ordinary",)), fixtures()[1])
        with self.assertRaisesRegex(ValueError, "expected 1 exact case, got 0"):
            build_plan(invs, SENSITIVE, Profile())

    def test_same_name_in_wrong_target_does_not_satisfy_exact_selector(self):
        invs = (fixtures()[0], inventory(Target("omoikane", "test", "other"),
                                        ("media_case",)))
        with self.assertRaisesRegex(ValueError, "expected 1 exact case, got 0"):
            build_plan(invs, SENSITIVE, Profile())

    def test_multiple_library_matches_are_rejected(self):
        invs = (*fixtures(), inventory(Target("other-package", "lib", "other"),
                                      ("js::slow",)))
        with self.assertRaisesRegex(ValueError, "expected 1 exact case, got 2"):
            build_plan(invs, SENSITIVE, Profile())

    def test_substring_collision_in_same_target_is_rejected(self):
        invs = (inventory(LIB, ("js::slow", "js::slow_extra")), fixtures()[1])
        with self.assertRaisesRegex(ValueError, "substring skip collision"):
            build_plan(invs, SENSITIVE, Profile())

    def test_substring_collision_in_other_target_is_rejected(self):
        invs = (*fixtures(), inventory(Target("omoikane", "bin", "helper"),
                                      ("prefix_js::slow_suffix",)))
        with self.assertRaisesRegex(ValueError, "substring skip collision"):
            build_plan(invs, SENSITIVE, Profile())

    def test_duplicate_sensitive_case_is_rejected(self):
        duplicate = SensitiveTest("other-label", "lib", None, "js::slow")
        with self.assertRaisesRegex(ValueError, "multiple sensitive definitions"):
            build_plan(fixtures(), (*SENSITIVE, duplicate), Profile())

    def test_duplicate_labels_cannot_overwrite_group_evidence(self):
        duplicate = SensitiveTest("slow", "test", "p1_media", "media_case")
        with self.assertRaisesRegex(ValueError, "duplicate sensitive labels"):
            build_plan(fixtures(), (SENSITIVE[0], duplicate), Profile())

    def test_duplicate_target_inventory_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "duplicate target inventory"):
            build_plan((*fixtures(), fixtures()[0]), SENSITIVE, Profile())

    def test_inventory_order_does_not_change_the_plan(self):
        self.assertEqual(build_plan(fixtures(), SENSITIVE, Profile()),
                         build_plan(tuple(reversed(fixtures())), SENSITIVE, Profile()))

    def test_empty_test_target_is_retained_in_parallel_plan(self):
        empty = inventory(Target("omoikane", "bin", "zero-tests"), ())
        plan = build_plan((*fixtures(), empty), SENSITIVE, Profile())
        groups = [g for g in plan.groups if g.target == empty.target]
        self.assertEqual(len(groups), 1)
        self.assertEqual(groups[0].cases, ())

    def test_empty_inventory_or_definitions_are_rejected(self):
        for invs, tests in (((), SENSITIVE), (fixtures(), ())):
            with self.subTest(invs=invs), self.assertRaises(ValueError):
                build_plan(invs, tests, Profile())


class SelectorImportTests(unittest.TestCase):
    def test_import_reuses_gate5_without_running_main_or_leaking_search_path(self):
        root = Path(__file__).resolve().parents[2]
        search_path = sys.path.copy()
        captured = io.StringIO()
        with patch("subprocess.run", side_effect=AssertionError("unexpected process")), \
                patch("subprocess.check_output", side_effect=AssertionError("unexpected process")), \
                redirect_stdout(captured), redirect_stderr(captured):
            selectors = load_sensitive_tests(root)
        self.assertEqual(sys.path, search_path)
        self.assertEqual(captured.getvalue(), "")
        self.assertEqual(len(selectors), 8)
        self.assertIn(SensitiveTest("node-lifetime", "lib", None,
            "js::node_lifetime_tests::old_document_generations_are_kept_by_references_without_an_eviction_limit"),
            selectors)
        self.assertIn(SensitiveTest("p1-media", "test", "p1_media",
            "media_changes_visit_nested_shadow_frames_and_skip_detached_documents"), selectors)


def artifact_message():
    return {"reason": "compiler-artifact", "package_id": "root-package",
            "profile": {"test": True}, "executable": "/target/omoikane-tests",
            "target": {"kind": ["rlib", "cdylib"], "name": "omoikane"},
            "features": ["jit-differential", "baseline-jit"]}


def cargo_output(*messages):
    return "\n".join(json.dumps(message) for message in (
        *messages, {"reason": "build-finished", "success": True})) + "\n"


class CargoReaderTests(unittest.TestCase):
    def test_root_package_uses_manifest_not_workspace_member_order(self):
        metadata = {"version": 1, "packages": [
            {"id": "dependency", "manifest_path": "/repo/engine/Cargo.toml"},
            {"id": "root-package", "manifest_path": "/repo/Cargo.toml"}]}
        self.assertEqual(root_package_id(json.dumps(metadata), Path("/repo/Cargo.toml")),
                         "root-package")

    def test_missing_or_duplicate_root_manifest_is_rejected(self):
        package = {"id": "root-package", "manifest_path": "/repo/Cargo.toml"}
        for packages in ([], [package, package]):
            with self.subTest(packages=packages), self.assertRaises(ValueError):
                root_package_id(json.dumps({"version": 1, "packages": packages}),
                                Path("/repo/Cargo.toml"))

    def test_metadata_schema_is_checked_once_at_reader(self):
        for metadata in ([], {"version": True}, {"version": 2},
                         {"version": 1, "packages": "not-array"}):
            with self.subTest(metadata=metadata), self.assertRaises(ValueError):
                root_package_id(json.dumps(metadata), Path("/repo/Cargo.toml"))

    def test_library_kinds_and_resolved_features_are_normalized(self):
        result, = parse_artifacts(cargo_output(artifact_message()), "root-package")
        self.assertEqual(result.target, Target("root-package", "lib", "omoikane"))
        self.assertEqual(result.features, ("baseline-jit", "jit-differential"))
        self.assertEqual(result.executable, Path("/target/omoikane-tests"))

    def test_dependency_and_non_test_executables_are_not_root_tests(self):
        dependency = artifact_message()
        dependency["package_id"] = "dependency"
        normal_binary = artifact_message()
        normal_binary["profile"] = {"test": False}
        result = parse_artifacts(cargo_output(dependency, normal_binary, artifact_message()),
                                 "root-package")
        self.assertEqual(len(result), 1)

    def test_integration_binary_example_kinds_are_retained(self):
        for kind in ("test", "bin", "example"):
            message = artifact_message()
            message["target"]["kind"] = [kind]
            with self.subTest(kind=kind):
                result, = parse_artifacts(cargo_output(message), "root-package")
                self.assertEqual(result.target.kind, kind)

    def test_null_executable_does_not_become_a_test_binary(self):
        library = artifact_message()
        library["executable"] = None
        result = parse_artifacts(cargo_output(library, artifact_message()), "root-package")
        self.assertEqual(len(result), 1)

    def test_incomplete_failed_or_empty_build_is_rejected(self):
        for output in (json.dumps(artifact_message()),
                       json.dumps({"reason": "build-finished", "success": False}),
                       cargo_output()):
            with self.subTest(output=output), self.assertRaises(ValueError):
                parse_artifacts(output, "root-package")

    def test_non_json_and_trailing_output_cannot_look_complete(self):
        valid = cargo_output(artifact_message())
        for output in ("proc macro message\n" + valid, valid + "unexpected\n",
                       valid + json.dumps({"reason": "build-finished", "success": True})):
            with self.subTest(output=output), self.assertRaises(ValueError):
                parse_artifacts(output, "root-package")

    def test_duplicate_targets_or_executable_paths_are_rejected(self):
        original = artifact_message()
        other = copy.deepcopy(original)
        for change in ("target", "executable"):
            if change == "target":
                other["target"]["name"] = "another-target"
            else:
                other = copy.deepcopy(original)
                other["executable"] = "/target/another-binary"
            with self.subTest(change=change), self.assertRaisesRegex(ValueError, "duplicate"):
                parse_artifacts(cargo_output(original, other), "root-package")

    def test_invalid_root_artifact_fields_fail_preflight(self):
        changes = (
            {"profile": {"test": 1}}, {"executable": 7}, {"executable": "relative-path"},
            {"target": {"kind": "lib", "name": "omoikane"}},
            {"target": {"kind": ["bench"], "name": "measure"}},
            {"features": "jit"}, {"features": ["a", "a"]},
        )
        for change in changes:
            message = artifact_message() | change
            with self.subTest(change=change), self.assertRaises(ValueError):
                parse_artifacts(cargo_output(message), "root-package")


def pretty_output(name="js::slow", outcome="ok", filtered=1):
    state = outcome.split(",", 1)[0]
    return ("running 1 test\n"
            f"test {name} ... {outcome}\n\n"
            f"test result: {'FAILED' if state == 'FAILED' else 'ok'}. "
            f"{int(state == 'ok')} passed; {int(state == 'FAILED')} failed; "
            f"{int(state == 'ignored')} ignored; 0 measured; {filtered} filtered out; "
            "finished in 0.01s\n")


class ObservationTests(unittest.TestCase):
    def setUp(self):
        self.plan = build_plan(fixtures(), SENSITIVE, Profile())
        self.group = next(g for g in self.plan.groups if g.label == "sensitive-slow")
        self.inventory = fixtures()[0]

    def audit(self, output, exit_code=0):
        return audit_output(self.group, self.inventory, output, exit_code)

    def test_complete_success_matches_exact_case_and_summary(self):
        result = self.audit(pretty_output())
        self.assertTrue(result.valid)
        self.assertTrue(result.passed)
        self.assertEqual(result.outcomes, (("js::slow", "ok"),))

    def test_valid_test_failure_is_preserved_without_becoming_invalid(self):
        result = self.audit(pretty_output(outcome="FAILED"), 101)
        self.assertTrue(result.valid)
        self.assertFalse(result.passed)
        self.assertEqual(result.summary.failed, 1)

    def test_captured_failure_output_is_not_an_additional_case(self):
        text = pretty_output(outcome="FAILED")
        text = text.replace("\n\ntest result:",
                            "\n\nfailures:\n---- js::slow stdout ----\n"
                            "test unexpected ... ok\n\ntest result:")
        self.assertTrue(self.audit(text, 101).valid)

    def test_should_panic_label_is_not_part_of_case_name(self):
        self.assertTrue(self.audit(pretty_output(name="js::slow - should panic")).passed)

    def test_ignored_reason_is_recorded_as_policy_ignored(self):
        group = next(g for g in self.plan.groups if g.label == "parallel-000")
        text = pretty_output(name="js::ordinary", outcome="ignored, manual utility")
        result = audit_output(group, self.inventory, text, 0)
        self.assertTrue(result.passed)
        self.assertEqual(result.outcomes, (("js::ordinary", "ignored"),))

    def test_ignored_case_cannot_disappear_or_become_executed_silently(self):
        group = next(g for g in self.plan.groups if g.label == "parallel-000")
        result = audit_output(group, self.inventory, pretty_output(name="js::ordinary"), 0)
        self.assertFalse(result.passed)
        self.assertIn("ignored result policy mismatch", result.problems)

    def test_crash_or_absent_summary_is_incomplete(self):
        for output in ("", "running 1 test\ntest js::slow ... ok\n"):
            with self.subTest(output=output):
                result = self.audit(output, -9)
                self.assertFalse(result.valid)
                self.assertFalse(result.passed)

    def test_duplicate_or_wrong_case_results_fail_coverage(self):
        for text in (pretty_output(name="wrong"),
                     pretty_output().replace("\n\ntest result:",
                                             "\ntest js::slow ... ok\n\ntest result:")):
            with self.subTest(text=text):
                self.assertFalse(self.audit(text).passed)

    def test_exit_status_and_failed_count_must_agree(self):
        self.assertFalse(self.audit(pretty_output(), 101).valid)
        self.assertFalse(self.audit(pretty_output(outcome="FAILED"), 0).valid)

    def test_summary_and_running_counts_must_match_inventory(self):
        for text in (pretty_output(filtered=0), pretty_output().replace("1 passed", "2 passed"),
                     pretty_output().replace("running 1 test", "running 2 tests")):
            with self.subTest(text=text):
                self.assertFalse(self.audit(text).valid)

    def test_duplicate_summary_is_not_a_success(self):
        self.assertFalse(self.audit(pretty_output() + pretty_output()).valid)

    def test_zero_selected_parallel_group_is_validated(self):
        invs = (inventory(LIB, ("js::slow",)), fixtures()[1])
        plan = build_plan(invs, SENSITIVE, Profile())
        group = next(g for g in plan.groups if g.label == "parallel-000")
        output = ("running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; "
                  "0 measured; 1 filtered out; finished in 0.00s\n")
        self.assertTrue(audit_output(group, invs[0], output, 0).passed)


class DoctestTests(unittest.TestCase):
    def test_completed_batches_are_retained_separately(self):
        result = audit_doctests(pretty_output(filtered=0) + pretty_output(filtered=0), 0)
        self.assertTrue(result.passed)
        self.assertEqual(len(result.summaries), 2)

    def test_missing_failed_or_filtered_doctests_cannot_pass(self):
        for output, code in (("", 0), (pretty_output(outcome="FAILED", filtered=0), 101),
                             (pretty_output(filtered=1), 0), (pretty_output(filtered=0), 101)):
            with self.subTest(output=output, code=code):
                self.assertFalse(audit_doctests(output, code).passed)


CLI_SPEC = importlib.util.spec_from_file_location(
    "local_diagnostics_cli_tests", Path(__file__).resolve().parents[1] / "local-test-diagnostics.py")
cli = importlib.util.module_from_spec(CLI_SPEC)
sys.modules[CLI_SPEC.name] = cli
_search_path = sys.path.copy()
try:
    CLI_SPEC.loader.exec_module(cli)
finally:
    sys.path[:] = _search_path


@unittest.skipIf(os.name == "nt", "native executable fixture uses a POSIX shebang")
class ExecutionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.output = self.root / "evidence"
        self.output.mkdir()
        self.counter = self.root / "calls.txt"
        self.inv = inventory(LIB, ("js::ordinary", "js::slow"))
        self.plan = build_plan((self.inv,), SENSITIVE[:1], Profile())

    def fixture(self, fail_first=False):
        executable = self.root / "fixture.py"
        source = '''import os, sys
exact = '--exact' in sys.argv
name = 'js::slow' if exact else 'js::ordinary'
with open(__COUNTER__, 'a') as calls:
    calls.write(name + '\\n')
print('running 1 test', flush=True)
print('test ' + name + ' ... ', end='', flush=True)
os.write(2, b'test stderr_only ... ok\\n')
failed = __FAIL_FIRST__ and not exact
print('FAILED' if failed else 'ok', flush=True)
print('\\ntest result: ' + ('FAILED' if failed else 'ok') + '. '
      + str(int(not failed)) + ' passed; ' + str(int(failed))
      + ' failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s')
sys.exit(101 if failed else 0)
'''
        source = source.replace("__COUNTER__", repr(str(self.counter)))
        source = source.replace("__FAIL_FIRST__", repr(fail_first))
        executable.write_text("#!" + sys.executable + "\n" + source)
        executable.chmod(0o755)
        return {LIB: (Artifact(LIB, executable, ()), cli.digest(executable))}

    def report(self):
        return {"status": "running", "steps": [],
                "groups": [{"name": group.label, "status": "not-run"} for group in self.plan.groups],
                "doctests": {"status": "not-run"}}

    def test_stderr_cannot_split_or_spoof_stdout_results(self):
        report = self.report()
        session = cli.Session(self.root, self.output, dict(os.environ), report)
        with redirect_stdout(io.StringIO()):
            session.run_groups(self.plan, self.fixture())
        self.assertTrue(all(row["status"] == "passed" for row in report["groups"]))
        for step in report["steps"]:
            self.assertIn("stderr_only", (self.output / step["stderr"]).read_text())
            self.assertNotIn("stderr_only", (self.output / step["stdout"]).read_text())
            self.assertEqual(step["stdout_sha256"], cli.digest(self.output / step["stdout"]))

    def test_failure_continues_remaining_groups_and_doctests_and_exits_nonzero(self):
        report = self.report()
        cli.save_report(self.output, report)
        pinned = self.fixture(fail_first=True)
        plan = self.plan

        def prepare(session, profile):
            session.report["identity_before"] = {"revision": "unchanged"}
            return plan, pinned

        def doctests(session, profile):
            session.report["doctests"]["status"] = "passed"

        options = type("Options", (), {"output": self.output, "features": "",
                                      "include_ignored": False, "plan_only": False})()
        with patch.object(cli.Session, "prepare", prepare), \
                patch.object(cli.Session, "run_doctests", doctests), \
                patch.object(cli.Session, "identity", return_value={"revision": "unchanged"}), \
                redirect_stdout(io.StringIO()):
            code = cli.worker(options)
        self.assertEqual(code, 1)
        self.assertEqual(cli.finalize(self.output, code), 1)
        stored = json.loads((self.output / "manifest.json").read_text())
        self.assertEqual([row["status"] for row in stored["groups"]], ["failed", "passed"])
        self.assertEqual(stored["doctests"]["status"], "passed")
        self.assertEqual(stored["status"], "failed")
        self.assertEqual(self.counter.read_text().splitlines(), ["js::ordinary", "js::slow"])

    def test_changed_binary_is_not_executed(self):
        pinned = self.fixture()
        executable = pinned[LIB][0].executable
        executable.write_text(executable.read_text() + "\n# changed after pin\n")
        report = self.report()
        session = cli.Session(self.root, self.output, dict(os.environ), report)
        session.run_groups(self.plan, pinned)
        self.assertFalse(self.counter.exists())
        self.assertTrue(all(row["status"] == "invalid" for row in report["groups"]))

    def test_guard_stop_preserves_incomplete_and_not_run_groups(self):
        report = self.report()
        report["groups"][0]["status"] = "running"
        report["steps"] = [{"name": "active", "status": "running"}]
        cli.save_report(self.output, report)
        self.assertEqual(cli.finalize(self.output, 75), 75)
        stored = json.loads((self.output / "manifest.json").read_text())
        self.assertEqual(stored["status"], "resource-blocked")
        self.assertEqual([row["status"] for row in stored["groups"]], ["incomplete", "not-run"])
        self.assertEqual(stored["doctests"]["status"], "not-run")
        self.assertEqual(stored["groups"][1]["stop_reason"], "resource-blocked")

    def test_worker_success_cannot_override_guard_failure_or_missing_terminal(self):
        for worker_status, guard_exit in (("passed", 2), (None, 0)):
            report = self.report()
            if worker_status:
                report["worker_status"] = worker_status
            cli.save_report(self.output, report)
            self.assertNotEqual(cli.finalize(self.output, guard_exit), 0)
            self.assertEqual(json.loads((self.output / "manifest.json").read_text())["status"],
                             "failed")

    def test_plan_only_is_not_reported_as_executed_success(self):
        report = self.report() | {"worker_status": "planned"}
        cli.save_report(self.output, report)
        self.assertEqual(cli.finalize(self.output, 0), 0)
        stored = json.loads((self.output / "manifest.json").read_text())
        self.assertEqual(stored["status"], "planned")
        self.assertTrue(all(row["status"] == "not-run" for row in stored["groups"]))

    def test_corrupt_manifest_is_preserved_and_cannot_finalize_success(self):
        manifest = self.output / "manifest.json"
        manifest.write_bytes(b'{"partial":')
        with self.assertRaises(ValueError):
            cli.finalize(self.output, 0)
        self.assertEqual(manifest.read_bytes(), b'{"partial":')

    def test_existing_evidence_directory_is_never_reused(self):
        manifest = self.output / "manifest.json"
        manifest.write_text("original evidence")
        with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(self.root / "target")}), \
                patch("subprocess.Popen", side_effect=AssertionError("unexpected process")), \
                self.assertRaises(FileExistsError):
            cli.main([str(self.output)])
        self.assertEqual(manifest.read_text(), "original evidence")

    def test_wpt_required_is_presence_based_even_when_value_is_zero(self):
        (self.root / "tests/wpt").mkdir(parents=True)
        (self.root / "tests/wpt/revision.txt").write_text("pinned-revision\n")
        (self.root / "tests/wpt/manifest.json").write_text("{}")
        checkout = self.root / "checkout"
        (checkout / "resources").mkdir(parents=True)
        (checkout / "resources/testharness.js").write_text("// fixture")
        session = cli.Session(self.root, self.output,
                              {"WPT_ROOT": str(checkout), "WPT_REQUIRED": "0"}, self.report())
        inputs = session.wpt_inputs()
        self.assertTrue(inputs["required_present"])
        self.assertEqual(inputs["required_value"], "0")
        self.assertTrue(inputs["testharness_present"])
        self.assertIsNone(inputs["checkout_revision"])
        self.assertIn("not attested", inputs["scope"])

    def test_plan_only_does_not_start_group_or_doctest_processes(self):
        report = self.report()
        cli.save_report(self.output, report)
        plan, pinned = self.plan, self.fixture()

        def prepare(session, profile):
            session.report["identity_before"] = {"revision": "unchanged"}
            return plan, pinned

        options = type("Options", (), {"output": self.output, "features": "",
                                      "include_ignored": False, "plan_only": True})()
        with patch.object(cli.Session, "prepare", prepare), \
                patch.object(cli.Session, "run_groups", side_effect=AssertionError("test body ran")), \
                patch.object(cli.Session, "run_doctests", side_effect=AssertionError("doctest ran")), \
                patch.object(cli.Session, "identity", return_value={"revision": "unchanged"}):
            self.assertEqual(cli.worker(options), 0)
        self.assertFalse(self.counter.exists())
        self.assertEqual(cli.finalize(self.output, 0), 0)
        self.assertEqual(json.loads((self.output / "manifest.json").read_text())["status"], "planned")

    def test_source_change_prevents_success_even_after_valid_plan(self):
        report = self.report()
        cli.save_report(self.output, report)
        plan, pinned = self.plan, self.fixture()

        def prepare(session, profile):
            session.report["identity_before"] = {"revision": "before"}
            return plan, pinned

        options = type("Options", (), {"output": self.output, "features": "",
                                      "include_ignored": False, "plan_only": True})()
        with patch.object(cli.Session, "prepare", prepare), \
                patch.object(cli.Session, "identity", return_value={"revision": "after"}):
            self.assertEqual(cli.worker(options), 1)
        self.assertFalse(json.loads((self.output / "manifest.json").read_text())["source_unchanged"])

    def test_doctests_use_same_explicit_profile_and_stdout_only(self):
        stdout = self.output / "doc.stdout"
        stderr = self.output / "doc.stderr"
        stdout.write_text(pretty_output(filtered=0))
        stderr.write_text(pretty_output(outcome="FAILED", filtered=0))
        report = self.report()
        session = cli.Session(self.root, self.output, dict(os.environ), report)
        with patch.object(session, "step", return_value=cli.Step(0, stdout, stderr)) as step:
            session.run_doctests(Profile(features=("gc-profile",), include_ignored=True))
        self.assertEqual(report["doctests"]["status"], "passed")
        self.assertEqual(step.call_args.args[1], ["cargo", "test", "--locked", "--doc", "--features",
                                                "gc-profile", "--", "--include-ignored"])

    def test_repeated_interrupts_preserve_actual_guard_exit(self):
        process = Mock()
        process.wait.side_effect = [KeyboardInterrupt(), KeyboardInterrupt(), 75]
        code, interrupted = cli.wait_guard(process)
        self.assertEqual((code, interrupted), (75, True))
        self.assertEqual(process.wait.call_count, 3)
        cli.save_report(self.output, self.report())
        self.assertEqual(cli.finalize(self.output, code, interrupted), 75)
        report = json.loads((self.output / "manifest.json").read_text())
        self.assertEqual(report["guard_exit"], 75)
        self.assertTrue(report["interrupted"])

    def test_outer_interrupt_does_not_turn_guard_zero_into_success(self):
        cli.save_report(self.output, self.report() | {"worker_status": "passed"})
        self.assertEqual(cli.finalize(self.output, 0, interrupted=True), 130)
        report = json.loads((self.output / "manifest.json").read_text())
        self.assertEqual(report["guard_exit"], 0)
        self.assertEqual(report["status"], "interrupted")

    def test_signal_exit_is_retained_but_cli_exit_is_positive(self):
        cli.save_report(self.output, self.report())
        self.assertEqual(cli.finalize(self.output, -9), 1)
        self.assertEqual(json.loads((self.output / "manifest.json").read_text())["guard_exit"], -9)


if __name__ == "__main__":
    unittest.main()
