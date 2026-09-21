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
