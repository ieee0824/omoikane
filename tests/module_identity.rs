use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;
use std::io::Write;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[path = "support/http_fixture.rs"]
mod http_fixture;

use http_fixture::{
    FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback, read_request_headers,
};

struct ModuleServer {
    url: String,
    stop: Arc<AtomicBool>,
    worker: Option<FixtureWorker<Vec<String>>>,
}

impl ModuleServer {
    fn start() -> Self {
        let listener = bind_loopback().unwrap();
        let url = format!("http://{}/index.html", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = FixtureWorker::spawn(move || {
            let mut requests = Vec::new();
            while !worker_stop.load(Ordering::Relaxed) {
                let mut stream = match accept_with_timeout(&listener, Duration::from_millis(50)) {
                    Ok(stream) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
                    Err(error) => panic!("accept: {error}"),
                };
                let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                let path = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                let body = match path.as_str() {
                    "/singleton.js" => {
                        "globalThis.executions = (globalThis.executions || 0) + 1; export const token = {};"
                    }
                    "/late.js" => "export { token } from './singleton.js';",
                    _ => panic!("unexpected module: {path}"),
                };
                requests.push(path);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        Self {
            url,
            stop,
            worker: Some(worker),
        }
    }

    fn finish(&mut self) -> Vec<String> {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join()
    }
}

impl Drop for ModuleServer {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.finish();
        }
    }
}

#[test]
fn separate_module_scripts_and_later_imports_share_document_module_identity() {
    let mut server = ModuleServer::start();
    let document = TreeBuilder::parse(
        r#"<html><body>
        <script type="module">
            import { token } from './singleton.js';
            globalThis.firstToken = token;
            globalThis.loadLater = () => import('./late.js').then(module => {
                globalThis.sameDynamicToken = module.token === token;
            });
        </script>
        <script type="module">
            import { token } from './singleton.js';
            globalThis.sameStaticToken = token === firstToken;
        </script>
    </body></html>"#,
    )
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, &server.url).unwrap();
    let errors = runtime.execute_document_scripts(Some(&server.url.parse().unwrap()));
    assert!(errors.is_empty(), "{errors:?}");
    // Start this import in a later task, after the entry module's host guard
    // has been restored. The module record must still belong to its Document.
    runtime.eval("setTimeout(loadLater, 0)").unwrap();
    runtime.run_timers(10, 1, 100);
    let requests = server.finish();
    assert_eq!(
        runtime.eval("sameStaticToken").unwrap().as_boolean(),
        Some(true)
    );
    assert_eq!(
        runtime.eval("sameDynamicToken").unwrap().as_boolean(),
        Some(true)
    );
    assert_eq!(runtime.eval("executions").unwrap().as_number(), Some(1.0));
    assert_eq!(requests, ["/singleton.js", "/late.js"]);
}
