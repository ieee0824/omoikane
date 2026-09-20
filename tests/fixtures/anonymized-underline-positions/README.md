# Underline position and offset (#751)

`fixed.html` has 90 solid blue underline cases: horizontal-tb / vertical-rl /
vertical-lr, six position values, and auto / zero / positive / negative /
percentage offsets. The shared bundled Liberation Sans fixture makes metrics
independent of installed system fonts. Its license remains alongside the font.

Viewport: 1000 × 900 CSS pixels, DPR 1. `text-decoration-skip-ink: none` isolates
line placement even where a negative offset intersects transparent glyphs.
Each 100 × 100 cell contains one case, with its element at cell origin + (20,20).

`fixed.firefox.png` and `firefox-bounds.json` are the Firefox 155.0.1 reference,
captured after document.fonts.ready on 2026-09-20. The PNG is losslessly
compressed with identical pixels and resolution. The capture used the same
font bytes embedded as a data URL; this checked-in input instead references
the shared local font file. Original inputs, images, numeric analysis and
capture script are preserved in `/workspace/.artifacts/issue751-firefox-matrix2`
and `/workspace/.artifacts/issue751-matrix-input2`.

Do not replace the Firefox reference with an Omoikane result. Re-capture with
Firefox explicitly, verify the font hash and all 90 cases, inspect the image,
and record the reason before changing this reference.

`overline.firefox.png` uses the identical input with only text-decoration-line
changed to overline. The regression test checks both 90-case images exactly;
all underline offset changes must leave overlines fixed. Its original capture
is in `/workspace/.artifacts/issue751-firefox-overline1`.


## Nested decoration origins

`nested.html` has 36 cases with parent font-size 20px and child font-size 20px
or 40px. Every second case adds child `text-underline-position: under right`
and `text-underline-offset: 30px`; the child does not originate a decoration.
The ancestor's authored 20% offset must remain 4px, and its underline must
stay a single straight line. Viewport: 1080 × 960, DPR 1.

`nested.firefox.png` and `nested-firefox-bounds.json` retain Firefox's actual
output without corrections. In eight vertical cases (13, 15, 17, 19, 25, 27,
29, 31), Firefox 155.0.1 moves the segment under the child toward the text.
The reference shows the resulting jog. This conflicts with the originating-box
rule in [CSS Text Decoration 4](https://drafts.csswg.org/css-text-decor-4/#text-underline-position-property)
and #751's explicit requirement that descendants cannot move an ancestor's
line. The regression therefore compares unchanged cases to Firefox and checks
ALL child-override cases against the corresponding unchanged ancestor line,
including every blue pixel; it does not reproduce or silently erase the
Firefox discrepancy. The other 28 cases match Firefox exactly after snapping
fragment endpoints consistently.

Original capture and comparison: `/workspace/.artifacts/issue751-firefox-nested1`,
`/workspace/.artifacts/issue751-native-nested2`. A separate pre-existing mixed
font-size horizontal line-height discrepancy (20px vs Firefox 27px) is tracked
in #781; neither that geometry nor a new baseline is used to hide decoration
failures.
