#!/usr/bin/env python3
"""Record Firefox CSSOM and sRGB canvas observations for the Color 4 fixture.

Set GECKODRIVER if geckodriver is not on PATH. The output is evidence; this
script never overwrites the committed reference fixture.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import time
import urllib.request

PROBE = """return arguments[0].map(value => {
  const element = document.createElement('div');
  document.body.append(element);
  element.style.color = value;
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = 1;
  const context = canvas.getContext('2d', {colorSpace: 'srgb'});
  context.fillStyle = value;
  context.fillRect(0, 0, 1, 1);
  const result = {
    input: value, specified: element.style.color,
    computed: getComputedStyle(element).color,
    pixel: Array.from(context.getImageData(0, 0, 1, 1).data)
  };
  element.remove();
  return result;
});"""


def capture(driver, values, output):
    with socket.socket() as reserve:
        reserve.bind(('127.0.0.1', 0))
        port = reserve.getsockname()[1]
    with (output / 'firefox-driver.log').open('w') as log:
        process = subprocess.Popen(
            [driver, '--host', '127.0.0.1', '--port', str(port)],
            stdout=log, stderr=log,
        )
        session_id = None

        def call(method, path, data=None):
            request = urllib.request.Request(
                f'http://127.0.0.1:{port}' + path,
                data=json.dumps(data).encode() if data is not None else None,
                headers={'Content-Type': 'application/json'}, method=method,
            )
            with urllib.request.urlopen(request, timeout=120) as response:
                return json.load(response)['value']

        try:
            for _ in range(100):
                try:
                    call('GET', '/status')
                    break
                except OSError:
                    if process.poll() is not None:
                        raise RuntimeError('geckodriver exited; see firefox-driver.log')
                    time.sleep(.1)
            session = call('POST', '/session', {'capabilities': {'alwaysMatch': {
                'browserName': 'firefox',
                'moz:firefoxOptions': {'args': ['-headless']},
            }}})
            session_id = session['sessionId']
            records = call('POST', f'/session/{session_id}/execute/sync', {
                'script': PROBE, 'args': [values],
            })
            return {'capabilities': session['capabilities'], 'records': records}
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
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--values', type=Path, default=(
        Path(__file__).resolve().parents[1] / 'tests/fixtures/css-color4/values.json'
    ))
    args = parser.parse_args()
    driver = os.environ.get('GECKODRIVER') or shutil.which('geckodriver')
    if not driver:
        parser.error('Set GECKODRIVER to a geckodriver executable')
    args.output.mkdir(parents=True, exist_ok=True)
    raw = args.values.read_bytes()
    result = capture(driver, json.loads(raw), args.output)
    result['values_sha256'] = hashlib.sha256(raw).hexdigest()
    (args.output / 'firefox-reference.json').write_text(
        json.dumps(result, indent=2) + '\n'
    )
    print(f"Recorded {len(result['records'])} colors in {args.output}")


if __name__ == '__main__':
    main()
