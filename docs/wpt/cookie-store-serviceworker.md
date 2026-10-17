# Cookie Store Service Worker WPT

Issue: [#1354](https://github.com/ieee0824/omoikane/issues/1354), parent
[#1298](https://github.com/ieee0824/omoikane/issues/1298).

The runner serves the pinned upstream `ServiceWorkersHandler` HTML and
`ClassicWorkerHandler` JavaScript wrappers. They register a real Service Worker,
import the original testharness and META scripts, and forward results using
`fetch_tests_from_worker`. The original tests, helpers, assertions, and timeouts
are unchanged. `scripts/fetch-wpt.sh` includes the wrapper dependencies.

At revision `dc97e7bed3096ac9e0e591ab5fa22e7fb8844ead`, the 25 Cookie Store
`.any.js` files declaring `serviceworker` and two original HTML cases produce
**24 PASS / 3 FAIL** after the update, partitioned storage, and subscription
history fixes (2026-10-17). Every harness completes with status 0; there are no
script errors. This is the result for these 27 Service Worker cases, not the
entire upstream Cookie Store directory.

The default CI manifest runs all 27 cases. Three known-failure entries name the
exact failing subtests, so an additional failure or unexpected pass requires
review. The diagnostic manifest keeps all original PASS expectations and
therefore exits with a test failure for the remaining specification differences.

The original `serviceworker_cookieStore_cross_origin.https.sub.html` passes
through transferred MessagePorts and the owning Worker event loops. It
previously failed with `DataCloneError` before the cookie assertions.

The original `serviceworker_cookieStore_subscriptions_reset.https.html` passes
both its unregister/re-register and update subtests. Updates refetch the worker
and its classic imports, compare sources, retain subscriptions across a new
worker version, and preserve the active worker when updating fails. Native
regressions also check that stale registration generations cannot modify a new
registration through retained Window or Worker objects.

The unpartitioned and partitioned no-change subtests both pass. A newly added
subscription records its starting change sequence, so deletions made by cleanup
while unsubscribed are not replayed after re-subscription. Duplicate subscribe
calls preserve already-pending changes. Partitioned cookie identity, filtering,
and mutation use the top-level site independently of Storage Access grants.

| Remaining case | Measured difference from the current normative algorithm |
| --- | --- |
| `cookieStore_get_arguments` | Four alternate-URL assertions fail. The pinned Worker test allows other same-origin paths; [get(options)](https://cookiestore.spec.whatwg.org/#dom-cookiestore-get) step 6.2 instead requires the creation URL, excluding fragments. The non-normative introduction still describes broader Worker URL access. |
| `cookieStore_getAll_arguments` | The same four assertions disagree with [getAll(options)](https://cookiestore.spec.whatwg.org/#dom-cookiestore-getall) step 5.2. |
| `serviceworker_cookiechange_eventhandler_already_expired` | Both unpartitioned and partitioned subtests expect no deletion for a newly created, immediately expired cookie. The [observable-changes algorithm](https://cookiestore.spec.whatwg.org/#observable-changes) instead describes it as deleted. |

The normative algorithms were checked on 2026-10-17. These differences remain
visible as original WPT failures; they are not counted as passing assertions.

To rerun all original cases with PASS expectations:

```sh
WPT_ROOT=target/wpt scripts/fetch-wpt.sh
WPT_ROOT=target/wpt WPT_REQUIRED=1 \
  WPT_MANIFEST=tests/wpt/cookie-store-serviceworker.json \
  WPT_REPORT=.artifacts/cookie-store-serviceworker.json \
  cargo test --locked --test wpt_smoke selected_wpt_testharness_cases_match_expectations -- --nocapture
```

Use the repository's dedicated `CARGO_TARGET_DIR` and space guard when compiling.
A green default CI run does not mean all original Cookie Store WPT cases pass.
