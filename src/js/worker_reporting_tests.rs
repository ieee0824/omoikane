//! Worker preparation uses the creating document's policy and immutable origin.
use super::*;
use crate::test_support::http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};
use serde_json::{Value, json};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};

const POLICY: &str = "connect-src 'none'; report-to worker-tests";

struct WorkerOwnerFixture {
    origin: String,
    stopped: Arc<AtomicBool>,
    worker: Option<FixtureWorker<Vec<String>>>,
}

impl WorkerOwnerFixture {
    fn new(report_only: bool) -> Self {
        Self::with_policy(report_only, POLICY)
    }

    fn with_policy(report_only: bool, policy: &str) -> Self {
        let listener = bind_loopback().unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let policy = policy.to_owned();
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
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n{header}: {policy}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
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

#[derive(Debug)]
struct CspRequest {
    method: String,
    path: String,
    body: String,
}

struct WorkerCspFixture {
    origin: String,
    stopped: Arc<AtomicBool>,
    worker: Option<FixtureWorker<Vec<CspRequest>>>,
}

impl WorkerCspFixture {
    fn new(import_header: Option<(&str, String)>, import_worker_source: &str) -> Self {
        let listener = bind_loopback().unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let import_header = import_header
            .map(|(name, value)| (name.to_owned(), value.replace("{ORIGIN}", &origin)));
        let import_worker_source = import_worker_source.to_owned();
        let worker = FixtureWorker::spawn(move || {
            let deadline = Instant::now() + ACCEPT_TIMEOUT;
            let mut requests = Vec::new();
            while !worker_stopped.load(Ordering::Acquire) {
                assert!(Instant::now() < deadline, "Worker CSP fixture timed out");
                let mut stream = match accept_with_timeout(&listener, Duration::from_millis(20)) {
                    Ok(stream) => stream,
                    Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
                    Err(error) => panic!("Worker CSP accept failed: {error}"),
                };
                let headers = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                let mut request_line = headers.lines().next().unwrap().split_whitespace();
                let method = request_line.next().unwrap().to_owned();
                let path = request_line.next().unwrap().to_owned();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Length"))
                    })
                    .and_then(|(_, length)| length.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let is_preflight = method == "OPTIONS";
                requests.push(CspRequest {
                    method,
                    path: path.clone(),
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                match path.as_str() {
                    "/dedicated.js" => {
                        let body = "postMessage('started');";
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    "/shared.js" => {
                        let body = "let connections=0;onconnect=e=>e.ports[0].postMessage(++connections);";
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    "/dedicated-globals.js" => {
                        let body = "postMessage([typeof DedicatedWorkerGlobalScope, typeof SharedWorkerGlobalScope]);";
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    "/shared-globals.js" => {
                        let body = "onconnect=e=>e.ports[0].postMessage([typeof DedicatedWorkerGlobalScope, typeof SharedWorkerGlobalScope]);";
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    "/import-worker.js" => {
                        let security_header = import_header.as_ref().map_or_else(String::new, |(name, value)| format!("{name}: {value}\r\nReporting-Endpoints: imports=\"/reports\"\r\n"));
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\n{security_header}Content-Length: {}\r\nConnection: close\r\n\r\n{import_worker_source}", import_worker_source.len()).unwrap();
                    }
                    "/import.js" => {
                        let body = "globalThis.imported=true;";
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    "/redirect" => stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: https://other.test/import.js\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                    "/import-redirect" => stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /import.js\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                    "/worker-redirect" => stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /dedicated.js\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                    "/reports" if is_preflight => stream.write_all(b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST\r\nAccess-Control-Allow-Headers: content-type\r\nAccess-Control-Max-Age: 600\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                    "/reports" => stream.write_all(b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                    _ => stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                }
            }
            requests
        });
        Self {
            origin,
            stopped,
            worker: Some(worker),
        }
    }

    fn finish(mut self) -> Vec<CspRequest> {
        self.stopped.store(true, Ordering::Release);
        self.worker.take().unwrap().join()
    }
}

impl Drop for WorkerCspFixture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
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
fn dedicated_worker_src_enforcement_blocks_before_http_and_sends_creator_report() {
    let fixture = WorkerCspFixture::new(None, "");
    let mut runtime = JsRuntime::with_document_and_url(
        crate::html::TreeBuilder::parse("<body></body>").document(),
        &format!("{}/page", fixture.origin),
    )
    .unwrap();
    runtime.install_response_security_headers(&[
        (
            "Content-Security-Policy".into(),
            "worker-src 'none'; report-to workers".into(),
        ),
        ("Reporting-Endpoints".into(), "workers=\"/reports\"".into()),
    ]);
    runtime.eval("globalThis.events=[];globalThis.reports=[];globalThis.errors=0;document.addEventListener('securitypolicyviolation',e=>events.push([e.target===document,e.effectiveDirective,e.blockedURI,e.disposition]));new ReportingObserver(batch=>reports.push(...batch)).observe();globalThis.worker=new Worker('/dedicated.js');worker.onerror=()=>errors++;").unwrap();
    runtime.run_until_idle().unwrap();
    security_reporting::wait_for_report_delivery(&mut runtime);
    let worker_url = format!("{}/dedicated.js", fixture.origin);
    assert_eq!(
        runtime
            .eval(&format!(
                "errors===1&&events.length===1&&events[0][0]&&events[0][1]==='worker-src'&&events[0][2]==={}&&events[0][3]==='enforce'&&reports.length===1&&reports[0].body.effectiveDirective==='worker-src'&&reports[0].body.blockedURL==={}",
                json!(worker_url),
                json!(worker_url),
            ))
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    drop(runtime);
    let requests = fixture.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/dedicated.js")
            .count(),
        0
    );
    let reports: Vec<_> = requests
        .iter()
        .filter(|request| request.path == "/reports" && request.method == "POST")
        .collect();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].method, "POST");
    let payload: Value = serde_json::from_str(&reports[0].body).unwrap();
    assert_eq!(payload[0]["body"]["blockedURL"], worker_url);
    assert_eq!(payload[0]["body"]["effectiveDirective"], "worker-src");
}

#[test]
fn dedicated_and_shared_workers_expose_only_their_own_global_scope_kind() {
    let fixture = WorkerCspFixture::new(None, "");
    let mut runtime = JsRuntime::with_document_and_url(
        crate::html::TreeBuilder::parse("<body></body>").document(),
        &format!("{}/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            "globalThis.dedicatedKinds=[];globalThis.sharedKinds=[];\
             globalThis.dedicated=new Worker('/dedicated-globals.js');\
             dedicated.onmessage=e=>dedicatedKinds.push(e.data);\
             globalThis.shared=new SharedWorker('/shared-globals.js');\
             shared.port.onmessage=e=>sharedKinds.push(e.data);shared.port.start();",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify({dedicatedKinds,sharedKinds})")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        r#"{"dedicatedKinds":[["function","undefined"]],"sharedKinds":[["undefined","function"]]}"#
    );
    drop(runtime);
    let requests = fixture.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/dedicated-globals.js")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/shared-globals.js")
            .count(),
        1
    );
}

#[test]
fn dedicated_worker_src_report_only_follows_redirect_and_keeps_initial_report_url() {
    let fixture = WorkerCspFixture::new(None, "");
    let mut runtime = JsRuntime::with_document_and_url(
        crate::html::TreeBuilder::parse("<body></body>").document(),
        &format!("{}/page", fixture.origin),
    )
    .unwrap();
    runtime.install_response_security_headers(&[
        (
            "Content-Security-Policy-Report-Only".into(),
            "worker-src 'none'; report-to workers".into(),
        ),
        ("Reporting-Endpoints".into(), "workers=\"/reports\"".into()),
    ]);
    runtime.eval("globalThis.events=[];globalThis.reports=[];globalThis.messages=[];document.addEventListener('securitypolicyviolation',e=>events.push([e.effectiveDirective,e.blockedURI,e.disposition]));new ReportingObserver(batch=>reports.push(...batch)).observe();globalThis.worker=new Worker('/worker-redirect');worker.onmessage=e=>messages.push(e.data);").unwrap();
    runtime.run_until_idle().unwrap();
    security_reporting::wait_for_report_delivery(&mut runtime);
    let initial = format!("{}/worker-redirect", fixture.origin);
    assert_eq!(
        runtime
            .eval(&format!(
                "messages.length===1&&messages[0]==='started'&&events.length===3&&events.every(e=>e[0]==='worker-src'&&e[1]==={}&&e[2]==='report')&&reports.length===3&&reports.every(r=>r.body.blockedURL==={}&&r.body.disposition==='report')",
                json!(initial),
                json!(initial),
            ))
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    drop(runtime);
    let requests = fixture.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/worker-redirect")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/dedicated.js")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/reports" && request.method == "POST")
            .count(),
        3
    );
}

fn import_worker_source(path: &str) -> String {
    format!(
        r#"
        globalThis.imported = false;
        globalThis.events = [];
        globalThis.reports = [];
        addEventListener('securitypolicyviolation', e => events.push({{
            target: e.target === self, directive: e.effectiveDirective,
            blockedURI: e.blockedURI, disposition: e.disposition,
            sourceFile: e.sourceFile, lineNumber: e.lineNumber
        }}));
        new ReportingObserver(batch => reports.push(...batch)).observe();
        globalThis.importOutcome = 'ok';
        try {{ importScripts({path:?}); }} catch (error) {{ importOutcome = error.name; }}
        "#
    )
}

fn run_import_worker(fixture: &WorkerCspFixture, owner_policy: Option<&str>) -> Value {
    let mut runtime = JsRuntime::with_document_and_url(
        crate::html::TreeBuilder::parse("<body></body>").document(),
        &format!("{}/page", fixture.origin),
    )
    .unwrap();
    if let Some(policy) = owner_policy {
        runtime.install_csp_policy(&[policy.to_owned()]);
    }
    runtime
        .eval("globalThis.worker=new Worker('/import-worker.js');")
        .unwrap();
    runtime.run_until_idle().unwrap();
    // The report sender belongs to the worker global, rather than its owner.
    let worker = runtime
        .host_state
        .borrow()
        .workers
        .values()
        .next()
        .unwrap()
        .clone();
    let mut worker = worker.borrow_mut();
    assert!(worker.startup_error.is_none(), "worker failed to start");
    worker.runtime.run_until_idle().unwrap();
    security_reporting::wait_for_report_delivery(&mut worker.runtime);
    worker.runtime.run_until_idle().unwrap();
    let observed = worker.runtime.eval("JSON.stringify({outcome:importOutcome,imported,events,reports:reports.map(r=>({blockedURL:r.body.blockedURL,directive:r.body.effectiveDirective,disposition:r.body.disposition}))})").unwrap();
    serde_json::from_str(&observed.as_string().unwrap().to_std_string_escaped()).unwrap()
}

#[test]
fn import_scripts_enforced_policy_blocks_before_http_and_reports_from_worker() {
    let source = import_worker_source("/import.js");
    let fixture = WorkerCspFixture::new(
        Some((
            "Content-Security-Policy",
            "script-src 'none'; report-to imports".into(),
        )),
        &source,
    );
    let observed = run_import_worker(&fixture, None);
    let worker_url = format!("{}/import-worker.js", fixture.origin);
    let import_url = format!("{}/import.js", fixture.origin);
    assert_eq!(observed["outcome"], "NetworkError");
    assert_eq!(observed["imported"], false);
    assert_eq!(observed["events"].as_array().unwrap().len(), 1);
    assert_eq!(observed["events"][0]["target"], true);
    assert_eq!(observed["events"][0]["directive"], "script-src-elem");
    assert_eq!(observed["events"][0]["blockedURI"], import_url);
    assert_eq!(observed["events"][0]["sourceFile"], worker_url);
    assert!(observed["events"][0]["lineNumber"].as_u64().unwrap() > 0);
    assert_eq!(observed["reports"].as_array().unwrap().len(), 1);
    let requests = fixture.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/import-worker.js")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/import.js")
            .count(),
        0
    );
    let reports: Vec<_> = requests
        .iter()
        .filter(|request| request.path == "/reports" && request.method == "POST")
        .collect();
    assert_eq!(reports.len(), 1);
    let payload: Value = serde_json::from_str(&reports[0].body).unwrap();
    assert_eq!(payload[0]["url"], worker_url);
    assert_eq!(payload[0]["body"]["documentURL"], worker_url);
    assert_eq!(payload[0]["body"]["blockedURL"], import_url);
}

#[test]
fn http_worker_imports_use_own_policy_instead_of_creators_script_src() {
    let source = import_worker_source("/import.js");
    let fixture = WorkerCspFixture::new(
        Some(("Content-Security-Policy", "script-src 'self'".into())),
        &source,
    );
    let observed = run_import_worker(&fixture, Some("worker-src 'self'; script-src 'none'"));
    assert_eq!(observed["outcome"], "ok");
    assert_eq!(observed["imported"], true);
    assert!(observed["events"].as_array().unwrap().is_empty());
    assert!(observed["reports"].as_array().unwrap().is_empty());
    let requests = fixture.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/import-worker.js")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/import.js")
            .count(),
        1
    );
}

