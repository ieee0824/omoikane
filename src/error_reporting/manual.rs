//! Explicit, local preview and submission boundary for sanitized reports.

use std::{
    cell::Cell,
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::{EventStore, ReporterConfig, Repository, StoreError, StoredReport};

const DELIVERY_LEASE: Duration = Duration::from_secs(30 * 60);

/// Approval provided after reviewing a sanitized report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmissionApproval {
    /// No approval has been given; a request must not leave this process.
    Unconfirmed,
    /// A person explicitly confirmed this one submission.
    Confirmed,
    /// An unattended caller explicitly opted in to this one submission.
    UnattendedOptIn,
}

/// Failure category used to choose a persistent retry delay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmissionFailure {
    /// The network or TLS exchange failed.
    Transport,
    /// Credentials or repository permissions were rejected.
    Denied,
    /// GitHub reported a rate limit.
    RateLimited { retry_at_ms: Option<i64> },
    /// The server response was unexpected.
    InvalidResponse,
    /// The open-Issue scan could not finish safely.
    SearchIncomplete,
}

/// Bounded delivery policy for one process run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubmissionPolicy {
    /// Maximum backend calls made by one [`ManualSubmission`] instance.
    pub max_per_run: usize,
    /// Maximum consecutive failures before explicit retry reset is required.
    pub max_failures: u32,
    /// Minimum interval between successful submissions of one fingerprint.
    pub cooldown: Duration,
    /// Initial retry delay for transport errors.
    pub initial_backoff: Duration,
    /// Maximum retry delay for transport errors.
    pub max_backoff: Duration,
}

impl Default for SubmissionPolicy {
    fn default() -> Self {
        Self {
            max_per_run: 10,
            max_failures: 8,
            cooldown: Duration::from_secs(60 * 60),
            initial_backoff: Duration::from_secs(60),
            max_backoff: Duration::from_secs(24 * 60 * 60),
        }
    }
}

impl SubmissionPolicy {
    /// Rejects zero limits and a backoff maximum below the initial delay.
    pub fn validate(self) -> Result<Self, ManualSubmissionError> {
        if self.max_per_run == 0
            || self.max_failures == 0
            || self.cooldown.is_zero()
            || self.initial_backoff.is_zero()
            || self.max_backoff < self.initial_backoff
        {
            return Err(ManualSubmissionError::InvalidPolicy);
        }
        Ok(self)
    }
}

/// Network-capable backend used only after configuration and approval checks.
///
/// The backend must return the canonical GitHub Issue number. It must not log
/// credentials, response bodies, or an unsanitized event on failure.
pub trait SubmissionBackend {
    /// Requests submission to the explicitly configured repository.
    fn submit(
        &mut self,
        repository: &Repository,
        report: &StoredReport,
    ) -> Result<u64, SubmissionFailure>;
}

/// A failure without credentials, report contents, or backend response details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManualSubmissionError {
    /// The configured delivery limits are invalid.
    InvalidPolicy,
    /// The current mode does not permit network submission.
    Disabled,
    /// This request was not explicitly approved.
    ConfirmationRequired,
    /// This process run has used its submission allowance.
    RunLimit,
    /// Another process is already delivering this fingerprint.
    Busy,
    /// Consecutive failures reached the persistent retry limit.
    RetryExhausted,
    /// A successful recent delivery is still inside its cooldown.
    Cooldown { retry_at_ms: i64 },
    /// A previous failure must wait until its persisted retry time.
    RetryLater { retry_at_ms: i64 },
    /// The selected report is not pending in the local store.
    NotPending,
    /// The submission backend failed or returned an invalid Issue number.
    Backend(SubmissionFailure),
    /// Local storage failed.
    Store(StoreError),
}

impl fmt::Display for ManualSubmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidPolicy => "invalid error-report delivery policy",
            Self::Disabled => "error-report submission is disabled",
            Self::ConfirmationRequired => "error-report submission requires explicit approval",
            Self::RunLimit => "error-report submission run limit reached",
            Self::Busy => "error report is already being submitted",
            Self::RetryExhausted => "error-report submission retry limit reached",
            Self::Cooldown { .. } => "error-report submission is cooling down",
            Self::RetryLater { .. } => "error-report submission retry is deferred",
            Self::NotPending => "error report is not pending",
            Self::Backend(_) => "error-report submission failed",
            Self::Store(_) => "error-report storage failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ManualSubmissionError {}

impl From<StoreError> for ManualSubmissionError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// A manual entry point that never submits while listing or previewing rows.
pub struct ManualSubmission<'a> {
    config: &'a ReporterConfig,
    store: &'a EventStore,
    policy: SubmissionPolicy,
    attempted: Cell<usize>,
}

