//! Worker preparation uses the creating document's policy and immutable origin.
use super::*;
use crate::test_support::http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

const POLICY: &str = "connect-src 'none'; report-to worker-tests";

struct WorkerOwnerFixture {
    origin: String,
    stopped: Arc<AtomicBool>,
    worker: Option<FixtureWorker<Vec<String>>>,
}

impl WorkerOwnerFixture {
    fn new(report_only: bool) -> Self {
        let listener = bind_loopback().unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let worker = FixtureWorker::spawn(move || {
            let deadline = Instant::now() + ACCEPT_TIMEOUT;
            let mut paths = Vec::new();
            while !worker_stopped.load(Ordering::Acquire) {
                assert!(Instant::now() < deadline, "worker owner fixture timed out");
                let mut stream = match accept_with_timeout(&listener, Duration::from_millis(20)) {
                    Ok(stream) => stream,
                    Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
                    Err(error) => panic!("worker owner accept failed: {error}"),
                };
                let headers = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                let path = headers
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                paths.push(path.to_owned());
                match path {
                    "/child/page" => {
                        let header = if report_only {
                            "Content-Security-Policy-Report-Only"
                        } else {
                            "Content-Security-Policy"
                        };
                        let body = "<html><body><script>globalThis.workerOwnerRealm = true;</script></body></html>";
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n{header}: {POLICY}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    "/resource" => stream.write_all(b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                    "/child/worker.js" => {
                        let body = "onconnect = event => event.ports[0].postMessage(location.href);";
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    _ => stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                }
            }
            paths
        });
        Self {
            origin,
            stopped,
            worker: Some(worker),
        }
    }

    fn finish(mut self) -> Vec<String> {
        self.stopped.store(true, Ordering::Release);
        self.worker.take().unwrap().join()
    }
}

impl Drop for WorkerOwnerFixture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

fn data_script_url(source: &str) -> String {
    format!(
        "data:text/javascript;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(source)
    )
}

fn runtime_with_worker_owner(fixture: &WorkerOwnerFixture) -> (JsRuntime, usize) {
    let document =
        crate::html::TreeBuilder::parse("<body><iframe src='/child/page'></iframe>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, &format!("{}/parent", fixture.origin)).unwrap();
    runtime
        .eval(
            r#"
        globalThis.received = [], globalThis.parentEvents = [], globalThis.parentReports = [];
        addEventListener('message', event => received.push(event.data));
        document.addEventListener('securitypolicyviolation', event => parentEvents.push(event));
        new ReportingObserver(batch => parentReports.push(...batch)).observe();
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let iframe = runtime.document().query_selector("iframe").unwrap();
    let child = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&iframe)
        .unwrap();
    assert!(
        runtime
            .existing_realm_for_document(child.identity())
            .is_some()
    );
    runtime
        .eval_in_document_realm(
            child.identity(),
            r#"
        globalThis.ownerEvents = [], globalThis.ownerReports = [];
        document.addEventListener('securitypolicyviolation', event => ownerEvents.push(event));
        new ReportingObserver(batch => ownerReports.push(...batch)).observe();
    "#,
        )
        .unwrap();
    (runtime, child.identity())
}

fn reporting_source(resource: &str) -> String {
    let resource = serde_json::to_string(resource).unwrap();
    format!(
        r#"
        let active = null, connections = 0;
        function publish() {{
            if (active) active.port.postMessage({{
                connection: active.connection, outcome: active.outcome,
                events: active.events, reports: active.reports
            }});
        }}
        addEventListener('securitypolicyviolation', event => {{
            if (!active) return;
            active.events.push([event.target === self, event.currentTarget === self,
                event.isTrusted, event.originalPolicy, event.disposition,
                event.sourceFile, event.lineNumber > 0, event.columnNumber > 0]);
            publish();
        }});
        new ReportingObserver(batch => {{
            if (!active) return;
            active.reports.push(...batch.map(report => [report.url,
                report.body.originalPolicy, report.body.disposition,
                report.body.blockedURL, report.body.statusCode,
                report.body.sourceFile, report.body.lineNumber > 0,
                report.body.columnNumber > 0]));
            publish();
        }}).observe();
        function fetchResource(port, connection) {{
            active = {{port, connection, outcome: 'pending', events: [], reports: []}};
            fetch({resource}).then(response => {{
                active.outcome = response.status; publish();
            }}, error => {{
                active.outcome = error instanceof TypeError ? 'TypeError' : error.name;
                publish();
            }});
        }}
        onmessage = () => fetchResource({{postMessage: value => postMessage(value)}}, 1);
        onconnect = event => {{
            const port = event.ports[0], connection = ++connections;
            port.onmessage = () => fetchResource(port, connection);
            port.start();
        }};
    "#
    )
}

fn connect_from_child(runtime: &mut JsRuntime, child_id: usize, url: &str) {
    runtime
        .eval_in_document_realm(
            child_id,
            &format!(
                r#"
        globalThis.worker = new SharedWorker({}, 'iframe-policy');
        worker.port.onmessage = event => parent.postMessage(event.data, '*');
        worker.port.start(); worker.port.postMessage('fetch');
    "#,
                serde_json::to_string(url).unwrap()
            ),
        )
        .unwrap();
}

fn shared_observation(runtime: &mut JsRuntime) -> Value {
    runtime.run_until_idle().unwrap();
    let result = runtime.eval("JSON.stringify(received)").unwrap();
    let received: Value =
        serde_json::from_str(&result.as_string().unwrap().to_std_string_escaped()).unwrap();
    received
        .as_array()
        .unwrap()
        .last()
        .expect("worker must publish a fetch result")
        .clone()
}

fn assert_shared_observation(observed: Value, report_only: bool, connection: u32, resource: &str) {
    let (disposition, count, outcome) = if report_only {
        ("report", 2, json!(204))
    } else {
        ("enforce", 1, json!("TypeError"))
    };
    assert_eq!(
        observed,
        json!({
            "connection": connection, "outcome": outcome,
            "events": vec![json!([
                true, true, true, POLICY, disposition, "data", true, true
            ]); count],
            "reports": vec![json!([
                "data", POLICY, disposition, resource, 200, "data", true, true
            ]); count],
        })
    );
}

#[test]
fn iframe_shared_data_worker_inherits_creator_policy_and_keeps_it_when_reused() {
    for report_only in [false, true] {
        let fixture = WorkerOwnerFixture::new(report_only);
        let (mut runtime, child_id) = runtime_with_worker_owner(&fixture);
        let resource = format!("{}/resource", fixture.origin);
        let url = data_script_url(&reporting_source(&resource));
        connect_from_child(&mut runtime, child_id, &url);
        let first = shared_observation(&mut runtime);
        assert_shared_observation(first, report_only, 1, &resource);
        runtime.install_response_security_headers(&[(
            "Content-Security-Policy".into(),
            "connect-src *; report-to parent".into(),
        )]);
        runtime
            .eval(&format!(
                r#"
            received = [];
            globalThis.reused = new SharedWorker({}, 'iframe-policy');
            reused.port.onmessage = event => received.push(event.data);
            reused.port.start(); reused.port.postMessage('fetch');
        "#,
                serde_json::to_string(&url).unwrap()
            ))
            .unwrap();
        let second = shared_observation(&mut runtime);
        assert_shared_observation(second, report_only, 2, &resource);
        assert_eq!(
            runtime
                .eval(
                    r#"
            const owner = document.querySelector('iframe').contentWindow;
            parentEvents.length === 0 && parentReports.length === 0 &&
            owner.ownerEvents.length === 0 && owner.ownerReports.length === 0
        "#
                )
                .unwrap()
                .as_boolean(),
            Some(true)
        );
        assert!(runtime.take_task_errors().is_empty());
        drop(runtime);
        let paths = fixture.finish();
        assert_eq!(
            paths
                .iter()
                .filter(|path| path.as_str() == "/resource")
                .count(),
            if report_only { 2 } else { 0 }
        );
        assert_eq!(
            paths
                .iter()
                .filter(|path| path.as_str() == "/child/page")
                .count(),
            1
        );
    }
}

#[test]
fn iframe_shared_worker_resolves_against_its_own_document_base() {
    let fixture = WorkerOwnerFixture::new(false);
    let (mut runtime, child_id) = runtime_with_worker_owner(&fixture);
    runtime
        .eval_in_document_realm(
            child_id,
            r#"
        globalThis.relative = new SharedWorker('worker.js', 'relative-iframe');
        relative.port.onmessage = event => parent.postMessage(event.data, '*');
        relative.port.start();
    "#,
        )
        .unwrap();
    assert_eq!(
        shared_observation(&mut runtime),
        json!(format!("{}/child/worker.js", fixture.origin))
    );
    assert!(runtime.take_task_errors().is_empty());
    drop(runtime);
    assert_eq!(fixture.finish(), vec!["/child/page", "/child/worker.js"]);
}

#[test]
fn iframe_dedicated_data_worker_inherits_its_actual_creator_policy() {
    for report_only in [false, true] {
        let fixture = WorkerOwnerFixture::new(report_only);
        let (mut runtime, child_id) = runtime_with_worker_owner(&fixture);
        let resource = format!("{}/resource", fixture.origin);
        let url = data_script_url(&reporting_source(&resource));
        runtime
            .eval_in_document_realm(
                child_id,
                &format!(
                    r#"
            globalThis.worker = new Worker({});
            worker.onmessage = event => parent.postMessage(event.data, '*');
            worker.postMessage('fetch');
        "#,
                    serde_json::to_string(&url).unwrap()
                ),
            )
            .unwrap();
        let observed = shared_observation(&mut runtime);
        assert_shared_observation(observed, report_only, 1, &resource);
        assert_eq!(
            runtime
                .eval(
                    r#"
            const owner = document.querySelector('iframe').contentWindow;
            parentEvents.length === 0 && parentReports.length === 0 &&
            owner.ownerEvents.length === 0 && owner.ownerReports.length === 0
        "#
                )
                .unwrap()
                .as_boolean(),
            Some(true)
        );
        assert!(runtime.take_task_errors().is_empty());
        drop(runtime);
        let paths = fixture.finish();
        assert_eq!(
            paths
                .iter()
                .filter(|path| path.as_str() == "/resource")
                .count(),
            if report_only { 1 } else { 0 }
        );
    }
}

#[test]
fn clearing_resource_base_preserves_shared_worker_eligibility_and_registry_identity() {
    let mut runtime = JsRuntime::with_document_and_url(
        crate::html::TreeBuilder::parse("<body>").document(),
        "https://example.test/parent",
    )
    .unwrap();
    let url = data_script_url(
        "let connections=0; onconnect=event=>event.ports[0].postMessage(++connections);",
    );
    runtime
        .eval("globalThis.received = []; globalThis.connections = [];")
        .unwrap();
    for expected in [1, 2] {
        if expected == 2 {
            runtime.host_state.borrow_mut().base_url = None;
        }
        runtime
            .eval(&format!(
                r#"
            var worker = new SharedWorker({}, 'stable-owner-origin');
            worker.port.onmessage = event => received.push(event.data);
            worker.port.start(); connections.push(worker);
        "#,
                serde_json::to_string(&url).unwrap()
            ))
            .unwrap();
        assert_eq!(shared_observation(&mut runtime), json!(expected));
    }
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn shared_worker_requires_immutable_tuple_origin_even_with_http_resource_base() {
    let mut runtime = JsRuntime::with_document_and_url(
        crate::html::TreeBuilder::parse(
            "<body><iframe sandbox='allow-scripts' srcdoc='<body><script>globalThis.workerOwnerRealm=true;</script>'></iframe>",
        )
        .document(),
        "https://example.test/parent",
    )
    .unwrap();
    runtime.run_until_idle().unwrap();
    let iframe = runtime.document().query_selector("iframe").unwrap();
    let child = runtime
        .host_state
        .borrow_mut()
        .iframe_content_document(&iframe)
        .unwrap();
    assert!(
        runtime
            .existing_realm_for_document(child.identity())
            .is_some()
    );
    runtime.eval("globalThis.received = []; addEventListener('message', event => received.push(event.data));").unwrap();
    runtime.eval_in_document_realm(child.identity(), r#"
        try { new SharedWorker('data:text/javascript,onconnect=()=>{}'); parent.postMessage('created', '*'); }
        catch (error) { parent.postMessage('rejected', '*'); }
    "#).unwrap();
    assert_eq!(shared_observation(&mut runtime), json!("rejected"));
}

#[test]
fn opaque_iframe_can_start_dedicated_data_worker_with_actual_data_location() {
    let mut runtime = JsRuntime::with_document_and_url(
        crate::html::TreeBuilder::parse("<body>").document(),
        "https://example.test/parent",
    )
    .unwrap();
    let worker_url = data_script_url(
        "postMessage({href:location.href, origin:location.origin, secure:isSecureContext, locks:'locks' in navigator, storage:'storage' in navigator});",
    );
    let child_source = format!(
        r#"<body><script>
        try {{
            const worker = new Worker({});
            worker.onmessage = event => parent.postMessage({{
                creatorSecure: isSecureContext, worker: event.data
            }}, '*');
            worker.onerror = event => parent.postMessage({{error:event.message}}, '*');
        }} catch (error) {{ parent.postMessage({{error:error.name}}, '*'); }}
    </script>"#,
        serde_json::to_string(&worker_url).unwrap()
    );
    runtime
        .eval(&format!(
            r#"
        globalThis.received = [];
        addEventListener('message', event => received.push(event.data));
        const frame = document.createElement('iframe');
        frame.setAttribute('sandbox', 'allow-scripts');
        frame.srcdoc = {}; document.body.appendChild(frame);
    "#,
            serde_json::to_string(&child_source).unwrap()
        ))
        .unwrap();
    assert_eq!(
        shared_observation(&mut runtime),
        json!({"creatorSecure":true, "worker":{
            "href":worker_url, "origin":"null", "secure":true,
            "locks":false, "storage":false,
        }})
    );
    assert!(runtime.take_task_errors().is_empty());
    let state = runtime.host_state.borrow();
    let child_id = state
        .iframe_documents
        .values()
        .next()
        .unwrap()
        .document
        .identity();
    let origin = &state.document_security_origins[&child_id];
    assert!(matches!(origin, DocumentSecurityOrigin::Opaque(_)));
    let worker = state.workers.values().next().unwrap().borrow();
    let worker_state = worker.runtime.host_state.borrow();
    let worker_document = worker_state.document.identity();
    assert_eq!(
        &worker_state.document_security_origins[&worker_document],
        origin
    );
    assert_eq!(worker_state.secure_context_override, Some(true));
    assert!(worker_state.document_origins[&worker_document].is_none());
    assert!(worker_state.base_url.is_none());
}
