#!/usr/bin/env python3
"""Measure GUI idle CPU and verify demand-driven rendering on private X11."""
import argparse
import json
import os
from pathlib import Path
import time
from PIL import ImageGrab
from gui_navigation_fixture import NavigationFixture
from gui_x11 import GuiSession, geometry

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / 'tests/fixtures/gui-render-demand'


def cpu_seconds(pid):
    fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK')


def paints(gui):
    """Require the paint trace, so an uninstrumented binary cannot pass idle."""
    records = [json.loads(line.removeprefix('OMOIKANE_PAINT '))
               for line in (gui.out / 'browser.log').read_text().splitlines()
               if line.startswith('OMOIKANE_PAINT ')]
    assert records, 'No paint trace: run a build with OMOIKANE_TRACE_PAINT support'
    assert [r['sequence'] for r in records] == list(range(1, len(records) + 1))
    return len(records)


def pixel(gui, bounds):
    image = ImageGrab.grab(xdisplay=gui.env['DISPLAY']).convert('RGB')
    return image.getpixel((bounds['X'] + 700, bounds['Y'] + 400))


def idle(gui, label, seconds=2):
    time.sleep(.3)
    before = paints(gui)
    time.sleep(seconds)
    after = paints(gui)
    assert before == after, (label, 'unexpected idle paints', after - before)
    gui.evidence.append({'step': label, 'seconds': seconds, 'paints': after - before})


def navigate(gui, window, fixture, path, bounds, expected=None):
    expected = tuple(expected or fixture.expectations["pages"][path]["initial_rgb"])
    gui.command('xdotool', 'key', '--window', window, 'ctrl+l')
    gui.command('xdotool', 'type', '--window', window, '--delay', '1', fixture.url(path))
    gui.command('xdotool', 'key', '--window', window, 'Return')
    fixture.wait_for_request(path)
    gui.command('xdotool', 'mousemove', '--window', window, '700', '400')
    gui.wait_for(path + ' painted', lambda: pixel(gui, bounds), lambda c: c == expected)


def click_page(gui):
    gui.command('xdotool', 'click', '1')


def verify_timer(gui, window, fixture, bounds, delay_ms=500):
    navigate(gui, window, fixture, '/timer.html', bounds)
    idle(gui, 'timer page before input')
    # timer.html selects 1000ms with event.shiftKey, otherwise 500ms.
    if delay_ms == 1000:
        gui.command('xdotool', 'keydown', '--window', window, 'Shift_L')
    started = time.monotonic()
    click_page(gui)
    if delay_ms == 1000:
        gui.command('xdotool', 'keyup', '--window', window, 'Shift_L')
    time.sleep(.15)
    assert pixel(gui, bounds) == tuple(fixture.expectations['pages']['/timer.html']['initial_rgb']), 'timer fired too early after idle'
    gui.wait_for(f'{delay_ms}ms timer painted', lambda: pixel(gui, bounds), lambda c: c == (0, 0, 255))
    elapsed = time.monotonic() - started
    delay = delay_ms / 1000
    assert delay - .05 <= elapsed <= delay + 1.3, ('timer real deadline', delay_ms, elapsed)
    gui.evidence.append({'step': 'timer deadline', 'delay_ms': delay_ms, 'seconds_from_input': elapsed})
    idle(gui, 'timer finished')


def verify_motion(gui, window, fixture, bounds, path, activate=True, minimum_colors=2):
    navigate(gui, window, fixture, path, bounds)
    if activate:
        click_page(gui)
    before = paints(gui)
    colors = []
    for _ in range(12):
        colors.append(pixel(gui, bounds))
        time.sleep(.08)
    count = paints(gui) - before
    assert len(set(colors)) >= minimum_colors, (path, colors)
    assert count >= 3, (path, 'animation stopped painting', count)
    gui.evidence.append({'step': path, 'colors': colors, 'paints': count})
    gui.screenshot(Path(path).stem + '-moving')
    gui.command('xdotool', 'key', '--window', window, 'Escape')
    idle(gui, path + ' cancelled')


