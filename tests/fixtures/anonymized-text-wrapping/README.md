# Text wrapping and whitespace fixture

`fixed.html` compares preserved and breakable spaces, balanced/pretty/stable
wrapping, strict Japanese line breaking, manual and automatic hyphenation with
custom hyphen characters, and numeric tab stops. It uses fixed dimensions and
the repository's Liberation Sans face so Firefox and Omoikane can be captured
under the same viewport.

Capture both engines without rewriting the fixture:

```sh
GECKODRIVER=<geckodriver> python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-text-wrapping --output <artifact-dir>
OMOIKANE_LIBRARY=<libomoikane.so> python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-text-wrapping --output <artifact-dir>
python3 scripts/summarize-firefox-rendering.py \
  tests/fixtures/anonymized-text-wrapping --output <artifact-dir> \
  --destination <artifact-dir>/comparison.json
```

The manifest pins the fixture hash. Captures, repeat captures, metrics, input
provenance, and the comparison report are review artifacts rather than checked-in
pixel baselines.

On 2026-10-29, Firefox 159.0 with geckodriver 0.37.1 and the issue branch
Omoikane library were captured twice at 640×720 CSS pixels. Both engines were
pixel-stable across repeats. Three of the nine probed boxes had identical
geometry; the other six differed by at most 1px (`max_geometry_delta: 1`).
38,610 pixels (8.379%) differed, primarily because the engines rasterize glyphs
differently. The report and source captures are retained under
`.artifacts/issue1269/issue1491-firefox-comparison/`.
