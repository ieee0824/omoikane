//! A popup can close during parser script execution without losing its reply.
use omoikane::html::TreeBuilder;
use omoikane::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use omoikane::js::JsRuntime;
use std::sync::Arc;
use std::time::Duration;

struct PopupTransport;

impl HttpTransport for PopupTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let script = match request.url().path() {
            "/popup" | "/popup-following-script" => {
                "document.cookie='prepared=1; Secure; SameSite=None; Path=/'; opener.postMessage('prepared','*'); close();"
            }
            "/popup-close-dcl" => {
                "document.addEventListener('DOMContentLoaded', () => { opener.postMessage('prepared','*'); close(); });"
            }
            "/popup-stay-open" => {
                "addEventListener('load', () => opener.postMessage('prepared','*'));"
            }
            path => panic!("unexpected popup fixture: {path}"),
        };
        let mut body = format!("<!doctype html><body><script>{script}</script></body>");
        if request.url().path() == "/popup-following-script" {
            body.push_str("<script>opener.postMessage('late', '*')</script>");
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        HttpResponse::parse(&mut response.as_bytes())
    }
}

fn assert_popup_reply(origin: &str, path: &str, closed: bool) {
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://top.test/page").unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(PopupTransport)));
    runtime
        .eval(&format!(
            "globalThis.received=[]; addEventListener('message', e => \
             received.push([e.data, e.origin, e.source===popup])); \
             globalThis.popup=open('{origin}/{path}');"
        ))
        .unwrap();
    for _ in 0..4 {
        runtime
            .tick(16)
            .unwrap_or_else(|error| panic!("popup task failed: {error}"));
    }
    assert!(runtime.take_task_errors().is_empty());
    let received = runtime
        .eval("JSON.stringify(received)")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(received, format!(r#"[["prepared","{origin}",true]]"#));
    assert_eq!(
        runtime.eval("popup.closed").unwrap().as_boolean(),
        Some(closed)
    );
}

#[test]
fn same_origin_popup_reply_survives_closing_during_its_parser_script() {
    assert_popup_reply("https://top.test", "popup", true);
}

#[test]
fn cross_origin_popup_reply_survives_closing_during_its_parser_script() {
    assert_popup_reply("https://third.test", "popup", true);
}

#[test]
fn closing_a_popup_stops_later_parser_scripts() {
    assert_popup_reply("https://third.test", "popup-following-script", true);
}

#[test]
fn closing_a_popup_during_dom_content_loaded_stops_its_load_event() {
    assert_popup_reply("https://third.test", "popup-close-dcl", true);
}

#[test]
fn a_live_popup_still_receives_its_load_event() {
    assert_popup_reply("https://third.test", "popup-stay-open", false);
}
