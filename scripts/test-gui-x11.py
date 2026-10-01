#!/usr/bin/env python3
"""Verify startup-URL paint and native hit testing at scale factors 1 and 2."""

import argparse
import http.server
import json
from pathlib import Path
import threading

from PIL import ImageGrab

from gui_x11 import GuiSession, geometry

# This scenario owns its fixture and pixel/DOM expectations; the helper does not.
FIXTURE = b'''<title>GUI SCALE READY</title><style>
body { margin:0; background:#f8f1db; }
#target { position:absolute; left:60px; top:60px; width:100px; height:70px;
background:#12ab34; }
</style><div id="target"></div><script>
document.getElementById('target').addEventListener('mousedown', function(e) {
 document.title = 'HIT:' + JSON.stringify({x:e.clientX,y:e.clientY,
 width:innerWidth,height:innerHeight});
});
</script>'''


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/html')
        self.send_header('Content-Length', str(len(FIXTURE)))
        self.end_headers()
        self.wfile.write(FIXTURE)

    def log_message(self, *args):
        pass


def marker_bounds(im):
    pixels = im.load()
    coordinates = [(x, y) for y in range(im.height) for x in range(im.width)
                   if pixels[x, y] == (18, 171, 52)]
    if not coordinates:
        return None
    xs, ys = zip(*coordinates)
    return [min(xs), min(ys), max(xs) + 1, max(ys) + 1]


def verify(binary, out, scale):
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with GuiSession(out, {'WINIT_X11_SCALE_FACTOR': str(scale)}) as gui:
            (gui.out / 'fixture.html').write_bytes(FIXTURE)
            (gui.out / 'harness.py').write_bytes(Path(__file__).read_bytes())
            app = gui.launch(binary, [f'http://127.0.0.1:{server.server_port}/'])
            win = gui.window(app)
            gui.command('xdotool', 'windowsize', '--sync', win, '1000', '700')
            gui.command('xdotool', 'windowmove', '--sync', win, '50', '50')
            gui.command('xdotool', 'windowactivate', '--sync', win)
            gui.wait_for('startup URL ready', lambda: gui.command('xdotool', 'getwindowname', win),
                         lambda title: title == 'GUI SCALE READY')
            box = geometry(gui, win)
            rect = gui.wait_for('marker painted at device scale',
                lambda: marker_bounds(ImageGrab.grab(xdisplay=gui.env['DISPLAY']).convert('RGB')),
                lambda rect: rect is not None and rect[2]-rect[0] == 100*scale
                             and rect[3]-rect[1] == 70*scale)
            assert rect[0] - box['X'] == 60*scale, (rect, box)
            toolbar = rect[1] - box['Y'] - 60*scale
            assert 0 <= toolbar < 200*scale, (toolbar, rect, box)
            gui.screenshot('startup')
            # An independently calculated CSS point, converted to physical client pixels.
            gui.command('xdotool', 'mousemove', '--sync', '--window', win,
                        str(100*scale), str(toolbar+90*scale))
            gui.command('xdotool', 'click', '1')
            hit = gui.wait_for('native click hits CSS target',
                lambda: gui.command('xdotool', 'getwindowname', win),
                lambda title: title.startswith('HIT:'))
            hit = json.loads(hit[4:])
            assert hit['x'] == 100 and hit['y'] == 90, hit
            assert hit['width'] == box['WIDTH']/scale, (hit, box)
            assert hit['height'] == (box['HEIGHT']-toolbar)/scale, (hit, box, toolbar)
            gui.screenshot('clicked')
            gui.evidence.append({'status': 'PASS', 'scale': scale, 'hit': hit,
                                 'marker': rect, 'bounds': box, 'toolbar': toolbar})
    finally:
        server.shutdown()
        server.server_close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    args.artifacts.mkdir(parents=True, exist_ok=False)
    for scale in (1, 2):
        verify(args.binary, args.artifacts / f'scale-{scale}', scale)


if __name__ == '__main__':
    main()
