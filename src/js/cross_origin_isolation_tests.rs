//! End-to-end policy tests across the response, browsing-context and fetch paths.

use super::*;
use crate::http::{HttpParseError, HttpResponse, HttpTransport};
use std::time::Duration;

fn isolated_headers() -> Vec<(String, String)> {
    vec![
        ("Cross-Origin-Opener-Policy".into(), "same-origin".into()),
        ("Cross-Origin-Embedder-Policy".into(), "require-corp".into()),
    ]
}

fn runtime_for(url: &str) -> JsRuntime {
    JsRuntime::with_document_and_url(
        crate::html::TreeBuilder::parse("<html><body></body></html>").document(),
        url,
    )
    .unwrap()
}

fn isolated(runtime: &mut JsRuntime) -> bool {
    runtime
        .eval("crossOriginIsolated")
        .unwrap()
        .as_boolean()
        .unwrap()
}

#[test]
fn location_stringifies_as_a_base_url_for_reporting_helpers() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    let result = runtime
        .eval("new URL('/common/dispatcher/dispatcher.py', location).href")
        .unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        "https://isolated.example.test/common/dispatcher/dispatcher.py"
    );
}

#[test]
fn popup_window_location_exposes_query_for_dispatcher_identity() {
    struct PopupQueryTransport;

    impl HttpTransport for PopupQueryTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            assert_eq!(request.url().path(), "/popup");
            Ok(HttpResponse::new(
                200,
                "OK",
                vec![("Content-Type".into(), "text/html".into())],
                b"<body>popup</body>".to_vec(),
            ))
        }
    }

    let mut runtime = runtime_for("https://isolated.example.test/main");
    runtime.set_http_resource_transport(Some(Arc::new(PopupQueryTransport)));
    runtime
        .eval("globalThis.popup = window.open('/popup?uuid=example')")
        .unwrap();
    runtime.run_until_idle().unwrap();
    let popup_realm = runtime
        .host_state
        .borrow()
        .auxiliary_contexts
        .values()
        .next()
        .unwrap()
        .realm
        .clone()
        .unwrap();
    let previous = runtime.context.enter_realm(popup_realm);
    let query = runtime.eval("window.location.search").unwrap();
    runtime.context.enter_realm(previous);
    assert_eq!(
        query.as_string().unwrap().to_std_string_escaped(),
        "?uuid=example"
    );
}

struct IsolationTransport;

impl HttpTransport for IsolationTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let (headers, body) = match request.url().path() {
            "/child-isolated" => (
                vec![
                    ("Content-Type".into(), "text/html".into()),
                    ("Cross-Origin-Embedder-Policy".into(), "require-corp".into()),
                    ("Cross-Origin-Resource-Policy".into(), "cross-origin".into()),
                ],
                b"<body>isolated child</body>".to_vec(),
            ),
            "/child-unisolated" => (
                vec![("Content-Type".into(), "text/html".into())],
                b"<body>must not commit</body>".to_vec(),
            ),
            "/child-isolated-csp" => (
                vec![
                    ("Content-Type".into(), "text/html".into()),
                    ("Cross-Origin-Embedder-Policy".into(), "require-corp".into()),
                    ("Content-Security-Policy".into(), "connect-src 'none'".into()),
                ],
                b"<body>cross-origin child</body>".to_vec(),
            ),
            "/child-redirect" => {
                return Ok(HttpResponse::new(
                    302,
                    "Found",
                    vec![(
                        "Location".into(),
                        "https://other.example.test/child-isolated".into(),
                    )],
                    Vec::new(),
                ));
            }
            "/popup-isolated" => (
                vec![
                    ("Content-Type".into(), "text/html".into()),
                    ("Cross-Origin-Opener-Policy".into(), "same-origin".into()),
                    ("Cross-Origin-Embedder-Policy".into(), "require-corp".into()),
                ],
                b"<body>popup</body>".to_vec(),
            ),
            "/popup-isolated-message" => (
                vec![
                    ("Content-Type".into(), "text/html".into()),
                    ("Cross-Origin-Opener-Policy".into(), "same-origin".into()),
                    ("Cross-Origin-Embedder-Policy".into(), "require-corp".into()),
                ],
                b"<script>globalThis.received=[];addEventListener('message', e=>received.push(e.data));</script>".to_vec(),
            ),
            "/popup-open" => (
                vec![("Content-Type".into(), "text/html".into())],
                b"<body>popup</body>".to_vec(),
            ),
            "/popup-b-policy" => (
                vec![
                    ("Content-Type".into(), "text/html".into()),
                    ("Cross-Origin-Embedder-Policy".into(), "credentialless".into()),
                    (
                        "Content-Security-Policy".into(),
                        "connect-src https://b.example.test".into(),
                    ),
                ],
                b"<body>popup B</body>".to_vec(),
            ),
            "/opaque" => (Vec::new(), b"opaque".to_vec()),
            "/opted-in" => (
                vec![("Cross-Origin-Resource-Policy".into(), "cross-origin".into())],
                b"opted in".to_vec(),
            ),
            "/worker-isolated.js" => (
                vec![("Cross-Origin-Embedder-Policy".into(), "require-corp".into())],
                b"onmessage = () => postMessage(crossOriginIsolated);".to_vec(),
            ),
            "/worker-unisolated.js" => (
                Vec::new(),
                b"onmessage = () => postMessage('must not run');".to_vec(),
            ),
            "/shared-worker-isolated.js" => (
                vec![("Cross-Origin-Embedder-Policy".into(), "require-corp".into())],
                b"onconnect = e => e.ports[0].postMessage(crossOriginIsolated);".to_vec(),
            ),
            "/shared-worker-unisolated.js" => (
                Vec::new(),
                b"globalThis.connectCount = 0; onconnect = e => e.ports[0].postMessage(++connectCount + ':' + crossOriginIsolated);".to_vec(),
            ),
            "/script-no-corp.js" => (Vec::new(), b"globalThis.scriptNoCorpRan = true;".to_vec()),
            "/script-corp.js" => (
                vec![("Cross-Origin-Resource-Policy".into(), "cross-origin".into())],
                b"globalThis.scriptCorpRan = true;".to_vec(),
            ),
            path => panic!("unexpected isolation fixture request: {path}"),
        };
        Ok(HttpResponse::new(200, "OK", headers, body))
    }
}

struct NavigationRedirectTransport(Arc<Mutex<Vec<String>>>);

impl HttpTransport for NavigationRedirectTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        self.0.lock().unwrap().push(request.url().to_string());
        let response = match request.url().path() {
            "/frame-redirect" => HttpResponse::new(
                302,
                "Found",
                vec![(
                    "Location".into(),
                    "https://other.example.test/frame-final".into(),
                )],
                Vec::new(),
            ),
            "/frame-final" => HttpResponse::new(
                200,
                "OK",
                vec![
                    ("Content-Type".into(), "text/html".into()),
                    ("Cross-Origin-Embedder-Policy".into(), "require-corp".into()),
                    ("Cross-Origin-Resource-Policy".into(), "cross-origin".into()),
                ],
                b"<p>allowed final response</p>".to_vec(),
            ),
            "/popup-redirect-coop" => HttpResponse::new(
                302,
                "Found",
                vec![
                    ("Location".into(), "/popup-final".into()),
                    ("Cross-Origin-Opener-Policy".into(), "same-origin".into()),
                ],
                Vec::new(),
            ),
            "/popup-final" => HttpResponse::new(
                200,
                "OK",
                vec![("Content-Type".into(), "text/html".into())],
                b"<p>open final response</p>".to_vec(),
            ),
            path => panic!("unexpected redirect navigation path: {path}"),
        };
        Ok(response)
    }
}

#[test]
fn top_level_capability_requires_secure_coop_coep_and_permission() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    assert!(
        runtime
            .eval("document.permissionsPolicy.allowsFeature('cross-origin-isolated')")
            .unwrap()
            .to_boolean()
    );
    assert!(!isolated(&mut runtime));
    runtime.install_isolation_policy(&isolated_headers());
    assert!(isolated(&mut runtime));

    let mut no_coep = runtime_for("https://isolated.example.test/page");
    no_coep
        .install_isolation_policy(&[("Cross-Origin-Opener-Policy".into(), "same-origin".into())]);
    assert!(!isolated(&mut no_coep));

    let mut disabled = runtime_for("https://isolated.example.test/page");
    let mut headers = isolated_headers();
    headers.push((
        "Permissions-Policy".into(),
        "cross-origin-isolated=()".into(),
    ));
    disabled.install_isolation_policy(&headers);
    assert!(!isolated(&mut disabled));

    let mut insecure = runtime_for("http://untrusted.example.test/page");
    insecure.install_isolation_policy(&isolated_headers());
    assert!(!isolated(&mut insecure));
}

#[test]
fn javascript_iframe_replacement_preserves_container_policy_and_isolation() {
    let document = crate::html::TreeBuilder::parse(
        "<body><iframe allow='geolocation' srcdoc='<p>old</p>'></iframe></body>",
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://isolated.example.test/page").unwrap();
    let mut headers = isolated_headers();
    headers.push(("Permissions-Policy".into(), "geolocation=()".into()));
    runtime.install_isolation_policy(&headers);
    let frame = runtime.document().query_selector("iframe").unwrap();
    let mut state = runtime.host_state.borrow_mut();
    let old_document = state.iframe_content_document(&frame).unwrap();
    let old_id = old_document.identity();
    let old_group = state.document_group_ids[&old_id];
    let old_declared_src_origin = state.iframe_documents[&frame.identity()]
        .declared_src_origin
        .clone();
    assert!(state.document_isolated_capabilities[&old_id]);
    assert!(!state.feature_enabled_in_document(old_id, PolicyFeature::Geolocation));

    let new_id = state
        .commit_javascript_iframe_document(frame.identity(), old_id, "<p>new</p>")
        .unwrap()
        .unwrap();
    assert_ne!(new_id, old_id);
    assert_eq!(state.document_group_ids[&new_id], old_group);
    assert_eq!(
        state.iframe_documents[&frame.identity()].declared_src_origin,
        old_declared_src_origin
    );
    assert!(state.document_isolated_capabilities[&new_id]);
    assert!(!state.feature_enabled_in_document(new_id, PolicyFeature::Geolocation));
}

#[test]
fn javascript_popup_replacement_preserves_policy_and_browsing_group() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    let mut headers = isolated_headers();
    headers.push(("Permissions-Policy".into(), "fullscreen=()".into()));
    runtime.install_isolation_policy(&headers);
    let mut state = runtime.host_state.borrow_mut();
    let owner_id = state.document.identity();
    let popup_id = state.open_auxiliary_context(owner_id, "stable").unwrap().0;
    let old_id = state.auxiliary_contexts[&popup_id].document.identity();
    let old_group = state.document_group_ids[&old_id];
    assert!(state.document_isolated_capabilities[&old_id]);
    assert!(state.fullscreen_allowed_for_document(old_id));

    let new_id = state
        .commit_javascript_auxiliary_document(popup_id, old_id, "<p>new</p>")
        .unwrap();
    assert_ne!(new_id, old_id);
    assert!(!state.auxiliary_contexts[&popup_id].initial_about_blank);
    assert_eq!(state.document_group_ids[&new_id], old_group);
    assert!(state.document_isolated_capabilities[&new_id]);
    assert!(state.fullscreen_allowed_for_document(new_id));
}

