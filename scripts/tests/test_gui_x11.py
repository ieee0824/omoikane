"""Failure-path contracts for the isolated GUI harness (requires Xvfb/Openbox)."""
from pathlib import Path
import json
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from gui_x11 import GuiSession, wait_for, trace_records

# A minimal client that maps a top-level window as soon as it connects.
MAPPING_CLIENT = '''
import ctypes as C, os, time
x = C.CDLL('libX11.so.6')
x.XOpenDisplay.restype = C.c_void_p
x.XOpenDisplay.argtypes = [C.c_char_p]
x.XDefaultRootWindow.restype = C.c_ulong
x.XDefaultRootWindow.argtypes = [C.c_void_p]
x.XCreateSimpleWindow.restype = C.c_ulong
x.XCreateSimpleWindow.argtypes = [C.c_void_p, C.c_ulong, C.c_int, C.c_int, C.c_uint,
                                  C.c_uint, C.c_uint, C.c_ulong, C.c_ulong]
x.XInternAtom.restype = C.c_ulong
x.XInternAtom.argtypes = [C.c_void_p, C.c_char_p, C.c_int]
x.XChangeProperty.argtypes = [C.c_void_p, C.c_ulong, C.c_ulong, C.c_ulong, C.c_int,
                              C.c_int, C.c_void_p, C.c_int]
x.XMapWindow.argtypes = [C.c_void_p, C.c_ulong]
x.XFlush.argtypes = [C.c_void_p]
display = x.XOpenDisplay(None)
assert display
window = x.XCreateSimpleWindow(display, x.XDefaultRootWindow(display),
                               0, 0, 640, 400, 0, 0, 0xffffff)
pid = C.c_ulong(os.getpid())
x.XChangeProperty(display, window, x.XInternAtom(display, b'_NET_WM_PID', 0),
                  6, 32, 0, C.byref(pid), 1)
x.XMapWindow(display, window)
x.XFlush(display)
time.sleep(60)
'''


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
                # Threads of processes still running at the failure are kept.
                evidence = json.loads((gui.out/'results.json').read_text())
                snapshot = next(row['threads'] for row in evidence
                                if row.get('pid') == owned.pid and 'threads' in row)
                self.assertEqual(snapshot[0]['name'], 'sleep')
                self.assertIn(snapshot[0]['state'], 'SRD')
                focus = next(row['focus'] for row in evidence if 'focus' in row)
                self.assertIn('_NET_ACTIVE_WINDOW', focus['active'])
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

    def test_window_mapped_right_after_cold_desktop_startup_is_shown(self):
        # An empty cache makes Openbox build its fontconfig cache during
        # startup, after it already advertises _NET_SUPPORTING_WM_CHECK.
        with tempfile.TemporaryDirectory() as temp:
            environment = {'XDG_CACHE_HOME': str(Path(temp)/'cache')}
            with GuiSession(Path(temp)/'run', environment) as gui:
                client = gui.start([sys.executable, '-c', MAPPING_CLIENT], 'client')
                gui.window(client)

    def test_display_survives_its_last_client_disconnecting(self):
        # Without -noreset Xvfb regenerates once no client is connected,
        # dropping root properties and refusing clients that connect then.
        with tempfile.TemporaryDirectory() as temp:
            with GuiSession(Path(temp)/'run') as gui:
                openbox = gui.processes[1]
                openbox.terminate()
                openbox.wait(timeout=5)
                gui.command('xprop', '-root', '-f', '_OMOIKANE_RESET_PROBE', '8s',
                            '-set', '_OMOIKANE_RESET_PROBE', 'kept')
                self.assertIn('"kept"', gui.command('xprop', '-root', '_OMOIKANE_RESET_PROBE'))

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


class TraceRecordTests(unittest.TestCase):
    def test_every_partial_write_waits_for_newline(self):
        line = 'OMOIKANE_PAINT {"sequence":1,"viewport":[640,400]}\n'
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'browser.log'
            for length in range(len(line)):
                path.write_text(line[:length])
                self.assertEqual(trace_records(path, 'PAINT'), [], length)
            path.write_text(line)
            self.assertEqual(trace_records(path, 'PAINT'),
                             [{'sequence': 1, 'viewport': [640, 400]}])

    def test_prior_complete_records_survive_partial_next_record(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'browser.log'
            path.write_text('unrelated\nOMOIKANE_WHEEL {"success":true}\n'
                            'OMOIKANE_PAINT {"sequence":1}\nOMOIKANE_PAINT {')
            self.assertEqual(trace_records(path, 'PAINT'), [{'sequence': 1}])
            self.assertEqual(trace_records(path, 'WHEEL'), [{'success': True}])

    def test_malformed_completed_record_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'browser.log'
            path.write_text('OMOIKANE_PAINT {"sequence"}\n')
            with self.assertRaises(json.JSONDecodeError):
                trace_records(path, 'PAINT')
