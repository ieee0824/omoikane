# CSS Color 4 regression fixtures

`values.json` covers the six functions, missing components, percentages, hue
units, alpha, predefined spaces and invalid grammars. `firefox-reference.json`
records actual Firefox 157 CSSOM, computed style and sRGB Canvas observations,
with browser capabilities and the input SHA-256. Regenerate observations without
modifying the fixture:

```sh
GECKODRIVER=/path/to/geckodriver python3 scripts/compare-css-color4-firefox.py \
  --output .artifacts/css-color4/firefox
cargo test --locked --test css_color4
```

Firefox 157 replaces missing HWB components with zero. The pinned WPT and CSS
Color 4 preserve `none`, so the regression test explicitly uses that specified
behavior for the missing-HWB case while retaining the unmodified observation.
Firefox's Canvas observations also use clipping for some out-of-gamut colors;
they are not an oracle for CSS local-MINDE gamut mapping. Rec.2020 conversion
uses the current CSS Color 4 BT.1886 gamma 2.4 transfer function, which also
differs from Firefox 157's recorded Canvas observation. CSSOM comparison
preserves the original color space; conversion is verified independently.

`gamut-reference.json` contains 22 independently generated ColorAide 8.13
conversion/local-MINDE vectors, including every predefined space, D50/D65,
nonneutral Lab/Oklab, out-of-gamut primaries and lightness limits. The generator
uses the current CSS Color 4 ProPhoto matrix (ColorAide's original matrix is
older), fits directly in sRGB, and bounds the final channels to [0, 1]. The
sRGB identity half-byte boundary has an explicit quantization note; floating
point values are compared separately from bytes. To reproduce in a Python
environment with `coloraide==8.13` installed:

```sh
python3 scripts/generate-css-color4-reference.py \
  --output .artifacts/css-color4/gamut-reference.json
cargo test --locked --lib paint::color4
```

`manifest.json` selects `render.html` and `render-colors.html`: all six functions
across background, border, gradient, box shadow and SVG fill/stroke. The
integration test additionally covers text, every independent gamut vector and
six alpha cases. Its text uses the committed CC0 A/B font fixture, verifies web
font selection and visible glyphs, and explicitly sizes the opaque background
to cover the canvas. Render through Firefox WebDriver and the production C FFI:

```sh
GECKODRIVER=/path/to/geckodriver python3 scripts/compare-firefox-rendering.py \
  firefox --fixtures tests/fixtures/css-color4 --output .artifacts/css-color4/render
OMOIKANE_LIBRARY="$CARGO_TARGET_DIR/debug/libomoikane.so" \
  python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/css-color4 --output .artifacts/css-color4/render
```

Both captures run twice and retain the original PNGs, dimensions, geometry,
input hashes and library hash. The checked-in `*.firefox-reference.expected.png`
images are losslessly compressed copies of the inspected Firefox captures.
Both 500×210 fixtures match Firefox exactly in every color region. Their only
image difference is 24 SVG stroke-corner pixels (four per rectangle), also
reproduced with equivalent named colors; this is existing SVG geometry behavior.
Explicit SVG size/viewBox attributes keep the fixture focused on color rather
than CSS-only SVG intrinsic sizing.

The regular WPT smoke manifest and fetch script include nine Color 4 parsing
files (`color-{valid,invalid,computed}-{hwb,lab,color-function}.html`) at revision
`dc97e7bed3096ac9e0e591ab5fa22e7fb8844ead`. Before implementation, the same nine
files had 148 passing / 1,172 failing subtests. The completed implementation has
1,320 passing / zero failing subtests. The PR records final run results.

```sh
scripts/fetch-wpt.sh
WPT_REQUIRED=1 cargo test --locked --test wpt_smoke -- --nocapture
```
