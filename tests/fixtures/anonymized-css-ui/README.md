# CSS UI control comparison fixture

`fixed.html` covers native checkbox, radio, and range painting; `accent-color`;
light and dark `color-scheme`; `appearance:none`; `field-sizing:content`; a
resizable textarea with `caret-color`; and a `user-select:none`/`cursor:grab`
target. Pointer, selection, touch, caret, and resize behavior is asserted by
Rust integration tests because a static screenshot cannot exercise it.

Capture both engines without rewriting the fixture:

```sh
GECKODRIVER=<geckodriver> python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-css-ui --output <artifact-dir>
OMOIKANE_LIBRARY=<libomoikane.so> python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-css-ui --output <artifact-dir>
python3 scripts/summarize-firefox-rendering.py \
  tests/fixtures/anonymized-css-ui --output <artifact-dir> \
  --destination <artifact-dir>/comparison.json
```

The manifest pins the exact input. Generated images and reports are review
artifacts, not checked-in pixel baselines.

The fixed WPT revision is `dff7cf45938f6e16427f5607bac82a803534484b`.
`accent-color-parsing.html` and `accent-color-computed.html` are selected in the
WPT smoke manifest. The other properties have focused engine regressions for
their computed values and input/paint behavior.
Both selected cases passed on 2026-10-30 (2 total, 0 regressions). The full
`wpt_smoke` test binary also attempted four unrelated wptserve transport
self-tests; those timed out during server startup, while the selected runner
passed independently and wrote `wpt-report.json` with the pinned revision.

On 2026-10-30, Firefox 159.0 and this branch were captured at 420×240 CSS
pixels. Both images had the requested dimensions. They differed at 8,405 of
100,800 pixels (8.338%), within x=15..259 and y=29..216; native widget shape,
font rasterization, and control spacing are intentionally not claimed to be
pixel-identical. The original PNGs and hashes are retained under
`.artifacts/issue1269/issue1548-firefox-comparison/`.
