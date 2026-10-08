#!/usr/bin/env python3
"""Verify first-script media settings on an isolated X11 display at DPR 2."""
import argparse
import json
from pathlib import Path
from urllib.parse import quote
from gui_x11 import GuiSession


FIXTURE = """<!doctype html><script>
document.title = 'MEDIA_INIT:' + JSON.stringify({
    dpr: devicePixelRatio,
    screenWidth: screen.width,
    screenHeight: screen.height,
    innerWidth, innerHeight,
    fine: matchMedia('(pointer:fine)').matches,
    resolution: matchMedia('(resolution:2dppx)').matches,
    deviceWidth: matchMedia('(device-width:800px)').matches,
    deviceHeight: matchMedia('(device-height:500px)').matches
});
</script>"""


def run(binary, artifacts):
    # GuiSession owns a 1600x1000 Xvfb display: at scale 2 its screen is 800x500.
    with GuiSession(artifacts, {'WINIT_X11_SCALE_FACTOR': '2'}) as gui:
        gui.inputs['startup_fixture'] = FIXTURE
        gui._write_inputs()
        app = gui.launch(binary, ['data:text/html,' + quote(FIXTURE)])
        window = gui.window(app)
        title = gui.wait_for('first-script media snapshot',
                            lambda: gui.command('xdotool', 'getwindowname', window),
                            lambda value: value.startswith('MEDIA_INIT:'))
        snapshot = json.loads(title.removeprefix('MEDIA_INIT:'))
        assert snapshot['dpr'] == 2, snapshot
        assert snapshot['screenWidth'] == 800, snapshot
        assert snapshot['screenHeight'] == 500, snapshot
        assert all(snapshot[key] for key in ['fine', 'resolution', 'deviceWidth', 'deviceHeight']), snapshot
        # The initial page viewport excludes the 40-CSS-pixel toolbar.
        assert 0 < snapshot['innerWidth'] <= 1280, snapshot
        assert 0 < snapshot['innerHeight'] <= 680, snapshot
        (gui.out / 'outcome.json').write_text(json.dumps(
            {'status': 'PASS', 'first_script': snapshot}, indent=2) + '\n')
        (gui.out / 'test-gui-media-environment.py').write_bytes(Path(__file__).read_bytes())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--artifacts', required=True, type=Path)
    args = parser.parse_args()
    run(args.binary, args.artifacts)


if __name__ == '__main__':
    main()
