#!/usr/bin/env python3
"""Summarize deterministic Firefox/Omoikane capture output."""

import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess


def rgb(path: Path, width: int, height: int) -> bytes:
    header = path.read_bytes()[:24]
    assert struct.unpack("!II", header[16:24]) == (width, height), path
    result = subprocess.check_output(
        ["convert", "-limit", "thread", "1", str(path), "-alpha", "remove", "-depth", "8", "rgb:-"]
    )
    assert len(result) == width * height * 3, path
    return result


def rect_delta(expected, actual):
    return [actual[index] - expected[index] for index in range(4)]


def summarize(root: Path, output: Path | None = None, destination: Path | None = None):
    output = output or root / "tests" / "output"
    report = []
    for case in json.loads((root / "manifest.json").read_text()):
        name = case["name"]
        width = case["width"]
        height = case["height"]
        prefix = output / f"{name}.firefox-reference"
        firefox_png = Path(f"{prefix}.expected.png")
        firefox_repeat_png = Path(f"{prefix}.expected-repeat.png")
        omoikane_png = Path(f"{prefix}.actual.png")
        omoikane_repeat_png = Path(f"{prefix}.actual-repeat.png")
        firefox_rgb = rgb(firefox_png, width, height)
        omoikane_rgb = rgb(omoikane_png, width, height)
        changed = 0
        strong = 0
        total_error = 0
        max_error = 0
        bounds = [width, height, -1, -1]
        for index in range(0, len(firefox_rgb), 3):
            errors = [
                abs(firefox_rgb[index + channel] - omoikane_rgb[index + channel])
                for channel in range(3)
            ]
            maximum = max(errors)
            total_error += sum(errors)
            max_error = max(max_error, maximum)
            if maximum:
                changed += 1
                pixel = index // 3
                x = pixel % width
                y = pixel // width
                bounds = [
                    min(bounds[0], x),
                    min(bounds[1], y),
                    max(bounds[2], x),
                    max(bounds[3], y),
                ]
            if maximum > 16:
                strong += 1

        firefox = json.loads((output / f"{name}.json").read_text())
        omoikane = json.loads((output / f"{name}.omoikane.json").read_text())
        assert firefox["viewport"] == omoikane["viewport"] == [width, height]
        assert len(firefox["elements"]) == len(omoikane["elements"])
        elements = []
        maximum_geometry_delta = 0.0
        for expected, actual in zip(firefox["elements"], omoikane["elements"]):
            assert expected["key"] == actual["key"]
            geometry = {
                key: actual[key] - expected[key]
                for key in ("x", "y", "width", "height")
                if actual[key] != expected[key]
            }
            maximum_geometry_delta = max(
                [maximum_geometry_delta, *(abs(value) for value in geometry.values())]
            )
            expected_rects = expected.get("clientRects", [])
            actual_rects = actual.get("clientRects", [])
            rect_deltas = [
                rect_delta(expected_rect, actual_rect)
                for expected_rect, actual_rect in zip(expected_rects, actual_rects)
            ]
            if geometry or len(expected_rects) != len(actual_rects) or any(
                any(value != 0 for value in delta) for delta in rect_deltas
            ) or any(expected.get(key) != actual.get(key) for key in (
                "value", "selectionStart", "selectionEnd"
            )):
                elements.append({
                    "key": expected["key"],
                    "firefox_geometry": {
                        key: expected[key] for key in ("x", "y", "width", "height")
                    },
                    "omoikane_geometry": {
                        key: actual[key] for key in ("x", "y", "width", "height")
                    },
                    "geometry_delta": geometry,
                    "firefox_client_rects": expected_rects,
                    "omoikane_client_rects": actual_rects,
                    "client_rect_deltas": rect_deltas,
                    "firefox_state": {
                        key: expected.get(key)
                        for key in ("value", "selectionStart", "selectionEnd")
                    },
                    "omoikane_state": {
                        key: actual.get(key)
                        for key in ("value", "selectionStart", "selectionEnd")
                    },
                })
        item = {
            **case,
            "changed_pixels": changed,
            "changed_percent": changed / (width * height) * 100,
            "strong_difference_pixels": strong,
            "mean_absolute_channel_error": total_error / len(firefox_rgb),
            "max_channel_error": max_error,
            "diff_bbox": bounds if changed else None,
            "repeat_stable": {
                "firefox": rgb(firefox_repeat_png, width, height) == firefox_rgb,
                "omoikane": rgb(omoikane_repeat_png, width, height) == omoikane_rgb,
            },
            "max_geometry_delta": maximum_geometry_delta,
            "differences": elements,
            "image_sha256": {
                "firefox": hashlib.sha256(firefox_png.read_bytes()).hexdigest(),
                "omoikane": hashlib.sha256(omoikane_png.read_bytes()).hexdigest(),
            },
        }
        report.append(item)
        print(
            f"{name}: pixels={item['changed_percent']:.3f}% "
            f"max-geometry={maximum_geometry_delta:.6f}px "
            f"reported-elements={len(elements)} "
            f"stable={item['repeat_stable']['firefox']}/{item['repeat_stable']['omoikane']}"
        )
    destination = destination or root / "comparison-current.json"
    destination.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(destination)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--destination", type=Path)
    arguments = parser.parse_args()
    summarize(arguments.root, arguments.output, arguments.destination)
