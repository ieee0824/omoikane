use super::browser_session::{
    BrowserSessionAction, BrowserSessionState, PendingPageNavigation, deadline_after,
    page_task_script_error_lines, pending_evaluation_busy_message, session_evaluation,
};
use super::dom::{cdp_node_type, serialize_outer_html};
use super::*;
use crate::error_reporting::{EventStore, ReporterConfig, RetentionPolicy};
use crate::test_support::http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::num::NonZeroUsize;
use std::rc::Rc;

#[test]
fn native_startup_paints_parsed_document_before_scripts_and_load() {
    let mut session = CdpSession::new().unwrap();
    session.set_deferred_document_startup(true);
    session
        .install_document(
            "https://example.test/",
            "<style>body { margin:0; background:red }</style><body><script>
             globalThis.started = true;
             globalThis.loads = 0;
             addEventListener('load', () => loads++);
             document.body.style.background = 'blue';
             </script></body>",
            1,
            "null",
        )
        .unwrap();
    assert_eq!(
        session
            .runtime
            .eval("typeof started")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
    assert_eq!(session.render_demand(None).next, NextRendering::EveryFrame);
    let first = crate::frame::render_browser_frame(&mut session, 100, 100, 0).unwrap();
    assert_eq!(&first.pixels()[0..4], &[255, 0, 0, 255]);
    assert_eq!(session.render_demand(None).next, NextRendering::EveryFrame);
    let second = crate::frame::render_browser_frame(&mut session, 100, 100, 16).unwrap();
    assert_eq!(&second.pixels()[0..4], &[0, 0, 255, 255]);
    assert_eq!(
        session.runtime.eval("loads").unwrap().as_number(),
        Some(1.0)
    );
    session.drive_event_loop(16).unwrap();
    assert_eq!(
        session.runtime.eval("loads").unwrap().as_number(),
        Some(1.0)
    );
}

#[test]
fn navigation_replaces_unstarted_document_scripts() {
    let mut session = CdpSession::new().unwrap();
    session.set_deferred_document_startup(true);
    session
        .install_document(
            "https://example.test/old",
            "<script>localStorage.setItem('oldRan', 'yes')</script>",
            1,
            "null",
        )
        .unwrap();
    session
        .install_document(
            "https://example.test/new",
            "<script>globalThis.newRan = true</script>",
            1,
            "null",
        )
        .unwrap();
    session.drive_event_loop(0).unwrap();
    session.drive_event_loop(16).unwrap();
    assert_eq!(
        session.runtime.eval("newRan").unwrap().as_boolean(),
        Some(true)
    );
    assert!(
        session
            .runtime
            .eval("localStorage.getItem('oldRan')")
            .unwrap()
            .is_null()
    );
}

#[test]
fn configured_resource_worker_count_keeps_replacement_free_of_retired_requests() {
    struct Release(Option<Vec<std::sync::mpsc::Sender<()>>>);
    impl Release {
        fn release(&mut self) {
            if let Some(senders) = self.0.take() {
                for sender in senders {
                    let _ = sender.send(());
                }
            }
        }
    }
    impl Drop for Release {
        fn drop(&mut self) {
            self.release();
        }
    }
    struct Transport {
        started: std::sync::mpsc::Sender<String>,
        finished: std::sync::mpsc::Sender<String>,
        release: Vec<Mutex<std::sync::mpsc::Receiver<()>>>,
    }
    impl crate::http::HttpTransport for Transport {
        fn send(
            &self,
            request: &crate::http::HttpRequest,
            _: Option<std::time::Duration>,
        ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
            let url = request.url().to_string();
            self.started.send(url.clone()).unwrap();
            let gate = if url.ends_with("/held") {
                Some(0)
            } else if url.ends_with("/configured") {
                Some(1)
            } else {
                None
            };
            if let Some(index) = gate {
                self.release[index].lock().unwrap().recv().unwrap();
                self.finished.send(url).unwrap();
            }
            Ok(crate::http::HttpResponse::new(
                200,
                "OK",
                vec![("Access-Control-Allow-Origin".into(), "*".into())],
                b"ready".to_vec(),
            ))
        }
    }
    let (started, observed) = std::sync::mpsc::channel();
    let (release, gates): (Vec<_>, Vec<_>) = (0..2).map(|_| std::sync::mpsc::channel()).unzip();
    let (finished, completion) = std::sync::mpsc::channel();
    let transport = Arc::new(Transport {
        started,
        finished,
        release: gates.into_iter().map(Mutex::new).collect(),
    });
    let mut session = CdpSession::new().unwrap();
    let mut release = Release(Some(release));
    session.set_resource_queue(crate::resource_worker::ResourceQueue::new(
        NonZeroUsize::new(1).unwrap(),
    ));
    session.set_nonblocking_fetch_loading(true);
    session
        .runtime
        .set_http_resource_transport(Some(transport.clone()));
    session
        .runtime
        .eval("void fetch('https://example.test/held')")
        .unwrap();
    session.runtime.run_jobs().unwrap();
    assert_eq!(
        observed.recv_timeout(Duration::from_secs(5)).unwrap(),
        "https://example.test/held"
    );
    assert_eq!(session.runtime.resource_worker_count().get(), 1);
    session.set_resource_worker_count(NonZeroUsize::new(1).unwrap());
    session
        .runtime
        .eval("void fetch('https://example.test/configured')")
        .unwrap();
    session.runtime.run_jobs().unwrap();
    assert_eq!(
        observed.recv_timeout(Duration::from_secs(5)).unwrap(),
        "https://example.test/configured"
    );

    session
        .install_document(
            "https://example.test/replacement",
            "<body>next</body>",
            1,
            "null",
        )
        .unwrap();
    assert_eq!(session.runtime.resource_worker_count().get(), 1);
    session.runtime.set_http_resource_transport(Some(transport));
    session
        .runtime
        .eval("void fetch('https://example.test/next')")
        .unwrap();
    session.runtime.run_jobs().unwrap();
    assert_eq!(
        observed.recv_timeout(Duration::from_secs(5)).unwrap(),
        "https://example.test/next",
        "new navigation must have its own worker while the retired request remains held",
    );
    assert_eq!(
        completion.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    );
    release.release();
    let mut finished = (0..2)
        .map(|_| completion.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect::<Vec<_>>();
    finished.sort();
    assert_eq!(
        finished,
        [
            "https://example.test/configured",
            "https://example.test/held"
        ]
    );
}

#[test]
fn configured_resource_queue_is_shared_by_current_and_replacement_documents() {
    struct Transport;
    impl crate::http::HttpTransport for Transport {
        fn send(
            &self,
            _: &crate::http::HttpRequest,
            _: Option<std::time::Duration>,
        ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
            Ok(crate::http::HttpResponse::new(
                200,
                "OK",
                vec![("Access-Control-Allow-Origin".into(), "*".into())],
                b"loaded".to_vec(),
            ))
        }
    }
    let queue = crate::resource_worker::ResourceQueue::new(NonZeroUsize::new(1).unwrap());
    let mut session = CdpSession::new().unwrap();
    session.set_resource_queue(queue.clone());
    session.set_nonblocking_fetch_loading(true);
    session
        .runtime
        .set_http_resource_transport(Some(Arc::new(Transport)));
    session
        .runtime
        .eval("fetch('https://example.test/current').catch(error => { globalThis.queueFetchError = String(error); })")
        .unwrap();
    session.runtime.run_jobs().unwrap();
    assert_eq!(
        session
            .runtime
            .eval("globalThis.queueFetchError")
            .unwrap()
            .as_string(),
        None,
        "the fetch must reach queue admission without rejecting",
    );

    session
        .install_document(
            "https://example.test/replacement",
            "<body>replacement</body>",
            1,
            "null",
        )
        .unwrap();
    session
        .runtime
        .set_http_resource_transport(Some(Arc::new(Transport)));
    session
        .runtime
        .eval("fetch('https://example.test/resource').catch(error => { globalThis.queueFetchError = String(error); })")
        .unwrap();
    session.runtime.run_jobs().unwrap();
    assert_eq!(
        session
            .runtime
            .eval("globalThis.queueFetchError")
            .unwrap()
            .as_string(),
        None,
        "the fetch must reach queue admission without rejecting",
    );

    let scope = crate::resource_worker::ResourceScope::default();
    let probe = queue
        .submit(
            &scope,
            crate::resource_worker::ResourcePriority::Background,
            crate::resource_worker::ResourceKind::Other,
            |_| (),
        )
        .unwrap();
    assert_eq!(
        probe.id(),
        2,
        "both runtimes must allocate task IDs from the configured queue",
    );
}

#[test]
fn static_render_settles_deferred_document_startup() {
    let mut session = CdpSession::new().unwrap();
    session.set_deferred_document_startup(true);
    session
        .install_document(
            "https://example.test/",
            "<body><script>document.title = 'Initialized'</script></body>",
            1,
            "null",
        )
        .unwrap();
    session.settle_for_render().unwrap();
    assert_eq!(
        session.document_title_without_tasks().unwrap(),
        "Initialized"
    );
}

#[test]
fn task_pump_limit_survives_navigation_and_defers_extra_timer_tasks() {
    let mut session = CdpSession::new().unwrap();
    session.set_task_pump_limit(NonZeroUsize::new(1));

    for _ in 0..2 {
        session
            .dispatch(
                "Page.navigate",
                json!({"url": "data:text/html,<script>globalThis.runs=0;for(let i=0;i<4;i++)setTimeout(()=>runs++,0)</script>"}),
            )
            .unwrap();
        let runs_after_navigation = session.runtime.eval("runs").unwrap().as_number().unwrap();
        assert!(runs_after_navigation < 4.0);
        for _ in 0..8 {
            session.drive_event_loop(0).unwrap();
        }
        assert_eq!(session.runtime.eval("runs").unwrap().as_number(), Some(4.0));
    }
}

#[test]
fn rendering_opportunity_defers_tasks_after_navigation_commit() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document("https://example.test/", "<body></body>", 1, "null")
        .unwrap();
    session.set_task_pump_limit(NonZeroUsize::new(1));
    session
        .runtime
        .eval("globalThis.runs=0;for(let i=0;i<3;i++)setTimeout(()=>runs++,0)")
        .unwrap();

    session.drive_event_loop(0).unwrap();
    assert_eq!(session.runtime.eval("runs").unwrap().as_number(), Some(1.0));
    session.drive_event_loop(0).unwrap();
    assert_eq!(session.runtime.eval("runs").unwrap().as_number(), Some(2.0));
    session.drive_event_loop(0).unwrap();
    assert_eq!(session.runtime.eval("runs").unwrap().as_number(), Some(3.0));
}

#[test]
fn bounded_rendering_opportunity_defers_timer_queued_by_frame_callback() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document("https://example.test/", "<body></body>", 1, "null")
        .unwrap();
    session.set_task_pump_limit(NonZeroUsize::new(1));
    session
        .runtime
        .eval("globalThis.runs = 0; requestAnimationFrame(() => setTimeout(() => runs++, 0))")
        .unwrap();

    session.drive_event_loop(0).unwrap();
    assert_eq!(session.runtime.eval("runs").unwrap().as_number(), Some(0.0));
    session.drive_event_loop(0).unwrap();
    assert_eq!(session.runtime.eval("runs").unwrap().as_number(), Some(1.0));
}

#[test]
fn javascript_navigation_keeps_history_for_non_string_and_replaces_on_html_completion() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/page",
            "<body><p id=old>old</p></body>",
            1,
            "null",
        )
        .unwrap();
    let original_document = session.runtime.document().identity();
    let history_length = session.history_entries.len();
    session
        .dispatch(
            "Page.navigate",
            json!({"url":"javascript:globalThis.sideEffect = 7;"}),
        )
        .unwrap();
    assert_eq!(session.runtime.document().identity(), original_document);
    assert_eq!(session.history_entries.len(), history_length);
    assert_eq!(session.current_url, "https://example.test/page");
    assert_eq!(
        session.runtime.eval("sideEffect").unwrap().as_number(),
        Some(7.0)
    );

    session
        .dispatch(
            "Page.navigate",
            json!({"url":"javascript:%27%3Cmain%20id%3Dnew%3Enew%3C%2Fmain%3E%27"}),
        )
        .unwrap();
    assert_ne!(session.runtime.document().identity(), original_document);
    assert_eq!(session.history_entries.len(), history_length);
    assert_eq!(
        session.history_entries[session.history_index].url,
        "https://example.test/page"
    );
    assert!(
        session
            .runtime
            .eval("document.getElementById('new') !== null")
            .unwrap()
            .as_boolean()
            .unwrap()
    );
}

#[test]
fn javascript_navigation_html_completion_inherits_trusted_types_policy() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document("https://example.test/page", "<body></body>", 1, "null")
        .unwrap();
    session.runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    session
        .runtime
        .eval("trustedTypes.createPolicy('default', {createScript: source => source})")
        .unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url":"javascript:%27%3Cmain%3Enew%3C%2Fmain%3E%27"}),
        )
        .unwrap();
    assert_eq!(
        session
            .runtime
            .eval("(() => { try { document.body.innerHTML = '<b>unsafe</b>'; return false; } catch (error) { return error instanceof TypeError; } })()")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn javascript_navigation_html_completion_inherits_permissions_and_isolation_policy() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document("https://example.test/page", "<body></body>", 1, "null")
        .unwrap();
    let headers = vec![
        ("Cross-Origin-Opener-Policy".into(), "same-origin".into()),
        ("Cross-Origin-Embedder-Policy".into(), "require-corp".into()),
        ("Permissions-Policy".into(), "fullscreen=()".into()),
    ];
    session.runtime.install_response_security_headers(&headers);
    session.runtime.install_isolation_policy(&headers);
    assert!(
        session
            .runtime
            .eval("crossOriginIsolated && !document.permissionsPolicy.allowsFeature('fullscreen')")
            .unwrap()
            .to_boolean()
    );
    let previous_document = session.runtime.document().identity();
    session
        .dispatch(
            "Page.navigate",
            json!({"url":"javascript:%27%3Cmain%3Enew%3C%2Fmain%3E%27"}),
        )
        .unwrap();
    assert_ne!(session.runtime.document().identity(), previous_document);
    assert!(
        session
            .runtime
            .eval("crossOriginIsolated && !document.permissionsPolicy.allowsFeature('fullscreen')")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn child_to_top_javascript_navigation_uses_child_request_client_csp() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/page",
            "<body><iframe id=child></iframe></body>",
            1,
            "null",
        )
        .unwrap();
    let initial_document = session.runtime.document().identity();
    session.runtime.eval(r#"
        globalThis.topExecuted = false;
        const frame = document.getElementById('child');
        frame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"><script>
          globalThis.childViolations = [];
          document.addEventListener('securitypolicyviolation', event => childViolations.push(event));
          top.location.href = 'javascript:globalThis.topExecuted=true;';
        <\/script>`;
    "#).unwrap();
    session.drive_navigation_requests().unwrap();
    assert_eq!(session.runtime.document().identity(), initial_document);
    assert_eq!(
        session.runtime.eval("topExecuted").unwrap().as_boolean(),
        Some(false)
    );
    assert_eq!(session.current_url, "https://example.test/page");
    assert_eq!(
        session.runtime.eval("frame.contentWindow.childViolations.length === 1 && frame.contentWindow.childViolations[0].effectiveDirective === 'require-trusted-types-for' && frame.contentWindow.childViolations[0].disposition === 'enforce'").unwrap().as_boolean(),
        Some(true),
    );
}

#[test]
fn javascript_top_navigation_inside_eval_and_function_uses_top_policy() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document("https://example.test/page", "<body></body>", 1, "null")
        .unwrap();
    session.runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    session.runtime.eval(r#"
        globalThis.topViolations = [];
        document.addEventListener('securitypolicyviolation', event => topViolations.push(event));
        globalThis.codePolicy = trustedTypes.createPolicy('code', {createScript: source => source});
        eval(codePolicy.createScript('location.href="javascript:globalThis.evalNavigationRan=true;"'));
    "#).unwrap();
    session.drive_navigation_requests().unwrap();
    session.runtime.eval(r#"
        new Function(codePolicy.createScript('location.href="javascript:globalThis.functionNavigationRan=true;"'))();
    "#).unwrap();
    session.drive_navigation_requests().unwrap();
    assert_eq!(
        session.runtime.eval("globalThis.evalNavigationRan === undefined && globalThis.functionNavigationRan === undefined && topViolations.length === 2 && topViolations.every(event => event.effectiveDirective === 'require-trusted-types-for' && event.disposition === 'enforce')").unwrap().as_boolean(),
        Some(true),
    );
}

#[test]
fn child_calling_top_author_function_uses_top_javascript_navigation_policy() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/page",
            "<body><iframe id=child></iframe></body>",
            1,
            "null",
        )
        .unwrap();
    session.runtime.eval(r#"
        globalThis.topNavigationRan = false;
        globalThis.navigateFromTopFunction = () => {
          location.href = 'javascript:globalThis.topNavigationRan=true;';
        };
        globalThis.frame = document.getElementById('child');
        frame.srcdoc = `<script>globalThis.callTopFunction = () => top.navigateFromTopFunction();<\/script>`;
    "#).unwrap();
    session.runtime.run_until_idle().unwrap();
    assert_eq!(
        session
            .runtime
            .eval("typeof frame.contentWindow.callTopFunction === 'function'")
            .unwrap()
            .as_boolean(),
        Some(true),
    );
    session.runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    session
        .runtime
        .eval(
            r#"
        globalThis.topViolations = [];
        document.addEventListener('securitypolicyviolation', event => topViolations.push(event));
        frame.contentWindow.callTopFunction();
    "#,
        )
        .unwrap();
    session.drive_navigation_requests().unwrap();
    assert_eq!(
        session.runtime.eval("topNavigationRan === false && topViolations.length === 1 && topViolations[0].effectiveDirective === 'require-trusted-types-for' && topViolations[0].disposition === 'enforce'").unwrap().as_boolean(),
        Some(true),
    );
}

#[test]
fn child_to_sibling_javascript_location_uses_child_request_client_csp() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/page",
            "<body><iframe id=target></iframe><iframe id=source></iframe></body>",
            1,
            "null",
        )
        .unwrap();
    session.runtime.eval(r#"
        globalThis.targetFrame = document.getElementById('target');
        globalThis.sourceFrame = document.getElementById('source');
        sourceFrame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"><script>
          globalThis.childViolations = [];
          document.addEventListener('securitypolicyviolation', event => childViolations.push(event));
          parent.document.getElementById('target').contentWindow.location.href =
            'javascript:globalThis.siblingExecuted=true;';
        <\/script>`;
    "#).unwrap();
    session.drive_navigation_requests().unwrap();
    assert_eq!(
        session.runtime.eval("targetFrame.contentWindow.siblingExecuted === undefined && sourceFrame.contentWindow.childViolations.length === 1 && sourceFrame.contentWindow.childViolations[0].effectiveDirective === 'require-trusted-types-for'").unwrap().as_boolean(),
        Some(true),
    );
}

#[test]
fn child_to_popup_javascript_location_uses_child_request_client_csp() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/page",
            "<body><iframe id=source></iframe></body>",
            1,
            "null",
        )
        .unwrap();
    session.runtime.eval(r#"
        globalThis.popup = window.open('', 'targetPopup');
        globalThis.sourceFrame = document.getElementById('source');
        sourceFrame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"><script>
          globalThis.childViolations = [];
          document.addEventListener('securitypolicyviolation', event => childViolations.push(event));
          parent.popup.location.href = 'javascript:globalThis.popupExecuted=true;';
        <\/script>`;
    "#).unwrap();
    session.drive_navigation_requests().unwrap();
    assert_eq!(
        session.runtime.eval("popup.popupExecuted === undefined && sourceFrame.contentWindow.childViolations.length === 1 && sourceFrame.contentWindow.childViolations[0].effectiveDirective === 'require-trusted-types-for'").unwrap().as_boolean(),
        Some(true),
    );
}

#[test]
fn child_opening_popup_through_top_uses_child_javascript_request_client() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/page",
            "<body><iframe id=source></iframe></body>",
            1,
            "null",
        )
        .unwrap();
    session.runtime.eval(r#"
        globalThis.sourceFrame = document.getElementById('source');
        sourceFrame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"><script>
          globalThis.childViolations = [];
          document.addEventListener('securitypolicyviolation', event => childViolations.push(event));
          globalThis.popup = top.open('javascript:globalThis.popupExecuted=true;', 'targetPopup');
        <\/script>`;
    "#).unwrap();
    session.drive_navigation_requests().unwrap();
    assert_eq!(
        session.runtime.eval("sourceFrame.contentWindow.popup.popupExecuted === undefined && sourceFrame.contentWindow.childViolations.length === 1 && sourceFrame.contentWindow.childViolations[0].effectiveDirective === 'require-trusted-types-for'").unwrap().as_boolean(),
        Some(true),
    );
}

