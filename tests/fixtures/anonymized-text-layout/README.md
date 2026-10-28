# Text layout and decoration fixture

`fixed.html` exercises every value of `text-decoration-style`, `text-decoration-skip-ink`
with and without skipping, `word-spacing`, and first-line `text-indent`. It includes
multiple inline fragments, anonymous block boxes, and `vertical-rl` text. The fixture
uses fixed dimensions and the repository's Liberation Sans face so Firefox and
Omoikane can be captured under the same viewport.

Capture both engines without rewriting the fixture:

```sh
GECKODRIVER=<geckodriver> python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-text-layout --output <artifact-dir>
OMOIKANE_LIBRARY=<libomoikane.so> python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-text-layout --output <artifact-dir>
python3 scripts/summarize-firefox-rendering.py \
  tests/fixtures/anonymized-text-layout --output <artifact-dir> \
  --destination <artifact-dir>/comparison.json
```

The manifest pins the fixture hash. Captures, repeat captures, metrics, input
provenance, and the comparison report are review artifacts rather than checked-in
pixel baselines.

On 2026-10-28, Firefox 158.0.1 with geckodriver 0.37.1 and the issue branch
Omoikane library were captured twice at 640×440 CSS pixels. Both engines were
pixel-stable across repeats. All six probed boxes had identical geometry
(`max_geometry_delta: 0`); 28,250 pixels (10.032%) differed, primarily because
the engines rasterize glyphs and decoration strokes differently. The report and
source captures are retained under
`.artifacts/issue1269/issue1495-firefox-comparison/`.
