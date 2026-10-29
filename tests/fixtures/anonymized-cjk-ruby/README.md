# CJK, vertical writing, and ruby Firefox comparison

This fixed fixture compares horizontal and vertical ruby annotation placement,
`ruby-align`, `ruby-position`, upright and combined vertical text,
`text-autospace`, contextual `text-spacing-trim`, and horizontal/vertical
emphasis marks. Both engines load the repository's pinned Noto CJK subset so
the comparison uses identical font bytes.

Capture both engines without rewriting the fixture:

```sh
GECKODRIVER=<geckodriver> python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-cjk-ruby --serve-root tests/fixtures \
  --output <artifact-dir>
OMOIKANE_LIBRARY=<libomoikane.so> python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-cjk-ruby --serve-root tests/fixtures \
  --output <artifact-dir>
python3 scripts/summarize-firefox-rendering.py \
  tests/fixtures/anonymized-cjk-ruby --output <artifact-dir> \
  --destination <artifact-dir>/comparison.json
```

The manifest pins the fixture hash. Captures, repeats, DOM geometry, binary
provenance, and the comparison report are retained under
`.artifacts/issue1269/issue1494-firefox-comparison/` rather than checked-in as
pixel baselines.

The 2026-10-29 capture was repeat-stable in both engines. It reported 18,860
changed pixels (3.638%), 12,500 strong-difference pixels, and a maximum DOM
geometry delta of 8.2px. The sole reported geometry difference was the height
of the horizontal ruby sample (Firefox 53px, Omoikane 61.2px); all probe
origins and widths, including vertical writing and emphasis samples, matched.
