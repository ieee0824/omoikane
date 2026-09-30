# HTTP fixture server helper

`http_fixture.rs` is a test-only module shared by integration tests through a
`#[path = "support/http_fixture.rs"]` import. It owns only loopback binding,
bounded connection acceptance, bounded request-header reading, and joining a
server worker during normal completion or client-side unwinding. Each test keeps
its own HTTP responses, cookie/origin rules, request order, and assertions.

Unit tests in `src/` import the same helper through `src/test_support/mod.rs`.
`src/js/stylesheet.rs` and `src/http/client.rs` use it; the latter migrates nine
local server setups while preserving its 22 tests, responses, and assertions.
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
`src/paint/tests.rs` uses it for image and stylesheet servers. The local
stylesheet responder keeps each test's original response bytes and request
count while sharing bounded accepts, header reads, and worker joins.

Issue #885 first migrates `document_cookie_bridge.rs` and
`fetch_authorization.rs`. `http_fixture_support.rs` checks the helper's timeout,
header/body boundary, and worker-join behavior. The remaining `src/` unit-test
servers use the same shared primitives.

`src/ffi/mod.rs` and `src/platform_browser.rs` use the shared fixture for their
navigation and download server tests. The platform browser's closed-port error
test still binds and closes a listener deliberately.

`src/js/document_write_tests.rs`, `src/js/module_loading_tests.rs`, and
`src/js/font_loading_tests.rs` use it for their fixture servers. The module
server still owns its concurrent request handlers and shutdown logic, while
the shared helper bounds accepts and request-header reads. The dedicated
nonblocking-socket test in `src/js/module_server_tests.rs` also uses the shared
loopback bind and retains its socket-mode assertions.

`src/js/tests.rs` and `src/cdp/tests.rs` use bounded accepts, complete header
reads, and joined fixture workers. JS request-body tests read exactly the
declared Content-Length after the headers; WebSocket framing remains local.
Persistent iframe servers stop and join at the end of their owning test.
The JS module retains all 773 existing tests and adds one regression for
repeated fixture requests and joining on drop. Header completeness and body
boundaries are covered by `http_fixture_support.rs`.
The CDP module retains its 84 tests.
The JS socket-policy tests deliberately keep listeners to assert that no
connection is made; connection-failure tests keep their closed-port binds.
`src/http/connection.rs` and `src/http/http2.rs` share bounded accepts and worker
joins while retaining their TLS, HTTP/2, and IPv6 protocol-specific behavior.
TLS fixtures set socket deadlines before handshakes and reads. IPv6 fixtures
retain their explicit loopback address, enable nonblocking listener acceptance,
and use the same bounded accept helper.
TLS HTTP headers use `read_request_headers_from`, which shares TCP's complete
header reads, size limit, and total deadline while configuring the TLS socket
through an explicit callback. The transport adapter has a deadline/body-boundary
regression in `http_fixture_support.rs`.
