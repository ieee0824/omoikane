#!/usr/bin/env python3
"""Exercise native close, queued input and scrolling during page-worker work."""
import argparse
import contextlib
import http.server
import json
from pathlib import Path
from urllib.parse import quote
import threading
import time

from PIL import ImageGrab

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


def runtime_error_html(blocking):
    script = 'const blocking=' + json.dumps(blocking) + ';' + r"""
    let total=0, runs=0;
    function update(){document.title='Wheel:'+total+':'+scrollY}
    addEventListener('wheel',event=>{total+=event.deltaY;update()}, {passive:true});
    addEventListener('scroll', update);
    function work(){
        runs++;
        document.body.style.setProperty('--worker-busy-'+runs, '1');
        if(blocking && runs===1){
            addEventListener('wheel',event=>event.preventDefault(),{passive:false});
        }
        const payload='x'.repeat(100000), until=performance.now()+20000;
        while(performance.now()<until){JSON.stringify([payload,payload,payload])}
    }
    setTimeout(()=>Promise.resolve().then(work),2000);
    setTimeout(()=>Promise.resolve().then(work),2200);
    document.title='Ready';
    """
    rows = '<div class="row"></div>' * 100
    return ('<style>html,body{margin:0}.row{height:80px;background:#e04040}'
            '.row:nth-child(3n+2){background:#40c080}'
            '.row:nth-child(3n){background:#4060e0}</style>'
            '<body>' + rows + '<script>' + script + '</script>')


def runtime_error_state(text):
    """Accept complete busy markers, ending them at the next owner turn result."""
    complete, separator, _ = text.rpartition('\n')
    lines = complete.splitlines() if separator else []
    second_busy, completed, errors = False, False, 0
    for line in lines:
        if line.startswith('OMOIKANE_ATTRIBUTE_SET '):
            try:
                row = json.loads(line.split(' ', 1)[1])
            except json.JSONDecodeError:
                continue
            if not isinstance(row, dict) or row.get('name') != 'style':
                continue
            declarations = row.get('declarations')
            changed = declarations.get('changed_properties') if isinstance(declarations, dict) else []
            if isinstance(changed, list) and '--worker-busy-2' in changed:
                second_busy, completed = True, False
        elif second_busy and line.startswith('OMOIKANE_FRAME_EVENT_LOOP '):
            completed = True
        elif line.startswith('page worker operation failed:') and 'RuntimeLimit' in line:
            errors += 1
    return {
        'errors': errors, 'second_busy': second_busy,
        'busy_in_progress': second_busy and not completed,
    }


def verify_scroll_after_runtime_error(binary, artifacts, blocking):
    environment = {'OMOIKANE_TRACE_PAINT': '1', 'OMOIKANE_TRACE_INPUT': '1',
                   'OMOIKANE_TRACE_NATIVE_POLICY': '1', 'OMOIKANE_TRACE_ATTRIBUTE_SET': '1'}
    with GuiSession(artifacts, environment) as gui:
        app = gui.launch(binary, ['data:text/html,' + quote(runtime_error_html(blocking))])
        window = gui.window(app)
        gui.command('xdotool', 'windowactivate', '--sync', window)
        gui.command('xdotool', 'windowsize', window, '640', '440')
        gui.command('xdotool', 'mousemove', '--window', window, '500', '240')
        log = gui.out / 'browser.log'
        probes = lambda: trace_records(log, 'NATIVE_SCROLL_PROBE')
        gui.wait_for('initial native authority',
                     lambda: any(row.get('event') == 'publication' and row.get('eligible')
                                 for row in probes()), bool)
        gui.wait_for('page error reported', lambda: runtime_error_state(log.read_text()),
                     lambda state: state['errors'] >= 1, seconds=15)
        gui.wait_for('second busy callback', lambda: runtime_error_state(log.read_text()),
                     lambda state: state['second_busy'] and state['busy_in_progress'], seconds=10)
        gui.command('xdotool', 'click', '--repeat', '2', '--delay', '40', '5')

        def presents():
            return [row for row in probes() if row.get('event') == 'present'
                    and row.get('success')]

        if blocking:
            time.sleep(.4)
            assert not presents(), 'blocking wheel listener permitted native default'
            expected_scroll, expected_color = 0, (224, 64, 64)
        else:
            rows = gui.wait_for('native surfaces after page error', presents,
                                lambda rows: len({row['gui_sequence'] for row in rows}) == 4,
                                seconds=2)
            assert {tuple(row['visual_offset']) for row in rows} >= {
                (0.0, 40.0), (0.0, 80.0), (0.0, 120.0), (0.0, 160.0)}
            expected_scroll, expected_color = 160, (64, 96, 224)
        assert runtime_error_state(log.read_text())['busy_in_progress'], 'owner finished before native check'
        geometry = dict(line.split('=', 1) for line in gui.command(
            'xdotool', 'getwindowgeometry', '--shell', window).splitlines())
        image = ImageGrab.grab(xdisplay=gui.env['DISPLAY']).convert('RGB')
        color = image.getpixel((int(geometry['X']) + 500, int(geometry['Y']) + 60))
        assert color == expected_color, (color, expected_color)
        gui.screenshot('after-error-scroll')
        gui.wait_for('owner delivers all wheel defaults exactly once',
                     lambda: gui.command('xdotool', 'getwindowname', window),
                     lambda title: title == f'Wheel:160:{expected_scroll}', seconds=15)
        if blocking:
            assert not presents(), 'blocking owner later accepted a native default'
        final_image = ImageGrab.grab(xdisplay=gui.env['DISPLAY']).convert('RGB')
        assert final_image.getpixel((int(geometry['X']) + 500,
                                     int(geometry['Y']) + 60)) == expected_color
        assert app.poll() is None
        receipts = {row['gui_sequence']: row['mono_ns'] for row in probes()
                    if row.get('event') == 'receipt' and row.get('nonzero')}
        first_presents = {}
        for row in presents():
            first_presents.setdefault(row['gui_sequence'], row)
        latencies = [(row['mono_ns'] - receipts[sequence]) / 1_000_000
                     for sequence, row in first_presents.items()]
        (gui.out / 'runtime-error-scroll-summary.json').write_text(json.dumps({
            'blocking': blocking, 'native_presented_inputs': len(first_presents),
            'max_receipt_to_present_ms': max(latencies, default=None),
            'scroll_y': expected_scroll, 'color': color,
            'runtime_errors': runtime_error_state(log.read_text())['errors']}, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--scenario', choices=('all', 'runtime-error-scroll'), default='all')
    args = parser.parse_args()
    if args.scenario == 'all':
        verify(args.binary, args.artifacts / 'busy')
        verify_hidden_timer(args.binary, args.artifacts / 'hidden')
        verify_wheel_queue(args.binary, args.artifacts / 'wheel-queue')
    verify_scroll_after_runtime_error(args.binary, args.artifacts / 'timeout-scroll-passive', False)
    verify_scroll_after_runtime_error(args.binary, args.artifacts / 'timeout-scroll-blocking', True)