impl<'a> ManualSubmission<'a> {
    /// Binds an explicit configuration to an already-open local store.
    pub fn new(config: &'a ReporterConfig, store: &'a EventStore) -> Self {
        Self {
            config,
            store,
            policy: SubmissionPolicy::default(),
            attempted: Cell::new(0),
        }
    }

    /// Uses explicit bounds, including a per-run backend-call allowance.
    pub fn with_policy(
        config: &'a ReporterConfig,
        store: &'a EventStore,
        policy: SubmissionPolicy,
    ) -> Result<Self, ManualSubmissionError> {
        Ok(Self {
            config,
            store,
            policy: policy.validate()?,
            attempted: Cell::new(0),
        })
    }

    /// Lists up to 100 pending, already sanitized reports, newest first.
    pub fn list_pending(&self, limit: usize) -> Result<Vec<StoredReport>, ManualSubmissionError> {
        if !self.config.records_locally() {
            return Err(ManualSubmissionError::Disabled);
        }
        self.store.list_pending(limit).map_err(Into::into)
    }

    /// Previews a selected, already sanitized pending report without network I/O.
    pub fn preview(&self, fingerprint: &str) -> Result<StoredReport, ManualSubmissionError> {
        if !self.config.records_locally() {
            return Err(ManualSubmissionError::Disabled);
        }
        self.store
            .get(fingerprint)?
            .filter(|report| report.submission_status == "pending")
            .ok_or(ManualSubmissionError::NotPending)
    }

    /// Requests one submission only after explicit approval and mode checks.
    ///
    /// Interactive callers should show [`Self::preview`] before passing
    /// [`SubmissionApproval::Confirmed`]. Unattended callers must explicitly
    /// select [`SubmissionApproval::UnattendedOptIn`] for each request.
    pub fn submit_selected(
        &self,
        fingerprint: &str,
        approval: SubmissionApproval,
        backend: &mut impl SubmissionBackend,
    ) -> Result<String, ManualSubmissionError> {
        self.submit_selected_at(fingerprint, approval, backend, now_ms())
    }

    /// Same submission path with an injected clock for deterministic policy tests.
    pub(crate) fn submit_selected_at(
        &self,
        fingerprint: &str,
        approval: SubmissionApproval,
        backend: &mut impl SubmissionBackend,
        now_ms: i64,
    ) -> Result<String, ManualSubmissionError> {
        if !self.config.permits_submission() {
            return Err(ManualSubmissionError::Disabled);
        }
        if approval == SubmissionApproval::Unconfirmed {
            return Err(ManualSubmissionError::ConfirmationRequired);
        }
        if self.attempted.get() >= self.policy.max_per_run {
            return Err(ManualSubmissionError::RunLimit);
        }
        let repository = self
            .config
            .repository()
            .ok_or(ManualSubmissionError::Disabled)?;
        let report = self.preview(fingerprint)?;
        let state = self
            .store
            .submission_state(fingerprint)?
            .ok_or(ManualSubmissionError::NotPending)?;
        if state.failures >= self.policy.max_failures {
            return Err(ManualSubmissionError::RetryExhausted);
        }
        if now_ms < state.next_retry_ms {
            return Err(ManualSubmissionError::RetryLater {
                retry_at_ms: state.next_retry_ms,
            });
        }
        if let Some(last_submitted_ms) = state.last_submitted_ms {
            let retry_at_ms = last_submitted_ms.saturating_add(duration_ms(self.policy.cooldown));
            if now_ms < retry_at_ms {
                return Err(ManualSubmissionError::Cooldown { retry_at_ms });
            }
        }
        let lease_until_ms = now_ms.saturating_add(duration_ms(DELIVERY_LEASE));
        if !self
            .store
            .claim_submission(&report, now_ms, lease_until_ms)?
        {
            return Err(ManualSubmissionError::Busy);
        }
        self.attempted.set(self.attempted.get().saturating_add(1));
        let issue_number = match backend.submit(repository, &report) {
            Ok(number) if number > 0 => number,
            Ok(_) => {
                self.defer_failure(
                    &report,
                    state.failures,
                    SubmissionFailure::InvalidResponse,
                    now_ms,
                    lease_until_ms,
                )?;
                return Err(ManualSubmissionError::Backend(
                    SubmissionFailure::InvalidResponse,
                ));
            }
            Err(failure) => {
                self.defer_failure(&report, state.failures, failure, now_ms, lease_until_ms)?;
                return Err(ManualSubmissionError::Backend(failure));
            }
        };
        let issue_url = format!(
            "https://github.com/{}/{}/issues/{issue_number}",
            repository.owner(),
            repository.name()
        );
        if !self
            .store
            .mark_submitted_snapshot(&report, &issue_url, now_ms, lease_until_ms)?
        {
            self.store
                .release_submission_lease(&report.fingerprint, lease_until_ms)?;
        }
        Ok(issue_url)
    }

