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

The initial job is intentionally a small PR smoke gate. Expansion toward the full WPT suite and official `wpt run` integration is tracked in GitHub issue #150.

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
