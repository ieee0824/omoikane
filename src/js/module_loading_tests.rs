use super::{JsRuntime, module_fetch::ModuleFetchPool};
use crate::html::TreeBuilder;
use crate::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use crate::test_support::http_fixture::{
    READ_TIMEOUT, accept_with_timeout, bind_loopback, read_request_headers,
};
use std::collections::HashMap;
use std::io::Write;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

struct Route {
    status: u16,
    body: String,
    headers: String,
    gate: bool,
}

const CSP_REDIRECT_SOURCE: &str = "https://source.example.test";
const CSP_REDIRECT_TARGET: &str = "https://target.example.test";

struct CspRedirectFixture {
    target_url: String,
    source: String,
    requests: Mutex<Vec<(String, bool, Option<String>)>>,
}

impl CspRedirectFixture {
    fn new(path: &str, source: &str) -> Arc<Self> {
        Arc::new(Self {
            target_url: format!("{CSP_REDIRECT_TARGET}{path}"),
            source: source.to_owned(),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn install(self: &Arc<Self>, runtime: &mut JsRuntime) {
        let cookies = Arc::clone(&runtime.host_state.borrow().cookie_store);
        *runtime.module_loader.fetch_pool.borrow_mut() =
            Some(ModuleFetchPool::new_for_test(cookies, self.clone()).unwrap());
    }

    fn assert_redirect_requests(&self) {
        assert_eq!(
            self.requests.lock().unwrap().as_slice(),
            &[
                (format!("{CSP_REDIRECT_SOURCE}/redirect.js"), false, None),
                (
                    self.target_url.clone(),
                    true,
                    Some(CSP_REDIRECT_SOURCE.into())
                ),
            ]
        );
    }
}

impl HttpTransport for CspRedirectFixture {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        self.requests.lock().unwrap().push((
            request.url().to_string(),
            request.requires_public_ip(),
            request.header("Origin").map(str::to_owned),
        ));
        if request.url().to_string() == format!("{CSP_REDIRECT_SOURCE}/redirect.js") {
            return Ok(HttpResponse::new(
                302,
                "Found",
                vec![("Location".into(), self.target_url.clone())],
                Vec::new(),
            ));
        }
        assert_eq!(request.url().to_string(), self.target_url);
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![
                ("Content-Type".into(), "text/javascript".into()),
                (
                    "Access-Control-Allow-Origin".into(),
                    CSP_REDIRECT_SOURCE.into(),
                ),
            ],
            self.source.as_bytes().to_vec(),
        ))
    }
}

struct ModuleServer {
    origin: String,
    requests: Arc<Mutex<Vec<(String, String)>>>,
    peak: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

fn read_module_request(stream: &mut TcpStream) -> std::io::Result<String> {
    // Accepted sockets can inherit the listener's nonblocking flag on BSD.
    // The accept loop polls for shutdown, but each handler waits for a request.
    stream.set_nonblocking(false)?;
    read_request_headers(stream, READ_TIMEOUT)
}

fn join_module_workers(workers: impl IntoIterator<Item = thread::JoinHandle<()>>) {
    let mut failure = None;
    for worker in workers {
        if let Err(error) = worker.join() {
            failure.get_or_insert(error);
        }
    }
    if let Some(error) = failure {
        if thread::panicking() {
            // Preserve the original failed assertion instead of aborting the
            // entire test process with a second panic from Drop.
            eprintln!("module test server also failed during test cleanup");
        } else {
            std::panic::resume_unwind(error);
        }
    }
}

impl ModuleServer {
    fn new(routes: HashMap<String, Route>) -> Self {
        let listener = bind_loopback().unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let peak = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let routes = Arc::new(routes);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_requests = requests.clone();
        let worker_peak = peak.clone();
        let worker_stop = stop.clone();
        let worker = thread::spawn(move || {
            let mut handlers = Vec::new();
            while !worker_stop.load(Ordering::Relaxed) {
                let mut stream = match accept_with_timeout(&listener, Duration::from_millis(20)) {
                    Ok(stream) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
                    Err(error) => panic!("accept: {error}"),
                };
                let routes = routes.clone();
                let requests = worker_requests.clone();
                let active = active.clone();
                let peak = worker_peak.clone();
                let gate = gate.clone();
                handlers.push(thread::spawn(move || {
                    let request = read_module_request(&mut stream).unwrap();
                    let path = request.split_whitespace().nth(1).unwrap().to_owned();
                    requests.lock().unwrap().push((path.clone(), request));
                    let route = routes.get(&path).unwrap_or_else(|| panic!("unexpected request: {path}"));
                    let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(count, Ordering::SeqCst);
                    if route.gate {
                        // The first response waits for a second live request.
                        // A serial loader falls back after the deadline, then
                        // fails the overlap assertion rather than hanging.
                        let (ready, wake) = &*gate;
                        let mut ready = ready.lock().unwrap();
                        if count >= 2 {
                            *ready = true;
                            wake.notify_all();
                        }
                        let (mut ready, _) = wake.wait_timeout_while(ready, Duration::from_secs(2), |ready| !*ready).unwrap();
                        *ready = true;
                        wake.notify_all();
                    }
                    active.fetch_sub(1, Ordering::SeqCst);
                    let mime = if path.ends_with(".html") { "text/html" } else if path.ends_with(".events") { "text/event-stream" } else { "text/javascript" };
                    write!(stream, "HTTP/1.1 {} Result\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}", route.status, route.body.len(), route.headers, route.body).unwrap();
                }));
            }
            join_module_workers(handlers);
        });
        Self {
            origin,
            requests,
            peak,
            stop,
            worker: Some(worker),
        }
    }

