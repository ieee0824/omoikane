//! Explicitly enabled unattended delivery using the manual submission policy.

use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    EventStore, ManualSubmission, ManualSubmissionError, ReporterConfig, SubmissionApproval,
    SubmissionBackend,
};

/// Outcome of one bounded automatic delivery scan.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AutoSubmissionStats {
    /// Backend calls made in this scan.
    pub attempted: usize,
    /// Reports successfully submitted in this scan.
    pub submitted: usize,
    /// Reports left pending by cooldown, retry, lease, or exhaustion.
    pub deferred: usize,
    /// Backend or storage failures; later rows are not sent in this scan.
    pub failed: usize,
}

/// A bounded automatic sender, constructed only with the dedicated opt-in.
pub struct AutoSubmission<'a> {
    manual: ManualSubmission<'a>,
}

impl<'a> AutoSubmission<'a> {
    /// Rejects default, off, and record-only settings without opening a network client.
    pub fn new(
        config: &'a ReporterConfig,
        store: &'a EventStore,
    ) -> Result<Self, ManualSubmissionError> {
        if !config.auto_submit_enabled() {
            return Err(ManualSubmissionError::Disabled);
        }
        Ok(Self {
            manual: ManualSubmission::new(config, store),
        })
    }

    /// Scans pending rows using the same payload, cooldown, and retry path as manual delivery.
    pub fn run_once(
        &self,
        backend: &mut impl SubmissionBackend,
    ) -> Result<AutoSubmissionStats, ManualSubmissionError> {
        self.run_once_at(backend, now_ms())
    }

