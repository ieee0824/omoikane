#!/usr/bin/env python3
"""Verify concurrent fetch/XHR and wheel painting before HTTP responses arrive."""
import argparse
import contextlib
import http.server
import json
from pathlib import Path
import threading

from gui_x11 import GuiSession, geometry, trace_records


@contextlib.contextmanager
def held_fetch_page():
    requested = [threading.Event() for _ in range(3)]
    release = [threading.Event() for _ in range(3)]

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path in ('/1', '/2', '/3'):
                index = int(self.path[1]) - 1
                requested[index].set()
                release[index].wait(30)
                content_type, body = 'text/plain', [b'one', b'two', b'three'][index]
            elif self.path == '/':
                content_type = 'text/html'
                body = (
                    b'<title>Fetch ready</title><body style="margin:0;height:8000px">'
                    b'<div style="height:8000px;background:linear-gradient(red,blue)"></div>'
                    b'<script>globalThis.events=[];'
                    b'function report(kind,text){events.push(kind+":"+text);document.title=events.join(",")}'
                    b'setTimeout(()=>{'
                    b'fetch("/1").then(r=>r.text()).then(t=>report("fetch1",t)).catch(()=>document.title="Fetch failed");'
                    b'fetch("/2").then(r=>r.text()).then(t=>report("fetch2",t)).catch(()=>document.title="Fetch failed");'
                    b'const x=new XMLHttpRequest();x.open("GET","/3");'
                    b'x.onload=()=>report("xhr3",x.responseText);'
                    b'x.onerror=()=>document.title="XHR failed";x.send();'
                    b'},2000)</script>'
                )
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
        for event in release:
            event.set()
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def verify(binary, artifacts):
    with held_fetch_page() as (url, requested, release):
        with GuiSession(artifacts, {'OMOIKANE_TRACE_INPUT': '1',
                                    'OMOIKANE_TRACE_PAINT': '1'}) as gui:
            app = gui.launch(binary, [url])
            window = gui.window(app)
            gui.command('xdotool', 'windowactivate', '--sync', window)
            bounds = geometry(gui, window)
            gui.command('xdotool', 'mousemove', str(bounds['X'] + 500),
                        str(bounds['Y'] + 300))
            gui.wait_for('all three fetch/XHR requests started before any response',
                         lambda: all(event.is_set() for event in requested), bool,
                         seconds=5)
            log = gui.out / 'browser.log'
            previous = len(trace_records(log, 'WHEEL'))
            gui.command('xdotool', 'click', '5')
            gui.wait_for('wheel processed while fetch/XHR responses are held',
                         lambda: len(trace_records(log, 'WHEEL')),
                         lambda count: count > previous, seconds=5)
            assert all(row['success'] for row in trace_records(log, 'WHEEL')[previous:])
            gui.wait_for('page painted after wheel before any fetch/XHR response',
                         log.read_text,
                         lambda text: 'OMOIKANE_PAINT ' in text[text.rfind('OMOIKANE_WHEEL '):],
                         seconds=5)

            def title():
                return gui.command('xdotool', 'getwindowname', window)

            assert title() == 'Fetch ready', 'response delivered before release'
            assert not any(event.is_set() for event in release)
            release[1].set()
            gui.wait_for('second fetch settles independently with its exact body',
                         title, lambda value: value == 'fetch2:two')
            release[2].set()
            gui.wait_for('XHR settles while the first fetch is still held',
                         title, lambda value: value == 'fetch2:two,xhr3:three')
            release[0].set()
            gui.wait_for('all responses delivered once with exact bodies',
                         title, lambda value: value == 'fetch2:two,xhr3:three,fetch1:one')
            report = {'concurrent_requests': 3, 'wheel_and_paint_before_responses': True,
                      'response_order': ['fetch2:two', 'xhr3:three', 'fetch1:one']}
            (gui.out / 'fetch-summary.json').write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(report), flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    verify(args.binary, args.artifacts)
