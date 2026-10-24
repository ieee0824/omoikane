#!/usr/bin/env python3
"""Measure unchanged-input surface transfers and protect chrome/resize updates."""
import argparse
import hashlib
import json
from pathlib import Path
import time
from urllib.parse import quote

from gui_x11 import GuiSession, geometry, trace_records


FIXTURE = ('<!doctype html><title>Surface fixture</title>'
           '<style>body{margin:0}div{height:8000px;background:#dbe8f1}</style><div></div>')


def records(gui, kind):
    return trace_records(gui.out / 'browser.log', kind)


def digest(image, rectangle):
    return hashlib.sha256(image.crop(rectangle).convert('RGBA').tobytes()).hexdigest()


def regions(gui, window):
    bounds = geometry(gui, window)
    paint = records(gui, 'PAINT')[-1]
    assert paint['scale'] == 1.0, paint
    width, height = paint['viewport']
    left, top = bounds['X'], bounds['Y'] + bounds['HEIGHT'] - height
    return ((left, top, left + width, top + height),
            (left, bounds['Y'], left + bounds['WIDTH'], top))


def unchanged_input(gui, window, expect_idle):
    gui.command('xdotool', 'mousemove', '--window', window, '120', '150')
    time.sleep(1)
    before_paints = len(records(gui, 'PAINT'))
    before_surfaces = len(records(gui, 'SURFACE_PRESENT'))
    before_inputs = len(records(gui, 'PAGE_INPUT'))
    page, toolbar = regions(gui, window)
    before = gui.screenshot('before-input')
    page_hash, toolbar_hash = digest(before, page), digest(before, toolbar)
    started = time.monotonic()
    for index in range(20):
        previous = len(records(gui, 'PAGE_INPUT'))
        gui.command('xdotool', 'mousemove', '--window', window,
                    str(140 + index * 5), str(150 + (index % 3) * 10))
        gui.wait_for('cursor delivered',
                     lambda: records(gui, 'PAGE_INPUT')[previous:],
                     lambda rows: any(row['kind'] == 'cursor' and row['success'] for row in rows))
    time.sleep(.5)
    transfers = records(gui, 'SURFACE_PRESENT')[before_surfaces:]
    inputs = records(gui, 'PAGE_INPUT')[before_inputs:]
    after = gui.screenshot('after-input')
    assert digest(after, page) == page_hash, 'unchanged input changed page pixels'
    assert len(records(gui, 'PAINT')) == before_paints, 'unchanged input rasterized the page'
    cursors = [row for row in inputs if row['kind'] == 'cursor']
    assert len(cursors) >= 20, len(cursors)
    for row in transfers:
        assert row['present_ms'] >= 0 and row['scale'] == 1.0, row
        assert not row['page_paint'], row
    if expect_idle:
        assert not transfers, ('unchanged input still transfers surfaces', len(transfers))
    return {
        'moves': 20, 'cursor_deliveries': len(cursors),
        'surface_transfers': len(transfers),
        'surface_ms': sum(row['present_ms'] for row in transfers),
        'duration_s': time.monotonic() - started,
        'page_rgba_sha256': page_hash,
    }, (page, toolbar, page_hash, toolbar_hash)


def chrome_and_resize(gui, window, original):
    page, toolbar, page_hash, toolbar_hash = original
    previous = len(records(gui, 'SURFACE_PRESENT'))
    gui.command('xdotool', 'key', 'ctrl+l')
    gui.command('xdotool', 'type', '--clearmodifiers', '--delay', '20', 'surface-edit')
    gui.wait_for('URL edit presented',
                 lambda: records(gui, 'SURFACE_PRESENT')[previous:], bool)
    time.sleep(.2)
    edited = gui.screenshot('url-edit')
    assert digest(edited, toolbar) != toolbar_hash, 'URL editing failed to update chrome'
    assert digest(edited, page) == page_hash, 'URL editing changed page pixels'
    gui.command('xdotool', 'key', 'Escape')
    previous = len(records(gui, 'SURFACE_PRESENT'))
    gui.command('xdotool', 'windowsize', window, '900', '700')
    gui.wait_for('resized viewport painted', lambda: records(gui, 'PAINT'),
                 lambda rows: rows[-1]['viewport'][0] == 900)
    gui.wait_for('resize presented',
                 lambda: records(gui, 'SURFACE_PRESENT')[previous:], bool)


def run(binary, artifacts, expect_idle):
    environment = {'WINIT_X11_SCALE_FACTOR': '1', 'OMOIKANE_TRACE_PAINT': '1',
                   'OMOIKANE_TRACE_INPUT': '1', 'OMOIKANE_TRACE_STARTUP': '1'}
    with GuiSession(artifacts, environment) as gui:
        gui.inputs['surface_fixture'] = FIXTURE
        gui._write_inputs()
        (gui.out / Path(__file__).name).write_bytes(Path(__file__).read_bytes())
        app = gui.launch(binary, ['data:text/html,' + quote(FIXTURE)])
        window = gui.window(app)
        gui.command('xdotool', 'windowactivate', '--sync', window)
        gui.wait_for('fixture painted', lambda: records(gui, 'PAINT'), bool)
        gui.wait_for('surface trace supported', lambda: records(gui, 'SURFACE_PRESENT'), bool)
        gui.wait_for('toolbar font ready', lambda: records(gui, 'STARTUP'),
                     lambda rows: any(row['phase'] == 'toolbar_font_ready' for row in rows))
        report, original = unchanged_input(gui, window, expect_idle)
        chrome_and_resize(gui, window, original)
        report.update({'status': 'PASS', 'build': records(gui, 'BUILD'),
                       'external_requests': 0, 'fixture': 'data-url',
                       'url_edit_presented': True, 'resize_presented': True})
        (gui.out / 'surface-summary.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report), flush=True)
        gui.command('xdotool', 'key', 'alt+F4')
        gui.wait_for('close', app.poll, lambda status: status is not None, seconds=5)
        assert app.returncode == 0, app.returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--artifacts', required=True, type=Path)
    parser.add_argument('--expect-idle', action='store_true',
                        help='Require unchanged input to produce no surface transfers.')
    args = parser.parse_args()
    run(args.binary, args.artifacts, args.expect_idle)


if __name__ == '__main__':
    main()
