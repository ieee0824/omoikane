"""Prevent absent, stale or fallback-only evidence from approving a release."""
import contextlib
import copy
import importlib.util
import io
from pathlib import Path
import tempfile
import tarfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("gate5", Path(__file__).parents[1] / "jit-gate5.py")
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)


def acid3_report():
    expected = gate.load_expectation()
    return {mode: {key: expected[key] for key in ("score", "total", "index")} | {
        "log": "\n".join(expected["failed_lines"]), "termination_reason": "completed",
        "script_errors": [], "task_errors": [], "drive_errors": [],
    } for mode in ("faithful", "direct")}



def probe(target):
    rows = [{"seed": seed, "shape": shape, "sample": sample, "jit_enabled": enabled,
             "success": True, "expected": 42, "actual": 42, "elapsed_ns": 1000,
             "compiled_entries": int(enabled), "generated_code_bytes": 128 * int(enabled),
             "compile_time_ns": 200 * int(enabled), "property_guard_hits": int(enabled),
             "bailouts": 0}
            for seed in (305, 541, 542, 543) for shape in ("arith", "prop-mono")
            for sample in range(5) for enabled in (False, True)]
    return {"status": "passed", "architecture": target.split("-")[0], "cases": rows}


class GateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.identity = {"revision": "a" * 40, "lock_sha256": "b" * 64, "source_dirty": False}
        self.save("inputs/identity.json", self.identity)
        for target, (library, archive) in gate.TARGETS.items():
            identity = dict(self.identity, target=target)
            self.save(f"{target}/suite/result.json", {
                "identity": identity, "status": "passed",
                "test_results": [["100", "0", "0"],
                                 *[["1", "0", "0"] for _ in gate.SERIAL_SUITE_COMMANDS]],
                "steps": [{"name": "fetch-wpt", "exit": 0},
                          {"name": "full-suite", "exit": 0, "command": gate.SUITE_COMMAND},
                          *[{"name": name, "exit": 0, "command": command}
                            for name, command in gate.SERIAL_SUITE_COMMANDS]]})
            manifest = {"identity": identity, "default_library_features": [],
                        "diagnostic_features": ["baseline-jit"], "files": {
                            name: {"sha256": "c" * 64, "bytes": 10}
                            for name in (library, "omoikane.h", "omoikane-jit-smoke")}}
            manifest["files"].update({name: {"sha256": gate.digest(source),
                                            "bytes": source.stat().st_size}
                                      for name, source in gate.ENGINE_NOTICES.items()})
            self.save(f"{target}/package/result.json", {
                "identity": identity, "status": "passed", "archive": {
                    "name": archive, "bytes": 30, "manifest": manifest},
                "steps": [{"name": name, "exit": 0} for name in
                          ("default-library", "jit-probe-build", "ffi-smoke", "jit-smoke")]})
            self.save(f"{target}/suite/acid3.json", acid3_report())
            self.save(f"{target}/suite/wpt.json", {"summary": {"total": 69, "regression": 0},
                       "revision": Path("tests/wpt/revision.txt").read_text().strip()})
            self.save(f"{target}/suite/web-api.json", {"regressions": [], "total": 78})
            self.save(f"{target}/suite/native.json", {"status": "passed", "jit_policy": "execution-confirmed",
                       "architecture": target.split("-")[0], "cases": [{"success": True}] * 16})
            self.save(f"{target}/package/ffi.json", {"status": "passed", "cases": [{}, {}],
                       "library_sha256": "c" * 64})
            self.save(f"{target}/package/probe/probe.json", probe(target))

    def save(self, relative, value):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        gate.write(path, value)

    def decision(self, result="success"):
        with patch.object(gate, "output", return_value=self.identity["revision"]), contextlib.redirect_stdout(io.StringIO()):
            return gate.aggregate(self.root, result)

    def test_partition_preserves_original_rust_commands_exactly_once(self):
        core = gate.suite_commands("suite-core")
        wpt = gate.suite_commands("suite-wpt")
        self.assertEqual(core + wpt, [("full-suite", gate.SUITE_COMMAND),
                                     *gate.SERIAL_SUITE_COMMANDS])
        self.assertEqual(len(core), 8)
        self.assertEqual(wpt, [gate.SERIAL_SUITE_COMMANDS[-1]])
        self.assertEqual(len({label for label, _ in core + wpt}), 9)
        with self.assertRaises(ValueError):
            gate.suite_commands("unknown")

    def test_wpt_partition_executes_only_pinned_setup_and_original_wpt(self):
        target = next(iter(gate.TARGETS))
        identity = dict(self.identity, target=target)
        calls = []

        def execute(command, **kwargs):
            calls.append(command)
            self.assertEqual(kwargs["env"]["WPT_REQUIRED"], "1")
            self.assertEqual(kwargs["env"]["CARGO_TERM_COLOR"], "never")
            if command[0] == "cargo":
                log = kwargs["stdout"]
                log.write("     Running tests/wpt_smoke.rs (/tmp/deps/wpt_smoke-abc)\n")
                name = gate.SENSITIVE_TESTS[-1][2]
                if "--list" in command:
                    log.write(f"{name}: test\n")
                else:
                    log.write(f"test {name} ... ok\n")
                    log.write("test result: ok. 1 passed; 0 failed; 0 ignored\n")
            return SimpleNamespace(returncode=0)

        with patch.object(gate, "identity", return_value=identity), \
                patch.object(gate.subprocess, "run", side_effect=execute), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertTrue(gate.run_target("suite-wpt", target, self.root))
        self.assertEqual(calls, [["bash", "scripts/fetch-wpt.sh"],
                                 gate.inventory_command("suite-wpt"),
                                 gate.SERIAL_SUITE_COMMANDS[-1][1]])
        result = gate.read(self.root / target / "suite-wpt/result.json")
        self.assertEqual(result["kind"], "suite-wpt")
        self.assertEqual(result["status"], "passed")
        self.assertFalse((self.root / target / "suite-wpt/native.json").exists())
        environment = gate.read(self.root / target / "suite-wpt/suite-environment.json")
        self.assertEqual(environment["CARGO_TERM_COLOR"], "never")
        self.assertEqual(environment["WPT_REPORT"], str((self.root / target / "suite-wpt/wpt.json").resolve()))

    def split_evidence(self):
        self.identity["run_id"] = "12345"
        self.save("inputs/identity.json", self.identity)
        for target in gate.TARGETS:
            base = self.root / target
            original = gate.read(base / "suite/result.json")
            original["identity"]["run_id"] = "12345"
            for kind in gate.SUITE_PARTS:
                self.save(f"{target}/{kind}/result.json", {
                    "kind": kind, "attempt": "1",
                    "identity": dict(original["identity"], rustc="fixed rustc"),
                    "status": "passed",
                    "test_results": [["1", "0", "0"] for _ in gate.suite_commands(kind)],
                    "steps": [{"name": name, "command": command, "exit": 0} for name, command in
                              [("fetch-wpt", ["bash", "scripts/fetch-wpt.sh"]),
                               ("list-tests", gate.inventory_command(kind)),
                               *gate.suite_commands(kind)]],
                    "expected_tests": [["tests/wpt_smoke.rs", gate.SENSITIVE_TESTS[-1][2]]]
                        + ([] if kind == "suite-wpt" else
                           [["src/lib.rs", label] for label, _ in gate.suite_commands("suite-core")]),
                    "executed_tests": {label: [["tests/wpt_smoke.rs", gate.SENSITIVE_TESTS[-1][2]]]
                                       if kind == "suite-wpt" else [["src/lib.rs", label]]
                                       for label, _ in gate.suite_commands(kind)}})
            package = gate.read(base / "package/result.json")
            package["attempt"] = "2"
            package["identity"]["run_id"] = "12345"
            package["archive"]["manifest"]["identity"]["run_id"] = "12345"
            package["identity"]["rustc"] = "fixed rustc"
            package["archive"]["manifest"]["identity"]["rustc"] = "fixed rustc"
            gate.write(base / "package/result.json", package)
            for path in (base / "suite").iterdir():
                if path.name != "result.json":
                    destination = "suite-wpt" if path.name == "wpt.json" else "suite-core"
                    path.rename(base / destination / path.name)
            (base / "suite/result.json").unlink()

    def split_decision(self, result="success"):
        with patch.object(gate, "output", return_value=self.identity["revision"]), \
                patch.dict(gate.os.environ, {"GITHUB_RUN_ID": "12345"}), \
                contextlib.redirect_stdout(io.StringIO()):
            return gate.aggregate(self.root, result, split_suite=True)

    def test_split_complete_and_real_job_failure(self):
        self.split_evidence()
        self.assertTrue(self.split_decision())
        for result in ("failure", "cancelled", "skipped"):
            self.assertFalse(self.split_decision(result))

    def test_split_rejects_missing_duplicate_argv_and_identity_mismatch(self):
        self.split_evidence()
        base = self.root / next(iter(gate.TARGETS))
        path = base / "suite-wpt/result.json"
        original = gate.read(path)
        for change in ("missing", "duplicate", "argv", "rustc", "kind", "failed"):
            with self.subTest(change=change):
                result = copy.deepcopy(original)
                if change == "missing":
                    path.unlink()
                else:
                    if change == "duplicate":
                        result["steps"].append(result["steps"][-1])
                    elif change == "argv":
                        result["steps"][-1]["command"].remove("--exact")
                    elif change == "rustc":
                        result["identity"]["rustc"] = "different"
                    elif change == "kind":
                        result["kind"] = "suite-core"
                    else:
                        result["status"] = "failed"
                    gate.write(path, result)
                self.assertFalse(self.split_decision())
                gate.write(path, original)
        self.save(f"{base.name}/suite/result.json", original)
        self.assertFalse(self.split_decision())

    def test_split_provenance_allows_attempts_but_rejects_cross_run(self):
        self.split_evidence()
        self.assertTrue(self.split_decision())
        path = self.root / next(iter(gate.TARGETS)) / "package/result.json"
        original = gate.read(path)
        for change in ("run", "attempt", "missing-attempt"):
            result = copy.deepcopy(original)
            if change == "run":
                result["identity"]["run_id"] = "99999"
            elif change == "attempt":
                result["attempt"] = "0"
            else:
                result.pop("attempt")
            gate.write(path, result)
            self.assertFalse(self.split_decision())
        gate.write(path, original)

    def test_split_inventory_rejects_missing_duplicate_and_wrong_part_tests(self):
        self.split_evidence()
        base = self.root / next(iter(gate.TARGETS))
        path = base / "suite-core/result.json"
        original = gate.read(path)
        for change in ("missing", "duplicate", "unexpected", "overlapping-skip", "wrong-part"):
            with self.subTest(change=change):
                result = copy.deepcopy(original)
                rows = result["executed_tests"]["full-suite"]
                if change == "missing":
                    rows.clear()
                elif change == "duplicate":
                    rows.append(rows[0])
                elif change == "unexpected":
                    rows[0][1] = "unexpected_test"
                elif change == "overlapping-skip":
                    result["expected_tests"].append(["src/lib.rs", "prefix_sensitive_test_suffix"])
                else:
                    rows.append(["tests/wpt_smoke.rs", gate.SENSITIVE_TESTS[-1][2]])
                gate.write(path, result)
                self.assertFalse(self.split_decision())
        gate.write(path, original)
        self.assertTrue(self.split_decision())

    def test_doctest_compile_modes_match_unsuffixed_inventory(self):
        listing = """   Doc-tests omoikane
src/http/client.rs - Client (line 65): test
src/http/connection.rs - send (line 285): test
src/lib.rs - example (line 12): test
"""
        executed = """\x1b[1m\x1b[92m   Doc-tests\x1b[0m omoikane
test src/http/client.rs - Client (line 65) - compile ... ok
test src/http/connection.rs - send (line 285) - compile fail ... ok
test src/lib.rs - example (line 12) - should panic ... ok
"""
        self.assertEqual(gate.harness_tests(listing, listing=True),
                         gate.harness_tests(executed))

    def test_harness_inventory_retains_target_identity_and_duplicates(self):
        listing = """     Running unittests src/lib.rs (/tmp/deps/omoikane-abc)
same_name: test
     Running tests/example.rs (/tmp/deps/example-def)
same_name: test
   Doc-tests omoikane
src/lib.rs - example (line 4): test
"""
        executed = """\x1b[1m\x1b[92m     Running\x1b[0m unittests src/lib.rs (/other/deps/omoikane-123)
latest unrelated ... text
contest unrelated ... text
table rows=test same_name - should panic ... diagnostic emitted before status
ok
     Running tests/example.rs (/other/deps/example-456)
test same_name ... ok
test same_name ... ok
   Doc-tests omoikane
test src/lib.rs - example (line 4) ... ok
"""
        expected = gate.harness_tests(listing, listing=True)
        actual = gate.harness_tests(executed)
        self.assertEqual(expected, [["src/lib.rs", "same_name"],
                                    ["tests/example.rs", "same_name"],
                                    ["doctest:omoikane", "src/lib.rs - example (line 4)"]])
        self.assertEqual(actual, [*expected[:2], expected[1], expected[2]])
        with self.assertRaises(ValueError):
            gate.harness_tests("test missing_harness ... ok")

    def test_complete_evidence_passes(self):
        self.assertTrue(self.decision())

    def test_full_suite_uses_the_test_harness_default_parallelism(self):
        self.assertNotIn("--test-threads=1", gate.SUITE_COMMAND)
        self.assertIn("--include-ignored", gate.SUITE_COMMAND)
        self.assertIn("--no-fail-fast", gate.SUITE_COMMAND)
        skips = [gate.SUITE_COMMAND[index + 1] for index, argument in
                 enumerate(gate.SUITE_COMMAND[:-1]) if argument == "--skip"]
        self.assertEqual(skips, [test for _, _, test in gate.SENSITIVE_TESTS])
        self.assertEqual(len(gate.SERIAL_SUITE_COMMANDS), len(gate.SENSITIVE_TESTS))
        for (suffix, selector, test), (label, command) in zip(
                gate.SENSITIVE_TESTS, gate.SERIAL_SUITE_COMMANDS, strict=True):
            self.assertEqual(label, "sensitive-" + suffix)
            self.assertIn("--no-fail-fast", command)
            self.assertIn("--test-threads=1", command)
            self.assertIn("--exact", command)
            self.assertIn(test, command)
            for argument in selector:
                self.assertIn(argument, command)

    def test_missing_or_changed_sensitive_execution_never_passes(self):
        path = self.root / next(iter(gate.TARGETS)) / "suite/result.json"
        original = gate.read(path)
        for change in ("missing", "parallel", "command"):
            with self.subTest(change=change):
                result = copy.deepcopy(original)
                if change == "missing":
                    result["steps"].pop()
                elif change == "parallel":
                    result["steps"][-1]["command"].remove("--test-threads=1")
                else:
                    result["steps"][-1]["command"][-1] = "changed"
                gate.write(path, result)
                self.assertFalse(self.decision())
        gate.write(path, original)

    def test_missing_or_changed_engine_notices_reject_the_archive(self):
        path = self.root / next(iter(gate.TARGETS)) / "package/result.json"
        original = gate.read(path)
        for name in gate.ENGINE_NOTICES:
            for change in ("missing", "altered"):
                with self.subTest(notice=name, change=change):
                    result = copy.deepcopy(original)
                    files = result["archive"]["manifest"]["files"]
                    if change == "missing":
                        del files[name]
                    else:
                        files[name]["sha256"] = "changed"
                    gate.write(path, result)
                    self.assertFalse(self.decision())
        gate.write(path, original)

    def test_archive_preserves_executable_and_library_bytes(self):
        source = self.root / "packaged-probe"
        source.write_bytes(b"packaged executable bytes")
        source.chmod(0o755)
        library = self.root / "library"
        library.write_bytes(b"packaged library bytes")
        library.chmod(0o644)
        archive = self.root / "distribution.tar.gz"
        with tarfile.open(archive, "w:gz") as bundle:
            for path in (source, library):
                bundle.add(path, arcname=path.name)
        destination = self.root / "extracted"
        gate.extract_archive(archive, destination, {source.name, library.name})
        for path in (source, library):
            self.assertEqual((destination / path.name).read_bytes(), path.read_bytes())
            self.assertEqual((destination / path.name).stat().st_mode & 0o777,
                             path.stat().st_mode & 0o777)

    def test_unexpected_packaging_error_is_recorded_as_failed(self):
        target = next(iter(gate.TARGETS))
        folder = self.root / "failure-evidence"
        self_identity = dict(self.identity, target=target)
        (folder / "inputs").mkdir(parents=True)
        gate.write(folder / "inputs/identity.json", self_identity)
        with patch.object(gate, "identity", return_value=self_identity), \
                patch.object(gate.subprocess, "run", side_effect=TypeError("unsupported API")), \
                contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            self.assertFalse(gate.run_target("package", target, folder))
        result = gate.read(folder / target / "package/result.json")
        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["error"], "TypeError: unsupported API")

    def test_parallel_failure_still_runs_every_sensitive_test(self):
        target = next(iter(gate.TARGETS))
        folder = self.root / "suite-failure-evidence"
        suite_identity = dict(self.identity, target=target)
        self.save("suite-failure-evidence/inputs/identity.json", suite_identity)
        calls = []

        def execute(command, **_kwargs):
            calls.append(command)
            failed = command == gate.SUITE_COMMAND or command == gate.SERIAL_SUITE_COMMANDS[-1][1]
            return SimpleNamespace(returncode=int(failed))

        with patch.object(gate, "identity", return_value=suite_identity), \
                patch.object(gate.subprocess, "run", side_effect=execute), \
                contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            self.assertFalse(gate.run_target("suite", target, folder))
        result = gate.read(folder / target / "suite/result.json")
        self.assertEqual([step["name"] for step in result["steps"]],
                         ["fetch-wpt", "full-suite",
                          *[name for name, _ in gate.SERIAL_SUITE_COMMANDS]])
        self.assertEqual(len(calls), 2 + len(gate.SERIAL_SUITE_COMMANDS))
        self.assertIn("full-suite", result["error"])
        self.assertIn(gate.SERIAL_SUITE_COMMANDS[-1][0], result["error"])

    def test_every_target_and_report_is_required(self):
        for target in gate.TARGETS:
            for name in ("suite/result.json", "package/result.json", "suite/acid3.json", "suite/wpt.json",
                         "suite/web-api.json", "suite/native.json", "package/ffi.json", "package/probe/probe.json"):
                with self.subTest(target=target, report=name):
                    path = self.root / target / name
                    original = path.read_bytes()
                    path.unlink()
                    self.assertFalse(self.decision())
                    path.write_bytes(original)

    def test_failed_cancelled_and_skipped_jobs_never_pass(self):
        for result in ("failure", "cancelled", "skipped"):
            self.assertFalse(self.decision(result))

    def test_mismatched_revision_lock_or_host_never_passes(self):
        path = self.root / next(iter(gate.TARGETS)) / "package/result.json"
        original = gate.read(path)
        for key in ("revision", "lock_sha256", "target", "source_dirty"):
            value = copy.deepcopy(original)
            value["identity"][key] = True if key == "source_dirty" else "different"
            gate.write(path, value)
            self.assertFalse(self.decision())
        gate.write(path, original)

    def test_native_fallback_duplicate_or_wrong_result_never_passes(self):
        target = next(iter(gate.TARGETS))
        baseline = probe(target)
        for key, value in (("compiled_entries", 0), ("property_guard_hits", 0), ("actual", 43),
                           ("generated_code_bytes", 0), ("compile_time_ns", 0)):
            altered = copy.deepcopy(baseline)
            row = next(row for row in altered["cases"] if row["jit_enabled"] and row["shape"] == "prop-mono")
            row[key] = value
            self.assertFalse(gate.valid_probe(altered, target), key)
        duplicate = copy.deepcopy(baseline)
        duplicate["cases"][-1] = duplicate["cases"][0]
        self.assertFalse(gate.valid_probe(duplicate, target))

    def test_regressions_or_different_library_never_pass(self):
        target = next(iter(gate.TARGETS))
        for name, altered in (("suite/acid3.json", {"faithful": {"score": 99, "total": 100}}),
                              ("suite/wpt.json", {"summary": {"total": 69, "regression": 1}}),
                              ("suite/web-api.json", {"total": 78, "regressions": ["DOM"]}),
                              ("package/ffi.json", {"status": "passed", "cases": [{}, {}], "library_sha256": "wrong"})):
            path = self.root / target / name
            original = path.read_bytes()
            gate.write(path, altered)
            self.assertFalse(self.decision())
            path.write_bytes(original)


if __name__ == "__main__":
    unittest.main()
