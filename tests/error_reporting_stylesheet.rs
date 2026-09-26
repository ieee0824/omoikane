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
    js::JsRuntime,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDatabase(PathBuf);

impl TestDatabase {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "omoikane-css-report-{}-{sequence}",
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

    fn assert_report(&self, code: &'static str, operation: &'static str, message: &'static str) {
        let event = RawEvent::new(
            ErrorCategory::Css,
            ErrorSeverity::Error,
            ErrorCode::new(code).unwrap(),
            ExecutionSurface::Headless,
            message,
            &[("operation", operation), ("resource", "stylesheet")],
        )
        .sanitize();
        let store = EventStore::open(self.path(), RetentionPolicy::default()).unwrap();
        let report = store
            .get(event.fingerprint())
            .unwrap()
            .unwrap_or_else(|| panic!("missing report: {code}"));
        assert_eq!(report.category, "css");
        assert_eq!(report.message, message);
        assert!(report.occurrences >= 1);
    }

    fn assert_no_secrets(&self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.0.as_os_str().to_os_string();
            path.push(suffix);
            if let Ok(bytes) = fs::read(PathBuf::from(path)) {
                for secret in [b"BODY_SECRET_937".as_slice(), b"QUERY_SECRET_937"] {
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

const HTML: &str = r#"<html><head>
    <link rel="stylesheet" href="data:text/css;base64,%%%?token=QUERY_SECRET_937">
    <style>@layer; /* BODY_SECRET_937 */</style>
    <style>.probe { color: rgb(1, 2, 3); }</style>
    </head><body class="probe"></body></html>"#;

fn computed_color(runtime: &mut JsRuntime) -> String {
    runtime
        .eval("getComputedStyle(document.body).color")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn stylesheet_fetch_and_parse_failures_are_distinct_and_keep_valid_styles() {
    let database = TestDatabase::new();
    let reporter = database.reporter();
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(HTML).document()).unwrap();
    runtime.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
    let actual = computed_color(&mut runtime);
    let mut control = JsRuntime::with_document(TreeBuilder::parse(HTML).document()).unwrap();
    let expected = computed_color(&mut control);
    assert_eq!(actual, expected);
    assert_eq!(actual, "rgb(1, 2, 3)");
    reporter.flush().unwrap();

    let store = EventStore::open(database.path(), RetentionPolicy::default()).unwrap();
    assert_eq!(store.len().unwrap(), 2);
    drop(store);
    database.assert_report(
        "CSS_STYLESHEET_FETCH_FAILED",
        "fetch",
        "Resource load failed",
    );
    database.assert_report(
        "CSS_STYLESHEET_PARSE_FAILED",
        "parse",
        "Stylesheet parse failed",
    );
    database.assert_no_secrets();
}
