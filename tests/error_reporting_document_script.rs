use std::{
    fs,
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll, Waker},
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
            "omoikane-document-script-report-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        Self(directory.join("events.sqlite"))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
    }
}

const HTML: &str = r#"<html><body>
    <script>throw new Error('SOURCE_SECRET_934')</script>
    <script>globalThis.afterFailure = true</script>
</body></html>"#;

fn assert_database_omits_page_secrets(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut file = path.as_os_str().to_os_string();
        file.push(suffix);
        if let Ok(bytes) = fs::read(PathBuf::from(file)) {
            for secret in [b"SOURCE_SECRET_934".as_slice(), b"QUERY_SECRET_934"] {
                assert!(!bytes.windows(secret.len()).any(|window| window == secret));
            }
        }
    }
}

#[test]
fn record_only_saves_one_safe_script_failure_and_preserves_execution() {
    let database = TestDatabase::new();
    let config = ReporterConfig::from_values(Some("record-only"), None).unwrap();
    let reporter = Arc::new(
        ErrorReporter::new(
            &config,
            database.path().to_path_buf(),
            RetentionPolicy::default(),
        )
        .unwrap(),
    );
    let document = TreeBuilder::parse(HTML).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    let url: Url = "https://example.test/page?token=QUERY_SECRET_934"
        .parse()
        .unwrap();

    let errors = runtime.execute_document_scripts(Some(&url));
    assert_eq!(errors.len(), 1);
    assert!(runtime.eval("afterFailure").unwrap().as_boolean().unwrap());
    reporter.flush().unwrap();

    let store = EventStore::open(database.path(), RetentionPolicy::default()).unwrap();
    assert_eq!(store.len().unwrap(), 1);
    let expected = RawEvent::new(
        ErrorCategory::JavaScript,
        ErrorSeverity::Error,
        ErrorCode::new("DOCUMENT_SCRIPT_EVALUATION_FAILED").unwrap(),
        ExecutionSurface::Headless,
        "JavaScript execution failed",
        &[("operation", "execute"), ("resource", "script")],
    )
    .sanitize();
    let row = store.get(expected.fingerprint()).unwrap().unwrap();
    assert_eq!(row.category, "javascript");
    assert_eq!(row.message, "JavaScript execution failed");
    assert_eq!(row.surface, "headless");
    assert_eq!(row.occurrences, 1);
    drop(store);
    drop(runtime);
    drop(reporter);

    assert_database_omits_page_secrets(database.path());
}

#[test]
fn owned_document_task_records_script_failure_without_aborting_later_scripts() {
    let database = TestDatabase::new();
    let config = ReporterConfig::from_values(Some("record-only"), None).unwrap();
    let reporter = Arc::new(
        ErrorReporter::new(
            &config,
            database.path().to_path_buf(),
            RetentionPolicy::default(),
        )
        .unwrap(),
    );
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(HTML).document()).unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    let url: Url = "https://example.test/page?token=QUERY_SECRET_934"
        .parse()
        .unwrap();
    let mut task = Box::pin(runtime.into_document_page_task(7, Some(url)));
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let mut completed = (0..100)
        .find_map(|_| match task.as_mut().poll(&mut context) {
            Poll::Ready(completed) => Some(completed),
            Poll::Pending => None,
        })
        .expect("document page task did not complete without a dialog");
    let errors = completed.result.as_ref().unwrap();
    assert_eq!(errors.len(), 1);
    assert!(
        completed
            .runtime
            .eval("afterFailure")
            .unwrap()
            .as_boolean()
            .unwrap()
    );
    drop(completed);
    reporter.flush().unwrap();
    let store = EventStore::open(database.path(), RetentionPolicy::default()).unwrap();
    assert_eq!(store.len().unwrap(), 1);
    drop(store);
    drop(reporter);
    assert_database_omits_page_secrets(database.path());
}

#[test]
fn off_mode_keeps_existing_error_result_and_does_not_create_a_database() {
    let database = TestDatabase::new();
    let config = ReporterConfig::from_values(None, None).unwrap();
    let reporter = Arc::new(
        ErrorReporter::new(
            &config,
            database.path().to_path_buf(),
            RetentionPolicy::default(),
        )
        .unwrap(),
    );
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(HTML).document()).unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    let reported_errors = runtime.execute_document_scripts(None);
    let mut control = JsRuntime::with_document(TreeBuilder::parse(HTML).document()).unwrap();
    let control_errors = control.execute_document_scripts(None);
    assert_eq!(reported_errors, control_errors);
    assert_eq!(reported_errors.len(), 1);
    assert!(runtime.eval("afterFailure").unwrap().as_boolean().unwrap());
    reporter.flush().unwrap();
    assert!(!database.path().exists());
}
