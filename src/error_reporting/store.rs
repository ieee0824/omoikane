//! SQLite storage for already sanitized general-error events.
//!
//! Expired rows are removed regardless of submission state. For count and byte
//! limits, the oldest submitted rows go first, then the oldest pending rows.
//! Pending rows may be dropped when necessary to keep a hard local bound;
//! delivery is best-effort, not guaranteed. The database/WAL/SHM size is
//! checked after each write and checkpointed before deleting for byte limits.

use std::{
    fmt, fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::SafeEvent;

const SCHEMA_VERSION: i64 = 2;
const APPLICATION_ID: i64 = 0x4f4d_4f45;

/// Hard limits for the general-error database, including its WAL and SHM files.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetentionPolicy {
    /// Maximum number of distinct fingerprints retained.
    pub max_rows: usize,
    /// Maximum age since the last occurrence, including unsent events.
    pub max_age: Duration,
    /// Maximum combined size of the database, WAL, and SHM files.
    pub max_bytes: u64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            max_rows: 10_000,
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_bytes: 64 * 1024 * 1024,
        }
    }
}

impl RetentionPolicy {
    /// Validates nonzero limits. A byte cap below the empty schema size will
    /// cause writes to fail closed rather than exceed the requested limit.
    pub fn validate(self) -> Result<Self, StoreError> {
        if self.max_rows == 0 || self.max_age.is_zero() || self.max_bytes == 0 {
            return Err(StoreError::InvalidPolicy);
        }
        Ok(self)
    }
}

/// A stored, already sanitized report row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredReport {
    /// Versioned fingerprint used as the primary key.
    pub fingerprint: String,
    /// Error category.
    pub category: String,
    /// Error severity.
    pub severity: String,
    /// Stable source-defined code.
    pub error_code: String,
    /// Fixed sanitized message template.
    pub message: String,
    /// JSON of allowlisted context only.
    pub context_json: String,
    /// First occurrence, milliseconds since the Unix epoch.
    pub first_seen_ms: i64,
    /// Latest occurrence, milliseconds since the Unix epoch.
    pub last_seen_ms: i64,
    /// Number of occurrences accumulated for this fingerprint.
    pub occurrences: i64,
    /// Build version.
    pub version: String,
    /// Validated build commit, when available.
    pub commit: Option<String>,
    /// Build target platform.
    pub platform: String,
    /// Execution surface.
    pub surface: String,
    /// `pending` or `submitted`.
    pub submission_status: String,
    /// GitHub Issue URL after a successful submission, if any.
    pub issue_url: Option<String>,
}

/// Persistent scheduling state for one fingerprint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SubmissionState {
    /// Consecutive delivery failures.
    pub failures: u32,
    /// Earliest next attempt, milliseconds since the Unix epoch.
    pub next_retry_ms: i64,
    /// Last successful delivery, milliseconds since the Unix epoch.
    pub last_submitted_ms: Option<i64>,
    /// Expiration of the current in-flight delivery lease.
    pub lease_until_ms: i64,
}

/// A sanitized storage error; never contains a path, SQL text, or event input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreError {
    /// Invalid retention limits were supplied.
    InvalidPolicy,
    /// SQLite or filesystem I/O failed.
    Database,
    /// The existing database has an unsupported schema version or identity.
    Schema,
    /// Even after pruning, the configured byte cap could not hold the schema.
    Capacity,
    /// Serialization of sanitized context failed.
    Serialization,
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidPolicy => "invalid error-report retention policy",
            Self::Database => "error-report database operation failed",
            Self::Schema => "unsupported error-report database schema",
            Self::Capacity => "error-report database capacity exceeded",
            Self::Serialization => "error-report context serialization failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Database
    }
}

/// SQLite-backed store for [`SafeEvent`] values only.
///
/// Opening a store creates the database. Callers must first check that the
/// reporting mode is enabled; the off-mode reporter never constructs a store.
pub struct EventStore {
    connection: Connection,
    path: PathBuf,
    retention: RetentionPolicy,
}

