# Rendering Fixture And Diff Rules

This document defines the anonymized fixture layout and diff output conventions used by rendering regression tests.

## Fixture Layout (`tests/fixtures`)

- Directory format: `tests/fixtures/<fixture-id>/`
- `<fixture-id>` must be anonymized (for example `anonymized-site-a`).
- Do not include production site names, domains, or URLs in fixture directory names.

Recommended files under each fixture directory:

- `<case>.html`: input fixture HTML
- `<case>.baseline.png`: checked-in baseline image
- Local assets required by the fixture HTML/CSS

## Diff Output (`tests/output`)

Generated PNG artifacts should use this name format:

`<fixture-id>.<scenario>.<variant>.png`

- `<scenario>` examples: `local-baseline`, `official-reference`, `viewport-1366x900`
- `<variant>`: `actual`, `expected`, `diff`

Example set:

- `anonymized-site-a.local-baseline.actual.png`
- `anonymized-site-a.local-baseline.expected.png`
- `anonymized-site-a.local-baseline.diff.png`

## Baseline Refresh Flow

1. Run the fixture baseline refresh test (usually `#[ignore]`). The refresh test only rewrites the checked-in baseline when opted in explicitly (for the Acid2 baseline: `OMOIKANE_REFRESH_BASELINE=1 cargo test refresh_acid2_baseline_png -- --ignored`), so that a plain `--include-ignored` run stays idempotent and never dirties the working tree.
2. Verify the generated baseline PNG visually.
3. Re-run the comparison test and confirm pass.
4. Commit baseline update with a short rationale in PR.

## Sanitization Rules

- Remove or anonymize site-identifying text and URL strings before check-in.
- Keep fixture files minimal and focused on rendering behavior.

## Web Platform Tests smoke subset

The smoke runner executes selected upstream `testharness.js` tests inside Omoikane. The upstream checkout is not vendored; its commit is pinned in `tests/wpt/revision.txt`.

```bash
scripts/fetch-wpt.sh
cargo test --test wpt_smoke -- --nocapture
```

Set `WPT_ROOT` to store the checkout elsewhere. Add cases to `tests/wpt/manifest.json` and add their paths to the sparse-checkout list in `scripts/fetch-wpt.sh`. Expectations may be `PASS`, `FAIL`, or `TIMEOUT`; both regressions and unexpected passes fail the runner so expectation changes stay explicit. Set `WPT_REPORT=path/to/report.json` to write the pinned revision, case outcomes, script errors, and individual subtest results as JSON. Set `WPT_JUNIT=path/to/junit.xml` to emit a JUnit XML testsuite for CI test-report consumers; expectation mismatches are represented as failures and subtest details are XML-escaped in `system-out`.

Set `WPT_RESULTS_DIR=path/to/results` to additionally store revision-scoped reports as
`<revision>/report.json` plus one JSON file per area (`css.json`, `dom.json`,
`shadow-dom.json`, and so on). If that directory already contains an older revision,
set `WPT_COMPARE_REVISION=<old-revision>` while running the new revision to print a
machine-readable diff of known failures, regressions, improvements, and changed cases.
CI automatically writes to `.artifacts/wpt/results` and uploads both these
revision-scoped files and the flat `WPT_REPORT`, matching the
`report.json` convention used by the Web API surface probe.

The print path has separate reftests for the four pinned
`css/css-page/layers-00*-print.html` cases and four
`css/css-page/page-name-and-break-00*-print.html` cases. It renders upstream
HTML and the matching reference as pages, then compares every page pixel:

```bash
WPT_ROOT=target/wpt scripts/fetch-wpt.sh
WPT_ROOT=target/wpt WPT_REQUIRED=1 cargo test --locked --test print_page_wpt
```

`tests/fixtures/print/page-layers.html` also has a two-page Firefox comparison.
With Firefox, geckodriver, Selenium, Pillow, and poppler-utils installed:

