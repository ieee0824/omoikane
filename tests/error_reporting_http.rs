use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use omoikane::{
    error_reporting::{
        ErrorCategory, ErrorCode, ErrorReporter, ErrorSeverity, EventStore, ExecutionSurface,
        RawEvent, ReporterConfig, RetentionPolicy,
    },
    html::TreeBuilder,
    http::{Client, HttpRequest, Url},
    js::JsRuntime,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDatabase(PathBuf);

impl TestDatabase {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "omoikane-http-report-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        Self(directory.join("events.sqlite"))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn reporter(&self) -> Arc<ErrorReporter> {
        let config = ReporterConfig::from_values(Some("record-only"), None).unwrap();
        Arc::new(ErrorReporter::new(&config, self.0.clone(), RetentionPolicy::default()).unwrap())
    }

    fn assert_report(&self, code: &'static str) {
        let event = RawEvent::new(
            ErrorCategory::Http,
            ErrorSeverity::Error,
            ErrorCode::new(code).unwrap(),
            ExecutionSurface::Headless,
            "Resource load failed",
            &[("operation", "fetch"), ("resource", "other")],
        )
        .sanitize();
        let store = EventStore::open(self.path(), RetentionPolicy::default()).unwrap();
        let report = store
            .get(event.fingerprint())
            .unwrap()
            .unwrap_or_else(|| panic!("missing report: {code}"));
        assert_eq!(report.category, "http");
        assert_eq!(report.message, "Resource load failed");
        assert_eq!(report.occurrences, 1);
    }

    fn assert_no_secrets(&self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.0.as_os_str().to_os_string();
            path.push(suffix);
            if let Ok(bytes) = fs::read(PathBuf::from(path)) {
                for secret in [
                    b"QUERY_SECRET_938".as_slice(),
                    b"BODY_SECRET_938",
                    b"AUTH_SECRET_938",
                ] {
                    assert!(!bytes.windows(secret.len()).any(|window| window == secret));
                }
            }
        }
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
    }
}

fn serve_once(response: &'static [u8]) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 2048];
        let _ = stream.read(&mut request).unwrap();
        stream.write_all(response).unwrap();
    });
    (format!("http://{address}"), handle)
}

#[test]
fn transport_and_redirect_failures_keep_http_error_results() {
    let database = TestDatabase::new();
    let reporter = database.reporter();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = listener.local_addr().unwrap();
    drop(listener);
    let mut client = Client::new();
    client.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    let mut request =
        HttpRequest::get(&format!("http://{closed}/?token=QUERY_SECRET_938")).unwrap();
    request.add_header("Authorization", "Bearer AUTH_SECRET_938");
    let transport_error = client.send(request).unwrap_err();
    assert!(transport_error.to_string().starts_with("I/O error:"));

    let (base, server) = serve_once(
        b"HTTP/1.1 302 Found\r\nLocation: /again\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    client.set_max_redirects(0);
    let redirect_error = client
        .get(&format!("{base}/start?token=QUERY_SECRET_938"))
        .unwrap_err();
    server.join().unwrap();
    assert_eq!(redirect_error.to_string(), "too many redirects");
    reporter.flush().unwrap();
    database.assert_report("HTTP_TRANSPORT_FAILED");
    database.assert_report("HTTP_REDIRECT_FAILED");
    database.assert_no_secrets();
}

#[test]
fn failed_script_response_is_reported_without_changing_later_script_execution() {
    let database = TestDatabase::new();
    let reporter = database.reporter();
    let (base, server) = serve_once(
        b"HTTP/1.1 404 Not Found\r\nContent-Type: text/javascript\r\nContent-Length: 15\r\nConnection: close\r\n\r\nBODY_SECRET_938",
    );
    let html = format!(
        "<script src='{base}/missing.js?token=QUERY_SECRET_938'></script><script>globalThis.afterFailure = true</script>"
    );
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(&html).document()).unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    let page_url: Url = format!("{base}/page").parse().unwrap();
    let errors = runtime.execute_document_scripts(Some(&page_url));
    server.join().unwrap();
    assert_eq!(errors.len(), 1);
    assert!(runtime.eval("afterFailure").unwrap().as_boolean().unwrap());
    reporter.flush().unwrap();
    database.assert_report("HTTP_RESOURCE_LOAD_FAILED");
    database.assert_no_secrets();
}
