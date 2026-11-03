"""Fail closed when process-group cleanup cannot be verified."""
import importlib.util
import subprocess
import os
import signal
import select
import sys
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("group_guard", Path(__file__).parents[1] / "cargo-space-guard.py")
guard = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = guard
spec.loader.exec_module(guard)

class GroupObservationTests(unittest.TestCase):
    def test_live_member_detected_without_leader(self):
        with patch.object(guard.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "99 S\n100 Z\n")):
            self.assertTrue(guard.process_group_has_live_members(99))
            self.assertFalse(guard.process_group_has_live_members(100))

    def test_missing_group_is_absent(self):
        with patch.object(guard.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "99 S\n")):
            self.assertFalse(guard.process_group_has_live_members(100))

    def test_unreadable_process_list_is_not_absence(self):
        with patch.object(guard.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "ps")):
            with self.assertRaises(subprocess.CalledProcessError):
                guard.process_group_has_live_members(100)

    def test_malformed_process_list_is_not_absence(self):
        with patch.object(guard.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "bad output\n")):
            with self.assertRaises(RuntimeError):
                guard.process_group_has_live_members(100)

    def test_observation_failure_still_kills_owned_process_and_raises(self):
        # Keep the leader alive through TERM so the injected observation failure
        # exercises escalation against a live owned group on every platform.
        process = subprocess.Popen(
            [sys.executable, "-u", "-c",
             "import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);"
             "print('ready',flush=True);time.sleep(30)"],
            start_new_session=True, stdout=subprocess.PIPE, text=True,
        )
        failure = RuntimeError("observation unavailable")
        try:
            self.assertTrue(select.select([process.stdout], [], [], 5)[0], "child readiness timed out")
            self.assertEqual(process.stdout.readline(), "ready\n")
            with patch.object(guard, "process_group_has_live_members", side_effect=failure):
                with self.assertRaisesRegex(RuntimeError, "observation unavailable"):
                    guard.stop_process_group(process, timeout_seconds=.01)
            self.assertIsNotNone(process.poll())
        finally:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=3)
            process.stdout.close()
