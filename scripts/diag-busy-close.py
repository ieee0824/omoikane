#!/usr/bin/env python3
"""TEMPORARY (#1532): repeat the busy-page close check and capture thread state on failure."""
import importlib.util, json, subprocess as S, sys, time, traceback
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
import gui_x11

spec = importlib.util.spec_from_file_location('page_worker', Path(__file__).with_name('test-gui-page-worker.py'))
page_worker = importlib.util.module_from_spec(spec); spec.loader.exec_module(page_worker)

def dump(gui, label):
    app = gui.processes[-1]
    out = gui.out / ('threads-' + label.replace(' ', '-') + '.txt')
    lines = []
    for task in sorted(Path(f'/proc/{app.pid}/task').glob('*')):
        try:
            lines.append('%s %s %s wchan=%s' % (task.name, (task/'comm').read_text().strip(),
                         (task/'stat').read_text().split(') ')[1].split()[0], (task/'wchan').read_text()))
        except OSError as error:
            lines.append(f'{task.name} {error}')
    for args in (['xprop', '-root', '_NET_ACTIVE_WINDOW'], ['xdotool', 'getwindowfocus']):
        r = S.run(args, env=gui.env, capture_output=True, text=True, timeout=5)
        lines.append(' '.join(args) + ' -> ' + r.stdout.strip() + r.stderr.strip())
    r = S.run(['sudo', '-n', 'gdb', '-p', str(app.pid), '-batch', '-ex', 'thread apply all bt 40'],
              capture_output=True, text=True, timeout=120)
    lines.append(r.stdout + r.stderr)
    out.write_text('\n'.join(lines))

orig_wait, orig_command = gui_x11.GuiSession.wait_for, gui_x11.GuiSession.command
def wait_for(self, label, probe, predicate, seconds=20):
    t = time.monotonic()
    try:
        return orig_wait(self, label, probe, predicate, seconds)
    except AssertionError:
        dump(self, label)
        raise
    finally:
        self.evidence.append({'diag_wait': label, 'ms': round((time.monotonic() - t) * 1000)})
def command(self, *args):
    t = time.monotonic()
    try:
        return orig_command(self, *args)
    finally:
        self.evidence.append({'diag_command': list(args)[:3], 'ms': round((time.monotonic() - t) * 1000)})
gui_x11.GuiSession.wait_for, gui_x11.GuiSession.command = wait_for, command
page_worker.GuiSession = gui_x11.GuiSession

binary, artifacts, count = sys.argv[1], Path(sys.argv[2]), int(sys.argv[3])
failures = []
for i in range(count):
    try:
        page_worker.verify(binary, artifacts / f'busy-{i}')
    except Exception as error:
        failures.append({'run': i, 'error': repr(error)})
        traceback.print_exc()
(artifacts / 'diag-summary.json').write_text(json.dumps({'runs': count, 'failures': failures}, indent=2))
print('DIAG busy close failures', len(failures), '/', count, flush=True)
