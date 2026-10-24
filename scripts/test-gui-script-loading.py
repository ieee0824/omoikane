#!/usr/bin/env python3
"""Check concurrent async script fetches and wheel painting before responses."""
import argparse
import contextlib
import http.server
import json
from pathlib import Path
import threading

from gui_x11 import GuiSession, geometry, trace_records


@contextlib.contextmanager
def held_script_page():
    requested = [threading.Event(), threading.Event()]
    release = [threading.Event(), threading.Event()]

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path in ('/1.js', '/2.js'):
                index = int(self.path[1]) - 1
                requested[index].set()
                release[index].wait(30)
                content_type = 'text/javascript'
                body = f'window.executed.push({index + 1});'.encode()
            elif self.path == '/':
                content_type = 'text/html'
                body = (
                    b'<title>Ready</title><body style="margin:0;height:8000px">'
                    b'<div style="height:8000px;background:linear-gradient(red,blue)"></div>'
                    b'<script>window.executed=[];window.loaded=[];setTimeout(()=>{'
                    b'for(let n of [1,2]){let s=document.createElement("script");'
                    b's.async=true;s.src="/"+n+".js";'
                    b's.onload=()=>{loaded.push(n);document.title="Loaded:"+'
                    b'loaded.join("")+";Executed:"+executed.join("")};'
                    b's.onerror=()=>document.title="Script failed";'
                    b'document.body.append(s)}},2000)</script>'
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
    with held_script_page() as (url, requested, release):
        with GuiSession(artifacts, {'OMOIKANE_TRACE_INPUT': '1',
                                    'OMOIKANE_TRACE_PAINT': '1'}) as gui:
            app = gui.launch(binary, [url])
            window = gui.window(app)
            gui.command('xdotool', 'windowactivate', '--sync', window)
            bounds = geometry(gui, window)
            gui.command('xdotool', 'mousemove', str(bounds['X'] + 500),
                        str(bounds['Y'] + 300))
            gui.wait_for('both async requests started before either response',
                         lambda: all(event.is_set() for event in requested), bool,
                         seconds=5)
            log = gui.out / 'browser.log'
            previous = len(trace_records(log, 'WHEEL'))
            gui.command('xdotool', 'click', '5')
            gui.wait_for('wheel processed while script responses are held',
                         lambda: len(trace_records(log, 'WHEEL')),
                         lambda count: count > previous, seconds=5)
            assert all(row['success'] for row in trace_records(log, 'WHEEL')[previous:])
            gui.wait_for('page painted after wheel before either script response',
                         log.read_text,
                         lambda text: 'OMOIKANE_PAINT ' in text[text.rfind('OMOIKANE_WHEEL '):],
                         seconds=5)
            title = lambda: gui.command('xdotool', 'getwindowname', window)
            assert title() == 'Ready', 'script ran before its response was released'
            assert not any(event.is_set() for event in release)
            release[1].set()
            gui.wait_for('second async script executed and loaded before first',
                         title, lambda value: value == 'Loaded:2;Executed:2')
            release[0].set()
            gui.wait_for('each async script executed and loaded exactly once',
                         title, lambda value: value == 'Loaded:21;Executed:21')
            report = {'concurrent_requests': True, 'wheel_and_paint_before_responses': True,
                      'execution_and_load_order': [2, 1]}
            (gui.out / 'script-summary.json').write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(report), flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    verify(args.binary, args.artifacts)
