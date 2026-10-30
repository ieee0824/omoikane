# Background image and compositing comparison fixture

This fixed input covers `background-clip:text`, isolated `background-blend-mode`,
element opacity after blending, `cross-fade()`, and `image-rendering`. Capture it
with `scripts/compare-firefox-rendering.py`; generated PNGs and reports remain in
`.artifacts/issue1269/issue1551-firefox-comparison/` rather than becoming pixel
baselines.

The fixed WPT revision is `dff7cf45938f6e16427f5607bac82a803534484b`.

The 2026-10-30 comparison against Firefox 159.0 was repeat-stable in both
browsers and reported no element geometry differences. The pixel comparison
changed 15,405 of 60,000 pixels (25.675%, mean absolute channel error 17.857);
the generated report records the remaining rendering differences without
turning this diagnostic fixture into a pixel baseline.
