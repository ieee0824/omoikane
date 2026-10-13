//! Response CSP and reporting metadata belong to each Worker global.
use std::io::{self, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use omoikane::{html::TreeBuilder, js::JsRuntime};
use serde_json::{Value, json};

#[path = "support/http_fixture.rs"]
mod http_fixture;
use http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};

const WORKER_SOURCE: &str = r#"
    const events = [], reports = [];
    let outcome = null;
    let connectedPort = null;
    function publish() {
        const result = {url: location.href, outcome, events, reports};
        if (connectedPort) connectedPort.postMessage(result);
        else postMessage(result);
    }
    self.addEventListener('securitypolicyviolation', event => {
        events.push({
            targetsSelf: event.target === self,
            currentTargetIsSelf: event.currentTarget === self,
            trusted: event.isTrusted, statusCode: event.statusCode,
            documentURI: event.documentURI, blockedURI: event.blockedURI,
            disposition: event.disposition, effectiveDirective: event.effectiveDirective
        });
        publish();
    });
    new ReportingObserver(batch => {
        reports.push(...batch.map(report => ({
            type: report.type, url: report.url,
            statusCode: report.body.statusCode,
            documentURL: report.body.documentURL, blockedURL: report.body.blockedURL,
            disposition: report.body.disposition,
            effectiveDirective: report.body.effectiveDirective
        })));
        publish();
    }, {types: ['csp-violation']}).observe();
    function fetchResource() {
        fetch('resource').then(response => {
            outcome = {success: response.ok, status: response.status};
            publish();
        }, error => {
            outcome = {success: false, name: error.name, isTypeError: error instanceof TypeError};
            publish();
        });
    }
    self.onmessage = event => {
        if (event.data !== 'import') {
            fetchResource();
            return;
        }
        import('./module.js').then(module => {
            postMessage({imported: module.answer, url: location.href});
        }, error => {
            postMessage({importError: String(error)});
        });
    };
    self.onconnect = event => {
        connectedPort = event.ports[0];
        connectedPort.onmessage = fetchResource;
        connectedPort.start();
    };
"#;

enum WorkerKind {
    Dedicated,
    Shared,
}

struct WorkerHttpFixture {
    origin: String,
    stopped: Arc<AtomicBool>,
    worker: Option<FixtureWorker<Vec<String>>>,
}

