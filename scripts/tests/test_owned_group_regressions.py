"""Real owned-process regressions, with portable ps live-member observation."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('owned_space', Path(__file__).parents[1] / 'gate5-space.py')
space = importlib.util.module_from_spec(spec)
spec.loader.exec_module(space)

class OwnedGroupRegressions(unittest.TestCase):
    def exercise_group(self, cancel_after_exit):
        original = subprocess.Popen
        handles = []
        triggered = False
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / 'member.pid'
            member = ('import signal,os,time;from pathlib import Path;'
                      'signal.signal(signal.SIGTERM,signal.SIG_IGN);'
                      f'Path({str(marker)!r}).write_text(str(os.getpid()));time.sleep(30)')
            parent = ('import signal,subprocess,sys,time;'
                      'signal.signal(signal.SIGUSR1,lambda *_:sys.exit(0));'
                      f'subprocess.Popen([sys.executable,"-c",{member!r}]);time.sleep(30)')
            command = [sys.executable, '-c', parent]
            deadline = time.monotonic() + 10
            def start(argv, **kwargs):
                process = original(argv, **kwargs)
                if argv == command:
                    handles.append(process)
                return process
            def usage(_):
                nonlocal triggered
                if time.monotonic() > deadline:
                    raise RuntimeError('fixture readiness deadline exceeded')
                if marker.exists() and not triggered:
                    triggered = True
                    self.assertEqual(os.getpgid(int(marker.read_text())), handles[0].pid)
                    if cancel_after_exit:
                        os.kill(handles[0].pid, signal.SIGUSR1)
                        self.assertEqual(handles[0].wait(timeout=3), 0)
                        os.kill(os.getpid(), signal.SIGTERM)
                return (1000, 999, 1) if triggered else (1000, 0, 1000)
            try:
                with patch.object(space.subprocess, 'Popen', side_effect=start):
                    code = space.run_monitored(command, root / 'evidence', root,
                                               dict(os.environ), 100, .01, usage)
                self.assertTrue(triggered)
                self.assertEqual(code, 143 if cancel_after_exit else 75)
                module = sys.modules['gate5_cargo_space_guard']
                self.assertFalse(module.process_group_has_live_members(handles[0].pid))
                report = json.loads((root / 'evidence/capacity-result.json').read_text())
                self.assertEqual(report['cleanup'], 'verified_no_live_members')
                self.assertNotIn('cleanup_error', report)
            finally:
                for process in handles:
                    self.assertNotEqual(process.pid, os.getpgrp())
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    process.wait(timeout=3)

    def test_capacity_stops_stubborn_member_after_leader_exits(self):
        self.exercise_group(False)

    def test_cancel_stops_member_after_leader_already_exited(self):
        self.exercise_group(True)
