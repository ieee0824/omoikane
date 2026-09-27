#[path = "support/http_fixture.rs"]
mod http_fixture;

use http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};
use omoikane::{html::TreeBuilder, js::JsRuntime};
use std::io::Write;

#[test]
fn server_and_document_cookies_share_one_jar_without_exposing_httponly() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        for path in ["/set", "/echo"] {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let (line, headers) = request.split_once("\r\n").unwrap();
            assert!(line.starts_with(&format!("GET {path} HTTP/1.1")));
            if path == "/set" {
                stream.write_all(b"HTTP/1.1 204 No Content\r\nSet-Cookie: visible=1; Path=/\r\nSet-Cookie: secret=server; HttpOnly; Path=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            } else {
                assert!(headers.contains("visible=1"));
                assert!(headers.contains("secret=server"));
                assert!(headers.contains("script=2"));
                assert!(!headers.contains("secret=script"));
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .unwrap();
            }
        }
    });

    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<body></body>").document(),
        &format!("http://{address}/page"),
    )
    .unwrap();
    runtime
        .eval(&format!("fetch('http://{address}/set')"))
        .unwrap();
    runtime.run_jobs().unwrap();
    assert!(
        runtime
            .eval("document.cookie === 'visible=1'")
            .unwrap()
            .to_boolean()
    );
    runtime
        .eval("document.cookie = 'secret=script; Path=/'; document.cookie = 'script=2; Path=/'")
        .unwrap();
    assert!(
        runtime
            .eval("document.cookie === 'visible=1; script=2'")
            .unwrap()
            .to_boolean()
    );
    runtime
        .eval(&format!("fetch('http://{address}/echo')"))
        .unwrap();
    runtime.run_jobs().unwrap();
    server.join();
}

#[test]
fn navigation_carries_server_and_script_cookies_between_documents() {
    use omoikane::cdp::CdpSession;
    use serde_json::json;

    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        for path in ["/first", "/second"] {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let (line, headers) = request.split_once("\r\n").unwrap();
            assert!(line.starts_with(&format!("GET {path} HTTP/1.1")), "{line}");
            if path == "/second" {
                assert!(headers.contains("server=1"), "{headers}");
                assert!(headers.contains("script=2"), "{headers}");
            }
            let extra = if path == "/first" {
                "Set-Cookie: server=1; Path=/\r\n"
            } else {
                ""
            };
            let body = "<html><body>cookie page</body></html>";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{}",
                body.len(),
                extra,
                body
            )
            .unwrap();
        }
    });

    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url": format!("http://{address}/first")}),
        )
        .unwrap();
    let first = session
        .dispatch("Runtime.evaluate", json!({"expression": "document.cookie"}))
        .unwrap();
    assert_eq!(first["result"]["value"], "server=1");
    session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression": "document.cookie = 'script=2; Path=/'"}),
        )
        .unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url": format!("http://{address}/second")}),
        )
        .unwrap();
    let second = session
        .dispatch("Runtime.evaluate", json!({"expression": "document.cookie"}))
        .unwrap();
    assert_eq!(second["result"]["value"], "server=1; script=2");
    server.join();
}

#[test]
fn cookie_averse_document_has_no_cookie_state() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<body></body>").document(),
        "data:text/html,<body></body>",
    )
    .unwrap();
    assert!(
        runtime
            .eval("document.cookie = 'x=1'; document.cookie === ''")
            .unwrap()
            .to_boolean()
    );
}
