use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use super::{
    ErrorCategory, ErrorCode, ErrorSeverity, EventStore, ExecutionSurface, RawEvent,
    RetentionPolicy, StoreError,
};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

struct TemporaryDatabase(PathBuf);

impl TemporaryDatabase {
    fn new() -> Self {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "omoikane-error-report-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path.join("events.sqlite"))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        if let Some(parent) = self.0.parent() {
            fs::remove_dir_all(parent).unwrap();
        }
    }
}

fn event(code: &'static str, message: &str) -> super::SafeEvent {
    RawEvent::new(
        ErrorCategory::JavaScript,
        ErrorSeverity::Error,
        ErrorCode::new(code).unwrap(),
        ExecutionSurface::Headless,
        message,
        &[("operation", "execute")],
    )
    .sanitize()
}

#[test]
fn schema_upsert_and_reopen_preserve_safe_fields() {
    let temporary = TemporaryDatabase::new();
    let raw = "Authorization: Bearer TOKEN_SECRET /home/user/private.js?key=QUERY_SECRET";
    let safe = event("SCRIPT_FAILURE", raw);
    {
        let mut store = EventStore::open(temporary.path(), RetentionPolicy::default()).unwrap();
        assert_eq!(store.schema_version().unwrap(), 2);
        store.record_at(&safe, 1_000).unwrap();
        store.record_at(&safe, 2_000).unwrap();
        assert_eq!(store.len().unwrap(), 1);
        let row = store.get(safe.fingerprint()).unwrap().unwrap();
        assert_eq!(row.category, "javascript");
        assert_eq!(row.severity, "error");
        assert_eq!(row.error_code, "SCRIPT_FAILURE");
        assert_eq!(row.first_seen_ms, 1_000);
        assert_eq!(row.last_seen_ms, 2_000);
        assert_eq!(row.occurrences, 2);
        assert_eq!(row.submission_status, "pending");
        assert_eq!(row.issue_url, None);
        assert_eq!(row.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(row.commit.as_deref(), safe.commit());
        assert!(row.platform.contains('/'));
        assert_eq!(row.surface, "headless");
        for value in [row.message, row.context_json] {
            assert!(!value.contains("TOKEN_SECRET"));
            assert!(!value.contains("QUERY_SECRET"));
            assert!(!value.contains("/home/user"));
        }
    }
    let reopened = EventStore::open(temporary.path(), RetentionPolicy::default()).unwrap();
    assert_eq!(reopened.len().unwrap(), 1);
    assert_eq!(
        reopened
            .get(safe.fingerprint())
            .unwrap()
            .unwrap()
            .occurrences,
        2
    );
    let connection = rusqlite::Connection::open(temporary.path()).unwrap();
    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode, "wal");
    drop(connection);
    drop(reopened);
    for suffix in ["", "-wal", "-shm"] {
        let mut path = temporary.path().as_os_str().to_os_string();
        path.push(suffix);
        if let Ok(bytes) = fs::read(PathBuf::from(path)) {
            assert!(
                !bytes
                    .windows(b"TOKEN_SECRET".len())
                    .any(|slice| slice == b"TOKEN_SECRET")
            );
        }
    }
}

#[test]
fn rejects_unknown_schema_without_mutating_it() {
    let temporary = TemporaryDatabase::new();
    let connection = rusqlite::Connection::open(temporary.path()).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    drop(connection);
    assert!(matches!(
        EventStore::open(temporary.path(), RetentionPolicy::default()),
        Err(StoreError::Schema)
    ));
    let connection = rusqlite::Connection::open(temporary.path()).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 99);
}

