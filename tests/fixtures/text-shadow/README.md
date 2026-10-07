# Text-shadow comparisons

`rendering.html` uses the existing `../acid3/font.ttf` Ahem font and eight
480×360 fixture regions: basic shadow, shadow list/underline, blur, inherited
currentcolor, vertical underline, opacity overflow, clip/blur, and rotation.
The integration test embeds the same font bytes because the local-image
base-path API does not load relative CSS fonts. Solid Ahem glyphs are checked.

Generate images without updating a baseline:

```sh
OMOIKANE_TEXT_SHADOW_IMAGES=<output-dir> cargo test --locked --test text_shadow text_shadow_firefox_fixture
python3 scripts/compare-text-shadow-firefox.py \
  --geckodriver <driver-path> --actual <output-dir>/rendering.actual.png \
  --output <reference-dir>
```

The script uses standard-library WebDriver and Pillow, waits for fonts,
checks device scale 1, keeps the original full screenshot, and compares the
480×360 fixture crop. Actual/reference/diff copies use lossless compression.
Exact changed pixels are recorded per region without a tolerance.

Initial Firefox157.0.1 results: basic/currentcolor zero changed pixels;
list/underline80, vertical/underline40, blur1548 (max channel delta18),
opacity800 (delta1), clip/blur375 (delta11), rotation153 (delta160).
The underline regions expose the existing foreground underline placement
difference; shadows reuse that geometry. Blur uses Omoikane's existing
Gaussian/box approximation, opacity differs in channel rounding, and rotated
edges use the engine's affine sampling. These comparisons document the
remaining rendering differences; they do not claim pixel parity with Firefox.

Record all pinned upstream text-shadow reftests:

```sh
WPT_ROOT=<wpt-root> scripts/fetch-wpt.sh
cargo run --locked --example text_shadow_wpt -- <wpt-root> <report-dir>
```

The utility checks the fixed revision, compares white-page RGBA, applies only
upstream-declared fuzzy limits, and retains all original PNGs and JSON results.
It exits nonzero for failures or unverified results. Both-empty SVG text cases
are unverified. First corrected results at revision
`dc97e7bed3096ac9e0e591ab5fa22e7fb8844ead`: 20 successful comparisons
(18 exact matches, one declared fuzzy match, one intended mismatch), two
failures (`textindent`, `text-shadow-emoji-transparent`), four unverified SVG
cases. Both failures also reproduced in the preserved binary from merged
PR1263, before this implementation: negative text-indent was ignored, and the
emoji reference's absolute white cover left the original foreground visible.
These observations do not establish support for the unrendered SVG cases.
