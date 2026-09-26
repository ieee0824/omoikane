//! Privacy-preserving event types for the optional general-error reporter.
//!
//! Raw events are sanitized before local persistence or submission. The manual
//! submission entry point requires explicit approval before calling a backend.
//! Unsupported CSS/HTML observation logs remain separate; their existing
//! environment variables and tables are not changed by the general-error reporter.

mod config;
mod fingerprint;
mod github;
mod manual;
mod panic_hook;
mod privacy;
mod reporter;
mod store;

pub use config::{ConfigError, ReporterConfig, ReportingMode, Repository};
pub use github::{GitHubApi, GitHubFailure, GitHubIssueBackend, GitHubRestApi, RemoteIssue};
pub use manual::{
    ManualSubmission, ManualSubmissionError, SubmissionApproval, SubmissionBackend,
    SubmissionFailure, SubmissionPolicy,
};
pub use panic_hook::install_panic_reporter;
pub use privacy::{RawEvent, SafeContext, SafeEvent};
pub use reporter::{ErrorReporter, ReporterError, ReporterStats};
pub use store::{EventStore, RetentionPolicy, StoreError, StoredReport, SubmissionState};

use serde::Serialize;

/// Broad subsystem responsible for an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCategory {
    /// JavaScript parsing, execution, or asynchronous tasks.
    JavaScript,
    /// Background worker startup or execution.
    Worker,
    /// JavaScript module graph loading or evaluation.
    Module,
    /// CSS loading, parsing, or style resolution.
    Css,
    /// HTTP, TLS, redirects, or resource loading.
    Http,
    /// HTML parsing or document construction.
    Html,
    /// Layout calculation.
    Layout,
    /// Painting or screenshot capture.
    Paint,
    /// Graphical user interface processing.
    Gui,
    /// Chrome DevTools Protocol processing.
    Cdp,
    /// Other browser-engine processing.
    Internal,
}

impl ErrorCategory {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::JavaScript => "javascript",
            Self::Worker => "worker",
            Self::Module => "module",
            Self::Css => "css",
            Self::Http => "http",
            Self::Html => "html",
            Self::Layout => "layout",
            Self::Paint => "paint",
            Self::Gui => "gui",
            Self::Cdp => "cdp",
            Self::Internal => "internal",
        }
    }
}

/// Diagnostic importance; it does not alter the browser API's return value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorSeverity {
    /// A recoverable condition worth retaining.
    Warning,
    /// An operation failed while the browser continued running.
    Error,
    /// A failure that prevented the requested browser operation.
    Critical,
}

impl ErrorSeverity {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Critical => "critical",
        }
    }
}

/// The execution surface on which an error was observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionSurface {
    /// Headless library or CLI use.
    Headless,
    /// A graphical browser window.
    Gui,
    /// A CDP client request or event.
    Cdp,
    /// A C FFI caller.
    Ffi,
    /// A background worker or asynchronous task.
    Background,
}

impl ExecutionSurface {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Headless => "headless",
            Self::Gui => "gui",
            Self::Cdp => "cdp",
            Self::Ffi => "ffi",
            Self::Background => "background",
        }
    }
}

/// A stable, source-defined error code, never a page-supplied message.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
pub struct ErrorCode(&'static str);

impl ErrorCode {
    /// Validates a static `UPPER_SNAKE_CASE` code of at most 64 bytes.
    ///
    /// Pass only source-defined literals. Runtime data, URLs, paths, and user
    /// input belong neither in this code nor in a fingerprint.
    pub fn new(code: &'static str) -> Result<Self, InvalidErrorCode> {
        let valid = !code.is_empty()
            && code.len() <= 64
            && code.as_bytes()[0].is_ascii_uppercase()
            && code
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
        if valid {
            Ok(Self(code))
        } else {
            Err(InvalidErrorCode)
        }
    }

    /// Returns the stable code used for aggregation and display.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// An invalid static error-code declaration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidErrorCode;

impl std::fmt::Display for InvalidErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("error code must be static UPPER_SNAKE_CASE (1..=64 bytes)")
    }
}

impl std::error::Error for InvalidErrorCode {}

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;