def verify_finite_motion(gui, window, fixture, bounds, path, duration):
    navigate(gui, window, fixture, path, bounds)
    started = time.monotonic()
    click_page(gui)
    time.sleep(.15)
    middle = pixel(gui, bounds)
    if path == '/transition.html':
        initial = tuple(fixture.expectations['pages'][path]['initial_rgb'])
        middle = gui.wait_for('transition intermediate color', lambda: pixel(gui, bounds),
                              lambda color: color not in [initial, (0, 0, 255)], seconds=.5)
    gui.wait_for(path + ' ended', lambda: pixel(gui, bounds), lambda c: c == (0, 0, 255))
    elapsed = time.monotonic() - started
    assert elapsed <= duration + 1.0, (path, elapsed)
    if path == '/transition.html':
        assert middle not in [initial, (0, 0, 255)], ('transition midpoint', middle)
        assert elapsed >= .9, ('transition ended early', elapsed)
    gui.evidence.append({'step': path, 'midpoint': middle, 'seconds': elapsed})
    idle(gui, path + ' finished')


def verify(binary, artifacts):
    with NavigationFixture(FIXTURE) as fixture:
        with GuiSession(artifacts, {'WINIT_X11_SCALE_FACTOR': '1', 'OMOIKANE_TRACE_PAINT': '1'}) as gui:
            (gui.out / 'harness.py').write_bytes(Path(__file__).read_bytes())
            app = gui.launch(binary, [fixture.url('/')])
            window = gui.window(app)
            gui.command('xdotool', 'windowsize', '--sync', window, '1000', '700')
            gui.command('xdotool', 'windowmove', '--sync', window, '50', '50')
            gui.command('xdotool', 'windowactivate', '--sync', window)
            bounds = geometry(gui, window)
            gui.wait_for('static page painted', lambda: pixel(gui, bounds), lambda c: c == (10, 20, 30))
            idle(gui, 'static page', 3)
            # Once hover has settled in chrome, editing only the toolbar must
            # compose retained page pixels without another page paint.
            gui.command('xdotool', 'mousemove', '--window', window, '300', '20')
            time.sleep(.4)
            before = paints(gui)
            gui.command('xdotool', 'click', '1')
            gui.command('xdotool', 'key', '--window', window, 'ctrl+a')
            gui.command('xdotool', 'type', '--window', window, '--delay', '10', 'toolbar-only')
            time.sleep(.3)
            assert paints(gui) == before, 'toolbar editing repainted the page'
            gui.screenshot('toolbar-only')
            gui.command('xdotool', 'key', '--window', window, 'Escape')
            verify_timer(gui, window, fixture, bounds)
            verify_timer(gui, window, fixture, bounds, delay_ms=1000)
            verify_motion(gui, window, fixture, bounds, '/raf.html')
            verify_finite_motion(gui, window, fixture, bounds, '/transition.html', 1)
            verify_motion(gui, window, fixture, bounds, '/keyframes.html', minimum_colors=3)
            verify_motion(gui, window, fixture, bounds, '/gif.html', activate=False)
            verify_finite_motion(gui, window, fixture, bounds, '/smooth.html', .3)
            assert app.poll() is None, ('GUI exited', app.returncode)
            (gui.out / 'outcome.json').write_text(json.dumps({'outcome':'PASS'}, indent=2) + '\n')


def measure_idle(binary, artifacts, seconds):
    with NavigationFixture(FIXTURE) as fixture:
        with GuiSession(artifacts, {'WINIT_X11_SCALE_FACTOR': '1'}) as gui:
            app = gui.launch(binary, [fixture.url('/')])
            window = gui.window(app)
            gui.command('xdotool', 'windowsize', '--sync', window, '1000', '700')
            gui.command('xdotool', 'windowmove', '--sync', window, '50', '50')
            bounds = geometry(gui, window)
            gui.wait_for('static page painted', lambda: pixel(gui, bounds),
                         lambda value: value == (10, 20, 30))
            time.sleep(2)
            before_cpu, before_time = cpu_seconds(app.pid), time.monotonic()
            time.sleep(seconds)
            elapsed = time.monotonic() - before_time
            assert app.poll() is None, ("GUI exited during CPU measurement", app.returncode)
            assert pixel(gui, bounds) == (10, 20, 30), "static page disappeared during measurement"
            cpu = cpu_seconds(app.pid) - before_cpu
            result = {'pid': app.pid, 'interval_seconds': elapsed,
                      'cpu_seconds': cpu, 'cpu_percent_one_core': cpu / elapsed * 100}
            (gui.out / 'cpu.json').write_text(json.dumps(result, indent=2) + '\n')
            print(json.dumps(result), flush=True)
            return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--artifacts', required=True, type=Path)
    parser.add_argument('--seconds', type=float, default=10)
    parser.add_argument('--verify', action='store_true', help='verify paint counts and real rendering deadlines')
    args = parser.parse_args()
    if args.verify:
        verify(args.binary, args.artifacts)
    else:
        measure_idle(args.binary, args.artifacts, args.seconds)


if __name__ == '__main__':
    main()
