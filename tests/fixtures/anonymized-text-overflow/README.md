# Text overflow rendering fixture (#697)

This fixed input covers one-value `text-overflow: ellipsis` and `clip`, clipped
and visible overflow, LTR and RTL inline ends, multiple inline fragments, a box
narrower than the ellipsis marker, and a larger font size. Source text is blue
while the block color is red, so the generated ellipsis can be identified in
paint output independently from the source glyphs.

Capture Firefox and Omoikane twice with `scripts/compare-firefox-rendering.py`
and write output outside this fixture directory. The manifest records the exact
input hash. Full-image differences are descriptive because rasterization can
differ; the Rust regression test checks the marker color and inline-end region.

## Verification

Linux ARM64, Firefox 155.0.1, geckodriver 0.37.1, 500x320 CSS pixels and DPR 1
were used on 2026-09-12. Both browsers produced identical RGB output across
their own two captures, and all six `data-probe` border-box geometries matched
exactly. The red marker bounds were x=171..185 in Firefox and x=175..188 in
Omoikane for LTR, and x=20..34 and x=15..28 respectively for RTL. The 4px box
showed no marker in either browser and clipped the blue first grapheme to
x=13..16, as required when the first grapheme and marker cannot both fit.

The full images differ in 9,514 pixels (5.946%), primarily from known glyph
advance, baseline and rasterization differences tracked in #677. This is a
descriptive measurement rather than a tolerance. Raw captures, repeated images,
input/library hashes, DOM measurements, `comparison.json`, `ink-bounds.json`
and lossless-compression hashes are retained under
`/workspace/.artifacts/issues697/rendering-final/` in the development record.
