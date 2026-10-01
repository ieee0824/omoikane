"""Failure-path contracts for the isolated GUI harness (requires Xvfb/Openbox)."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from gui_x11 import GuiSession, wait_for


class GuiFailureTests(unittest.TestCase):
    def test_failure_keeps_evidence_and_only_stops_owned_processes(self):
        with tempfile.TemporaryDirectory() as temp:
            unrelated = subprocess.Popen(['sleep', '60'])
            try:
                with self.assertRaisesRegex(RuntimeError, 'scenario failed'):
                    with GuiSession(Path(temp)/'run', {'DISPLAY': ':1'}) as gui:
                        self.assertNotEqual(gui.env['DISPLAY'], ':1')
                        owned = gui.start(['sleep', '60'], 'owned')
                        raise RuntimeError('scenario failed')
                self.assertIsNotNone(owned.poll())
                self.assertIsNone(unrelated.poll())
                self.assertTrue(all(p.poll() is not None for p in gui.processes))
                for name in ('inputs.json', 'results.json', 'failure.original.png', 'failure.png'):
                    self.assertTrue((gui.out/name).is_file(), name)
            finally:
                unrelated.terminate()
                unrelated.wait(timeout=5)

    def test_exited_binary_is_observed_and_cleaned(self):
        with tempfile.TemporaryDirectory() as temp:
            with self.assertRaisesRegex(AssertionError, 'GUI process exited'):
                with GuiSession(Path(temp)/'run') as gui:
                    app = gui.launch('/bin/false', ['unused-url'])
                    app.wait(timeout=5)
                    gui.window(app)
            self.assertTrue(all(p.poll() is not None for p in gui.processes))
            self.assertIn('GUI process exited', (gui.out/'results.json').read_text())

    def test_startup_failure_preserves_logs_and_closes_files(self):
        with tempfile.TemporaryDirectory() as temp:
            gui = GuiSession(Path(temp)/'run', {'PATH': temp})
            with self.assertRaises(FileNotFoundError):
                with gui:
                    self.fail('desktop setup unexpectedly succeeded')
            self.assertTrue((gui.out/'results.json').is_file())
            self.assertTrue((gui.out/'xvfb.log').is_file())
            self.assertTrue(all(log.closed for log in gui.logs))

    def test_concurrent_desktops_do_not_share_or_stop_each_other(self):
        with tempfile.TemporaryDirectory() as temp:
            with GuiSession(Path(temp)/'first') as first:
                with GuiSession(Path(temp)/'second') as second:
                    self.assertNotEqual(first.env['DISPLAY'], second.env['DISPLAY'])
                self.assertTrue(all(p.poll() is None for p in first.processes))
                self.assertIn('dimensions:', first.command('xdpyinfo'))

    def test_condition_timeout_reports_last_observation(self):
        with self.assertRaisesRegex(AssertionError, 'still waiting'):
            wait_for('deadline', lambda: 'still waiting', lambda value: False, seconds=.01)

    def test_existing_evidence_directory_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            marker = Path(temp)/'marker'
            marker.write_text('keep')
            with self.assertRaises(FileExistsError):
                GuiSession(temp)
            self.assertEqual(marker.read_text(), 'keep')


if __name__ == '__main__':
    unittest.main()
