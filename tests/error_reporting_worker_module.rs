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
fn shared_worker_runtime_exception_is_recorded_without_owner_error_or_page_failure() {
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
        // Runtime errors are reported at SharedWorkerGlobalScope. Only a
        // DedicatedWorkerGlobalScope forwards them to its Worker object.
        Some(0.0)
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

#[test]
fn explicit_worker_error_is_recorded_once_and_canceled_error_is_not_recorded() {
    let database = TestDatabase::new();
    let reporter = database.reporter();
    let mut runtime = JsRuntime::new().unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    runtime
        .eval(
            r#"
        globalThis.ownerReports = 0;
        for (const cancel of [true, false]) {
            const source = `onerror = () => ${cancel}; reportError(new Error('BODY_SECRET_936'));`;
            const worker = new Worker('data:text/javascript,' + encodeURIComponent(source));
            worker.onerror = event => { ownerReports++; event.preventDefault(); };
        }
    "#,
        )
        .unwrap();
    runtime.run_timers(20, 1, 80);
    assert_eq!(runtime.eval("ownerReports").unwrap().as_number(), Some(1.0));
    reporter.flush().unwrap();
    database.assert_report(ErrorCategory::Worker, "WORKER_RUNTIME_FAILED", "execute");
    database.assert_no_secrets();
}

#[test]
fn shared_worker_runtime_errors_record_once_and_respect_global_cancellation() {
    for trigger in [
        "reportError(new Error('BODY_SECRET_936'))",
        "queueMicrotask(() => { throw new Error('BODY_SECRET_936'); })",
        "setTimeout(() => { throw new Error('BODY_SECRET_936'); }, 0)",
        "throw new Error('BODY_SECRET_936')",
    ] {
        let database = TestDatabase::new();
        let reporter = database.reporter();
        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
        let trigger = serde_json::to_string(trigger).unwrap();
        runtime.eval(&format!(r#"
            globalThis.sharedLocalReports = [];
            globalThis.sharedOwnerReports = 0;
            for (const canceled of [false, true]) {{
                const source = `onconnect = event => {{
                    const port = event.ports[0];
                    onerror = message => {{ port.postMessage(message); return ${{canceled}}; }};
                    port.onmessage = () => {{ ${{{trigger}}} }};
                }};`;
                const worker = new SharedWorker('data:text/javascript,' + encodeURIComponent(source));
                worker.onerror = () => sharedOwnerReports++;
                worker.port.onmessage = event => sharedLocalReports.push(event.data);
                worker.port.postMessage('trigger');
            }}
        "#)).unwrap();
        runtime.run_until_idle().unwrap();
        runtime.tick(0).unwrap();
        assert_eq!(
            runtime
                .eval("sharedLocalReports.length")
                .unwrap()
                .as_number(),
            Some(2.0)
        );
        assert_eq!(
            runtime.eval("sharedOwnerReports").unwrap().as_number(),
            Some(0.0)
        );
        reporter.flush().unwrap();
        database.assert_report(
            ErrorCategory::Worker,
            "SHARED_WORKER_RUNTIME_FAILED",
            "execute",
        );
        database.assert_no_secrets();
    }
}

#[test]
fn worker_startup_host_abort_records_diagnostic_without_author_error_event() {
    let database = TestDatabase::new();
    let reporter = database.reporter();
    let mut runtime = JsRuntime::new().unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    runtime
        .eval(
            r#"
        globalThis.abortOwnerEvents = 0;
        const source = 'while (true) {}';
        for (const Constructor of [Worker, SharedWorker]) {
            const worker = new Constructor('data:text/javascript,' + encodeURIComponent(source));
            worker.onerror = event => { abortOwnerEvents++; event.preventDefault(); };
        }
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.eval("abortOwnerEvents").unwrap().as_number(),
        Some(0.0)
    );
    assert_eq!(runtime.eval("2 + 3").unwrap().as_number(), Some(5.0));
    reporter.flush().unwrap();
    database.assert_report(ErrorCategory::Worker, "WORKER_STARTUP_FAILED", "execute");
    database.assert_report(
        ErrorCategory::Worker,
        "SHARED_WORKER_STARTUP_FAILED",
        "execute",
    );
    database.assert_no_secrets();
}

#[test]
fn worker_task_host_aborts_record_diagnostics_without_author_error_events() {
    for trigger in [
        "setTimeout(() => { while (true) {} }, 0)",
        "queueMicrotask(() => { while (true) {} })",
        "while (true) {}",
    ] {
        eprintln!("host-abort trigger: {trigger}");
        let database = TestDatabase::new();
        let reporter = database.reporter();
        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
        let trigger = serde_json::to_string(trigger).unwrap();
        runtime.eval(&format!(r#"
            globalThis.abortAuthorEvents = 0;
            const dedicatedSource = `onerror = () => {{ postMessage('unexpected'); }};
                onmessage = () => {{ ${{{trigger}}} }};`;
            const dedicated = new Worker('data:text/javascript,' + encodeURIComponent(dedicatedSource));
            dedicated.onerror = event => {{ abortAuthorEvents++; event.preventDefault(); }};
            dedicated.onmessage = () => abortAuthorEvents++;
            dedicated.postMessage('trigger');
            const sharedSource = `onconnect = event => {{
                const port = event.ports[0];
                onerror = () => {{ port.postMessage('unexpected'); }};
                port.onmessage = () => {{ ${{{trigger}}} }};
            }};`;
            const shared = new SharedWorker('data:text/javascript,' + encodeURIComponent(sharedSource));
            shared.onerror = event => {{ abortAuthorEvents++; event.preventDefault(); }};
            shared.port.onmessage = () => abortAuthorEvents++;
            shared.port.postMessage('trigger');
        "#)).unwrap();
        runtime.run_until_idle().unwrap();
        runtime.tick(0).unwrap();
        assert_eq!(
            runtime.eval("abortAuthorEvents").unwrap().as_number(),
            Some(0.0),
            "{trigger}"
        );
        assert_eq!(runtime.eval("2 + 3").unwrap().as_number(), Some(5.0));
        reporter.flush().unwrap();
        database.assert_report(ErrorCategory::Worker, "WORKER_RUNTIME_FAILED", "execute");
        database.assert_report(
            ErrorCategory::Worker,
            "SHARED_WORKER_RUNTIME_FAILED",
            "execute",
        );
        database.assert_no_secrets();
    }
}
