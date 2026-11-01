"""Exercise font preparation without sudo privileges or external mirrors."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest


SCRIPT = Path(__file__).parents[1] / "install-ci-fonts.py"


class InstallCiFontsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.output = self.root / "setup evidence"
        self.events = self.root / "events.jsonl"
        self.program("sudo", "import os, sys\nassert sys.argv[1] == '-n'\n"
                     "os.execvp(sys.argv[2], sys.argv[2:])\n")
        self.program("apt-get", '''import json, os, pathlib, signal, sys, time
events = pathlib.Path(os.environ['FONT_TEST_EVENTS'])
previous = [json.loads(line) for line in events.read_text().splitlines()] if events.exists() else []
phase = 'update' if 'update' in sys.argv else 'install'
attempt = 1 + sum(event['phase'] == phase for event in previous)
with events.open('a') as log:
    log.write(json.dumps({'phase': phase, 'attempt': attempt, 'args': sys.argv[1:],
                          'pid': os.getpid(), 'frontend': os.environ.get('DEBIAN_FRONTEND')}) + '\\n')
mode = os.environ['FONT_TEST_MODE']
print(f'fake apt {phase} attempt {attempt}', flush=True)
if phase == 'update' and mode in ('hang', 'ignore-term'):
    if mode == 'ignore-term':
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    time.sleep(60)
if ((mode == 'transient' and phase == 'update' and attempt == 1)
    or (mode == 'update-failure' and phase == 'update')
    or (mode == 'install-failure' and phase == 'install')):
    print('mirror failure', flush=True)
    sys.exit(100)
''')

    def program(self, name, body):
        path = self.bin / name
        path.write_text(f"#!{sys.executable}\n" + body)
        path.chmod(0o755)

    def run_installer(self, mode, timeout="2", path=None):
        environment = dict(os.environ, FONT_TEST_MODE=mode, FONT_TEST_EVENTS=str(self.events),
                           PATH=path or str(self.bin) + os.pathsep + os.environ["PATH"])
        run = subprocess.run([sys.executable, str(SCRIPT), str(self.output),
                              "--timeout-seconds", timeout], env=environment,
                             text=True, capture_output=True, timeout=20)
        result_path = self.output / "result.json"
        result = json.loads(result_path.read_text()) if result_path.exists() else None
        return run, result

    def read_events(self):
        return [json.loads(line) for line in self.events.read_text().splitlines()]

    def test_success_installs_original_font_set_with_bounded_network_options(self):
        run, result = self.run_installer("success")
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertEqual(result["status"], "success")
        events = self.read_events()
        self.assertEqual([event["phase"] for event in events], ["update", "install"])
        for event in events:
            for option in ("Acquire::Retries=1", "Acquire::http::Timeout=20",
                           "Acquire::https::Timeout=20", "DPkg::Lock::Timeout=30"):
                self.assertIn(option, event["args"])
            self.assertEqual(event["frontend"], "noninteractive")
        self.assertIn("APT::Update::Error-Mode=any", events[0]["args"])
        self.assertEqual(events[1]["args"][-4:],
                         ["fonts-dejavu", "fonts-liberation", "fonts-noto-core", "fonts-noto-cjk"])
        self.assertTrue((self.output / "update-1.log").is_file())
        self.assertTrue((self.output / "install-1.log").is_file())

    def test_transient_failure_retries_and_keeps_both_attempt_logs(self):
        run, result = self.run_installer("transient")
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertEqual(result["status"], "success")
        self.assertEqual([attempt["exit_code"] for attempt in result["attempts"]], [100, 0, 0])
        self.assertIn("mirror failure", (self.output / "update-1.log").read_text())
        self.assertIn("attempt 2", (self.output / "update-2.log").read_text())

    def test_permanent_update_failure_never_starts_install_or_reports_success(self):
        run, result = self.run_installer("update-failure")
        self.assertEqual(run.returncode, 1)
        self.assertEqual(result["status"], "infrastructure_failure")
        self.assertEqual(result["failed_phase"], "update")
        self.assertEqual([event["phase"] for event in self.read_events()], ["update", "update"])
        self.assertIn("Rust tests were not started", run.stdout)

    def test_install_failure_retains_successful_update_evidence(self):
        run, result = self.run_installer("install-failure")
        self.assertEqual(run.returncode, 1)
        self.assertEqual(result["status"], "infrastructure_failure")
        self.assertEqual(result["failed_phase"], "install")
        self.assertEqual([attempt["exit_code"] for attempt in result["attempts"]], [0, 100, 100])
        self.assertTrue((self.output / "update-1.log").is_file())

    def test_unresponsive_mirror_is_terminated_and_retries_are_finite(self):
        run, result = self.run_installer("hang", timeout="0.5")
        self.assertEqual(run.returncode, 1)
        self.assertEqual(result["status"], "infrastructure_failure")
        self.assertEqual([attempt["exit_code"] for attempt in result["attempts"]], [124, 124])
        self.assertEqual([attempt["outcome"] for attempt in result["attempts"]], ["timeout"] * 2)
        for event in self.read_events():
            with self.assertRaises(ProcessLookupError):
                os.kill(event["pid"], 0)

    def test_term_ignoring_mirror_is_forcibly_killed(self):
        started = time.monotonic()
        run, result = self.run_installer("ignore-term", timeout="0.5")
        self.assertEqual(run.returncode, 1)
        self.assertEqual(result["status"], "infrastructure_failure")
        self.assertEqual([attempt["exit_code"] for attempt in result["attempts"]], [137, 137])
        self.assertLess(time.monotonic() - started, 18)

    def test_missing_launcher_is_infrastructure_failure_without_retry(self):
        empty = self.root / "empty"
        empty.mkdir()
        run, result = self.run_installer("success", path=str(empty))
        self.assertEqual(run.returncode, 1)
        self.assertEqual(result["status"], "infrastructure_failure")
        self.assertEqual([attempt["exit_code"] for attempt in result["attempts"]], [127])

    def test_invalid_deadlines_are_rejected_before_any_apt_execution(self):
        for timeout in ("0", "nan", "inf", "121"):
            with self.subTest(timeout=timeout):
                run, result = self.run_installer("success", timeout=timeout)
                self.assertEqual(run.returncode, 2)
                self.assertIsNone(result)
                self.assertFalse(self.events.exists())


if __name__ == "__main__":
    unittest.main()