#[test]
fn isolated_parent_inherits_into_blank_and_requires_child_coep() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    runtime.install_isolation_policy(&isolated_headers());
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime
        .eval(
            r#"const blank = document.createElement('iframe');
               document.body.appendChild(blank);
               const child = document.createElement('iframe');
               child.src = '/child-isolated'; document.body.appendChild(child);
               const bad = document.createElement('iframe');
               bad.src = '/child-unisolated'; document.body.appendChild(bad);"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    assert!(
        runtime
            .eval(
                "blank.contentWindow.crossOriginIsolated && child.contentWindow.crossOriginIsolated"
            )
            .unwrap()
            .to_boolean()
    );
    let state = runtime.host_state.borrow();
    let entry = state
        .iframe_documents
        .get(
            &state
                .document
                .query_selector("iframe[src='/child-unisolated']")
                .unwrap()
                .identity(),
        )
        .unwrap();
    assert_eq!(entry.document_url, "about:blank");
    assert!(!state.document_isolated_capabilities[&entry.document.identity()]);
}

#[test]
fn about_blank_inherits_its_navigation_initiator_but_srcdoc_inherits_its_parent() {
    let mut runtime = runtime_for("https://parent.example.test/page");
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime
        .eval(
            "globalThis.frame=document.createElement('iframe'); \
             frame.src='https://child.example.test/child-isolated-csp'; \
             document.body.appendChild(frame);",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let frame = runtime.document().query_selector("iframe").unwrap();
    let (child_id, child_policy, child_origin, child_storage_origin, child_base) = {
        let state = runtime.host_state.borrow();
        let child_id = state.iframe_documents[&frame.identity()]
            .document
            .identity();
        let child_origin = state.document_security_origins[&child_id].clone();
        assert_ne!(
            child_origin,
            state.document_security_origins[&state.document.identity()]
        );
        assert!(
            !state.document_csp[&child_id].allows_reference_after_redirects(
                ResourceType::Connect,
                "https://child.example.test/api",
                0,
            )
        );
        (
            child_id,
            state.document_isolation_policies[&child_id].clone(),
            child_origin,
            state.document_origins[&child_id].clone(),
            state.base_url_for_document(child_id),
        )
    };
    assert_eq!(
        child_policy.embedder.value,
        cross_origin_isolation::EmbedderPolicyValue::RequireCorp
    );

    runtime
        .ensure_iframe_realm(frame.identity(), child_id)
        .unwrap();
    runtime
        .eval_in_document_realm(child_id, "location.href='about:blank'")
        .unwrap();
    runtime.run_until_idle().unwrap();
    let blank_id = runtime.host_state.borrow().iframe_documents[&frame.identity()]
        .document
        .identity();
    assert_ne!(blank_id, child_id);
    {
        let state = runtime.host_state.borrow();
        assert_eq!(
            state.iframe_documents[&frame.identity()].document_url,
            "about:blank"
        );
        assert_eq!(state.document_security_origins[&blank_id], child_origin);
        assert_eq!(state.document_origins[&blank_id], child_storage_origin);
        assert_eq!(
            state.document_isolation_policies[&blank_id].embedder,
            child_policy.embedder
        );
        assert_eq!(state.base_url_for_document(blank_id), child_base);
        assert!(
            !state.document_csp[&blank_id].allows_reference_after_redirects(
                ResourceType::Connect,
                "https://child.example.test/api",
                0,
            )
        );
    }

    runtime.eval("frame.srcdoc='<p>parent policy</p>'").unwrap();
    runtime.run_until_idle().unwrap();
    let srcdoc_id = runtime.host_state.borrow().iframe_documents[&frame.identity()]
        .document
        .identity();
    let state = runtime.host_state.borrow();
    assert_eq!(
        state.iframe_documents[&frame.identity()].document_url,
        "about:srcdoc"
    );
    assert_eq!(
        state.document_security_origins[&srcdoc_id],
        state.document_security_origins[&state.document.identity()]
    );
    assert_eq!(
        state.document_origins[&srcdoc_id],
        state.document_origins[&state.document.identity()]
    );
    assert_eq!(
        state.document_isolation_policies[&srcdoc_id],
        IsolationPolicy::default()
    );
    assert!(
        state.document_csp[&srcdoc_id].allows_reference_after_redirects(
            ResourceType::Connect,
            "https://child.example.test/api",
            0,
        )
    );
}

#[test]
fn data_iframe_inherits_initiator_coep_without_inheriting_its_origin() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    let mut headers = isolated_headers();
    headers.push((
        "Cross-Origin-Embedder-Policy-Report-Only".into(),
        "credentialless; report-to=\"monitor\"".into(),
    ));
    runtime.install_isolation_policy(&headers);
    runtime
        .eval(
            "const frame=document.createElement('iframe'); \
             frame.src='data:text/html,%3Cp%3Elocal%3C/p%3E'; \
             document.body.appendChild(frame);",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());

    let state = runtime.host_state.borrow();
    let owner_id = state.document.identity();
    let frame = state.document.query_selector("iframe").unwrap();
    let child = &state.iframe_documents[&frame.identity()];
    let child_id = child.document.identity();
    assert!(child.document_url.starts_with("data:text/html,"));
    assert!(matches!(
        state.document_security_origins.get(&child_id),
        Some(DocumentSecurityOrigin::Opaque(_))
    ));
    assert_eq!(
        state.document_isolation_policies[&child_id].embedder,
        state.document_isolation_policies[&owner_id].embedder
    );
    assert_eq!(
        state.document_isolation_policies[&child_id].opener,
        OpenerPolicy::default()
    );
}

