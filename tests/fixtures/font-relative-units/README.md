# Font-relative CSS units against Firefox (#841)

`units.html` is a comparison fixture for `ex`, `ch`, `cap`, `ic`, `lh`, and
`rlh`. The nested element also checks that `font-size: 2lh` and
`line-height: 1.5lh` use the parent's line height, while its child width uses
the nested element's line height. `data-probe` records each rectangle through
`scripts/compare-firefox-rendering.py`.

Capture at 500×320 CSS pixels and DPR 1 on a host with Liberation Mono
installed so both browsers can select the same named font. Generated PNGs and
metrics belong in an external artifact directory, not in this fixture.

Firefox 156.0.1 with geckodriver 0.37.1 and Omoikane were captured twice on
Linux x86-64 on 2026-09-27. The probe widths in CSS pixels were:

| Probe | Firefox | Omoikane |
| --- | ---: | ---: |
| `10ex` | 105.667 | 105.664 |
| `10ch` | 120.000 | 120.020 |
| `10cap` | 131.667 | 131.738 |
| `10ic` | 200 | 200 |
| `3lh` | 90 | 90 |
| `3rlh` | 72 | 72 |
| Nested `2lh` | 90 | 90 |

All probe positions and heights match. The largest width difference is 0.072
CSS pixels; the decoded screenshots differ at 12 of 160,000 pixels. Each
browser's repeated capture is pixel-identical to its first capture. Original
PNGs, numeric metrics, and browser inputs are retained in
`/tmp/issue841-firefox-liberation/`; the PNG sequence is also registered under
Visual Store run `issue841-font-relative-liberation` in
`/tmp/omoikane-geckodriver.3ogxZv/.visual-store/` with source bytes retained.

The pinned WPT revision `dc97e7bed3096ac9e0e591ab5fa22e7fb8844ead`
passed `ch-recalc-on-font-load.html`, `cap-invalidation.html`, and
`rlh-invalidation.html`. The full manifest run had 298 passes, one existing
known failure, and zero regressions across 299 cases. Its log is retained at
`/tmp/issue841-wpt.log`.
