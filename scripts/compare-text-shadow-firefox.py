#!/usr/bin/env python3
"""Capture the fixed-font text-shadow fixture in Firefox and compare all pixels.

Uses only Python's standard library for WebDriver and Pillow for PNG comparison.
Keeps reference, actual and difference PNGs; does not update any baseline.
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path

from PIL import Image, ImageChops


def capture_screenshot(driver, fixture, output):
    import socket
    import subprocess
    import time
    import urllib.request
    with socket.socket() as reserve:
        reserve.bind(('127.0.0.1', 0))
        port = reserve.getsockname()[1]
    with (output / 'firefox-driver.log').open('w') as log:
        process = subprocess.Popen([driver, '--host', '127.0.0.1', '--port', str(port)], stdout=log, stderr=log)
        session_id = None

        def call(method, path, data=None):
            request = urllib.request.Request(f'http://127.0.0.1:{port}{path}', data=json.dumps(data).encode() if data is not None else None, headers={'Content-Type': 'application/json'}, method=method)
            with urllib.request.urlopen(request, timeout=120) as response:
                return json.load(response)['value']

        try:
            for _ in range(100):
                try:
                    call('GET', '/status')
                    break
                except OSError:
                    if process.poll() is not None:
                        raise RuntimeError('geckodriver exited')
                    time.sleep(.1)
            session = call('POST', '/session', {'capabilities': {'alwaysMatch': {'browserName': 'firefox', 'moz:firefoxOptions': {'args': ['-headless']}}}})
            session_id = session['sessionId']
            prefix = f'/session/{session_id}'
            call('POST', prefix + '/window/rect', {'width': 700, 'height': 600})
            call('POST', prefix + '/url', {'url': fixture.resolve().as_uri()})
            call('POST', prefix + '/timeouts', {'script': 30000})
            call('POST', prefix + '/execute/async', {'script': 'const done=arguments[arguments.length-1];document.fonts.ready.then(()=>requestAnimationFrame(()=>requestAnimationFrame(done)));', 'args': []})
            geometry = call('POST', prefix + '/execute/sync', {'script': 'return {width:innerWidth,height:innerHeight,devicePixelRatio:devicePixelRatio,fontLoaded:document.fonts.check("20px ShadowAhem")};', 'args': []})
            if geometry['devicePixelRatio'] != 1 or not geometry['fontLoaded']:
                raise RuntimeError(f'unexpected capture geometry/font: {geometry}')
            screenshot = base64.b64decode(call('GET', prefix + '/screenshot'))
            (output / 'firefox.original.png').write_bytes(screenshot)
            image = Image.open(output / 'firefox.original.png').convert('RGBA').crop((0, 0, 480, 360))
            image.save(output / 'firefox.png', optimize=True)
            return {'capabilities': session['capabilities'], 'geometry': geometry}
        finally:
            try:
                if session_id:
                    call('DELETE', '/session/' + session_id)
            finally:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--geckodriver', required=True)
    parser.add_argument('--actual', type=Path, required=True)
    parser.add_argument('--fixture', type=Path, default=Path('tests/fixtures/text-shadow/rendering.html'))
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    report = capture_screenshot(args.geckodriver, args.fixture, args.output)
    actual = Image.open(args.actual).convert('RGBA')
    expected = Image.open(args.output / 'firefox.png').convert('RGBA')
    if actual.size != expected.size:
        raise RuntimeError(f'dimensions differ: {actual.size} != {expected.size}')
    actual.save(args.output / 'actual.lossless.png', optimize=True)
    diff = ImageChops.difference(actual, expected)
    diff.save(args.output / 'diff.lossless.png', optimize=True)
    pixels = list(diff.getdata())
    report.update({'dimensions': list(actual.size), 'changed_pixels': sum(any(p) for p in pixels), 'max_channel_delta': max(max(p) for p in pixels), 'fixture_sha256': hashlib.sha256(args.fixture.read_bytes()).hexdigest()})
    names = ['basic', 'list-decoration', 'blur', 'inherit-currentcolor', 'vertical-decoration', 'opacity-overflow', 'clip-blur', 'transform']
    report['regions'] = []
    for i, name in enumerate(names):
        x, y = (i % 4) * 120, (i // 4) * 180
        values = list(diff.crop((x,y,x+120,y+180)).getdata())
        report['regions'].append({'name':name,'changed_pixels':sum(any(p) for p in values),'max_channel_delta':max(max(p) for p in values)})
    (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key:report[key] for key in ['dimensions','changed_pixels','regions']}, indent=2))


if __name__ == '__main__':
    main()
