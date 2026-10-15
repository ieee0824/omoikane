//! CORP reaches the browser's no-CORS fetch path before an opaque response is exposed.

use omoikane::{
    html::TreeBuilder,
    http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport},
    js::JsRuntime,
};
use std::sync::Arc;
use std::time::Duration;

struct CorpTransport;

impl HttpTransport for CorpTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let (headers, body) = match request.url().path() {
            "/blocked" => ("Cross-Origin-Resource-Policy: same-origin\r\n", "ok"),
            "/same-site" => ("Cross-Origin-Resource-Policy: same-site\r\n", "ok"),
            "/cross-origin" => ("Cross-Origin-Resource-Policy: cross-origin\r\n", "ok"),
            "/default" => ("", "ok"),
            "/cors" => (
                "Cross-Origin-Resource-Policy: same-origin\r\nAccess-Control-Allow-Origin: https://a.example.test\r\n",
                "ok",
            ),
            "/script-blocked" => (
                "Cross-Origin-Resource-Policy: same-origin\r\n",
                "globalThis.blockedScriptRan = true;",
            ),
            "/script-allowed" => (
                "Cross-Origin-Resource-Policy: cross-origin\r\n",
                "globalThis.allowedScriptRan = true;",
            ),
            path => panic!("unexpected request: {path}"),
        };
        let raw = format!(
            "HTTP/1.1 200 OK\r\n{headers}Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        HttpResponse::parse(&mut raw.as_bytes())
    }
}

#[test]
fn no_cors_fetch_rejects_same_origin_corp_but_allows_opted_in_opaque_responses() {
    let document = TreeBuilder::parse("<body></body>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://a.example.test/page").unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(CorpTransport)));
    runtime
        .eval(
            r#"
            globalThis.corpResults = null;
            const base = "https://b.example.test";
            Promise.all([
              fetch(base + "/blocked", {mode: "no-cors"}).then(
                () => "unexpected", error => error instanceof TypeError ? "blocked" : "wrong-error"),
              fetch(base + "/same-site", {mode: "no-cors"}).then(r => r.type, () => "unexpected"),
              fetch(base + "/cross-origin", {mode: "no-cors"}).then(r => r.type, () => "unexpected"),
              fetch(base + "/default", {mode: "no-cors"}).then(r => r.type, () => "unexpected"),
              fetch(base + "/cors", {mode: "cors"}).then(r => r.type, () => "unexpected"),
            ]).then(results => { corpResults = results; });
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                "JSON.stringify(corpResults) === '[\"blocked\",\"opaque\",\"opaque\",\"opaque\",\"cors\"]'"
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn sandboxed_child_fetch_uses_its_opaque_origin_for_corp() {
    let document = TreeBuilder::parse("<body></body>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://a.example.test/page").unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(CorpTransport)));
    runtime
        .eval(
            "globalThis.corpChildResults = []; addEventListener('message', event => corpChildResults.push([event.data, event.origin]));",
        )
        .unwrap();
    let child_markup = r#"<script>
        fetch('https://a.example.test/blocked', {mode: 'no-cors'}).then(
            () => parent.postMessage('unexpected', '*'),
            error => parent.postMessage(error instanceof TypeError ? 'blocked' : 'wrong-error', '*'));
    </script>"#;
    runtime
        .eval(&format!(
            "const frame = document.createElement('iframe'); frame.setAttribute('sandbox', 'allow-scripts'); frame.srcdoc = {}; document.body.appendChild(frame);",
            serde_json::to_string(child_markup).unwrap()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("JSON.stringify(corpChildResults) === '[[\"blocked\",\"null\"]]'")
            .unwrap()
            .to_boolean(),
        "child result: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(corpChildResults)").unwrap(),
        runtime.take_task_errors()
    );
}

#[test]
fn classic_script_with_same_origin_corp_does_not_execute_cross_origin() {
    let document = TreeBuilder::parse(
        "<body><script src='https://b.example.test/script-blocked'></script><script src='https://b.example.test/script-allowed'></script></body>",
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://a.example.test/page").unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(CorpTransport)));
    let base = "https://a.example.test/page".parse().unwrap();
    let errors = runtime.execute_document_scripts(Some(&base));
    assert_eq!(
        errors,
        vec!["failed to fetch script: https://b.example.test/script-blocked".to_string()]
    );
    assert!(
        runtime
            .eval(
                "globalThis.blockedScriptRan === undefined && globalThis.allowedScriptRan === true"
            )
            .unwrap()
            .to_boolean()
    );
}