#[test]
fn import_scripts_report_only_executes_and_reports_request_and_response() {
    let source = import_worker_source("/import.js");
    let fixture = WorkerCspFixture::new(
        Some((
            "Content-Security-Policy-Report-Only",
            "script-src 'none'; report-to imports".into(),
        )),
        &source,
    );
    let observed = run_import_worker(&fixture, None);
    assert_eq!(observed["outcome"], "ok");
    assert_eq!(observed["imported"], true);
    assert_eq!(observed["events"].as_array().unwrap().len(), 2);
    assert!(
        observed["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["disposition"] == "report")
    );
    assert_eq!(observed["reports"].as_array().unwrap().len(), 2);
    let requests = fixture.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/import.js")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/reports" && request.method == "POST")
            .count(),
        2
    );
}

#[test]
fn data_import_scripts_report_only_reports_request_and_response_once_each() {
    let source = import_worker_source("data:text/javascript,globalThis.imported%3Dtrue");
    let fixture = WorkerCspFixture::new(
        Some((
            "Content-Security-Policy-Report-Only",
            "script-src 'none'; report-to imports".into(),
        )),
        &source,
    );
    let observed = run_import_worker(&fixture, None);
    assert_eq!(observed["outcome"], "ok");
    assert_eq!(observed["imported"], true);
    assert_eq!(observed["events"].as_array().unwrap().len(), 2);
    assert_eq!(observed["reports"].as_array().unwrap().len(), 2);
    assert!(
        observed["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["disposition"] == "report")
    );
    let requests = fixture.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/reports" && request.method == "POST")
            .count(),
        2
    );
}