    fn paths(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|(path, _)| path.clone())
            .collect()
    }
}

impl Drop for ModuleServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        join_module_workers(self.worker.take());
    }
}

#[path = "module_server_tests.rs"]
mod server_tests;

#[test]
fn imports_overlap_share_identity_and_cookies_and_keep_dynamic_imports_lazy() {
    let mut routes = HashMap::new();
    let mut entry = String::new();
    for index in 0..8 {
        routes.insert(format!("/dep{index}.js"), Route {
            status: 200,
            body: format!("import {{ token }} from './shared.js'; export {{ token }}; order.push({index});"),
            headers: format!("Set-Cookie: dep{index}=yes; Path=/\r\n"),
            gate: true,
        });
        entry.push_str(&format!(
            "import {{ token as t{index} }} from './dep{index}.js';\n"
        ));
    }
    routes.insert("/shared.js".to_string(), Route {
        status: 200,
        body: "globalThis.sharedExecutions = (globalThis.sharedExecutions || 0) + 1; export const token = {};".to_string(),
        headers: String::new(), gate: false,
    });
    routes.insert(
        "/dormant.js".to_string(),
        Route {
            status: 200,
            body: "export { token } from './shared.js';".to_string(),
            headers: String::new(),
            gate: false,
        },
    );
    entry.push_str("globalThis.sameTokens = [t1,t2,t3,t4,t5,t6,t7].every(token => token === t0); globalThis.loadLater = () => import('./dormant.js').then(module => globalThis.sameLaterToken = module.token === t0);");
    let server = ModuleServer::new(routes);
    let url = format!("{}/index.html", server.origin);
    let document = TreeBuilder::parse(&format!("<html><body><script>globalThis.order = [];</script><script type='module'>{entry}</script></body></html>")).document();
    let mut runtime = JsRuntime::with_document_and_url(document, &url).unwrap();
    let errors = runtime.execute_document_scripts(Some(&url.parse().unwrap()));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        runtime
            .eval("sameTokens && sharedExecutions === 1 && order.join(',') === '0,1,2,3,4,5,6,7'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    assert!(!server.paths().contains(&"/dormant.js".to_string()));
    assert_eq!(
        server
            .paths()
            .iter()
            .filter(|path| *path == "/shared.js")
            .count(),
        1
    );
    runtime.eval("loadLater()").unwrap();
    runtime.run_jobs().unwrap();
    assert_eq!(
        runtime
            .eval("sameLaterToken && sharedExecutions === 1")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    let requests = server.requests.lock().unwrap();
    let dormant = &requests
        .iter()
        .find(|(path, _)| path == "/dormant.js")
        .unwrap()
        .1;
    for index in 0..8 {
        assert!(dormant.contains(&format!("dep{index}=yes")), "{dormant}");
    }
    let peak = server.peak.load(Ordering::SeqCst);
    assert!(peak >= 2, "module requests never overlapped (peak={peak})");
    assert!(
        peak <= 4,
        "module requests exceeded the concurrency bound (peak={peak})"
    );
}

fn route(body: &str) -> Route {
    Route {
        status: 200,
        body: body.to_owned(),
        headers: String::new(),
        gate: false,
    }
}

#[test]
fn dedicated_worker_module_imports_share_owner_cookie_jar() {
    let server = ModuleServer::new(HashMap::from([
        (
            "/worker.js".into(),
            route(
                "globalThis.load = () => import('./dep.js').then(() => globalThis.loaded = true)",
            ),
        ),
        (
            "/dep.js".into(),
            Route {
                headers: "Set-Cookie: module=stored; Path=/; SameSite=Lax\r\n".into(),
                ..route("export const loaded = true")
            },
        ),
    ]));
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, &format!("{}/index.html", server.origin))
            .unwrap();
    let cookie_url: crate::http::Url = format!("{}/", server.origin).parse().unwrap();
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url("session=owner; Path=/; SameSite=Lax", &cookie_url);
    runtime
        .eval(&format!(
            "new Worker({})",
            serde_json::to_string(&format!("{}/worker.js", server.origin)).unwrap()
        ))
        .unwrap();
    let worker = runtime
        .host_state
        .borrow()
        .workers
        .values()
        .next()
        .cloned()
        .unwrap();
    {
        let mut worker = worker.borrow_mut();
        worker.runtime.eval("load()").unwrap();
        worker.runtime.run_until_idle().unwrap();
        assert_eq!(
            worker.runtime.eval("loaded").unwrap().as_boolean(),
            Some(true)
        );
    }
    let requests = server.requests.lock().unwrap();
    let module_request = &requests
        .iter()
        .find(|(path, _)| path == "/dep.js")
        .expect("worker module was fetched")
        .1;
    assert_eq!(
        module_request
            .lines()
            .find_map(|line| line.strip_prefix("Cookie: ")),
        Some("session=owner"),
        "worker module request must use the owner's cookie jar: {requests:?}"
    );
    drop(requests);
    let cookies = runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .document_cookie(&cookie_url, &cookie_url);
    assert!(cookies.contains("module=stored"));
}

fn runtime_with_module(origin: &str, source: &str) -> JsRuntime {
    runtime_with_policy(origin, source, "")
}

fn runtime_with_policy(origin: &str, source: &str, policy: &str) -> JsRuntime {
    let document = TreeBuilder::parse(&format!(
        "<html><head><meta http-equiv='Content-Security-Policy' content=\"{policy}\"></head><body><script type='module'>{source}</script></body></html>"
    ))
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, &format!("{origin}/index.html")).unwrap();
    runtime.install_csp_policy(&[]);
    runtime
}

