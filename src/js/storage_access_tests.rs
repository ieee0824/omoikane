use super::*;

struct StorageAccessTransport;

impl crate::http::HttpTransport for StorageAccessTransport {
    fn send(
        &self,
        _request: &HttpRequest,
        _timeout: Option<Duration>,
    ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
        Ok(crate::http::HttpResponse::new(
            200,
            "OK",
            vec![("Content-Type".into(), "text/html".into())],
            b"<html><body><script>window.frameReady = true</script></body></html>".to_vec(),
        ))
    }
}

fn runtime_at(url: &str) -> JsRuntime {
    JsRuntime::with_document_and_url(
        crate::html::TreeBuilder::parse("<html><body></body></html>").document(),
        url,
    )
    .unwrap()
}

fn child_document(runtime: &JsRuntime) -> NodeHandle {
    runtime
        .host_state
        .borrow()
        .iframe_documents
        .values()
        .next()
        .expect("iframe navigation committed")
        .document
        .clone()
}

fn child_result(document: &NodeHandle) -> Option<String> {
    document
        .child_nodes()
        .into_iter()
        .find(|node| node.node_type() == NodeType::Element)
        .and_then(|html| html.get_attribute("data-result"))
}

#[test]
fn storage_access_top_level_secure_insecure_and_opaque_documents() {
    let mut secure = runtime_at("https://top.test/");
    secure
        .eval(
            "document.hasStorageAccess().then(v => globalThis.hasAccess = v); \
             document.requestStorageAccess().then(() => globalThis.requested = true)",
        )
        .unwrap();
    secure.run_jobs().unwrap();
    assert_eq!(
        secure.eval("hasAccess && requested").unwrap().as_boolean(),
        Some(true)
    );
    assert_eq!(
        secure
            .eval("typeof __omoikane_storage_access_request")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
    assert_eq!(
        secure
            .eval(
                "(() => { try { Document.prototype.requestStorageAccess.call({__id: document.__id}); \
                 return 'accepted'; } catch (error) { return error.name; } })()"
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "TypeError"
    );

    for url in ["http://insecure.test/", "data:text/html,opaque"] {
        let mut runtime = runtime_at(url);
        runtime
            .eval(
                "document.hasStorageAccess().then(v => globalThis.hasAccess = v); \
                 document.requestStorageAccess().catch(e => globalThis.errorName = e.name)",
            )
            .unwrap();
        runtime.run_jobs().unwrap();
        assert_eq!(
            runtime
                .eval("String(hasAccess) + '|' + errorName")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "false|NotAllowedError"
        );
    }
}

#[test]
fn trusted_input_marker_is_cleared_after_script_error() {
    let mut runtime = runtime_at("https://top.test/");
    let result = runtime.with_trusted_storage_access_input(|runtime| {
        runtime.eval("throw new Error('input handler failed')")
    });
    assert!(result.is_err());
    assert!(!runtime.host_state.borrow().trusted_storage_access_input);
}

#[test]
fn third_party_storage_access_grant_changes_dom_cookie_visibility() {
    let mut runtime = runtime_at("https://top.test/");
    runtime.set_http_resource_transport(Some(Arc::new(StorageAccessTransport)));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=preexisting; Path=/; Secure; SameSite=None",
            &"https://third.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const f = document.createElement('iframe'); \
             f.src = 'https://third.test/frame'; document.body.appendChild(f)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let child = child_document(&runtime);
    let id = child.identity();

    runtime
        .eval_in_document_realm(
            id,
            "Promise.all([document.hasStorageAccess(), \
              document.requestStorageAccess().then(() => 'granted', e => e.name)]) \
              .then(([has, requested]) => { \
                document.cookie = 'blocked=1; Secure; SameSite=None'; \
                document.documentElement.setAttribute('data-result', \
                  [has, requested, document.cookie].join('|')); \
              })",
        )
        .unwrap();
    assert_eq!(
        child_result(&child).as_deref(),
        Some("false|NotAllowedError|")
    );

    runtime
        .eval_in_document_realm(
            id,
            "document.addEventListener('keydown', () => { \
                document.requestStorageAccess().then( \
                  () => document.documentElement.setAttribute('data-result', 'forged-grant'), \
                  e => document.documentElement.setAttribute('data-result', e.name)); \
              }, {once: true}); \
              __omoikane_dispatch_keyboard_input('keydown', {key: 'Enter'})",
        )
        .unwrap();
    assert_eq!(child_result(&child).as_deref(), Some("NotAllowedError"));

    runtime
        .with_trusted_storage_access_input(|runtime| {
            runtime.eval_in_document_realm(
                id,
                "document.addEventListener('keydown', () => { \
                    Promise.resolve().then(() => document.requestStorageAccess()).then(() => { \
                      document.cookie = 'written=1; Secure; SameSite=None'; \
                      document.hasStorageAccess().then(has => \
                        document.documentElement.setAttribute('data-result', \
                          [has, document.cookie].join('|'))); \
                    }); \
                  }, {once: true}); \
                  __omoikane_dispatch_keyboard_input('keydown', {key: 'Enter'})",
            )
        })
        .unwrap();
    assert_eq!(
        child_result(&child).as_deref(),
        Some("true|session=preexisting; written=1")
    );
    assert!(!runtime.host_state.borrow().trusted_storage_access_input);
}

#[test]
fn sandbox_token_controls_storage_access_request() {
    for (tokens, expected) in [
        ("allow-scripts allow-same-origin", "NotAllowedError"),
        (
            "allow-scripts allow-same-origin allow-storage-access-by-user-activation",
            "granted",
        ),
    ] {
        let mut runtime = runtime_at("https://top.test/");
        runtime.set_http_resource_transport(Some(Arc::new(StorageAccessTransport)));
        runtime
            .eval(&format!(
                "const f = document.createElement('iframe'); \
                 f.setAttribute('sandbox', {tokens:?}); \
                 f.src = 'https://third.test/frame'; document.body.appendChild(f)"
            ))
            .unwrap();
        runtime.tick(0).unwrap();
        let child = child_document(&runtime);
        runtime
            .with_trusted_storage_access_input(|runtime| {
                runtime.eval_in_document_realm(
                    child.identity(),
                    "document.addEventListener('keydown', () => { \
                        document.requestStorageAccess().then( \
                          () => document.documentElement.setAttribute('data-result', 'granted'), \
                          e => document.documentElement.setAttribute('data-result', e.name)); \
                      }, {once: true}); \
                      __omoikane_dispatch_keyboard_input('keydown', {key: 'Enter'})",
                )
            })
            .unwrap();
        assert_eq!(child_result(&child).as_deref(), Some(expected));
    }
}
