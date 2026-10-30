# Nine-slice border and mask fixture (#1550)

This fixed input compares `border-image` and `mask-border` nine-slice painting.
It covers the four repeat modes, mixed-axis repeat, outset, corner/edge/center
regions, gradient sources, and alpha masking at fixed element geometry.

Capture with `scripts/compare-firefox-rendering.py` at 500×300 CSS pixels and
DPR 1. Repeated screenshots must be byte-identical for each browser; the
comparison record and retained artifacts document remaining pixel differences.

## Recorded comparison

Captured on 2026-10-30 with Firefox 159.0 / geckodriver 0.37.1 and the
Omoikane library built from this change. Both browsers produced byte-identical
repeated captures, and every probed border box matched exactly. Across the
500×300 image, 27,509 pixels differ and ImageMagick reports normalized MAE
0.063336. The largest visible differences are repeat phase/edge clipping;
Firefox 159 does not apply the unprefixed `mask-border` sample, while Omoikane
does. Original PNGs, metrics, inputs, logs, and the side-by-side image are kept
under `/workspace/.artifacts/issue1269/issue1550-{firefox,omoikane}` and
`issue1550-nine-slice-comparison.png`.
