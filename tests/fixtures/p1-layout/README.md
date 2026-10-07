This fixture exercises Flexbox order, painting and hit-testing, row/column
placement, dense packing, repeated implicit sizes, and implicit tracks before
the explicit grid. It contains no fonts or network resources.

Run `probe.js` as an expression in either browser to obtain relative rectangle
coordinates, the hit-test target, DOM order, and the canonical dense value.
The `p1_layout` integration test checks the same fixture in Omoikane.

Firefox comparison (Python requires Pillow):

```sh
P1_LAYOUT_OUTPUT=.artifacts/p1/fixture \
  cargo test --locked --test p1_layout reference_fixture
python3 scripts/compare-p1-layout-firefox.py \
  --geckodriver /path/to/geckodriver --output .artifacts/p1/firefox \
  --actual .artifacts/p1/fixture/omoikane.original.png
```

The comparison saves the browser version, geometry, original screenshot and
driver log. It does not update a screenshot baseline.

The page explicitly supplies an opaque white background: low-level Omoikane
painting otherwise returns a transparent canvas, while Firefox screenshots
include the browser's default viewport background.

`validation.json` records the pinned WPT revision, source hashes, 13 cases
and 168 passing subtests, and Firefox comparison results. The integration test
also checks intrinsic implicit track widths against Firefox: two 30px inline
blocks produce 30px with `min-content`, 60px with `max-content`, 40px with
`fit-content(40px)`, 30px with `fit-content(20px)` (the minimum still applies),
and 50px with `fit-content(25%)` in a 200px container.

A separate existing placement regression uses a 100px explicit column and
two implicit auto columns with a 5px gap in a 200px container. Firefox measures
the spanning child at `[105, 25, 95, 5]`: normal alignment stretches the auto
columns. Its previous 5px width expectation omitted that distribution.

The placement-phase check fixes one item in column 3, row 1, then requests
row 1 for an automatic-column item. Firefox places that item at `[0, 0, 20,
20]`; fixed positions do not advance the cursor used by the later row-locked
placement phase. The regression covers row/column flow and dense/sparse modes.
