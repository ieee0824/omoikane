# Scroll UI comparison fixture

`fixed.html` covers thin custom-color scrollbars, `scrollbar-gutter: stable
both-edges`, `overflow-clip-margin`, and a DOM insertion above a visible scroll
anchor. The same immutable input is captured in Firefox and Omoikane.

```sh
GECKODRIVER=<geckodriver> python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-scroll-ui --output <artifact-dir>
OMOIKANE_LIBRARY=<libomoikane.so> python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-scroll-ui --output <artifact-dir>
python3 scripts/summarize-firefox-rendering.py \
  tests/fixtures/anonymized-scroll-ui --output <artifact-dir> \
  --destination <artifact-dir>/comparison.json
```

Generated PNGs and reports are review artifacts rather than checked-in pixel
baselines. The fixed WPT revision and selected parsing cases are documented in
the issue/PR validation record.

On 2026-10-30, Firefox 159.0 and this branch were captured twice at 500×260 CSS
pixels. Both engines were repeat-stable and reported identical fixture geometry.
The images differed at 25,478 of 130,000 pixels (19.598%); native scrollbar and
font rasterization are intentionally not claimed to be pixel-identical. Original
PNGs, input hashes, browser capabilities, and the comparison report are retained
under `.artifacts/issue1269/issue1549-firefox-comparison/`.
