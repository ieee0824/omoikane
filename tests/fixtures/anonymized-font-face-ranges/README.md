# @font-face range matching against Firefox (#745)

This fixture uses the original CC0 fonts from `anonymized-font-selection`.
Their 1000-unit em and distinct advances make the selected resource observable:
Regular advances 500 units, Bold 700, and Condensed 350.

The five rows verify stretch-before-style/weight matching, a weight range,
an oblique-angle range, disjoint unicode-range composition, and later-source
selection where unicode ranges overlap. `font-sha256.json` records the exact
font inputs. The page publishes the five measured widths as
`globalThis.__fontFaceRangeWidths` after `document.fonts.ready`.

Capture both browsers from the repository root:

```sh
python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-font-face-ranges --output tests/output/font-face-ranges
OMOIKANE_LIBRARY=/absolute/path/to/libomoikane.so \
  python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-font-face-ranges --output tests/output/font-face-ranges
```

The capture output is comparison evidence. It is not a checked-in pixel baseline;
font rasterization can differ while face selection and DOM advances agree.

Reference capture: Linux ARM64, Firefox/geckodriver versions and exact results are
recorded in `comparison.json`. Both browsers produced byte-identical repeat images.
All five DOM widths match exactly: 28, 40, 40, 48, and 24 CSS px. The full-image
difference is 4,519 of 135,200 pixels (3.34%); visual review found the differences
in system-label rasterization and rule color, while the fixed-font sample geometry
and selected advances agree. Original captures remain under
`/workspace/.artifacts/issues745/{firefox-final,omoikane-final}/`.
