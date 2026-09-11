# Textarea visual lines against Firefox (#673)

The eight cases cover initial LF, JS-assigned CRLF/CR, wrapping, line height and
padding, clipping, a single-line input control, a second-line caret and selection.
All inputs use the local Liberation Sans file, whose license is included here.
`manifest.json` records the exact HTML hashes.

The implementation splits hard and soft lines, retains UTF-16 editing offsets,
places the first row at the content top, and clips text at the padding edge.
The content width still determines wrapping. Selection and caret use the same
line positions. Enter inserts LF and fires the existing beforeinput/input path;
the textarea value setter normalizes CRLF and CR to LF.

The clipped case specifically distinguishes the content bottom at y=68 from
the padding bottom at y=74. Firefox paints the second row between those edges;
the regression checks visible text there, the intact border at y=74..76 and no
text below it. A separate test preserves a tighter ancestor clip.

Run the capture tool from the repository root, using distinct output directories
for the before and after libraries. Firefox and Omoikane each capture twice:

```sh
python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-textarea-lines --output tests/output/textarea
OMOIKANE_LIBRARY=/absolute/path/to/libomoikane.so \
  python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-textarea-lines --output tests/output/textarea
cargo test --locked --lib paint::form_control -- --nocapture
cargo test --locked --lib textarea_normalizes_newlines -- --nocapture
```

The capture uses production C FFI, localhost HTML, 800x600 CSS pixels and DPR 1.
Firefox waits for fonts and two animation frames. Set `GECKODRIVER` if it is not
on PATH. Linux ARM64 reference: Firefox 155.0.1, geckodriver 0.37.1, 2026-09-11.
Before is main at `c6ab6e8409a0d215ed8e19a6892761bd968e6910`, using the saved
library from the equivalent PR #679 tree. After is this change.

Screenshots are comparison evidence, not a whole-image pass threshold or updated
regression baselines. Font rasterization/baseline differences remain in #677.
Selection color and selected glyph color differ from Firefox's native theme;
the checks concern the selected text range and row. The captured Firefox caret
is not reliably visible, so caret geometry is verified by the native pixel test
and DOM selection offsets rather than claimed pixel equality with Firefox.

`comparison.json` contains all eight before/after/Firefox measurements. All
outer rectangles, values and selection offsets match Firefox after the fix;
repeat images are identical within each state. Full-image differences are
0.121–0.348%, including visible rasterization differences. The input's ink stays
within the same single row/bounds before and after; its pixel difference from
Firefox changes from 951 to 949 pixels in the inner control. Pixel identity with
before is not claimed because restoring the `font` shorthand family also repairs
the specified web font selection.

The final PNGs are losslessly compressed copies. Original/compressed hashes,
sizes and identical RGBA hashes are in `lossless-before.json` / `lossless-after.json`;
library/input metadata is in `provenance.json`. Raw captures remain at
`/workspace/.artifacts/issues673-675/final-captures/textarea-lines/`.
