# CSS generated counters fixture

`fixed.html` exercises sibling increments, nested `counters()`, `display:none`,
`visibility:hidden`, and DOM insertion followed by full counter recomputation.
It uses the repository's fixed Liberation Sans file so Firefox and Omoikane shape
the same glyphs.

Adjacent items use inline padding because non-replaced inline margins are tracked
separately by #802. The hidden item also sets a transparent color because #801
tracks the existing inline-fragment paint bug where `visibility:hidden` text is
otherwise painted. The counter assertions remain observable: the hidden item
advances the value from `[05]` to `[07]`, while the `display:none` item does not.

Capture both engines without rewriting the fixture:

```sh
GECKODRIVER=/tmp/geckodriver-v0.37.1/geckodriver \
  python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-css-counters --output <artifact-dir>
OMOIKANE_LIBRARY=<libomoikane.so> \
  python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-css-counters --output <artifact-dir>
python3 scripts/summarize-firefox-rendering.py \
  tests/fixtures/anonymized-css-counters --output <artifact-dir> \
  --destination <artifact-dir>/comparison.json
```

The manifest pins the fixture hash. Captures, repeat captures, metrics, inputs,
and comparison reports are review artifacts; they are not checked-in baselines.
