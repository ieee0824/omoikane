# Multi-column rendering fixtures (#696, #795)

This fixed input covers balanced columns, a full-width `column-span: all`
element, fixed-height `column-fill: auto` overflow, column rules and line
fragmentation with `orphans`/`widows`. The #795 cases add general block
fragments, `box-decoration-break: slice` / `clone`, and visible descendant
overflow that continues into a later column without advancing following flow.
The solid block colors make column geometry and rule paint independent of
installed fonts; the final text sample in `basic.html` also exercises inline
flow between fragmentainers.

Capture Firefox and Omoikane twice with `scripts/compare-firefox-rendering.py`
and write generated output outside this fixture directory. The manifest records
the exact input hash. Full-image text differences are descriptive because the
two browsers use different text rasterizers. The Rust regression test checks
the fixed block/rule pixels and line fragment geometry.

Firefox 155.0.1 and Omoikane were captured twice at DPR 1 on Linux ARM64. The
latest #795 comparison was recorded on 2026-09-22. Both browsers were stable
across repeats. `general-block-fragments.html`, `box-decoration-break.html`,
and `overflow-fragments.html` have identical geometry and all RGBA pixels.
For `overflow-fragments.html`, the outer block exposes 16px, 8px, and 0px
client rects while its 40px descendant exposes 16px, 16px, and 8px rects; the
following block still begins at the second column's 8px offset. The existing
text-bearing `basic.html` remains within 0.031 CSS pixels and differs in 1,682
pixels (1.05125%), primarily from text rasterization.

Raw #795 images, DOM measurements, hashes, and the numeric comparison are
retained under `/workspace/.artifacts/issue795/firefox-repo-fixtures-overflow-v1/`
and `/workspace/.artifacts/issue795/firefox-comparison-overflow-v1.json` in the
development record. The exact overflow pair is also stored in visual-store run
`issue795-overflow-parity-final`; its pixel hash is identical across both
browsers and both repeats.

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