#[test]
fn data_iframe_from_unisolated_initiator_is_blocked_by_isolated_embedder() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime
        .eval(
            "globalThis.outer=document.createElement('iframe'); \
             outer.src='/child-isolated'; document.body.appendChild(outer);",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let outer = runtime.document().query_selector("iframe").unwrap();
    let outer_id = runtime.host_state.borrow().iframe_documents[&outer.identity()]
        .document
        .identity();
    runtime
        .ensure_iframe_realm(outer.identity(), outer_id)
        .unwrap();
    runtime
        .eval_in_document_realm(
            outer_id,
            "const nested=document.createElement('iframe'); \
             nested.src='/child-isolated'; document.body.appendChild(nested);",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let nested_id = runtime.host_state.borrow().iframe_documents[&outer.identity()]
        .document
        .query_selector("iframe")
        .unwrap()
        .identity();

    runtime
        .eval(
            "outer.contentDocument.querySelector('iframe').contentWindow.location= \
             'data:text/html,%3Cp%3Eblocked%3C/p%3E';",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    let state = runtime.host_state.borrow();
    let nested = &state.iframe_documents[&nested_id];
    assert_eq!(nested.document_url, "about:blank");
    assert_eq!(
        state.document_isolation_policies[&nested.document.identity()],
        IsolationPolicy::default()
    );
}

#[test]
fn shared_worker_uses_its_own_coep_independent_of_creator() {
    let mut unisolated = runtime_for("https://isolated.example.test/page");
    unisolated.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    unisolated
        .eval(
            "globalThis.sharedResult='pending'; \
             globalThis.shared=new SharedWorker('/shared-worker-isolated.js','own-coep'); \
             shared.port.onmessage=e=>sharedResult=e.data; shared.port.start();",
        )
        .unwrap();
    unisolated.run_until_idle().unwrap();
    assert!(!isolated(&mut unisolated));
    assert!(
        unisolated
            .eval("sharedResult === true")
            .unwrap()
            .to_boolean()
    );

    let mut isolated_owner = runtime_for("https://isolated.example.test/page");
    isolated_owner.install_isolation_policy(&isolated_headers());
    isolated_owner.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    isolated_owner
        .eval(
            "globalThis.sharedResult='pending'; \
             globalThis.shared=new SharedWorker('/shared-worker-unisolated.js','unisolated-coep'); \
             shared.onerror=()=>sharedResult='blocked'; \
             shared.port.onmessage=e=>sharedResult=e.data; shared.port.start();",
        )
        .unwrap();
    isolated_owner.run_until_idle().unwrap();
    assert!(isolated(&mut isolated_owner));
    assert_eq!(
        isolated_owner
            .eval("sharedResult")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "1:false"
    );
}

#[test]
fn shared_worker_reuse_keeps_its_own_coep() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime
        .eval(
            "globalThis.results=[]; \
             addEventListener('message',e=>results.push(e.data)); \
             globalThis.first=new SharedWorker('/shared-worker-unisolated.js','reuse-coep'); \
             first.port.onmessage=e=>results.push(e.data); first.port.start(); \
             globalThis.frame=document.createElement('iframe'); \
             frame.src='/child-isolated'; document.body.appendChild(frame);",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let frame = runtime.document().query_selector("iframe").unwrap();
    let child_id = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&frame)
        .unwrap()
        .identity();
    runtime
        .eval_in_document_realm(
            child_id,
            "globalThis.second=new SharedWorker('/shared-worker-unisolated.js','reuse-coep'); \
             second.onerror=()=>parent.postMessage('blocked','*'); \
             second.port.onmessage=e=>parent.postMessage('connected:'+e.data,'*'); \
             second.port.start();",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("results.includes('1:false') && results.includes('connected:2:false') && !results.includes('blocked')")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn cross_origin_child_needs_both_parent_permission_and_iframe_delegation() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    let mut headers = isolated_headers();
    headers.push((
        "Permissions-Policy".into(),
        "cross-origin-isolated=(self \"https://child.example.test\" \"https://other.example.test\")".into(),
    ));
    runtime.install_isolation_policy(&headers);
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime
        .eval(
            r#"const blocked = document.createElement('iframe');
               blocked.src = 'https://child.example.test/child-isolated';
               document.body.appendChild(blocked);
               const delegated = document.createElement('iframe');
               delegated.setAttribute('allow', 'cross-origin-isolated *');
               delegated.src = 'https://child.example.test/child-isolated';
               document.body.appendChild(delegated);
               const redirected = document.createElement('iframe');
               redirected.setAttribute('allow', 'cross-origin-isolated');
               redirected.src = 'https://child.example.test/child-redirect';
               document.body.appendChild(redirected);"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    let state = runtime.host_state.borrow();
    let blocked_id = state.iframe_documents
        [&state.document.query_selector("iframe").unwrap().identity()]
        .document
        .identity();
    let delegated_id = state.iframe_documents[&state
        .document
        .query_selector("iframe[allow]")
        .unwrap()
        .identity()]
        .document
        .identity();
    let redirected_id = state.iframe_documents[&state
        .document
        .query_selector("iframe[src='https://child.example.test/child-redirect']")
        .unwrap()
        .identity()]
        .document
        .identity();
    assert!(!state.document_isolated_capabilities[&blocked_id]);
    assert!(state.document_isolated_capabilities[&delegated_id]);
    assert!(!state.document_isolated_capabilities[&redirected_id]);
}

#[test]
fn iframe_corp_blocks_an_unopted_in_redirect_before_following_it() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut isolated = runtime_for("https://owner.example.test/page");
    isolated.install_isolation_policy(&isolated_headers());
    isolated.set_http_resource_transport(Some(Arc::new(NavigationRedirectTransport(Arc::clone(
        &seen,
    )))));
    isolated
        .eval("const frame=document.createElement('iframe'); frame.src='https://other.example.test/frame-redirect'; document.body.appendChild(frame);")
        .unwrap();
    isolated.run_until_idle().unwrap();
    let state = isolated.host_state.borrow();
    let iframe = state.document.query_selector("iframe").unwrap();
    assert_eq!(
        state.iframe_documents[&iframe.identity()].document_url,
        "about:blank"
    );
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert!(seen.lock().unwrap()[0].ends_with("/frame-redirect"));
    drop(state);

    let mut permissive = runtime_for("https://owner.example.test/page");
    permissive.set_http_resource_transport(Some(Arc::new(NavigationRedirectTransport(
        Arc::clone(&seen),
    ))));
    permissive
        .eval("const frame=document.createElement('iframe'); frame.src='https://other.example.test/frame-redirect'; document.body.appendChild(frame);")
        .unwrap();
    permissive.run_until_idle().unwrap();
    let state = permissive.host_state.borrow();
    let iframe = state.document.query_selector("iframe").unwrap();
    assert_eq!(
        state.iframe_documents[&iframe.identity()].document_url,
        "https://other.example.test/frame-final"
    );
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[test]
fn popup_keeps_a_coop_group_switch_requested_by_a_redirect() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = runtime_for("https://owner.example.test/page");
    runtime.set_http_resource_transport(Some(Arc::new(NavigationRedirectTransport(Arc::clone(
        &seen,
    )))));
    runtime
        .eval("globalThis.popup=window.open('/popup-redirect-coop', 'redirected');")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.eval("popup.closed").unwrap().to_boolean());
    let state = runtime.host_state.borrow();
    let popup = state
        .auxiliary_contexts
        .values()
        .find(|entry| entry.name == "redirected")
        .unwrap();
    assert!(popup.opener_detached);
    assert_ne!(
        state.document_group_ids[&popup.document.identity()],
        state.document_group_ids[&state.document.identity()]
    );
    assert_eq!(popup.document_url, "https://owner.example.test/popup-final");
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[test]
fn popup_about_blank_uses_the_navigation_initiators_security_snapshot() {
    fn assert_inherited(
        state: &HostState,
        popup_id: u64,
        client: &NavigationSecuritySnapshot,
    ) -> usize {
        let document_id = state.auxiliary_contexts[&popup_id].document.identity();
        assert_eq!(state.document_origins[&document_id], client.storage_origin);
        assert_eq!(
            state.document_security_origins[&document_id],
            client.security_origin
        );
        assert_eq!(state.base_url_for_document(document_id), client.base_url);
        assert_eq!(
            state.document_isolation_policies[&document_id],
            client.isolation
        );
        assert!(state.feature_enabled_in_document(document_id, PolicyFeature::Geolocation));
        for url in [
            "https://a.example.test/connect",
            "https://b.example.test/connect",
        ] {
            assert_eq!(
                state.document_csp[&document_id].allows_reference_after_redirects(
                    ResourceType::Connect,
                    url,
                    0,
                ),
                client
                    .csp
                    .allows_reference_after_redirects(ResourceType::Connect, url, 0),
                "inherited CSP for {url}"
            );
        }
        document_id
    }

    let mut runtime = runtime_for("https://a.example.test/a/page");
    runtime.install_csp_policy(&["connect-src https://a.example.test".into()]);
    runtime.install_isolation_policy(&[
        ("Cross-Origin-Embedder-Policy".into(), "require-corp".into()),
        ("Permissions-Policy".into(), "geolocation=()".into()),
    ]);
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    let a_client = runtime
        .host_state
        .borrow()
        .navigation_security_snapshot(runtime.document().identity())
        .unwrap();
    runtime
        .eval("globalThis.popup=window.open('about:blank','initiator')")
        .unwrap();
    let popup_id = {
        let state = runtime.host_state.borrow();
        let popup_id = *state
            .auxiliary_contexts
            .iter()
            .find(|(_, entry)| entry.name == "initiator")
            .unwrap()
            .0;
        assert_inherited(&state, popup_id, &a_client);
        popup_id
    };

    runtime
        .eval("popup.location.href='https://b.example.test/popup-b-policy'")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    let (b_document_id, b_client) = {
        let state = runtime.host_state.borrow();
        let b_document_id = state.auxiliary_contexts[&popup_id].document.identity();
        let b_client = state.navigation_security_snapshot(b_document_id).unwrap();
        assert_ne!(b_client.security_origin, a_client.security_origin);
        assert_eq!(
            b_client.isolation.embedder.value,
            cross_origin_isolation::EmbedderPolicyValue::Credentialless
        );
        assert!(state.feature_enabled_in_document(b_document_id, PolicyFeature::Geolocation));
        (b_document_id, b_client)
    };

    runtime
        .eval_in_document_realm(b_document_id, "location.href='about:blank'")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    {
        let state = runtime.host_state.borrow();
        let blank_id = assert_inherited(&state, popup_id, &b_client);
        assert_ne!(blank_id, b_document_id);
        assert_eq!(
            state.auxiliary_contexts[&popup_id].document_url,
            "about:blank"
        );
    }

    runtime.eval("popup.location.href='about:blank'").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    let state = runtime.host_state.borrow();
    assert_inherited(&state, popup_id, &a_client);
    assert_eq!(
        state.auxiliary_contexts[&popup_id].document_url,
        "about:blank"
    );
}

#[test]
fn named_popup_about_blank_reuse_navigates_but_new_query_keeps_initial_document() {
    let mut runtime = runtime_for("https://a.example.test/page");
    runtime.install_csp_policy(&["connect-src https://a.example.test".into()]);
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime
        .eval("globalThis.popup=window.open('/popup-b-policy','reuse')")
        .unwrap();
    runtime.run_until_idle().unwrap();
    let (popup_id, old_document_id) = {
        let state = runtime.host_state.borrow();
        let (&id, entry) = state
            .auxiliary_contexts
            .iter()
            .find(|(_, entry)| entry.name == "reuse")
            .unwrap();
        (id, entry.document.identity())
    };
    runtime.eval("window.open('about:blank','reuse')").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    {
        let state = runtime.host_state.borrow();
        let entry = &state.auxiliary_contexts[&popup_id];
        let document_id = entry.document.identity();
        assert_ne!(document_id, old_document_id);
        assert_eq!(entry.document_url, "about:blank");
        assert_eq!(entry.opener_document_id, Some(state.document.identity()));
        assert_eq!(
            state.document_security_origins[&document_id],
            state.document_security_origins[&state.document.identity()]
        );
        assert!(
            !state.document_csp[&document_id].allows_reference_after_redirects(
                ResourceType::Connect,
                "https://b.example.test/api",
                0
            )
        );
    }
    runtime
        .eval("globalThis.other=window.open('/popup-b-policy','other')")
        .unwrap();
    runtime.run_until_idle().unwrap();
    let other_document_id = {
        let state = runtime.host_state.borrow();
        state
            .auxiliary_contexts
            .values()
            .find(|entry| entry.name == "other")
            .unwrap()
            .document
            .identity()
    };
    let reused_document_id = runtime.host_state.borrow().auxiliary_contexts[&popup_id]
        .document
        .identity();
    runtime
        .eval_in_document_realm(other_document_id, "window.open('', 'reuse')")
        .unwrap();
    {
        let state = runtime.host_state.borrow();
        assert_eq!(
            state.auxiliary_contexts[&popup_id].opener_document_id,
            Some(other_document_id)
        );
        assert_eq!(
            state.auxiliary_contexts[&popup_id].document.identity(),
            reused_document_id
        );
    }
    runtime
        .eval("globalThis.queried=window.open('about:blank?fresh#part','fresh')")
        .unwrap();
    let state = runtime.host_state.borrow();
    let entry = state
        .auxiliary_contexts
        .values()
        .find(|entry| entry.name == "fresh")
        .unwrap();
    assert!(entry.initial_about_blank);
    assert_eq!(entry.document_url, "about:blank?fresh#part");
    assert_eq!(
        state.document_urls[&entry.document.identity()],
        entry.document_url
    );
}

#[test]
fn named_popup_reuse_sets_opener_from_entry_scripts_document_realm() {
    let mut runtime = runtime_for("https://a.example.test/page");
    runtime
        .eval("globalThis.first=window.open('', 'first'); globalThis.target=window.open('', 'target'); first.open('', 'target')")
        .unwrap();
    assert!(
        runtime
            .eval("target.opener === window")
            .unwrap()
            .to_boolean()
    );
    let (first_id, target_id) = {
        let state = runtime.host_state.borrow();
        let first_id = state
            .auxiliary_contexts
            .values()
            .find(|entry| entry.name == "first")
            .unwrap()
            .document
            .identity();
        let target_id = state
            .auxiliary_contexts
            .values()
            .find(|entry| entry.name == "target")
            .unwrap()
            .document
            .identity();
        assert_eq!(
            state
                .auxiliary_contexts
                .values()
                .find(|entry| entry.name == "target")
                .unwrap()
                .opener_document_id,
            Some(state.document.identity())
        );
        (first_id, target_id)
    };
    runtime
        .eval_in_document_realm(first_id, "window.open('', 'target')")
        .unwrap();
    assert!(
        runtime
            .eval("target.opener === first")
            .unwrap()
            .to_boolean()
    );
    let state = runtime.host_state.borrow();
    let target = state
        .auxiliary_contexts
        .values()
        .find(|entry| entry.name == "target")
        .unwrap();
    assert_eq!(target.document.identity(), target_id);
    assert_eq!(target.opener_document_id, Some(first_id));
    assert!(target.window_proxy.is_some());
}

#[test]
fn cross_origin_named_popup_reuse_preserves_window_proxy_identity() {
    let mut runtime = runtime_for("https://a.example.test/page");
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime
        .eval("globalThis.__ownerSecret='owner secret'; globalThis.target=window.open('', 'target'); globalThis.source=window.open('https://b.example.test/popup-b-policy', 'source')")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    runtime
        .eval("target.addEventListener('message', event => { globalThis.crossOriginMessage={data:event.data, origin:event.origin, source:event.source===source}; })")
        .unwrap();
    let (target_id, source_id) = {
        let state = runtime.host_state.borrow();
        let target = state
            .auxiliary_contexts
            .values()
            .find(|entry| entry.name == "target")
            .unwrap();
        let source = state
            .auxiliary_contexts
            .values()
            .find(|entry| entry.name == "source")
            .unwrap();
        (target.document.identity(), source.document.identity())
    };
    runtime
        .eval_in_document_realm(
            source_id,
            concat!(
                "globalThis.crossRef=window.open('', 'target'); ",
                "if (crossRef.postMessage.constructor('return globalThis.__ownerSecret')() === 'owner secret') ",
                "  throw new Error('cross-origin method leaked the owner Realm'); ",
                "try { ",
                "  if (crossRef.location.constructor.constructor('return globalThis.__ownerSecret')() === 'owner secret') ",
                "    throw new Error('cross-origin location leaked the owner Realm'); ",
                "} catch (error) { if (error.name !== 'SecurityError') throw error; } ",
                "const methodDescriptor=Object.getOwnPropertyDescriptor(crossRef, 'postMessage'); ",
                "if (methodDescriptor.value.constructor('return globalThis.__ownerSecret')() === 'owner secret' || ",
                "    methodDescriptor.constructor.constructor('return globalThis.__ownerSecret')() === 'owner secret') ",
                "  throw new Error('cross-origin descriptor leaked the owner Realm'); ",
                "const fallbackDescriptor=Object.getOwnPropertyDescriptor(crossRef, 'then'); ",
                "if (fallbackDescriptor.constructor.constructor('return globalThis.__ownerSecret')() === 'owner secret') ",
                "  throw new Error('cross-origin fallback descriptor leaked the owner Realm'); ",
                "try { void crossRef.document; throw new Error('cross-origin document exposed'); } ",
                "catch (error) { ",
                "  if (error.name !== 'SecurityError') throw error; ",
                "  if (error.constructor.constructor('return globalThis.__ownerSecret')() === 'owner secret') ",
                "    throw new Error('cross-origin exception leaked the owner Realm'); ",
                "} ",
                "try { void crossRef.location.href; throw new Error('cross-origin location exposed'); } ",
                "catch (error) { if (error.name !== 'SecurityError') throw error; } ",
                "crossRef.postMessage('from-b', '*');",
            ),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    let message = runtime
        .eval("JSON.stringify(crossOriginMessage)")
        .unwrap()
        .to_string(&mut runtime.context)
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        message,
        r#"{"data":"from-b","origin":"https://b.example.test","source":true}"#
    );
    assert!(
        runtime
            .eval("target.document.defaultView === target && target.window === target && target.opener !== window && typeof target.opener.postMessage === 'function'")
            .unwrap()
            .to_boolean()
    );
    let state = runtime.host_state.borrow();
    let target = state
        .auxiliary_contexts
        .values()
        .find(|entry| entry.name == "target")
        .unwrap();
    assert_eq!(target.document.identity(), target_id);
    assert!(target.window_proxy.is_some());
    assert_eq!(target.opener_document_id, Some(source_id));
}

#[test]
fn popup_window_proxy_survives_creator_iframe_navigation_and_named_reuse() {
    let mut runtime = runtime_for("https://a.example.test/page");
    runtime
        .eval("globalThis.frame=document.createElement('iframe'); document.body.appendChild(frame); frame.contentDocument")
        .unwrap();
    let creator_document_id = {
        let state = runtime.host_state.borrow();
        let frame_id = state.document.query_selector("iframe").unwrap().identity();
        state.iframe_documents[&frame_id].document.identity()
    };
    runtime
        .eval_in_document_realm(
            creator_document_id,
            "globalThis.createdPopup=window.open('', 'surviving-popup')",
        )
        .unwrap();
    runtime
        .eval("globalThis.popup=frame.contentWindow.createdPopup")
        .unwrap();
    assert!(
        runtime
            .eval("popup.document.defaultView === popup")
            .unwrap()
            .to_boolean()
    );

    runtime
        .eval("frame.src='data:text/html,<body>replacement</body>'")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    assert!(
        runtime
            .host_state
            .borrow()
            .iframe_navigation
            .owner(creator_document_id)
            .is_none()
    );
    boa_gc::force_collect();
    runtime
        .eval("globalThis.reused=window.open('', 'surviving-popup')")
        .unwrap();
    assert!(
        runtime
            .eval(
                "reused === popup && popup.document.defaultView === popup && popup.window === popup"
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn retired_creator_realm_can_still_read_write_and_message_its_popup() {
    let mut runtime = runtime_for("https://a.example.test/page");
    runtime
        .eval("globalThis.frame=document.createElement('iframe'); document.body.appendChild(frame); frame.contentDocument")
        .unwrap();
    let creator_document_id = {
        let state = runtime.host_state.borrow();
        let frame_id = state.document.query_selector("iframe").unwrap().identity();
        state.iframe_documents[&frame_id].document.identity()
    };
    runtime
        .eval_in_document_realm(
            creator_document_id,
            concat!(
                "globalThis.oldPopup=window.open('', 'retired-creator-target'); ",
                "oldPopup.document.body.textContent='popup marker'; ",
                "oldPopup.addEventListener('message', event => { ",
                "  oldPopup.document.body.dataset.message=event.data; ",
                "  oldPopup.document.body.dataset.origin=event.origin; ",
                "}); ",
                "globalThis.readOldPopup=() => oldPopup.closed ? 'closed' : oldPopup.document.body.textContent; ",
                "globalThis.writeOldPopup=() => { oldPopup.retiredMarker='retired write'; return oldPopup.retiredMarker; }; ",
                "globalThis.messageOldPopup=() => oldPopup.postMessage('retired message', '*');",
            ),
        )
        .unwrap();
    runtime
        .eval("globalThis.oldRead=frame.contentWindow.readOldPopup; globalThis.oldWrite=frame.contentWindow.writeOldPopup; globalThis.oldMessage=frame.contentWindow.messageOldPopup")
        .unwrap();
    runtime
        .eval("frame.src='data:text/html,<body>replacement</body>'")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    assert_eq!(
        runtime
            .eval("oldRead()")
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped(),
        "popup marker"
    );
    assert_eq!(
        runtime
            .eval("oldWrite()")
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped(),
        "retired write"
    );
    runtime.eval("oldMessage()").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    assert!(
        runtime
            .eval("(() => { const popup=window.open('', 'retired-creator-target'); return popup.document.body.dataset.message === 'retired message' && popup.document.body.dataset.origin === 'https://a.example.test'; })()")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn named_popup_reuse_preserves_author_opener_properties() {
    let mut runtime = runtime_for("https://a.example.test/page");
    runtime
        .eval("globalThis.popup=window.open('', 'stable'); popup.opener='sentinel'; window.open('', 'stable')")
        .unwrap();
    assert!(
        runtime
            .eval("popup.opener === 'sentinel'")
            .unwrap()
            .to_boolean()
    );
    runtime.eval("delete popup.opener").unwrap();
    assert!(
        runtime
            .eval("popup.opener === window")
            .unwrap()
            .to_boolean()
    );

    runtime
        .eval("Object.defineProperty(popup, 'opener', {value:'locked', writable:false, configurable:true}); window.open('', 'stable')")
        .unwrap();
    assert!(
        runtime
            .eval("popup.opener === 'locked'")
            .unwrap()
            .to_boolean()
    );
    runtime.eval("delete popup.opener").unwrap();
    assert!(
        runtime
            .eval("popup.opener === window")
            .unwrap()
            .to_boolean()
    );

    runtime
        .eval("globalThis.savedPopupView = popup.document.defaultView; popup.opener = null")
        .unwrap();
    assert!(runtime.eval("popup.opener === null").unwrap().to_boolean());
    assert!(
        runtime
            .eval("popup.document.defaultView === savedPopupView && savedPopupView === popup && popup.window === popup")
            .unwrap()
            .to_boolean()
    );
    let popup_id = {
        let state = runtime.host_state.borrow();
        *state
            .auxiliary_contexts
            .iter()
            .find(|(_, entry)| entry.name == "stable")
            .unwrap()
            .0
    };
    assert_eq!(
        runtime.host_state.borrow().auxiliary_contexts[&popup_id].opener_document_id,
        None
    );
    assert!(
        runtime.host_state.borrow().auxiliary_contexts[&popup_id]
            .window_proxy
            .is_some()
    );
    runtime.eval("window.open('', 'stable')").unwrap();
    assert!(
        runtime
            .eval("popup.opener === window")
            .unwrap()
            .to_boolean()
    );
    assert_eq!(
        runtime.host_state.borrow().auxiliary_contexts[&popup_id].opener_document_id,
        Some(runtime.document().identity())
    );
}

#[test]
fn local_popup_navigation_clones_policy_container_without_inheriting_coop() {
    let mut runtime = runtime_for("https://a.example.test/page");
    runtime.install_response_security_headers(&[("Referrer-Policy".into(), "no-referrer".into())]);
    runtime.install_csp_policy(&["connect-src 'self'".into()]);
    runtime.install_isolation_policy(&isolated_headers());
    let owner_id = runtime.document().identity();
    let (popup_id, initial_id) = {
        let mut state = runtime.host_state.borrow_mut();
        let (popup_id, created) = state.open_auxiliary_context(owner_id, "local").unwrap();
        assert!(created);
        let initial_id = state.auxiliary_contexts[&popup_id].document.identity();
        assert_eq!(
            state.document_isolation_policies[&initial_id].opener.value,
            OpenerPolicyValue::SameOriginPlusCoep
        );
        (popup_id, initial_id)
    };
    for url in ["about:blank", "data:text/html,<p>opaque</p>"] {
        let mut state = runtime.host_state.borrow_mut();
        let source = state.navigation_referrer_source_for_document(owner_id);
        let client = state.navigation_security_snapshot(owner_id).unwrap();
        let prepared = state
            .prepare_auxiliary_navigation(popup_id, url, &source, &client)
            .unwrap();
        let next_id = state.commit_auxiliary_navigation(prepared).unwrap();
        assert_ne!(next_id, initial_id);
        assert_eq!(
            state.document_isolation_policies[&next_id].opener.value,
            OpenerPolicyValue::UnsafeNone
        );
        assert_eq!(
            state.document_isolation_policies[&next_id].embedder,
            client.isolation.embedder
        );
        assert_eq!(
            state.document_reporting[&next_id].referrer_policy,
            source.policy
        );
        assert_eq!(
            state.document_csp[&next_id].allows_reference_after_redirects(
                ResourceType::Connect,
                "https://a.example.test/api",
                0
            ),
            !url.starts_with("data:")
        );
        if url.starts_with("data:") {
            assert_eq!(state.document_origins[&next_id], None);
            assert!(matches!(
                state.document_security_origins[&next_id],
                DocumentSecurityOrigin::Opaque(_)
            ));
        } else {
            assert_eq!(
                state.document_security_origins[&next_id],
                client.security_origin
            );
        }
    }
}

#[test]
fn cross_origin_iframe_creator_does_not_inherit_top_level_coop_into_popup() {
    let mut runtime = runtime_for("https://a.example.test/page");
    runtime.install_isolation_policy(&isolated_headers());
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime.eval("const child=document.createElement('iframe'); child.src='https://b.example.test/child-isolated'; document.body.appendChild(child)").unwrap();
    runtime.run_until_idle().unwrap();
    let mut state = runtime.host_state.borrow_mut();
    let child_id = state.iframe_documents
        [&state.document.query_selector("iframe").unwrap().identity()]
        .document
        .identity();
    let (popup_id, created) = state
        .open_auxiliary_context(child_id, "child-popup")
        .unwrap();
    assert!(created);
    let popup_document_id = state.auxiliary_contexts[&popup_id].document.identity();
    assert_eq!(
        state.document_isolation_policies[&popup_document_id]
            .opener
            .value,
        OpenerPolicyValue::UnsafeNone
    );
    assert_eq!(
        state.document_isolation_policies[&popup_document_id].embedder,
        state.document_isolation_policies[&child_id].embedder
    );
}

#[test]
fn popup_coop_switch_detaches_opener_but_open_popup_keeps_it() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime
        .eval("globalThis.cut = window.open('/popup-isolated', 'cut');")
        .expect("the first popup opens");
    assert_eq!(
        runtime
            .eval("typeof window.open")
            .expect("the opener remains live")
            .as_string()
            .expect("typeof returns a string")
            .to_std_string_escaped(),
        "function"
    );
    runtime
        .eval("globalThis.kept = window.open('/popup-open', 'kept');")
        .expect("the second popup opens");
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    assert!(
        runtime
            .eval("cut.closed && !kept.closed")
            .unwrap()
            .to_boolean()
    );
    let state = runtime.host_state.borrow();
    let cut = state
        .auxiliary_contexts
        .values()
        .find(|entry| entry.name == "cut")
        .unwrap();
    let kept = state
        .auxiliary_contexts
        .values()
        .find(|entry| entry.name == "kept")
        .unwrap();
    assert!(cut.opener_detached);
    assert_ne!(
        state.document_group_ids[&cut.document.identity()],
        state.document_group_ids[&state.document.identity()]
    );
    assert!(state.document_isolated_capabilities[&cut.document.identity()]);
    assert!(!kept.opener_detached);
    let cut_realm = cut.realm.clone().expect("the isolated popup has a Realm");
    let cut_url = cut.document_url.clone();
    drop(state);
    let previous = runtime.context.enter_realm(cut_realm.clone());
    let self_access = runtime.eval(
        "window === self && window.closed === false && opener === null && crossOriginIsolated",
    );
    runtime.context.enter_realm(previous);
    assert!(self_access.unwrap().to_boolean());

    let write = runtime
        .eval("try { cut.__crossGroupWrite = 1; 'allowed' } catch (error) { error.name }")
        .unwrap();
    assert_eq!(
        write.as_string().unwrap().to_std_string_escaped(),
        "SecurityError"
    );
    let previous = runtime.context.enter_realm(cut_realm);
    let popup_was_unchanged = runtime
        .eval("typeof window.__crossGroupWrite === 'undefined'")
        .unwrap()
        .to_boolean();
    runtime.context.enter_realm(previous);
    assert!(popup_was_unchanged);

    runtime.eval("cut.location.href='/popup-open'").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .host_state
            .borrow()
            .auxiliary_contexts
            .values()
            .find(|entry| entry.name == "cut")
            .unwrap()
            .document_url,
        cut_url
    );

    runtime.eval("cut.close()").unwrap();
    assert!(
        runtime
            .host_state
            .borrow()
            .auxiliary_contexts
            .values()
            .any(|entry| entry.name == "cut")
    );
}

#[test]
fn queued_message_does_not_enter_a_popup_after_its_coop_group_switch() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime
        .eval("globalThis.popup=window.open('', 'queued'); popup.postMessage('stale', '*');")
        .unwrap();
    let (id, client_document_id, source_group_id, client_security, source) = {
        let state = runtime.host_state.borrow();
        let client_document_id = state.document.identity();
        let id = *state
            .auxiliary_contexts
            .iter()
            .find(|(_, entry)| entry.name == "queued")
            .unwrap()
            .0;
        (
            id,
            client_document_id,
            state.document_group_ids[&client_document_id],
            state.navigation_security_snapshot(client_document_id),
            state.navigation_referrer_source_for_document(client_document_id),
        )
    };
    runtime
        .run_auxiliary_navigation(
            id,
            "/popup-isolated-message",
            client_document_id,
            source_group_id,
            client_security,
            None,
            source,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    assert!(runtime.eval("popup.closed").unwrap().to_boolean());
    let realm = runtime.host_state.borrow().auxiliary_contexts[&id]
        .realm
        .clone()
        .unwrap();
    let previous = runtime.context.enter_realm(realm);
    let delivered = runtime.eval("received.length").unwrap().as_number();
    runtime.context.enter_realm(previous);
    assert_eq!(delivered, Some(0.0));
}

#[test]
fn queued_auxiliary_navigation_does_not_cross_a_coop_group_switch() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime
        .eval("globalThis.popup=window.open('', 'queued-navigation'); popup.location.href='/popup-open'")
        .unwrap();
    let (id, client_document_id, source_group_id, client_security, referrer_source) = {
        let state = runtime.host_state.borrow();
        let client_document_id = state.document.identity();
        let id = *state
            .auxiliary_contexts
            .iter()
            .find(|(_, entry)| entry.name == "queued-navigation")
            .unwrap()
            .0;
        (
            id,
            client_document_id,
            state.document_group_ids[&client_document_id],
            state.navigation_security_snapshot(client_document_id),
            state.navigation_referrer_source_for_document(client_document_id),
        )
    };
    runtime
        .run_auxiliary_navigation(
            id,
            "/popup-isolated-message",
            client_document_id,
            source_group_id,
            client_security,
            None,
            referrer_source,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    let state = runtime.host_state.borrow();
    let popup = &state.auxiliary_contexts[&id];
    assert_eq!(
        popup.document_url,
        "https://isolated.example.test/popup-isolated-message"
    );
    assert_ne!(
        state.document_group_ids[&popup.document.identity()],
        source_group_id
    );
}

#[test]
fn queued_auxiliary_navigation_survives_source_popup_close() {
    let mut runtime = runtime_for("https://a.example.test/page");
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime
        .eval("globalThis.target=window.open('', 'target'); globalThis.source=window.open('', 'source')")
        .unwrap();
    let source_document_id = {
        let state = runtime.host_state.borrow();
        state
            .auxiliary_contexts
            .values()
            .find(|entry| entry.name == "source")
            .unwrap()
            .document
            .identity()
    };
    runtime
        .eval_in_document_realm(
            source_document_id,
            "opener.target.location.href='/popup-open'; close()",
        )
        .unwrap();
    assert!(
        runtime
            .host_state
            .borrow()
            .auxiliary_contexts
            .values()
            .all(|entry| entry.name != "source")
    );
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    let state = runtime.host_state.borrow();
    let target = state
        .auxiliary_contexts
        .values()
        .find(|entry| entry.name == "target")
        .unwrap();
    assert_eq!(target.document_url, "https://a.example.test/popup-open");
}

#[test]
fn coop_navigation_reports_use_old_and_new_document_endpoints_and_realms() {
    #[derive(Clone)]
    struct ReportingTransport(Arc<Mutex<Vec<(String, serde_json::Value)>>>);

    impl HttpTransport for ReportingTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            let path = request.url().path();
            match (request.method(), path) {
                (crate::http::Method::Get, "/popup-old") => Ok(HttpResponse::new(
                    200,
                    "OK",
                    vec![
                        ("Content-Type".into(), "text/html".into()),
                        (
                            "Cross-Origin-Opener-Policy".into(),
                            "same-origin-allow-popups; report-to=\"old\"".into(),
                        ),
                        ("Reporting-Endpoints".into(), "old=\"/report-old\"".into()),
                    ],
                    br#"<script>
                        new ReportingObserver(batch => {
                          opener.oldCoopReports = (opener.oldCoopReports || []).concat(
                            batch.map(report => ({type: report.type, url: report.url, body: report.body})));
                        }, {types: ['coop']}).observe();
                        location.href = '/popup-new';
                    </script>"#
                        .to_vec(),
                )),
                (crate::http::Method::Get, "/popup-new") => {
                    assert_eq!(
                        request.header("Referer"),
                        Some("https://isolated.example.test/popup-old")
                    );
                    Ok(HttpResponse::new(
                        200,
                        "OK",
                        vec![
                            ("Content-Type".into(), "text/html".into()),
                            (
                                "Cross-Origin-Opener-Policy".into(),
                                "same-origin; report-to=\"new\"".into(),
                            ),
                            ("Reporting-Endpoints".into(), "new=\"/report-new\"".into()),
                        ],
                        br#"<script>
                            globalThis.newCoopReports = [];
                            new ReportingObserver(batch => newCoopReports.push(...batch.map(
                              report => ({type: report.type, url: report.url, body: report.body}))),
                              {types: ['coop'], buffered: true}).observe();
                        </script>"#
                            .to_vec(),
                    ))
                }
                (crate::http::Method::Post, "/report-old" | "/report-new") => {
                    let report = serde_json::from_slice(request.body().unwrap()).unwrap();
                    self.0.lock().unwrap().push((path.to_owned(), report));
                    Ok(HttpResponse::new(204, "No Content", Vec::new(), Vec::new()))
                }
                (method, path) => panic!("unexpected COOP request: {method:?} {path}"),
            }
        }
    }

    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = runtime_for("https://isolated.example.test/main");
    runtime.install_isolation_policy(&[(
        "Cross-Origin-Opener-Policy".into(),
        "same-origin-allow-popups".into(),
    )]);
    runtime.set_http_resource_transport(Some(Arc::new(ReportingTransport(Arc::clone(&sent)))));
    runtime
        .eval("globalThis.oldCoopReports=[]; globalThis.popup=window.open('/popup-old','reporting-popup');")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    assert!(runtime.eval("popup.closed").unwrap().to_boolean());
    let old = runtime
        .eval("JSON.stringify(oldCoopReports)")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    let old: serde_json::Value = serde_json::from_str(&old).unwrap();
    assert_eq!(old.as_array().unwrap().len(), 1);
    assert_eq!(old[0]["url"], "https://isolated.example.test/popup-old");
    assert_eq!(old[0]["body"]["type"], "navigation-from-response");
    assert_eq!(
        old[0]["body"]["nextResponseURL"],
        "https://isolated.example.test/popup-new"
    );
    let (new_realm, opener_detached) = {
        let state = runtime.host_state.borrow();
        let popup = state
            .auxiliary_contexts
            .values()
            .find(|entry| entry.name == "reporting-popup")
            .unwrap();
        (popup.realm.clone().unwrap(), popup.opener_detached)
    };
    assert!(opener_detached);
    let previous = runtime.context.enter_realm(new_realm);
    let new = runtime
        .eval(
            "JSON.stringify({load: document.readyState, opener: opener, reports: newCoopReports})",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    runtime.context.enter_realm(previous);
    let new: serde_json::Value = serde_json::from_str(&new).unwrap();
    assert_eq!(new["load"], "complete");
    assert!(new["opener"].is_null());
    assert_eq!(new["reports"].as_array().unwrap().len(), 1);
    assert_eq!(
        new["reports"][0]["url"],
        "https://isolated.example.test/popup-new"
    );
    assert_eq!(new["reports"][0]["body"]["type"], "navigation-to-response");
    assert_eq!(
        new["reports"][0]["body"]["previousResponseURL"],
        "https://isolated.example.test/popup-old"
    );
    assert_eq!(
        new["reports"][0]["body"]["referrer"],
        "https://isolated.example.test/popup-old"
    );
    runtime
        .host_state
        .borrow_mut()
        .report_delivery
        .as_mut()
        .unwrap()
        .wait_for_pending();
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert!(
        sent.iter()
            .any(|(path, report)| path == "/report-old" && report[0]["body"] == old[0]["body"])
    );
    assert!(
        sent.iter().any(|(path, report)| path == "/report-new"
            && report[0]["body"] == new["reports"][0]["body"])
    );
}

#[test]
fn report_only_coop_queues_a_report_without_cutting_the_popup_group() {
    #[derive(Clone)]
    struct MonitoringTransport(Arc<Mutex<Vec<serde_json::Value>>>);

    impl HttpTransport for MonitoringTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            match (request.method(), request.url().path()) {
                (crate::http::Method::Get, "/monitor-old") => Ok(HttpResponse::new(
                    200,
                    "OK",
                    vec![
                        ("Content-Type".into(), "text/html".into()),
                        (
                            "Cross-Origin-Opener-Policy-Report-Only".into(),
                            "same-origin; report-to=\"monitor\"".into(),
                        ),
                        (
                            "Reporting-Endpoints".into(),
                            "monitor=\"/monitor-report\"".into(),
                        ),
                    ],
                    br#"<script>
                        new ReportingObserver(batch => opener.monitored = batch.map(
                          report => ({url: report.url, body: report.body})),
                          {types: ['coop']}).observe();
                        location.href = '/monitor-new';
                    </script>"#
                        .to_vec(),
                )),
                (crate::http::Method::Get, "/monitor-new") => Ok(HttpResponse::new(
                    200,
                    "OK",
                    vec![("Content-Type".into(), "text/html".into())],
                    b"<body>kept</body>".to_vec(),
                )),
                (crate::http::Method::Post, "/monitor-report") => {
                    self.0
                        .lock()
                        .unwrap()
                        .push(serde_json::from_slice(request.body().unwrap()).unwrap());
                    Ok(HttpResponse::new(204, "No Content", Vec::new(), Vec::new()))
                }
                (method, path) => panic!("unexpected COOP monitor request: {method:?} {path}"),
            }
        }
    }

    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = runtime_for("https://isolated.example.test/main");
    runtime.set_http_resource_transport(Some(Arc::new(MonitoringTransport(Arc::clone(&sent)))));
    runtime
        .eval("globalThis.monitored=[]; globalThis.popup=window.open('/monitor-old','monitor');")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    assert!(
        runtime
            .eval("!popup.closed && popup.opener === window")
            .unwrap()
            .to_boolean()
    );
    let observed = runtime
        .eval("JSON.stringify(monitored)")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    let observed: serde_json::Value = serde_json::from_str(&observed).unwrap();
    assert_eq!(observed.as_array().unwrap().len(), 1);
    assert_eq!(
        observed[0]["url"],
        "https://isolated.example.test/monitor-old"
    );
    assert_eq!(observed[0]["body"]["disposition"], "reporting");
    assert_eq!(observed[0]["body"]["type"], "navigation-from-response");
    runtime
        .host_state
        .borrow_mut()
        .report_delivery
        .as_mut()
        .unwrap()
        .wait_for_pending();
    let sent = sent.lock().unwrap();
    let reports = sent
        .iter()
        .flat_map(|batch| batch.as_array().unwrap().iter())
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 2);
    assert!(
        reports
            .iter()
            .any(|report| report["body"] == observed[0]["body"])
    );
    assert!(
        reports
            .iter()
            .any(|report| { report["body"]["type"] == "navigation-to-response" })
    );
}

#[test]
fn redirect_report_only_coop_uses_the_redirects_url_and_endpoint() {
    #[derive(Clone)]
    struct RedirectReportingTransport(Arc<Mutex<Vec<(String, serde_json::Value)>>>, bool);

    impl HttpTransport for RedirectReportingTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            match (request.method(), request.url().path()) {
                (crate::http::Method::Get, "/popup-old") => Ok(HttpResponse::new(
                    200,
                    "OK",
                    vec![("Content-Type".into(), "text/html".into())],
                    b"<body>old popup</body>".to_vec(),
                )),
                (crate::http::Method::Get, "/hop") => Ok(HttpResponse::new(
                    302,
                    "Found",
                    vec![
                        ("Location".into(), "/final".into()),
                        (
                            "Cross-Origin-Opener-Policy-Report-Only".into(),
                            if self.1 {
                                "same-origin-allow-popups; report-to=\"coop\""
                            } else {
                                "same-origin; report-to=\"coop\""
                            }
                            .into(),
                        ),
                        ("Reporting-Endpoints".into(), "coop=\"/hop-report\"".into()),
                    ],
                    Vec::new(),
                )),
                (crate::http::Method::Get, "/final") => Ok(HttpResponse::new(
                    200,
                    "OK",
                    if self.1 {
                        vec![
                            ("Content-Type".into(), "text/html".into()),
                            (
                                "Cross-Origin-Opener-Policy-Report-Only".into(),
                                "same-origin; report-to=\"final\"".into(),
                            ),
                            (
                                "Reporting-Endpoints".into(),
                                "final=\"/final-report\"".into(),
                            ),
                        ]
                    } else {
                        vec![("Content-Type".into(), "text/html".into())]
                    },
                    b"<body>final popup</body>".to_vec(),
                )),
                (crate::http::Method::Post, "/hop-report" | "/final-report") => {
                    self.0.lock().unwrap().push((
                        request.url().path().to_string(),
                        serde_json::from_slice(request.body().unwrap()).unwrap(),
                    ));
                    Ok(HttpResponse::new(204, "No Content", Vec::new(), Vec::new()))
                }
                (method, path) => panic!("unexpected COOP redirect request: {method:?} {path}"),
            }
        }
    }

    for access_reports in [false, true] {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let mut runtime = runtime_for("https://isolated.example.test/main");
        runtime.set_http_resource_transport(Some(Arc::new(RedirectReportingTransport(
            Arc::clone(&sent),
            access_reports,
        ))));
        runtime
            .eval("globalThis.popup = window.open('/popup-old', 'redirect-reporting');")
            .unwrap();
        runtime.run_until_idle().unwrap();
        let (popup_id, actual_group, virtual_group) = {
            let state = runtime.host_state.borrow();
            let (id, popup) = state
                .auxiliary_contexts
                .iter()
                .find(|(_, entry)| entry.name == "redirect-reporting")
                .unwrap();
            (
                *id,
                state.document_group_ids[&popup.document.identity()],
                popup.virtual_group_id,
            )
        };
        runtime.eval("popup.location.href = '/hop';").unwrap();
        runtime.run_until_idle().unwrap();
        assert!(runtime.take_task_errors().is_empty());
        {
            let state = runtime.host_state.borrow();
            let popup = &state.auxiliary_contexts[&popup_id];
            assert_eq!(
                state.document_group_ids[&popup.document.identity()],
                actual_group
            );
            assert_ne!(popup.virtual_group_id, virtual_group);
            assert_ne!(popup.virtual_group_id, state.main_virtual_group_id);
        }
        assert!(
            runtime
                .eval("!popup.closed && popup.opener === window")
                .unwrap()
                .to_boolean()
        );
        if let Some(delivery) = runtime.host_state.borrow_mut().report_delivery.as_mut() {
            delivery.wait_for_pending();
        }
        let sent = sent.lock().unwrap();
        assert_eq!(
            sent.iter()
                .map(|(_, batch)| batch.as_array().unwrap().len())
                .sum::<usize>(),
            if access_reports { 5 } else { 2 },
            "redirect transitions and window reads must report: {sent:?}"
        );
        assert_eq!(
            sent.iter()
                .filter(|(path, _)| path == "/hop-report")
                .map(|(_, batch)| batch.as_array().unwrap().len())
                .sum::<usize>(),
            2
        );
        assert_eq!(
            sent.iter()
                .filter(|(path, _)| path == "/final-report")
                .map(|(_, batch)| batch.as_array().unwrap().len())
                .sum::<usize>(),
            if access_reports { 3 } else { 0 }
        );
        assert!(
            sent.iter()
                .filter(|(path, _)| path == "/hop-report")
                .all(|(_, report)| { report[0]["url"] == "https://isolated.example.test/hop" })
        );
        assert!(sent.iter().any(|(_, report)| {
            report[0]["body"]["type"] == "navigation-to-response"
                && report[0]["body"]["previousResponseURL"]
                    == "https://isolated.example.test/popup-old"
        }));
        assert!(sent.iter().any(|(_, report)| {
            report[0]["body"]["type"] == "navigation-from-response"
                && report[0]["body"]["nextResponseURL"] == "https://isolated.example.test/final"
        }));
        if access_reports {
            assert!(sent.iter().any(|(path, batch)| {
                path == "/final-report"
                    && batch.as_array().unwrap().iter().any(|report| {
                        report["body"]["property"] == "closed"
                            && report["body"]["type"] == "access-to-coop-page-from-opener"
                            && report["body"]["openerURL"] == "https://isolated.example.test/main"
                    })
            }));
            assert!(sent.iter().any(|(path, batch)| {
                path == "/final-report"
                    && batch
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|report| report["body"]["property"] == "opener")
            }));
        }
    }
}

