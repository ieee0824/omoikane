#!/usr/bin/env python3
"""Measure DOM query growth through the public C FFI, with setup excluded.

Build the library with `cargo build` (or `cargo build --release`), then pass
its path and an output JSON file. Compare identical profiles on an idle host.
The report records the actual binary hash; timing includes the evaluation API
and JSON result decoding. Every warmup and sample validates the result count.
"""

import argparse
import ctypes
import hashlib
import json
from pathlib import Path
import platform
import statistics
import time
from urllib.parse import quote


class Browser:
    def __init__(self, library):
        self.lib = library
        self.pointer = library.omoikane_init()
        if not self.pointer:
            raise RuntimeError("browser initialization failed")

    def string(self, pointer):
        if not pointer:
            raise RuntimeError("null string result")
        try:
            return ctypes.string_at(pointer).decode()
        finally:
            self.lib.omoikane_string_free(pointer)

    def navigate(self, html):
        url = ("data:text/html," + quote(html)).encode()
        if not self.lib.omoikane_navigate(self.pointer, url):
            raise RuntimeError(self.string(self.lib.omoikane_last_error(self.pointer)))

    def evaluate(self, source):
        pointer = self.lib.omoikane_evaluate(self.pointer, source.encode())
        if not pointer:
            raise RuntimeError(self.string(self.lib.omoikane_last_error(self.pointer)))
        response = json.loads(self.string(pointer))
        if "exceptionDetails" in response:
            raise RuntimeError(response["exceptionDetails"])
        return response["result"].get("value")

    def close(self):
        self.lib.omoikane_free(self.pointer)


def load_library(path):
    lib = ctypes.CDLL(str(path.resolve()))
    pointer = ctypes.c_void_p
    signatures = {
        "omoikane_init": ([], pointer),
        "omoikane_free": ([pointer], None),
        "omoikane_navigate": ([pointer, ctypes.c_char_p], ctypes.c_bool),
        "omoikane_evaluate": ([pointer, ctypes.c_char_p], pointer),
        "omoikane_last_error": ([pointer], pointer),
        "omoikane_string_free": ([pointer], None),
    }
    for name, (arguments, result) in signatures.items():
        function = getattr(lib, name)
        function.argtypes, function.restype = arguments, result
    return lib


def prepare(browser, case, size):
    if case == "slots":
        contents = "".join(f'<i slot="s{i}"></i>' for i in range(size))
        html = f'<div id="host">{contents}</div>'
    elif case in ("siblings", "wide"):
        html = "<main>" + "<i></i>" * size + "</main>"
    elif case == "query":
        html = "<main>" + "<div>" * size + "</div>" * size + "</main>"
    else:
        html = "<main>" + "".join(
            "<div>" * size + f'<span id="{name}">text</span>' + "</div>" * size
            for name in ("left", "right")
        ) + "</main>"
    browser.navigate("<!doctype html><html><head></head><body>" + html + "</body></html>")
    if case == "slots":
        markup = "".join(f'<slot name="s{i}"></slot>' for i in range(size))
        browser.evaluate(
            'var root = document.getElementById("host").attachShadow({mode:"open"});'
            "root.innerHTML = " + json.dumps(markup) + ";"
            'var slots = Array.from(root.querySelectorAll("slot"));'
        )
        return "(() => { let n=0; for (const slot of slots) n += slot.assignedNodes().length; return n; })()", size
    if case == "siblings":
        return 'document.querySelectorAll("i + i").length', size - 1
    if case == "wide":
        return 'document.querySelectorAll("i").length', size
    if case == "query":
        return 'document.querySelectorAll("div").length', size
    browser.evaluate(
        "var left = document.createRange(), right = document.createRange();"
        'left.selectNodeContents(document.getElementById("left"));'
        'right.selectNodeContents(document.getElementById("right"));'
    )
    return "left.compareBoundaryPoints(Range.START_TO_START, right)", -1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--library", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profile", required=True, help="Profile of the supplied binary")
    parser.add_argument("--case", choices=["all", "query", "wide", "slots", "siblings", "range"], default="all")
    parser.add_argument("--samples", type=int, default=5)
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("--samples must be positive")
    library = load_library(args.library)
    sizes = {"query": [32, 64, 128, 256], "wide": [128, 256, 512, 1024], "slots": [16, 32, 64, 128],
             "siblings": [128, 256, 512, 1024], "range": [32, 64, 128, 256]}
    results = []
    for case, inputs in sizes.items():
        if args.case not in ("all", case):
            continue
        for size in inputs:
            browser = Browser(library)
            try:
                source, expected = prepare(browser, case, size)
                if browser.evaluate(source) != expected:
                    raise RuntimeError(f"incorrect warmup result: {case}, {size}")
                samples = []
                for _ in range(args.samples):
                    started = time.perf_counter_ns()
                    actual = browser.evaluate(source)
                    samples.append((time.perf_counter_ns() - started) / 1e6)
                    if actual != expected:
                        raise RuntimeError(f"incorrect result: {case}, {size}: {actual}")
                row = {"case": case, "n": size, "samples_ms": samples,
                       "median_ms": statistics.median(samples), "expected": expected}
                results.append(row)
                print(json.dumps(row), flush=True)
            finally:
                browser.close()
    with args.library.open("rb") as binary:
        digest = hashlib.file_digest(binary, "sha256").hexdigest()
    report = {"platform": platform.platform(), "library": str(args.library.resolve()),
              "sha256": digest, "profile": args.profile, "warmups": 1,
              "samples": args.samples, "setup_excluded": True, "results": results}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
