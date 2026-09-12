# Acid2 rendering fixtures

`acid2.html`, `reference.html`, and `reference.png` preserve the Acid2 test and
its official reference. `acid2.baseline.png` is Omoikane's local regression
snapshot at an 800 × 600 viewport; it shows the introduction before scrolling
to the face. The separate official-reference comparison aligns the face.

## Fixed font for the local baseline

The local baseline comparison and its refresh utility use the unmodified
`LiberationSans-Regular.ttf` from Liberation Fonts 2.1.5. A scoped, thread-local
test override supplies the same font to the existing document-rendering
pipeline for both layout and paint. It does not affect production font
selection or other rendering tests. The comparison requires every pixel to
match; it does not allow platform-specific text differences.

The previous baseline was generated with macOS Hiragino fonts, while Linux
selected installed sans-serif fonts. Its 12,000-pixel allowance could both
hide rendering changes and fail when the host font or glyph metrics changed.
Pinning the font removes that environmental difference and permits strict
regression detection.

## Baseline refresh review (2026-09-08)

The previous checked-in baseline, the unchanged engine's Linux rendering, and
the rendering with the bundled font were inspected before refreshing the PNG.
The pre-change test passed locally with 11,940 changed pixels under its
12,000-pixel allowance; CI failed with its host's font selection.

Comparing the decoded RGBA bytes from the pre-change and fixed-font renders
finds 2,015 changed pixels, all within the text region `(61, 21)`–`(722, 65)`.
There are no changes outside that region: link underlines and both fixed bars
keep their existing positions and colors. These are font rasterization
changes, not a new layout or link-style change in this patch.

The old checked-in baseline differs from the fixed-font render by 11,994 raw
pixels. It predates commit `00d93ca` (2026-03-23), which intentionally added blue
links and underlines to the user-agent defaults. Its wider Hiragino text also
wraps the introduction onto an extra line. The only non-text difference is
144 pixels at `(132, 108)`–`(179, 110)`: the shorter introduction exposes three
more rows of the upper black bar. The fixture gives that bar `top: 9em` at
12px, hence y=108, and gives the introduction a white background and
`z-index: 2`. The yellow band at y=120–125, lower black band at y=144–155, and
red band at y=156–161 match the old baseline exactly.

The expected canvas must receive the decoded RGBA bytes directly. Passing the
baseline through `draw_image` would resample it even at its native size;
floating-point coordinate rounding alone produces 630 false differing pixels
when that operation is applied to the fixed-font image itself. The strict
comparison therefore checks the stored pixels without involving image drawing.

## Baseline refresh review for #677 (2026-09-12)

The CSS em-square correction changed 7,931 pixels within the introduction text
region `(60, 101)`–`(729, 152)`. It did not change wrapping, link positions,
bars, or the Acid2 face. The old and new text-region crops were inspected before
refreshing: the new glyph size and baseline follow the same Liberation Sans
`unitsPerEm` scale used by shaping and layout. The strict local-baseline test
passes with zero changed pixels after the refresh.

The uncompressed old baseline, new render, and diff remain in the development
evidence under `/workspace/.artifacts/issues677/acid2-baseline-review/`. The
displayed crop was losslessly compressed and the original images were retained.

Font source:
[official Liberation Fonts 2.1.5 release](https://github.com/liberationfonts/liberation-fonts/releases/tag/2.1.5),
[`liberation-fonts-ttf-2.1.5.tar.gz`](https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz).
The complete SIL Open Font License 1.1 and copyright notices are preserved in
[`LiberationSans-LICENSE.txt`](LiberationSans-LICENSE.txt).

Font SHA-256:
`76d04c18ea243f426b7de1f3ad208e927008f961dc5945e5aad352d0dfde8ee8`.

Regenerate only after visually comparing the old baseline, actual image,
and diff, following [the rendering fixture rules](../../README.md):

```sh
OMOIKANE_REFRESH_BASELINE=1 cargo test refresh_acid2_baseline_png -- --ignored
cargo test acid2_fixture_matches_local_baseline_png
```