#[test]
fn redirect_coop_reports_to_the_outgoing_documents_endpoint() {
    #[derive(Clone)]
    struct OldEndpointTransport(Arc<Mutex<Vec<serde_json::Value>>>);

    impl HttpTransport for OldEndpointTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            match (request.method(), request.url().path()) {
                (crate::http::Method::Get, "/old") => Ok(HttpResponse::new(
                    200,
                    "OK",
                    vec![
                        ("Content-Type".into(), "text/html".into()),
                        (
                            "Cross-Origin-Opener-Policy-Report-Only".into(),
                            "same-origin; report-to=\"old\"".into(),
                        ),
                        ("Reporting-Endpoints".into(), "old=\"/old-report\"".into()),
                    ],
                    b"<body>old</body>".to_vec(),
                )),
                (crate::http::Method::Get, "/redirect") => Ok(HttpResponse::new(
                    302,
                    "Found",
                    vec![("Location".into(), "/final".into())],
                    Vec::new(),
                )),
                (crate::http::Method::Get, "/final") => Ok(HttpResponse::new(
                    200,
                    "OK",
                    vec![("Content-Type".into(), "text/html".into())],
                    b"<body>final</body>".to_vec(),
                )),
                (crate::http::Method::Post, "/old-report") => {
                    self.0
                        .lock()
                        .unwrap()
                        .push(serde_json::from_slice(request.body().unwrap()).unwrap());
                    Ok(HttpResponse::new(204, "No Content", Vec::new(), Vec::new()))
                }
                (method, path) => panic!("unexpected old COOP endpoint request: {method:?} {path}"),
            }
        }
    }

    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = runtime_for("https://isolated.example.test/main");
    runtime.set_http_resource_transport(Some(Arc::new(OldEndpointTransport(Arc::clone(&sent)))));
    runtime
        .eval("globalThis.popup = window.open('/old')")
        .unwrap();
    runtime.run_until_idle().unwrap();
    let old_document_id = runtime
        .host_state
        .borrow()
        .auxiliary_contexts
        .values()
        .next()
        .unwrap()
        .document
        .identity();
    runtime
        .eval_in_document_realm(old_document_id, "location.href = '/redirect'")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    if let Some(delivery) = runtime.host_state.borrow_mut().report_delivery.as_mut() {
        delivery.wait_for_pending();
    }
    let sent = sent.lock().unwrap();
    let reports = sent
        .iter()
        .flat_map(|batch| batch.as_array().unwrap().iter())
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 2);
    assert!(
        reports
            .iter()
            .all(|report| { report["url"] == "https://isolated.example.test/old" })
    );
    assert!(
        reports
            .iter()
            .any(|report| { report["body"]["type"] == "navigation-to-response" })
    );
    assert!(reports.iter().any(|report| {
        report["body"]["type"] == "navigation-from-response"
            && report["body"]["nextResponseURL"] == "https://isolated.example.test/redirect"
    }));
}

