"""No native processes: verify diagnostic invocation and failure provenance."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "asan_diagnostic", Path(__file__).resolve().parents[1] / "diagnose-macos-wpt-asan.py")
DIAG = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DIAG)


class AsanDiagnosticTests(unittest.TestCase):
    def test_build_is_locked_targeted_and_does_not_run_full_suite(self):
        command = DIAG.build_command()
        self.assertIn("--locked", command)
        self.assertIn("--no-run", command)
        self.assertEqual(command[command.index("--test") + 1], "wpt_smoke")
        self.assertEqual(command[command.index("--features") + 1], "baseline-jit,jit-differential")
        self.assertEqual(command[command.index("--target") + 1], "aarch64-apple-darwin")
        self.assertEqual(command[1], "+1.98.1")

    def test_environment_prevents_flag_override_and_gate_report_overwrite(self):
        with patch.dict(os.environ, {"CARGO_ENCODED_RUSTFLAGS": "wrong", "WPT_UPSTREAM": "1",
                                     "OMOIKANE_JIT_GATE_REPORT_DIR": "original"}):
            env = DIAG.diagnostic_environment(Path("/evidence"), Path("/asan-target"))
        self.assertNotIn("CARGO_ENCODED_RUSTFLAGS", env)
        self.assertNotIn("WPT_UPSTREAM", env)
        self.assertNotIn("OMOIKANE_JIT_GATE_REPORT_DIR", env)
        self.assertIn("-Zsanitizer=address", env["RUSTFLAGS"])
        self.assertEqual(env["WPT_REPORT"], "/evidence/wpt.json")

    def test_failed_command_preserves_exit_and_log(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            records = []
            with patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], -6)):
                success = DIAG.record_command("run", ["wpt"], {}, folder, records)
            self.assertFalse(success)
            self.assertEqual(records[0]["exit"], -6)
            self.assertTrue((folder / "run.log").exists())

    def test_launch_failure_is_distinct_from_process_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            records = []
            with patch.object(subprocess, "run", side_effect=FileNotFoundError("missing compiler")):
                self.assertFalse(DIAG.record_command("build", ["cargo"], {}, Path(directory), records))
            self.assertIsNone(records[0]["exit"])
            self.assertIn("missing compiler", records[0]["error"])

    def test_executable_selection_ignores_build_scripts_and_rejects_ambiguity(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "build.log"
            artifact = {"reason": "compiler-artifact", "target": {"name": "wpt_smoke"},
                        "profile": {"test": True}, "executable": "/target/wpt"}
            log.write_text("warning\n" + json.dumps(artifact) + "\n" + json.dumps(artifact))
            self.assertEqual(DIAG.test_binary(log), Path("/target/wpt"))
            artifact["executable"] = "/target/other"
            log.write_text(log.read_text() + "\n" + json.dumps(artifact))
            with self.assertRaises(ValueError):
                DIAG.test_binary(log)

    def test_build_failure_never_runs_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            report = {"steps": []}
            with patch.object(DIAG.platform, "system", return_value="Darwin"), \
                 patch.object(DIAG.platform, "machine", return_value="arm64"), \
                 patch.object(DIAG, "provenance", return_value={}), \
                 patch.object(DIAG, "record_command", side_effect=[True, True, False]) as run, \
                 patch.object(DIAG, "test_binary") as select:
                self.assertFalse(DIAG.execute(Path(directory), Path("/target"), report))
            self.assertEqual(run.call_count, 3)
            select.assert_not_called()
            self.assertEqual(report["failed_stage"], "build")


if __name__ == "__main__":
    unittest.main()
