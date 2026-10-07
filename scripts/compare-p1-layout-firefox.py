#!/usr/bin/env python3
"""Compare the P1 layout fixture's geometry and optional screenshot with Firefox."""

import argparse
import base64
import hashlib
import json
from pathlib import Path
import socket
import subprocess
import time
import urllib.request

from PIL import Image, ImageChops


def capture(driver, fixture, output):
    with socket.socket() as reserve:
        reserve.bind(("127.0.0.1", 0))
        port = reserve.getsockname()[1]
    with (output / "driver.log").open("w") as log:
        process = subprocess.Popen(
            [driver, "--host", "127.0.0.1", "--port", str(port)],
            stdout=log, stderr=log,
        )
        session_id = None

        def call(method, path, data=None):
            request = urllib.request.Request(
                f"http://127.0.0.1:{port}{path}", method=method,
                data=json.dumps(data).encode() if data is not None else None,
                headers={"Content-Type": "application/json"},
            )
            with urllib.request.urlopen(request, timeout=120) as response:
                return json.load(response)["value"]

        try:
            for _ in range(100):
                try:
                    call("GET", "/status")
                    break
                except OSError:
                    if process.poll() is not None:
                        raise RuntimeError("geckodriver exited")
                    time.sleep(0.1)
            session = call("POST", "/session", {"capabilities": {"alwaysMatch": {
                "browserName": "firefox", "moz:firefoxOptions": {"args": ["-headless"]},
            }}})
            session_id = session["sessionId"]
            prefix = f"/session/{session_id}"
            call("POST", prefix + "/window/rect", {"width": 700, "height": 600})
            call("POST", prefix + "/url", {"url": fixture.resolve().as_uri()})
            probe = fixture.with_name("probe.js").read_text()
            geometry = call("POST", prefix + "/execute/sync", {"script": "return " + probe, "args": []})
            screenshot = base64.b64decode(call("GET", prefix + "/screenshot"))
            (output / "firefox.original.png").write_bytes(screenshot)
            Image.open(output / "firefox.original.png").convert("RGBA").crop((0, 0, 300, 320)).save(
                output / "firefox.lossless.png", optimize=True,
            )
            return {"capabilities": session["capabilities"], "geometry": geometry}
        finally:
            try:
                if session_id:
                    call("DELETE", "/session/" + session_id)
            finally:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--geckodriver", required=True)
    parser.add_argument("--fixture", type=Path, default=Path("tests/fixtures/p1-layout/index.html"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--actual", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    report = capture(args.geckodriver, args.fixture, args.output)
    expected = json.loads(args.fixture.with_name("expected.json").read_text())
    report["geometry_matches"] = report["geometry"] == expected
    report["fixture_sha256"] = hashlib.sha256(args.fixture.read_bytes()).hexdigest()
    if args.actual:
        actual = Image.open(args.actual).convert("RGBA")
        reference = Image.open(args.output / "firefox.lossless.png").convert("RGBA")
        if actual.size != reference.size:
            raise RuntimeError(f"dimensions differ: {actual.size} != {reference.size}")
        actual.save(args.output / "omoikane.lossless.png", optimize=True)
        difference = ImageChops.difference(actual, reference)
        difference.save(args.output / "diff.lossless.png", optimize=True)
        pixels = list(difference.getdata())
        report["changed_pixels"] = sum(any(pixel) for pixel in pixels)
        report["max_channel_delta"] = max(max(pixel) for pixel in pixels)
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({key: report[key] for key in ["geometry_matches", "changed_pixels"] if key in report}))
    if not report["geometry_matches"] or report.get("changed_pixels", 0):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
