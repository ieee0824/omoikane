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
