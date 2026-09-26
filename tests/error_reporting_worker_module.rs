use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use omoikane::{
    error_reporting::{
        ErrorCategory, ErrorCode, ErrorReporter, ErrorSeverity, EventStore, ExecutionSurface,
        RawEvent, ReporterConfig, RetentionPolicy,
    },
    html::TreeBuilder,
    http::Url,
    js::JsRuntime,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDatabase(PathBuf);

impl TestDatabase {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "omoikane-worker-module-report-{}-{sequence}",
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

    fn assert_report(&self, category: ErrorCategory, code: &'static str, operation: &'static str) {
        let resource = match category {
            ErrorCategory::Worker => "worker",
            ErrorCategory::Module => "module",
            _ => panic!("unexpected category"),
        };
        let message = match category {
            ErrorCategory::Worker => "Worker execution failed",
            ErrorCategory::Module => "Module loading failed",
            _ => unreachable!(),
        };
        let event = RawEvent::new(
            category,
            ErrorSeverity::Error,
            ErrorCode::new(code).unwrap(),
            ExecutionSurface::Headless,
            message,
            &[("operation", operation), ("resource", resource)],
        )
        .sanitize();
        let store = EventStore::open(self.path(), RetentionPolicy::default()).unwrap();
        let report = store
            .get(event.fingerprint())
            .unwrap()
            .unwrap_or_else(|| panic!("missing report: {code}"));
        assert_eq!(report.category, resource);
        assert_eq!(report.message, message);
        assert_eq!(report.occurrences, 1);
    }

    fn assert_no_secrets(&self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.0.as_os_str().to_os_string();
            path.push(suffix);
            if let Ok(bytes) = fs::read(PathBuf::from(path)) {
                for secret in [b"BODY_SECRET_936".as_slice(), b"QUERY_SECRET_936"] {
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

#[test]
fn worker_startup_and_task_failures_keep_owner_errors_and_sanitize_reports() {
    let database = TestDatabase::new();
    let reporter = database.reporter();
    let mut runtime = JsRuntime::new().unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    runtime
        .eval(
            r#"globalThis.workerErrors = [];
               const startup = new Worker('data:text/javascript,throw%20new%20Error(%22BODY_SECRET_936%22)');
               startup.onerror = event => workerErrors.push(event.message);
               const task = new Worker('data:text/javascript,setTimeout(()=%3E%7Bthrow%20new%20Error(%22BODY_SECRET_936%22)%7D%2C1)');
               task.onerror = event => workerErrors.push(event.message);"#,
        )
        .unwrap();
    runtime.tick(1).unwrap();
    assert_eq!(
        runtime.eval("workerErrors.length").unwrap().as_number(),
        Some(2.0)
    );
    assert_eq!(runtime.eval("2 + 3").unwrap().as_number(), Some(5.0));
    reporter.flush().unwrap();
    database.assert_report(ErrorCategory::Worker, "WORKER_STARTUP_FAILED", "execute");
    database.assert_report(ErrorCategory::Worker, "WORKER_RUNTIME_FAILED", "execute");
    database.assert_no_secrets();
}

#[test]
fn shared_worker_startup_failure_is_recorded_without_stopping_page() {
    let database = TestDatabase::new();
    let reporter = database.reporter();
    let mut runtime = JsRuntime::new().unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    runtime
        .eval(
            r#"globalThis.sharedErrors = [];
               const startup = new SharedWorker('data:text/javascript,throw%20new%20Error(%22BODY_SECRET_936%22)');
               startup.onerror = event => sharedErrors.push(event.message);"#,
        )
        .unwrap();
    runtime.tick(1).unwrap();
    assert_eq!(
        runtime.eval("sharedErrors.length").unwrap().as_number(),
        Some(1.0)
    );
    assert_eq!(runtime.eval("3 + 4").unwrap().as_number(), Some(7.0));
    reporter.flush().unwrap();
    database.assert_report(
        ErrorCategory::Worker,
        "SHARED_WORKER_STARTUP_FAILED",
        "execute",
    );
    database.assert_no_secrets();
}

#[test]
fn module_evaluation_failure_keeps_later_scripts_and_omits_source_and_query() {
    let database = TestDatabase::new();
    let reporter = database.reporter();
    let html = r#"<script type="module">throw new Error('BODY_SECRET_936')</script>
        <script>globalThis.afterModule = true</script>"#;
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(html).document()).unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    let url: Url = "https://example.test/page?token=QUERY_SECRET_936"
        .parse()
        .unwrap();
    let errors = runtime.execute_document_scripts(Some(&url));
    assert_eq!(errors.len(), 1);
    assert!(runtime.eval("afterModule").unwrap().as_boolean().unwrap());
    reporter.flush().unwrap();
    database.assert_report(
        ErrorCategory::Module,
        "DOCUMENT_MODULE_EVALUATION_FAILED",
        "execute",
    );
    database.assert_no_secrets();
}

#[test]
fn imported_module_load_failure_preserves_evaluation_error() {
    let database = TestDatabase::new();
    let reporter = database.reporter();
    let html = r#"<script type="module">import 'http://['; globalThis.moduleLoaded = true;</script>
        <script>globalThis.afterImport = true</script>"#;
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(html).document()).unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    let url: Url = "https://example.test/page?token=QUERY_SECRET_936"
        .parse()
        .unwrap();
    let errors = runtime.execute_document_scripts(Some(&url));
    assert_eq!(errors.len(), 1);
    assert!(runtime.eval("afterImport").unwrap().as_boolean().unwrap());
    assert!(
        runtime
            .eval("typeof moduleLoaded === 'undefined'")
            .unwrap()
            .as_boolean()
            .unwrap()
    );
    reporter.flush().unwrap();
    database.assert_report(ErrorCategory::Module, "MODULE_IMPORT_LOAD_FAILED", "fetch");
    database.assert_report(
        ErrorCategory::Module,
        "DOCUMENT_MODULE_EVALUATION_FAILED",
        "execute",
    );
    database.assert_no_secrets();
}
