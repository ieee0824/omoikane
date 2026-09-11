# Table dimensions against Firefox (#672)

These anonymized inputs reproduce the table sizing defects in
[#672](https://github.com/ieee0824/omoikane/issues/672), a child of
[#668](https://github.com/ieee0824/omoikane/issues/668).
`comparison.json` records the measured rectangles relative to each table, the
absolute table-origin difference, input hashes and the comparison environment.
It is a measurement record, not a refreshed screenshot baseline.

## Method

On 2026-09-11, Firefox 155.0.1 (geckodriver 0.37.1) and Omoikane rendered the
same localhost-served HTML on Linux ARM64, at 800 × 600 CSS pixels and DPR 1.
Firefox waited for `document.fonts.ready` and two animation frames. Omoikane
used the production C FFI load/evaluate/screenshot path. Each browser captured
each case twice; within-browser RGB equality was checked independently.
The files use the same installed Liberation Sans font family. This does not
assert identical font selection or rasterization between engines.

The measured elements have `data-probe` attributes. Their rectangles can be
collected with:

```js
Array.from(document.querySelectorAll('[data-probe]'), e => {
  const r = e.getBoundingClientRect();
  return {id: e.id, x: r.x, y: r.y, width: r.width, height: r.height};
});
```

Serve this directory over localhost and use an 800 × 600 viewport to repeat
the measurement. For screenshot comparisons, keep full-page coordinates in
the raw measurements, then crop each image to its own measured table box.
This makes the independent top-margin defect visible in the origin delta
without conflating it with table sizing. The original logs, all captures,
JSON and scripts are retained in the task artifact directory
`/workspace/.artifacts/issue672/` of the development environment.

## Before the fix

At base `468d0f4a605ab602a6f5945e0b0c2ec53bb6c048`, the collapsed fixed table
was 500 × 192 instead of Firefox's 500 × 128; its rows were 64 instead of
42 pixels, and the colspan cell was 520 instead of 498 pixels wide.
With separate borders and spacing 4px 6px, the table was 500 × 208 instead
of 500 × 156. The new geometry tests failed on these original dimensions
before the table implementation changed.

Cell border-box dimensions had been stored as content dimensions, so padding
and borders were counted again. The fixed layout path also used content-driven
column sizing. Column hints exposed a separate parser defect: `colgroup` and
`col` were placed outside the table instead of forming column definitions.
Rowspan redistribution additionally translated decorated cells twice.

## Coverage and interpretation

The 20 cases cover fixed/auto with collapsed/separate borders, column and
first-row hints, later-row width hints, equal columns, colspan hints, rowspan
starting in the first or a later row, minimum cell heights, mixed-case CSS
keywords, and a text-free shared-border painting control.

The fixed collapsed regression asserts the following table-relative boxes:

| Element | x | y | width | height |
| --- | ---: | ---: | ---: | ---: |
| table | 0 | 0 | 500 | 128 |
| h1 | 1 | 1 | 249 | 42 |
| h2 | 250 | 1 | 249 | 42 |
| d1 | 1 | 43 | 249 | 42 |
| d2 | 250 | 43 | 249 | 42 |
| span | 1 | 85 | 498 | 42 |

For separate borders, the table is 500 × 156, cells are 244 × 44, the
colspan cell is 492 × 44, and row origins are 6, 56 and 106 pixels.

The final comparison matched table width and height in **20/20** cases, and
all probed table-relative rectangles in **18/20**. The two auto-layout cases
retain maximum column-distribution differences of 1.4091px (collapsed) and
1.3091px (separate); these figures round the measured maxima upwards. All
80 captures were stable within each browser. The text-free shared-border
control matched Firefox RGB exactly after aligning the table origins.

These fixtures exercise uniform solid borders. They do not establish complete
CSS table border-conflict resolution. Auto layout retains its existing surplus
allocation algorithm; any residual column distribution difference is recorded
as measured, not dismissed as rounding. The independent top-origin difference
is tracked by [#671](https://github.com/ieee0824/omoikane/issues/671), and text
rasterization by [#677](https://github.com/ieee0824/omoikane/issues/677).

## Automated regression checks

```sh
cargo test --locked --lib layout::table
cargo test --locked --lib html::tree_builder
```

The table geometry tests use the checked-in Liberation Sans font, assert exact
known fixed-layout rectangles and content alignment, check auto-table bounds,
and verify shared-border pixels without a whole-image text golden. Parser
tests assert explicit and implicit column-group parentage. Existing offset,
colspan/rowspan and general table layout tests remain enabled.
