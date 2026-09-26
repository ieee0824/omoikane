//! Explicit, local preview and submission boundary for sanitized reports.

use std::fmt;

use super::{EventStore, ReporterConfig, Repository, StoreError, StoredReport};

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

/// Network-capable backend used only after configuration and approval checks.
///
/// The backend must return the canonical GitHub Issue number. It must not log
/// credentials, response bodies, or an unsanitized event on failure.
pub trait SubmissionBackend {
    /// Requests submission to the explicitly configured repository.
    fn submit(&mut self, repository: &Repository, report: &StoredReport) -> Result<u64, ()>;
}

/// A failure without credentials, report contents, or backend response details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManualSubmissionError {
    /// The current mode does not permit network submission.
    Disabled,
    /// This request was not explicitly approved.
    ConfirmationRequired,
    /// The selected report is not pending in the local store.
    NotPending,
    /// The submission backend failed or returned an invalid Issue number.
    Backend,
    /// Local storage failed.
    Store(StoreError),
}

impl fmt::Display for ManualSubmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Disabled => "error-report submission is disabled",
            Self::ConfirmationRequired => "error-report submission requires explicit approval",
            Self::NotPending => "error report is not pending",
            Self::Backend => "error-report submission failed",
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
}

impl<'a> ManualSubmission<'a> {
    /// Binds an explicit configuration to an already-open local store.
    pub const fn new(config: &'a ReporterConfig, store: &'a EventStore) -> Self {
        Self { config, store }
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
        if !self.config.permits_submission() {
            return Err(ManualSubmissionError::Disabled);
        }
        if approval == SubmissionApproval::Unconfirmed {
            return Err(ManualSubmissionError::ConfirmationRequired);
        }
        let repository = self
            .config
            .repository()
            .ok_or(ManualSubmissionError::Disabled)?;
        let report = self.preview(fingerprint)?;
        let issue_number = backend
            .submit(repository, &report)
            .map_err(|()| ManualSubmissionError::Backend)?;
        if issue_number == 0 {
            return Err(ManualSubmissionError::Backend);
        }
        let issue_url = format!(
            "https://github.com/{}/{}/issues/{issue_number}",
            repository.owner(),
            repository.name()
        );
        self.store.mark_submitted(fingerprint, &issue_url)?;
        Ok(issue_url)
    }
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
    }

    impl SubmissionBackend for Backend {
        fn submit(&mut self, repository: &Repository, report: &StoredReport) -> Result<u64, ()> {
            self.calls += 1;
            self.repository = Some(format!("{}/{}", repository.owner(), repository.name()));
            self.fingerprint = Some(report.fingerprint.clone());
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
}