    fn defer_failure(
        &self,
        report: &StoredReport,
        failures: u32,
        failure: SubmissionFailure,
        now_ms: i64,
        lease_until_ms: i64,
    ) -> Result<(), ManualSubmissionError> {
        let delay = match failure {
            SubmissionFailure::Transport => {
                let multiplier = 1u64 << failures.min(30);
                self.policy
                    .initial_backoff
                    .saturating_mul(u32::try_from(multiplier).unwrap_or(u32::MAX))
                    .min(self.policy.max_backoff)
            }
            SubmissionFailure::RateLimited { .. } | SubmissionFailure::SearchIncomplete => {
                let multiplier = 1u64 << failures.min(30);
                Duration::from_secs(60 * 60).max(
                    self.policy
                        .initial_backoff
                        .saturating_mul(u32::try_from(multiplier).unwrap_or(u32::MAX))
                        .min(self.policy.max_backoff),
                )
            }
            SubmissionFailure::Denied => Duration::from_secs(24 * 60 * 60),
            SubmissionFailure::InvalidResponse => Duration::from_secs(60 * 60),
        };
        let mut retry_at_ms = now_ms.saturating_add(duration_ms(delay));
        if let SubmissionFailure::RateLimited {
            retry_at_ms: Some(server_retry_ms),
        } = failure
        {
            retry_at_ms = retry_at_ms.max(server_retry_ms);
        }
        self.store
            .record_submission_failure(report, retry_at_ms, lease_until_ms)?;
        Ok(())
    }
}

