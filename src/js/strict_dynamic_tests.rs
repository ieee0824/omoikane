use super::JsRuntime;
use crate::html::TreeBuilder;
use crate::test_support::http_fixture::{
    FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback, read_request_headers,
};
use std::collections::HashMap;
use std::io::Write;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[derive(Clone)]
struct Route {
    status: u16,
    body: String,
    headers: String,
}

impl Route {
    fn script(source: &str) -> Self {
        Self {
            status: 200,
            body: source.into(),
            headers: String::new(),
        }
    }

    fn document(markup: &str) -> Self {
        Self {
            status: 200,
            body: markup.into(),
            headers:
                "Content-Security-Policy: script-src-elem 'strict-dynamic' http: 'nonce-ok'\r\n"
                    .into(),
        }
    }

    fn redirect(destination: &str) -> Self {
        Self {
            status: 302,
            body: String::new(),
            headers: format!("Location: {destination}\r\n"),
        }
    }
}

struct ScriptServer {
    origin: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<FixtureWorker<()>>,
}

impl ScriptServer {
    fn new(routes: HashMap<&'static str, Route>) -> Self {
        let listener = bind_loopback().unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_requests = requests.clone();
        let worker_stop = stop.clone();
        let worker = FixtureWorker::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                let mut stream = match accept_with_timeout(&listener, Duration::from_millis(20)) {
                    Ok(stream) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
                    Err(error) => panic!("accept: {error}"),
                };
                let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .split('?')
                    .next()
                    .unwrap();
                worker_requests.lock().unwrap().push(path.into());
                let route = routes
                    .get(path)
                    .unwrap_or_else(|| panic!("unexpected {path}"));
                let mime = if path.ends_with(".html") {
                    "text/html"
                } else {
                    "text/javascript"
                };
                write!(stream, "HTTP/1.1 {} Fixture\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}", route.status, route.body.len(), route.headers, route.body).unwrap();
            }
        });
        Self {
            origin,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn paths(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for ScriptServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        drop(self.worker.take());
    }
}

fn observed(runtime: &mut JsRuntime, expression: &str) -> String {
    runtime
        .eval(expression)
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn parser_script_needs_nonce_while_dom_script_and_redirect_run() {
    let server = ScriptServer::new(HashMap::from([
        ("/blocked.js", Route::script("globalThis.blockedRan = true")),
        ("/trusted.js", Route::redirect("/trusted-final.js")),
        (
            "/trusted-final.js",
            Route::script("globalThis.trustedRan = true"),
        ),
        ("/inline", Route::script("globalThis.inlineNamedRan = true")),
        ("/dynamic.js", Route::redirect("/dynamic-final.js")),
        (
            "/dynamic-final.js",
            Route::script("globalThis.dynamicRan = true"),
        ),
    ]));
    let document = TreeBuilder::parse(&format!("<body><script id=blocked src='{0}/blocked.js'></script><script nonce=ok src='{0}/trusted.js'></script><script nonce=ok src='inline'></script>", server.origin)).document();
    let url = format!("{}/index.html", server.origin);
    let mut runtime = JsRuntime::with_document_and_url(document, &url).unwrap();
    runtime.install_response_security_headers(&[
        (
            "Content-Security-Policy".into(),
            "script-src-elem 'strict-dynamic' http: 'nonce-ok'".into(),
        ),
        (
            "Content-Security-Policy-Report-Only".into(),
            "script-src-elem 'strict-dynamic' 'nonce-monitor'".into(),
        ),
    ]);
    runtime.eval("globalThis.events = []; document.addEventListener('securitypolicyviolation', e => events.push([e.disposition, e.target.id || '', e.blockedURI]));").unwrap();
    assert!(
        runtime
            .execute_document_scripts(Some(&url.parse().unwrap()))
            .is_empty()
    );
    runtime.eval(&format!("const dynamic = document.createElement('script'); dynamic.src = '{}/dynamic.js'; document.body.appendChild(dynamic);", server.origin)).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        observed(
            &mut runtime,
            "JSON.stringify([typeof blockedRan, trustedRan, inlineNamedRan, dynamicRan, document.cspViolations.length, events.filter(e => e[0] === 'enforce').map(e => e[1]), events.filter(e => e[0] === 'report').length, events.filter(e => e[2].endsWith('/inline')).length])"
        ),
        "[\"undefined\",true,true,true,1,[\"blocked\"],5,2]"
    );
    assert_eq!(
        server.paths(),
        [
            "/trusted.js",
            "/trusted-final.js",
            "/inline",
            "/dynamic.js",
            "/dynamic-final.js"
        ]
    );
}

#[test]
fn document_write_keeps_parser_metadata_and_nonce() {
    let server = ScriptServer::new(HashMap::from([
        ("/blocked.js", Route::script("globalThis.blockedRan = true")),
        ("/written.js", Route::script("globalThis.writtenRan = true")),
    ]));
    let mut runtime =
        JsRuntime::with_document_and_url(TreeBuilder::parse("<body>").document(), &server.origin)
            .unwrap();
    runtime.install_csp_policy(&["script-src-elem 'strict-dynamic' http: 'nonce-ok'".into()]);
    runtime.eval("document.write('<script src=\"/blocked.js\"></script><script nonce=\"ok\" src=\"/written.js\"></script>')").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        observed(
            &mut runtime,
            "JSON.stringify([typeof blockedRan, writtenRan, document.cspViolations.length])"
        ),
        "[\"undefined\",true,1]"
    );
    assert_eq!(server.paths(), ["/written.js"]);
}

#[test]
fn iframe_and_popup_parser_scripts_use_their_response_policy() {
    let page =
        "<body><script src='/blocked.js'></script><script nonce=ok src='/allowed.js'></script>";
    let server = ScriptServer::new(HashMap::from([
        ("/frame.html", Route::document(page)),
        ("/popup.html", Route::document(page)),
        ("/blocked.js", Route::script("globalThis.blockedRan = true")),
        ("/allowed.js", Route::script("globalThis.allowedRan = true")),
    ]));
    let mut runtime =
        JsRuntime::with_document_and_url(TreeBuilder::parse("<body>").document(), &server.origin)
            .unwrap();
    runtime.eval("globalThis.frame = document.createElement('iframe'); frame.src = '/frame.html'; document.body.appendChild(frame); globalThis.popup = window.open('/popup.html', 'strict-dynamic-popup');").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        observed(
            &mut runtime,
            "JSON.stringify([typeof frame.contentWindow.blockedRan, frame.contentWindow.allowedRan, frame.contentDocument.cspViolations.length, typeof popup.blockedRan, popup.allowedRan, popup.document.cspViolations.length])"
        ),
        "[\"undefined\",true,1,\"undefined\",true,1]"
    );
    let paths = server.paths();
    assert_eq!(
        paths
            .iter()
            .filter(|path| path.as_str() == "/allowed.js")
            .count(),
        2
    );
    assert!(!paths.contains(&"/blocked.js".to_string()));
}