    pub(crate) fn run_once_at(
        &self,
        backend: &mut impl SubmissionBackend,
        now_ms: i64,
    ) -> Result<AutoSubmissionStats, ManualSubmissionError> {
        let before = self.manual.attempted_count();
        let mut result = AutoSubmissionStats::default();
        for report in self.manual.list_pending(100)? {
            match self.manual.submit_selected_at(
                &report.fingerprint,
                SubmissionApproval::UnattendedOptIn,
                backend,
                now_ms,
            ) {
                Ok(_) => result.submitted += 1,
                Err(ManualSubmissionError::RunLimit) => break,
                Err(
                    ManualSubmissionError::Cooldown { .. }
                    | ManualSubmissionError::RetryLater { .. }
                    | ManualSubmissionError::RetryExhausted
                    | ManualSubmissionError::Busy
                    | ManualSubmissionError::NotPending,
                ) => result.deferred += 1,
                Err(ManualSubmissionError::Backend(_) | ManualSubmissionError::Store(_)) => {
                    result.failed += 1;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        result.attempted = self.manual.attempted_count().saturating_sub(before);
        Ok(result)
    }
}

fn now_ms() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;
    use crate::error_reporting::{
        ErrorCategory, ErrorCode, ErrorSeverity, ExecutionSurface, RawEvent, Repository,
        RetentionPolicy, StoredReport, SubmissionFailure,
    };

    static NEXT_DB: AtomicU64 = AtomicU64::new(0);

    struct Backend {
        calls: usize,
        fail: bool,
    }

    impl SubmissionBackend for Backend {
        fn submit(&mut self, _: &Repository, _: &StoredReport) -> Result<u64, SubmissionFailure> {
            self.calls += 1;
            if self.fail {
                Err(SubmissionFailure::Transport)
            } else {
                Ok(42)
            }
        }
    }

    #[test]
    fn unattended_delivery_requires_opt_in_and_uses_manual_retry_and_cooldown() {
        let path = std::env::temp_dir().join(format!(
            "omoikane-auto-report-{}-{}",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let mut store =
            EventStore::open(path.join("events.sqlite"), RetentionPolicy::default()).unwrap();
        let safe = RawEvent::new(
            ErrorCategory::JavaScript,
            ErrorSeverity::Error,
            ErrorCode::new("AUTO_TEST").unwrap(),
            ExecutionSurface::Headless,
            "JavaScript execution failed",
            &[],
        )
        .sanitize();
        store.record_at(&safe, 1_000).unwrap();
        let mut backend = Backend {
            calls: 0,
            fail: false,
        };
        let off = ReporterConfig::default();
        let record_only =
            ReporterConfig::from_values(Some("record-only"), Some("owner/repo")).unwrap();
        let manual_submit =
            ReporterConfig::from_values(Some("submit"), Some("owner/repo")).unwrap();
        for config in [&off, &record_only, &manual_submit] {
            assert!(matches!(
                AutoSubmission::new(config, &store),
                Err(ManualSubmissionError::Disabled)
            ));
        }
        assert_eq!(backend.calls, 0);

        let config =
            ReporterConfig::from_values_with_auto_submit(Some("submit"), Some("owner/repo"), true)
                .unwrap();
        let sender = AutoSubmission::new(&config, &store).unwrap();
        assert_eq!(
            sender.run_once_at(&mut backend, 1_000).unwrap(),
            AutoSubmissionStats {
                attempted: 1,
                submitted: 1,
                deferred: 0,
                failed: 0
            }
        );
        assert_eq!(
            sender.run_once_at(&mut backend, 2_000).unwrap(),
            AutoSubmissionStats::default()
        );
        assert_eq!(backend.calls, 1);
        drop(sender);
        store.record_at(&safe, 3_000).unwrap();
        let sender = AutoSubmission::new(&config, &store).unwrap();
        assert_eq!(sender.run_once_at(&mut backend, 2_000).unwrap().deferred, 1);
        assert_eq!(backend.calls, 1);
        assert_eq!(
            sender
                .run_once_at(&mut backend, 3_601_000)
                .unwrap()
                .submitted,
            1
        );
        assert_eq!(backend.calls, 2);
        drop(sender);
        drop(store);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn backend_failure_stops_the_scan_and_keeps_reports_pending() {
        let path = std::env::temp_dir().join(format!(
            "omoikane-auto-failure-{}-{}",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let mut store =
            EventStore::open(path.join("events.sqlite"), RetentionPolicy::default()).unwrap();
        for code in ["AUTO_ONE", "AUTO_TWO"] {
            let safe = RawEvent::new(
                ErrorCategory::JavaScript,
                ErrorSeverity::Error,
                ErrorCode::new(code).unwrap(),
                ExecutionSurface::Headless,
                "JavaScript execution failed",
                &[],
            )
            .sanitize();
            store.record_at(&safe, 1_000).unwrap();
        }
        let config =
            ReporterConfig::from_values_with_auto_submit(Some("submit"), Some("owner/repo"), true)
                .unwrap();
        let sender = AutoSubmission::new(&config, &store).unwrap();
        let mut backend = Backend {
            calls: 0,
            fail: true,
        };
        assert_eq!(
            sender.run_once_at(&mut backend, 2_000).unwrap(),
            AutoSubmissionStats {
                attempted: 1,
                submitted: 0,
                deferred: 0,
                failed: 1
            }
        );
        assert_eq!(backend.calls, 1);
        assert_eq!(store.list_pending(10).unwrap().len(), 2);
        drop(sender);
        drop(store);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn automatic_delivery_stops_after_the_per_run_limit() {
        let path = std::env::temp_dir().join(format!(
            "omoikane-auto-limit-{}-{}",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let mut store =
            EventStore::open(path.join("events.sqlite"), RetentionPolicy::default()).unwrap();
        for (index, code) in [
            "AUTO_LIMIT_0",
            "AUTO_LIMIT_1",
            "AUTO_LIMIT_2",
            "AUTO_LIMIT_3",
            "AUTO_LIMIT_4",
            "AUTO_LIMIT_5",
            "AUTO_LIMIT_6",
            "AUTO_LIMIT_7",
            "AUTO_LIMIT_8",
            "AUTO_LIMIT_9",
            "AUTO_LIMIT_10",
        ]
        .into_iter()
        .enumerate()
        {
            let safe = RawEvent::new(
                ErrorCategory::JavaScript,
                ErrorSeverity::Error,
                ErrorCode::new(code).unwrap(),
                ExecutionSurface::Headless,
                "JavaScript execution failed",
                &[],
            )
            .sanitize();
            store.record_at(&safe, 1_000 + index as i64).unwrap();
        }
        let config =
            ReporterConfig::from_values_with_auto_submit(Some("submit"), Some("owner/repo"), true)
                .unwrap();
        let sender = AutoSubmission::new(&config, &store).unwrap();
        let mut backend = Backend {
            calls: 0,
            fail: false,
        };
        let result = sender.run_once_at(&mut backend, 2_000).unwrap();
        assert_eq!(result.attempted, 10);
        assert_eq!(result.submitted, 10);
        assert_eq!(backend.calls, 10);
        assert_eq!(store.list_pending(100).unwrap().len(), 1);
        assert_eq!(
            sender.run_once_at(&mut backend, 3_000).unwrap().attempted,
            0
        );
        assert_eq!(backend.calls, 10);
        drop(sender);
        drop(store);
        fs::remove_dir_all(path).unwrap();
    }
}