fn execute(runtime: &mut JsRuntime, origin: &str) -> Vec<String> {
    runtime.execute_document_scripts(Some(&format!("{origin}/index.html").parse().unwrap()))
}

fn observe_report_only_modules(runtime: &mut JsRuntime, policy: &str) {
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy-Report-Only".into(),
        policy.into(),
    )]);
    runtime
        .eval(
            "globalThis.moduleReports = []; new ReportingObserver(batch => moduleReports.push(...batch), {types: ['csp-violation']}).observe();",
        )
        .unwrap();
}

fn module_report_snapshot(runtime: &mut JsRuntime) -> serde_json::Value {
    let serialized = runtime
        .eval("JSON.stringify([dependencyValue, document.cspViolations.length, moduleReports.map(report => [report.type, report.body.disposition, report.body.blockedURL])])")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    serde_json::from_str(&serialized).unwrap()
}

#[test]
fn report_only_module_dependency_runs_and_reports_request_and_response() {
    let server = ModuleServer::new(HashMap::from([(
        "/dependency.js".into(),
        route("export const value = 42;"),
    )]));
    let mut runtime = runtime_with_module(
        &server.origin,
        "import { value } from './dependency.js'; globalThis.dependencyValue = value;",
    );
    observe_report_only_modules(
        &mut runtime,
        "script-src 'unsafe-inline'; report-to modules",
    );

    let errors = execute(&mut runtime, &server.origin);
    assert!(errors.is_empty(), "{errors:?}");
    runtime.run_until_idle().unwrap();

    let dependency_url = format!("{}/dependency.js", server.origin);
    assert_eq!(
        module_report_snapshot(&mut runtime),
        serde_json::json!([
            42,
            0,
            [
                ["csp-violation", "report", dependency_url],
                ["csp-violation", "report", dependency_url]
            ]
        ])
    );
    assert_eq!(server.paths(), ["/dependency.js"]);
}

#[test]
fn report_only_module_redirect_reports_request_and_response_without_blocking_execution() {
    let transport = CspRedirectFixture::new("/dependency.js", "export const value = 73;");
    let mut runtime = runtime_with_module(
        CSP_REDIRECT_SOURCE,
        "import { value } from './redirect.js'; globalThis.dependencyValue = value;",
    );
    transport.install(&mut runtime);
    let policy =
        format!("script-src 'unsafe-inline' {CSP_REDIRECT_SOURCE}/redirect.js; report-to modules");
    observe_report_only_modules(&mut runtime, &policy);

    let errors = execute(&mut runtime, CSP_REDIRECT_SOURCE);
    assert!(errors.is_empty(), "{errors:?}");
    runtime.run_until_idle().unwrap();

    // CSP reports the original request URL, so redirects cannot expose a
    // destination path that the requesting document was not allowed to know.
    // https://w3c.github.io/webappsec-csp/#create-violation-for-request
    assert_eq!(
        module_report_snapshot(&mut runtime),
        serde_json::json!([
            73,
            0,
            [
                [
                    "csp-violation",
                    "report",
                    format!("{CSP_REDIRECT_SOURCE}/redirect.js")
                ],
                [
                    "csp-violation",
                    "report",
                    format!("{CSP_REDIRECT_SOURCE}/redirect.js")
                ],
            ]
        ])
    );
    transport.assert_redirect_requests();
}

#[test]
fn report_only_module_redirect_ignores_source_path_on_allowed_origin() {
    let server = ModuleServer::new(HashMap::from([
        (
            "/allowed.js".into(),
            Route {
                status: 302,
                headers: "Location: /other.js\r\n".into(),
                ..route("")
            },
        ),
        ("/other.js".into(), route("export const value = 19;")),
    ]));
    let mut runtime = runtime_with_module(
        &server.origin,
        "import { value } from './allowed.js'; globalThis.dependencyValue = value;",
    );
    let policy = format!(
        "script-src 'unsafe-inline' {}/allowed.js; report-to modules",
        server.origin
    );
    observe_report_only_modules(&mut runtime, &policy);

    let errors = execute(&mut runtime, &server.origin);
    assert!(errors.is_empty(), "{errors:?}");
    runtime.run_until_idle().unwrap();

    assert_eq!(
        module_report_snapshot(&mut runtime),
        serde_json::json!([19, 0, []])
    );
    assert_eq!(server.paths(), ["/allowed.js", "/other.js"]);
}

