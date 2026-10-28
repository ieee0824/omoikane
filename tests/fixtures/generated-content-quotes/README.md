# Generated quotes and counter-set Firefox comparison

`fixed.html` compares the generated-content behavior added for #1496. It covers
custom nested quote pairs, `no-open-quote` / `no-close-quote` depth changes,
language-sensitive automatic quotes on nested `<q>` elements, and the required
`counter-reset` → `counter-increment` → `counter-set` ordering.

The visible strings in Firefox 158.0.1 are:

- `[outer {inner}]`
- `<depth-only> next`
- `「日本語 『内側』」`
- `Chapter 10: set after increment`

On 2026-10-28, Firefox 158.0.1 headless and the production Omoikane C FFI
rendered this fixture twice at 620×260 and DPR 1. Each browser's repeated PNG
was byte-identical. All four visible generated strings and their row placement
matched on visual inspection. ImageMagick `compare -metric AE` reported 5,680
changed pixels, confined to the engines' different glyph rasterization; pixel
identity is not claimed.

Capture both engines without rewriting the fixture:

```sh
GECKODRIVER=/path/to/geckodriver \
  python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/generated-content-quotes --output <artifact-dir>
OMOIKANE_LIBRARY=<libomoikane.so> \
  python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/generated-content-quotes --output <artifact-dir>
python3 scripts/summarize-firefox-rendering.py \
  tests/fixtures/generated-content-quotes --output <artifact-dir> \
  --destination <artifact-dir>/comparison.json
```

The manifest pins the input hash. Captures and comparison reports remain review
artifacts rather than checked-in image baselines.
