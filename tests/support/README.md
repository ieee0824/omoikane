# HTTP fixture server helper

`http_fixture.rs` is a test-only module shared by integration tests through a
`#[path = "support/http_fixture.rs"]` import. It owns only loopback binding,
bounded connection acceptance, bounded request-header reading, and joining a
server worker during normal completion or client-side unwinding. Each test keeps
its own HTTP responses, cookie/origin rules, request order, and assertions.

Unit tests in `src/` import the same helper through `src/test_support/mod.rs`.
`src/js/stylesheet.rs` and `src/http/client.rs` use it; the latter migrates nine
local server setups while preserving its 22 tests, responses, and assertions.
The other local unit-test servers remain tracked by Issue #1089.
`print_page_margin_content.rs` also uses the shared fixture for its image request.
`location_pseudo_target.rs` also uses the shared fixture for its iframe requests.
`cookie_samesite.rs`, `error_reporting_http.rs`, `form_target.rs`, and
`module_identity.rs` also use it; their 24 test cases retain their responses
and assertions.
`browser_journeys.rs`, `page_visibility.rs`, and `pointer_lock.rs` also use it;
their 45 test cases retain their responses and assertions.
`wpt_smoke/server.rs` also uses it for bounded request handling and worker join.
`acid3_common/harness.rs` uses the same helper in both the integration test
and the CLI example; its worker is joined when the server is dropped.
`subresource_cookie_store.rs` also uses it for all seven tests, including the
delayed-request regression from Issue #983. Its accepted sockets are explicitly
returned to blocking mode before bounded header reads.
`src/screenshot/mod.rs` uses it for three frameset fixture servers; their
responses and path assertions are unchanged, and the file retains 11 unit tests.

Issue #885 first migrates `document_cookie_bridge.rs` and
`fetch_authorization.rs`. `http_fixture_support.rs` checks the helper's timeout,
header/body boundary, and worker-join behavior. The remaining `src/` unit-test
servers are tracked by Issue #1089 and should be migrated in reviewable changes.