impl EventStore {
    /// Opens or creates a versioned database with WAL and a bounded busy wait.
    pub fn open(path: impl AsRef<Path>, retention: RetentionPolicy) -> Result<Self, StoreError> {
        let retention = retention.validate()?;
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|_| StoreError::Database)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_millis(500))?;
        let application_id: i64 =
            connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if !matches!((application_id, version), (0, 0) | (APPLICATION_ID, 1 | 2)) {
            return Err(StoreError::Schema);
        }
        if version == 0 {
            let has_existing_objects: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name NOT LIKE 'sqlite_%')",
                [],
                |row| row.get(0),
            )?;
            if has_existing_objects {
                return Err(StoreError::Schema);
            }
        }
        if version == 0 {
            connection.pragma_update(None, "auto_vacuum", "FULL")?;
        }
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS error_reports (
                fingerprint TEXT PRIMARY KEY NOT NULL,
                category TEXT NOT NULL,
                severity TEXT NOT NULL,
                error_code TEXT NOT NULL,
                message TEXT NOT NULL,
                context_json TEXT NOT NULL,
                first_seen_ms INTEGER NOT NULL,
                last_seen_ms INTEGER NOT NULL,
                occurrences INTEGER NOT NULL CHECK (occurrences > 0),
                version TEXT NOT NULL,
                build_commit TEXT,
                platform TEXT NOT NULL,
                surface TEXT NOT NULL,
                submission_status TEXT NOT NULL DEFAULT 'pending'
                    CHECK (submission_status IN ('pending', 'submitted')),
                issue_url TEXT,
                delivery_failures INTEGER NOT NULL DEFAULT 0,
                next_retry_ms INTEGER NOT NULL DEFAULT 0,
                last_submitted_ms INTEGER,
                delivery_lease_until_ms INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS error_reports_retention_idx
                ON error_reports (submission_status, last_seen_ms, fingerprint);",
        )?;
        if version == 1 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE error_reports ADD COLUMN delivery_failures INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE error_reports ADD COLUMN next_retry_ms INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE error_reports ADD COLUMN last_submitted_ms INTEGER;
                 ALTER TABLE error_reports ADD COLUMN delivery_lease_until_ms INTEGER NOT NULL DEFAULT 0;
                 PRAGMA user_version = 2;
                 COMMIT;",
            )?;
        }
        if version == 0 {
            connection.pragma_update(None, "application_id", APPLICATION_ID)?;
        }
        if version == 0 {
            connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(Self {
            connection,
            path: path.to_path_buf(),
            retention,
        })
    }

    /// Inserts or updates a sanitized event and enforces all retention limits.
    pub fn record(&mut self, event: &SafeEvent) -> Result<(), StoreError> {
        self.record_at(event, now_ms())
    }

    /// Inserts an event at an explicit timestamp for deterministic callers and tests.
    pub fn record_at(&mut self, event: &SafeEvent, timestamp_ms: i64) -> Result<(), StoreError> {
        let context_json =
            serde_json::to_string(event.context()).map_err(|_| StoreError::Serialization)?;
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO error_reports (
                fingerprint, category, severity, error_code, message, context_json,
                first_seen_ms, last_seen_ms, occurrences, version, build_commit, platform, surface,
                submission_status, issue_url
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 1, ?8, ?9, ?10, ?11, 'pending', NULL)
            ON CONFLICT(fingerprint) DO UPDATE SET
                severity = excluded.severity,
                message = excluded.message,
                context_json = excluded.context_json,
                first_seen_ms = MIN(error_reports.first_seen_ms, excluded.first_seen_ms),
                last_seen_ms = MAX(error_reports.last_seen_ms, excluded.last_seen_ms),
                occurrences = CASE
                    WHEN error_reports.occurrences < 9223372036854775807
                    THEN error_reports.occurrences + 1
                    ELSE error_reports.occurrences END,
                version = excluded.version,
                build_commit = excluded.build_commit,
                platform = excluded.platform,
                surface = excluded.surface,
                submission_status = 'pending'",
            params![
                event.fingerprint(),
                event.category().as_str(),
                event.severity().as_str(),
                event.code().as_str(),
                event.message(),
                context_json,
                timestamp_ms,
                event.version(),
                event.commit(),
                event.platform(),
                event.surface().as_str(),
            ],
        )?;
        let age_ms = i64::try_from(self.retention.max_age.as_millis()).unwrap_or(i64::MAX);
        let cutoff = timestamp_ms.saturating_sub(age_ms);
        transaction.execute(
            "DELETE FROM error_reports WHERE last_seen_ms < ?1",
            [cutoff],
        )?;
        let row_count: i64 =
            transaction.query_row("SELECT COUNT(*) FROM error_reports", [], |row| row.get(0))?;
        let excess =
            row_count.saturating_sub(i64::try_from(self.retention.max_rows).unwrap_or(i64::MAX));
        if excess > 0 {
            transaction.execute(
                "DELETE FROM error_reports WHERE fingerprint IN (
                    SELECT fingerprint FROM error_reports
                    ORDER BY CASE submission_status WHEN 'submitted' THEN 0 ELSE 1 END,
                             last_seen_ms ASC, fingerprint ASC
                    LIMIT ?1
                )",
                [excess],
            )?;
        }
        transaction.commit()?;
        self.enforce_byte_limit()
    }

    /// Returns one report by fingerprint, for preview and submission stages.
    pub fn get(&self, fingerprint: &str) -> Result<Option<StoredReport>, StoreError> {
        self.connection
            .query_row(
                "SELECT fingerprint, category, severity, error_code, message, context_json,
                        first_seen_ms, last_seen_ms, occurrences, version, build_commit, platform, surface,
                        submission_status, issue_url
                 FROM error_reports WHERE fingerprint = ?1",
                [fingerprint],
                stored_report_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Returns persistent delivery scheduling state for a fingerprint.
    pub fn submission_state(
        &self,
        fingerprint: &str,
    ) -> Result<Option<SubmissionState>, StoreError> {
        self.connection
            .query_row(
                "SELECT delivery_failures, next_retry_ms, last_submitted_ms, delivery_lease_until_ms
                 FROM error_reports WHERE fingerprint = ?1",
                [fingerprint],
                |row| {
                    let failures: i64 = row.get(0)?;
                    Ok(SubmissionState {
                        failures: u32::try_from(failures).unwrap_or(u32::MAX),
                        next_retry_ms: row.get(1)?,
                        last_submitted_ms: row.get(2)?,
                        lease_until_ms: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Clears a pending row's retry block after an operator fixes credentials or service state.
    pub fn reset_submission_retry(&self, fingerprint: &str) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE error_reports SET delivery_failures = 0, next_retry_ms = 0
             WHERE fingerprint = ?1 AND submission_status = 'pending'",
            [fingerprint],
        )?;
        Ok(changed == 1)
    }

    /// Atomically leases a pending snapshot before any backend request.
    pub(super) fn claim_submission(
        &self,
        report: &StoredReport,
        now_ms: i64,
        lease_until_ms: i64,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE error_reports SET delivery_lease_until_ms = ?2
             WHERE fingerprint = ?1 AND submission_status = 'pending'
               AND occurrences = ?3 AND last_seen_ms = ?4
               AND delivery_lease_until_ms <= ?5 AND next_retry_ms <= ?5",
            params![
                report.fingerprint,
                lease_until_ms,
                report.occurrences,
                report.last_seen_ms,
                now_ms,
            ],
        )?;
        Ok(changed == 1)
    }

    /// Releases only the lease acquired by the caller.
    pub(super) fn release_submission_lease(
        &self,
        fingerprint: &str,
        lease_until_ms: i64,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE error_reports SET delivery_lease_until_ms = 0
             WHERE fingerprint = ?1 AND delivery_lease_until_ms = ?2",
            params![fingerprint, lease_until_ms],
        )?;
        Ok(())
    }

    /// Lists at most 100 unsent reports, newest first, for explicit preview.
    pub fn list_pending(&self, limit: usize) -> Result<Vec<StoredReport>, StoreError> {
        let limit = i64::try_from(limit.min(100)).unwrap_or(100);
        let mut statement = self.connection.prepare(
            "SELECT fingerprint, category, severity, error_code, message, context_json,
                    first_seen_ms, last_seen_ms, occurrences, version, build_commit, platform, surface,
                    submission_status, issue_url
             FROM error_reports WHERE submission_status = 'pending'
             ORDER BY last_seen_ms DESC, fingerprint ASC LIMIT ?1",
        )?;
        statement
            .query_map([limit], stored_report_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Returns the number of retained fingerprints.
    pub fn len(&self) -> Result<usize, StoreError> {
        let count: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM error_reports", [], |row| row.get(0))?;
        usize::try_from(count).map_err(|_| StoreError::Database)
    }

    /// Returns whether the store currently contains no reports.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }

    /// Returns the current database schema version.
    pub fn schema_version(&self) -> Result<i64, StoreError> {
        self.connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(Into::into)
    }

    /// Returns combined database, WAL, and SHM bytes on disk.
    pub fn disk_usage_bytes(&self) -> Result<u64, StoreError> {
        let mut total = 0u64;
        for path in [
            self.path.clone(),
            suffix_path(&self.path, "-wal"),
            suffix_path(&self.path, "-shm"),
        ] {
            match fs::metadata(path) {
                Ok(metadata) => total = total.saturating_add(metadata.len()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(StoreError::Database),
            }
        }
        Ok(total)
    }

    fn enforce_byte_limit(&mut self) -> Result<(), StoreError> {
        if self.disk_usage_bytes()? <= self.retention.max_bytes {
            return Ok(());
        }
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        while self.disk_usage_bytes()? > self.retention.max_bytes {
            let changed = self.connection.execute(
                "DELETE FROM error_reports WHERE fingerprint = (
                    SELECT fingerprint FROM error_reports
                    ORDER BY CASE submission_status WHEN 'submitted' THEN 0 ELSE 1 END,
                             last_seen_ms ASC, fingerprint ASC LIMIT 1
                )",
                [],
            )?;
            if changed == 0 {
                return Err(StoreError::Capacity);
            }
            self.connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        }
        Ok(())
    }

    /// Marks an event as submitted after a backend returns a canonical Issue URL.
    #[cfg(test)]
    pub(super) fn mark_submitted(
        &self,
        fingerprint: &str,
        issue_url: &str,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE error_reports SET submission_status = 'submitted', issue_url = ?2
             WHERE fingerprint = ?1",
            params![fingerprint, issue_url],
        )?;
        Ok(())
    }

    /// Records delivery only if the submitted snapshot is still current.
    /// A concurrent new occurrence remains pending for a later update.
    pub(super) fn mark_submitted_snapshot(
        &self,
        report: &StoredReport,
        issue_url: &str,
        now_ms: i64,
        lease_until_ms: i64,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE error_reports SET submission_status = 'submitted', issue_url = ?2,
                delivery_failures = 0, next_retry_ms = 0, last_submitted_ms = ?5,
                delivery_lease_until_ms = 0
             WHERE fingerprint = ?1 AND submission_status = 'pending'
               AND occurrences = ?3 AND last_seen_ms = ?4
               AND delivery_lease_until_ms = ?6",
            params![
                report.fingerprint,
                issue_url,
                report.occurrences,
                report.last_seen_ms,
                now_ms,
                lease_until_ms,
            ],
        )?;
        Ok(changed == 1)
    }

    /// Persists a bounded retry time even if another occurrence arrived mid-send.
    pub(super) fn record_submission_failure(
        &self,
        report: &StoredReport,
        next_retry_ms: i64,
        lease_until_ms: i64,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE error_reports SET
                delivery_failures = MIN(delivery_failures + 1, 31),
                next_retry_ms = MAX(next_retry_ms, ?2),
                delivery_lease_until_ms = 0
             WHERE fingerprint = ?1 AND submission_status = 'pending'
               AND delivery_lease_until_ms = ?3",
            params![report.fingerprint, next_retry_ms, lease_until_ms],
        )?;
        Ok(())
    }
}

fn stored_report_from_row(row: &Row<'_>) -> rusqlite::Result<StoredReport> {
    Ok(StoredReport {
        fingerprint: row.get(0)?,
        category: row.get(1)?,
        severity: row.get(2)?,
        error_code: row.get(3)?,
        message: row.get(4)?,
        context_json: row.get(5)?,
        first_seen_ms: row.get(6)?,
        last_seen_ms: row.get(7)?,
        occurrences: row.get(8)?,
        version: row.get(9)?,
        commit: row.get(10)?,
        platform: row.get(11)?,
        surface: row.get(12)?,
        submission_status: row.get(13)?,
        issue_url: row.get(14)?,
    })
}

fn suffix_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn now_ms() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}
