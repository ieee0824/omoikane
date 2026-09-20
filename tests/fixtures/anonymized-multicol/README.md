# Multi-column rendering fixture (#696)

This fixed input covers balanced columns, a full-width `column-span: all`
element, fixed-height `column-fill: auto` overflow, column rules and line
fragmentation with `orphans`/`widows`. The solid block colors make column
geometry and rule paint independent of installed fonts; the final text sample
also exercises inline flow between fragmentainers.

Capture Firefox and Omoikane twice with `scripts/compare-firefox-rendering.py`
and write generated output outside this fixture directory. The manifest records
the exact input hash. Full-image text differences are descriptive because the
two browsers use different text rasterizers. The Rust regression test checks
the fixed block/rule pixels and line fragment geometry.

Firefox 155.0.1 and Omoikane were captured twice at 500x320 CSS pixels and
DPR 1 on Linux ARM64 on 2026-09-20. Each browser produced identical RGBA in
its two captures. All probed block geometries agree within 0.031 CSS pixels;
1,682 pixels (1.05125%) differ, primarily in text rasterization. The solid
column blocks, spanner, overflow column and rules align visually. Raw images,
DOM measurements, hashes, the numeric comparison and lossless-compression
record are retained under
`/workspace/.artifacts/issue696-firefox-comparison-final-6lines/` in the
development record. General block fragment boxes and their CSSOM rectangles
are tracked separately in #795.

## Verification

Run the two capture modes and summarize them with:

```sh
python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-multicol --output /tmp/multicol-capture
OMOIKANE_LIBRARY=/absolute/path/to/libomoikane.so \
  python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-multicol --output /tmp/multicol-capture
python3 scripts/summarize-firefox-rendering.py \
  tests/fixtures/anonymized-multicol --output /tmp/multicol-capture
```