#[test]
fn redirect_coop_hides_a_cross_origin_previous_response_url() {
    let old_url = "https://first.example.test/old";
    let hop_url = "https://second.example.test/hop";
    let final_url = "https://second.example.test/final";
    let old_origin = DocumentSecurityOrigin::Tuple(StorageOrigin::from_url(old_url).unwrap());
    let final_origin = DocumentSecurityOrigin::Tuple(StorageOrigin::from_url(final_url).unwrap());
    let redirect = NavigationRedirect {
        url: hop_url.into(),
        headers: vec![(
            "Cross-Origin-Opener-Policy-Report-Only".into(),
            "same-origin; report-to=\"coop\"".into(),
        )],
        request_referrer: old_url.into(),
    };
    let evaluation = coop_navigation_evaluation(
        &OpenerPolicy::default(),
        old_url,
        Some(&old_origin),
        &[redirect],
        &OpenerPolicy::default(),
        final_url,
        old_url,
        &final_origin,
        false,
    );
    assert_eq!(evaluation.same_origin, [false, true]);
    assert!(evaluation.report_only_switch);
    assert!(!evaluation.enforce_switch);
    let body = coop_navigation_to_body(
        evaluation.responses[1].opener.report_only_value,
        &evaluation.responses[0].url,
        evaluation.same_origin[0],
        &evaluation.responses[1].request_referrer,
        Disposition::Reporting,
    );
    assert!(body["previousResponseURL"].is_null());
}

