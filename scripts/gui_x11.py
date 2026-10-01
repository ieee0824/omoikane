"""Small real-GUI harness: private X11 desktop, owned processes and evidence.

Callers own fixtures and assertions. Coordinates passed to xdotool are physical
client pixels; CSS-to-device conversion belongs to the scenario.
"""

import ctypes as C
import hashlib
import json
import os
from pathlib import Path
import select
import subprocess as S
import time

from PIL import Image, ImageGrab

ROOT = Path(__file__).resolve().parents[1]


def wait_for(label, probe, predicate, seconds=20, evidence=None):
    """Observe a condition until its monotonic deadline, retaining the last value."""
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        last = probe()
        if predicate(last):
            if evidence is not None:
                evidence.append({'step': label, 'result': last})
            print(label, json.dumps(last), flush=True)
            return last
        time.sleep(.1)
    raise AssertionError((label, last))


class GuiSession:
    """Own an isolated desktop and save evidence even when setup or a test fails."""

    def __init__(self, artifacts, environment=None):
        self.out = Path(artifacts).resolve()
        self.out.mkdir(parents=True, exist_ok=False)
        self.env = os.environ.copy()
        self.env.pop('DISPLAY', None)
        self.env.pop('WAYLAND_DISPLAY', None)
        self.env.update(environment or {})
        self.env.pop('DISPLAY', None)
        self.env.pop('WAYLAND_DISPLAY', None)
        self.env.pop('XAUTHORITY', None)
        self.env['WINIT_UNIX_BACKEND'] = 'x11'
        self.env['WINIT_X11_SCALE_FACTOR'] = (environment or {}).get(
            'WINIT_X11_SCALE_FACTOR', '1')
        self.processes = []
        self.logs = []
        self.closed = False
        self.evidence = []
        self.inputs = {'revision': S.check_output(
            ['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
            'environment': dict(environment or {}),
            'x11_environment': {key: self.env[key] for key in (
                'WINIT_UNIX_BACKEND', 'WINIT_X11_SCALE_FACTOR')},
            'commands': []}
        (self.out / 'gui_x11.py').write_bytes(Path(__file__).read_bytes())
        self._write_inputs()

    def _write_inputs(self):
        (self.out / 'inputs.json').write_text(json.dumps(self.inputs, indent=2) + '\n')

    def start(self, command, name, **kwargs):
        """Start and track only a process owned by this session."""
        command = [str(arg) for arg in command]
        self.inputs['commands'].append({'name': name, 'argv': command})
        self._write_inputs()
        log = (self.out / (name + '.log')).open('w')
        self.logs.append(log)
        process = S.Popen(command, env=self.env, stdout=log, stderr=S.STDOUT, **kwargs)
        self.processes.append(process)
        return process

    def command(self, *args):
        """Run a bounded native-input or observation command on this desktop."""
        return S.check_output(args, env=self.env, text=True, timeout=5).strip()

    def wait_for(self, label, probe, predicate, seconds=20):
        return wait_for(label, probe, predicate, seconds, self.evidence)

    def __enter__(self):
        try:
            self._desktop()
            return self
        except BaseException as error:
            self.__exit__(type(error), error, error.__traceback__)
            raise

    def _desktop(self):
        read_fd, write_fd = os.pipe()
        try:
            process = self.start(['Xvfb', '-displayfd', write_fd, '-screen', '0',
                                  '1600x1000x24', '-nolisten', 'tcp'],
                                 'xvfb', pass_fds=(write_fd,))
        except BaseException:
            os.close(read_fd)
            raise
        finally:
            os.close(write_fd)
        try:
            # Xvfb writes the number and the newline separately and exits if
            # the pipe is closed in between, so read through the newline.
            data = b''
            deadline = time.monotonic() + 10
            while not data.endswith(b'\n'):
                remaining = deadline - time.monotonic()
                assert remaining > 0 and select.select([read_fd], [], [], remaining)[0], \
                    'Xvfb startup timeout'
                chunk = os.read(read_fd, 64)
                assert chunk, 'Xvfb closed the display pipe'
                data += chunk
            number = data.decode().strip()
            assert number.isdecimal() and process.poll() is None, 'Xvfb startup failed'
            self.env['DISPLAY'] = ':' + number
        finally:
            os.close(read_fd)
        self.inputs['display'] = self.env['DISPLAY']
        self._write_inputs()
        self.command('xdpyinfo')
        self.start(['openbox'], 'openbox')
        self.wait_for('window manager ready',
                      lambda: self.command('xprop', '-root', '_NET_SUPPORTING_WM_CHECK'),
                      lambda value: 'window id #' in value)
        (self.out / 'display.txt').write_text(self.command('xdpyinfo'))

    def launch(self, binary, arguments=()):
        """Record executable identity and launch it with arbitrary CLI arguments."""
        binary = Path(binary).resolve()
        self.inputs['binary'] = str(binary)
        self.inputs['binary_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
        self.inputs['binary_size'] = binary.stat().st_size
        self._write_inputs()
        return self.start([binary, *arguments], 'browser')

    def window(self, app):
        """Identify the application window by PID, without relying on page titles."""
        def probe():
            assert app.poll() is None, ('GUI process exited', app.returncode)
            result = S.run(['xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)],
                           env=self.env, capture_output=True, text=True, timeout=5)
            assert result.returncode in (0, 1), result.stderr
            return result.stdout.splitlines()
        return self.wait_for('window created', probe, bool)[0]

    def screenshot(self, name):
        """Keep original and losslessly compressed PNGs with equal pixels and size."""
        im = ImageGrab.grab(xdisplay=self.env['DISPLAY']).convert('RGB')
        original = self.out / (name + '.original.png')
        compressed = self.out / (name + '.png')
        im.save(original, compress_level=0)
        im.save(compressed, compress_level=9)
        with Image.open(compressed) as saved:
            assert saved.size == im.size and saved.convert('RGB').tobytes() == im.tobytes()
        self.evidence.append({'image': name, 'size': im.size,
                              'original_bytes': original.stat().st_size,
                              'compressed_bytes': compressed.stat().st_size})
        return im

    def __exit__(self, kind, error, traceback):
        if self.closed:
            return False
        self.closed = True
        if error is not None:
            self.evidence.append({'status': 'FAIL', 'error': str(error)})
            if 'DISPLAY' in self.env:
                try:
                    self.screenshot('failure')
                except Exception as capture_error:
                    self.evidence.append({'capture_error': str(capture_error)})
        try:
            for process in reversed(self.processes):
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except S.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=5)
                self.evidence.append({'pid': process.pid, 'returncode': process.returncode})
        finally:
            for log in self.logs:
                log.close()
            (self.out / 'results.json').write_text(json.dumps(self.evidence, indent=2) + '\n')
        return False


def geometry(session, window):
    """Return physical client bounds, correcting WM decoration translation."""
    bounds = {key: int(value) for key, value in (
        line.split('=', 1) for line in session.command(
            'xdotool', 'getwindowgeometry', '--shell', str(window)).splitlines())}
    x = C.CDLL('libX11.so.6')
    x.XOpenDisplay.argtypes = [C.c_char_p]
    x.XOpenDisplay.restype = C.c_void_p
    x.XDefaultRootWindow.argtypes = [C.c_void_p]
    x.XDefaultRootWindow.restype = C.c_ulong
    x.XTranslateCoordinates.argtypes = [C.c_void_p, C.c_ulong, C.c_ulong,
        C.c_int, C.c_int, C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(C.c_ulong)]
    x.XCloseDisplay.argtypes = [C.c_void_p]
    display = x.XOpenDisplay(session.env['DISPLAY'].encode())
    assert display, 'Cannot open private display'
    try:
        origin_x, origin_y, child = C.c_int(), C.c_int(), C.c_ulong()
        assert x.XTranslateCoordinates(display, int(window), x.XDefaultRootWindow(display),
                                       0, 0, C.byref(origin_x), C.byref(origin_y), C.byref(child))
        bounds.update(X=origin_x.value, Y=origin_y.value)
        return bounds
    finally:
        x.XCloseDisplay(display)