#[test]
fn csp_blocks_dependency_and_redirect_target_before_fetch() {
    let transport = CspRedirectFixture::new("/blocked.js", "globalThis.blockedRan = true;");
    let policy = format!("script-src 'unsafe-inline' {CSP_REDIRECT_SOURCE}");
    let source = format!("import '{CSP_REDIRECT_TARGET}/blocked.js';");
    let mut runtime = runtime_with_policy(CSP_REDIRECT_SOURCE, &source, &policy);
    transport.install(&mut runtime);
    let errors = execute(&mut runtime, CSP_REDIRECT_SOURCE);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("CSP blocked module import")),
        "{errors:?}"
    );
    assert!(transport.requests.lock().unwrap().is_empty());

    let mut runtime = runtime_with_policy(CSP_REDIRECT_SOURCE, "import './redirect.js';", &policy);
    transport.install(&mut runtime);
    let errors = execute(&mut runtime, CSP_REDIRECT_SOURCE);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("CSP blocked module redirect")),
        "{errors:?}"
    );
    assert_eq!(
        runtime
            .eval("typeof blockedRan === 'undefined' && document.cspViolations.length === 1")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    assert_eq!(
        *transport.requests.lock().unwrap(),
        vec![(format!("{CSP_REDIRECT_SOURCE}/redirect.js"), false, None)]
    );
}

#[test]
fn strict_dynamic_allows_module_graph_import_and_redirect_with_report_only_events() {
    let server = ModuleServer::new(HashMap::from([
        (
            "/redirect.js".into(),
            Route {
                status: 302,
                headers: "Location: /dependency.js\r\n".into(),
                ..route("")
            },
        ),
        ("/dependency.js".into(), route("export const value = 42;")),
    ]));
    let source = "import { value } from './redirect.js'; globalThis.dependencyValue = value;";
    let document = TreeBuilder::parse(&format!(
        "<body><script type=module nonce=ok>{source}</script>"
    ))
    .document();
    let url = format!("{}/index.html", server.origin);
    let mut runtime = JsRuntime::with_document_and_url(document, &url).unwrap();
    runtime.install_response_security_headers(&[
        (
            "Content-Security-Policy".into(),
            "script-src-elem 'strict-dynamic' 'nonce-ok'".into(),
        ),
        (
            "Content-Security-Policy-Report-Only".into(),
            "script-src-elem https:; report-to modules".into(),
        ),
    ]);
    runtime
        .eval("globalThis.moduleReports = []; new ReportingObserver(batch => moduleReports.push(...batch), {types: ['csp-violation']}).observe();")
        .unwrap();

    let errors = execute(&mut runtime, &server.origin);
    assert!(errors.is_empty(), "{errors:?}");
    runtime.run_until_idle().unwrap();
    assert_eq!(
        module_report_snapshot(&mut runtime),
        serde_json::json!([
            42,
            0,
            [
                ["csp-violation", "report", "inline"],
                [
                    "csp-violation",
                    "report",
                    format!("{}/redirect.js", server.origin)
                ],
                [
                    "csp-violation",
                    "report",
                    format!("{}/redirect.js", server.origin)
                ],
                [
                    "csp-violation",
                    "report",
                    format!("{}/redirect.js", server.origin)
                ]
            ]
        ])
    );
    assert_eq!(server.paths(), ["/redirect.js", "/dependency.js"]);
}

#[test]
fn second_enforced_policy_can_block_strict_dynamic_module_import() {
    let server = ModuleServer::new(HashMap::from([(
        "/dependency.js".into(),
        route("export const value = 42;"),
    )]));
    let document =
        TreeBuilder::parse("<body><script type=module nonce=ok>import './dependency.js';</script>")
            .document();
    let url = format!("{}/index.html", server.origin);
    let mut runtime = JsRuntime::with_document_and_url(document, &url).unwrap();
    runtime.install_csp_policy(&[
        "script-src-elem 'strict-dynamic' 'nonce-ok'".into(),
        "script-src-elem 'nonce-ok' https:".into(),
    ]);
    let errors = execute(&mut runtime, &server.origin);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("CSP blocked module import")),
        "{errors:?}"
    );
    assert_eq!(server.paths(), Vec::<String>::new());
    assert_eq!(
        runtime
            .eval("document.cspViolations.length")
            .unwrap()
            .as_number(),
        Some(1.0)
    );
}

#[test]
fn cross_origin_import_cannot_connect_to_private_address() {
    let server = ModuleServer::new(HashMap::new());
    let mut runtime = runtime_with_module(
        "https://example.test",
        &format!("import '{}/private.js';", server.origin),
    );
    let errors = execute(&mut runtime, "https://example.test");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("private") || errors[0].contains("public"),
        "{errors:?}"
    );
    assert!(server.paths().is_empty());
}