#[test]
fn enforced_coop_switch_survives_later_redirect_responses() {
    let origin = DocumentSecurityOrigin::Tuple(
        StorageOrigin::from_url("https://isolated.example.test/old").unwrap(),
    );
    let redirects = [
        NavigationRedirect {
            url: "https://isolated.example.test/first".into(),
            headers: vec![("Cross-Origin-Opener-Policy".into(), "same-origin".into())],
            request_referrer: String::new(),
        },
        NavigationRedirect {
            url: "https://isolated.example.test/second".into(),
            headers: Vec::new(),
            request_referrer: String::new(),
        },
    ];
    let evaluation = coop_navigation_evaluation(
        &OpenerPolicy::default(),
        "https://isolated.example.test/old",
        Some(&origin),
        &redirects,
        &OpenerPolicy::default(),
        "https://isolated.example.test/final",
        "",
        &origin,
        false,
    );
    assert_eq!(evaluation.decisions.len(), 3);
    assert!(evaluation.decisions[0].enforce);
    assert!(evaluation.enforce_switch);
    assert_eq!(evaluation.responses[1].url, redirects[0].url);
    assert_eq!(evaluation.responses[2].url, redirects[1].url);
}

#[test]
fn initial_about_blank_coop_exception_does_not_apply_to_later_redirects() {
    let origin = DocumentSecurityOrigin::Tuple(
        StorageOrigin::from_url("https://isolated.example.test/first").unwrap(),
    );
    let redirects = [
        NavigationRedirect {
            url: "https://isolated.example.test/first".into(),
            headers: vec![(
                "Cross-Origin-Opener-Policy".into(),
                "same-origin-allow-popups".into(),
            )],
            request_referrer: String::new(),
        },
        NavigationRedirect {
            url: "https://isolated.example.test/second".into(),
            headers: Vec::new(),
            request_referrer: String::new(),
        },
    ];
    let evaluation = coop_navigation_evaluation(
        &OpenerPolicy::default(),
        "about:blank",
        Some(&origin),
        &redirects,
        &OpenerPolicy::default(),
        "https://isolated.example.test/final",
        "",
        &origin,
        true,
    );
    assert!(evaluation.decisions[0].enforce);
    assert!(evaluation.decisions[1].enforce);
}

