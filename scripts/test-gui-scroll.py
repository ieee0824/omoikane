#!/usr/bin/env python3
"""Measure real native wheel/render/presentation stages on an owned X11 desktop."""
import argparse
import json
from pathlib import Path
import re
import statistics
import time

from PIL import ImageGrab
from gui_navigation_fixture import NavigationFixture
from gui_x11 import GuiSession, geometry

ROOT = Path(__file__).resolve().parents[1]


def records(gui, kind):
    prefix = 'OMOIKANE_' + kind + ' '
    return [json.loads(line[len(prefix):]) for line in
            (gui.out / 'browser.log').read_text().splitlines() if line.startswith(prefix)]


def status(gui, window):
    title = gui.command('xdotool', 'getwindowname', window)
    match = re.search(r'scroll:(\d+) wheel:(\d+) click:([^ ]+) events:(\d+)', title)
    return (int(match[1]), int(match[2]), match[3], int(match[4])) if match else None


def last_frame(gui):
    frames = records(gui, 'PAINT')
    return frames[-1] if frames else None


def verify(binary, artifacts, scale, iterations, baseline):
    environment = {'WINIT_X11_SCALE_FACTOR': str(scale), 'OMOIKANE_TRACE_PAINT': '1'}
    with NavigationFixture(ROOT / 'tests/fixtures/gui-scroll') as fixture:
        with GuiSession(artifacts, environment) as gui:
            app = gui.launch(binary, [fixture.url('/long.html')])
            window = gui.window(app)
            gui.command('xdotool', 'windowactivate', '--sync', window)
            fixture.wait_for_request('/long.html')
            gui.command('xdotool', 'windowsize', window, str(640 * scale), str(440 * scale))
            gui.wait_for('initial page', lambda: status(gui, window), lambda s: s == (0, 0, 'none', 0))
            gui.wait_for('initial frame', lambda: last_frame(gui),
                         lambda frame: frame and frame['viewport'] == [640, 400])
            bounds = geometry(gui, window)
            gui.command('xdotool', 'mousemove', '--window', window, str(500 * scale), str(240 * scale))
            time.sleep(.3)
            samples = []
            for index in range(iterations + 2):
                before = len(records(gui, 'PAINT'))
                started = time.monotonic()
                # This Winit X11 backend translates both XTest wheel-button
                # edges. Alternate edges to measure one native wheel event.
                edge = 'mousedown' if index % 2 == 0 else 'mouseup'
                gui.command('xdotool', edge, '4')
                expected = (40 * (index + 1), index + 1)
                gui.wait_for('wheel and scroll delivered', lambda: status(gui, window),
                             lambda s: s and s[:2] == expected and s[3] == index + 1)
                frame = gui.wait_for('scrolled frame presented', lambda: last_frame(gui),
                                    lambda row: row and row['sequence'] > before)
                wheel = records(gui, 'WHEEL')[-1]
                assert wheel['success']
                if index >= 2:
                    samples.append({**frame, **wheel,
                                    'observed_ms': (time.monotonic() - started) * 1000})
            # Native events arrive faster than frames. Preserve every wheel's
            # delivery and accumulated default scroll while drawing progress.
            singles = iterations + 2
            if singles % 2:
                gui.command('xdotool', 'mouseup', '4')
                singles += 1
            before_burst = len(records(gui, 'PAINT'))
            gui.command('xdotool', 'click', '--repeat', '10', '--delay', '20', '4')
            expected = (40 * (singles + 20), singles + 20)
            gui.wait_for('continuous wheels consumed', lambda: status(gui, window),
                         lambda s: s and s[:2] == expected, seconds=60)
            gui.wait_for('burst frame presented', lambda: last_frame(gui),
                         lambda frame: frame and frame['sequence'] > before_burst)
            burst_frames = len(records(gui, 'PAINT')) - before_burst
            gui.command('xdotool', 'click', '1')
            expected_row = (expected[0] + 200) // 80
            gui.wait_for('post-scroll hit test', lambda: status(gui, window),
                         lambda s: s and s[2] == f'row-{expected_row}')
            image = ImageGrab.grab(xdisplay=gui.env['DISPLAY']).convert('RGB')
            color = image.getpixel((bounds['X'] + 500 * scale, bounds['Y'] + 240 * scale))
            expected_color = (192, 128, 64) if expected_row % 2 else (64, 192, 128)
            assert color == expected_color, (color, expected_color)
            gui.screenshot('scrolled')
            report = {
                'scale': scale, 'iterations': iterations, 'samples': samples,
                'page_bounds': [bounds['X'], bounds['Y'] + 40 * scale, 640 * scale, 400 * scale],
                'median_ms': {key: statistics.median(s[key] for s in samples) for key in
                              ('dispatch_ms', 'adjusted_layout_ms', 'paint_ms', 'frame_ms', 'present_ms')},
                'continuous_status': expected, 'hit_row': expected_row,
                'burst_frames': burst_frames,
            }
            (gui.out / 'benchmark.json').write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(report['median_ms']), flush=True)
            assert baseline or burst_frames > 1, 'No rendering progress during continuous input'


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--artifacts', required=True)
    parser.add_argument('--scale', type=int, choices=(1, 2), default=1)
    parser.add_argument('--iterations', type=int, default=10)
    parser.add_argument('--baseline', action='store_true',
                        help='Record the old stalled burst without claiming the progress check passes')
    options = parser.parse_args()
    assert options.iterations > 0
    verify(options.binary, options.artifacts, options.scale, options.iterations, options.baseline)
