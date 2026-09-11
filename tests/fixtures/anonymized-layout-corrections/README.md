# Grid, flex text and collapsed margins against Firefox (#669–#671)

These anonymized inputs reproduce the three P1 layout issues under
[#668](https://github.com/ieee0824/omoikane/issues/668).
`manifest.json` gives the input hashes and viewport for the 50 primary cases.
The supplementary scroll and clear inputs are indexed by `scroll-comparison.json`
and `clear-comparison.json`. The source
HTML stays unchanged between the before and after captures. The measurement
record is not a screenshot baseline or a blanket browser conformance result.

## Method

Firefox 155.0.1 / geckodriver 0.37.1 and Omoikane ran on the same Linux ARM64
host on 2026-09-11, with DPR 1. Serve these files over localhost; use the
manifest viewport (800 × 600 for the isolated cases, 800/1280 × 900 for the
portal). Firefox waits for `document.fonts.ready` and two animation frames.
Omoikane uses the production C FFI navigate/evaluate/screenshot entry points.
Capture twice in each browser and compare RGB within each browser separately
before comparing engines. Measurements use absolute viewport coordinates;
no translation aligns away the original margin defect.

The isolated cases request Liberation Sans. The portal retains its original
sans-serif font request. This does not guarantee identical system font choice
or text rasterization. The Rust regression tests explicitly load the checked-in
Liberation Sans file, avoiding platform-dependent numeric expectations.

```js
Array.from(document.querySelectorAll(
  '[data-probe],.shell,.header,.brand,.main,.hero,.grid,.card,.meter,.footer'
), e => {
  const r = e.getBoundingClientRect();
  return {key: e.id || e.className || e.tagName.toLowerCase(),
          x: r.x, y: r.y, width: r.width, height: r.height};
});
```

Keep the returned order: several elements deliberately share a class/key.
The original scripts, logs, JSON and all PNG captures are retained at
`/workspace/.artifacts/issues669-671/` in the development environment.

## Causes and coverage

- **#669:** Grid computed the final row area but did not pass stretch height
  to child layout. Fixed rows now provide a definite used height on the first
  pass; content-sized tracks reflow only when their final used height changes.
  Tests cover empty and content-bearing cells, explicit height, start alignment,
  padding/borders, min/max constraints, percentage descendants and subgrids.
- **#670:** The flex item list discarded text nodes, and intrinsic sizing of
  nested flex containers also excluded direct text and gaps. Contiguous text
  now forms anonymous items using the existing inline formatter. Comments and
  hidden elements do not split a text run; whitespace-only runs create no item,
  while NBSP remains content. DOM nodes and their parentage stay unchanged.
  Cases cover direct versus span-wrapped text, adjacent text nodes, mixed items,
  row/column layout, wrapping, centered columns and the original portal brand.
- **#671:** Collapsed child margins were subtracted without propagating the
  resulting margin through the parent. A scoped margin strut carries the largest
  positive and most negative adjoining margins separately. Parent/child,
  sibling and empty-block collapse, root boundaries, border/padding, min-height,
  formatting contexts, clear, and positioned descendants have explicit tests.
  Clear checks float occupancy at the cleared border edge, so a cleared block
  regains its full available width.

The margin metadata lasts for one layout invocation, does not retain DOM nodes,
and is discarded or replaced on reflow. Child position corrections are combined
before walking descendants. The changes do not add a dependency or alter the
public layout struct or FFI API.

## Regression commands

```sh
cargo test --locked --lib layout
WPT_ROOT=/path/to/pinned/wpt WPT_REQUIRED=1 cargo test --locked -- --include-ignored
cargo build --locked
```

The margin tests read these exact fixtures and assert all four rectangle
coordinates against Firefox. Grid and flex tests also inspect nested layout
and inline text fragments; DOM-only or image-only checks would miss several
of the reproduced failures. Existing assertions remain enabled; the justified fixture and baseline updates
below keep their original contracts.

## Measured results

The broader comparison rendered 59 cases twice in each browser (236 stable
captures): 50 of the 58 cases with probes had exactly equal rectangles, and
39 cases had exactly equal RGB images. Acid2 has no selected rectangle probes
and is excluded from geometry counts. The 50 cases checked into this directory contribute 200 captures,
43 exact-geometry cases and all 39 exact-image cases. The remaining nine broader
cases exercise other issues and are not represented as resolved by this change.

All isolated Grid and margin cases have exact Firefox rectangles. The original
block, flex and grid examples and their four padding/explicit-height controls
also have exactly equal RGB images. In particular, the original grid's six cells
now have heights 60/90 instead of 0, and the block's outer y is 24 instead of 0.

The direct flex text appears with the same line structure as its span control.
The nested portal brand now appears on one line and has the same y=17.5 and
height=36 as Firefox. Its width is 236.289 versus Firefox's 247.16667, with the
original generic font request preserved. This remaining font/measurement
difference is not silently included in a passing geometric tolerance.
The isolated direct/adjacent-text rectangle differences are at most 0.005329px;
the centered-column case is at most 0.011004px (maxima rounded upwards). They
are recorded unrounded in `comparison.json`; the Rust tests compare direct and
wrapped layout under the same fixed font. No screenshot baseline or existing
test tolerance was relaxed.

The original portal still differs in card/line heights, fonts, shadows and
rounded/SVG edges. The ordinary portal case also retains the scrollbar width
difference; the separate no-scrollbar input is recorded independently. These
pages are not claimed to match Firefox as a whole. Other text/inline/paint issues
remain tracked under #668, including #674 and #676–#677.

The `*.firefox-reference.actual.png` files are the observed Omoikane output;
`*.firefox-reference.expected.png` are Firefox captures. They are review evidence,
not test baselines. The direct-text and portal examples were visually inspected.

## Existing-test updates required by the margin correction

The full suite initially passed 2301 tests and failed the local Acid2 image
comparison and the nested-scroll pixel test. These were investigated before
changing either fixture.

Firefox confirms that the original nested-scroll input has outer scrollHeight
30 and clientHeight 30, so scrollTop=10 clamps to 0. Its inner box belongs at
y=10. The old layout lost that margin, putting the inner box at y=0, which made
the old pixel assertions pass without actually scrolling the outer box.
The nested-scroll fixture now adds a transparent 10px trailing spacer, giving
the outer box a real 10px range. All original pixel assertions remain, and
unscrolled assertions require the inner box's 10px margin. A second test keeps
the original input and asserts its clamped result. `scroll-comparison.json`
records before/Firefox/after geometry and scroll values for these two additional
inputs; they are separate from the 50-case manifest above.

The local Acid2 baseline moved its introductory text down by 84px when its
3.5em top margin stopped disappearing. The intro's white background now covers
the fixed black/color bars, as intended by the fixture's own CSS comment and
visible in Firefox. Both the old and new images were visually inspected.
A new paint regression makes the intro transparent and requires the fixed bar
to remain visible at the same viewport coordinate, ruling out loss of that box.
After these checks, the existing opt-in refresh utility updated
`../acid2/acid2.baseline.png`; the exact all-pixel comparison remains unchanged.
The refresh changes 15,253 pixels. This local baseline is still not a claim of
Acid2 compliance: the original browser comparison retains other font/border
and line-wrapping differences, and the face is not the tested viewport.

## Clear boundary controls

Ten additional inputs cover margin-top -10, 0, 10, 50 and 60px with a 50px
float, both in an ordinary parent and a flow-root. `clear-comparison.json`
records the pre-boundary-fix, Firefox and final rectangles and input hashes.
All ten use exact rectangle and RGB comparisons with repeated captures.
Together with the original 24 margin inputs, all 34 are asserted directly in
the Rust geometry regression.

The large-margin ordinary-parent case originally moved the parent and float
to y=61 and squeezed the cleared child to x=30,width=770. Firefox keeps the
parent/float at y=1 and puts the child at y=51,width=800. Inside flow-root,
Firefox puts that same child at y=61,width=800. Clearance must be determined
from the hypothetical border position before parent/first-child collapse;
restoring the specified margin can then require zero or negative clearance.
A non-moving clear also must query float occupancy at the border, not at the
margin edge. These cases validate both distinctions without changing the input
or applying a pixel/geometry tolerance.

See [CSS clearance](https://www.w3.org/TR/CSS22/visuren.html#flow-control).

## Final local validation

`cargo test --locked -- --include-ignored` with the pinned WPT checkout required
passed 2399 tests, including doctests, with zero failures and zero ignored tests.
`cargo build --locked` also passed. The final shared library was then used to
repeat the 59 primary, two scroll and ten clear cases (71 cases / 284 captures).
The supplementary 12 cases have exact rectangles and RGB in both engines.

## References

- [Grid item sizing](https://www.w3.org/TR/css-grid-1/#grid-item-sizing)
- [Anonymous flex items](https://www.w3.org/TR/css-flexbox-1/#flex-items)
- [Collapsing margins](https://www.w3.org/TR/CSS22/box.html#collapsing-margins)

The whitespace-only flex input intentionally contains a space before a newline.
Its scoped `.gitattributes` entry permits that data byte; the captured HTML and
its hash are preserved, and other whitespace checks remain enabled. The Rust
string counterpart uses explicit `\n`/`\t` escapes for the same input bytes.
