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
from gui_x11 import GuiSession, geometry


def wheel_progress(text):
    """Observe delivery and presentation after the test's input boundary."""
    inputs, presented = [], set()
    painted_after_delivery = False
    for line in text.rpartition('\n')[0].splitlines():
        if line.startswith('OMOIKANE_PAGE_INPUT '):
            row = json.loads(line.split(' ', 1)[1])
            if row['kind'] in ('wheel', 'native_scroll_accepted', 'native_scroll_fallback'):
                assert row['success'], row
                sources = row['gui_sequences']
                assert not sources['truncated'], row
                assert len(sources['ids']) == sources['total_count'] == row['source_count'], row
                inputs.extend(sources['ids'])
                painted_after_delivery = False
        elif line.startswith('OMOIKANE_PAINT '):
            painted_after_delivery = bool(inputs)
        elif line.startswith('OMOIKANE_NATIVE_SCROLL_PROBE '):
            row = json.loads(line.split(' ', 1)[1])
            if row['event'] == 'present' and row['success']:
                presented.add(row['gui_sequence'])
    assert len(inputs) == len(set(inputs)), ('duplicate wheel delivery', inputs)
    return {'sources': len(inputs),
            'presented': painted_after_delivery or bool(presented.intersection(inputs))}


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
            # Retain a line boundary so earlier paints cannot satisfy the test.
            previous = log.read_text().rfind('\n') + 1
            started = time.monotonic()
            try:
                gui.command('xdotool', 'click', '5')
                gui.wait_for('wheel processed before image response',
                             lambda: wheel_progress(log.read_text()[previous:]),
                             lambda result: result['sources'] == 2, seconds=5)
                gui.wait_for('page painted after wheel while image response is held',
                             lambda: wheel_progress(log.read_text()[previous:]),
                             lambda result: result['sources'] == 2 and result['presented'],
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
