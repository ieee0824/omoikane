# Clip path, paint order, and outline comparison

`page.html` is the fixed Firefox/Omoikane comparison input for Issue #1552. It
covers compound `path()` fill rules, quadratic/cubic and arc `shape()` commands,
rounded `xywh()`, HTML text `paint-order`, dashed/dotted/double/auto outlines,
multiline inline and form-control integration, and a negative-offset elliptical
rounded outline. Solid backgrounds keep the clip and outline boundaries visible;
font rasterization differences are descriptive rather than a pixel-parity gate.

Capture both engines twice without modifying the fixture:

```sh
python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/clip-path-outline \
  --output .artifacts/issue1269/issue1552-firefox-comparison
OMOIKANE_LIBRARY=/absolute/path/to/libomoikane.so \
python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/clip-path-outline \
  --output .artifacts/issue1269/issue1552-firefox-comparison
python3 scripts/summarize-firefox-rendering.py \
  tests/fixtures/clip-path-outline \
  --output .artifacts/issue1269/issue1552-firefox-comparison \
  --destination .artifacts/issue1269/issue1552-firefox-comparison/comparison.json
```

The generated screenshots, browser/library provenance, repeat hashes, DOM
geometry, and numeric comparison are review artifacts, not checked-in pixel
baselines. `manifest.json` pins the exact HTML input.

On 2026-10-30, Firefox 159.0/geckodriver 0.37.1 and the branch Omoikane
library were captured twice at 720×420 CSS pixels and DPR 1. Both engines were
pixel-stable across repeats, and all twelve probed border boxes matched with a
maximum geometry delta of 0px. Visual review confirmed that all four clip
shapes, all four box outlines, the multiline inline outline, form-control
outline, and the distinct fill-first/stroke-first text samples painted in both
engines. The full images differ at 13,536 of 302,400 pixels (4.476%), primarily
in curved-edge antialiasing, dashed/dotted phase, control styling, and font
rasterization; this is descriptive and not a parity tolerance. Original PNGs,
repeat captures, hashes, capabilities, logs, and `comparison.json` are retained
under `/workspace/.artifacts/issue1269/issue1552-firefox-comparison/`.