#[test]
fn version_one_database_migrates_delivery_state_and_remains_usable() {
    let temporary = TemporaryDatabase::new();
    let connection = rusqlite::Connection::open(temporary.path()).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE error_reports (
                fingerprint TEXT PRIMARY KEY NOT NULL,
                category TEXT NOT NULL,
                severity TEXT NOT NULL,
                error_code TEXT NOT NULL,
                message TEXT NOT NULL,
                context_json TEXT NOT NULL,
                first_seen_ms INTEGER NOT NULL,
                last_seen_ms INTEGER NOT NULL,
                occurrences INTEGER NOT NULL,
                version TEXT NOT NULL,
                build_commit TEXT,
                platform TEXT NOT NULL,
                surface TEXT NOT NULL,
                submission_status TEXT NOT NULL DEFAULT 'pending',
                issue_url TEXT
            );
            INSERT INTO error_reports (
                fingerprint, category, severity, error_code, message, context_json,
                first_seen_ms, last_seen_ms, occurrences, version, build_commit, platform,
                surface, submission_status, issue_url
            ) VALUES (
                'legacy-row', 'javascript', 'error', 'OLD_FAILURE', 'Error details withheld',
                '{}', 1000, 1000, 1, '0.4.0', NULL, 'linux/aarch64', 'headless', 'pending', NULL
            );",
        )
        .unwrap();
    connection
        .pragma_update(None, "application_id", 0x4f4d_4f45_i64)
        .unwrap();
    connection.pragma_update(None, "user_version", 1).unwrap();
    drop(connection);

    let mut store = EventStore::open(temporary.path(), RetentionPolicy::default()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 2);
    assert_eq!(store.len().unwrap(), 1);
    assert_eq!(
        store.submission_state("legacy-row").unwrap().unwrap(),
        super::SubmissionState::default()
    );
    let safe = event("MIGRATION_TEST", "details");
    store.record_at(&safe, 1_000).unwrap();
    assert_eq!(
        store.submission_state(safe.fingerprint()).unwrap().unwrap(),
        super::SubmissionState::default()
    );
    drop(store);
    let reopened = EventStore::open(temporary.path(), RetentionPolicy::default()).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), 2);
    assert_eq!(reopened.len().unwrap(), 2);
}

#[test]
fn rejects_a_foreign_database_with_the_same_schema_version() {
    let temporary = TemporaryDatabase::new();
    let connection = rusqlite::Connection::open(temporary.path()).unwrap();
    connection.pragma_update(None, "user_version", 1).unwrap();
    connection
        .pragma_update(None, "application_id", 0x1234)
        .unwrap();
    drop(connection);
    assert!(matches!(
        EventStore::open(temporary.path(), RetentionPolicy::default()),
        Err(StoreError::Schema)
    ));
}

#[test]
fn rejects_an_unversioned_database_with_unrelated_tables() {
    let temporary = TemporaryDatabase::new();
    let connection = rusqlite::Connection::open(temporary.path()).unwrap();
    connection
        .execute_batch("CREATE TABLE unrelated (value TEXT)")
        .unwrap();
    drop(connection);
    assert!(matches!(
        EventStore::open(temporary.path(), RetentionPolicy::default()),
        Err(StoreError::Schema)
    ));
    let connection = rusqlite::Connection::open(temporary.path()).unwrap();
    let objects: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'unrelated'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(objects, 1);
}

#[test]
fn retention_removes_expired_and_prefers_submitted_rows() {
    let temporary = TemporaryDatabase::new();
    let policy = RetentionPolicy {
        max_rows: 2,
        max_age: Duration::from_secs(2),
        max_bytes: 64 * 1024 * 1024,
    };
    let mut store = EventStore::open(temporary.path(), policy).unwrap();
    let pending = event("PENDING_FAILURE", "raw-1");
    let submitted = event("SUBMITTED_FAILURE", "raw-2");
    let newest = event("NEW_FAILURE", "raw-3");
    store.record_at(&pending, 1_000).unwrap();
    store.record_at(&submitted, 2_000).unwrap();
    store
        .mark_submitted(
            submitted.fingerprint(),
            "https://github.com/example/repo/issues/1",
        )
        .unwrap();
    store.record_at(&newest, 3_000).unwrap();
    assert_eq!(store.len().unwrap(), 2);
    assert!(store.get(pending.fingerprint()).unwrap().is_some());
    assert!(store.get(submitted.fingerprint()).unwrap().is_none());
    assert!(store.get(newest.fingerprint()).unwrap().is_some());

    let later = event("LATER_FAILURE", "raw-4");
    store.record_at(&later, 5_100).unwrap();
    assert!(store.get(pending.fingerprint()).unwrap().is_none());
    assert!(store.get(newest.fingerprint()).unwrap().is_none());
    assert_eq!(store.len().unwrap(), 1);
}

#[test]
fn repeated_submitted_fingerprint_becomes_pending_with_issue_reference() {
    let temporary = TemporaryDatabase::new();
    let mut store = EventStore::open(temporary.path(), RetentionPolicy::default()).unwrap();
    let safe = event("REPEAT_FAILURE", "details");
    store.record_at(&safe, 1_000).unwrap();
    store
        .mark_submitted(
            safe.fingerprint(),
            "https://github.com/example/repo/issues/1",
        )
        .unwrap();
    store.record_at(&safe, 2_000).unwrap();
    let row = store.get(safe.fingerprint()).unwrap().unwrap();
    assert_eq!(row.occurrences, 2);
    assert_eq!(row.submission_status, "pending");
    assert_eq!(
        row.issue_url.as_deref(),
        Some("https://github.com/example/repo/issues/1")
    );
}

