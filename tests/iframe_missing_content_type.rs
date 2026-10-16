//! Navigation must identify unlabeled HTML without executing labeled plain text.
use omoikane::html::TreeBuilder;
use omoikane::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use omoikane::js::JsRuntime;
use std::sync::Arc;
use std::time::Duration;

struct UnlabeledTransport;

impl HttpTransport for UnlabeledTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let headers = match request.url().path() {
            "/unlabeled" => "",
            "/plain" => "Content-Type: text/plain\r\n",
            "/unknown" => "Content-Type: application/unknown\r\n",
            "/nosniff" => "X-Content-Type-Options: nosniff\r\n",
            "/csp" => "Content-Security-Policy: script-src 'none'\r\n",
            path => panic!("unexpected iframe fixture: {path}"),
        };
        let body = "\n  <!DOCTYPE html><body><script>\
            addEventListener('message', e => e.source.postMessage('pong', '*'));\
            </script></body>";
        let response = format!(
            "HTTP/1.1 200 OK\r\n{headers}Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        HttpResponse::parse(&mut response.as_bytes())
    }
}

fn iframe_reply(origin: &str, path: &str) -> String {
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://top.test/page").unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(UnlabeledTransport)));
    runtime
        .eval(&format!(
            "globalThis.received=[]; globalThis.loaded=false; \
             addEventListener('message', e => received.push(e.data)); \
             const frame=document.createElement('iframe'); \
             frame.onload=()=>{{loaded=true;frame.contentWindow.postMessage('ping','*');}}; \
             frame.src='{origin}/{path}';document.body.appendChild(frame);"
        ))
        .unwrap();
    for _ in 0..4 {
        runtime
            .tick(16)
            .unwrap_or_else(|error| panic!("iframe task failed: {error}"));
    }
    assert_eq!(runtime.eval("loaded").unwrap().as_boolean(), Some(true));
    assert!(runtime.take_task_errors().is_empty());
    runtime
        .eval("JSON.stringify(received)")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn unlabeled_html_iframe_installs_its_message_responder_before_load() {
    assert_eq!(
        iframe_reply("https://third.test", "unlabeled"),
        r#"["pong"]"#
    );
}

#[test]
fn explicit_plain_text_does_not_execute_html_looking_script() {
    assert_eq!(iframe_reply("https://third.test", "plain"), "[]");
}

#[test]
fn same_origin_unlabeled_html_also_runs_its_responder() {
    assert_eq!(iframe_reply("https://top.test", "unlabeled"), r#"["pong"]"#);
}

#[test]
fn explicitly_unknown_mime_allows_document_sniffing() {
    assert_eq!(iframe_reply("https://third.test", "unknown"), r#"["pong"]"#);
}

#[test]
fn nosniff_prevents_unknown_html_from_executing_script() {
    assert_eq!(iframe_reply("https://third.test", "nosniff"), "[]");
}

#[test]
fn sniffed_html_keeps_its_response_csp() {
    assert_eq!(iframe_reply("https://third.test", "csp"), "[]");
}
