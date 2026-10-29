# Line and inline metrics fixture

`fixed.html` compares standard line clamping with a custom marker,
`text-align-last` and inter-word justification, sunk initial letters, hanging
punctuation, text-box trimming, and their vertical-writing interaction. It uses
fixed dimensions and the repository's Liberation Sans face so Firefox and
Omoikane can be captured under the same viewport.

Capture both engines without rewriting the fixture:

```sh
GECKODRIVER=<geckodriver> python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-line-metrics --output <artifact-dir>
OMOIKANE_LIBRARY=<libomoikane.so> python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-line-metrics --output <artifact-dir>
python3 scripts/summarize-firefox-rendering.py \
  tests/fixtures/anonymized-line-metrics --output <artifact-dir> \
  --destination <artifact-dir>/comparison.json
```

The manifest pins the fixture hash. Captures, repeats, DOM geometry, binary
provenance, and the comparison report are review artifacts rather than
checked-in pixel baselines.

On 2026-10-29, Firefox 159.0 with geckodriver 0.37.1 and the rebased issue
branch Omoikane library were captured twice at 720×760 CSS pixels. Both
engines were pixel-stable across repeats. The initial-letter and vertical
cases visibly exercise the requested metrics; text-box height differs by
0.22px and the later rows share a 5px vertical offset. Firefox did not apply
the standard `line-clamp` declaration in this fixture (96px content height),
while Omoikane produced the requested two 24px lines and custom marker (48px),
so the recorded maximum geometry delta is 48px. Overall, 68,725 pixels
(12.559%) differ. The report, DOM measurements, hashes, and original captures
are retained under
`.artifacts/issue1269/issue1492-firefox-comparison/`; this comparison records
the current cross-engine result and does not claim pixel parity.
