"""Check that capacity evidence covers each filesystem and preserves errors."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import signal
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "gate5_space", Path(__file__).parents[1] / "gate5-space.py")
space = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(space)


class SpaceTests(unittest.TestCase):
    def test_explicit_target_and_cache_paths_have_separate_filesystems(self):
        root = Path("/workspace")
        paths = space.measured_paths(root, {
            "HOME": "/home/runner", "CARGO_TARGET_DIR": "/target/gate5",
            "CARGO_HOME": "cargo-cache", "RUNNER_TEMP": "/runner-temp",
        })
        self.assertEqual(paths["target"], Path("/target/gate5"))
        self.assertEqual(paths["cargo_home"], root / "cargo-cache")
        self.assertEqual(paths["runner_temp"], Path("/runner-temp"))
        self.assertEqual(paths["wpt"], root / ".cache/wpt")
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "evidence"
            with patch.object(space, "measure_path", side_effect=lambda p: {"path": str(p)}) as measure:
                result = space.snapshot(output, "before-cache", root, {})
            self.assertEqual(measure.call_count, 5)
            self.assertEqual(json.loads((output / "before-cache.json").read_text()), result)
            self.assertFalse((output / "before-cache.json.tmp").exists())

    def test_missing_target_still_records_its_existing_parent_filesystem(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "not-created" / "target"
            with patch.object(space.shutil, "disk_usage", return_value=(100, 30, 70)) as usage:
                result = space.measure_path(target)
            usage.assert_called_once_with(Path(directory))
            self.assertEqual(result["filesystem"], {"total": 100, "used": 30, "free": 70})
            self.assertFalse(result["exists"])
            self.assertNotIn("du", result)
            self.assertFalse(target.exists())

    def test_du_timeout_and_filesystem_errors_are_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(space.shutil, "disk_usage", side_effect=OSError("full")), \
                    patch.object(space.subprocess, "run", side_effect=subprocess.TimeoutExpired("du", 10)):
                result = space.measure_path(Path(directory))
            self.assertIn("full", result["filesystem_error"])
            self.assertIn("timed out", result["du"]["error"])

    def test_stage_cannot_escape_evidence_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            for stage in ["../escape", "", "/absolute", "after/cache"]:
                with self.assertRaises(ValueError):
                    space.snapshot(Path(directory) / "output", stage, Path(directory), {})
            self.assertFalse((Path(directory) / "output").exists())


class MonitorTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform.startswith("linux"), "isolated Linux subreaper fixture")
    def test_capacity_stop_reaps_group_after_leader_exits_first(self):
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run([sys.executable, str(Path(__file__).with_name("process_group_fixture.py")),
                                     "monitor", space.__file__, directory],
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            report = json.loads(result.stdout)
            self.assertEqual(report["exit"], 75)
            self.assertEqual(report["status"], "failed")
            self.assertFalse(report["group_exists_after_cleanup"])

    def test_unproven_cleanup_is_a_failure_even_during_cancellation(self):
        def incomplete(*args):
            state = args[-2]
            state.update(process=object(), pgid=123, signal=signal.SIGTERM)
            return 75

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(space, "poll_command", side_effect=incomplete), \
                    patch.object(space, "stop_process_group", side_effect=RuntimeError("group remains")):
                with self.assertRaises(RuntimeError):
                    space.run_monitored(["unused"], root / "evidence", root, {}, 100, 0.001)
            report = json.loads((root / "evidence/capacity-result.json").read_text())
            self.assertEqual(report["status"], "cancelled")
            self.assertEqual(report["exit"], 128 + signal.SIGTERM)
            self.assertEqual(report["signal"], signal.SIGTERM)
            self.assertIn("group remains", report["cleanup_error"]["message"])
            self.assertNotIn("cleanup", report)

    def test_command_failure_and_environment_are_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            environment = {"APP_SETTING": "preserved"}
            command = [sys.executable, "-c", "import os,sys; sys.exit(7 if os.environ['APP_SETTING']=='preserved' else 9)"]
            code = space.run_monitored(command, root / "evidence", root, environment,
                                       100, 0.001, usage=lambda _: (1000, 0, 1000))
            self.assertEqual(code, 7)
            result = json.loads((root / "evidence/capacity-result.json").read_text())
            self.assertEqual(result["command"], command)
            self.assertEqual(result["exit"], 7)
            self.assertEqual(result["status"], "failed")
            self.assertNotIn("capacity_failure", result)

    def test_initial_low_space_never_starts_the_suite(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(space.subprocess, "Popen") as start:
                code = space.run_monitored(["must-not-run"], root / "evidence", root,
                                           {}, 100, 0.001, usage=lambda _: (1000, 999, 1))
            start.assert_not_called()
            self.assertEqual(code, 75)
            result = json.loads((root / "evidence/capacity-result.json").read_text())
            self.assertEqual(result["status"], "failed")
            self.assertIn("target", result["capacity_failure"]["paths"])

    def test_low_space_stops_an_actual_running_process_group(self):
        calls = 0

        def usage(_):
            nonlocal calls
            calls += 1
            return (1000, 0, 1000) if calls <= 5 else (1000, 999, 1)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            command = [sys.executable, "-c", "import time; time.sleep(60)"]
            with patch.object(space, "stop_process_group", wraps=space.stop_process_group) as stop:
                code = space.run_monitored(command, root / "evidence", root, {},
                                           100, 0.001, usage=usage)
            self.assertEqual(code, 75)
            stop.assert_called_once()
            self.assertIsNotNone(stop.call_args.args[0].poll())
            rows = (root / "evidence/filesystem-samples.jsonl").read_text().splitlines()
            self.assertEqual(len(rows), 2)
            self.assertEqual(json.loads(rows[-1])["paths"]["target"]["free"], 1)

    def test_measurement_error_is_not_reported_as_a_capacity_or_test_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def failed_usage(_):
                raise OSError("unavailable filesystem")
            with self.assertRaises(OSError):
                space.run_monitored(["must-not-run"], root / "evidence", root,
                                    {}, 100, 0.001, usage=failed_usage)
            result = json.loads((root / "evidence/capacity-result.json").read_text())
            self.assertEqual(result["exit"], 1)
            self.assertEqual(result["status"], "failed")
            self.assertEqual(result["monitor_error"]["type"], "OSError")

    def test_cli_forwards_command_arguments_and_success(self):
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run([sys.executable, str(Path(space.__file__)), "run",
                                     "--poll-seconds", "0.001", "--minimum-free-gib", "0.000001",
                                     str(Path(directory) / "evidence"), "--", sys.executable,
                                     "-c", "import sys; assert sys.argv[1:] == ['--exact', 'one two']",
                                     "--exact", "one two"], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            record = json.loads((Path(directory) / "evidence/capacity-result.json").read_text())
            self.assertEqual(record["exit"], 0)
            self.assertEqual(record["status"], "passed")

    def test_invalid_poll_limits_do_not_spawn_a_command(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(space.subprocess, "Popen") as start:
                for interval in [0, -1, float("nan"), float("inf")]:
                    with self.assertRaises(ValueError):
                        space.run_monitored(["unused"], root / "evidence", root,
                                            {}, 100, interval)
            start.assert_not_called()

    def assert_signal_cancellation(self, number, resist_termination=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / "child-group.json"
            child_code = (
                "import subprocess,sys,os,json,signal,time\n"
                "from pathlib import Path\n"
                "child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'])\n"
                "def stop(signum,frame):\n"
                " child.wait(timeout=0.5)\n"
                " sys.exit(0)\n"
                "signal.signal(signal.SIGTERM,stop)\n"
                "Path(sys.argv[1]).write_text(json.dumps([os.getpid(),child.pid]))\n"
                "time.sleep(60)\n")
            if resist_termination:
                child_code = (
                    "import os,signal,sys,time,json\nfrom pathlib import Path\n"
                    "signal.signal(signal.SIGTERM,signal.SIG_IGN)\n"
                    "Path(sys.argv[1]).write_text(json.dumps([os.getpid(),os.getpid()]))\n"
                    "time.sleep(60)\n")
            command = [sys.executable, str(Path(space.__file__)), "run",
                       "--poll-seconds", "0.01", "--minimum-free-gib", "0.000001",
                       str(root / "evidence"), "--", sys.executable, "-c", child_code, str(marker)]
            monitor = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                deadline = time.monotonic() + 5
                while not marker.exists() and monitor.poll() is None and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(marker.exists(), "monitor did not start its child")
                leader, descendant = json.loads(marker.read_text())
                monitor.send_signal(number)
                stdout, stderr = monitor.communicate(timeout=5)
                self.assertEqual(monitor.returncode, 128 + number, stdout + stderr)
                result = json.loads((root / "evidence/capacity-result.json").read_text())
                self.assertEqual(result["status"], "cancelled")
                self.assertEqual(result["signal"], number)
                self.assertEqual(result["exit"], 128 + number)
                self.assertNotIn("capacity_failure", result)
                self.assertFalse((root / "evidence/capacity-result.json.tmp").exists())
                with self.assertRaises(ProcessLookupError):
                    os.killpg(leader, 0)
                with self.assertRaises(ProcessLookupError):
                    os.kill(descendant, 0)
            finally:
                if monitor.poll() is None:
                    monitor.kill()
                    monitor.communicate()

    def test_sigterm_cancels_and_reaps_the_child_group(self):
        self.assert_signal_cancellation(signal.SIGTERM)

    def test_sigint_cancels_and_reaps_the_child_group(self):
        self.assert_signal_cancellation(signal.SIGINT)

    def test_cancellation_kills_and_reaps_a_term_resistant_child(self):
        self.assert_signal_cancellation(signal.SIGTERM, resist_termination=True)

    def test_signal_handlers_are_restored_on_success_and_measurement_error(self):
        previous = {n: signal.getsignal(n) for n in (signal.SIGTERM, signal.SIGINT)}
        handler = lambda _signum, _frame: None
        try:
            for n in previous:
                signal.signal(n, handler)
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                code = space.run_monitored([sys.executable, "-c", "pass"], root / "success",
                                           root, {}, 100, 0.001, usage=lambda _: (1000, 0, 1000))
                self.assertEqual(code, 0)
                for n in previous:
                    self.assertIs(signal.getsignal(n), handler)
                with self.assertRaises(OSError):
                    space.run_monitored(["unused"], root / "error", root, {}, 100, 0.001,
                                        usage=lambda _: (_ for _ in ()).throw(OSError("unavailable")))
                for n in previous:
                    self.assertIs(signal.getsignal(n), handler)
        finally:
            for n, old in previous.items():
                signal.signal(n, old)

    def test_failed_atomic_result_write_keeps_previous_complete_json(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            previous = {"status": "old-observation"}
            (output / "capacity-result.json").write_text(json.dumps(previous))
            with patch.object(Path, "replace", side_effect=OSError("interrupted replacement")):
                with self.assertRaises(OSError):
                    space.save_terminal_result(output, {}, 7, {"signal": None})
            self.assertEqual(json.loads((output / "capacity-result.json").read_text()), previous)


if __name__ == "__main__":
    unittest.main()
