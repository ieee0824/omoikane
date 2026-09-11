#!/usr/bin/env python3
"""Capture a rendering manifest twice per browser through WebDriver / production FFI.

Requires Firefox and geckodriver for `firefox`, OMOIKANE_LIBRARY for `omoikane`.
No fixture or baseline is rewritten. Outputs include input and library hashes.
"""
import argparse
import base64
import ctypes
import functools
import hashlib
import http.server
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import threading
import time
import urllib.request

METRICS = """JSON.stringify({viewport:[innerWidth,innerHeight],dpr:devicePixelRatio,
elements:Array.from(document.querySelectorAll('[data-probe]')).map(function(e){
var r=e.getBoundingClientRect();return {key:e.id||e.tagName,x:r.x,y:r.y,width:r.width,height:r.height,
clientRects:Array.from(e.getClientRects()).map(function(r){return [r.x,r.y,r.width,r.height]}),
text:e.textContent,value:typeof e.value==='string'?e.value:null,
selectionStart:typeof e.selectionStart==='number'?e.selectionStart:null,
selectionEnd:typeof e.selectionEnd==='number'?e.selectionEnd:null};})})"""


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def firefox(cases, url, output):
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    driver = os.environ.get('GECKODRIVER') or shutil.which('geckodriver')
    if not driver:
        raise RuntimeError('Set GECKODRIVER to a geckodriver executable')
    with (output / 'firefox-driver.log').open('w') as log:
        proc = subprocess.Popen([driver, '--host', '127.0.0.1', '--port', str(port)], stdout=log, stderr=log)
        sid = None

        def call(method, path, data=None):
            request = urllib.request.Request(
                f'http://127.0.0.1:{port}' + path,
                data=json.dumps(data).encode() if data is not None else None,
                headers={'Content-Type': 'application/json'}, method=method,
            )
            with urllib.request.urlopen(request, timeout=120) as response:
                return json.load(response)['value']

        def execute(script):
            return call('POST', '/session/' + sid + '/execute/sync', {'script': script, 'args': []})

        try:
            for _ in range(100):
                try:
                    call('GET', '/status')
                    break
                except OSError:
                    if proc.poll() is not None:
                        raise RuntimeError('geckodriver exited; see firefox-driver.log')
                    time.sleep(.1)
            session = call('POST', '/session', {'capabilities': {'alwaysMatch': {
                'browserName': 'firefox', 'moz:firefoxOptions': {
                    'args': ['-headless'], 'prefs': {'layout.css.devPixelsPerPx': '1.0'},
                },
            }}})
            sid = session['sessionId']
            (output / 'firefox-capabilities.json').write_text(json.dumps(session, indent=2))
            for case in cases:
                width, height = case['width'], case['height']
                prefix = '/session/' + sid
                call('POST', prefix + '/window/rect', {'width': width, 'height': height})
                delta = execute('return [outerWidth-innerWidth,outerHeight-innerHeight]')
                call('POST', prefix + '/window/rect', {'width': width + delta[0], 'height': height + delta[1]})
                call('POST', prefix + '/url', {'url': url + case['path']})
                call('POST', prefix + '/execute/async', {'script':
                    'var done=arguments[0];document.fonts.ready.then(function(){requestAnimationFrame(function(){requestAnimationFrame(function(){done(true);});});});',
                    'args': [],
                })
                metrics = json.loads(execute('return ' + METRICS))
                assert metrics['viewport'] == [width, height] and metrics['dpr'] == 1, metrics
                (output / (case['name'] + '.json')).write_text(json.dumps(metrics, indent=2, ensure_ascii=False))
                for variant in ('expected', 'expected-repeat'):
                    image = call('GET', prefix + '/screenshot')
                    (output / f"{case['name']}.firefox-reference.{variant}.png").write_bytes(base64.b64decode(image))
                print('Firefox captured ' + case['name'], flush=True)
        finally:
            try:
                if sid:
                    call('DELETE', '/session/' + sid)
            finally:
                proc.terminate()
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()