#[test]
fn cross_origin_static_and_dynamic_imports_require_cors_without_coep() {
    const OWNER: &str = "https://owner.example.test";
    const MODULE: &str = "https://modules.example.test/dependency.js";

    struct ModuleTransport {
        allow: bool,
        requests: Arc<Mutex<Vec<(String, Option<String>, bool)>>>,
    }

    impl HttpTransport for ModuleTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            self.requests.lock().unwrap().push((
                request.url().to_string(),
                request.header("Origin").map(str::to_owned),
                request.requires_public_ip(),
            ));
            let mut headers = vec![("Content-Type".into(), "text/javascript".into())];
            if self.allow {
                headers.push(("Access-Control-Allow-Origin".into(), OWNER.into()));
            }
            Ok(HttpResponse::new(
                200,
                "OK",
                headers,
                b"globalThis.dependencyRan=true; export const value=42;".to_vec(),
            ))
        }
    }

    for kind in ["static", "dynamic"] {
        for allow in [false, true] {
            let source = if kind == "static" {
                format!("import {{ value }} from '{MODULE}'; globalThis.moduleValue=value;")
            } else {
                format!(
                    "globalThis.load=()=>import('{MODULE}').then(module=>{{globalThis.moduleValue=module.value}},error=>{{globalThis.importError=String(error)}});"
                )
            };
            let mut runtime = runtime_with_module(OWNER, &source);
            let requests = Arc::new(Mutex::new(Vec::new()));
            let transport = Arc::new(ModuleTransport {
                allow,
                requests: Arc::clone(&requests),
            });
            let cookies = Arc::clone(&runtime.host_state.borrow().cookie_store);
            *runtime.module_loader.fetch_pool.borrow_mut() =
                Some(ModuleFetchPool::new_for_test(cookies, transport).unwrap());

            let errors = execute(&mut runtime, OWNER);
            if kind == "dynamic" {
                assert!(errors.is_empty(), "{kind} {allow}: {errors:?}");
                runtime.eval("load()").unwrap();
                runtime.run_jobs().unwrap();
            } else {
                assert_eq!(errors.is_empty(), allow, "{kind} {allow}: {errors:?}");
            }
            assert_eq!(
                runtime
                    .eval("typeof moduleValue !== 'undefined' && moduleValue===42 && dependencyRan===true")
                    .unwrap()
                    .to_boolean(),
                allow,
                "{kind} {allow}"
            );
            assert_eq!(
                requests.lock().unwrap().as_slice(),
                &[(MODULE.to_owned(), Some(OWNER.to_owned()), true)],
                "{kind} {allow}"
            );
        }
    }
}

#[test]
fn external_module_use_credentials_applies_to_root_and_imports() {
    const OWNER: &str = "https://owner.example.test";
    const MODULE_ORIGIN: &str = "https://modules.example.test";

    struct CredentialTransport(Arc<Mutex<Vec<(String, Option<String>, Option<String>)>>>);

    impl HttpTransport for CredentialTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            self.0.lock().unwrap().push((
                request.url().to_string(),
                request.header("Origin").map(str::to_owned),
                request.header("Cookie").map(str::to_owned),
            ));
            let source = match request.url().path() {
                "/root.js" => {
                    "import { value } from './dependency.js'; globalThis.moduleValue=value; globalThis.loadLater=()=>import('./later.js').then(module=>globalThis.laterValue=module.later);"
                }
                "/dependency.js" => "export const value=42;",
                "/later.js" => "export const later=73;",
                path => panic!("unexpected module request: {path}"),
            };
            Ok(HttpResponse::new(
                200,
                "OK",
                vec![
                    ("Content-Type".into(), "text/javascript".into()),
                    ("Access-Control-Allow-Origin".into(), OWNER.into()),
                    ("Access-Control-Allow-Credentials".into(), "true".into()),
                ],
                source.as_bytes().to_vec(),
            ))
        }
    }

    for (crossorigin, expect_cookie) in [("use-credentials", true), ("anonymous", false)] {
        let document = TreeBuilder::parse(&format!(
            "<html><body><script type='module' src='{MODULE_ORIGIN}/root.js' crossorigin='{crossorigin}'></script></body></html>"
        ))
        .document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, &format!("{OWNER}/index.html")).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let transport = Arc::new(CredentialTransport(Arc::clone(&requests)));
        runtime.set_http_resource_transport(Some(transport.clone()));
        let cookies = Arc::clone(&runtime.host_state.borrow().cookie_store);
        cookies.lock().unwrap().add_from_header_for_url(
            "module=secret; Path=/; Secure; SameSite=None",
            &format!("{MODULE_ORIGIN}/root.js").parse().unwrap(),
        );
        *runtime.module_loader.fetch_pool.borrow_mut() =
            Some(ModuleFetchPool::new_for_test(cookies, transport).unwrap());

        let errors = execute(&mut runtime, OWNER);
        assert!(errors.is_empty(), "{crossorigin}: {errors:?}");
        assert_eq!(runtime.eval("moduleValue").unwrap().as_number(), Some(42.0));
        runtime.eval("loadLater()").unwrap();
        runtime.run_jobs().unwrap();
        assert_eq!(runtime.eval("laterValue").unwrap().as_number(), Some(73.0));
        assert_eq!(
            requests.lock().unwrap().as_slice(),
            &[
                (
                    format!("{MODULE_ORIGIN}/root.js"),
                    Some(OWNER.into()),
                    expect_cookie.then(|| "module=secret".into())
                ),
                (
                    format!("{MODULE_ORIGIN}/dependency.js"),
                    Some(OWNER.into()),
                    expect_cookie.then(|| "module=secret".into())
                ),
                (
                    format!("{MODULE_ORIGIN}/later.js"),
                    Some(OWNER.into()),
                    expect_cookie.then(|| "module=secret".into())
                ),
            ],
            "{crossorigin}"
        );
    }
}