#[test]
fn child_to_top_javascript_navigation_uses_child_default_policy() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/page",
            "<body><iframe id=child></iframe></body>",
            1,
            "null",
        )
        .unwrap();
    session.runtime.eval(r#"
        const frame = document.getElementById('child');
        frame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"><script>
          trustedTypes.createPolicy('default', {createScript: source => source.replace('originalMarker', 'approvedMarker')});
          top.location.href = 'javascript:globalThis.originalMarker=true;';
        <\/script>`;
    "#).unwrap();
    session.drive_navigation_requests().unwrap();
    assert_eq!(
        session
            .runtime
            .eval("globalThis.approvedMarker === true && globalThis.originalMarker === undefined",)
            .unwrap()
            .as_boolean(),
        Some(true),
    );
}

#[test]
fn typed_session_errors_preserve_json_rpc_messages_and_sources() {
    let mut session = CdpSession::new().unwrap();
    let error = session
        .load_page_request("not-a-url", Method::Get, None, None)
        .unwrap_err();
    assert!(matches!(error, CdpSessionError::Url(_)));
    assert_eq!(error.to_string(), "missing '://' in URL");
    assert!(std::error::Error::source(&error).is_some());

    let rpc_error = session
        .dispatch("Page.navigate", json!({ "url": "not-a-url" }))
        .unwrap_err();
    assert_eq!(rpc_error.code, -32000);
    assert_eq!(rpc_error.message, "missing '://' in URL");

    let http_error = CdpSessionError::Http(HttpParseError::InvalidHeader);
    assert_eq!(http_error.to_string(), "invalid HTTP header");
    assert!(std::error::Error::source(&http_error).is_some());
}

#[test]
fn timed_out_page_task_records_safe_failure_without_committing_document() {
    let directory =
        std::env::temp_dir().join(format!("omoikane-cdp-page-task-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let database = directory.join("events.sqlite");
    let config = ReporterConfig::from_values(Some("record-only"), None).unwrap();
    let reporter = Arc::new(
        ErrorReporter::new(&config, database.clone(), RetentionPolicy::default()).unwrap(),
    );
    let mut session = CdpSession::new().unwrap();
    session.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Cdp);
    let previous_url = session.current_url().to_string();
    let generation = session.document_generation + 1;
    let completed = CompletedPageTask {
        runtime: JsRuntime::new().unwrap(),
        generation,
        result: Err(PageTaskError::TimedOut),
    };
    let pending = PendingDocumentCommit {
        url: "http://localhost/?token=QUERY_SECRET_942".into(),
        html: "BODY_SECRET_942".into(),
        generation,
        history_commit: NavigationCommit::Replace,
        loader_id: "1".into(),
        status: 200,
    };
    let error = session
        .commit_document_page_task(completed, pending)
        .unwrap_err();
    assert!(matches!(error, CdpSessionError::PageTaskTimedOut));
    assert_eq!(
        error.to_string(),
        "page startup task exceeded its wall-clock timeout"
    );
    assert_eq!(session.current_url(), previous_url);
    reporter.flush().unwrap();
    let expected = RawEvent::new(
        ErrorCategory::Cdp,
        ErrorSeverity::Error,
        ErrorCode::new("CDP_PAGE_TASK_FAILED").unwrap(),
        ExecutionSurface::Cdp,
        "CDP operation failed",
        &[("operation", "execute")],
    )
    .sanitize();
    let store = EventStore::open(&database, RetentionPolicy::default()).unwrap();
    assert!(store.get(expected.fingerprint()).unwrap().is_some());
    drop(store);
    drop(session);
    drop(reporter);
    let bytes = std::fs::read(&database).unwrap();
    for secret in [b"QUERY_SECRET_942".as_slice(), b"BODY_SECRET_942"] {
        assert!(!bytes.windows(secret.len()).any(|part| part == secret));
    }
    std::fs::remove_dir_all(directory).unwrap();
}

fn ax_node_by_name<'a>(nodes: &'a [Value], name: &str) -> &'a Value {
    nodes
        .iter()
        .find(|node| node["name"]["value"] == name)
        .unwrap_or_else(|| panic!("missing accessibility node named {name:?}: {nodes:?}"))
}

fn ax_node_by_role_and_name<'a>(nodes: &'a [Value], role: &str, name: &str) -> &'a Value {
    nodes
        .iter()
        .find(|node| node["role"]["value"] == role && node["name"]["value"] == name)
        .unwrap_or_else(|| {
            panic!("missing accessibility node with role {role:?} and name {name:?}: {nodes:?}")
        })
}

fn ax_property<'a>(node: &'a Value, name: &str) -> &'a Value {
    node["properties"]
        .as_array()
        .and_then(|properties| properties.iter().find(|property| property["name"] == name))
        .unwrap_or_else(|| panic!("missing accessibility property {name:?}: {node:?}"))
}

#[test]
fn evaluation_busy_message_distinguishes_navigation_from_evaluation() {
    assert_eq!(
        pending_evaluation_busy_message(true, false),
        Some("A Runtime.evaluate request is already pending")
    );
    assert_eq!(
        pending_evaluation_busy_message(false, true),
        Some("Browser session is busy with a page navigation")
    );
}

#[test]
fn completed_page_task_errors_are_formatted_for_script_logging() {
    assert_eq!(
        page_task_script_error_lines(&Ok(vec!["startup failed".to_string()])),
        ["[omoikane][js-error] startup failed"]
    );
    assert!(page_task_script_error_lines(&Err(PageTaskError::Cancelled)).is_empty());
}

#[test]
fn completed_page_task_drains_startup_and_timer_errors_once() {
    let runtime = JsRuntime::new().unwrap();
    let mut task = Box::pin(runtime.into_page_task(
        1,
        vec![PageTaskSource::Classic {
            source: "setTimeout(() => { throw new Error('startup timer failed') }, 0); throw new Error('initial script failed')".to_string(),
            label: "startup".to_string(),
            script_node_id: None,
        }],
    ));
    let waker = Waker::noop();
    let mut context = TaskContext::from_waker(waker);
    let mut completed = loop {
        if let Poll::Ready(completed) = task.as_mut().poll(&mut context) {
            break completed;
        }
    };

    let lines = take_page_task_script_error_lines(&mut completed);
    assert_eq!(lines.len(), 2);
    assert!(
        lines
            .iter()
            .all(|line| line.starts_with("[omoikane][js-error] "))
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("startup timer failed"))
            .count(),
        1
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("initial script failed"))
            .count(),
        1
    );
    assert!(take_page_task_script_error_lines(&mut completed).is_empty());
}

#[test]
fn page_navigation_reports_an_unavailable_session_distinctly_from_busy() {
    let mut state = BrowserSessionState {
        session: None,
        pending: None,
        pending_page: None,
        actions: Vec::new(),
    };

    let error = state
        .begin_page_navigation(DeferredResponseToken(1), "Page.reload", &json!({}))
        .unwrap_err();
    assert_eq!(error.code, -32000);
    assert_eq!(error.message, "Browser session is unavailable");
}

fn sample_upgrade_request() -> &'static str {
    "GET /devtools/browser HTTP/1.1\r\n\
         Host: localhost:9222\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
}

fn decode_text(frame: &WebSocketFrame) -> String {
    String::from_utf8(frame.payload.clone()).unwrap()
}

#[test]
fn computes_websocket_accept_key_from_rfc_example() {
    assert_eq!(
        websocket_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
}

#[test]
fn parses_websocket_upgrade_request() {
    let request = parse_upgrade_request(sample_upgrade_request()).unwrap();

    assert_eq!(request.path, "/devtools/browser");
    assert_eq!(request.websocket_key, "dGhlIHNhbXBsZSBub25jZQ==");
}

#[test]
fn encodes_and_decodes_masked_text_frames() {
    let original = WebSocketFrame::text("Browser.getVersion");
    let encoded = original.encode(true);
    let (decoded, consumed) = WebSocketFrame::decode(&encoded).unwrap();

    assert_eq!(consumed, encoded.len());
    assert_eq!(decoded, original);
}

#[test]
fn rejects_unmasked_client_frames_without_dispatching() {
    let mut server = CdpServer::new();
    let upgrade = server.accept_upgrade(sample_upgrade_request()).unwrap();
    let ping = WebSocketFrame {
        fin: true,
        opcode: WebSocketOpcode::Ping,
        payload: b"hello".to_vec(),
    };
    assert!(
        server
            .receive(upgrade.client_id, &ping.encode(false))
            .is_err()
    );
    assert!(server.drain_outgoing(upgrade.client_id).unwrap().is_empty());
    server
        .receive(upgrade.client_id, &ping.encode(true))
        .unwrap();
    assert_eq!(
        server.drain_outgoing(upgrade.client_id).unwrap(),
        vec![WebSocketFrame::pong(ping.payload)]
    );
}

#[test]
fn upgrades_connections_and_dispatches_json_rpc_requests() {
    let mut server = CdpServer::new();
    server.register_method("Browser.getVersion", |_| {
        Ok(json!({
            "product": "Omoikane/0.1",
            "protocolVersion": "1.3"
        }))
    });

    let upgrade = server.accept_upgrade(sample_upgrade_request()).unwrap();
    assert_eq!(upgrade.client_id, 0);
    assert!(upgrade.response.contains("101 Switching Protocols"));

    let request = WebSocketFrame::text(
        r#"{"jsonrpc":"2.0","id":1,"method":"Browser.getVersion","params":{}}"#,
    )
    .encode(true);
    server.receive(upgrade.client_id, &request).unwrap();

    let outgoing = server.drain_outgoing(upgrade.client_id).unwrap();
    assert_eq!(outgoing.len(), 1);
    let payload: Value = serde_json::from_str(&decode_text(&outgoing[0])).unwrap();
    assert_eq!(payload["id"], 1);
    assert_eq!(payload["result"]["product"], "Omoikane/0.1");
}

#[test]
fn deferred_response_keeps_processing_commands_and_preserves_client_and_id() {
    let mut server = CdpServer::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let handler_tokens = captured.clone();
    server.register_deferred_method("Runtime.evaluate", move |token, _| {
        handler_tokens.borrow_mut().push(token);
        Ok(CdpMethodResult::Deferred)
    });
    server.register_method("Page.handleJavaScriptDialog", |_| {
        Ok(json!({ "handled": true }))
    });
    let first = server.accept_upgrade(sample_upgrade_request()).unwrap();
    let second = server.accept_upgrade(sample_upgrade_request()).unwrap();

    let evaluate = WebSocketFrame::text(
        r#"{"jsonrpc":"2.0","id":"eval-1","method":"Runtime.evaluate","params":{}}"#,
    )
    .encode(true);
    server.receive(first.client_id, &evaluate).unwrap();
    assert_eq!(server.pending_response_count(), 1);
    assert!(server.drain_outgoing(first.client_id).unwrap().is_empty());

    server
        .notify(
            "Page.javascriptDialogOpening",
            json!({ "message": "Continue?", "type": "confirm" }),
        )
        .unwrap();
    for client_id in [first.client_id, second.client_id] {
        let event = server.drain_outgoing(client_id).unwrap();
        assert_eq!(event.len(), 1);
        let payload: Value = serde_json::from_str(&decode_text(&event[0])).unwrap();
        assert_eq!(payload["method"], "Page.javascriptDialogOpening");
    }

    let handle = WebSocketFrame::text(
        r#"{"jsonrpc":"2.0","id":42,"method":"Page.handleJavaScriptDialog","params":{"accept":true}}"#,
    )
    .encode(true);
    server.receive(first.client_id, &handle).unwrap();
    let immediate = server.drain_outgoing(first.client_id).unwrap();
    let payload: Value = serde_json::from_str(&decode_text(&immediate[0])).unwrap();
    assert_eq!(payload["id"], 42);
    assert_eq!(payload["result"]["handled"], true);
    assert_eq!(server.pending_response_count(), 1);

    let token = captured.borrow()[0];
    server
        .complete_deferred_response(token, Ok(json!({ "result": { "value": true } })))
        .unwrap();
    let completed = server.drain_outgoing(first.client_id).unwrap();
    let payload: Value = serde_json::from_str(&decode_text(&completed[0])).unwrap();
    assert_eq!(payload["id"], "eval-1");
    assert_eq!(payload["result"]["result"]["value"], true);
    assert_eq!(server.pending_response_count(), 0);
    assert_eq!(
        server.complete_deferred_response(token, Ok(Value::Null)),
        Err(CdpError::UnknownDeferredRequest(token.0))
    );
}

fn browser_request(session: &mut BrowserSession, client_id: u64, payload: &str) {
    session
        .receive(client_id, &WebSocketFrame::text(payload).encode(true))
        .unwrap();
}

fn browser_session_with_timeout(timeout: Duration) -> BrowserSession {
    let session = BrowserSession::new().unwrap();
    session.state.borrow_mut().session.as_mut().unwrap().runtime =
        JsRuntime::with_document_and_sandbox(
            TreeBuilder::parse("<html><head></head><body></body></html>").document(),
            crate::js::SandboxConfig {
                timeout,
                max_loop_iterations: u64::MAX,
            },
        )
        .unwrap();
    session
}

fn browser_payloads(session: &mut BrowserSession, client_id: u64) -> Vec<Value> {
    session
        .drain_outgoing(client_id)
        .unwrap()
        .iter()
        .map(|frame| serde_json::from_str(&decode_text(frame)).unwrap())
        .collect()
}

fn browser_payloads_waiting(session: &mut BrowserSession, client_id: u64) -> Vec<Value> {
    session
        .wait_for_outgoing(client_id)
        .unwrap()
        .iter()
        .map(|frame| serde_json::from_str(&decode_text(frame)).unwrap())
        .collect()
}

#[test]
fn browser_session_forwards_existing_dom_commands() {
    let mut session = BrowserSession::new().unwrap();
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"document","method":"DOM.getDocument","params":{"depth":1}}"#,
    );
    let response = browser_payloads(&mut session, client.client_id);
    assert_eq!(response.len(), 1);
    assert_eq!(response[0]["id"], "document");
    assert_eq!(response[0]["result"]["root"]["nodeName"], "#document");
    assert!(response[0].get("error").is_none());

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"enable-ax","method":"Accessibility.enable","params":{}}"#,
    );
    let response = browser_payloads(&mut session, client.client_id);
    assert_eq!(response[0]["id"], "enable-ax");
    assert_eq!(response[0]["result"], json!({}));

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"ax-root","method":"Accessibility.getRootAXNode","params":{}}"#,
    );
    let response = browser_payloads(&mut session, client.client_id);
    assert_eq!(response[0]["id"], "ax-root");
    assert_eq!(
        response[0]["result"]["node"]["role"]["value"],
        "RootWebArea"
    );
    assert!(
        response[0]["result"]["node"]["backendDOMNodeId"]
            .as_u64()
            .is_some()
    );
}

#[test]
fn busy_forwarded_command_does_not_cancel_the_pending_dialog() {
    let mut session = BrowserSession::new().unwrap();
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"eval","method":"Runtime.evaluate","params":{"expression":"confirm('Still pending?')"}}"#,
    );
    browser_payloads(&mut session, client.client_id);

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"dom","method":"DOM.getDocument","params":{}}"#,
    );
    let busy = browser_payloads(&mut session, client.client_id);
    assert_eq!(busy.len(), 1);
    assert_eq!(busy[0]["id"], "dom");
    assert_eq!(busy[0]["error"]["code"], -32000);
    assert_eq!(session.pending_response_count(), 1);

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"handle","method":"Page.handleJavaScriptDialog","params":{"accept":true}}"#,
    );
    let completed = browser_payloads(&mut session, client.client_id);
    assert!(completed.iter().any(|value| {
        value["method"] == "Page.javascriptDialogClosed" && value["params"]["result"] == true
    }));
    assert!(
        completed
            .iter()
            .any(|value| { value["id"] == "eval" && value["result"]["result"]["value"] == true })
    );
    assert_eq!(session.pending_response_count(), 0);
}

#[test]
fn browser_session_round_trips_confirm_while_serving_another_command() {
    let mut session = BrowserSession::new().unwrap();
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"eval","method":"Runtime.evaluate","params":{"expression":"confirm('Continue?')","returnByValue":true}}"#,
    );
    let opening = browser_payloads(&mut session, client.client_id);
    assert_eq!(session.pending_response_count(), 1, "{opening:?}");
    assert_eq!(opening.len(), 1);
    assert_eq!(opening[0]["method"], "Page.javascriptDialogOpening");
    assert_eq!(opening[0]["params"]["type"], "confirm");
    assert_eq!(opening[0]["params"]["message"], "Continue?");
    assert_eq!(opening[0]["params"]["defaultPrompt"], "");
    assert_eq!(opening[0]["params"]["url"], "about:blank");

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"version","method":"Browser.getVersion","params":{}}"#,
    );
    let version = browser_payloads(&mut session, client.client_id);
    assert_eq!(version[0]["id"], "version");
    assert_eq!(version[0]["result"]["product"], "Omoikane/0.1");
    assert_eq!(session.pending_response_count(), 1);

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"handle","method":"Page.handleJavaScriptDialog","params":{"accept":false}}"#,
    );
    let completed = browser_payloads(&mut session, client.client_id);
    assert_eq!(completed[0]["id"], "handle");
    assert_eq!(completed[1]["method"], "Page.javascriptDialogClosed");
    assert_eq!(completed[1]["params"]["result"], false);
    assert_eq!(completed[2]["id"], "eval");
    assert_eq!(completed[2]["result"]["result"]["value"], false);
    assert_eq!(session.pending_response_count(), 0);
}

#[test]
fn session_evaluation_expires_before_its_first_poll() {
    let session = CdpSession::new().unwrap();
    let controller = session.runtime.javascript_dialog_controller();
    let mut evaluation = session_evaluation(
        session,
        "globalThis.expiredRequestRan = true; alert('late')".to_string(),
        true,
        Rc::new(Cell::new(false)),
        Instant::now(),
    );
    let mut context = TaskContext::from_waker(Waker::noop());
    let Poll::Ready((mut session, result)) = evaluation.as_mut().poll(&mut context) else {
        panic!("an expired request must complete on its first poll");
    };
    assert!(result.unwrap_err().message.contains("wall-clock timeout"));
    assert!(controller.pending().is_none());
    assert_eq!(
        session
            .runtime
            .eval("typeof expiredRequestRan")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
}

#[test]
fn session_evaluation_expires_at_request_deadline_while_suspended() {
    let session = CdpSession::new().unwrap();
    let controller = session.runtime.javascript_dialog_controller();
    // Queueing can consume part of the request budget before the runtime
    // starts its own evaluator. Keep the request deadline earlier than the
    // runtime's deadline to reproduce that difference without a busy loop.
    let deadline = deadline_after(Duration::from_secs(1));
    let mut evaluation = session_evaluation(
        session,
        "alert('suspended'); globalThis.expiredRequestRan = true".to_string(),
        true,
        Rc::new(Cell::new(false)),
        deadline,
    );
    let mut context = TaskContext::from_waker(Waker::noop());
    assert!(evaluation.as_mut().poll(&mut context).is_pending());
    assert!(controller.pending().is_some());
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
    let Poll::Ready((mut session, result)) = evaluation.as_mut().poll(&mut context) else {
        panic!("a suspended evaluation must expire at the request deadline");
    };
    assert!(result.unwrap_err().message.contains("wall-clock timeout"));
    assert!(controller.pending().is_none());
    assert_eq!(
        session.runtime.eval("21 * 2").unwrap().as_number(),
        Some(42.0)
    );
    assert_eq!(
        session
            .runtime
            .eval("typeof expiredRequestRan")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
}

#[test]
fn browser_session_times_out_a_suspended_runtime_evaluation() {
    // A two-millisecond budget can expire during parsing or scheduling on
    // a loaded runner, before alert suspends. Allow the setup to finish,
    // then wait for the real evaluation deadline below. The contract here
    // is cancellation of a suspended dialog, not script startup speed.
    let mut session = browser_session_with_timeout(Duration::from_secs(1));
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"eval","method":"Runtime.evaluate","params":{"expression":"alert('timeout')","returnByValue":true}}"#,
    );
    let mut payloads = browser_payloads(&mut session, client.client_id);
    assert_eq!(
        payloads[0]["method"], "Page.javascriptDialogOpening",
        "{payloads:?}"
    );
    assert_eq!(session.pending_response_count(), 1);
    assert!(browser_payloads(&mut session, client.client_id).is_empty());
    payloads.extend(browser_payloads_waiting(&mut session, client.client_id));
    assert_eq!(session.pending_response_count(), 0, "{payloads:#?}");
    assert!(payloads.iter().any(|value| {
        value["method"] == "Page.javascriptDialogClosed" && value["params"]["result"] == false
    }));
    assert!(payloads.iter().any(|value| {
        value["id"] == "eval"
            && value["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("wall-clock timeout"))
    }));
    assert_eq!(session.pending_response_count(), 0);
}

#[test]
fn browser_session_emits_each_sequential_dialog_opening() {
    let mut session = BrowserSession::new().unwrap();
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"eval","method":"Runtime.evaluate","params":{"expression":"alert('First'); confirm('Second')"}}"#,
    );
    let first = browser_payloads(&mut session, client.client_id);
    assert_eq!(first[0]["method"], "Page.javascriptDialogOpening");
    assert_eq!(first[0]["params"]["type"], "alert");
    assert_eq!(first[0]["params"]["message"], "First");

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"first-handle","method":"Page.handleJavaScriptDialog","params":{"accept":true}}"#,
    );
    let between = browser_payloads(&mut session, client.client_id);
    assert!(between.iter().any(|value| {
        value["method"] == "Page.javascriptDialogClosed" && value["params"]["result"] == true
    }));
    assert!(between.iter().any(|value| {
        value["method"] == "Page.javascriptDialogOpening"
            && value["params"]["type"] == "confirm"
            && value["params"]["message"] == "Second"
    }));
    assert_eq!(session.pending_response_count(), 1);

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"second-handle","method":"Page.handleJavaScriptDialog","params":{"accept":false}}"#,
    );
    let completed = browser_payloads(&mut session, client.client_id);
    assert!(completed.iter().any(|value| {
        value["method"] == "Page.javascriptDialogClosed" && value["params"]["result"] == false
    }));
    assert!(
        completed
            .iter()
            .any(|value| { value["id"] == "eval" && value["result"]["result"]["value"] == false })
    );
    assert_eq!(session.pending_response_count(), 0);
}

#[test]
fn resumed_async_evaluation_commits_queued_location_navigation() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        for body in ["<title>Start</title>", "<title>Async next</title>"] {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let _ = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    let mut session = BrowserSession::new().unwrap();
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
    let start_url = format!("http://127.0.0.1:{}/start", address.port());
    let next_url = format!("http://127.0.0.1:{}/next", address.port());
    browser_request(
        &mut session,
        client.client_id,
        &json!({
            "jsonrpc": "2.0",
            "id": "navigate",
            "method": "Page.navigate",
            "params": { "url": start_url },
        })
        .to_string(),
    );
    browser_payloads(&mut session, client.client_id);
    browser_request(
        &mut session,
        client.client_id,
        &json!({
            "jsonrpc": "2.0",
            "id": "eval",
            "method": "Runtime.evaluate",
            "params": {
                "expression": format!(
                    "confirm('Leave?'); location.href = {next_url:?}; 'navigating'"
                ),
            },
        })
        .to_string(),
    );
    browser_payloads(&mut session, client.client_id);
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"handle","method":"Page.handleJavaScriptDialog","params":{"accept":true}}"#,
    );
    let completed = browser_payloads(&mut session, client.client_id);
    assert!(
        completed.iter().any(|value| {
            value["id"] == "eval" && value["result"]["result"]["value"] == "navigating"
        }),
        "{completed:?}"
    );
    assert!(
        completed.iter().any(|value| {
            value["method"] == "Page.frameNavigated" && value["params"]["frame"]["url"] == next_url
        }),
        "{completed:?}"
    );

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"tree","method":"Page.getFrameTree","params":{}}"#,
    );
    let tree = browser_payloads(&mut session, client.client_id);
    assert_eq!(tree[0]["result"]["frameTree"]["frame"]["url"], next_url);
    server.join();
}

#[test]
fn browser_session_javascript_navigation_drains_queued_location_request() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let body = "<title>Next</title>";
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        let _ = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    let mut session = BrowserSession::new().unwrap();
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
    let start_url = format!("http://127.0.0.1:{}/start", address.port());
    let next_url = format!("http://127.0.0.1:{}/next", address.port());
    session
        .state
        .borrow_mut()
        .session
        .as_mut()
        .unwrap()
        .install_document(&start_url, "<title>Start</title>", 1, "null")
        .unwrap();

    browser_request(
        &mut session,
        client.client_id,
        &json!({
            "jsonrpc": "2.0",
            "id": "javascript",
            "method": "Page.navigate",
            "params": { "url": format!("javascript:location.href={next_url:?};void(0)") },
        })
        .to_string(),
    );
    let navigation = browser_payloads(&mut session, client.client_id);
    assert!(
        navigation
            .iter()
            .any(|value| { value["id"] == "javascript" && value["result"]["frameId"].is_string() }),
        "{navigation:?}"
    );
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"tree","method":"Page.getFrameTree","params":{}}"#,
    );
    let tree = browser_payloads(&mut session, client.client_id);
    assert!(
        tree.iter().any(|value| {
            value["id"] == "tree" && value["result"]["frameTree"]["frame"]["url"] == next_url
        }),
        "{tree:?}"
    );
    server.join();
}

#[test]
fn async_remote_objects_match_sync_serialization_for_functions_and_edge_objects() {
    for (index, (expression, return_by_value)) in [
        ("(function namedEdge(value) { return value; })", true),
        (
            "({ kept: 1, omitted: undefined, nested: [NaN, null] })",
            true,
        ),
        ("(function referenced(value) { return value; })", false),
        ("null", false),
    ]
    .into_iter()
    .enumerate()
    {
        let params = json!({
            "expression": expression,
            "returnByValue": return_by_value,
        });
        let mut direct = CdpSession::new().unwrap();
        let expected = direct.dispatch("Runtime.evaluate", params.clone()).unwrap();

        let mut browser = BrowserSession::new().unwrap();
        let client = browser.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut browser,
            client.client_id,
            &json!({
                "jsonrpc": "2.0",
                "id": index,
                "method": "Runtime.evaluate",
                "params": params,
            })
            .to_string(),
        );
        let response = browser_payloads(&mut browser, client.client_id);
        assert_eq!(response.len(), 1);
        assert_eq!(response[0]["result"], expected, "expression: {expression}");
    }

    let mut browser = BrowserSession::new().unwrap();
    let client = browser.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut browser,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"null","method":"Runtime.evaluate","params":{"expression":"null","returnByValue":false}}"#,
    );
    let response = browser_payloads(&mut browser, client.client_id);
    let remote = &response[0]["result"]["result"];
    assert_eq!(remote["type"], "object");
    assert_eq!(remote["subtype"], "null");
    assert_eq!(remote["value"], Value::Null);
    assert!(remote.get("objectId").is_none());
}

#[test]
fn async_serializer_microtasks_settle_before_the_evaluate_response() {
    let mut browser = BrowserSession::new().unwrap();
    let client = browser.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut browser,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"serialize","method":"Runtime.evaluate","params":{"expression":"({ toJSON() { queueMicrotask(() => globalThis.serializerMicrotask = 'done'); return { ok: true }; } })","returnByValue":true}}"#,
    );
    let serialized = browser_payloads(&mut browser, client.client_id);
    assert_eq!(serialized.len(), 1);
    assert_eq!(serialized[0]["id"], "serialize");
    assert_eq!(serialized[0]["result"]["result"]["value"]["ok"], true);

    browser_request(
        &mut browser,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"observe","method":"Runtime.evaluate","params":{"expression":"globalThis.serializerMicrotask","returnByValue":true}}"#,
    );
    let observed = browser_payloads(&mut browser, client.client_id);
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0]["id"], "observe");
    assert_eq!(observed[0]["result"]["result"]["value"], "done");
}

#[test]
fn sync_and_async_raw_source_execution_share_scope_completion_and_serialization() {
    let mut direct = CdpSession::new().unwrap();
    let mut browser = BrowserSession::new().unwrap();
    let client = browser.accept_upgrade(sample_upgrade_request()).unwrap();
    let cases = [
        ("var sharedVar = 7; sharedVar", true),
        ("sharedVar", true),
        ("function sharedFunction(value) { return value * 2; }", true),
        ("sharedFunction(4)", true),
        ("1; 2; 3", true),
        (
            "({ toJSON() { queueMicrotask(() => globalThis.sharedSerializerOrder = 'settled'); return { ok: true }; } })",
            true,
        ),
        ("sharedSerializerOrder", true),
        ("(function referenced(value) { return value; })", false),
    ];

    for (index, (expression, return_by_value)) in cases.into_iter().enumerate() {
        let params = json!({
            "expression": expression,
            "returnByValue": return_by_value,
        });
        let expected = direct.dispatch("Runtime.evaluate", params.clone()).unwrap();
        browser_request(
            &mut browser,
            client.client_id,
            &json!({
                "jsonrpc": "2.0",
                "id": index,
                "method": "Runtime.evaluate",
                "params": params,
            })
            .to_string(),
        );
        let response = browser_payloads(&mut browser, client.client_id);
        assert_eq!(response.len(), 1);
        assert_eq!(response[0]["result"], expected, "expression: {expression}");
    }

    assert_eq!(
        direct
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "typeof sharedFunction", "returnByValue": true }),
            )
            .unwrap()["result"]["value"],
        "function"
    );
}

#[test]
fn browser_session_passes_prompt_text_to_the_suspended_evaluation() {
    let mut session = BrowserSession::new().unwrap();
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":7,"method":"Runtime.evaluate","params":{"expression":"prompt('Name', 'Ada')","returnByValue":true}}"#,
    );
    let opening = browser_payloads(&mut session, client.client_id);
    assert_eq!(opening[0]["params"]["type"], "prompt");
    assert_eq!(opening[0]["params"]["defaultPrompt"], "Ada");

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":8,"method":"Page.handleJavaScriptDialog","params":{"accept":true,"promptText":"Grace"}}"#,
    );
    let completed = browser_payloads(&mut session, client.client_id);
    assert_eq!(completed[1]["params"]["userInput"], "Grace");
    assert_eq!(completed[2]["id"], 7);
    assert_eq!(completed[2]["result"]["result"]["value"], "Grace");
}

#[test]
fn navigation_pumps_startup_and_load_dialogs_before_responding() {
    let mut session = BrowserSession::new().unwrap();
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"nav","method":"Page.navigate","params":{"url":"data:text/html,%3Cscript%3EglobalThis.startupOrder%3D%5B'script-before'%5D%3Balert('startup')%3BstartupOrder.push('script-after')%3BaddEventListener('load'%2C()%3D%3E%7BstartupOrder.push('load-before')%3Balert('load')%3BstartupOrder.push('load-after')%7D)%3C%2Fscript%3E"}}"#,
    );
    let opening = browser_payloads(&mut session, client.client_id);
    assert!(opening.iter().any(|value| {
        value["method"] == "Page.javascriptDialogOpening" && value["params"]["message"] == "startup"
    }));
    assert!(!opening.iter().any(|value| value["id"] == "nav"));

    for expected in ["startup", "load"] {
        browser_request(
            &mut session,
            client.client_id,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("handle-{expected}"),
                "method": "Page.handleJavaScriptDialog",
                "params": { "accept": true },
            })
            .to_string(),
        );
        let payloads = browser_payloads(&mut session, client.client_id);
        if expected == "startup" {
            assert!(
                payloads.iter().any(|value| {
                    value["method"] == "Page.javascriptDialogOpening"
                        && value["params"]["message"] == "load"
                }),
                "{payloads:#?}"
            );
            assert!(!payloads.iter().any(|value| value["id"] == "nav"));
        } else {
            assert!(payloads.iter().any(|value| value["id"] == "nav"));
        }
    }

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"state","method":"Runtime.evaluate","params":{"expression":"startupOrder.join(',')","returnByValue":true}}"#,
    );
    let state = browser_payloads(&mut session, client.client_id);
    assert!(state.iter().any(|value| {
        value["id"] == "state"
            && value["result"]["result"]["value"]
                == "script-before,script-after,load-before,load-after"
    }));
}

#[test]
fn navigation_pumps_module_timer_and_animation_frame_dialogs() {
    let mut session = BrowserSession::new().unwrap();
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"nav","method":"Page.navigate","params":{"url":"data:text/html,%3Cscript%20type%3D%22module%22%3EglobalThis.pageOrder%3D%5B'module-before'%5D%3Balert('module')%3BpageOrder.push('module-after')%3BsetTimeout(()%3D%3E%7BpageOrder.push('timer-before')%3Balert('timer')%3BpageOrder.push('timer-after')%7D%2C0)%3BrequestAnimationFrame(()%3D%3E%7BpageOrder.push('frame-before')%3Balert('frame')%3BpageOrder.push('frame-after')%7D)%3C%2Fscript%3E"}}"#,
    );
    let opening = browser_payloads(&mut session, client.client_id);
    assert!(opening.iter().any(|value| {
        value["method"] == "Page.javascriptDialogOpening" && value["params"]["message"] == "module"
    }));

    for (index, expected) in ["module", "timer", "frame"].into_iter().enumerate() {
        browser_request(
            &mut session,
            client.client_id,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("handle-{expected}"),
                "method": "Page.handleJavaScriptDialog",
                "params": { "accept": true },
            })
            .to_string(),
        );
        let payloads = browser_payloads(&mut session, client.client_id);
        if let Some(next) = ["timer", "frame"].get(index) {
            assert!(payloads.iter().any(|value| {
                value["method"] == "Page.javascriptDialogOpening"
                    && value["params"]["message"] == *next
            }));
        } else {
            assert!(payloads.iter().any(|value| value["id"] == "nav"));
        }
    }

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"state","method":"Runtime.evaluate","params":{"expression":"pageOrder.join(',')","returnByValue":true}}"#,
    );
    let state = browser_payloads(&mut session, client.client_id);
    assert!(state.iter().any(|value| {
        value["id"] == "state"
            && value["result"]["result"]["value"]
                == "module-before,module-after,timer-before,timer-after,frame-before,frame-after"
    }));
}

#[test]
fn replacement_navigation_cancels_a_suspended_startup_task() {
    let mut session = BrowserSession::new().unwrap();
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"old-nav","method":"Page.navigate","params":{"url":"data:text/html,%3Cscript%3EglobalThis.oldStartupBefore%3Dtrue%3Balert('old-startup')%3BglobalThis.oldStartupAfter%3Dtrue%3C%2Fscript%3E"}}"#,
    );
    browser_payloads(&mut session, client.client_id);
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"new-nav","method":"Page.navigate","params":{"url":"data:text/html,%3Ctitle%3Enew%3C%2Ftitle%3E"}}"#,
    );
    let payloads = browser_payloads(&mut session, client.client_id);
    assert!(payloads.iter().any(|value| {
        value["method"] == "Page.javascriptDialogClosed" && value["params"]["result"] == false
    }));
    assert!(
        payloads
            .iter()
            .any(|value| value["id"] == "old-nav" && value["error"].is_object()),
        "{payloads:#?}"
    );
    assert!(
        payloads
            .iter()
            .any(|value| value["id"] == "new-nav" && value["result"].is_object())
    );

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"state","method":"Runtime.evaluate","params":{"expression":"document.title + ':' + typeof oldStartupAfter","returnByValue":true}}"#,
    );
    let state = browser_payloads(&mut session, client.client_id);
    assert!(state.iter().any(|value| {
        value["id"] == "state" && value["result"]["result"]["value"] == "new:undefined"
    }));
}

#[test]
fn pending_page_navigation_times_out_while_a_dialog_is_open() {
    // Leave enough time for the first poll to reach the dialog even when
    // the test process is descheduled under load. The timeout is exercised
    // only after the opening notification has been observed.
    let timeout = Duration::from_secs(1);
    let runtime = JsRuntime::with_document_and_sandbox(
        crate::dom::NodeHandle::document(),
        crate::js::SandboxConfig {
            timeout,
            max_loop_iterations: u64::MAX,
        },
    )
    .unwrap();
    let task = runtime.into_page_task(
        1,
        vec![PageTaskSource::Classic {
            source: "alert('startup-timeout')".to_string(),
            label: "startup".to_string(),
            script_node_id: None,
        }],
    );
    let controller = task.dialog_controller();
    let mut state = BrowserSessionState {
        session: Some(CdpSession::new().unwrap()),
        pending: None,
        pending_page: Some(PendingPageNavigation {
            token: DeferredResponseToken(1),
            controller,
            page_url: "about:blank".to_string(),
            opened: None,
            timeout_deadline: deadline_after(timeout),
            task: Box::pin(task),
            commit: PendingDocumentCommit {
                url: "about:blank".to_string(),
                html: "<html><head></head><body></body></html>".to_string(),
                generation: 1,
                history_commit: NavigationCommit::Push,
                loader_id: "1".to_string(),
                status: 200,
            },
            response: json!({ "frameId": "frame-0", "loaderId": "1" }),
        }),
        actions: Vec::new(),
    };

    state.poll_page_navigation();
    assert!(matches!(
        state.actions.first(),
        Some(BrowserSessionAction::Notify(
            "Page.javascriptDialogOpening",
            _
        ))
    ));
    state.actions.clear();

    std::thread::sleep(timeout + Duration::from_millis(100));
    for _ in 0..8 {
        if state.pending_page.is_none() {
            break;
        }
        state.poll_page_navigation();
    }

    assert!(state.pending_page.is_none());
    assert!(state.actions.iter().any(|action| matches!(
        action,
        BrowserSessionAction::Notify("Page.javascriptDialogClosed", params)
            if params["result"] == false
    )));
    assert!(state.actions.iter().any(|action| matches!(
        action,
        BrowserSessionAction::Complete(_, Err(error))
            if error.message.contains("wall-clock timeout")
    )));
}

#[test]
fn drain_outgoing_returns_immediately_while_dialog_is_pending() {
    let mut session = BrowserSession::new().unwrap();
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"eval","method":"Runtime.evaluate","params":{"expression":"alert('still-open')","returnByValue":true}}"#,
    );
    let opening = browser_payloads(&mut session, client.client_id);
    assert_eq!(opening.len(), 1);
    assert_eq!(opening[0]["method"], "Page.javascriptDialogOpening");

    let start = Instant::now();
    let queued = session.drain_outgoing(client.client_id).unwrap();
    assert!(queued.is_empty());
    assert!(
        start.elapsed() < Duration::from_millis(250),
        "drain_outgoing must not block while only a deferred response is pending"
    );
    assert_eq!(session.pending_response_count(), 1);
}

#[test]
fn startup_navigation_owner_disconnect_clears_task_and_token() {
    let mut session = BrowserSession::new().unwrap();
    let owner = session.accept_upgrade(sample_upgrade_request()).unwrap();
    let observer = session.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut session,
        owner.client_id,
        r#"{"jsonrpc":"2.0","id":"nav","method":"Page.navigate","params":{"url":"data:text/html,%3Cscript%3Ealert('disconnect-startup')%3BglobalThis.afterStartupDisconnect%3Dtrue%3C%2Fscript%3E"}}"#,
    );
    browser_payloads(&mut session, owner.client_id);
    browser_payloads(&mut session, observer.client_id);
    session
        .receive(
            owner.client_id,
            &WebSocketFrame {
                fin: true,
                opcode: WebSocketOpcode::Close,
                payload: Vec::new(),
            }
            .encode(true),
        )
        .unwrap();
    assert_eq!(session.pending_response_count(), 0);
    let events = browser_payloads(&mut session, observer.client_id);
    assert!(events.iter().any(|value| {
        value["method"] == "Page.javascriptDialogClosed" && value["params"]["result"] == false
    }));
    browser_request(
        &mut session,
        observer.client_id,
        r#"{"jsonrpc":"2.0","id":"state","method":"Runtime.evaluate","params":{"expression":"location.href + ':' + typeof afterStartupDisconnect","returnByValue":true}}"#,
    );
    let state = browser_payloads(&mut session, observer.client_id);
    assert!(state.iter().any(|value| {
        value["id"] == "state" && value["result"]["result"]["value"] == "about:blank:undefined"
    }));
}

#[test]
fn navigation_dismisses_a_dialog_and_new_runtime_dialog_ids_are_isolated() {
    let mut session = BrowserSession::new().unwrap();
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"old","method":"Runtime.evaluate","params":{"expression":"confirm('Old runtime')"}}"#,
    );
    browser_payloads(&mut session, client.client_id);

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"nav","method":"Page.navigate","params":{"url":"data:text/html,<title>next</title>"}}"#,
    );
    let navigation = browser_payloads(&mut session, client.client_id);
    assert!(navigation.iter().any(|value| {
        value["method"] == "Page.javascriptDialogClosed" && value["params"]["result"] == false
    }));
    assert!(navigation.iter().any(|value| value["id"] == "old"));
    assert!(navigation.iter().any(|value| value["id"] == "nav"));

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"new","method":"Runtime.evaluate","params":{"expression":"prompt('New runtime', 'fresh')"}}"#,
    );
    let opening = browser_payloads(&mut session, client.client_id);
    assert!(opening.iter().any(|value| {
        value["method"] == "Page.javascriptDialogOpening"
            && value["params"]["message"] == "New runtime"
    }));
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"new-handle","method":"Page.handleJavaScriptDialog","params":{"accept":true}}"#,
    );
    let completed = browser_payloads(&mut session, client.client_id);
    assert!(
        completed
            .iter()
            .any(|value| { value["id"] == "new" && value["result"]["result"]["value"] == "fresh" })
    );
}

#[test]
fn reload_dismisses_a_dialog_and_replaces_the_script_state() {
    let mut session = BrowserSession::new().unwrap();
    let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"initial-nav","method":"Page.navigate","params":{"url":"data:text/html,<title>reload</title>"}}"#,
    );
    browser_payloads(&mut session, client.client_id);
    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"old-eval","method":"Runtime.evaluate","params":{"expression":"globalThis.beforeReload = 1; confirm('Reload?')"}}"#,
    );
    browser_payloads(&mut session, client.client_id);

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"reload","method":"Page.reload","params":{}}"#,
    );
    let reload = browser_payloads(&mut session, client.client_id);
    assert!(reload.iter().any(|value| {
        value["method"] == "Page.javascriptDialogClosed" && value["params"]["result"] == false
    }));
    assert!(reload.iter().any(|value| value["id"] == "old-eval"));
    assert!(reload.iter().any(|value| value["id"] == "reload"));

    browser_request(
        &mut session,
        client.client_id,
        r#"{"jsonrpc":"2.0","id":"check","method":"Runtime.evaluate","params":{"expression":"typeof beforeReload"}}"#,
    );
    let checked = browser_payloads(&mut session, client.client_id);
    assert!(checked.iter().any(|value| {
        value["id"] == "check" && value["result"]["result"]["value"] == "undefined"
    }));
}

#[test]
fn owner_disconnect_dismisses_dialog_without_leaking_deferred_state() {
    let mut session = BrowserSession::new().unwrap();
    let owner = session.accept_upgrade(sample_upgrade_request()).unwrap();
    let observer = session.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut session,
        owner.client_id,
        r#"{"jsonrpc":"2.0","id":1,"method":"Runtime.evaluate","params":{"expression":"alert('bye'); globalThis.afterDisconnect = true"}}"#,
    );
    browser_payloads(&mut session, owner.client_id);
    browser_payloads(&mut session, observer.client_id);

    session
        .receive(
            owner.client_id,
            &WebSocketFrame {
                fin: true,
                opcode: WebSocketOpcode::Close,
                payload: Vec::new(),
            }
            .encode(true),
        )
        .unwrap();
    assert_eq!(session.pending_response_count(), 0);
    let observer_events = browser_payloads(&mut session, observer.client_id);
    assert!(observer_events.iter().any(|value| {
        value["method"] == "Page.javascriptDialogClosed" && value["params"]["result"] == false
    }));

    browser_request(
        &mut session,
        observer.client_id,
        r#"{"jsonrpc":"2.0","id":2,"method":"Runtime.evaluate","params":{"expression":"typeof afterDisconnect"}}"#,
    );
    let response = browser_payloads(&mut session, observer.client_id);
    assert!(
        response
            .iter()
            .any(|value| { value["id"] == 2 && value["result"]["result"]["value"] == "undefined" })
    );
}

#[test]
fn observer_disconnect_does_not_cancel_owner_dialog() {
    let mut session = BrowserSession::new().unwrap();
    let owner = session.accept_upgrade(sample_upgrade_request()).unwrap();
    let observer = session.accept_upgrade(sample_upgrade_request()).unwrap();
    browser_request(
        &mut session,
        owner.client_id,
        r#"{"jsonrpc":"2.0","id":"eval","method":"Runtime.evaluate","params":{"expression":"alert('keep pending')"}}"#,
    );
    browser_payloads(&mut session, owner.client_id);
    browser_payloads(&mut session, observer.client_id);

    session
        .receive(
            observer.client_id,
            &WebSocketFrame {
                fin: true,
                opcode: WebSocketOpcode::Close,
                payload: Vec::new(),
            }
            .encode(true),
        )
        .unwrap();
    assert_eq!(session.pending_response_count(), 1);

    browser_request(
        &mut session,
        owner.client_id,
        r#"{"jsonrpc":"2.0","id":"handle","method":"Page.handleJavaScriptDialog","params":{"accept":true}}"#,
    );
    let payloads = browser_payloads(&mut session, owner.client_id);
    assert!(payloads.iter().any(|value| value["id"] == "eval"));
}

#[test]
fn disconnect_drops_only_that_clients_deferred_responses() {
    let mut server = CdpServer::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let handler_tokens = captured.clone();
    server.register_deferred_method("Runtime.evaluate", move |token, _| {
        handler_tokens.borrow_mut().push(token);
        Ok(CdpMethodResult::Deferred)
    });
    let first = server.accept_upgrade(sample_upgrade_request()).unwrap();
    let second = server.accept_upgrade(sample_upgrade_request()).unwrap();

    for (client_id, id) in [(first.client_id, 1), (second.client_id, 2)] {
        let request = WebSocketFrame::text(format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"Runtime.evaluate","params":{{}}}}"#
        ))
        .encode(true);
        server.receive(client_id, &request).unwrap();
    }
    assert_eq!(server.pending_response_count(), 2);

    let close = WebSocketFrame {
        fin: true,
        opcode: WebSocketOpcode::Close,
        payload: Vec::new(),
    }
    .encode(true);
    server.receive(first.client_id, &close).unwrap();
    assert_eq!(server.pending_response_count(), 1);

    let tokens = captured.borrow();
    assert_eq!(
        server.complete_deferred_response(tokens[0], Ok(Value::Null)),
        Err(CdpError::UnknownDeferredRequest(tokens[0].0))
    );
    server
        .complete_deferred_response(
            tokens[1],
            Err(JsonRpcError {
                code: -32001,
                message: "dialog dismissed".to_string(),
            }),
        )
        .unwrap();
    let completed = server.drain_outgoing(second.client_id).unwrap();
    let payload: Value = serde_json::from_str(&decode_text(&completed[0])).unwrap();
    assert_eq!(payload["id"], 2);
    assert_eq!(payload["error"]["code"], -32001);
}

#[test]
fn deferred_capable_method_can_complete_immediately() {
    let mut server = CdpServer::new();
    server.register_deferred_method("Runtime.evaluate", |_, params| {
        Ok(CdpMethodResult::Complete(
            json!({ "echo": params["expression"] }),
        ))
    });
    let client = server.accept_upgrade(sample_upgrade_request()).unwrap();
    let request = WebSocketFrame::text(
        r#"{"jsonrpc":"2.0","id":7,"method":"Runtime.evaluate","params":{"expression":"1 + 1"}}"#,
    )
    .encode(true);

    server.receive(client.client_id, &request).unwrap();

    let outgoing = server.drain_outgoing(client.client_id).unwrap();
    assert_eq!(outgoing.len(), 1);
    let payload: Value = serde_json::from_str(&decode_text(&outgoing[0])).unwrap();
    assert_eq!(payload["id"], 7);
    assert_eq!(payload["result"]["echo"], "1 + 1");
    assert_eq!(server.pending_response_count(), 0);
}

#[test]
fn deferred_capable_method_preserves_an_immediate_error_code() {
    let mut server = CdpServer::new();
    server.register_deferred_method("Runtime.evaluate", |_, _| {
        Err(JsonRpcError {
            code: -32001,
            message: "dialog dismissed".to_string(),
        })
    });
    let client = server.accept_upgrade(sample_upgrade_request()).unwrap();
    let request =
        WebSocketFrame::text(r#"{"jsonrpc":"2.0","id":9,"method":"Runtime.evaluate","params":{}}"#)
            .encode(true);

    server.receive(client.client_id, &request).unwrap();

    let outgoing = server.drain_outgoing(client.client_id).unwrap();
    assert_eq!(outgoing.len(), 1);
    let payload: Value = serde_json::from_str(&decode_text(&outgoing[0])).unwrap();
    assert_eq!(payload["id"], 9);
    assert_eq!(payload["error"]["code"], -32001);
    assert_eq!(payload["error"]["message"], "dialog dismissed");
    assert_eq!(server.pending_response_count(), 0);
}

#[test]
fn deferred_token_exhaustion_does_not_reuse_a_token() {
    let mut server = CdpServer::new();
    server.next_deferred_token = u64::MAX;
    server.register_deferred_method("Runtime.evaluate", |_, _| Ok(CdpMethodResult::Deferred));
    let client = server.accept_upgrade(sample_upgrade_request()).unwrap();
    let request = WebSocketFrame::text(
        r#"{"jsonrpc":"2.0","id":10,"method":"Runtime.evaluate","params":{}}"#,
    )
    .encode(true);

    assert_eq!(
        server.receive(client.client_id, &request),
        Err(CdpError::DeferredTokenExhausted)
    );
    assert_eq!(server.next_deferred_token, u64::MAX);
    assert_eq!(server.pending_response_count(), 0);
    assert!(server.drain_outgoing(client.client_id).unwrap().is_empty());
}

#[test]
fn later_registration_replaces_the_previous_handler_kind() {
    let mut server = CdpServer::new();
    server.register_deferred_method("Runtime.evaluate", |_, _| Ok(CdpMethodResult::Deferred));
    server.register_method("Runtime.evaluate", |_| Ok(json!({ "kind": "synchronous" })));
    let client = server.accept_upgrade(sample_upgrade_request()).unwrap();
    let request =
        WebSocketFrame::text(r#"{"jsonrpc":"2.0","id":8,"method":"Runtime.evaluate","params":{}}"#)
            .encode(true);

    server.receive(client.client_id, &request).unwrap();

    let outgoing = server.drain_outgoing(client.client_id).unwrap();
    assert_eq!(outgoing.len(), 1);
    let payload: Value = serde_json::from_str(&decode_text(&outgoing[0])).unwrap();
    assert_eq!(payload["result"]["kind"], "synchronous");
    assert_eq!(server.pending_response_count(), 0);
}

#[test]
fn returns_method_not_found_for_unknown_calls() {
    let mut server = CdpServer::new();
    let upgrade = server.accept_upgrade(sample_upgrade_request()).unwrap();

    let request =
        WebSocketFrame::text(r#"{"jsonrpc":"2.0","id":7,"method":"Page.enable","params":{}}"#)
            .encode(true);
    server.receive(upgrade.client_id, &request).unwrap();

    let outgoing = server.drain_outgoing(upgrade.client_id).unwrap();
    let payload: Value = serde_json::from_str(&decode_text(&outgoing[0])).unwrap();
    assert_eq!(payload["error"]["code"], -32601);
    assert_eq!(payload["id"], 7);
}

#[test]
fn broadcasts_notifications_to_multiple_clients() {
    let mut server = CdpServer::new();
    let first = server.accept_upgrade(sample_upgrade_request()).unwrap();
    let second = server.accept_upgrade(sample_upgrade_request()).unwrap();

    server
        .notify("Page.loadEventFired", json!({ "timestamp": 1.25 }))
        .unwrap();

    assert_eq!(server.client_count(), 2);

    let first_payload: Value = serde_json::from_str(&decode_text(
        &server.drain_outgoing(first.client_id).unwrap()[0],
    ))
    .unwrap();
    let second_payload: Value = serde_json::from_str(&decode_text(
        &server.drain_outgoing(second.client_id).unwrap()[0],
    ))
    .unwrap();

    assert_eq!(first_payload["method"], "Page.loadEventFired");
    assert_eq!(second_payload["params"]["timestamp"], 1.25);
}

#[test]
fn responds_to_ping_and_removes_closed_clients() {
    let mut server = CdpServer::new();
    let upgrade = server.accept_upgrade(sample_upgrade_request()).unwrap();

    let ping = WebSocketFrame {
        fin: true,
        opcode: WebSocketOpcode::Ping,
        payload: b"hi".to_vec(),
    }
    .encode(true);
    server.receive(upgrade.client_id, &ping).unwrap();

    let outgoing = server.drain_outgoing(upgrade.client_id).unwrap();
    assert_eq!(outgoing[0], WebSocketFrame::pong(b"hi".to_vec()));

    let close = WebSocketFrame {
        fin: true,
        opcode: WebSocketOpcode::Close,
        payload: Vec::new(),
    }
    .encode(true);
    server.receive(upgrade.client_id, &close).unwrap();

    assert_eq!(server.client_count(), 0);
}

#[test]
fn page_domain_navigates_reloads_and_emits_network_events() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let body = "<html><body><main id=\"app\">Hello</main></body></html>";
        for _ in 0..2 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let _ = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let mut session = CdpSession::new().unwrap();
    let url = format!("http://127.0.0.1:{}/", address.port());
    let navigate = session
        .dispatch("Page.navigate", json!({ "url": url.clone() }))
        .unwrap();
    assert_eq!(navigate["frameId"], "frame-0");

    let frame_tree = session.dispatch("Page.getFrameTree", json!({})).unwrap();
    assert_eq!(frame_tree["frameTree"]["frame"]["url"], url);

    session.dispatch("Page.reload", json!({})).unwrap();

    let events = session.drain_events();
    assert!(
        events
            .iter()
            .any(|event| event.method == "Network.requestWillBeSent")
    );
    assert!(
        events
            .iter()
            .any(|event| event.method == "Network.responseReceived")
    );
    assert!(
        events
            .iter()
            .any(|event| event.method == "Page.loadEventFired")
    );

    server.join();
}

#[test]
fn page_navigation_installs_isolation_headers_before_author_script() {
    let listener = bind_loopback().unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = FixtureWorker::spawn(move || {
        for _ in 0..2 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let isolated = request.starts_with("GET /isolated ");
            let policy = if isolated {
                "Cross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\n"
            } else {
                ""
            };
            let body = "<script>globalThis.initialIsolation = crossOriginIsolated</script>";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\n{policy}Content-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            )
            .unwrap();
        }
    });
    let mut session = CdpSession::new().unwrap();
    for (path, expected) in [("isolated", true), ("plain", false)] {
        session
            .dispatch(
                "Page.navigate",
                json!({"url": format!("http://127.0.0.1:{port}/{path}")}),
            )
            .unwrap();
        assert_eq!(
            session
                .runtime
                .eval("initialIsolation")
                .unwrap()
                .as_boolean(),
            Some(expected),
            "{path}"
        );
    }
    server.join();
}

#[test]
fn navigation_survives_a_dynamically_inserted_module_script_that_throws() {
    // The shape blog.piapro.net failed on: a page script inserts a
    // `type="module"` script, and that module throws. Neither the module's
    // syntax nor its exception may cost the navigation (issue #303).
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        for index in 0..2 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let _ = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let body: &[u8] = match index {
                0 => b"<html><body><main id='content'>rendered</main><script>                           const s = document.createElement('script');                           s.type = 'module';                           s.src = '/module.js';                           document.head.appendChild(s);                           </script></body></html>",
                _ => b"export const answer = 42; throw new Error('module boom');",
            };
            let content_type = if index == 0 {
                "text/html"
            } else {
                "text/javascript"
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(body).unwrap();
        }
    });
    let origin = format!("http://127.0.0.1:{}", address.port());
    let mut session = CdpSession::new().unwrap();

    session
        .dispatch("Page.navigate", json!({ "url": format!("{origin}/page") }))
        .expect("a throwing module script must not fail the navigation");

    // The document is installed and usable, not discarded.
    let content = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "document.getElementById('content').textContent" }),
        )
        .unwrap();
    assert_eq!(content["result"]["value"], "rendered");
    assert_eq!(session.current_url(), format!("{origin}/page"));
    server.join();
}

#[test]
fn get_and_post_form_submissions_reach_http_server_and_install_documents() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let server = FixtureWorker::spawn(move || {
        for index in 0..3 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let mut request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let content_length = request
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; content_length];
            stream.set_read_timeout(Some(READ_TIMEOUT)).unwrap();
            stream.read_exact(&mut body).unwrap();
            request.push_str(&String::from_utf8(body).unwrap());
            if index > 0 {
                sender.send(request).unwrap();
            }
            let body = match index {
                0 => {
                    "<form id='f' action='/search?old=1' method='get'><input name='q' value='hello world'><button name='via' value='button'>Search</button></form>"
                }
                1 => {
                    "<form id='f' action='/submit' method='post'><input name='q' value='hello world'><button name='via' value='button'>Send</button></form>"
                }
                _ => "<html><body><main id='submitted'>Saved</main></body></html>",
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    let origin = format!("http://127.0.0.1:{}", address.port());
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({ "url": format!("{origin}/form") }))
        .unwrap();
    session.dispatch("Runtime.evaluate", json!({ "expression": "document.getElementById('f').requestSubmit(document.querySelector('button'))" })).unwrap();
    assert_eq!(
        session.current_url(),
        format!("{origin}/search?q=hello+world&via=button")
    );
    session.dispatch("Runtime.evaluate", json!({ "expression": "document.getElementById('f').requestSubmit(document.querySelector('button'))" })).unwrap();
    assert_eq!(session.current_url(), format!("{origin}/submit"));
    let installed = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "document.getElementById('submitted').textContent" }),
        )
        .unwrap();
    assert_eq!(installed["result"]["value"], "Saved");
    let get_request = receiver.recv().unwrap();
    assert!(get_request.starts_with("GET /search?q=hello+world&via=button HTTP/1.1\r\n"));
    let post_request = receiver.recv().unwrap();
    assert!(post_request.starts_with("POST /submit HTTP/1.1\r\n"));
    assert!(post_request.contains("Content-Type: application/x-www-form-urlencoded\r\n"));
    assert!(post_request.ends_with("q=hello+world&via=button"));
    server.join();
}

#[test]
fn web_storage_survives_same_origin_document_navigation() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        for _ in 0..2 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let _ = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let body = "<html><body></body></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let origin = format!("http://127.0.0.1:{}", address.port());
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({ "url": format!("{origin}/first") }))
        .unwrap();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "localStorage.setItem('local', 'kept'); sessionStorage.setItem('session', 'kept')" }),
        )
        .unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({ "url": format!("{origin}/second") }),
        )
        .unwrap();
    let result = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "localStorage.getItem('local') === 'kept' && sessionStorage.getItem('session') === 'kept'" }),
        )
        .unwrap();
    assert_eq!(result["result"]["value"], true);

    server.join();
}

#[test]
fn location_requests_install_new_documents_and_preserve_commit_semantics() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        for _ in 0..4 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let body = match path {
                "/first" => "<html><body><main id='first'></main></body></html>",
                "/second" => {
                    r#"<html><body><main id='second'></main><script>
                        document.addEventListener('DOMContentLoaded', () => document.body.setAttribute('data-dcl', 'yes'));
                        window.addEventListener('load', () => document.body.setAttribute('data-load', 'yes'));
                    </script></body></html>"#
                }
                "/third" => "<html><body><main id='third'></main></body></html>",
                _ => "<html><body>missing</body></html>",
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let origin = format!("http://127.0.0.1:{}", address.port());
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({ "url": format!("{origin}/first") }))
        .unwrap();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "location.assign('/second')" }),
        )
        .unwrap();

    assert_eq!(session.current_url(), format!("{origin}/second"));
    let lifecycle = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "location.href === document.URL && document.querySelector('#second') !== null && document.body.getAttribute('data-dcl') === 'yes' && document.body.getAttribute('data-load') === 'yes'"
            }),
        )
        .unwrap();
    assert_eq!(lifecycle["result"]["value"], true);

    let history_len_before_replace = session.history_entries.len();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "location.replace('/third')" }),
        )
        .unwrap();
    assert_eq!(session.current_url(), format!("{origin}/third"));
    assert_eq!(session.history_entries.len(), history_len_before_replace);

    session.dispatch("Page.reload", json!({})).unwrap();
    assert_eq!(session.current_url(), format!("{origin}/third"));
    assert_eq!(session.history_entries.len(), history_len_before_replace);

    server.join();
}

#[test]
fn script_navigation_records_only_its_origin_and_top_level_site() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        for _ in 0..2 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let _ = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let body = "<html><body>visited navigation</body></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let origin = format!("http://127.0.0.1:{}", address.port());
    let start = format!("{origin}/start");
    let state_only = format!("{origin}/state-only");
    let next = format!("{origin}/next");
    let mut session = CdpSession::new().unwrap();
    let same_partition =
        VisitSource::new(crate::js::StorageOrigin::from_url(&start).unwrap(), &start).unwrap();
    let other_origin = VisitSource::new(
        crate::js::StorageOrigin::from_url("https://other.test/").unwrap(),
        &start,
    )
    .unwrap();
    let other_site = VisitSource::new(
        crate::js::StorageOrigin::from_url(&start).unwrap(),
        "https://other.test/",
    )
    .unwrap();

    session
        .dispatch("Page.navigate", json!({ "url": start }))
        .unwrap();
    assert!(
        !session
            .storage_manager
            .has_visited_url(&start, &same_partition)
    );
    assert!(
        !session
            .storage_manager
            .has_visited_url(&next, &same_partition)
    );

    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "history.pushState(null, '', '/state-only')" }),
        )
        .unwrap();
    assert!(
        !session
            .storage_manager
            .has_visited_url(&state_only, &same_partition)
    );

    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "location.assign('/next')" }),
        )
        .unwrap();
    assert_eq!(session.current_url(), next);
    assert!(
        session
            .storage_manager
            .has_visited_url(&next, &same_partition)
    );
    assert!(
        !session
            .storage_manager
            .has_visited_url(&next, &other_origin)
    );
    assert!(!session.storage_manager.has_visited_url(&next, &other_site));
    server.join();
}

#[test]
fn cdp_observes_unvisited_style_while_paint_uses_visited_color() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        let _ = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        let body = r#"<!doctype html><html><head><style>
                body { margin: 0 }
                a { display:block; width:40px; height:40px; color:#aa0000; background-color:#ff0000 }
                a:visited { display:none; width:400px; color:#00aa00; background-color:#00ff00 }
                </style></head><body><a id="link" href="/destination"></a></body></html>"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    let start = format!("http://127.0.0.1:{}/start", address.port());
    let destination = format!("http://127.0.0.1:{}/destination", address.port());
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({ "url": start }))
        .unwrap();
    server.join();

    let query = || {
        json!({
            "expression": "(() => { const a = document.getElementById('link'); const s = getComputedStyle(a); return [s.color, s.backgroundColor, s.display, s.width, a.offsetWidth, a.getBoundingClientRect().width, a.matches(':visited'), a.matches(':link')].join('|') })()",
            "returnByValue": true,
        })
    };
    let before = session.dispatch("Runtime.evaluate", query()).unwrap();
    assert_eq!(
        before["result"]["value"],
        "rgb(170, 0, 0)|rgb(255, 0, 0)|block|40px|40|40|false|true"
    );
    assert_eq!(
        session
            .runtime
            .paint_current_document()
            .unwrap()
            .pixel(20, 20),
        Some(crate::paint::Color::rgb(255, 0, 0))
    );
    let document = session.dispatch("DOM.getDocument", json!({})).unwrap();
    let root_id = document["root"]["nodeId"].as_u64().unwrap();
    let link = session
        .dispatch(
            "DOM.querySelector",
            json!({"nodeId": root_id, "selector": "#link"}),
        )
        .unwrap();
    let link_id = link["nodeId"].as_u64().unwrap();
    assert!(link_id > 0);
    let attributes_before = session
        .dispatch("DOM.getAttributes", json!({"nodeId": link_id}))
        .unwrap();

    let source =
        VisitSource::new(crate::js::StorageOrigin::from_url(&start).unwrap(), &start).unwrap();
    session
        .storage_manager
        .record_page_navigation(&destination, &destination, source);
    assert_eq!(
        session
            .runtime
            .paint_current_document()
            .unwrap()
            .pixel(20, 20),
        Some(crate::paint::Color::rgb(0, 255, 0))
    );
    let after = session.dispatch("Runtime.evaluate", query()).unwrap();
    assert_eq!(before["result"]["value"], after["result"]["value"]);
    assert_eq!(
        session
            .dispatch("DOM.getAttributes", json!({"nodeId": link_id}))
            .unwrap(),
        attributes_before
    );
    assert_eq!(
        session
            .dispatch(
                "DOM.querySelector",
                json!({"nodeId": root_id, "selector": "#link"}),
            )
            .unwrap()["nodeId"],
        link_id
    );
}

#[test]
fn fragment_navigation_keeps_document_and_skips_network_fetch() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        let _ = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        let body = r#"<html><head><style>:target { color: rgb(13, 42, 71) }</style></head>
                <body><main id='persistent'></main><div id='section'></div><div id='next'></div>
                <iframe id='child' srcdoc="<div id='section'></div>"></iframe></body></html>"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let url = format!("http://127.0.0.1:{}/page", address.port());
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({ "url": url }))
        .unwrap();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "globalThis.hashChanges = 0; globalThis.popChanges = 0; addEventListener('hashchange', () => hashChanges++); addEventListener('popstate', () => popChanges++); location.assign('#section')" }),
        )
        .unwrap();

    assert_eq!(session.current_url(), format!("{url}#section"));
    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "document.querySelector('#persistent') !== null && hashChanges === 1 && popChanges === 1 && location.hash === '#section' && document.querySelector(':target')?.id === 'section' && document.getElementById('section').matches(':target') && getComputedStyle(document.getElementById('section')).color === 'rgb(13, 42, 71)' && document.getElementById('child').contentDocument.querySelector(':target') === null" }),
        )
        .unwrap();
    assert_eq!(state["result"]["value"], true);

    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "location.hash = '#next'" }),
        )
        .unwrap();
    assert_eq!(session.current_url(), format!("{url}#next"));
    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "hashChanges === 2 && popChanges === 2 && document.querySelector(':target')?.id === 'next' && document.querySelectorAll(':target').length === 1 && !document.getElementById('section').matches(':target') && getComputedStyle(document.getElementById('section')).color !== 'rgb(13, 42, 71)' && getComputedStyle(document.getElementById('next')).color === 'rgb(13, 42, 71)' && document.getElementById('child').contentDocument.querySelector(':target') === null" }),
        )
        .unwrap();
    assert_eq!(state["result"]["value"], true);

    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "location.hash = ''" }),
        )
        .unwrap();
    assert_eq!(session.current_url(), url);
    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "hashChanges === 3 && popChanges === 3 && document.querySelector(':target') === null && !document.getElementById('next').matches(':target') && getComputedStyle(document.getElementById('next')).color !== 'rgb(13, 42, 71)'" }),
        )
        .unwrap();
    assert_eq!(state["result"]["value"], true);
    assert!(
        session
            .drain_events()
            .iter()
            .any(|event| event.method == "Page.navigatedWithinDocument")
    );
    server.join();
}

#[test]
fn history_api_url_changes_keep_target_until_traversal() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        let _ = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        let body = "<html><head><style>:target { color: rgb(13, 42, 71) }</style></head><body><div id='one'></div><div id='two'></div><div id='three'></div></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let base_url = format!("http://127.0.0.1:{}/page", address.port());
    let url = format!("{base_url}#one");
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({ "url": base_url }))
        .unwrap();
    server.join();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "location.hash = '#one'" }),
        )
        .unwrap();
    let generation = session.document_generation;
    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "globalThis.originalDocument = document; globalThis.historyEvents = []; addEventListener('popstate', () => historyEvents.push('popstate')); addEventListener('hashchange', () => historyEvents.push('hashchange')); history.pushState({}, '', '/renamed#two'); document.URL === location.href ? document.querySelector(':target')?.id : 'stale URL'" }),
        )
        .unwrap();
    assert_eq!(state["result"]["value"], "one");
    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "history.replaceState({}, '', '/renamed#three'); document.URL === location.href ? document.querySelector(':target')?.id : 'stale URL'" }),
        )
        .unwrap();
    assert_eq!(state["result"]["value"], "one");
    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "historyEvents.length === 0" }),
        )
        .unwrap();
    assert_eq!(state["result"]["value"], true);

    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "history.back()" }),
        )
        .unwrap();
    assert_eq!(session.current_url(), url);
    assert_eq!(session.document_generation, generation);
    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "document === originalDocument && document.querySelector(':target')?.id === 'one' && getComputedStyle(document.getElementById('one')).color === 'rgb(13, 42, 71)' && historyEvents.join(',') === 'popstate,hashchange'" }),
        )
        .unwrap();
    assert_eq!(state["result"]["value"], true);

    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "history.forward()" }),
        )
        .unwrap();
    assert_eq!(session.document_generation, generation);
    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "document === originalDocument && location.pathname === '/renamed' && document.querySelector(':target')?.id === 'three' && getComputedStyle(document.getElementById('one')).color !== 'rgb(13, 42, 71)' && getComputedStyle(document.getElementById('three')).color === 'rgb(13, 42, 71)' && historyEvents.join(',') === 'popstate,hashchange,popstate,hashchange'" }),
        )
        .unwrap();
    assert_eq!(state["result"]["value"], true);
}

#[test]
fn history_state_and_traversal_are_owned_by_browser_session() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        for _ in 0..1 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let body = format!("<html><body><main data-path='{path}'></main></body></html>");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let origin = format!("http://127.0.0.1:{}", address.port());
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({ "url": format!("{origin}/start") }))
        .unwrap();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "globalThis.initialDocument = document" }),
        )
        .unwrap();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "history.pushState({ page: 2 }, '', '/state')" }),
        )
        .unwrap();
    assert_eq!(session.current_url(), format!("{origin}/state"));
    assert_eq!(session.history_entries.len(), 2);

    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "history.back()" }),
        )
        .unwrap();
    assert_eq!(session.current_url(), format!("{origin}/start"));

    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "history.forward()" }),
        )
        .unwrap();
    assert_eq!(session.current_url(), format!("{origin}/state"));
    let restored = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "history.length === 2 && history.state.page === 2 && document === initialDocument && document.URL === location.href && document.querySelector('main').getAttribute('data-path') === '/start'" }),
        )
        .unwrap();
    assert_eq!(restored["result"]["value"], true);

    server.join();
}

#[test]
fn restored_document_reuses_its_same_document_history_entries() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..3 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_owned();
            let body = format!("<html><body><main data-path='{path}'></main></body></html>");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
            requests.push(path);
        }
        requests
    });

    let origin = format!("http://127.0.0.1:{}", address.port());
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({"url": format!("{origin}/start")}))
        .unwrap();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression": "history.pushState({page: 2}, '', '/state')"}),
        )
        .unwrap();
    session
        .dispatch("Page.navigate", json!({"url": format!("{origin}/other")}))
        .unwrap();
    session
        .dispatch("Runtime.evaluate", json!({"expression": "history.back()"}))
        .unwrap();
    assert_eq!(session.current_url(), format!("{origin}/state"));
    let restored_generation = session.document_generation;
    session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression": "globalThis.restoredDocument = document; history.back()"}),
        )
        .unwrap();
    assert_eq!(session.document_generation, restored_generation);
    assert_eq!(session.current_url(), format!("{origin}/start"));
    let result = session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression": "document === restoredDocument && history.state === null && document.URL === location.href"}),
        )
        .unwrap();
    assert_eq!(result["result"]["value"], true);
    assert_eq!(server.join(), ["/start", "/other", "/state"]);
}

fn custom_form_state_server(requests: usize, redirect_reload: bool) -> (String, FixtureWorker<()>) {
    let listener = bind_loopback().unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = FixtureWorker::spawn(move || {
        for index in 0..requests {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            if redirect_reload && index == 1 {
                stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /changed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                continue;
            }
            let html = if request.starts_with("GET /other ") {
                "<title>other</title>"
            } else {
                "<form><x-history name='value'></x-history></form><script>\
                     globalThis.restored = [];\
                     customElements.define('x-history', class extends HTMLElement {\
                       static formAssociated = true;\
                       constructor() { super(); this.i = this.attachInternals(); this.i.setFormValue('default'); }\
                       formStateRestoreCallback(value, mode) { restored.push([value, mode]); this.i.setFormValue(value); }\
                     });</script>"
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            )
            .unwrap();
        }
    });
    (origin, server)
}

fn assert_custom_form_state(session: &mut CdpSession, expected: &str) {
    let result = session.dispatch("Runtime.evaluate", json!({
        "expression": "JSON.stringify([restored, new FormData(document.querySelector('form')).get('value')])"
    })).unwrap();
    assert_eq!(result["result"]["value"], expected);
}

#[test]
fn custom_form_state_survives_back_forward_and_reload() {
    let (origin, server) = custom_form_state_server(6, false);
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({"url": format!("{origin}/form")}))
        .unwrap();
    session.dispatch("Runtime.evaluate", json!({
        "expression": "document.querySelector('x-history').i.setFormValue('submitted', 'saved')"
    })).unwrap();
    session
        .dispatch("Page.navigate", json!({"url": format!("{origin}/other")}))
        .unwrap();
    session
        .dispatch("Runtime.evaluate", json!({"expression": "history.back()"}))
        .unwrap();
    assert_custom_form_state(&mut session, r#"[[["saved","restore"]],"saved"]"#);
    session.dispatch("Page.reload", json!({})).unwrap();
    assert_custom_form_state(&mut session, r#"[[["saved","restore"]],"saved"]"#);
    session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression": "history.forward()"}),
        )
        .unwrap();
    assert!(session.current_url().ends_with("/other"));
    session
        .dispatch("Runtime.evaluate", json!({"expression": "history.back()"}))
        .unwrap();
    assert_custom_form_state(&mut session, r#"[[["saved","restore"]],"saved"]"#);
    server.join();
}

#[test]
fn custom_form_state_is_restored_by_suspendable_page_startup() {
    let (origin, server) = custom_form_state_server(2, false);
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({"url": format!("{origin}/form")}))
        .unwrap();
    session.dispatch("Runtime.evaluate", json!({
        "expression": "document.querySelector('x-history').i.setFormValue('submitted', 'async saved')"
    })).unwrap();
    let PreparedPageNavigation::Pending { task, commit, .. } =
        session.prepare_page_reload().unwrap()
    else {
        panic!("reload must recreate the document");
    };
    let mut task = Box::pin(task);
    let mut context = TaskContext::from_waker(Waker::noop());
    let started = std::time::Instant::now();
    let completed = loop {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "page startup timed out"
        );
        if let Poll::Ready(completed) = task.as_mut().poll(&mut context) {
            break completed;
        }
    };
    assert!(completed.result.as_ref().unwrap().is_empty());
    session
        .commit_document_page_task(completed, commit)
        .unwrap();
    assert_custom_form_state(
        &mut session,
        r#"[[["async saved","restore"]],"async saved"]"#,
    );
    server.join();
}

#[test]
fn custom_form_state_is_saved_before_pushstate_changes_the_current_entry() {
    let (origin, server) = custom_form_state_server(1, false);
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({"url": format!("{origin}/form")}))
        .unwrap();
    session.dispatch("Runtime.evaluate", json!({
        "expression": "document.querySelector('x-history').i.setFormValue('first'); history.pushState(null, '', '/step')"
    })).unwrap();
    session.dispatch("Runtime.evaluate", json!({
        "expression": "document.querySelector('x-history').i.setFormValue('second'); history.back()"
    })).unwrap();
    assert_custom_form_state(&mut session, r#"[[["first","restore"]],"first"]"#);
    session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression": "history.forward()"}),
        )
        .unwrap();
    assert_custom_form_state(
        &mut session,
        r#"[[["first","restore"],["second","restore"]],"second"]"#,
    );
    server.join();
}

#[test]
fn custom_form_state_is_not_delivered_to_a_reload_redirect() {
    let (origin, server) = custom_form_state_server(3, true);
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({"url": format!("{origin}/form")}))
        .unwrap();
    session.dispatch("Runtime.evaluate", json!({
        "expression": "document.querySelector('x-history').i.setFormValue('submitted', 'private state')"
    })).unwrap();
    session.dispatch("Page.reload", json!({})).unwrap();
    assert_eq!(session.current_url(), format!("{origin}/changed"));
    assert_eq!(
        session.history_entries[session.history_index].url,
        session.current_url()
    );
    assert_custom_form_state(&mut session, r#"[[],"default"]"#);
    server.join();
}

#[test]
fn failed_script_navigation_preserves_current_document_and_url() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        let _ = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        let body = "<html><body><main id='stable'></main></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let stable_url = format!("http://127.0.0.1:{}/stable", address.port());
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({ "url": stable_url }))
        .unwrap();
    server.join();

    let failed = session.dispatch(
        "Runtime.evaluate",
        json!({ "expression": "location.assign('/unreachable')" }),
    );
    assert!(failed.is_err());
    assert_eq!(session.current_url(), stable_url);
    let source = VisitSource::new(
        crate::js::StorageOrigin::from_url(&stable_url).unwrap(),
        &stable_url,
    )
    .unwrap();
    let unreachable = format!("http://127.0.0.1:{}/unreachable", address.port());
    assert!(
        !session
            .storage_manager
            .has_visited_url(&unreachable, &source)
    );
    let preserved = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "location.href === document.URL && location.href.endsWith('/stable') && document.querySelector('#stable') !== null" }),
        )
        .unwrap();
    assert_eq!(preserved["result"]["value"], true);
}

#[test]
fn redirect_commits_final_url_to_document_location_and_history() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        for request_index in 0..2 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let _ = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let response = if request_index == 0 {
                "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
            } else {
                let body = "<html><body><main id='final'></main></body></html>";
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
            };
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let origin = format!("http://127.0.0.1:{}", address.port());
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({ "url": format!("{origin}/redirect") }),
        )
        .unwrap();

    assert_eq!(session.current_url(), format!("{origin}/final"));
    assert_eq!(
        session.history_entries[session.history_index].url,
        format!("{origin}/final")
    );
    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "location.href.endsWith('/final') && document.URL === location.href && document.querySelector('#final') !== null" }),
        )
        .unwrap();
    assert_eq!(state["result"]["value"], true);
    server.join();
}

#[test]
fn event_loop_driver_commits_timer_and_animation_frame_navigation() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        for _ in 0..3 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let body = format!("<html><body><main data-path='{path}'></main></body></html>");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let origin = format!("http://127.0.0.1:{}", address.port());
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({ "url": format!("{origin}/start") }))
        .unwrap();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "setTimeout(() => location.assign('/timer'), 10)" }),
        )
        .unwrap();
    assert_eq!(session.current_url(), format!("{origin}/start"));
    session.drive_event_loop(10).unwrap();
    assert_eq!(session.current_url(), format!("{origin}/timer"));

    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "requestAnimationFrame(() => location.assign('/frame'))" }),
        )
        .unwrap();
    session.drive_event_loop(16).unwrap();
    assert_eq!(session.current_url(), format!("{origin}/frame"));

    server.join();
}

#[test]
fn dom_domain_exposes_document_query_attributes_and_outer_html() {
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({ "url": "data:text/html,<html><head><meta property=\"og:image\" content=\"https://example.com/image.jpg\"></head><body><main id=\"app\"><p>Hello</p></main></body></html>" }),
        )
        .unwrap();

    let document = session.dispatch("DOM.getDocument", json!({})).unwrap();
    let root_id = document["root"]["nodeId"].as_u64().unwrap();
    let queried = session
        .dispatch(
            "DOM.querySelector",
            json!({ "nodeId": root_id, "selector": "#app" }),
        )
        .unwrap();
    let app_id = queried["nodeId"].as_u64().unwrap();
    let meta = session
        .dispatch(
            "DOM.querySelector",
            json!({ "nodeId": root_id, "selector": r#"meta[property="og:image"]"# }),
        )
        .unwrap();
    let meta_id = meta["nodeId"].as_u64().unwrap();
    let attributes = session
        .dispatch("DOM.getAttributes", json!({ "nodeId": meta_id }))
        .unwrap();
    let html = session
        .dispatch("DOM.getOuterHTML", json!({ "nodeId": app_id }))
        .unwrap();

    assert_eq!(document["root"]["nodeName"], "#document");
    assert!(app_id > 0);
    assert!(meta_id > 0);
    assert_eq!(
        attributes["attributes"],
        json!([
            "content",
            "https://example.com/image.jpg",
            "property",
            "og:image"
        ])
    );
    assert_eq!(html["outerHTML"], "<main id=\"app\"><p>Hello</p></main>");
}

#[test]
fn cdp_data_uri_navigation_decodes_utf8_without_replacing_plus() {
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url": "data:text/html,%3Cp%3E%E3%81%82+%E3%81%84%3C%2Fp%3E"}),
        )
        .unwrap();
    let result = session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression": "document.body.textContent"}),
        )
        .unwrap();
    assert_eq!(result["result"]["value"], "あ+い");
}

#[test]
fn cdp_data_uri_navigation_accepts_charset_and_base64() {
    let mut session = CdpSession::new().unwrap();
    for (url, expected) in [
        (
            "data:text/html;charset=utf-8,%3Cp%3Echarset%3C%2Fp%3E",
            "charset",
        ),
        ("data:text/html;base64,PHA+YmFzZTY0PC9wPg==", "base64"),
    ] {
        session
            .dispatch("Page.navigate", json!({"url": url}))
            .unwrap();
        let result = session
            .dispatch(
                "Runtime.evaluate",
                json!({"expression": "document.body.textContent"}),
            )
            .unwrap();
        assert_eq!(result["result"]["value"], expected);
    }
}

#[test]
fn cdp_preserves_cdata_identity_and_markup() {
    let cdata = NodeHandle::cdata_section("<raw>&data");

    assert_eq!(cdp_node_type(&cdata), 4);
    assert_eq!(cdata.node_name(), "#cdata-section");
    assert_eq!(serialize_outer_html(&cdata), "<![CDATA[<raw>&data]]>");
}

#[test]
fn accessibility_domain_exposes_semantics_relations_state_and_css_visibility() {
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({
                "url": "data:text/html,<html><head><title>Settings</title><style>.gone{display:none}</style></head><body>\
                    <main aria-label='Preferences'><h2 id='heading'>Account</h2>\
                    <button id='save' aria-labelledby='heading' aria-describedby='help' aria-expanded='false'>Save</button>\
                    <span id='help' hidden>Stores changes</span>\
                    <input id='agree' type='checkbox' checked aria-label='Accept'>\
                    <img src='x' alt='Avatar'><button id='hidden-action' class='gone'>Hidden action</button>\
                    <div role='slider' aria-label='Volume' aria-valuemin='0' aria-valuemax='10' aria-valuenow='5' aria-valuetext='Medium'></div>\
                    </main></body></html>"
            }),
        )
        .unwrap();

    let disabled_error = session
        .dispatch("Accessibility.getRootAXNode", json!({}))
        .unwrap_err();
    assert_eq!(disabled_error.code, -32000);
    assert!(
        session
            .dispatch("Accessibility.getFullAXTree", json!({ "depth": -1 }))
            .is_err()
    );

    session.dispatch("Accessibility.enable", json!({})).unwrap();
    // This test observes the AX focus state, not focus-driven scrolling.
    // Avoid coupling it to a synchronous layout while the full lib suite
    // is contending for CI runner CPU time.
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "document.querySelector('#save').focus({ preventScroll: true })" }),
        )
        .unwrap();
    let full = session
        .dispatch("Accessibility.getFullAXTree", json!({}))
        .unwrap();
    let nodes = full["nodes"].as_array().unwrap();
    let root = &nodes[0];
    assert_eq!(root["role"]["value"], "RootWebArea");
    assert_eq!(root["name"]["value"], "Settings");
    assert_eq!(root["frameId"], "frame-0");
    assert!(root["backendDOMNodeId"].as_u64().is_some());
    assert!(!root["childIds"].as_array().unwrap().is_empty());
    let root_ax_id = root["nodeId"].as_str().unwrap().to_string();
    let root_children = session
        .dispatch(
            "Accessibility.getChildAXNodes",
            json!({ "id": root_ax_id, "frameId": "frame-0" }),
        )
        .unwrap();
    assert!(!root_children["nodes"].as_array().unwrap().is_empty());
    assert!(
        root_children["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node.get("frameId").is_none())
    );

    let button = ax_node_by_role_and_name(nodes, "button", "Account");
    assert_eq!(button["role"]["value"], "button");
    assert_eq!(button["description"]["value"], "Stores changes");
    assert!(button.get("frameId").is_none());
    assert!(button["parentId"].as_str().is_some());
    assert_eq!(ax_property(button, "expanded")["value"]["value"], false);
    assert_eq!(ax_property(button, "focused")["value"]["value"], true);
    let labelledby = ax_property(button, "labelledby");
    assert_eq!(labelledby["value"]["type"], "idrefList");
    assert_eq!(labelledby["value"]["value"], "heading");
    assert_eq!(labelledby["value"]["relatedNodes"][0]["idref"], "heading");
    assert!(
        labelledby["value"]["relatedNodes"][0]["backendDOMNodeId"]
            .as_u64()
            .is_some()
    );
    let describedby = ax_property(button, "describedby");
    assert_eq!(
        describedby["value"]["relatedNodes"][0]["text"],
        "Stores changes"
    );

    let checkbox = ax_node_by_name(nodes, "Accept");
    assert_eq!(checkbox["role"]["value"], "checkbox");
    assert_eq!(
        ax_property(checkbox, "checked")["value"]["type"],
        "tristate"
    );
    assert_eq!(ax_property(checkbox, "checked")["value"]["value"], "true");
    assert_eq!(ax_node_by_name(nodes, "Avatar")["role"]["value"], "image");

    let slider = ax_node_by_name(nodes, "Volume");
    assert_eq!(slider["value"]["value"], "Medium");
    assert_eq!(ax_property(slider, "valuemin")["value"]["type"], "number");
    assert_eq!(ax_property(slider, "valuemax")["value"]["value"], 10.0);
    assert_eq!(ax_property(slider, "valuetext")["value"]["value"], "Medium");
    assert!(
        slider["properties"]
            .as_array()
            .unwrap()
            .iter()
            .all(|property| property["name"] != "valuenow")
    );

    assert!(
        !nodes
            .iter()
            .any(|node| node["name"]["value"] == "Hidden action")
    );
    let document = session.dispatch("DOM.getDocument", json!({})).unwrap();
    let root_dom_id = document["root"]["nodeId"].as_u64().unwrap();
    let hidden_query = session
        .dispatch(
            "Accessibility.queryAXTree",
            json!({ "nodeId": root_dom_id, "accessibleName": "Hidden action" }),
        )
        .unwrap();
    assert!(hidden_query["nodes"].as_array().unwrap().is_empty());
    let hidden_dom_id = session
        .dispatch(
            "DOM.querySelector",
            json!({ "nodeId": root_dom_id, "selector": "#hidden-action" }),
        )
        .unwrap()["nodeId"]
        .as_u64()
        .unwrap();
    let hidden_partial = session
        .dispatch(
            "Accessibility.getPartialAXTree",
            json!({ "nodeId": hidden_dom_id, "fetchRelatives": false }),
        )
        .unwrap();
    let hidden = &hidden_partial["nodes"][0];
    assert_eq!(hidden["ignored"], true);
    assert_eq!(hidden["ignoredReasons"][0]["name"], "notRendered");
    assert_eq!(hidden["role"]["value"], "none");
    let direct_hidden_query = session
        .dispatch(
            "Accessibility.queryAXTree",
            json!({ "nodeId": hidden_dom_id, "role": "button" }),
        )
        .unwrap();
    assert!(direct_hidden_query["nodes"].as_array().unwrap().is_empty());
    assert!(
        session
            .dispatch(
                "Accessibility.getFullAXTree",
                json!({ "frameId": "missing" }),
            )
            .is_err()
    );
    assert!(
        session
            .dispatch("Accessibility.getRootAXNode", json!({ "frameId": 1 }),)
            .is_err()
    );
}

#[test]
fn accessibility_partial_query_and_dynamic_snapshots_use_stable_generation_ids() {
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({
                "url": "data:text/html,<html><head><title>First</title><style>.gone{display:none}</style></head>\
                    <body><main id='scope'><button id='target'>Before</button><button>Sibling</button>\
                    <button aria-hidden='true'>Ignored match</button></main></body></html>"
            }),
        )
        .unwrap();
    let document = session.dispatch("DOM.getDocument", json!({})).unwrap();
    let root_dom_id = document["root"]["nodeId"].as_u64().unwrap();
    let target_dom_id = session
        .dispatch(
            "DOM.querySelector",
            json!({ "nodeId": root_dom_id, "selector": "#target" }),
        )
        .unwrap()["nodeId"]
        .as_u64()
        .unwrap();
    let target_object_id = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "document.querySelector('#target')",
                "returnByValue": false,
            }),
        )
        .unwrap()["result"]["objectId"]
        .as_str()
        .unwrap()
        .to_string();
    let non_node_object_id = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "({ __id: document.querySelector('#target').__id })",
                "returnByValue": false,
            }),
        )
        .unwrap()["result"]["objectId"]
        .as_str()
        .unwrap()
        .to_string();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "globalThis.Node = function ReplacedNode() {}" }),
        )
        .unwrap();
    boa_gc::force_collect();

    let partial = session
        .dispatch(
            "Accessibility.getPartialAXTree",
            json!({ "nodeId": target_dom_id, "fetchRelatives": true }),
        )
        .unwrap();
    let partial_nodes = partial["nodes"].as_array().unwrap();
    let target = ax_node_by_role_and_name(partial_nodes, "button", "Before");
    let stable_target_id = target["nodeId"].as_str().unwrap().to_string();
    assert_eq!(partial_nodes[0]["nodeId"], stable_target_id);
    assert!(stable_target_id.starts_with("ax-1-"));
    assert!(
        partial_nodes
            .iter()
            .any(|node| node["role"]["value"] == "RootWebArea")
    );
    assert!(
        partial_nodes
            .iter()
            .any(|node| node["name"]["value"] == "Sibling")
    );
    let object_partial = session
        .dispatch(
            "Accessibility.getPartialAXTree",
            json!({ "objectId": target_object_id, "fetchRelatives": false }),
        )
        .unwrap();
    assert_eq!(object_partial["nodes"][0]["nodeId"], stable_target_id);
    assert!(
        session
            .dispatch(
                "Accessibility.getPartialAXTree",
                json!({ "objectId": non_node_object_id }),
            )
            .is_err()
    );

    assert!(
        session
            .dispatch(
                "Accessibility.getAXNodeAndAncestors",
                json!({ "backendNodeId": target_dom_id }),
            )
            .is_err()
    );
    session.dispatch("Accessibility.enable", json!({})).unwrap();
    let ancestors = session
        .dispatch(
            "Accessibility.getAXNodeAndAncestors",
            json!({ "backendNodeId": target_dom_id }),
        )
        .unwrap();
    assert_eq!(ancestors["nodes"][0]["nodeId"], stable_target_id);
    assert!(
        ancestors["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["nodeId"] == stable_target_id)
    );
    let rooted_query = session
        .dispatch(
            "Accessibility.queryAXTree",
            json!({
                "nodeId": target_dom_id,
                "accessibleName": "Before",
                "role": "button",
            }),
        )
        .unwrap();
    assert!(rooted_query["nodes"][0]["parentId"].as_str().is_some());
    assert!(rooted_query["nodes"][0].get("frameId").is_none());

    session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "document.querySelector('#target').textContent = 'After'; const button = document.createElement('button'); button.textContent = 'Added'; document.querySelector('#scope').appendChild(button)"
            }),
        )
        .unwrap();
    let updated = session
        .dispatch("Accessibility.getFullAXTree", json!({}))
        .unwrap();
    let updated_nodes = updated["nodes"].as_array().unwrap();
    assert_eq!(
        ax_node_by_role_and_name(updated_nodes, "button", "After")["nodeId"],
        stable_target_id
    );
    assert!(
        updated_nodes
            .iter()
            .any(|node| node["name"]["value"] == "Added")
    );

    let query = session
        .dispatch(
            "Accessibility.queryAXTree",
            json!({
                "nodeId": root_dom_id,
                "accessibleName": "Added",
                "role": "button"
            }),
        )
        .unwrap();
    assert_eq!(query["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(query["nodes"][0]["name"]["value"], "Added");
    let ignored_query = session
        .dispatch(
            "Accessibility.queryAXTree",
            json!({
                "nodeId": root_dom_id,
                "accessibleName": "Ignored match",
                "role": "button",
            }),
        )
        .unwrap();
    assert_eq!(ignored_query["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(ignored_query["nodes"][0]["ignored"], true);
    assert_eq!(ignored_query["nodes"][0]["role"]["value"], "button");
    assert!(ignored_query["nodes"][0].get("properties").is_none());
    assert!(
        session
            .dispatch("Accessibility.queryAXTree", json!({ "role": "button" }))
            .is_err()
    );

    let first_root = session
        .dispatch("Accessibility.getRootAXNode", json!({}))
        .unwrap()["node"]["nodeId"]
        .as_str()
        .unwrap()
        .to_string();
    session
        .dispatch(
            "Page.navigate",
            json!({ "url": "data:text/html,<html><head><title>Second</title></head><body></body></html>" }),
        )
        .unwrap();
    let second_root = session
        .dispatch("Accessibility.getRootAXNode", json!({}))
        .unwrap()["node"]
        .clone();
    assert_eq!(second_root["name"]["value"], "Second");
    assert!(second_root["nodeId"].as_str().unwrap().starts_with("ax-2-"));
    assert_ne!(second_root["nodeId"], first_root);
    assert!(
        session
            .dispatch(
                "Accessibility.getPartialAXTree",
                json!({ "objectId": target_object_id }),
            )
            .is_err()
    );
}

#[test]
fn accessibility_dom_targets_synthesize_missing_nodes_and_preserve_relatives() {
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({
                "url": "data:text/html,<html><body><div id='host'><button id='unassigned'>Light only</button></div>\
                    <img id='empty' alt=''><section id='owner' aria-owns='owned'></section>\
                    <section id='second-owner' aria-owns='owned'></section>\
                    <button id='owned'>Owned</button><script>const root = document.querySelector('#host').attachShadow({mode:'open'});\
                    const child = document.createElement('span'); child.textContent = 'Shadow child'; root.appendChild(child);</script>\
                    </body></html>"
            }),
        )
        .unwrap();
    let document = session.dispatch("DOM.getDocument", json!({})).unwrap();
    let root_dom_id = document["root"]["nodeId"].as_u64().unwrap();
    let query_dom = |session: &mut CdpSession, selector: &str| {
        session
            .dispatch(
                "DOM.querySelector",
                json!({ "nodeId": root_dom_id, "selector": selector }),
            )
            .unwrap()["nodeId"]
            .as_u64()
            .unwrap()
    };
    let host_dom_id = query_dom(&mut session, "#host");
    let target_dom_id = query_dom(&mut session, "#unassigned");
    let empty_dom_id = query_dom(&mut session, "#empty");
    let owner_dom_id = query_dom(&mut session, "#owner");
    let second_owner_dom_id = query_dom(&mut session, "#second-owner");
    let shadow_root_object_id = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "document.querySelector('#host').shadowRoot",
                "returnByValue": false,
            }),
        )
        .unwrap()["result"]["objectId"]
        .as_str()
        .unwrap()
        .to_string();

    let full = session
        .dispatch("Accessibility.getFullAXTree", json!({}))
        .unwrap();
    let full_nodes = full["nodes"].as_array().unwrap();
    let empty = full_nodes
        .iter()
        .find(|node| node["backendDOMNodeId"] == empty_dom_id)
        .unwrap();
    assert_eq!(empty["ignored"], true);
    assert_eq!(empty["role"]["value"], "none");
    assert!(empty.get("name").is_none());
    assert!(empty.get("properties").is_none());

    let host_partial = session
        .dispatch(
            "Accessibility.getPartialAXTree",
            json!({ "nodeId": host_dom_id, "fetchRelatives": false }),
        )
        .unwrap();
    let shadow_root_partial = session
        .dispatch(
            "Accessibility.getPartialAXTree",
            json!({ "objectId": shadow_root_object_id, "fetchRelatives": false }),
        )
        .unwrap();
    assert_eq!(shadow_root_partial["nodes"], host_partial["nodes"]);
    assert_ne!(shadow_root_partial["nodes"][0]["nodeId"], "0");

    let ignored_host_partial = session
        .dispatch(
            "Accessibility.getPartialAXTree",
            json!({ "nodeId": host_dom_id, "fetchRelatives": true }),
        )
        .unwrap();
    let ignored_host_nodes = ignored_host_partial["nodes"].as_array().unwrap();
    assert_eq!(ignored_host_nodes[0]["ignored"], true);
    assert!(
        ignored_host_nodes[0]["childIds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|child_id| ignored_host_nodes
                .iter()
                .any(|node| node["nodeId"] == *child_id))
    );
    assert!(
        ignored_host_nodes
            .iter()
            .any(|node| node["name"]["value"] == "Shadow child")
    );

    let partial = session
        .dispatch(
            "Accessibility.getPartialAXTree",
            json!({ "nodeId": target_dom_id, "fetchRelatives": true }),
        )
        .unwrap();
    let nodes = partial["nodes"].as_array().unwrap();
    assert_eq!(nodes[0]["nodeId"], "0");
    assert_eq!(nodes[0]["ignored"], true);
    assert_eq!(nodes[0]["role"]["value"], "none");
    assert_eq!(nodes[0]["ignoredReasons"][0]["name"], "notRendered");
    assert_eq!(nodes[0]["backendDOMNodeId"], target_dom_id);
    let parent_id = nodes[0]["parentId"].as_str().unwrap();
    assert_eq!(nodes[1]["nodeId"], parent_id);
    let parent_children = nodes[1]["childIds"].as_array().unwrap();
    assert_eq!(parent_children[0], "0");
    assert!(parent_children.len() > 1);

    session.dispatch("Accessibility.enable", json!({})).unwrap();
    let host_ancestors = session
        .dispatch(
            "Accessibility.getAXNodeAndAncestors",
            json!({ "nodeId": host_dom_id }),
        )
        .unwrap();
    let shadow_root_ancestors = session
        .dispatch(
            "Accessibility.getAXNodeAndAncestors",
            json!({ "objectId": shadow_root_object_id }),
        )
        .unwrap();
    assert_eq!(shadow_root_ancestors["nodes"], host_ancestors["nodes"]);

    let ancestors = session
        .dispatch(
            "Accessibility.getAXNodeAndAncestors",
            json!({ "nodeId": target_dom_id }),
        )
        .unwrap();
    let ancestor_nodes = ancestors["nodes"].as_array().unwrap();
    assert_eq!(ancestor_nodes[0]["nodeId"], "0");
    assert_eq!(ancestor_nodes[0]["parentId"], ancestor_nodes[1]["nodeId"]);
    assert!(
        ancestor_nodes
            .iter()
            .any(|node| node["role"]["value"] == "RootWebArea")
    );

    let owned_query = session
        .dispatch(
            "Accessibility.queryAXTree",
            json!({
                "nodeId": owner_dom_id,
                "accessibleName": "Owned",
                "role": "button",
            }),
        )
        .unwrap();
    assert_eq!(owned_query["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(owned_query["nodes"][0]["name"]["value"], "Owned");
    assert!(owned_query["nodes"][0]["parentId"].as_str().is_some());
    assert!(owned_query["nodes"][0].get("frameId").is_none());
    let second_owner_query = session
        .dispatch(
            "Accessibility.queryAXTree",
            json!({
                "nodeId": second_owner_dom_id,
                "accessibleName": "Owned",
                "role": "button",
            }),
        )
        .unwrap();
    assert!(second_owner_query["nodes"].as_array().unwrap().is_empty());
}

#[test]
fn accessibility_snapshot_internals_are_not_page_visible() {
    let mut session = CdpSession::new().unwrap();
    let result = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "(() => {\
                      const names = ['__omoikane_accessibility_snapshot',\
                        '__omoikane_register_canonical_node_identity',\
                        '__omoikane_get_option_selected',\
                        '__omoikane_set_option_selected'];\
                      const originalMapHas = Map.prototype.has;\
                      const originalMapSet = Map.prototype.set;\
                      let leakedCache = null;\
                      Map.prototype.has = function(key) {\
                        leakedCache = this; return originalMapHas.call(this, key);\
                      };\
                      Map.prototype.set = function(key, value) {\
                        leakedCache = this; return originalMapSet.call(this, key, value);\
                      };\
                      const option = document.createElement('option');\
                      const accessors = Object.getOwnPropertyDescriptor(\
                        HTMLOptionElement.prototype, 'selected');\
                      const forged = { __id: option.__id };\
                      const originalMapGet = Map.prototype.get;\
                      Map.prototype.get = function() { return forged; };\
                      document.body.appendChild(option);\
                      const cacheHitPreserved = document.querySelector('option') === option;\
                      if (leakedCache) {\
                        originalMapSet.call(leakedCache, option.__id, forged);\
                      }\
                      let getterRejected = false; let setterRejected = false;\
                      try { accessors.get.call(forged); }\
                      catch (error) { getterRejected = error instanceof TypeError; }\
                      try { accessors.set.call(forged, true); }\
                      catch (error) { setterRejected = error instanceof TypeError; }\
                      Map.prototype.has = originalMapHas;\
                      Map.prototype.get = originalMapGet;\
                      Map.prototype.set = originalMapSet;\
                      return names.map(name => typeof globalThis[name]).join('|') + '|' +\
                        (leakedCache === null) + '|' + cacheHitPreserved + '|' + getterRejected + '|' +\
                        setterRejected + '|' + option.selected;\
                    })()",
                "returnByValue": true,
            }),
        )
        .unwrap();

    assert_eq!(
        result["result"]["value"],
        "undefined|undefined|undefined|undefined|true|true|true|true|false"
    );
}

#[test]
fn accessibility_uses_live_form_disclosure_and_closed_shadow_state() {
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({
                "url": "data:text/html,<html><body><input id='password' type='password' value='old' aria-label='Password'>\
                    <select id='choice' aria-label='Choice'><option>First</option><option>Second</option></select>\
                    <details id='details'><summary>More</summary><p>Body</p></details><div id='host'></div></body></html>"
            }),
        )
        .unwrap();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "document.querySelector('#password').value = 's😀';\
                      document.querySelector('#choice').selectedIndex = 1;\
                      document.querySelector('#details').open = true;\
                      const root = document.querySelector('#host').attachShadow({mode:'closed'});\
                      const select = document.createElement('select'); select.setAttribute('aria-label', 'Shadow choice');\
                      const first = document.createElement('option'); first.textContent = 'Alpha';\
                      const second = document.createElement('option'); second.textContent = 'Beta';\
                      select.appendChild(first); select.appendChild(second); second.selected = true; root.appendChild(select);\
                      const details = document.createElement('details'); const summary = document.createElement('summary');\
                      summary.textContent = 'Shadow details'; details.appendChild(summary);\
                      const body = document.createElement('p'); body.textContent = 'Shadow body'; details.appendChild(body);\
                      details.open = true; root.appendChild(details);\
                      globalThis.accessibilitySelectedGetterCalls = 0;\
                      Object.defineProperty(HTMLOptionElement.prototype, 'selected', {\
                        configurable: true, get() { accessibilitySelectedGetterCalls++; return false; }\
                      });\
                      globalThis.__omoikane_accessibility_snapshot = () => '{\"selectedOptions\":[],\"openDetails\":[]}';"
            }),
        )
        .unwrap();
    boa_gc::force_collect();
    let full = session
        .dispatch("Accessibility.getFullAXTree", json!({}))
        .unwrap();
    let nodes = full["nodes"].as_array().unwrap();

    assert_eq!(ax_node_by_name(nodes, "Password")["value"]["value"], "•••");
    assert_eq!(ax_node_by_name(nodes, "Choice")["value"]["value"], "Second");
    assert_eq!(
        ax_property(
            ax_node_by_role_and_name(nodes, "button", "More"),
            "expanded"
        )["value"]["value"],
        true
    );
    assert!(nodes.iter().any(|node| node["name"]["value"] == "Body"));
    assert_eq!(
        ax_node_by_name(nodes, "Shadow choice")["value"]["value"],
        "Beta"
    );
    assert_eq!(
        ax_property(
            ax_node_by_role_and_name(nodes, "button", "Shadow details"),
            "expanded"
        )["value"]["value"],
        true
    );
    assert!(
        nodes
            .iter()
            .any(|node| node["name"]["value"] == "Shadow body")
    );
    let getter_calls = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "accessibilitySelectedGetterCalls",
                "returnByValue": true,
            }),
        )
        .unwrap();
    assert_eq!(getter_calls["result"]["value"], 0);
}

#[test]
fn navigation_invalidates_old_document_node_ids_without_reusing_them() {
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({ "url": "data:text/html,<html><body><main id='old'></main></body></html>" }),
        )
        .unwrap();
    let old_document = session.dispatch("DOM.getDocument", json!({})).unwrap();
    let old_root_id = old_document["root"]["nodeId"].as_u64().unwrap();
    let old_main_id = session
        .dispatch(
            "DOM.querySelector",
            json!({ "nodeId": old_root_id, "selector": "#old" }),
        )
        .unwrap()["nodeId"]
        .as_u64()
        .unwrap();

    session
        .dispatch(
            "Page.navigate",
            json!({ "url": "data:text/html,<html><body><main id='new'></main></body></html>" }),
        )
        .unwrap();
    let new_document = session.dispatch("DOM.getDocument", json!({})).unwrap();
    let new_root_id = new_document["root"]["nodeId"].as_u64().unwrap();

    assert_ne!(new_root_id, old_root_id);
    assert!(
        session
            .dispatch("DOM.getOuterHTML", json!({ "nodeId": old_main_id }))
            .is_err()
    );
}

#[test]
fn runtime_domain_evaluates_and_calls_functions_on_remote_objects() {
    let mut session = CdpSession::new().unwrap();

    let value = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "21 * 2", "returnByValue": true }),
        )
        .unwrap();
    assert_eq!(value["result"]["value"], 42);

    let object = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "({ count: 2 })", "returnByValue": false }),
        )
        .unwrap();
    let object_id = object["result"]["objectId"].as_str().unwrap().to_string();
    assert!(object_id.starts_with("remote-"));

    // Remote handles are host-owned. A page-visible property with the
    // legacy predictable name must not be able to replace or delete the
    // value retained for the protocol client.
    session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "globalThis.__cdp_object_0 = { count: 99 }; delete globalThis.__cdp_object_0; Object.prototype.__cdp_object_0 = { count: 777 }"
            }),
        )
        .unwrap();
    boa_gc::force_collect();

    let called = session
        .dispatch(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": "function(multiplier) { return this.count * multiplier; }",
                "arguments": [{ "value": 3 }],
                "returnByValue": true,
            }),
        )
        .unwrap();

    assert_eq!(called["result"]["value"], 6);
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "delete Object.prototype.__cdp_object_0" }),
        )
        .unwrap();

    let multiplier = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "({ factor: 3 })", "returnByValue": false }),
        )
        .unwrap()["result"]["objectId"]
        .as_str()
        .unwrap()
        .to_string();
    let called_with_remote_argument = session
        .dispatch(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": "function(other) { return this.count * other.factor; }",
                "arguments": [{ "objectId": multiplier }],
                "returnByValue": true,
            }),
        )
        .unwrap();
    assert_eq!(called_with_remote_argument["result"]["value"], 6);

    session
        .dispatch("Runtime.releaseObject", json!({ "objectId": object_id }))
        .unwrap();
    assert!(
        session
            .dispatch(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": "function() { return this.count; }",
                    "returnByValue": true,
                }),
            )
            .is_err()
    );
}

#[test]
fn runtime_remote_object_handles_expire_when_navigation_replaces_runtime() {
    let mut session = CdpSession::new().unwrap();
    let object_id = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "({ old: true })", "returnByValue": false }),
        )
        .unwrap()["result"]["objectId"]
        .as_str()
        .unwrap()
        .to_string();

    session
        .dispatch(
            "Page.navigate",
            json!({ "url": "data:text/html,<html><body>new</body></html>" }),
        )
        .unwrap();

    assert!(
        session
            .dispatch(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": "function() { return this.old; }",
                    "returnByValue": true,
                }),
            )
            .is_err()
    );
}

#[test]
fn remote_objects_survive_same_document_history_but_expire_on_document_commit() {
    let mut session = CdpSession::new().unwrap();
    let object = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "({ count: 7 })",
                "returnByValue": false,
            }),
        )
        .unwrap();
    let object_id = object["result"]["objectId"].as_str().unwrap().to_string();
    let initial_generation = session.document_generation;

    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "history.pushState({ same: true }, '')" }),
        )
        .unwrap();
    assert_eq!(session.document_generation, initial_generation);
    let same_document_call = session
        .dispatch(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": "function() { return this.count; }",
                "returnByValue": true,
            }),
        )
        .unwrap();
    assert_eq!(same_document_call["result"]["value"], 7);

    session
        .dispatch(
            "Page.navigate",
            json!({ "url": "data:text/html,<p>replacement</p>" }),
        )
        .unwrap();
    let stale = session
        .dispatch(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": "function() { return this.count; }",
            }),
        )
        .unwrap_err();
    assert_eq!(stale.code, -32000);
    assert_eq!(
        stale.message,
        format!("Could not find object with given id: {object_id}")
    );

    let replacement = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "({ fresh: true })",
                "returnByValue": false,
            }),
        )
        .unwrap();
    let replacement_id = replacement["result"]["objectId"]
        .as_str()
        .unwrap()
        .to_string();
    session.dispatch("Page.reload", json!({})).unwrap();
    let stale_argument = session
        .dispatch(
            "Runtime.callFunctionOn",
            json!({
                "functionDeclaration": "function(value) { return value; }",
                "arguments": [{ "objectId": replacement_id }],
                "returnByValue": true,
            }),
        )
        .unwrap_err();
    assert_eq!(stale_argument.code, -32000);
    assert_eq!(
        stale_argument.message,
        format!("Could not find object with given id: {replacement_id}")
    );
    assert!(session.remote_object_generations.is_empty());
}

#[test]
fn target_and_input_domains_manage_contexts_and_dispatch_dom_events() {
    let mut session = CdpSession::new().unwrap();

    let first = session
        .dispatch("Target.createBrowserContext", json!({}))
        .unwrap();
    let second = session
        .dispatch("Target.createBrowserContext", json!({}))
        .unwrap();
    let contexts = session
        .dispatch("Target.getBrowserContexts", json!({}))
        .unwrap();
    assert_eq!(contexts["browserContextIds"].as_array().unwrap().len(), 2);

    session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "globalThis.keyCount = 0; globalThis.mouseCount = 0; document.addEventListener('keydown', () => { globalThis.keyCount += 1; }); document.addEventListener('click', () => { globalThis.mouseCount += 1; }); 0",
                "returnByValue": true
            }),
        )
        .unwrap();
    session
        .dispatch("Input.dispatchKeyEvent", json!({ "type": "keydown" }))
        .unwrap();
    session
        .dispatch("Input.dispatchMouseEvent", json!({ "type": "click" }))
        .unwrap();

    let counts = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "JSON.stringify({ keyCount: globalThis.keyCount, mouseCount: globalThis.mouseCount })",
                "returnByValue": true
            }),
        )
        .unwrap();
    assert_eq!(
        counts["result"]["value"],
        "{\"keyCount\":1,\"mouseCount\":1}"
    );

    session
        .dispatch(
            "Target.disposeBrowserContext",
            json!({ "browserContextId": first["browserContextId"] }),
        )
        .unwrap();
    let remaining = session
        .dispatch("Target.getBrowserContexts", json!({}))
        .unwrap();
    assert_eq!(remaining["browserContextIds"].as_array().unwrap().len(), 1);
    assert_eq!(
        remaining["browserContextIds"][0],
        second["browserContextId"].clone()
    );
}

#[test]
fn pointer_leaving_surface_clears_hover_selector() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/",
            "<html><body><button id='target' style='width:100px;height:50px'>Go</button></body></html>",
            1,
            "null",
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseMoved","x":20,"y":20}),
        )
        .unwrap();
    assert_eq!(
        session
            .runtime
            .eval("document.getElementById('target').matches(':hover')")
            .unwrap()
            .as_boolean(),
        Some(true),
    );
    session.pointer_left_surface();
    assert_eq!(
        session
            .runtime
            .eval("document.getElementById('target').matches(':hover')")
            .unwrap()
            .as_boolean(),
        Some(false),
    );
}

#[test]
fn native_input_does_not_run_due_timer_tasks() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/",
            "<html><body style='height:1000px'></body></html>",
            1,
            "null",
        )
        .unwrap();
    session
        .runtime
        .eval("globalThis.timerFired = false; setTimeout(() => timerFired = true, 1000)")
        .unwrap();
    session
        .runtime
        .eval("document.addEventListener('wheel', () => { globalThis.wheelFired = true; history.pushState(null, '', '/wheel/first'); history.pushState(null, '', '/wheel'); })")
        .unwrap();
    session.advance_clock_without_tasks(1000);

    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseWheel", "x": 20, "y": 20, "deltaY": 10 }),
        )
        .unwrap();
    assert_eq!(
        session.runtime.eval("wheelFired").unwrap().as_boolean(),
        Some(true),
    );
    assert_eq!(session.current_url(), "https://example.test/wheel");
    assert_eq!(
        session.runtime.eval("timerFired").unwrap().as_boolean(),
        Some(false),
    );

    session.drive_event_loop(0).unwrap();
    assert_eq!(
        session.runtime.eval("timerFired").unwrap().as_boolean(),
        Some(true),
    );
}

fn cross_origin_wheel_session(child_html: &str) -> (CdpSession, NodeHandle, usize) {
    let child = format!(
        "data:text/html;base64,{}",
        BASE64_STANDARD.encode(child_html)
    );
    let mut session = CdpSession::new().unwrap();
    session.set_viewport(200, 100);
    session
        .install_document(
            "https://top.test/",
            &format!(
                "<style>*{{margin:0}}iframe{{display:block;border:0;width:200px;height:100px}}</style>\
                 <iframe id=child src='{child}'></iframe>"
            ),
            1,
            "null",
        )
        .unwrap();
    for _ in 0..8 {
        session.drive_event_loop(0).unwrap();
    }
    assert_eq!(
        session
            .runtime
            .eval("document.getElementById('child').contentDocument === null")
            .unwrap()
            .as_boolean(),
        Some(true),
        "the data URL child must remain cross-origin to the parent Realm",
    );

    let target = session
        .runtime
        .hit_test(10.0, 10.0)
        .expect("the child target must be hit");
    assert_eq!(target.get_attribute("id").as_deref(), Some("target"));
    let target_document_id = session
        .runtime
        .input_position_for_node(&target, 10.0, 10.0)
        .4;
    (session, target, target_document_id)
}

#[test]
fn native_wheel_enters_cross_origin_hit_target_realm_without_exposing_it_to_author_code() {
    let (mut session, target, _) = cross_origin_wheel_session(
        "<style>*{margin:0}#target{height:1000px}</style><div id=target>child</div>\
         <script>const target=document.getElementById('target');\
         target.addEventListener('wheel',event=>target.setAttribute('data-wheel',\
         [event.deltaY,Object.getPrototypeOf(event)===WheelEvent.prototype,\
         event.target.ownerDocument===document].join(':')));\
         target.addEventListener('wheel',()=>Promise.resolve().then(()=>\
         target.setAttribute('data-job',String(target.ownerDocument===document))))</script>",
    );
    let target_id = target.identity();
    let author_result = session
        .runtime
        .eval(&format!(
            "(()=>{{try{{__omoikane_dispatch_wheel_input({target_id},{{deltaX:0,deltaY:40}});\
             return 'allowed'}}catch(error){{return error.name}}}})()"
        ))
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(author_result, "SecurityError");

    let result = session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseWheel","x":10,"y":10,"deltaX":0,"deltaY":40}),
        )
        .unwrap();
    assert_eq!(result["defaultPrevented"], json!(false));
    assert_eq!(result["hostOverscrollY"], json!(false));
    assert_eq!(
        target.get_attribute("data-wheel").as_deref(),
        Some("40:true:true"),
        "the child listener and WheelEvent must use the child Realm",
    );
    assert_eq!(target.get_attribute("data-job").as_deref(), Some("true"));
}

#[test]
fn native_cross_origin_wheel_propagates_prevent_default_to_the_host_result() {
    let (mut session, target, _) = cross_origin_wheel_session(
        "<style>*{margin:0}#target{height:1000px}</style><div id=target>child</div>\
         <script>const target=document.getElementById('target');\
         target.addEventListener('wheel',event=>{event.preventDefault();\
         target.setAttribute('data-cancel',\
         [event.cancelable,event.defaultPrevented,event.target.ownerDocument===document].join(':'))})\
         </script>",
    );

    let result = session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseWheel","x":10,"y":10,"deltaX":0,"deltaY":40}),
        )
        .unwrap();
    assert_eq!(result["defaultPrevented"], json!(true));
    assert_eq!(result["hostOverscrollX"], json!(false));
    assert_eq!(result["hostOverscrollY"], json!(false));
    assert_eq!(
        target.get_attribute("data-cancel").as_deref(),
        Some("true:true:true"),
        "the child Realm must cancel the trusted event before the host reads the outcome",
    );
}

#[test]
fn input_realm_evaluation_restores_the_main_realm_after_an_error() {
    let (mut session, _, target_document_id) = cross_origin_wheel_session(
        "<style>*{margin:0}#target{height:1000px}</style><div id=target>child</div>\
         <script>globalThis.realmMarker='child'</script>",
    );
    session
        .runtime
        .eval("globalThis.realmMarker = 'top'")
        .unwrap();

    let error = session
        .runtime
        .eval_input_in_document_realm(target_document_id, "throw new Error('input realm failure')")
        .expect_err("the child input evaluation must propagate its exception");
    assert!(
        error.to_string().contains("input realm failure"),
        "unexpected child error: {error}",
    );
    assert_eq!(
        session
            .runtime
            .eval("realmMarker + ':' + document.URL")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "top:https://top.test/",
        "an exceptional child evaluation must restore the calling main Realm",
    );
}

#[test]
fn input_realm_evaluation_rejects_a_retired_document() {
    let (mut session, _, target_document_id) = cross_origin_wheel_session(
        "<style>*{margin:0}#target{height:1000px}</style><div id=target>child</div>",
    );
    session
        .runtime
        .eval("document.getElementById('child').remove()")
        .unwrap();

    let error = session
        .runtime
        .eval_input_in_document_realm(target_document_id, "globalThis.retiredInputRan = true")
        .expect_err("a retired iframe Document must reject trusted input evaluation");
    assert!(
        error
            .to_string()
            .contains("input Document Realm is not active"),
        "unexpected retired Document error: {error}",
    );
    assert_eq!(
        session
            .runtime
            .eval("document.URL")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "https://top.test/",
        "rejecting a retired child Document must leave the main Realm active",
    );
}

#[test]
fn mouse_input_hit_tests_paint_order_transforms_clips_and_scroll() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/",
            r#"<html><head><style>
                    * { margin: 0; padding: 0; } body { height: 1000px; }
                    .box { position: absolute; width: 80px; height: 80px; }
                    #back { left: 0; top: 0; z-index: 1; }
                    #front { left: 0; top: 0; width: 50px; height: 50px; z-index: 2; }
                    #none { left: 0; top: 0; z-index: 3; pointer-events: none; }
                    #transformed { left: 0; top: 120px; width: 40px; height: 40px;
                                   transform: translateX(100px); }
                    #under { position: absolute; left: 0; top: 200px; width: 100px; height: 50px; }
                    #clip { position: absolute; left: 0; top: 200px; width: 50px; height: 50px;
                            overflow: hidden; z-index: 2; }
                    #clipped { position: absolute; left: 60px; top: 0; width: 30px; height: 30px; }
                    #scroller { position: absolute; left: 0; top: 300px; width: 100px; height: 50px;
                                overflow: hidden; }
                    #content { position: relative; height: 200px; }
                    #scrolled { position: absolute; left: 0; top: 100px; width: 60px; height: 30px; }
                    #windowScrolled { position: absolute; left: 0; top: 450px; width: 60px; height: 30px; }
                </style></head><body>
                  <div id="back" class="box"></div><div id="front" class="box"></div>
                  <div id="none" class="box"></div><div id="transformed" class="box"></div>
                  <div id="under"></div><div id="clip"><div id="clipped"></div></div>
                  <div id="scroller"><div id="content"><button id="scrolled">s</button></div></div>
                  <button id="windowScrolled">w</button>
                  <script>globalThis.hits=[]; for (const id of ['back','front','none','transformed','under','clipped','scrolled','windowScrolled']) document.getElementById(id).addEventListener('mousemove',()=>hits.push(id)); scroller.scrollTop=80;</script>
                </body></html>"#,
            1,
            "null",
        )
        .unwrap();

    for (x, y) in [(10, 10), (110, 130), (70, 210), (10, 325)] {
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({ "type": "mouseMoved", "x": x, "y": y }),
            )
            .unwrap();
    }
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "scrollTo(0,50)", "returnByValue": true }),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseMoved", "x": 10, "y": 410 }),
        )
        .unwrap();
    let hits = session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "hits.join(',')", "returnByValue": true }),
        )
        .unwrap();
    assert_eq!(
        hits["result"]["value"],
        "front,transformed,under,scrolled,windowScrolled"
    );
}

#[test]
fn mouse_input_sequences_clicks_focus_and_reports_prevent_default() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/",
            r#"<html><head><style>*{margin:0} #ok,#blocked{position:absolute;width:80px;height:40px}#ok{left:0;top:0}#blocked{left:100px;top:0}</style></head><body>
                    <button id="ok">ok</button><input id="blocked">
                    <script>globalThis.events=[];globalThis.mouseFields='';globalThis.mouseBubble='';globalThis.moveFields=''; for(const type of ['mousedown','mouseup','click']) { ok.addEventListener(type,e=>{events.push('ok:'+type);if(type==='mousedown')mouseFields=[e.clientX,e.clientY,e.pageX,e.pageY,e.button,e.buttons,e.altKey,e.shiftKey,e.bubbles,e.composed].join(':')}); blocked.addEventListener(type,e=>{events.push('blocked:'+type);if(type==='mousedown')e.preventDefault()}) } ok.addEventListener('mousemove',e=>moveFields=[e.button,e.buttons,e.cancelable].join(':')); document.addEventListener('mousedown',e=>mouseBubble=e.target.id);</script>
                </body></html>"#,
            1,
            "null",
        )
        .unwrap();

    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mousePressed","x":10,"y":10,"button":"left","buttons":1,"modifiers":9}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseReleased","x":10,"y":10,"button":"left","buttons":0}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mousePressed","x":10,"y":10,"button":"left","modifiers":9}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseReleased","x":110,"y":10,"button":"left"}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseMoved","x":10,"y":10}),
        )
        .unwrap();
    let prevented = session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mousePressed","x":110,"y":10,"button":"left"}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseReleased","x":110,"y":10,"button":"left"}),
        )
        .unwrap();

    assert_eq!(prevented["defaultPrevented"], true);
    let state = session.dispatch("Runtime.evaluate", json!({
        "expression": "JSON.stringify({events,active:document.activeElement.id,mouseFields,moveFields,mouseBubble})",
        "returnByValue": true
    })).unwrap();
    assert_eq!(
        state["result"]["value"],
        r#"{"events":["ok:mousedown","ok:mouseup","ok:click","ok:mousedown","blocked:mouseup","blocked:mousedown","blocked:mouseup","blocked:click"],"active":"ok","mouseFields":"10:10:10:10:0:1:true:true:true:true","moveFields":"0:0:true","mouseBubble":"blocked"}"#
    );
}

#[test]
fn mouse_input_dispatches_deterministic_drag_lifecycle_and_shared_data_transfer() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/",
            r#"<html><head><style>*{margin:0}#source,#target{position:absolute;top:0;width:80px;height:40px}#source{left:0}#target{left:100px}</style></head><body>
                    <div id="source" draggable="true">source</div><div id="target">target</div>
                    <script>
                      globalThis.dragEvents=[];globalThis.transfer=null;globalThis.sameTransfer=true;globalThis.clicks=0;
                      source.addEventListener('click',()=>clicks++);
                      source.addEventListener('dragstart',e=>{dragEvents.push('start:'+e.bubbles+':'+e.cancelable+':'+(e instanceof DragEvent));transfer=e.dataTransfer;e.dataTransfer.setData('text/plain','payload')});
                      source.addEventListener('drag',e=>{dragEvents.push('drag');sameTransfer=sameTransfer&&e.dataTransfer===transfer});
                      source.addEventListener('dragover',e=>{dragEvents.push('over');sameTransfer=sameTransfer&&e.dataTransfer===transfer;e.preventDefault()});
                      source.addEventListener('drop',e=>{dragEvents.push('drop:'+e.dataTransfer.getData('text/plain'));sameTransfer=sameTransfer&&e.dataTransfer===transfer});
                      target.addEventListener('dragenter',e=>{dragEvents.push('enter');sameTransfer=sameTransfer&&e.dataTransfer===transfer});
                      target.addEventListener('dragleave',e=>{dragEvents.push('leave');sameTransfer=sameTransfer&&e.dataTransfer===transfer});
                      target.addEventListener('dragover',e=>{dragEvents.push('over');sameTransfer=sameTransfer&&e.dataTransfer===transfer;e.preventDefault()});
                      target.addEventListener('drop',e=>{dragEvents.push('drop:'+e.dataTransfer.getData('text/plain'));sameTransfer=sameTransfer&&e.dataTransfer===transfer});
                      source.addEventListener('dragend',e=>{dragEvents.push('end:'+e.dataTransfer.getData('text/plain'));sameTransfer=sameTransfer&&e.dataTransfer===transfer});
                    </script>
                </body></html>"#,
            1,
            "null",
        )
        .unwrap();

    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mousePressed","x":10,"y":10,"button":"left","buttons":1}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseMoved","x":110,"y":10,"button":"none","buttons":1}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseMoved","x":110,"y":10,"button":"none","buttons":1}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseReleased","x":10,"y":10,"button":"left","buttons":0}),
        )
        .unwrap();

    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "JSON.stringify({events:dragEvents,sameTransfer,clicks:globalThis.clicks||0})",
                "returnByValue": true,
            }),
        )
        .unwrap();
    assert_eq!(
        state["result"]["value"],
        r#"{"events":["start:true:true:true","drag","enter","over","drag","over","leave","over","drop:payload","end:payload"],"sameTransfer":true,"clicks":0}"#
    );
}

#[test]
fn canceled_dragstart_consumes_candidate_and_keeps_click_default() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/",
            r#"<html><head><style>*{margin:0}#source{position:absolute;left:0;top:0;width:80px;height:40px}</style></head><body>
                    <div id="source" draggable="true">source</div>
                    <script>
                      globalThis.events=[];globalThis.clicks=0;
                      source.addEventListener('dragstart',e=>{events.push('dragstart');e.preventDefault()});
                      source.addEventListener('click',()=>{events.push('click');clicks++});
                    </script>
                </body></html>"#,
            1,
            "null",
        )
        .unwrap();

    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mousePressed","x":10,"y":10,"button":"left","buttons":1}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseMoved","x":20,"y":10,"button":"none","buttons":1}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseMoved","x":30,"y":10,"button":"none","buttons":1}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseReleased","x":10,"y":10,"button":"left","buttons":0}),
        )
        .unwrap();

    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "JSON.stringify({events,clicks})",
                "returnByValue": true,
            }),
        )
        .unwrap();
    assert_eq!(
        state["result"]["value"],
        r#"{"events":["dragstart","click"],"clicks":1}"#
    );
}

#[test]
fn mouse_hit_test_respects_pointer_events_and_svg_geometry() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/",
            r#"<html><head><style>*{margin:0}#back,#overlay{position:absolute;left:0;top:0;width:120px;height:80px}#back{background:gray}#overlay{background:red;pointer-events:none}#icon{position:absolute;left:130px;top:0;width:100px;height:100px;pointer-events:none}#shape{pointer-events:fill}</style></head><body>
                    <div id="back"></div><div id="overlay"></div>
                    <svg id="icon" width="100" height="100" viewBox="0 0 100 100">
                      <rect id="base" x="0" y="0" width="100" height="100" fill="red" pointer-events="none"></rect>
                      <rect id="shape" x="20" y="20" width="30" height="30" fill="blue"></rect>
                      <rect id="stroke" x="60" y="20" width="30" height="30" fill="none" stroke="green" stroke-width="4" pointer-events="stroke"></rect>
                      <g id="inherited" stroke-width="10">
                        <line id="wide" x1="10" y1="70" x2="90" y2="70" fill="none" stroke="green" pointer-events="stroke"></line>
                      </g>
                      <circle id="bounds" cx="70" cy="70" r="10" fill="none" pointer-events="bounding-box"></circle>
                    </svg>
                    <script>
                      globalThis.targets=[];
                      document.addEventListener('click',e=>targets.push(e.target.id||e.target.localName));
                    </script>
                </body></html>"#,
            1,
            "null",
        )
        .unwrap();

    for (x, y) in [(10, 10), (155, 35), (190, 35), (140, 66), (190, 60)] {
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mousePressed","x":x,"y":y,"button":"left","buttons":1}),
            )
            .unwrap();
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mouseReleased","x":x,"y":y,"button":"left","buttons":0}),
            )
            .unwrap();
    }
    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "JSON.stringify({targets,none:getComputedStyle(document.getElementById('overlay')).pointerEvents})",
                "returnByValue": true,
            }),
        )
        .unwrap();
    assert_eq!(
        state["result"]["value"],
        r#"{"targets":["back","shape","stroke","wide","bounds"],"none":"none"}"#
    );
}

#[test]
fn mouse_hit_test_maps_svg_children_to_the_painted_content_box() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/",
            r#"<html><head><style>*{margin:0}#back{position:absolute;left:0;top:0;width:240px;height:180px;background:gray}#icon{position:absolute;left:20px;top:20px;width:100px;height:100px;padding:10px;border:5px solid black;pointer-events:none}</style></head><body>
                    <div id="back"></div>
                    <svg id="icon" width="100" height="100" viewBox="0 0 100 100">
                      <rect id="shape" x="0" y="0" width="100" height="100" fill="blue" pointer-events="fill"></rect>
                    </svg>
                    <script>
                      globalThis.targets=[];
                      document.addEventListener('click',e=>targets.push(e.target.id||e.target.localName));
                    </script>
                </body></html>"#,
            1,
            "null",
        )
        .unwrap();

    let bounds = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression": "(()=>{const r=document.getElementById('icon').getBoundingClientRect();return JSON.stringify([r.x,r.y,r.width,r.height])})()",
                "returnByValue": true,
            }),
        )
        .unwrap();
    let bounds: Vec<f32> =
        serde_json::from_str(bounds["result"]["value"].as_str().expect("SVG bounds JSON")).unwrap();
    assert_eq!(bounds.len(), 4);
    for (x, y) in [
        (bounds[0] + 2.0, bounds[1] + bounds[3] / 2.0),
        (bounds[0] + bounds[2] / 2.0, bounds[1] + bounds[3] / 2.0),
    ] {
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mousePressed","x":x,"y":y,"button":"left","buttons":1}),
            )
            .unwrap();
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mouseReleased","x":x,"y":y,"button":"left","buttons":0}),
            )
            .unwrap();
    }
    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression":"JSON.stringify(targets)","returnByValue":true}),
        )
        .unwrap();
    assert_eq!(state["result"]["value"], r#"["back","shape"]"#);
}

#[test]
fn mouse_hit_test_targets_geometry_painted_through_svg_use() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/",
            r##"<html><body><svg id="icon" width="100" height="100" viewBox="0 0 100 100">
                    <defs><rect id="template" x="0" y="0" width="20" height="20" fill="blue"></rect></defs>
                    <use id="instance" href="#template" x="40" y="40" pointer-events="fill"></use>
                    <use id="blocked" href="#template" x="40" y="40" pointer-events="none"></use>
                    <script>globalThis.targets=[];document.addEventListener('click',e=>targets.push(e.target.id||e.target.localName));</script>
                </svg></body></html>"##,
            1,
            "null",
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mousePressed","x":50,"y":50,"button":"left","buttons":1}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseReleased","x":50,"y":50,"button":"left","buttons":0}),
        )
        .unwrap();
    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression":"JSON.stringify({targets,instancePE:getComputedStyle(document.getElementById('instance')).pointerEvents,blockedPE:getComputedStyle(document.getElementById('blocked')).pointerEvents,href:document.getElementById('instance').getAttribute('href')})","returnByValue":true}),
        )
        .unwrap();
    assert_eq!(
        state["result"]["value"],
        r##"{"targets":["instance"],"instancePE":"fill","blockedPE":"none","href":"#template"}"##,
    );
}

#[test]
fn keyboard_input_targets_the_focused_element_with_cdp_fields() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/",
            r#"<html><body><input id="field"><script>
                    globalThis.keys=[]; field.focus();
                    field.addEventListener('keydown',e=>{keys.push(['field',e.type,e.key,e.code,e.keyCode,e.ctrlKey,e.shiftKey,e.bubbles,e.composed].join(':'));e.preventDefault()});
                    field.addEventListener('keyup',e=>keys.push(['field',e.type,e.key].join(':')));
                    field.addEventListener('keypress',e=>keys.push(['field',e.type,e.key,e.charCode].join(':')));
                    document.addEventListener('keydown',e=>keys.push('document:'+e.target.id));
                </script></body></html>"#,
            1,
            "null",
        )
        .unwrap();

    let down = session
        .dispatch(
            "Input.dispatchKeyEvent",
            json!({
                "type":"keyDown","key":"A","code":"KeyA","windowsVirtualKeyCode":65,"modifiers":10
            }),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchKeyEvent",
            json!({"type":"keyUp","key":"A","code":"KeyA"}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchKeyEvent",
            json!({"type":"char","text":"a","key":"a"}),
        )
        .unwrap();
    assert_eq!(down["defaultPrevented"], true);
    let keys = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression":"keys.join('|')","returnByValue":true
            }),
        )
        .unwrap();
    assert_eq!(
        keys["result"]["value"],
        "field:keydown:A:KeyA:65:true:true:true:true|document:field|field:keyup:A|field:keypress:a:97"
    );
}

#[test]
fn ime_composition_rejects_invalid_selection_ranges() {
    let mut session = CdpSession::new().unwrap();
    for (start, end) in [(2, 1), (0, 3)] {
        let error = session
            .dispatch(
                "Input.imeSetComposition",
                json!({ "text": "a", "selectionStart": start, "selectionEnd": end }),
            )
            .unwrap_err();
        assert_eq!(error.code, -32602);
        assert_eq!(
            error.message,
            "Composition selection must be ordered and within the text"
        );
    }
}

#[test]
fn keyboard_input_edits_focused_text_control_end_to_end() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/",
            r#"<html><body><input id="login"><script>
                    globalThis.editEvents=[];
                    login.addEventListener('beforeinput',e=>editEvents.push(e.type+':'+e.inputType+':'+e.data));
                    login.addEventListener('input',e=>editEvents.push(e.type+':'+e.inputType+':'+e.data));
                    login.focus();
                </script></body></html>"#,
            1,
            "null",
        )
        .unwrap();

    for character in ["m", "i", "k", "u"] {
        session
            .dispatch(
                "Input.dispatchKeyEvent",
                json!({"type":"keyDown","key":character,"text":character}),
            )
            .unwrap();
    }
    session
        .dispatch(
            "Input.dispatchKeyEvent",
            json!({"type":"keyDown","key":"ArrowLeft"}),
        )
        .unwrap();
    session
        .dispatch(
            "Input.dispatchKeyEvent",
            json!({"type":"keyDown","key":"Backspace"}),
        )
        .unwrap();

    let state = session
        .dispatch(
            "Runtime.evaluate",
            json!({
                "expression":"JSON.stringify({value:login.value,start:login.selectionStart,end:login.selectionEnd,events:editEvents})",
                "returnByValue":true
            }),
        )
        .unwrap();
    assert_eq!(
        state["result"]["value"],
        r#"{"value":"miu","start":2,"end":2,"events":["beforeinput:insertText:m","input:insertText:m","beforeinput:insertText:i","input:insertText:i","beforeinput:insertText:k","input:insertText:k","beforeinput:insertText:u","input:insertText:u","beforeinput:deleteContentBackward:null","input:deleteContentBackward:null"]}"#
    );
}
