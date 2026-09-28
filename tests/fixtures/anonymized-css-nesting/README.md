# CSS Nesting Firefox comparison fixture

`nesting.html` uses nested style rules, an explicit `&` rule, declarations
after a nested rule, and a nested `@media` rule. The tile and its child use
solid colors and fixed geometry, so font and antialiasing differences do not
affect the comparison. The manifest pins the HTML hash and a 500×200 viewport.

Capture both browsers with `scripts/compare-firefox-rendering.py`, using this
directory as `--fixtures`, then compare them with
`scripts/summarize-firefox-rendering.py`. The Firefox capture requires
`GECKODRIVER`; the Omoikane capture requires `OMOIKANE_LIBRARY` pointing to
the built `libomoikane.so`.

At Firefox 155.0.1 and the #838 development build, the two 500×200 PNGs had
the same decoded pixel hash and zero changed pixels. Both captures were stable
across two runs. The tile and child geometry matched exactly (tile 24,24 at
128×128; child 48,48 at 80×80). The raw captures, input hashes, and comparison
report are retained as work artifacts; this directory keeps the reproducible
fixture rather than a generated baseline.