#[test]
fn submission_does_not_hide_an_occurrence_added_during_delivery() {
    let temporary = TemporaryDatabase::new();
    let mut store = EventStore::open(temporary.path(), RetentionPolicy::default()).unwrap();
    let safe = event("DELIVERY_RACE", "details");
    store.record_at(&safe, 1_000).unwrap();
    let snapshot = store.get(safe.fingerprint()).unwrap().unwrap();
    assert!(store.claim_submission(&snapshot, 1_500, 1_801_500).unwrap());
    store.record_at(&safe, 2_000).unwrap();
    assert!(
        !store
            .mark_submitted_snapshot(
                &snapshot,
                "https://github.com/owner/repo/issues/42",
                2_500,
                1_801_500
            )
            .unwrap()
    );
    store
        .release_submission_lease(safe.fingerprint(), 1_801_500)
        .unwrap();
    assert_eq!(
        store
            .get(safe.fingerprint())
            .unwrap()
            .unwrap()
            .submission_status,
        "pending"
    );
    let current = store.get(safe.fingerprint()).unwrap().unwrap();
    assert!(store.claim_submission(&current, 2_500, 1_802_500).unwrap());
    assert!(
        store
            .mark_submitted_snapshot(
                &current,
                "https://github.com/owner/repo/issues/42",
                2_500,
                1_802_500
            )
            .unwrap()
    );
    assert_eq!(
        store
            .get(safe.fingerprint())
            .unwrap()
            .unwrap()
            .submission_status,
        "submitted"
    );
}

#[test]
fn sqlite_lease_prevents_parallel_delivery_across_connections() {
    let temporary = TemporaryDatabase::new();
    let mut first = EventStore::open(temporary.path(), RetentionPolicy::default()).unwrap();
    let safe = event("LEASE_TEST", "details");
    first.record_at(&safe, 1_000).unwrap();
    let second = EventStore::open(temporary.path(), RetentionPolicy::default()).unwrap();
    let row = first.get(safe.fingerprint()).unwrap().unwrap();
    assert!(first.claim_submission(&row, 2_000, 102_000).unwrap());
    assert!(!second.claim_submission(&row, 2_000, 102_000).unwrap());
    first
        .release_submission_lease(safe.fingerprint(), 102_000)
        .unwrap();
    assert!(second.claim_submission(&row, 2_000, 102_000).unwrap());
    second
        .record_submission_failure(&row, 62_000, 102_000)
        .unwrap();
    assert!(!first.claim_submission(&row, 61_999, 162_000).unwrap());
    assert!(first.claim_submission(&row, 62_000, 162_000).unwrap());
}

#[test]
fn pending_preview_is_bounded_and_sorted_without_submitted_rows() {
    let temporary = TemporaryDatabase::new();
    let mut store = EventStore::open(temporary.path(), RetentionPolicy::default()).unwrap();
    let older = event("PREVIEW_OLDER", "details");
    let newer = event("PREVIEW_NEWER", "details");
    let submitted = event("PREVIEW_SUBMITTED", "details");
    store.record_at(&older, 1_000).unwrap();
    store.record_at(&newer, 2_000).unwrap();
    store.record_at(&submitted, 3_000).unwrap();
    store
        .mark_submitted(
            submitted.fingerprint(),
            "https://github.com/example/repo/issues/7",
        )
        .unwrap();

    assert!(store.list_pending(0).unwrap().is_empty());
    assert_eq!(
        store
            .list_pending(1)
            .unwrap()
            .iter()
            .map(|report| report.fingerprint.as_str())
            .collect::<Vec<_>>(),
        vec![newer.fingerprint()]
    );
    assert_eq!(
        store
            .list_pending(101)
            .unwrap()
            .iter()
            .map(|report| report.fingerprint.as_str())
            .collect::<Vec<_>>(),
        vec![newer.fingerprint(), older.fingerprint()]
    );
}

#[test]
fn byte_limit_includes_wal_and_shm() {
    let temporary = TemporaryDatabase::new();
    let policy = RetentionPolicy {
        max_rows: 1_000,
        max_age: Duration::from_secs(1_000),
        max_bytes: 80 * 1024,
    };
    let mut store = EventStore::open(temporary.path(), policy).unwrap();
    for index in 0..120 {
        let code: &'static str = Box::leak(format!("ERROR_{index}").into_boxed_str());
        let safe = event(code, "opaque details");
        store.record_at(&safe, index).unwrap();
        assert!(store.disk_usage_bytes().unwrap() <= policy.max_bytes);
    }
    assert!(store.len().unwrap() < 120, "byte limit must prune rows");
}
