"""Fail closed when process-group cleanup cannot be verified."""
import importlib.util
import subprocess
import os
import signal
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
        process = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(30)"], start_new_session=True)
        failure = RuntimeError("observation unavailable")
        try:
            with patch.object(guard, "process_group_has_live_members", side_effect=failure):
                with self.assertRaisesRegex(RuntimeError, "observation unavailable"):
                    guard.stop_process_group(process, timeout_seconds=.01)
            self.assertIsNotNone(process.poll())
        finally:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=3)