```bash
OMOIKANE_PRINT_ARTIFACTS=<output-dir> cargo test --locked --test print_page_layers
python3 scripts/compare-print-page-firefox.py \
  --geckodriver <path> --actual-dir <output-dir> --output-dir <reference-dir>
```

The script saves the Firefox PDF/PNGs and reports page counts, dimensions, and
changed pixels as JSON. It exits nonzero when pages or pixels differ.

`combined-pagination.html` combines a named page size and margin change,
side-requested blank pages, a nested `break-inside: avoid-page` block, wrapped
paragraph text, and a tall table cell. Run
`cargo test --locked --test print_page_combined` with
`OMOIKANE_PRINT_ARTIFACTS=<output-dir>` to save the
pages under `<output-dir>/combined-pagination/`. For the Firefox reference, pass
`--fixture tests/fixtures/print/combined-pagination.html`, that artifact
subdirectory, `--page-width-css-px 100`, and `--page-height-css-px 60` to the
comparison script. It saves the PDF/PNGs before reporting the expected page
count mismatch.

In the Firefox 155.0.1 comparison on 2026-09-28, Omoikane produced 10 pages:
the first was 100×60 pixels, later pages were 140×60, and pages 2 and 9 were
blank. The heading, kept block, and table lines appeared on pages 3, 4, and
6–8 respectively. Firefox's print API returned seven 100×60-pixel pages with
no blank page; the heading, kept block, and table lines appeared on pages 2,
3, and 5–6. Its explicit print sheet size means this reference cannot verify
the named page width change. The first 100×60 page matched exactly (zero
changed pixels); later pages have different dimensions or content placement,
so a sheet-by-sheet pixel match is not expected. The regression test asserts
Omoikane's complete marker order, both blank sheets, and wrapped paragraph
content without discarding the Firefox discrepancy.

For the page-margin text fixture, set `OMOIKANE_PRINT_ARTIFACTS=<output-dir>`
and run `cargo test --locked --test print_page_margin_content`. Pass
`--fixture tests/fixtures/print/page-margin-text.html` and
`--page-width-css-px 200 --page-height-css-px 200` to the comparison script.
Firefox 155 currently prints the document body but omits the margin-at-rule
content; keep that unsupported-reference result separate from Omoikane's
regression checks. The image-content fixture is `page-margin-images.html`.
The same test target checks its data and relative URL images alongside text;
its optional PNG artifact is written under `<output-dir>/images/`. Pass that
directory to the comparison script for the image fixture. Chromium prints the
markers in the same order as Omoikane.

`page-margin-paint.html` exercises page background and border layers, a
background image, and a margin-box border. Its Omoikane PNG is written under
`<output-dir>/paint/`; use that directory with the comparison script. Firefox
155 prints only the document canvas for this fixture, so its difference image
is a record of unsupported page and margin-box painting rather than a passing
pixel reference. Chromium prints the page background, border, and margin-box
paint; the fixed pixel assertions also cover clockwise and `z-index` ordering.

`page-orientation-left.html` and `page-orientation-right.html` exercise rotation
after page layout. Run `print_page_layers` with `OMOIKANE_PRINT_ARTIFACTS` to
write each two-page result under `orientation-left/` or `orientation-right/`.
For Firefox comparison, pass the matching fixture, artifact subdirectory,
`--page-width-css-px 200`, and `--page-height-css-px 300` to the script above.
The test also checks that changing `size` to 300×200 leaves the contents upright.
Common page rendering, color bounds, and optional PNG export live in
`tests/common/print.rs`; the fixture-specific pixel assertions remain in their
respective integration tests.

For a known `FAIL` that affects only specific subtests, set
`known_failure.failed_subtests` to their exact names. The runner then rejects any
additional failure, missing expected failure, or timeout instead of accepting all
failures in that file.