#[test]
fn failed_imports_settle_without_blocking_other_imports() {
    let server = ModuleServer::new(HashMap::from([
        (
            "/missing.js".into(),
            Route {
                status: 404,
                ..route("")
            },
        ),
        ("/invalid.js".into(), route("export const = ;")),
        ("/good.js".into(), route("export const answer = 42;")),
    ]));
    let mut runtime = runtime_with_module(
        &server.origin,
        "globalThis.done = Promise.allSettled([import('./missing.js'), import('./invalid.js'), import('./good.js')]).then(results => { globalThis.statuses = results.map(r => r.status).join(','); globalThis.answer = results[2].value.answer; });",
    );
    assert!(execute(&mut runtime, &server.origin).is_empty());
    assert_eq!(
        runtime
            .eval("statuses === 'rejected,rejected,fulfilled' && answer === 42")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    assert_eq!(server.paths().len(), 3);
}

#[test]
fn concurrent_iframe_imports_keep_separate_realms_and_csp() {
    let server = ModuleServer::new(HashMap::from([
        (
            "/shared.js".into(),
            Route {
                gate: true,
                ..route(
                    "globalThis.executions = (globalThis.executions || 0) + 1; export const token = {};",
                )
            },
        ),
        ("/first.html".into(), route("<html><body></body></html>")),
        ("/second.html".into(), route("<html><body></body></html>")),
        (
            "/blocked.html".into(),
            Route {
                headers: "Content-Security-Policy: script-src 'unsafe-inline'\r\n".into(),
                ..route("<html><body></body></html>")
            },
        ),
    ]));
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, &format!("{}/index.html", server.origin))
            .unwrap();
    for name in ["first", "second", "blocked"] {
        runtime.eval(&format!("globalThis.{name} = document.createElement('iframe'); {name}.src = '{}/{name}.html'; {name}.setAttribute('data-name', '{name}'); document.body.appendChild({name});", server.origin)).unwrap();
        runtime.run_until_idle().unwrap();
        let source = format!(
            "globalThis.loadLater = () => import('{}/shared.js').then(module => {{ globalThis.token = module.token; }}).catch(() => {{ globalThis.rejected = true; }});",
            server.origin
        );
        let (iframe_id, document) = {
            let state = runtime.host_state.borrow();
            let frame = state
                .document
                .query_selector(&format!("iframe[data-name='{name}']"))
                .unwrap();
            (
                frame.identity(),
                state.iframe_documents[&frame.identity()].document.clone(),
            )
        };
        if name == "blocked" {
            assert!(
                !runtime
                    .host_state
                    .borrow()
                    .csp_policy_for_document(&document)
                    .allows_reference(
                        super::ResourceType::Script,
                        &format!("{}/shared.js", server.origin)
                    ),
                "test frame must have its response CSP installed"
            );
        }
        let realm = runtime
            .ensure_iframe_realm(iframe_id, document.identity())
            .unwrap();
        let old_realm = runtime.context.enter_realm(realm);
        let result = runtime.eval_module_timed(
            &source,
            &format!("{}/entry-{name}.js", server.origin),
            document,
            crate::http::cors::CredentialsMode::SameOrigin,
        );
        runtime.context.enter_realm(old_realm);
        assert!(result.0.is_ok(), "{:?}", result.0);
    }
    runtime.eval("first.contentWindow.loadLater(); second.contentWindow.loadLater(); blocked.contentWindow.loadLater();").unwrap();
    runtime.run_jobs().unwrap();
    let observed = runtime.eval("JSON.stringify([first.contentWindow.executions, second.contentWindow.executions, blocked.contentWindow.rejected, typeof blocked.contentWindow.executions, typeof globalThis.executions, first.contentWindow.token === second.contentWindow.token])").unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(observed, r#"[1,1,true,"undefined","undefined",false]"#);

    assert_eq!(
        server
            .paths()
            .iter()
            .filter(|path| *path == "/shared.js")
            .count(),
        2
    );
    assert!(server.peak.load(Ordering::SeqCst) >= 2);
}

#[test]
fn csp_redirect_keeps_host_restrictions_but_ignores_source_path() {
    let server = ModuleServer::new(HashMap::from([
        (
            "/allowed.js".into(),
            Route {
                status: 302,
                headers: "Location: /other.js\r\n".into(),
                ..route("")
            },
        ),
        ("/other.js".into(), route("globalThis.redirectRan = true;")),
    ]));
    let policy = format!("script-src 'unsafe-inline' {}/allowed.js", server.origin);
    let mut runtime = runtime_with_policy(&server.origin, "import './allowed.js';", &policy);
    let errors = execute(&mut runtime, &server.origin);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        runtime.eval("redirectRan").unwrap().as_boolean(),
        Some(true)
    );
}

