#!/usr/bin/env python3
"""Acceptance E2E for #1131: type a URL into the native GUI and see that page.

The browser starts on the fixture's start page. Native X11 input clicks the
visible URL field, replaces its text with the destination URL and presses
Enter. The scenario passes only when the fixture server received the
destination request *and* the page area shows the destination's colour; a
title, a "loaded" message or a request alone is not enough.

Outcomes are written to ``outcome.json`` and returned as the exit status:

==============  ====  ===================================================
PASS            0     URL entry navigated and painted the destination.
REGRESSION      1     Declared implemented, but the scenario failed.
NOT_IMPLEMENTED 2     Declared unimplemented, and the scenario failed.
ENV_ERROR       3     Desktop, browser launch or start page never ready.
UNKNOWN         4     Declared unimplemented, but the scenario passed.
==============  ====  ===================================================

Whether URL entry exists is a declaration (``URL_ENTRY_UI`` or
``--url-entry``), never guessed from screenshots or the request log.
"""

import argparse
import json
from pathlib import Path
import sys

from PIL import ImageGrab

from gui_navigation_fixture import NavigationFixture
from gui_x11 import GuiSession, geometry

# Expected state of the build under test. #1134 recorded the baseline
# (eec3e01c) as "unimplemented"; #1139 implemented the URL field.
URL_ENTRY_UI = 'implemented'

EXIT = {'PASS': 0, 'REGRESSION': 1, 'NOT_IMPLEMENTED': 2, 'ENV_ERROR': 3, 'UNKNOWN': 4}
# The URL field's vertical centre in logical pixels (TOOLBAR_HEIGHT 40 in
# src/bin/omoikane/chrome_layout.rs). Only used to aim the click.
URL_FIELD_CENTER_Y = 20
WINDOW = (1000, 700)
MATCH_RATIO = 0.99


def grab(gui):
    return ImageGrab.grab(xdisplay=gui.env['DISPLAY']).convert('RGB')


def region_ratio(im, bounds, fraction, rgb):
    """Fraction of pixels in a client-area region that equal `rgb` exactly."""
    left = bounds['X'] + int(bounds['WIDTH'] * fraction[0])
    top = bounds['Y'] + int(bounds['HEIGHT'] * fraction[1])
    right = bounds['X'] + int(bounds['WIDTH'] * fraction[2])
    bottom = bounds['Y'] + int(bounds['HEIGHT'] * fraction[3])
    pixels = im.crop((left, top, right, bottom)).getdata()
    return sum(1 for p in pixels if p == tuple(rgb)) / max(1, len(pixels))


def classify(declared, failure):
    """Map the declaration and the observed failure to an outcome name."""
    if failure is not None and failure['stage'] == 'environment':
        return 'ENV_ERROR'
    if failure is None:
        return 'PASS' if declared == 'implemented' else 'UNKNOWN'
    return 'REGRESSION' if declared == 'implemented' else 'NOT_IMPLEMENTED'


def run(binary, out, declared, negative):
    """Run the scenario once and return (outcome, detail)."""
    fixture = NavigationFixture()
    if negative == 'stale-page':
        # Control: the request arrives, but the start page is served again.
        fixture._bodies['/destination.html'] = fixture._bodies['/start.html']
    expectations = fixture.expectations
    fraction = expectations['sample_region']['fraction']
    start = expectations['pages']['/start.html']
    destination = expectations['pages']['/destination.html']
    stage = 'environment'
    detail = None
    with fixture:
        try:
            with GuiSession(out, {'WINIT_X11_SCALE_FACTOR': '1'}) as gui:
                (gui.out / 'harness.py').write_bytes(Path(__file__).read_bytes())
                gui.inputs.update({'declared_url_entry_ui': declared, 'negative': negative})
                gui._write_inputs()
                app = gui.launch(binary, [fixture.url('/start.html')])
                win = gui.window(app)
                gui.command('xdotool', 'windowsize', '--sync', win, *map(str, WINDOW))
                gui.command('xdotool', 'windowmove', '--sync', win, '50', '50')
                gui.command('xdotool', 'windowactivate', '--sync', win)
                fixture.wait_for_request('/start.html')
                bounds = geometry(gui, win)
                gui.wait_for('start page painted',
                             lambda: region_ratio(grab(gui), bounds, fraction,
                                                  start['background_rgb']),
                             lambda ratio: ratio >= MATCH_RATIO)
                gui.screenshot('start')

                stage = 'url entry'
                assert '/destination.html' not in fixture.page_requests(), \
                    'destination requested before input'
                gui.command('xdotool', 'mousemove', '--sync', '--window', win,
                            str(bounds['WIDTH'] // 2), str(URL_FIELD_CENTER_Y))
                gui.command('xdotool', 'click', '1')
                gui.command('xdotool', 'key', '--window', win, 'ctrl+a')
                gui.command('xdotool', 'type', '--window', win, '--delay', '20',
                            fixture.url('/destination.html'))
                gui.command('xdotool', 'key', '--window', win, 'Return')

                stage = 'navigation request'
                fixture.wait_for_request('/destination.html')
                stage = 'destination paint'
                ratio = gui.wait_for('destination page painted',
                                     lambda: region_ratio(grab(gui), bounds, fraction,
                                                          destination['background_rgb']),
                                     lambda ratio: ratio >= MATCH_RATIO)
                gui.screenshot('destination')
                assert fixture.page_requests() == ['/start.html', '/destination.html'], \
                    fixture.page_requests()
                assert app.poll() is None, ('GUI process exited', app.returncode)
                gui.evidence.append({'status': 'PASS', 'destination_ratio': ratio})
        except Exception as error:
            detail = {'stage': stage, 'error': repr(error)}
        finally:
            (out / 'requests.json').write_text(json.dumps(
                [r.__dict__ for r in fixture.requests()], indent=2) + '\n')
    return classify(declared, detail), detail


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--url-entry', choices=['implemented', 'unimplemented'],
                        default=URL_ENTRY_UI,
                        help='declared state of the build (default: %(default)s)')
    parser.add_argument('--negative', choices=['stale-page'],
                        help='controlled failure that must not pass')
    args = parser.parse_args()
    args.artifacts.parent.mkdir(parents=True, exist_ok=True)
    outcome, detail = run(args.binary, args.artifacts, args.url_entry, args.negative)
    record = {'outcome': outcome, 'declared_url_entry_ui': args.url_entry,
              'negative': args.negative, 'failure': detail}
    (args.artifacts / 'outcome.json').write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps(record), flush=True)
    sys.exit(EXIT[outcome])


if __name__ == '__main__':
    main()
