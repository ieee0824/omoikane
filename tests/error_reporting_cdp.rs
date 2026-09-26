use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use omoikane::{
    cdp::CdpSession,
    error_reporting::{
        ErrorCategory, ErrorCode, ErrorReporter, ErrorSeverity, EventStore, ExecutionSurface,
        RawEvent, ReporterConfig, RetentionPolicy,
    },
};
use serde_json::json;

struct TestDatabase(PathBuf);

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

impl TestDatabase {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "omoikane-cdp-report-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        Self(directory.join("events.sqlite"))
    }

    fn reporter(&self, mode: &str) -> Arc<ErrorReporter> {
        let config = ReporterConfig::from_values(Some(mode), None).unwrap();
        Arc::new(ErrorReporter::new(&config, self.0.clone(), RetentionPolicy::default()).unwrap())
    }

    fn assert_event(&self, code: &'static str, operation: &'static str) {
        let expected = RawEvent::new(
            ErrorCategory::Cdp,
            ErrorSeverity::Error,
            ErrorCode::new(code).unwrap(),
            ExecutionSurface::Cdp,
            "CDP operation failed",
            &[("operation", operation)],
        )
        .sanitize();
        let store = EventStore::open(&self.0, RetentionPolicy::default()).unwrap();
        let report = store.get(expected.fingerprint()).unwrap().unwrap();
        assert_eq!(report.category, "cdp");
        assert_eq!(report.error_code, code);
        assert_eq!(report.surface, "cdp");
        assert_eq!(report.message, "CDP operation failed");
    }

    fn assert_no_secrets(&self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.0.as_os_str().to_os_string();
            path.push(suffix);
            if let Ok(contents) = fs::read(PathBuf::from(path)) {
                for secret in [
                    b"METHOD_SECRET_942".as_slice(),
                    b"BODY_SECRET_942",
                    b"QUERY_SECRET_942",
                    b"AUTH_SECRET_942",
                ] {
                    assert!(!contents.windows(secret.len()).any(|bytes| bytes == secret));
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
fn request_and_navigation_failures_keep_cdp_errors_without_storing_inputs() {
    let database = TestDatabase::new();
    let reporter = database.reporter("record-only");
    let mut session = CdpSession::new().unwrap();
    session.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Cdp);
    let mut control = CdpSession::new().unwrap();

    let method = "METHOD_SECRET_942.invalid";
    let params = json!({
        "body": "BODY_SECRET_942",
        "Authorization": "Bearer AUTH_SECRET_942",
    });
    let expected = control.dispatch(method, params.clone()).unwrap_err();
    assert_eq!(session.dispatch(method, params).unwrap_err(), expected);

    // A committed page replaces the runtime. Reporting must remain attached.
    let page = json!({"url": "data:text/html,<title>CDP</title>"});
    assert_eq!(
        session.dispatch("Page.navigate", page.clone()),
        control.dispatch("Page.navigate", page)
    );
    let bad_url = json!({"url": "http://127.0.0.1:0/?token=QUERY_SECRET_942"});
    let expected = control
        .dispatch("Page.navigate", bad_url.clone())
        .unwrap_err();
    assert_eq!(
        session.dispatch("Page.navigate", bad_url).unwrap_err(),
        expected
    );
    assert_eq!(
        session
            .dispatch(method, json!({"body": "BODY_SECRET_942"}))
            .unwrap_err(),
        control
            .dispatch(method, json!({"body": "BODY_SECRET_942"}))
            .unwrap_err(),
    );

    reporter.flush().unwrap();
    database.assert_event("CDP_REQUEST_FAILED", "execute");
    database.assert_event("CDP_NAVIGATION_FAILED", "navigate");
    database.assert_no_secrets();
}

#[test]
fn off_mode_creates_no_database_for_cdp_failures() {
    let database = TestDatabase::new();
    let reporter = database.reporter("off");
    let mut session = CdpSession::new().unwrap();
    session.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Cdp);
    session.dispatch("unknown", json!({})).unwrap_err();
    reporter.flush().unwrap();
    assert!(!database.0.exists());
}