impl WorkerHttpFixture {
    fn new(report_only: bool) -> Self {
        let listener = bind_loopback().unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let worker = FixtureWorker::spawn(move || {
            let deadline = Instant::now() + ACCEPT_TIMEOUT;
            let mut paths = Vec::new();
            while !worker_stopped.load(Ordering::Acquire) {
                assert!(Instant::now() < deadline, "worker HTTP fixture timed out");
                let mut stream = match accept_with_timeout(&listener, Duration::from_millis(20)) {
                    Ok(stream) => stream,
                    Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
                    Err(error) => panic!("worker fixture accept failed: {error}"),
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
                    "/redirect-worker.js" => stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /final/worker.js\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                    "/final/worker.js" => {
                        let header = if report_only {
                            "Content-Security-Policy-Report-Only"
                        } else {
                            "Content-Security-Policy"
                        };
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\n{header}: connect-src 'none'; report-to workers\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{WORKER_SOURCE}", WORKER_SOURCE.len()).unwrap();
                    }
                    // Remains reachable in both cases: enforcement must reject
                    // before transport, rather than relying on a closed server.
                    "/final/resource" => stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                    "/final/module.js" => {
                        let body = "export const answer = 42;";
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    _ => panic!("unexpected worker request: {path}"),
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

impl Drop for WorkerHttpFixture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

fn observe_worker(runtime: &mut JsRuntime, kind: WorkerKind) -> Value {
    runtime.eval(r#"
        globalThis.parentEvents = [], globalThis.parentReports = [];
        globalThis.workerErrors = [], globalThis.workerResult = null;
        document.addEventListener('securitypolicyviolation', event => parentEvents.push(event.type));
        new ReportingObserver(batch => parentReports.push(...batch)).observe();
    "#).unwrap();
    let create = match kind {
        WorkerKind::Dedicated => {
            r#"
            globalThis.worker = new Worker('/redirect-worker.js');
            globalThis.endpoint = worker;
        "#
        }
        WorkerKind::Shared => {
            r#"
            globalThis.worker = new SharedWorker('/redirect-worker.js');
            globalThis.endpoint = worker.port;
            endpoint.start();
        "#
        }
    };
    runtime.eval(create).unwrap();
    runtime
        .eval(
            r#"
        worker.onerror = event => { workerErrors.push(event.message); event.preventDefault(); };
        endpoint.onmessage = event => { workerResult = event.data; };
        endpoint.postMessage('start');
    "#,
        )
        .unwrap();
    runtime.run_timers(50, 1, 200);
    runtime.run_until_idle().unwrap();
    let serialized = runtime
        .eval("JSON.stringify({workerResult, workerErrors, parentEvents, parentReports})")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    serde_json::from_str(&serialized).unwrap()
}

fn assert_worker_response_policy(report_only: bool, kind: WorkerKind) {
    let fixture = WorkerHttpFixture::new(report_only);
    let worker_url = format!("{}/final/worker.js", fixture.origin);
    let resource_url = format!("{}/final/resource", fixture.origin);
    // This synthetic owner has response status 0. The worker must retain its
    // own final 200 response, independently of the 302 and fetched 204 statuses.
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<!doctype html><body>").document(),
        &format!("{}/owner", fixture.origin),
    )
    .unwrap();
    let observed = observe_worker(&mut runtime, kind);
    let paths = fixture.finish();
    let expected_paths = if report_only {
        vec!["/redirect-worker.js", "/final/worker.js", "/final/resource"]
    } else {
        vec!["/redirect-worker.js", "/final/worker.js"]
    };
    assert_eq!(paths, expected_paths, "{observed}");
    assert_eq!(observed["workerErrors"], json!([]));
    assert_eq!(observed["parentEvents"], json!([]));
    assert_eq!(observed["parentReports"], json!([]));
    let result = &observed["workerResult"];
    assert_eq!(result["url"], worker_url, "{observed}");
    let expected_outcome = if report_only {
        json!({"success": true, "status": 204})
    } else {
        json!({"success": false, "name": "TypeError", "isTypeError": true})
    };
    assert_eq!(result["outcome"], expected_outcome, "{observed}");
    let disposition = if report_only { "report" } else { "enforce" };
    let count = if report_only { 2 } else { 1 };
    let event = json!({
        "targetsSelf": true, "currentTargetIsSelf": true, "trusted": true,
        "statusCode": 200, "documentURI": worker_url,
        "blockedURI": resource_url, "disposition": disposition,
        "effectiveDirective": "connect-src"
    });
    let report = json!({
        "type": "csp-violation", "url": worker_url, "statusCode": 200,
        "documentURL": worker_url, "blockedURL": resource_url,
        "disposition": disposition, "effectiveDirective": "connect-src"
    });
    assert_eq!(result["events"], json!(vec![event; count]), "{observed}");
    assert_eq!(result["reports"], json!(vec![report; count]), "{observed}");
}

#[test]
fn dedicated_worker_response_csp_blocks_fetch_and_reports_to_its_own_global() {
    assert_worker_response_policy(false, WorkerKind::Dedicated);
}

#[test]
fn dedicated_worker_report_only_checks_preserve_fetch_and_final_response_metadata() {
    assert_worker_response_policy(true, WorkerKind::Dedicated);
}

#[test]
fn shared_worker_response_csp_blocks_fetch_and_reports_to_its_own_global() {
    assert_worker_response_policy(false, WorkerKind::Shared);
}

#[test]
fn redirected_worker_resolves_relative_dynamic_import_from_its_final_script_url() {
    let fixture = WorkerHttpFixture::new(false);
    let worker_url = format!("{}/final/worker.js", fixture.origin);
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<!doctype html><body>").document(),
        &format!("{}/owner", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
        globalThis.workerResult = null, globalThis.workerErrors = [];
        globalThis.worker = new Worker('/redirect-worker.js');
        worker.onerror = event => { workerErrors.push(event.message); event.preventDefault(); };
        worker.onmessage = event => { workerResult = event.data; };
        worker.postMessage('import');
    "#,
        )
        .unwrap();
    runtime.run_timers(50, 1, 200);
    runtime.run_until_idle().unwrap();
    let observed: Value = serde_json::from_str(
        &runtime
            .eval("JSON.stringify({workerResult, workerErrors})")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
    )
    .unwrap();
    assert_eq!(
        fixture.finish(),
        [
            "/redirect-worker.js",
            "/final/worker.js",
            "/final/module.js"
        ],
        "{observed}"
    );
    assert_eq!(
        observed,
        json!({"workerResult": {"imported": 42, "url": worker_url}, "workerErrors": []})
    );
}
