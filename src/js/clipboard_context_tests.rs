//! Clipboard exposure uses the owning Window, rather than its opener's URL.

use super::*;
use crate::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};

struct ClipboardDocumentTransport;

impl HttpTransport for ClipboardDocumentTransport {
    fn send(
        &self,
        _: &HttpRequest,
        _: Option<std::time::Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![("Content-Type".into(), "text/html".into())],
            b"<!doctype html><body>clipboard context</body>".to_vec(),
        ))
    }
}

fn popup_clipboard_exposure(opener: &str, destination: &str) -> String {
    let mut runtime = JsRuntime::with_document_and_url(default_document(), opener).unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(ClipboardDocumentTransport)));
    runtime
        .eval(&format!("window.open({destination:?})"))
        .unwrap();
    runtime.run_until_idle().unwrap();
    let realm = {
        let state = runtime.host_state.borrow();
        let popup = state.auxiliary_contexts.values().next().unwrap();
        assert_eq!(popup.document_url, destination);
        popup.realm.clone().unwrap()
    };
    let previous = runtime.context.enter_realm(realm);
    let exposed = runtime
        .eval("isSecureContext + '|' + typeof ClipboardItem")
        .unwrap();
    runtime.context.enter_realm(previous);
    exposed.as_string().unwrap().to_std_string_escaped()
}

#[test]
fn secure_clipboard_item_popup_does_not_inherit_its_insecure_opener() {
    assert_eq!(
        popup_clipboard_exposure(
            "http://clipboard.example.test/opener",
            "https://clipboard.example.test/popup"
        ),
        "true|function"
    );
}

#[test]
fn insecure_clipboard_item_popup_does_not_inherit_its_secure_opener() {
    assert_eq!(
        popup_clipboard_exposure(
            "https://clipboard.example.test/opener",
            "http://clipboard.example.test/popup"
        ),
        "false|undefined"
    );
}

#[test]
fn secure_clipboard_item_iframe_requires_a_secure_ancestor() {
    let mut runtime = JsRuntime::with_document_and_url(
        default_document(),
        "http://clipboard.example.test/parent",
    )
    .unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(ClipboardDocumentTransport)));
    runtime
        .eval(
            "const child = document.createElement('iframe'); \
         child.src = 'https://clipboard.example.test/frame'; document.body.append(child);",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let (frame_id, document_id) = {
        let state = runtime.host_state.borrow();
        let (&frame_id, frame) = state.iframe_documents.iter().next().unwrap();
        assert_eq!(
            state.document_urls[&frame.document.identity()],
            "https://clipboard.example.test/frame"
        );
        (frame_id, frame.document.identity())
    };
    let realm = runtime.ensure_iframe_realm(frame_id, document_id).unwrap();
    let previous = runtime.context.enter_realm(realm);
    let exposed = runtime
        .eval("isSecureContext + '|' + typeof ClipboardItem")
        .unwrap();
    runtime.context.enter_realm(previous);
    assert_eq!(
        exposed.as_string().unwrap().to_std_string_escaped(),
        "false|undefined"
    );
}
