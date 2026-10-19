#!/usr/bin/env python3
"""Close the native window while the page worker executes a long script."""
import argparse
import contextlib
import http.server
import json
from pathlib import Path
from urllib.parse import quote
import threading
import time

from gui_x11 import GuiSession, trace_records


@contextlib.contextmanager
def busy_page():
    delivered = threading.Event()
    release = threading.Event()
    script_delivered_at = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == '/':
                body = b"<title>Busy page</title><script src='/busy.js'></script>"
                kind = 'text/html'
            elif self.path == '/busy.js':
                if not release.wait(15):
                    self.send_error(504)
                    return
                # A real monotonic clock bounds the work, independently of
                # interpreter speed. Large JSON work stays below the loop cap.
                body = (b"const payload='x'.repeat(100000);"
                        b"const until=performance.now()+3000;"
                        b"while(performance.now()<until){JSON.stringify([payload,payload,payload])}"
                        b"document.title='Done';")
                kind = 'text/javascript'
            else:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header('Content-Type', kind)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            if self.path == '/busy.js':
                script_delivered_at.append(time.monotonic())
                delivered.set()

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f'http://127.0.0.1:{server.server_port}/', delivered, release, script_delivered_at
    finally:
        release.set()
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def verify(binary, artifacts):
    with busy_page() as (url, delivered, release, script_delivered_at):
        with GuiSession(artifacts, {'OMOIKANE_TRACE_STARTUP': '1'}) as gui:
            app = gui.launch(binary, [url])
            window = gui.window(app)
            gui.command('xdotool', 'windowactivate', '--sync', window)
            release.set()
            gui.wait_for('busy script delivered', delivered.is_set, bool)
            time.sleep(.2)
            assert app.poll() is None, 'application exited before close test'
            assert time.monotonic() - script_delivered_at[0] < 1.5, 'missed busy interval'
            gui.command('xdotool', 'windowactivate', '--sync', window)
            started = time.monotonic()
            # Ask the window manager for WM_DELETE_WINDOW. windowclose would
            # destroy the X11 window and can terminate an unresponsive client.
            gui.command('xdotool', 'key', 'alt+F4')
            gui.wait_for('native close during page script', app.poll,
                         lambda status: status is not None, seconds=2)
            elapsed_ms = (time.monotonic() - started) * 1000
            assert elapsed_ms < 750, elapsed_ms
            report = {'native_close_ms': elapsed_ms, 'busy_duration_ms': 3000,
                      'close_after_script_delivery_ms': (started - script_delivered_at[0])*1000}
            (gui.out / 'page-worker-summary.json').write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(report), flush=True)


def verify_hidden_timer(binary, artifacts):
    html = """<title>Loading</title><script>
    document.addEventListener('visibilitychange', () => {
        if (document.hidden) {
            document.title = 'Hidden waiting';
            setTimeout(() => { document.title = 'Hidden timer completed'; }, 500);
        }
    });
    document.title = 'Ready';
    </script>"""
    with GuiSession(artifacts) as gui:
        app = gui.launch(binary, ['data:text/html,' + quote(html)])
        window = gui.window(app)
        title = lambda: gui.command('xdotool', 'getwindowname', window)
        gui.wait_for('hidden fixture loaded', title, lambda value: value == 'Ready')
        gui.command('xdotool', 'windowminimize', window)
        gui.wait_for('timer completes while hidden', title,
                     lambda value: value == 'Hidden timer completed', seconds=5)
        assert app.poll() is None
        (gui.out / 'hidden-timer-summary.json').write_text(json.dumps(
            {'hidden_timer_completed': True}, indent=2) + '\n')


@contextlib.contextmanager
def held_wheel_page():
    requested, release = threading.Event(), threading.Event()

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == '/hold.js':
                requested.set()
                if not release.wait(20):
                    self.send_error(504)
                    return
                body, kind = b"document.title='Released';", 'text/javascript'
            elif self.path == '/':
                body = b"""<title>Wheel queue</title><style>body{height:50000px}</style>
                <script>let total=0,shifted=0;
                window.addEventListener('wheel',event=>{
                    event.preventDefault();total+=event.deltaY;
                    if(event.shiftKey)shifted+=event.deltaY;
                    document.title='Wheel:'+total+':'+shifted+':'+window.scrollY;
                },{passive:false});</script><script src='/hold.js'></script>"""
                kind = 'text/html'
            else:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header('Content-Type', kind)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f'http://127.0.0.1:{server.server_port}/', requested, release
    finally:
        release.set()
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def verify_wheel_queue(binary, artifacts):
    with held_wheel_page() as (url, requested, release):
        with GuiSession(artifacts, {'OMOIKANE_TRACE_INPUT': '1'}) as gui:
            app = gui.launch(binary, [url])
            window = gui.window(app)
            gui.command('xdotool', 'windowactivate', '--sync', window)
            gui.wait_for('wheel script held', requested.is_set, bool)
            gui.command('xdotool', 'mousemove', '--window', window, '600', '350')
            # Accumulate real native inputs while the page owner is blocked.
            # A genuine Shift transition must remain a FIFO boundary.
            gui.command('xdotool', 'click', '--repeat', '20', '--delay', '15', '5')
            gui.command('xdotool', 'keydown', 'Shift_L')
            gui.command('xdotool', 'click', '--repeat', '20', '--delay', '15', '5')
            gui.command('xdotool', 'keyup', 'Shift_L')
            release.set()
            log = gui.out / 'browser.log'
            def wheels():
                return [row for row in trace_records(log, 'PAGE_INPUT')
                        if row['kind'] == 'wheel']
            rows = gui.wait_for('all native wheels delivered', wheels,
                                lambda rows: sum(r['source_count'] for r in rows) == 80)
            assert max(row['source_count'] for row in rows) > 1, rows
            title = gui.wait_for('wheel result presented',
                                 lambda: gui.command('xdotool', 'getwindowname', window),
                                 lambda value: value == 'Wheel:3200:1600:0')
            assert app.poll() is None
            (gui.out / 'wheel-queue-summary.json').write_text(json.dumps({
                'native_sources': 80, 'delivered_events': len(rows),
                'max_batch': max(row['source_count'] for row in rows),
                'title': title}, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    verify(args.binary, args.artifacts / 'busy')
    verify_hidden_timer(args.binary, args.artifacts / 'hidden')

    verify_wheel_queue(args.binary, args.artifacts / 'wheel-queue')
