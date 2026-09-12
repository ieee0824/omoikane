# Inline background and CSSOM rectangles against Firefox (#674)

These thirteen fixed-font inputs cover the originally missing span background and
all-zero `getBoundingClientRect()`, wrapping, nested and empty spans, font-size,
line-height, padding/borders, preserved whitespace, preformatted empty/trailing
lines, and empty-line margin collapse.
`manifest.json` records each exact input hash; the TTF embedded in each HTML is
`tests/fixtures/acid2/LiberationSans-Regular.ttf` (SHA-256
`76d04c18ea243f426b7de1f3ad208e927008f961dc5945e5aad352d0dfde8ee8`).
Its license remains in `tests/fixtures/acid2/LiberationSans-LICENSE.txt`.

## Method and evidence

Linux ARM64, Firefox 155.0.1 / geckodriver 0.37.1, 800x600 CSS pixels, DPR 1,
2026-09-11. Both browsers navigate to the same localhost-served HTML. Firefox
waits for `document.fonts.ready` and two animation frames; Omoikane uses its
production C FFI navigate/evaluate/screenshot functions. Before is the merged
main tree at `c6ab6e8409a0d215ed8e19a6892761bd968e6910` (the immutable library
from the equivalent PR #679 tree). After contains the inline-box change in
this directory's commit. Capture each state twice; all 78 captures have stable
RGB within their respective before/after/Firefox pair.

`comparison.json` retains the before/after/Firefox DOM rectangles and client
rectangles, full-image pixel differences, and repetition results. The three
PNG variants per case preserve the actual captured images. These are comparison
evidence, not refreshed screenshot baselines or a whole-browser conformance claim.
`firefox-before.json` supplies the immutable reference measurements to the Rust
CSSOM regression test. `provenance.json` records the libraries and inputs;
`lossless-before.json` and `lossless-after.json` record original/compressed PNG
hashes, sizes and identical RGBA hashes. The raw captures remain under
`/workspace/.artifacts/issues673-675/final-captures/inline-boxes/`.

The embedded-font control also exposed URL payload corruption and a missing
data-URL font-loading path. Those are fixed with #675. Final captures use the
requested web face; an earlier comparison had silently used an installed font.

## Fix and results

Previously inline elements were flattened into text fragments with no box owned
by the element. Paint and CSSOM therefore had no corresponding background or
rectangle to use. Inline boundaries now retain element ownership through line
breaking; one region per occupied line is shared by paint and CSSOM. Parent
backgrounds precede descendants and text; the original text DOM nodes remain
unchanged. Empty-only lines retain their zero-area position without creating
vertical space or preventing adjoining margins from collapsing. Plain text and
replaced-only lines skip the owner-region/font-metric pass.

All thirteen cases have the expected rectangle counts and exact measured vertical
positions/heights, including three wrapped regions, nested 20px/14px font areas,
zero-area empty spans and preserved preformatted trailing spaces. Backgrounds
are visible in the original, nested and wrapped cases. Tests additionally check
painted interior pixels and geometry invalidation after text/style/scroll changes.

Horizontal positions and advances are not claimed to be numerically identical.
The largest measured border-box width delta is 0.051663px. An independent
fixed-font shaping probe gives `highlighted inline` raw advance 149.00390625px;
summing each glyph advance rounded to 1/60px gives 148.9666748px, matching the
Firefox measurement. The analogous `Normal ` prefix explains its -0.006663px
origin delta. CSSOM tests bound only nonzero inline horizontal coordinates by
half a 1/60px unit per character in the containing test paragraph, plus 0.001px
serialization allowance. Empty widths, heights, vertical positions, block
geometry and rectangle counts do not receive this horizontal budget. An
independent unit test asserts the actual unrounded advance to 0.0001px.

The first client rect of `anonymized-inline-pre` is an additional known shaping
difference: `A  ` measures 23.349609px in Omoikane and 24.433334px in Firefox.
Both values are asserted explicitly and tracked in #677. The 0.051663px figure
above is the maximum **bounding-box** delta, not a bound for every client rect.

The images still show different glyph rasterization and baseline placement;
those are visible, retained evidence for #677, not hidden by the geometry test.
Full-image differences are descriptive measurements, not a pass threshold.
Multiline border slicing, arbitrary bidi/vertical-writing combinations and
all CSS inline formatting behavior are not established by these thirteen cases.

## #677 raster scale follow-up

The checked-in `anonymized-font-file.after.png` retains the pre-#677 output.
Omoikane had passed the CSS em size directly to `ab_glyph`, whose pixel scale
uses `ascent - descent` rather than `unitsPerEm`. Shaping and layout already
used `font-size / unitsPerEm`, so glyph outlines were rendered at a smaller,
inconsistent scale. The #677 fix converts CSS pixels to the `ab_glyph` scale
and uses the selected face's ascent and descent plus half-leading for the text
baseline.

With the same 800x600, DPR 1 input, Firefox's dark-glyph bounds are 19px high
at y=6 within every 28px line. The fixed Omoikane output is 18px high at y=7,
improved from 17px at y=10. Changed pixels fell from 8,439 (1.758%) to 7,332
(1.528%), and ImageMagick normalized MAE fell from 0.00699089 to 0.00354003.
Omoikane's two captures were identical. Block geometry remained exact; the
largest inline geometry delta remained 0.037325px because Firefox quantizes
glyph advances to 1/60px. The remaining one-pixel ink and coverage differences
come from the separate `ab_glyph` and Firefox rasterizers and are not used as a
pass tolerance.

Firefox also omits the fixed font's A-space kerning from the preserved trailing
spaces in `A  `, while Rustybuzz applies it. CSS `font-kerning: auto` permits
the user agent to choose whether to apply kerning, so #677 records this result
without adding a Firefox-specific shaping exception.

## Regression commands

```sh
python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-inline-boxes --output tests/output/inline
OMOIKANE_LIBRARY=/absolute/path/to/libomoikane.so \
  python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-inline-boxes --output tests/output/inline
cargo test --locked --lib inline_ -- --nocapture
WPT_ROOT=/path/to/pinned/wpt WPT_REQUIRED=1 cargo test --locked -- --include-ignored
cargo build --locked
```

The implementation follows the distinction between line-box leading and the
inline content area in [CSS inline formatting](https://www.w3.org/TR/CSS22/visuren.html#inline-formatting),
and retains per-fragment boxes for [CSSOM geometry](https://drafts.csswg.org/cssom-view/#dom-element-getclientrects).
