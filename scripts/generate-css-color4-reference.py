#!/usr/bin/env python3
"""Generate independent Color 4 gamut vectors (requires coloraide==8.13).

Write to an explicit output path; the committed fixture is never overwritten
unless it is deliberately passed as --output.
"""
import argparse
import importlib.metadata
import json
import math
from pathlib import Path

VALUES = [
    'color(srgb .2 .3 .4)', 'color(srgb-linear .2 .3 .4)',
    'color(display-p3 .2 .3 .4)', 'color(display-p3-linear .2 .3 .4)',
    'color(display-p3 1 0 0)', 'color(display-p3 0 1 0)',
    'color(display-p3 0 0 1)', 'color(a98-rgb .2 .3 .4)',
    'color(prophoto-rgb .2 .3 .4)', 'color(rec2020 .2 .3 .4)',
    'color(xyz-d50 .2 .3 .4)', 'color(xyz-d65 .2 .3 .4)',
    'lab(50 20 30)', 'lch(50 30 40)', 'oklab(.5 .1 .05)', 'oklch(.5 .2 30)',
    'lab(50 125 -125)', 'oklch(.5 .4 180)', 'color(srgb -.1 .5 .5)',
    'color(display-p3 1.01 0 0)', 'color(display-p3 2 2 2)', 'lab(0 100 100)',
]


def generate():
    from coloraide import Color
    from coloraide.spaces import prophoto_rgb_linear

    # ColorAide 8.13 has an older ProPhoto matrix. Use the current CSS matrix
    # while retaining the independent conversion and fitting implementation.
    prophoto_rgb_linear.RGB_TO_XYZ[:] = [
        [0.7977666449006423, 0.13518129740053308, 0.0313477341283922],
        [0.2880748288194013, 0.711835234241873, 0.00008993693872564],
        [0.0, 0.0, 0.8251046025104602],
    ]
    records = []
    for text in VALUES:
        color = Color(text)
        raw = color.convert('srgb').coords()
        mapped = color.convert('srgb').fit('srgb', method='oklch-chroma').coords()
        # Its in-gamut tolerance admits tiny negative values; the CSS final
        # output and the canvas channels are bounded to [0, 1].
        mapped = [min(1.0, max(0.0, c)) for c in mapped]
        record = {
            'input': text, 'unmapped_srgb': raw, 'mapped_srgb': mapped,
            'pixel': [math.floor(c * 255 + .5) for c in mapped] + [255],
        }
        if text == 'color(srgb -.1 .5 .5)':
            record['expected_canvas_pixel'] = [0, 128, 128, 255]
            record['quantization_note'] = (
                'ColorAide roundtrip leaves .5 slightly below the exact half-byte '
                'boundary; sRGB identity conversion retains .5 and rounds ties upward.'
            )
        records.append(record)
    return {
        'tool': 'ColorAide 8.13', 'method': 'oklch-chroma',
        'prophoto_matrix': 'CSS Color 4 conversions.js, 2026-10-01',
        'records': records,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    try:
        version = importlib.metadata.version('coloraide')
    except importlib.metadata.PackageNotFoundError:
        version = None
    if version != '8.13':
        parser.error('Install coloraide==8.13 for reproducible vectors')
    result = generate()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(f"Recorded {len(result['records'])} gamut vectors in {args.output}")


if __name__ == '__main__':
    main()
