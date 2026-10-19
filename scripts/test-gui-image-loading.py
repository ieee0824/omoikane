#!/usr/bin/env python3
"""Verify wheel input and image load delivery while an HTTP response is held."""
import argparse
import contextlib
import http.server
import io
import json
from pathlib import Path
import threading
import time

from PIL import Image
from gui_x11 import GuiSession, geometry, trace_records


@contextlib.contextmanager
def held_image_page():
    requested = threading.Event()
    release = threading.Event()
    encoded = io.BytesIO()
    Image.new('RGB', (1, 1), 'red').save(encoded, format='PNG')

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == '/slow.png':
                requested.set()
                release.wait(15)
                content_type, body = 'image/png', encoded.getvalue()
            elif self.path == '/':
                content_type = 'text/html'
                body = (b'<title>Ready</title><body style="margin:0;height:8000px">'
                        b'<div style="height:8000px;background:linear-gradient(red,blue)"></div>'
                        b'<script>setTimeout(()=>{let i=new Image();'
                        b'i.onload=()=>document.title="Image loaded";'
                        b'i.onerror=()=>document.title="Image failed";'
                        b'i.src="/slow.png";document.body.append(i)},2000)</script>')
            else:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header('Content-Type', content_type)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f'http://127.0.0.1:{server.server_port}/', requested, release
    finally:
        release.set()
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def verify(binary, artifacts):
    with held_image_page() as (url, requested, release):
        with GuiSession(artifacts, {'OMOIKANE_TRACE_INPUT': '1',
                                    'OMOIKANE_TRACE_PAINT': '1'}) as gui:
            app = gui.launch(binary, [url])
            window = gui.window(app)
            gui.command('xdotool', 'windowactivate', '--sync', window)
            bounds = geometry(gui, window)
            gui.command('xdotool', 'mousemove', str(bounds['X'] + 500), str(bounds['Y'] + 300))
            gui.wait_for('image request started', requested.is_set, bool)
            log = gui.out / 'browser.log'
            previous = len(trace_records(log, 'WHEEL'))
            started = time.monotonic()
            try:
                gui.command('xdotool', 'click', '5')
                gui.wait_for('wheel processed before image response',
                             lambda: len(trace_records(log, 'WHEEL')),
                             lambda count: count > previous, seconds=5)
                assert all(row['success'] for row in trace_records(log, 'WHEEL')[previous:])
                gui.wait_for('page painted after wheel while image response is held',
                             log.read_text,
                             lambda text: 'OMOIKANE_PAINT ' in text[text.rfind('OMOIKANE_WHEEL '):],
                             seconds=5)
                wheel_ms = (time.monotonic() - started) * 1000
                assert not release.is_set()
                title = gui.command('xdotool', 'getwindowname', window)
                assert title == 'Ready', ('premature image notification', title)
            finally:
                release.set()
            gui.wait_for('image load delivered after response',
                         lambda: gui.command('xdotool', 'getwindowname', window),
                         lambda title: title == 'Image loaded')
            report = {'wheel_and_paint_before_response_ms': wheel_ms,
                      'image_load_delivered': True}
            (gui.out / 'image-summary.json').write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(report), flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    verify(args.binary, args.artifacts)
