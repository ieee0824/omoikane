#!/usr/bin/env python3
"""Check native frameset rendering and named frame navigation on loopback."""
import argparse
import json
from pathlib import Path
from PIL import ImageGrab
from gui_navigation_fixture import NavigationFixture
from gui_x11 import GuiSession, geometry

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / 'tests/fixtures/frameset-live'


def run(binary, artifacts):
    with NavigationFixture(FIXTURE) as fixture:
        with GuiSession(artifacts, {'WINIT_X11_SCALE_FACTOR': '1'}) as gui:
            app = gui.launch(binary, [fixture.url('/')])
            window = gui.window(app)
            gui.command('xdotool', 'windowsize', '--sync', window, '1000', '700')
            gui.command('xdotool', 'windowmove', '--sync', window, '50', '50')
            gui.command('xdotool', 'windowactivate', '--sync', window)
            bounds = geometry(gui, window)

            def colors():
                image = ImageGrab.grab(xdisplay=gui.env['DISPLAY']).convert('RGB')
                return [image.getpixel((bounds['X'] + x, bounds['Y'] + 240))
                        for x in (100, 700)]

            fixture.wait_for_request('/left')
            fixture.wait_for_request('/right')
            gui.wait_for('both frames painted', colors,
                         lambda values: values == [(255, 0, 0), (0, 0, 255)])
            gui.screenshot('frames-before')
            gui.command('xdotool', 'mousemove', '--sync', '--window', window, '20', '50')
            gui.command('xdotool', 'click', '1')
            fixture.wait_for_request('/next')
            gui.wait_for('named right frame navigated', colors,
                         lambda values: values == [(255, 0, 0), (0, 128, 0)])
            gui.screenshot('frames-after')
            paths = fixture.page_requests()
            assert paths.count('/') == 1, paths
            assert paths.count('/left') == 1, paths
            assert paths.count('/right') == 1, paths
            assert paths.count('/next') == 1, paths
            gui.inputs['page_requests'] = paths
            gui._write_inputs()
            (gui.out / 'outcome.json').write_text(json.dumps({'status':'PASS', 'page_requests':paths}, indent=2)+'\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--artifacts', required=True, type=Path)
    args = parser.parse_args()
    run(args.binary, args.artifacts)
    print('PASS: both frames painted and only the named right frame navigated')


if __name__ == '__main__':
    main()