def omoikane(case, url, output):
    library = Path(os.environ['OMOIKANE_LIBRARY']).resolve()
    lib = ctypes.CDLL(str(library))
    ptr = ctypes.c_void_p
    for name, args, result in [
        ('init', [], ptr), ('navigate', [ptr, ctypes.c_char_p], ctypes.c_bool),
        ('screenshot_png_with_viewport', [ptr, ctypes.c_uint32, ctypes.c_uint32], ptr),
        ('evaluate', [ptr, ctypes.c_char_p], ptr), ('last_error', [ptr], ptr),
        ('string_free', [ptr], None), ('free', [ptr], None),
    ]:
        function = getattr(lib, 'omoikane_' + name)
        function.argtypes, function.restype = args, result

    def owned(pointer):
        if not pointer:
            raise RuntimeError('FFI returned null')
        try:
            return ctypes.string_at(pointer).decode()
        finally:
            lib.omoikane_string_free(pointer)

    browser = lib.omoikane_init()
    assert browser
    try:
        assert lib.omoikane_navigate(browser, (url + case['path']).encode()), owned(lib.omoikane_last_error(browser))
        for variant in ('actual', 'actual-repeat'):
            image = owned(lib.omoikane_screenshot_png_with_viewport(browser, case['width'], case['height']))
            (output / f"{case['name']}.firefox-reference.{variant}.png").write_bytes(base64.b64decode(image))
        raw = json.loads(owned(lib.omoikane_evaluate(browser, METRICS.encode())))
        (output / (case['name'] + '.omo-raw.json')).write_text(json.dumps(raw, indent=2))
        metrics = json.loads(raw['result']['value'])
        assert metrics['viewport'] == [case['width'], case['height']] and metrics['dpr'] == 1, metrics
        (output / (case['name'] + '.omoikane.json')).write_text(json.dumps(metrics, indent=2, ensure_ascii=False))
    finally:
        lib.omoikane_free(browser)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['firefox', 'omoikane', 'omo-child'])
    parser.add_argument('--fixtures', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--case')
    parser.add_argument('--url', help=argparse.SUPPRESS)
    args = parser.parse_args()
    fixtures, output = args.fixtures.resolve(), args.output.resolve()
    cases = json.loads((fixtures / 'manifest.json').read_text())
    if args.case:
        cases = [case for case in cases if case['name'] == args.case]
    assert cases, 'No matching cases'
    for case in cases:
        assert digest(fixtures / case['path']) == case['fixture_sha256'], case['name']
    output.mkdir(parents=True, exist_ok=True)
    if args.mode == 'omo-child':
        errors = []
        def worker():
            try:
                omoikane(cases[0], args.url, output)
            except BaseException as error:
                errors.append(error)
        threading.stack_size(64 * 1024 * 1024)
        thread = threading.Thread(target=worker)
        thread.start()
        thread.join()
        if errors:
            raise errors[0]
        return
    provenance = {'cases': cases, 'capture_script_sha256': digest(Path(__file__))}
    if args.mode == 'omoikane':
        library = Path(os.environ['OMOIKANE_LIBRARY']).resolve()
        provenance.update(library=str(library), library_sha256=digest(library))
    (output / (args.mode + '-inputs.json')).write_text(json.dumps(provenance, indent=2))
    with (output / 'http.log').open('a') as log:
        class Handler(http.server.SimpleHTTPRequestHandler):
            def log_message(self, fmt, *values):
                log.write((fmt % values) + '\n')
                log.flush()
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(Handler, directory=str(fixtures)))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            url = f'http://127.0.0.1:{server.server_port}/'
            if args.mode == 'firefox':
                firefox(cases, url, output)
            else:
                for case in cases:
                    with (output / (case['name'] + '.omo.log')).open('w') as child_log:
                        subprocess.run([
                            sys.executable, __file__, 'omo-child', '--fixtures', str(fixtures),
                            '--output', str(output), '--case', case['name'], '--url', url,
                        ], stdout=child_log, stderr=child_log, check=True, timeout=120)
                    print('Omoikane captured ' + case['name'], flush=True)
        finally:
            server.shutdown()
            server.server_close()
            thread.join()


if __name__ == '__main__':
    main()
