use super::JsRuntime;
use crate::html::TreeBuilder;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
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

struct ModuleServer {
    origin: String,
    requests: Arc<Mutex<Vec<(String, String)>>>,
    peak: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ModuleServer {
    fn new(routes: HashMap<String, Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
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
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("accept: {error}"),
                };
                let routes = routes.clone();
                let requests = worker_requests.clone();
                let active = active.clone();
                let peak = worker_peak.clone();
                let gate = gate.clone();
                handlers.push(thread::spawn(move || {
                    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut reader = BufReader::new(&mut stream);
                    let mut request = String::new();
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        assert!(!line.is_empty(), "incomplete request");
                        request.push_str(&line);
                        if line == "\r\n" { break; }
                    }
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
                    let mime = if path.ends_with(".html") { "text/html" } else { "text/javascript" };
                    write!(stream, "HTTP/1.1 {} Result\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}", route.status, route.body.len(), route.headers, route.body).unwrap();
                }));
            }
            for handler in handlers {
                handler.join().unwrap();
            }
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
        self.worker.take().unwrap().join().unwrap();
    }
}

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

#[test]
fn csp_blocks_dependency_before_fetch_and_redirect_before_execution() {
    let target = ModuleServer::new(HashMap::from([(
        "/blocked.js".into(),
        route("globalThis.blockedRan = true;"),
    )]));
    let server = ModuleServer::new(HashMap::from([(
        "/redirect.js".into(),
        Route {
            status: 302,
            headers: format!("Location: {}/blocked.js\r\n", target.origin),
            ..route("")
        },
    )]));
    let policy = format!("script-src 'unsafe-inline' {}", server.origin);
    let source = format!("import '{}/blocked.js';", target.origin);
    let mut runtime = runtime_with_policy(&server.origin, &source, &policy);
    let errors = execute(&mut runtime, &server.origin);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("CSP blocked module import")),
        "{errors:?}"
    );
    assert!(server.paths().is_empty());
    assert!(target.paths().is_empty());

    let mut runtime = runtime_with_policy(&server.origin, "import './redirect.js';", &policy);
    let errors = execute(&mut runtime, &server.origin);
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
    assert_eq!(server.paths(), ["/redirect.js"]);
    assert_eq!(target.paths(), ["/blocked.js"]);
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