The ElementInternals subset exercises attachment conditions, form ownership,
submission values, validation, and disabled/reset reactions. `element_internals`
also checks restoration snapshots and callbacks through navigation. The
`form_validity_css` target compares native/custom control validity, form and
fieldset aggregation, detached/shadow trees, computed styles, and painted
pixels in `anonymized-form-validity/states.html`. Set
`OMOIKANE_FORM_VALIDITY_IMAGES=<artifact-directory>` to save the initial,
changed, and disabled frames for visual review; it never updates a baseline.
The fixture uses normal flow to isolate validity styling. Nested fixed-position
translation observed during review is covered by the `fixed_position` target
(#773). Its `anonymized-nested-fixed/states.html` fixture preserves the original
absolute form / fixed fieldset / nested fixed controls. It checks the three
validity states at viewport 320x180; set `OMOIKANE_FIXED_IMAGES=<artifact-directory>`
to save the frames without updating a baseline. Pure layout tests also cover
transformed/perspective/contained ancestors, percentage and logical insets,
auto-height containing blocks, and flex/grid/table/inline/float placement.
Paint tests cover scroll offsets, CSSOM client rectangles, and native hit testing.

The initial job is intentionally a small PR smoke gate. Expansion toward the full WPT suite and official `wpt run` integration is tracked in GitHub issue #150.

The Fullscreen subset checks `fullscreenEnabled`, rejection and
`fullscreenerror` without user activation, removal of legacy prefixed APIs,
and `:fullscreen` selector support. Interactive entry, host acceptance,
iframe policy, exit paths, and top-layer rendering are covered by the native
Fullscreen regression tests and the fixed
`anonymized-fullscreen-top-layer` fixture.

The Pointer Lock subset checks `MouseEvent.movementX/Y` construction and
`NotAllowedError` without user activation. `cargo test --locked --test pointer_lock`
additionally exercises the host acquisition handshake, refusal, queued requests,
relative input, pointer capture, sandbox inheritance, Shadow DOM, removal,
Escape, focus loss, and navigation. The `input.pointer-lock` surface probe checks
the acquisition/release events and Promise result through the headless host.
The input regressions use same-origin HTTP documents with child-realm listeners
to check cross-frame ordering, cancellation, text/IME input, and nested focus.
Child scripts are inserted dynamically; initial HTML iframe inline scripts are
tracked separately in [#766](https://github.com/ieee0824/omoikane/issues/766).
The Rust tests emulate presentation-host responses. The actual Linux GUI is also
tested in a dedicated Xvfb/Openbox display by `scripts/test-pointer-lock-x11.py`.
It sends XTEST input, checks exact relative deltas (including identical successive
events), frozen page coordinates, cursor confinement/hiding/restoration, button
and wheel delivery, explicit exit, Escape with `preventDefault()`, focus loss,
and navigation. The focus regressions queue focus restoration and a single real
L press while the test's browser process is stopped, then resume it to exercise
winit's synthetic press followed by the queued real press. Only one DOM keydown
and one lock acquisition are allowed. A second case holds L in the other window
while restoring focus; the synthetic press must not acquire Pointer Lock.
Auto-repeat is disabled only on the test's private X server for these cases.
`OMOIKANE_TRACE_INPUT=1` enables ordered native focus/key diagnostics on stderr;
the harness saves them as `input-events.json` alongside the DOM event history.
GUI bridge unit tests also check that synthetic events cannot edit text or grant
activation, while real keydown, repeat, keyup and focus-loss cleanup still work.
It preserves logs, JSON results, original screenshots and
losslessly compressed copies in a new artifact directory. The script never
attaches to an existing display. Its CI job runs on Linux x86_64 and aarch64.

```sh
sudo apt-get install --no-install-recommends xvfb xdotool x11-utils openbox libxkbcommon-x11-0 python3-pil fonts-dejavu
cargo build --locked --features gui --bin omoikane
/usr/bin/python3 scripts/test-pointer-lock-x11.py --binary target/debug/omoikane --artifacts .artifacts/gui-pointer-lock
```

`scripts/gui_x11.py` provides the shared `GuiSession` context manager. It
allocates its own DISPLAY using Xvfb's `-displayfd`, waits for Openbox readiness,
finds visible application windows by PID, and provides bounded xdotool commands
and lossless screenshots. Callers supply their own fixtures and assertions.
`launch(binary, arguments)` records the revision, executable SHA-256/size,
arguments, environment overrides and desktop commands in `inputs.json`.
`GuiSession(artifacts, environment={...})` accepts GUI environment overrides,
including `WINIT_X11_SCALE_FACTOR`, but always selects its private X11 display.
Each artifact directory must be new. Exceptions retain logs, JSON failure details
and, when the desktop is available, original/compressed failure screenshots.
Cleanup terminates only processes started by that session.

The startup-URL smoke scenario checks an independently positioned colored DOM
target's physical dimensions and native click coordinates at scale factors 1
and 2. It also verifies the CSS viewport dimensions. Run both scenarios and the
failure-path tests with the same GUI binary:

```sh
/usr/bin/python3 -m unittest discover -s scripts/tests -p test_gui_x11.py
/usr/bin/python3 scripts/test-gui-x11.py --binary target/debug/omoikane --artifacts .artifacts/gui-scale
```

The URL-entry acceptance scenario (#1131) starts the GUI on the
[`gui-navigation`](fixtures/gui-navigation/README.md) start page, clicks the
visible URL field, presses Ctrl+A, types the destination URL with XTEST and
presses Enter. It passes only when the fixture server logged the destination
request and the page area shows the destination colour; it does not use CDP,
in-page JavaScript or the window title. Whether the build has URL entry is
declared (`URL_ENTRY_UI` or `--url-entry`), so the result separates `PASS` (0),
`REGRESSION` (1), `NOT_IMPLEMENTED` (2), `ENV_ERROR` (3, desktop, launch or
start page never ready) and `UNKNOWN` (4, declared unimplemented but passed).
`--negative stale-page` serves the start page at the destination URL; CI
requires it to fail as `REGRESSION`. Each run saves `outcome.json`,
`requests.json`, logs and original/compressed screenshots.

```sh
/usr/bin/python3 -m unittest discover -s scripts/tests -p test_gui_url_entry.py
/usr/bin/python3 scripts/test-gui-url-entry.py --binary target/debug/omoikane --artifacts .artifacts/gui-url-entry
# Baseline before URL entry (eec3e01c): expected NOT_IMPLEMENTED, exit 2
/usr/bin/python3 scripts/test-gui-url-entry.py --binary <eec3e01c build> --artifacts .artifacts/gui-url-entry-baseline --url-entry unimplemented
```

The scenario covers scale factor 1 on Linux X11 only. Loading blocks the event
loop until the page is fetched (#1139); asynchronous loading, macOS windows and
external sites are not covered.

The render-demand scenario (#1217) uses the same private desktop and synthetic
fixtures to verify three idle seconds with zero page paints, toolbar-only
composition, 500 ms and 1 second timers after idle, and rAF, CSS transitions/keyframes,
GIF playback and smooth scrolling. Each animation must move and then stop
painting after completion or cancellation. `OMOIKANE_TRACE_PAINT` records only
actual page paints; an uninstrumented binary cannot pass the idle assertion.
Initial page colors differ, so an old frame cannot satisfy navigation readiness.

```sh
/usr/bin/python3 scripts/test-gui-render-demand.py --verify --binary target/debug/omoikane --artifacts .artifacts/gui-render-demand
# Separate CPU measurement; CPU percentages are evidence, not a CI threshold.
/usr/bin/python3 scripts/test-gui-render-demand.py --binary target/debug/omoikane --artifacts .artifacts/gui-idle-cpu --seconds 10
```

The long-page scroll scenario (#1225) records ten warmed samples of native wheel
dispatch, adjusted layout, paint and presentation on a 300-row fixture. It checks
every wheel delivery, scroll position (including `scrollTop`), continued painting
under a burst of input, and the final pixel color and click target. Run each build
at both device scales; the JSON report and original/losslessly compressed PNGs
are retained in the artifact directory. `--baseline` records the old stalled
burst for comparison and does not claim the continuous-paint check passed.
The [2026-10-07 measurements](../docs/performance/gui-scroll-2026-10-07.json)
retain all samples for debug/release at scales 1/2, along with the method and limits.

```sh
python3 scripts/test-gui-scroll.py --binary target/debug/omoikane --scale 1 --artifacts .artifacts/gui-scroll-debug-1
python3 scripts/test-gui-scroll.py --binary target/debug/omoikane --scale 2 --artifacts .artifacts/gui-scroll-debug-2
```

Xvfb validates the X11 path with synthetic device input; physical devices,
Wayland and macOS still need desktop validation. `cargo check --locked --features
gui` only checks compilation. `unadjustedMovement: true` is currently rejected
with `NotSupportedError` by the built-in hosts. The Linux GUI directly uses the
already-resolved `x11-dl` crate for an exclusive grab: Winit's X11 confinement
uses `owner_events=true`, which duplicates raw events during pointer lock.

The encoding-stream subset covers chunk boundaries, BOM handling, encoding labels,
fatal errors, BufferSource conversion and backpressure. These `.any.js` cases run
in the smoke runner's document realm; `src/js/text_stream_tests.rs` additionally
checks a Worker round trip, cancellation/abort, readonly attributes, shared buffers
and large chunks. The `network.text-streams` surface probe verifies a split
surrogate pair through an encoder/decoder pipeline.
For `/common/sab.js`, this smoke runner uses the exposed native
`SharedArrayBuffer` constructor directly: upstream discovers it through
`WebAssembly.Memory`, which Omoikane does not implement. The test inputs remain
real shared buffers and all assertions are retained. This adapter does not test
WebAssembly compatibility. The transferred-input subtests exercise actual
ArrayBuffer detachment through MessagePort; focused engine tests also cover
structuredClone and Worker transfer ordering, aliases and resizable views.

The form-associated custom-element subset submits GET and multipart POST
requests into named iframes. The runner gives each document its actual local
HTTP URL and implements the pinned `echo-content-escaped.py` endpoint in Rust,
including request-body byte escaping. This exercises the engine's real form
navigation and text response handling; it does not substitute successful
JavaScript results or change upstream assertions.

# Web API surface probe

主要なWeb APIの存在・型・基本挙動は、manifest駆動のintegration testで継続計測します。

```bash
cargo test --test web_api_surface -- --nocapture
```

manifestは [`web_api_surface/manifest.json`](web_api_surface/manifest.json) にあります。
各probeの`baseline_supported`が`true`の機能は非退行対象です。`false`の機能は出力上
`unsupported`として集計され、後から実装されてprobeが通ると`improvements`に表示されます。

machine-readable JSON reportが必要な場合は出力先を指定します。

```bash
OMOIKANE_WEB_API_REPORT=target/web-api-surface.json \
  cargo test --test web_api_surface -- --nocapture
```

## Browser operation journeys

`browser_journeys` starts a local HTTP fixture server and drives the public
`PlatformBrowser`/CDP APIs used by native frontends. It covers redirect and resource
loading, keyboard/mouse input, fetch and animation-frame DOM updates, form POST and
history, tab storage isolation, Worker cloning and child Realm/event-loop behavior.
External stylesheet ordering, imports, media changes, CSP and resource reuse are
checked against both CSSOM geometry and painted pixels.

```bash
OMOIKANE_BROWSER_REPORT_DIR=.artifacts/browser/journeys \
  cargo test --test browser_journeys --test system_font_selection -- --nocapture --test-threads=1
OMOIKANE_JIT_GATE_REPORT_DIR=.artifacts/browser/acid3 \
  cargo test --test acid3_harness -- --nocapture --test-threads=1
```

Add `--features baseline-jit` before `--` to repeat these contracts with that
feature enabled. Acid3 requires 100/100 and no script/drive errors in both drive
modes, including the default interpreter build. `browser-behavior.yml` runs both
configurations on Linux ARM64 and macOS ARM64, and `baseline-jit` on Linux x86_64.
The Linux x86_64 interpreter configuration runs inside CI's full `test` job, which
uploads the same `browser-x86_64-unknown-linux-gnu-interpreter` artifact.

The operation report directory contains one JSON per successful journey and PNGs
named `anonymized-browser-journey.<scenario>.actual.png`. Assertions use specified
geometry, DOM values and stable interior pixels; system-dependent text rasterization
is not compared to a whole-image golden. Typed input must visibly change the input's
interior. Review actual images as well: this found missing input text after the
original DOM assertions already passed. A failed assertion remains a failed test;
absence of a successful JSON report is not success.

The `fonts/` subdirectory records each journey's actual primary font selections
in layout and paint: the requested CSS family list, weight and style, plus the
selected system file, collection face index and OpenType metadata. The inventory
report includes all discovered faces and hashes of the files selected for the
three generic families in normal/bold and upright/italic styles. Input CSS is in
the named fixture and test source at the revision recorded by the workflow.
Font diagnostics are opt-in and scoped to the thread executing the journey.

`cargo test --locked --test legacy_encoding` uses the local
[Japanese encoding fixture](fixtures/legacy-encoding/README.md) to check
Shift_JIS meta decoding in frames, iframes, objects and direct navigation,
HTTP charset precedence, and frame-link navigation. Body/title values and full
painted frames must match UTF-8 references. Japanese fallback glyphs must be
available and nonempty: install `fonts-noto-cjk` on Linux; macOS uses its installed
Hiragino fonts. CI and Browser behavior run the same tests and save PNGs and font
metadata under `OMOIKANE_BROWSER_REPORT_DIR/legacy-encoding`. These tests exercise
the GUI's shared browser/painting code and title metadata, not OS window chrome.

The workflow also runs `cargo test --lib font -- --include-ignored --nocapture`.
The original fonts in [anonymized-font-selection](fixtures/anonymized-font-selection/README.md)
verify metadata-based selection independently of filenames and directory order,
TTC face indices through shaping and rasterization, and CJK/combining fallback.
Their layout/paint test emits four `anonymized-font-selection.*.actual.png` files
when `OMOIKANE_BROWSER_REPORT_DIR` is set. These controlled faces have distinct
advances and outlines; their geometric assertions do not depend on installed fonts.

These fixed journeys exercise selected browser behavior, not arbitrary website
compatibility or native window creation. They accompany the full suite, WPT and Web
API surface tests. Separate browser instances have separate in-memory storage;
persistence across process restarts and cross-tab storage events are not asserted
by these cases. See [the Gate 6 browser baseline](../docs/jit/gate6-browser-baseline.md)
for the defects, source baseline and verification order.


## Underline position and offset

`underline_fixed_font_matrix_matches_firefox_pixels` compares all pixels of the
90-case `anonymized-underline-positions/fixed.html` fixture to Firefox references
for both underlines and overlines (180 cases).
It covers horizontal and both vertical writing modes, underline position,
positive/negative/percentage offsets, and a shared bundled font. Related paint
tests cover calc offsets, overline side swapping, and decoration propagation
across descendants with different font sizes. Set `OMOIKANE_BROWSER_REPORT_DIR`
to retain generated actual/diff images without changing the checked-in reference.
Reference provenance and refresh rules are in the fixture's README.
