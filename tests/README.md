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
and navigation. It preserves logs, JSON results, original screenshots and
losslessly compressed copies in a new artifact directory. The script never
attaches to an existing display. Its CI job runs on Linux x86_64 and aarch64.

```sh
sudo apt-get install --no-install-recommends xvfb xdotool x11-utils openbox libxkbcommon-x11-0 python3-pil fonts-dejavu
cargo build --locked --features gui --bin omoikane
/usr/bin/python3 scripts/test-pointer-lock-x11.py --binary target/debug/omoikane --artifacts .artifacts/gui-pointer-lock
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
WebAssembly compatibility. ArrayBuffer transfer via MessagePort is tracked in
#763; decoder regressions use the engine's detach API independently of it.

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
configurations on Linux x86_64, Linux ARM64 and macOS ARM64.

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
