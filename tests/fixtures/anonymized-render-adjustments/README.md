# Render adjustment comparison fixture

`fixed.html` compares ordinary, direct, and nested CSS `zoom` geometry while
also retaining `print-color-adjust` and `will-change` declarations in one
stable fixture. Print economy/exact behavior and forced colors are asserted by
pixel-level Rust tests because this comparison captures the normal screen
medium.

Capture Firefox and Omoikane with `scripts/compare-firefox-rendering.py`, then
summarize with `scripts/summarize-firefox-rendering.py`. Generated PNGs and
reports remain review artifacts rather than checked-in baselines.

The fixed WPT revision is `dff7cf45938f6e16427f5607bac82a803534484b`.
The selected zoom, print/forced color adjustment, and will-change parsing and
computed-value cases live in `tests/wpt/css-render-adjustments.json`.

On 2026-10-31, all 10 selected WPT cases passed. Firefox 159.0 and Omoikane
were captured twice at 500×300 CSS pixels; both were repeat-stable, all six
probed boxes had identical geometry, and 6,723 of 150,000 pixels differed
(4.482%) because of font rasterization. Original PNGs, hashes, metrics, and the
summary are retained under
`.artifacts/issue1269/issue1553-firefox-comparison/`.
