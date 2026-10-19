#!/usr/bin/env python3
"""Verify that parsed page pixels are presented before a slow initial script."""
import argparse
import contextlib
import http.server
import json
from pathlib import Path
import threading
import time

from PIL import ImageGrab

from gui_x11 import GuiSession, geometry, trace_records


@contextlib.contextmanager
def slow_script_page():
    requested = threading.Event()
    delivered = {}

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == '/slow.js':
                requested.set()
                time.sleep(3)
                content_type = 'text/javascript'
                body = (b"document.body.style.background='blue'; document.title='Initialized';"
                        b"setTimeout(()=>document.title='Timer fired',1000);")
            elif self.path == '/':
                content_type = 'text/html'
                body = (b"<title>Parsed</title><style>body{margin:0;background:red}</style>"
                        b"<body><h1>Parsed content</h1><script src='/slow.js'></script></body>")
            else:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header('Content-Type', content_type)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            if self.path == '/slow.js':
                delivered['at'] = time.monotonic()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f'http://127.0.0.1:{server.server_port}/', requested, delivered
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def verify(binary, artifacts):
    env = {'OMOIKANE_TRACE_STARTUP': '1', 'OMOIKANE_TRACE_PAINT': '1'}
    with slow_script_page() as (url, requested, delivered):
        with GuiSession(artifacts, env) as gui:
            app = gui.launch(binary, [url])
            window = gui.window(app)
            gui.command('xdotool', 'windowactivate', '--sync', window)
            log = gui.out / 'browser.log'
            gui.wait_for('slow script requested', requested.is_set, bool)
            bounds = geometry(gui, window)
            point = (bounds['X'] + 500, bounds['Y'] + 200)
            pixel = ImageGrab.grab(xdisplay=gui.env['DISPLAY']).convert('RGB').getpixel(point)
            assert pixel == (255, 0, 0), ('parsed document not presented', pixel)
            paints = trace_records(log, 'PAINT')
            assert len(paints) >= 2, paints
            initial = trace_records(log, 'STARTUP')
            first_page = next(row for row in initial
                              if row['phase'] == 'first_page_redraw_attempted')
            observed_timer = {}

            def title():
                value = gui.command('xdotool', 'getwindowname', window)
                if value == 'Timer fired':
                    observed_timer.setdefault('at', time.monotonic())
                return value

            gui.wait_for('startup script completed', title,
                         lambda value: value in ('Initialized', 'Timer fired'))
            # X11 title changes can be observed before the surface is presented.
            pixel = gui.wait_for(
                'initialized document presented',
                lambda: ImageGrab.grab(xdisplay=gui.env['DISPLAY']).convert('RGB').getpixel(point),
                lambda value: value == (0, 0, 255))
            gui.wait_for('new startup timer fired',
                         title, lambda value: value == 'Timer fired')
            timer_ms = (observed_timer['at'] - delivered['at']) * 1000
            assert timer_ms >= 800, ('startup timer fired too early', timer_ms)
            report = {'first_page_redraw_ms': first_page['elapsed_ms'],
                      'parsed_pixel': [255, 0, 0], 'initialized_pixel': list(pixel),
                      'script_delay_ms': 3000, 'timer_observed_ms': timer_ms}
            (gui.out / 'startup-summary.json').write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(report), flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    verify(args.binary, args.artifacts)