#[test]
fn redirect_coop_without_reporting_endpoint_does_not_send_a_report() {
    struct NoEndpointTransport;

    impl HttpTransport for NoEndpointTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            let response = match (request.method(), request.url().path()) {
                (crate::http::Method::Get, "/old") => HttpResponse::new(
                    200,
                    "OK",
                    vec![("Content-Type".into(), "text/html".into())],
                    b"<body>old</body>".to_vec(),
                ),
                (crate::http::Method::Get, "/hop") => HttpResponse::new(
                    302,
                    "Found",
                    vec![
                        ("Location".into(), "/final".into()),
                        (
                            "Cross-Origin-Opener-Policy-Report-Only".into(),
                            "same-origin; report-to=\"missing\"".into(),
                        ),
                    ],
                    Vec::new(),
                ),
                (crate::http::Method::Get, "/final") => HttpResponse::new(
                    200,
                    "OK",
                    vec![("Content-Type".into(), "text/html".into())],
                    b"<body>final</body>".to_vec(),
                ),
                (method, path) => panic!("unexpected report without endpoint: {method:?} {path}"),
            };
            Ok(response)
        }
    }

    let mut runtime = runtime_for("https://isolated.example.test/main");
    runtime.set_http_resource_transport(Some(Arc::new(NoEndpointTransport)));
    runtime
        .eval("globalThis.popup = window.open('/old')")
        .unwrap();
    runtime.run_until_idle().unwrap();
    runtime.eval("popup.location.href = '/hop'").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    if let Some(delivery) = runtime.host_state.borrow_mut().report_delivery.as_mut() {
        delivery.wait_for_pending();
    }
}

