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
        RawEvent, ReporterConfig, RetentionPolicy, StoredReport,
    },
    js::JsRuntime,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDatabase(PathBuf);

impl TestDatabase {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "omoikane-task-error-report-{}-{sequence}",
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

    fn row(&self, code: &'static str, task_kind: &'static str) -> StoredReport {
        let expected = RawEvent::new(
            ErrorCategory::JavaScript,
            ErrorSeverity::Error,
            ErrorCode::new(code).unwrap(),
            ExecutionSurface::Headless,
            "JavaScript execution failed",
            &[
                ("operation", "execute"),
                ("resource", "script"),
                ("task_kind", task_kind),
            ],
        )
        .sanitize();
        EventStore::open(self.path(), RetentionPolicy::default())
            .unwrap()
            .get(expected.fingerprint())
            .unwrap()
            .unwrap()
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
    }
}

#[test]
fn timer_failures_aggregate_without_changing_the_bounded_task_buffer() {
    let database = TestDatabase::new();
    let reporter = database.reporter();
    let mut runtime = JsRuntime::new().unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    runtime
        .eval(
            "for (let i = 0; i < 40; i++) setTimeout(() => { throw new Error('TIMER_SECRET_935'); }, 0);",
        )
        .unwrap();
    runtime.tick(1).unwrap();
    let errors = runtime.take_task_errors();
    assert_eq!(errors.len(), 33);
    assert_eq!(errors[32], "8 further task errors suppressed");
    assert!(errors[0].contains("TIMER_SECRET_935"));
    reporter.flush().unwrap();

    let store = EventStore::open(database.path(), RetentionPolicy::default()).unwrap();
    assert_eq!(store.len().unwrap(), 1);
    drop(store);
    let row = database.row("JS_TIMER_CALLBACK_FAILED", "timer");
    assert_eq!(row.category, "javascript");
    assert_eq!(row.occurrences, 40);
    assert!(row.context_json.contains("task_kind"));
    assert!(row.context_json.contains("timer"));
    assert!(!row.message.contains("TIMER_SECRET_935"));
}

#[test]
fn event_listener_errors_are_recorded_without_changing_dispatch_result() {
    let database = TestDatabase::new();
    let reporter = database.reporter();
    let mut runtime = JsRuntime::new().unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    runtime
        .eval(
            "const node = document.createElement('div'); \
             node.addEventListener('probe', () => { throw new Error('LISTENER_SECRET_935'); }); \
             globalThis.dispatchResults = [node.dispatchEvent(new Event('probe')), node.dispatchEvent(new Event('probe'))];",
        )
        .unwrap();
    assert_eq!(
        runtime
            .eval("dispatchResults.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "true,true"
    );
    assert!(runtime.take_task_errors().is_empty());
    reporter.flush().unwrap();

    let store = EventStore::open(database.path(), RetentionPolicy::default()).unwrap();
    assert_eq!(store.len().unwrap(), 1);
    drop(store);
    let row = database.row("JS_EVENT_LISTENER_FAILED", "event-listener");
    assert_eq!(row.occurrences, 2);
    assert!(row.context_json.contains("event-listener"));
    assert!(!row.message.contains("LISTENER_SECRET_935"));
}

#[test]
fn animation_frame_errors_aggregate_and_keep_the_existing_returned_error() {
    let database = TestDatabase::new();
    let reporter = database.reporter();
    let mut runtime = JsRuntime::new().unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    runtime
        .eval(
            "for (let i = 0; i < 2; i++) requestAnimationFrame(() => { throw new Error('FRAME_SECRET_935'); });",
        )
        .unwrap();
    let error = runtime.run_animation_frame(16).unwrap_err();
    assert!(error.to_string().contains("FRAME_SECRET_935"));
    reporter.flush().unwrap();

    let store = EventStore::open(database.path(), RetentionPolicy::default()).unwrap();
    assert_eq!(store.len().unwrap(), 1);
    drop(store);
    let row = database.row("JS_ANIMATION_FRAME_CALLBACK_FAILED", "animation-frame");
    assert_eq!(row.occurrences, 2);
    assert!(row.context_json.contains("animation-frame"));
    assert!(!row.message.contains("FRAME_SECRET_935"));
}