#[test]
fn srcdoc_meta_csp_restricts_scripts_without_weakening_parent_or_siblings() {
    let server = ModuleServer::new(HashMap::from([(
        "/child.js".into(),
        route("globalThis.childRan = true;"),
    )]));
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, &format!("{}/index.html", server.origin))
            .unwrap();
    runtime.install_csp_policy(&["script-src 'unsafe-inline' 'self'".to_string()]);
    for (name, policy) in [
        ("blocked", "script-src 'none'"),
        ("allowed", "script-src 'unsafe-inline' *"),
    ] {
        let child = format!(
            "<html><head><meta http-equiv='Content-Security-Policy' content=\"{policy}\"></head><body></body></html>"
        );
        runtime.eval(&format!("globalThis.{name} = document.createElement('iframe'); {name}.srcdoc = {}; document.body.appendChild({name});", serde_json::to_string(&child).unwrap())).unwrap();
        runtime.run_until_idle().unwrap();
        runtime.eval(&format!("(() => {{ const doc = {name}.contentDocument; const script = doc.createElement('script'); script.src = '{}/child.js'; doc.body.appendChild(script); }})()", server.origin)).unwrap();
    }
    runtime.run_until_idle().unwrap();
    let observed = runtime.eval("JSON.stringify([typeof blocked.contentWindow.childRan, allowed.contentWindow.childRan, blocked.contentDocument.cspViolations.length, allowed.contentDocument.cspViolations.length, document.cspViolations.length])").unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(
        observed,
        r#"["undefined",true,1,0,0]"#,
        "requests={:?}, errors={:?}",
        server.paths(),
        runtime.take_task_errors()
    );
    assert_eq!(server.paths(), ["/child.js"]);

    let state = runtime.host_state.borrow();
    for entry in state.iframe_documents.values() {
        let policy = state.csp_policy_for_document(&entry.document);
        assert!(
            !policy.allows_reference(super::ResourceType::Script, "https://other.test/app.js"),
            "child meta must not relax inherited 'self'"
        );
    }
}

#[test]
fn srcdoc_connect_csp_checks_and_violations_belong_to_the_calling_realm() {
    let server = ModuleServer::new(HashMap::from([
        (
            "/child.js".into(),
            route(
                "globalThis.load = () => fetch(new URL('/body', location.href === 'about:srcdoc' ? parent.location.href : location.href).href).then(() => globalThis.loaded = true).catch(() => globalThis.loaded = false);",
            ),
        ),
        ("/body".into(), route("ok")),
    ]));
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, &format!("{}/index.html", server.origin))
            .unwrap();
    runtime.install_csp_policy(&["script-src 'self'; connect-src 'self'".into()]);
    for (name, policy) in [
        ("blocked", "connect-src 'none'"),
        ("allowed", "connect-src 'self'"),
    ] {
        let child = format!(
            "<html><head><meta http-equiv='Content-Security-Policy' content=\"{policy}\"></head><body></body></html>"
        );
        runtime.eval(&format!("globalThis.{name} = document.createElement('iframe'); {name}.srcdoc = {}; document.body.appendChild({name});", serde_json::to_string(&child).unwrap())).unwrap();
        runtime.run_until_idle().unwrap();
        runtime.eval(&format!("(() => {{ const doc = {name}.contentDocument; const script = doc.createElement('script'); script.src = '{}/child.js'; doc.body.appendChild(script); }})()", server.origin)).unwrap();
        runtime.run_until_idle().unwrap();
    }
    runtime
        .eval("blocked.contentWindow.load(); allowed.contentWindow.load();")
        .unwrap();
    runtime.run_jobs().unwrap();
    let observed = runtime.eval("JSON.stringify([blocked.contentWindow.loaded, allowed.contentWindow.loaded, blocked.contentDocument.cspViolations.length, allowed.contentDocument.cspViolations.length, document.cspViolations.length])").unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(observed, "[false,true,1,0,0]");
    assert_eq!(
        server
            .paths()
            .iter()
            .filter(|path| *path == "/body")
            .count(),
        1
    );
}

