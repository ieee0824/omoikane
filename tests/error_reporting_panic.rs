use std::{
    fs,
    panic::{self, catch_unwind},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use omoikane::error_reporting::{
    ErrorCategory, ErrorCode, ErrorReporter, ErrorSeverity, EventStore, ExecutionSurface, RawEvent,
    ReporterConfig, RetentionPolicy, install_panic_reporter,
};

fn reporter(mode: &str, path: PathBuf) -> Arc<ErrorReporter> {
    let config = ReporterConfig::from_values(Some(mode), None).unwrap();
    Arc::new(ErrorReporter::new(&config, path, RetentionPolicy::default()).unwrap())
}

fn expected(kind: &'static str) -> omoikane::error_reporting::SafeEvent {
    RawEvent::new(
        ErrorCategory::Internal,
        ErrorSeverity::Critical,
        ErrorCode::new("UNEXPECTED_PANIC").unwrap(),
        ExecutionSurface::Headless,
        "Unexpected panic",
        &[
            ("operation", "execute"),
            ("failure_kind", "internal"),
            ("panic_origin", "tests"),
            ("panic_kind", kind),
        ],
    )
    .sanitize()
}

#[test]
fn panic_hook_is_single_best_effort_and_never_stores_payload_or_path() {
    let directory =
        std::env::temp_dir().join(format!("omoikane-panic-report-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let off_path = directory.join("off.sqlite");
    let database = directory.join("events.sqlite");

    let original_hook = panic::take_hook();
    let previous_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&previous_calls);
    panic::set_hook(Box::new(move |_| {
        calls.fetch_add(1, Ordering::Relaxed);
    }));

    let off = reporter("off", off_path.clone());
    assert!(!install_panic_reporter(&off, ExecutionSurface::Headless));
    assert!(catch_unwind(|| panic::panic_any("OFF_SECRET_943".to_string())).is_err());
    assert_eq!(previous_calls.load(Ordering::Relaxed), 1);
    assert!(!off_path.exists());

    let active = reporter("record-only", database.clone());
    assert!(install_panic_reporter(&active, ExecutionSurface::Headless));
    assert!(!install_panic_reporter(&active, ExecutionSurface::Headless));
    assert!(
        catch_unwind(|| panic::panic_any(
            "PAYLOAD_SECRET_943 /home/private/SECRET_PATH_943".to_string()
        ))
        .is_err()
    );
    assert!(catch_unwind(|| panic::panic_any(7_u8)).is_err());
    assert_eq!(previous_calls.load(Ordering::Relaxed), 3);
    active.flush().unwrap();

    let store = EventStore::open(&database, RetentionPolicy::default()).unwrap();
    assert_eq!(store.len().unwrap(), 2);
    for kind in ["string", "other"] {
        let event = expected(kind);
        let report = store.get(event.fingerprint()).unwrap().unwrap();
        assert_eq!(report.occurrences, 1);
        assert_eq!(report.message, "Unexpected panic");
        assert_eq!(report.context_json.contains("panic_origin"), true);
    }
    drop(store);
    drop(active);
    assert!(catch_unwind(|| panic::panic_any("AFTER_DROP_SECRET_943")).is_err());
    assert_eq!(previous_calls.load(Ordering::Relaxed), 4);
    let store = EventStore::open(&database, RetentionPolicy::default()).unwrap();
    assert_eq!(store.len().unwrap(), 2);
    drop(store);

    for suffix in ["", "-wal", "-shm"] {
        let mut path = database.as_os_str().to_os_string();
        path.push(suffix);
        if let Ok(contents) = fs::read(PathBuf::from(path)) {
            for secret in [
                b"PAYLOAD_SECRET_943".as_slice(),
                b"SECRET_PATH_943",
                b"/home/private",
                b"OFF_SECRET_943",
            ] {
                assert!(!contents.windows(secret.len()).any(|bytes| bytes == secret));
            }
        }
    }

    panic::set_hook(original_hook);
    drop(off);
    fs::remove_dir_all(directory).unwrap();
}