#[test]
fn import_scripts_redirect_checks_target_before_http_without_exposing_it() {
    let source = import_worker_source("/redirect");
    let fixture = WorkerCspFixture::new(
        Some((
            "Content-Security-Policy",
            "script-src {ORIGIN}/redirect; report-to imports".into(),
        )),
        &source,
    );
    let observed = run_import_worker(&fixture, None);
    let initial = format!("{}/redirect", fixture.origin);
    assert_eq!(observed["outcome"], "NetworkError");
    assert_eq!(observed["imported"], false);
    assert_eq!(observed["events"].as_array().unwrap().len(), 1);
    assert_eq!(observed["events"][0]["blockedURI"], initial);
    assert_eq!(observed["reports"][0]["blockedURL"], initial);
    let requests = fixture.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/redirect")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/import.js")
            .count(),
        0
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/reports" && request.method == "POST")
            .count(),
        1
    );
}

#[test]
fn import_scripts_allowed_redirect_ignores_source_path_after_first_hop() {
    let source = import_worker_source("/import-redirect");
    let fixture = WorkerCspFixture::new(
        Some((
            "Content-Security-Policy",
            "script-src {ORIGIN}/import-redirect".into(),
        )),
        &source,
    );
    let observed = run_import_worker(&fixture, None);
    assert_eq!(observed["outcome"], "ok");
    assert_eq!(observed["imported"], true);
    assert!(observed["events"].as_array().unwrap().is_empty());
    let requests = fixture.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/import-redirect")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/import.js")
            .count(),
        1
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
fn shared_worker_reuse_checks_each_new_owner_before_connecting() {
    let fixture =
        WorkerOwnerFixture::with_policy(false, "worker-src 'none'; report-to worker-tests");
    let (mut runtime, child_id) = runtime_with_worker_owner(&fixture);
    runtime.eval("globalThis.first=new SharedWorker('/child/worker.js','csp-reuse');first.port.onmessage=e=>received.push(e.data);first.port.start();").unwrap();
    assert_eq!(
        shared_observation(&mut runtime),
        json!(format!("{}/child/worker.js", fixture.origin))
    );
    runtime.eval_in_document_realm(child_id, "globalThis.denied=new SharedWorker('/child/worker.js','csp-reuse');denied.onerror=()=>parent.postMessage('blocked','*');denied.port.onmessage=()=>parent.postMessage('connected','*');denied.port.start();").unwrap();
    assert_eq!(shared_observation(&mut runtime), json!("blocked"));
    let expected_url = format!("{}/child/worker.js", fixture.origin);
    assert_eq!(
        runtime
            .eval(&format!(
                "parentEvents.length===0&&parentReports.length===0&&document.querySelector('iframe').contentWindow.ownerEvents.length===1&&document.querySelector('iframe').contentWindow.ownerEvents[0].effectiveDirective==='worker-src'&&document.querySelector('iframe').contentWindow.ownerEvents[0].blockedURI==={}&&document.querySelector('iframe').contentWindow.ownerReports.length===1&&document.querySelector('iframe').contentWindow.ownerReports[0].body.blockedURL==={}",
                json!(expected_url),
                json!(expected_url),
            ))
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    drop(runtime);
    assert_eq!(fixture.finish(), vec!["/child/page", "/child/worker.js"]);
}

#[test]
fn denied_shared_worker_does_not_poison_an_allowed_owner_before_pruning() {
    let fixture = WorkerOwnerFixture::new(false);
    let (mut runtime, child_id) = runtime_with_worker_owner(&fixture);
    runtime.install_csp_policy(&["worker-src 'none'".into()]);
    runtime.eval("globalThis.denied=new SharedWorker('/child/worker.js','csp-retry');denied.onerror=()=>received.push('blocked');").unwrap();
    runtime.eval_in_document_realm(child_id, "globalThis.allowed=new SharedWorker('/child/worker.js','csp-retry');allowed.port.onmessage=e=>parent.postMessage(e.data,'*');allowed.port.start();").unwrap();
    runtime.run_until_idle().unwrap();
    let worker_url = format!("{}/child/worker.js", fixture.origin);
    assert_eq!(
        runtime
            .eval(&format!(
                "received.includes('blocked')&&received.includes({})&&parentEvents.length===1&&parentEvents[0].effectiveDirective==='worker-src'&&document.querySelector('iframe').contentWindow.ownerEvents.length===0",
                json!(worker_url),
            ))
            .unwrap()
            .as_boolean(),
        Some(true)
    );
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