#[test]
fn report_only_popup_access_to_its_opener_uses_virtual_group_ids() {
    #[derive(Clone)]
    struct AccessTransport(Arc<Mutex<Vec<(String, serde_json::Value)>>>);

    impl HttpTransport for AccessTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            match (request.method(), request.url().path()) {
                (crate::http::Method::Get, "/popup") => Ok(HttpResponse::new(
                    200,
                    "OK",
                    vec![
                        ("Content-Type".into(), "text/html".into()),
                        (
                            "Cross-Origin-Opener-Policy-Report-Only".into(),
                            "same-origin; report-to=\"popup\"".into(),
                        ),
                        (
                            "Reporting-Endpoints".into(),
                            "popup=\"/popup-report\"".into(),
                        ),
                    ],
                    b"<body>popup</body>".to_vec(),
                )),
                (crate::http::Method::Post, "/main-report" | "/popup-report") => {
                    self.0.lock().unwrap().push((
                        request.url().path().to_owned(),
                        serde_json::from_slice(request.body().unwrap()).unwrap(),
                    ));
                    Ok(HttpResponse::new(204, "No Content", Vec::new(), Vec::new()))
                }
                (method, path) => panic!("unexpected opener access request: {method:?} {path}"),
            }
        }
    }

    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = runtime_for("https://isolated.example.test/main");
    runtime.install_document_response_headers(&[
        (
            "Cross-Origin-Opener-Policy-Report-Only".into(),
            "unsafe-none; report-to=\"main\"".into(),
        ),
        ("Reporting-Endpoints".into(), "main=\"/main-report\"".into()),
    ]);
    runtime.set_http_resource_transport(Some(Arc::new(AccessTransport(Arc::clone(&sent)))));
    runtime.eval("window.open('/popup')").unwrap();
    runtime.run_until_idle().unwrap();
    let popup_id = {
        let state = runtime.host_state.borrow();
        let popup = state.auxiliary_contexts.values().next().unwrap();
        assert_ne!(popup.virtual_group_id, state.main_virtual_group_id);
        popup.document.identity()
    };
    runtime
        .eval_in_document_realm(popup_id, "void opener.closed")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    if let Some(delivery) = runtime.host_state.borrow_mut().report_delivery.as_mut() {
        delivery.wait_for_pending();
    }
    let sent = sent.lock().unwrap();
    let access_reports = sent
        .iter()
        .flat_map(|(path, batch)| {
            batch
                .as_array()
                .unwrap()
                .iter()
                .map(move |report| (path, report))
        })
        .filter(|(_, report)| report["body"]["property"] == "closed")
        .collect::<Vec<_>>();
    assert_eq!(access_reports.len(), 2, "{sent:?}");
    assert!(access_reports.iter().any(|(path, report)| {
        *path == "/popup-report" && report["body"]["type"] == "access-from-coop-page-to-opener"
    }));
    assert!(access_reports.iter().any(|(path, report)| {
        *path == "/main-report" && report["body"]["type"] == "access-to-coop-page-from-openee"
    }));
}

#[test]
fn require_corp_blocks_cross_origin_opaque_fetch_without_opt_in() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    runtime.install_isolation_policy(&isolated_headers());
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime
        .eval(
            r#"globalThis.results = [];
               Promise.all([
                 fetch('https://resource.example.test/opaque', {mode:'no-cors'}).then(
                   () => 'unexpected', error => error instanceof TypeError ? 'blocked' : 'wrong error'),
                 fetch('https://resource.example.test/opted-in', {mode:'no-cors'}).then(
                   response => response.type, () => 'unexpected')
               ]).then(values => results = values);"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    assert_eq!(
        runtime
            .eval("JSON.stringify(results)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        r#"["blocked","opaque"]"#
    );
}

#[test]
fn require_corp_blocks_classic_script_without_response_opt_in() {
    let document = crate::html::TreeBuilder::parse(
        "<body><script src='https://resource.example.test/script-no-corp.js'></script><script src='https://resource.example.test/script-corp.js'></script></body>",
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://isolated.example.test/page").unwrap();
    runtime.install_isolation_policy(&isolated_headers());
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    let base = "https://isolated.example.test/page".parse().unwrap();
    let errors = runtime.execute_document_scripts(Some(&base));
    assert_eq!(
        errors,
        vec!["failed to fetch script: https://resource.example.test/script-no-corp.js".to_owned()]
    );
    assert!(
        runtime
            .eval("globalThis.scriptNoCorpRan === undefined && globalThis.scriptCorpRan === true")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn report_only_coep_keeps_classic_script_and_records_its_opaque_response() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    runtime.install_isolation_policy(&[(
        "Cross-Origin-Embedder-Policy-Report-Only".into(),
        "require-corp; report-to=\"coep\"".into(),
    )]);
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    let document_id = runtime.document().identity();
    let base = "https://isolated.example.test/page".parse().unwrap();
    let source = runtime
        .host_state
        .borrow_mut()
        .fetch_classic_source_for_document(
            document_id,
            "https://resource.example.test/script-no-corp.js",
            Some(&base),
            None,
        );
    assert!(source.is_some_and(|(_, source, _, _)| source.contains("scriptNoCorpRan")));
    let state = runtime.host_state.borrow();
    assert_eq!(state.pending_isolation_reports.len(), 1);
    let report = &state.pending_isolation_reports[0];
    assert_eq!(report.document_id, document_id);
    assert_eq!(report.envelope["type"], "coep");
    assert_eq!(report.envelope["body"]["type"], "corp");
    assert_eq!(report.envelope["body"]["destination"], "script");
    assert_eq!(report.envelope["body"]["disposition"], "reporting");
    assert_eq!(
        report.envelope["body"]["blockedURL"],
        "https://resource.example.test/script-no-corp.js"
    );
}

#[test]
fn coep_report_reaches_http_and_observer_once_across_reentrant_jobs() {
    #[derive(Clone)]
    struct ReportingTransport(Arc<Mutex<Vec<serde_json::Value>>>);

    impl HttpTransport for ReportingTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            match (request.method(), request.url().path()) {
                (crate::http::Method::Get, "/script-no-corp.js") => Ok(HttpResponse::new(
                    200,
                    "OK",
                    Vec::new(),
                    b"globalThis.scriptLoaded = true;".to_vec(),
                )),
                (crate::http::Method::Post, "/reports") => {
                    let reports: serde_json::Value =
                        serde_json::from_slice(request.body().unwrap()).unwrap();
                    self.0.lock().unwrap().push(reports);
                    Ok(HttpResponse::new(204, "No Content", Vec::new(), Vec::new()))
                }
                (method, path) => panic!("unexpected reporting request: {method:?} {path}"),
            }
        }
    }

    for http_first in [false, true] {
        let mut runtime = runtime_for("https://isolated.example.test/page");
        let headers = vec![
            (
                "Cross-Origin-Embedder-Policy-Report-Only".into(),
                "require-corp; report-to=\"coep\"".into(),
            ),
            ("Reporting-Endpoints".into(), "coep=\"/reports\"".into()),
        ];
        runtime.install_response_security_headers(&headers);
        runtime.install_isolation_policy(&headers);
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime.set_http_resource_transport(Some(Arc::new(ReportingTransport(Arc::clone(&sent)))));
        runtime
            .eval(
                "globalThis.isolationReports=[];\
                 new ReportingObserver(batch => isolationReports.push(...batch),\
                   {types:['coep']}).observe();",
            )
            .unwrap();
        let document_id = runtime.document().identity();
        {
            let state = runtime.host_state.borrow();
            assert_eq!(
                state.document_isolation_policies[&document_id]
                    .embedder
                    .report_only_reporting_endpoint,
                "coep"
            );
            assert_eq!(
                state.document_csp[&document_id].reporting_endpoint("coep"),
                Some("https://isolated.example.test/reports".into())
            );
        }
        let base = "https://isolated.example.test/page".parse().unwrap();
        assert!(
            runtime
                .host_state
                .borrow_mut()
                .fetch_classic_source_for_document(
                    document_id,
                    "https://resource.example.test/script-no-corp.js",
                    Some(&base),
                    None,
                )
                .is_some()
        );
        if http_first {
            runtime
                .host_state
                .borrow_mut()
                .report_delivery
                .as_mut()
                .unwrap()
                .wait_for_pending();
        }
        runtime.run_until_idle().unwrap();
        runtime.run_jobs().unwrap();
        let observer = runtime
            .eval("JSON.stringify(isolationReports.map(report => ({type:report.type,url:report.url,body:report.body})))")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        let observer: serde_json::Value = serde_json::from_str(&observer).unwrap();
        assert_eq!(
            observer.as_array().unwrap().len(),
            1,
            "http_first={http_first}"
        );
        assert_eq!(observer[0]["type"], "coep");
        assert_eq!(observer[0]["url"], "https://isolated.example.test/page");
        assert_eq!(observer[0]["body"]["disposition"], "reporting");
        if !http_first {
            runtime
                .host_state
                .borrow_mut()
                .report_delivery
                .as_mut()
                .unwrap()
                .wait_for_pending();
        }
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1, "http_first={http_first}");
        assert_eq!(sent[0][0]["type"], "coep");
        assert_eq!(sent[0][0]["body"], observer[0]["body"]);
        assert!(
            runtime
                .host_state
                .borrow()
                .pending_isolation_reports
                .is_empty()
        );
    }
}

#[test]
fn isolated_dedicated_worker_requires_compatible_response_policy() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    runtime.install_isolation_policy(&isolated_headers());
    runtime.set_http_resource_transport(Some(Arc::new(IsolationTransport)));
    runtime
        .eval(
            r#"globalThis.workerMessages = [];
               globalThis.workerErrors = [];
               const good = new Worker('/worker-isolated.js');
               good.onmessage = event => workerMessages.push(event.data);
               good.postMessage('go');
               const bad = new Worker('/worker-unisolated.js');
               bad.onmessage = event => workerMessages.push(event.data);
               bad.onerror = event => workerErrors.push(event.message);"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    assert_eq!(
        runtime
            .eval("JSON.stringify(workerMessages)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true]"
    );
    assert_eq!(
        runtime.eval("workerErrors.length").unwrap().as_number(),
        Some(1.0)
    );
}

#[test]
fn isolated_page_does_not_grant_isolation_to_data_dedicated_worker() {
    let mut runtime = runtime_for("https://isolated.example.test/page");
    runtime.install_isolation_policy(&isolated_headers());
    assert!(isolated(&mut runtime));
    runtime
        .eval(
            "globalThis.workerMessages = []; \
             const source = 'onmessage = () => postMessage(crossOriginIsolated)'; \
             const worker = new Worker('data:text/javascript,' + encodeURIComponent(source)); \
             worker.onmessage = event => workerMessages.push(event.data); \
             worker.postMessage('go');",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_task_errors().is_empty());
    assert_eq!(
        runtime
            .eval("JSON.stringify(workerMessages)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[false]"
    );
}