fn duration_ms(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
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
        ErrorCategory, ErrorCode, ErrorSeverity, ExecutionSurface, RawEvent, RetentionPolicy,
    };

    static NEXT_DB: AtomicU64 = AtomicU64::new(0);

    struct Backend {
        calls: usize,
        repository: Option<String>,
        fingerprint: Option<String>,
        failure: Option<SubmissionFailure>,
    }

    impl SubmissionBackend for Backend {
        fn submit(
            &mut self,
            repository: &Repository,
            report: &StoredReport,
        ) -> Result<u64, SubmissionFailure> {
            self.calls += 1;
            self.repository = Some(format!("{}/{}", repository.owner(), repository.name()));
            self.fingerprint = Some(report.fingerprint.clone());
            if let Some(failure) = self.failure {
                return Err(failure);
            }
            Ok(42)
        }
    }

    #[test]
    fn preview_redacts_and_submission_requires_explicit_approval() {
        let path = std::env::temp_dir().join(format!(
            "omoikane-manual-report-{}-{}",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let mut store =
            EventStore::open(path.join("events.sqlite"), RetentionPolicy::default()).unwrap();
        let raw = "Authorization: Bearer PRIVATE /home/user/private.js?key=SECRET";
        let report = RawEvent::new(
            ErrorCategory::JavaScript,
            ErrorSeverity::Error,
            ErrorCode::new("MANUAL_TEST").unwrap(),
            ExecutionSurface::Headless,
            raw,
            &[
                ("operation", "execute"),
                ("url", "https://private.example/?key=SECRET"),
            ],
        )
        .sanitize();
        store.record_at(&report, 1_000).unwrap();
        let fingerprint = report.fingerprint().to_owned();
        let mut backend = Backend {
            calls: 0,
            repository: None,
            fingerprint: None,
            failure: None,
        };
        let off = ReporterConfig::default();
        let disabled = ManualSubmission::new(&off, &store);
        assert!(matches!(
            disabled.list_pending(10),
            Err(ManualSubmissionError::Disabled)
        ));
        assert_eq!(
            disabled.submit_selected(&fingerprint, SubmissionApproval::Confirmed, &mut backend),
            Err(ManualSubmissionError::Disabled)
        );
        let record_only =
            ReporterConfig::from_values(Some("record-only"), Some("owner/repo")).unwrap();
        let local = ManualSubmission::new(&record_only, &store);
        let preview = local.preview(&fingerprint).unwrap();
        assert_eq!(local.list_pending(10).unwrap(), vec![preview.clone()]);
        let text = format!("{preview:?}");
        assert!(!text.contains("PRIVATE"));
        assert!(!text.contains("SECRET"));
        assert!(!text.contains("private.example"));
        assert_eq!(
            local.submit_selected(&fingerprint, SubmissionApproval::Confirmed, &mut backend),
            Err(ManualSubmissionError::Disabled)
        );
        let submit = ReporterConfig::from_values(Some("submit"), Some("owner/repo")).unwrap();
        let manual = ManualSubmission::new(&submit, &store);
        assert_eq!(
            manual.submit_selected(&fingerprint, SubmissionApproval::Unconfirmed, &mut backend),
            Err(ManualSubmissionError::ConfirmationRequired)
        );
        assert_eq!(backend.calls, 0);
        assert_eq!(
            manual
                .submit_selected(&fingerprint, SubmissionApproval::Confirmed, &mut backend)
                .unwrap(),
            "https://github.com/owner/repo/issues/42"
        );
        assert_eq!(backend.calls, 1);
        assert_eq!(backend.repository.as_deref(), Some("owner/repo"));
        assert_eq!(backend.fingerprint.as_deref(), Some(fingerprint.as_str()));
        assert_eq!(
            manual.submit_selected(&fingerprint, SubmissionApproval::Confirmed, &mut backend),
            Err(ManualSubmissionError::NotPending)
        );
        assert_eq!(backend.calls, 1);
        let unattended = RawEvent::new(
            ErrorCategory::JavaScript,
            ErrorSeverity::Error,
            ErrorCode::new("UNATTENDED_TEST").unwrap(),
            ExecutionSurface::Headless,
            "JavaScript execution failed",
            &[],
        )
        .sanitize();
        drop(manual);
        store.record_at(&unattended, 2_000).unwrap();
        let manual = ManualSubmission::new(&submit, &store);
        assert_eq!(
            manual
                .submit_selected(
                    unattended.fingerprint(),
                    SubmissionApproval::UnattendedOptIn,
                    &mut backend,
                )
                .unwrap(),
            "https://github.com/owner/repo/issues/42"
        );
        assert_eq!(backend.calls, 2);
        assert!(manual.list_pending(10).unwrap().is_empty());
        drop(store);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn run_limit_cooldown_and_persisted_exponential_retry() {
        let path = std::env::temp_dir().join(format!(
            "omoikane-manual-policy-{}-{}",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let mut store =
            EventStore::open(path.join("events.sqlite"), RetentionPolicy::default()).unwrap();
        let safe = RawEvent::new(
            ErrorCategory::JavaScript,
            ErrorSeverity::Error,
            ErrorCode::new("RETRY_TEST").unwrap(),
            ExecutionSurface::Headless,
            "JavaScript execution failed",
            &[],
        )
        .sanitize();
        store.record_at(&safe, 1_000).unwrap();
        let config = ReporterConfig::from_values(Some("submit"), Some("owner/repo")).unwrap();
        let policy = SubmissionPolicy {
            max_per_run: 2,
            cooldown: Duration::from_secs(60),
            ..SubmissionPolicy::default()
        };
        let mut backend = Backend {
            calls: 0,
            repository: None,
            fingerprint: None,
            failure: Some(SubmissionFailure::Transport),
        };
        let manual = ManualSubmission::with_policy(&config, &store, policy).unwrap();
        assert_eq!(
            manual.submit_selected_at(
                safe.fingerprint(),
                SubmissionApproval::Confirmed,
                &mut backend,
                1_000
            ),
            Err(ManualSubmissionError::Backend(SubmissionFailure::Transport))
        );
        assert_eq!(
            store
                .submission_state(safe.fingerprint())
                .unwrap()
                .unwrap()
                .next_retry_ms,
            61_000
        );
        assert_eq!(
            manual.submit_selected_at(
                safe.fingerprint(),
                SubmissionApproval::Confirmed,
                &mut backend,
                60_999
            ),
            Err(ManualSubmissionError::RetryLater {
                retry_at_ms: 61_000
            })
        );
        assert_eq!(backend.calls, 1);
        assert_eq!(
            manual.submit_selected_at(
                safe.fingerprint(),
                SubmissionApproval::Confirmed,
                &mut backend,
                61_000
            ),
            Err(ManualSubmissionError::Backend(SubmissionFailure::Transport))
        );
        assert_eq!(
            store
                .submission_state(safe.fingerprint())
                .unwrap()
                .unwrap()
                .next_retry_ms,
            181_000
        );
        assert_eq!(
            manual.submit_selected_at(
                safe.fingerprint(),
                SubmissionApproval::Confirmed,
                &mut backend,
                181_000
            ),
            Err(ManualSubmissionError::RunLimit)
        );
        assert_eq!(backend.calls, 2);
        drop(manual);
        backend.failure = None;
        let manual = ManualSubmission::with_policy(&config, &store, policy).unwrap();
        assert_eq!(
            manual
                .submit_selected_at(
                    safe.fingerprint(),
                    SubmissionApproval::Confirmed,
                    &mut backend,
                    181_000
                )
                .unwrap(),
            "https://github.com/owner/repo/issues/42"
        );
        assert_eq!(
            store
                .submission_state(safe.fingerprint())
                .unwrap()
                .unwrap()
                .failures,
            0
        );
        drop(manual);
        store.record_at(&safe, 182_000).unwrap();
        let manual = ManualSubmission::with_policy(&config, &store, policy).unwrap();
        assert_eq!(
            manual.submit_selected_at(
                safe.fingerprint(),
                SubmissionApproval::Confirmed,
                &mut backend,
                200_000
            ),
            Err(ManualSubmissionError::Cooldown {
                retry_at_ms: 241_000
            })
        );
        assert_eq!(backend.calls, 3);
        assert_eq!(
            manual
                .submit_selected_at(
                    safe.fingerprint(),
                    SubmissionApproval::Confirmed,
                    &mut backend,
                    241_000
                )
                .unwrap(),
            "https://github.com/owner/repo/issues/42"
        );
        assert_eq!(backend.calls, 4);
        drop(manual);
        drop(store);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn rate_limit_and_denied_failures_are_deferred_without_losing_pending_row() {
        let path = std::env::temp_dir().join(format!(
            "omoikane-manual-deferral-{}-{}",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let mut store =
            EventStore::open(path.join("events.sqlite"), RetentionPolicy::default()).unwrap();
        let safe = RawEvent::new(
            ErrorCategory::Http,
            ErrorSeverity::Error,
            ErrorCode::new("RATE_TEST").unwrap(),
            ExecutionSurface::Headless,
            "Resource load failed",
            &[],
        )
        .sanitize();
        store.record_at(&safe, 1_000).unwrap();
        let config = ReporterConfig::from_values(Some("submit"), Some("owner/repo")).unwrap();
        let manual = ManualSubmission::with_policy(
            &config,
            &store,
            SubmissionPolicy {
                max_failures: 2,
                ..SubmissionPolicy::default()
            },
        )
        .unwrap();
        let limited = SubmissionFailure::RateLimited {
            retry_at_ms: Some(7_200_000),
        };
        let mut backend = Backend {
            calls: 0,
            repository: None,
            fingerprint: None,
            failure: Some(limited),
        };
        assert_eq!(
            manual.submit_selected_at(
                safe.fingerprint(),
                SubmissionApproval::Confirmed,
                &mut backend,
                1_000
            ),
            Err(ManualSubmissionError::Backend(limited))
        );
        assert_eq!(
            store
                .submission_state(safe.fingerprint())
                .unwrap()
                .unwrap()
                .next_retry_ms,
            7_200_000
        );
        backend.failure = Some(SubmissionFailure::Denied);
        assert_eq!(
            manual.submit_selected_at(
                safe.fingerprint(),
                SubmissionApproval::Confirmed,
                &mut backend,
                7_200_000
            ),
            Err(ManualSubmissionError::Backend(SubmissionFailure::Denied))
        );
        assert_eq!(
            store
                .submission_state(safe.fingerprint())
                .unwrap()
                .unwrap()
                .next_retry_ms,
            93_600_000
        );
        assert_eq!(
            store
                .get(safe.fingerprint())
                .unwrap()
                .unwrap()
                .submission_status,
            "pending"
        );
        assert_eq!(backend.calls, 2);
        assert_eq!(
            manual.submit_selected_at(
                safe.fingerprint(),
                SubmissionApproval::Confirmed,
                &mut backend,
                93_600_000,
            ),
            Err(ManualSubmissionError::RetryExhausted)
        );
        assert_eq!(backend.calls, 2);
        assert!(store.reset_submission_retry(safe.fingerprint()).unwrap());
        backend.failure = None;
        assert!(
            manual
                .submit_selected_at(
                    safe.fingerprint(),
                    SubmissionApproval::Confirmed,
                    &mut backend,
                    93_600_000,
                )
                .is_ok()
        );
        assert_eq!(backend.calls, 3);
        drop(manual);
        drop(store);
        fs::remove_dir_all(path).unwrap();
    }
}