#[test]
fn csp_connect_redirects_allow_new_paths_for_fetch_xhr_and_event_source() {
    let server = ModuleServer::new(HashMap::from([
        (
            "/allowed/body".into(),
            Route {
                status: 302,
                headers: "Location: /other/body\r\n".into(),
                ..route("")
            },
        ),
        ("/other/body".into(), route("ok")),
        (
            "/allowed/stream".into(),
            Route {
                status: 302,
                headers: "Location: /other/stream.events\r\n".into(),
                ..route("")
            },
        ),
        ("/other/stream.events".into(), route("data: ready\n\n")),
    ]));
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, &format!("{}/index.html", server.origin))
            .unwrap();
    runtime.install_csp_policy(&[format!("connect-src {}/allowed/", server.origin)]);
    runtime
        .eval(&format!(
            r#"
        globalThis.result = {{}};
        fetch('{0}/other/body').catch(() => result.directBlocked = true);
        fetch('{0}/allowed/body').then(r => r.text()).then(body => result.fetch = body);
        const xhr = new XMLHttpRequest();
        xhr.open('GET', '{0}/allowed/body');
        xhr.onload = () => result.xhr = xhr.responseText;
        xhr.send();
        const events = new EventSource('{0}/allowed/stream');
        events.onmessage = event => {{ result.event = event.data; events.close(); }};
        events.onerror = () => {{ result.event = 'error'; events.close(); }};
    "#,
            server.origin
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.eval("JSON.stringify([result.directBlocked, result.fetch, result.xhr, result.event, document.cspViolations.length])").unwrap().as_string().unwrap().to_std_string_escaped(), r#"[true,"ok","ok","ready",1]"#);
    assert_eq!(
        server
            .paths()
            .iter()
            .filter(|path| *path == "/other/body")
            .count(),
        2
    );
    assert_eq!(server.paths().len(), 6);
}

#[test]
fn csp_classic_script_redirects_work_in_parser_and_dynamic_execution_paths() {
    let server = ModuleServer::new(HashMap::from([
        (
            "/allowed.js".into(),
            Route {
                status: 302,
                headers: "Location: /other.js\r\n".into(),
                ..route("")
            },
        ),
        (
            "/other.js".into(),
            route(
                "globalThis.executions = (globalThis.executions || 0) + 1;\
                 fetch('https://blocked.test/from-script').catch(() => {});",
            ),
        ),
    ]));
    for page_task in [false, true] {
        let document = TreeBuilder::parse("<html><body><script src='/allowed.js'></script><script defer src='/allowed.js'></script></body></html>").document();
        let url: crate::http::Url = format!("{}/index.html", server.origin).parse().unwrap();
        let mut runtime = JsRuntime::with_document_and_url(document, &url.to_string()).unwrap();
        runtime.install_csp_policy(&[format!(
            "script-src {0}/allowed.js; connect-src 'none'; report-to observations",
            server.origin
        )]);
        runtime
            .eval(
                "globalThis.scriptSourceReports=[];\
                 new ReportingObserver(batch=>scriptSourceReports.push(...batch),{\
                   types:['csp-violation']\
                 }).observe();",
            )
            .unwrap();
        if page_task {
            use std::future::Future;
            use std::task::{Context, Poll, Waker};
            let mut task = Box::pin(runtime.into_document_page_task(1, Some(url)));
            let mut context = Context::from_waker(Waker::noop());
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let completed = loop {
                if let Poll::Ready(completed) = task.as_mut().poll(&mut context) {
                    break completed;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "page task did not settle"
                );
                thread::sleep(Duration::from_millis(1));
            };
            assert_eq!(completed.result, Ok(Vec::new()));
            runtime = completed.runtime;
        } else {
            assert!(runtime.execute_document_scripts(Some(&url)).is_empty());
        }
        assert_eq!(runtime.eval("executions").unwrap().as_number(), Some(2.0));
        runtime.eval("const inserted = document.createElement('script'); inserted.src = '/allowed.js'; document.body.appendChild(inserted);").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(runtime.eval("executions").unwrap().as_number(), Some(3.0));
        let requested_url = format!("{}/allowed.js", server.origin);
        assert!(
            runtime
                .eval(&format!(
                    "scriptSourceReports.length===3 && \
                     scriptSourceReports.every(report=>\
                       report.body.sourceFile==={} && \
                       report.body.lineNumber===1 && report.body.columnNumber>0) && \
                     document.cspViolations.length===1 && \
                     document.cspViolations[0].resourceType==='connect'",
                    serde_json::to_string(&requested_url).unwrap()
                ))
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        assert!(runtime.take_task_errors().is_empty());
    }
}

#[test]
fn srcdoc_meta_csp_limits_dynamic_module_imports_to_the_owning_document() {
    for kind in ["", "module"] {
        let server = ModuleServer::new(HashMap::from([
            (
                "/bootstrap.js".into(),
                route(
                    "globalThis.load = () => import('./dependency.js').then(() => globalThis.loaded = true).catch(error => { globalThis.importError = String(error); globalThis.loaded = false; });",
                ),
            ),
            (
                "/dependency.js".into(),
                route("globalThis.moduleRan = true;"),
            ),
        ]));
        let document = TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, &format!("{}/index.html", server.origin))
                .unwrap();
        runtime.install_csp_policy(&["script-src 'self'".into()]);
        for (name, policy) in [
            (
                "blocked",
                format!("script-src {}/bootstrap.js", server.origin),
            ),
            ("allowed", "script-src *".to_string()),
        ] {
            let child = format!(
                "<html><head><meta http-equiv='Content-Security-Policy' content=\"{policy}\"></head><body></body></html>"
            );
            runtime.eval(&format!("globalThis.{name} = document.createElement('iframe'); {name}.srcdoc = {}; document.body.appendChild({name});", serde_json::to_string(&child).unwrap())).unwrap();
            runtime.run_until_idle().unwrap();
            runtime.eval(&format!("(() => {{ const doc = {name}.contentDocument; const script = doc.createElement('script'); script.type = '{kind}'; script.src = '{}/bootstrap.js'; doc.body.appendChild(script); }})()", server.origin)).unwrap();
            runtime.run_until_idle().unwrap();
        }
        runtime
            .eval("blocked.contentWindow.load(); allowed.contentWindow.load();")
            .unwrap();
        runtime.run_jobs().unwrap();
        assert_eq!(runtime.eval("JSON.stringify([blocked.contentWindow.loaded, allowed.contentWindow.loaded, typeof blocked.contentWindow.moduleRan, allowed.contentWindow.moduleRan, blocked.contentDocument.cspViolations.length, allowed.contentDocument.cspViolations.length, document.cspViolations.length])").unwrap().as_string().unwrap().to_std_string_escaped(), r#"[false,true,"undefined",true,1,0,0]"#, "errors={}, requests={:?}", runtime.eval("JSON.stringify([blocked.contentWindow.importError, allowed.contentWindow.importError])").unwrap().as_string().unwrap().to_std_string_escaped(), server.paths());
        assert_eq!(
            server
                .paths()
                .iter()
                .filter(|path| *path == "/dependency.js")
                .count(),
            1
        );
    }
}
