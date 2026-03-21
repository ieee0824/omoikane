# Fixture Layout Rules

This directory stores rendering fixtures used for screenshot-diff regression tests.

## Naming

- Directory: `tests/fixtures/<fixture-id>/`
- `<fixture-id>` must be anonymized (for example `anonymized-site-a`).
- Do not include production site names, domains, or URLs in directory names.

## Recommended Files

- `<case>.html`: input HTML fixture
- `<case>.baseline.png`: checked-in baseline image
- Optional local assets referenced by fixture HTML/CSS

## Baseline Refresh Flow

1. Run an ignored refresh test for the target fixture.
2. Confirm generated PNG visually.
3. Re-run the comparison test and ensure it passes.
4. Commit baseline updates with a short reason in the PR.

## Privacy / Sanitization

- Remove or anonymize site-identifying text and URLs before check-in.
- Keep fixture content minimal and focused on rendering behavior.
