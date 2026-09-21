# CSS shape-outside fixture

`fixed.html` fixes the viewport and Liberation Sans font while exercising a left
circle with `shape-margin`, a right concave polygon, and an inset shape whose
percentages use `border-box`. The painted float mirrors each exclusion shape so
line geometry can be compared visually with Firefox.

Capture each engine twice and summarize the pixel and element geometry:

```sh
GECKODRIVER=/tmp/geckodriver-v0.37.1/geckodriver \
  python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-shape-outside --output <artifact-dir>
OMOIKANE_LIBRARY=<libomoikane.so> \
  python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-shape-outside --output <artifact-dir>
python3 scripts/summarize-firefox-rendering.py \
  tests/fixtures/anonymized-shape-outside --output <artifact-dir> \
  --destination <artifact-dir>/comparison.json
```

The manifest pins the HTML hash. Screenshots, repeat captures, metrics, library
hashes, and comparison JSON are retained as review artifacts rather than
checked-in baselines.

`vertical.html` adds `vertical-rl` and `vertical-lr` containers. It fixes left
floats at the physical top and right floats at the physical bottom, uses RTL
inline flow for the right-float cases, then checks circle, inset, polygon, and
`shape-margin` exclusions along successive columns.
Its cases correspond to the vertical-writing coverage in the pinned WPT
`shape-outside-circle-048/051`, `shape-outside-inset-022/025`, and
`shape-outside-polygon-020/023` series; the local fixture uses text and DOMRect
probes so both geometry and painting can be compared with Firefox.
