# HTTP fixture server helper

`http_fixture.rs` is a test-only module shared by integration tests through a
`#[path = "support/http_fixture.rs"]` import. It owns only loopback binding,
bounded connection acceptance, bounded request-header reading, and joining a
server worker during normal completion or client-side unwinding. Each test keeps
its own HTTP responses, cookie/origin rules, request order, and assertions.

Unit tests in `src/` import the same helper through `src/test_support/mod.rs`.
`src/js/stylesheet.rs` is the first migrated unit-test server; the other local
servers remain tracked by Issue #1089.
`print_page_margin_content.rs` also uses the shared fixture for its image request.
`location_pseudo_target.rs` also uses the shared fixture for its iframe requests.
`cookie_samesite.rs`, `error_reporting_http.rs`, `form_target.rs`, and
`module_identity.rs` also use it; their 24 test cases retain their responses
and assertions.
`browser_journeys.rs`, `page_visibility.rs`, and `pointer_lock.rs` also use it;
their 45 test cases retain their responses and assertions.

Issue #885 first migrates `document_cookie_bridge.rs` and
`fetch_authorization.rs`. `http_fixture_support.rs` checks the helper's timeout,
header/body boundary, and worker-join behavior. The following current fixture
files still use local server logic and have not been migrated:

- `acid3_common/harness.rs`
- `subresource_cookie_store.rs`
- `wpt_smoke/server.rs`

Future migrations should be separate, reviewable changes. In particular,
`subresource_cookie_store.rs` has a separate macOS timeout investigation in
Issue #983; changing its synchronization should be reviewed with that issue.
