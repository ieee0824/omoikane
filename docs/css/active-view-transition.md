# Active view transition selector state

Omoikane accepts `:active-view-transition` in stylesheet, DOM selector, and
`CSS.supports(selector())` parsing. It matches only a document element whose
node document currently owns an active same-document view transition.

The state is owned by `DocumentMetadata`; selector matching does not call
author JavaScript. `Document.startViewTransition()` activates it before
returning, and completion or `skipTransition()` clears it. Both changes
invalidate the affected document's style cache. The API exposes
`Document.activeViewTransition`, `ViewTransition.updateCallbackDone`, `ready`,
`finished`, and `skipTransition()`. A skipped transition rejects `ready` with
`AbortError` while still running the update callback and settling `finished`.

This slice intentionally does not claim snapshot capture, transition
pseudo-element painting, typed transitions, element-scoped transitions, or
cross-document transitions. Those remain part of the broader View Transitions
API work in issue #1283.

## Verification

- `tests/css_active_view_transition.rs` checks parsing, root-only matching,
  computed-style invalidation, lifecycle identity, promises, and explicit skip.
- Pinned WPT revision `dff7cf45938f6e16427f5607bac82a803534484b`:
  `document-active-view-transition.html` passes. The two accompanying
  `active-view-transition-*.html` files are visual reftests, not testharness
  cases, and are retained separately for renderer comparison.
- Firefox 158.0.1 with geckodriver 0.37.1 matches all nine observations for
  initial, active, and explicitly skipped states. The raw result is stored in
  `.artifacts/issue1269/firefox-active-view-transition-comparison.json`.

Specification: CSS View Transitions Module Level 2, “Selecting based on the
active view transition”, and Level 1 `Document.activeViewTransition` lifecycle.
