# Cookie Store Service Worker WPT

Issue: [#1354](https://github.com/ieee0824/omoikane/issues/1354), parent
[#1298](https://github.com/ieee0824/omoikane/issues/1298).

The runner serves the pinned upstream `ServiceWorkersHandler` HTML and
`ClassicWorkerHandler` JavaScript wrappers. They register a real Service Worker,
import the original testharness and META scripts, and forward results using
`fetch_tests_from_worker`. The original tests, helpers, assertions, and timeouts
are unchanged. `scripts/fetch-wpt.sh` includes the wrapper dependencies.

At revision `dc97e7bed3096ac9e0e591ab5fa22e7fb8844ead`, the 25 Cookie Store
`.any.js` files declaring `serviceworker` produce **21 PASS / 4 FAIL** after the
subscription/lifecycle fixes (2026-10-17). Every harness completes with status 0;
there are no script errors. The initial run was 19 PASS / 6 FAIL and exposed
missing-name serialization, registration lifecycle, and query-name normalization
bugs. These were fixed without modifying WPT.

The default CI manifest runs all 25 variants. Four new known-failure entries
name the exact failing subtests, so an additional failure or an unexpected pass
requires review. They do not establish completion of #1354.

Two additional original HTML cases were measured on 2026-10-17. The
`serviceworker_cookieStore_cross_origin.https.sub.html` case now passes after
connecting transferred MessagePorts to the Worker message tasks and their
owning event loops. It previously failed with `DataCloneError` before the cookie
assertions. This case also runs in the default CI manifest.

`serviceworker_cookieStore_subscriptions_reset.https.html` passes its unregister
and re-register subtest, but its update subtest remains unfinished (harness
status 2): `ServiceWorkerRegistration.update()` does not yet run the update
lifecycle. This remains required work under #1354. The diagnostic manifest
below includes both HTML cases as well as the 25 generated variants, all with
their original PASS expectations.

| Remaining case | Evidence and remaining work |
| --- | --- |
| `cookieStore_get_arguments` | Four alternate-URL assertions fail. The pinned Worker test allows other same-origin paths; the current [get(options) algorithm](https://cookiestore.spec.whatwg.org/#dom-cookiestore-get) requires the creation URL, excluding fragments. The whitespace-name assertion passes. |
| `cookieStore_getAll_arguments` | The same four alternate-URL assertions disagree with the current [getAll(options) algorithm](https://cookiestore.spec.whatwg.org/#dom-cookiestore-getall). The whitespace-name assertion passes. |
| `serviceworker_cookiechange_eventhandler_already_expired` | The first assertion expects no deletion for a newly created, immediately expired cookie; the current [observable-changes algorithm](https://cookiestore.spec.whatwg.org/#observable-changes) describes it as deleted. The second subtest requires partitioned cookie storage, which is not implemented. |
| `serviceworker_cookiechange_eventhandler_no_change` | The unpartitioned subtest passes. The partitioned subtest fails because partitioned cookie storage is not implemented. |

To rerun all original cases with PASS expectations, including the unfinished
ones (this currently exits with a test failure):

```sh
WPT_ROOT=target/wpt scripts/fetch-wpt.sh
WPT_ROOT=target/wpt WPT_REQUIRED=1 \
  WPT_MANIFEST=tests/wpt/cookie-store-serviceworker.json \
  WPT_REPORT=.artifacts/cookie-store-serviceworker.json \
  cargo test --locked --test wpt_smoke selected_wpt_testharness_cases_match_expectations -- --nocapture
```

Use the repository's dedicated `CARGO_TARGET_DIR` and space guard when compiling.
Partitioned storage and its event filtering remain required work; a green
default CI run does not mean all original Cookie Store WPT cases pass.
