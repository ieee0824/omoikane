//! Registered blob URLs can supply iframe Documents without a network request.

use omoikane::html::TreeBuilder;
use omoikane::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use omoikane::js::JsRuntime;
use std::sync::Arc;
use std::time::Duration;

struct BlobChildTransport;

impl HttpTransport for BlobChildTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        assert_eq!(request.url().path(), "/child");
        let body = r#"<!doctype html><body><script>
          parent.postMessage({stage:'initial', origin:location.origin, fullscreen:document.fullscreenEnabled}, '*');
          const html = '<scr' + 'ipt>parent.postMessage({stage:"blob", origin:location.origin, fullscreen:document.fullscreenEnabled}, "*")</scr' + 'ipt>';
          location.replace(URL.createObjectURL(new Blob([html], {type:'text/html'})));
        </script>"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        HttpResponse::parse(&mut response.as_bytes())
    }
}

#[test]
fn iframe_blob_document_executes_with_the_creators_origin() {
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://owner.test:8443/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.blobMessages = [];
            addEventListener('message', event => blobMessages.push(event.data));
            const html = '<script>parent.postMessage({stage:"blob", origin:location.origin, fullscreen:document.fullscreenEnabled}, "*")</script>';
            const blobUrl = URL.createObjectURL(new Blob([html], {type:'text/html'}));
            const frame = document.createElement('iframe');
            frame.src = blobUrl;
            document.body.appendChild(frame);
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let errors = runtime.take_task_errors();
    assert!(errors.is_empty(), "iframe task errors: {errors:?}");
    let result = runtime.eval("JSON.stringify(blobMessages)").unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        r#"[{"stage":"blob","origin":"https://owner.test:8443","fullscreen":true}]"#
    );
}

#[test]
fn child_location_replace_loads_a_blob_document() {
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://owner.test:8443/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.blobMessages = [];
            addEventListener('message', event => blobMessages.push(event.data));
            globalThis.blobHtml = '<script>parent.postMessage({stage:"blob", origin:location.origin, fullscreen:document.fullscreenEnabled}, "*")</script>';
            const frame = document.createElement('iframe');
            frame.srcdoc = '<script>parent.postMessage({stage:"initial", origin:location.origin, fullscreen:document.fullscreenEnabled}, "*"); const url = URL.createObjectURL(new Blob([parent.blobHtml], {type:"text/html"})); location.replace(url);</script>';
            document.body.appendChild(frame);
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let errors = runtime.take_task_errors();
    assert!(errors.is_empty(), "iframe task errors: {errors:?}");
    let result = runtime.eval("JSON.stringify(blobMessages)").unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        r#"[{"stage":"initial","origin":"null","fullscreen":true},{"stage":"blob","origin":"https://owner.test:8443","fullscreen":true}]"#
    );
}

#[test]
fn same_origin_http_child_keeps_fullscreen_after_blob_navigation() {
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://owner.test:8443/page").unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(BlobChildTransport)));
    runtime
        .eval(
            r#"
            globalThis.blobMessages = [];
            addEventListener('message', event => blobMessages.push(event.data));
            const frame = document.createElement('iframe');
            document.body.appendChild(frame);
            frame.src = 'http://owner.test:8443/child';
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let errors = runtime.take_task_errors();
    assert!(errors.is_empty(), "iframe task errors: {errors:?}");
    let result = runtime.eval("JSON.stringify(blobMessages)").unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        r#"[{"stage":"initial","origin":"http://owner.test:8443","fullscreen":true},{"stage":"blob","origin":"http://owner.test:8443","fullscreen":true}]"#
    );
}
