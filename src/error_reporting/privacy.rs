use std::collections::BTreeMap;

use serde::Serialize;

use super::{ErrorCategory, ErrorCode, ErrorSeverity, ExecutionSurface, fingerprint};

/// An untrusted event at the reporter boundary.
///
/// The message and context may contain page data, credentials, or local paths.
/// This type is intentionally not serializable or printable. Convert it with
/// [`RawEvent::sanitize`] before retaining, displaying, or submitting anything.
pub struct RawEvent<'a> {
    category: ErrorCategory,
    severity: ErrorSeverity,
    code: ErrorCode,
    surface: ExecutionSurface,
    message: &'a str,
    context: &'a [(&'a str, &'a str)],
}

impl<'a> RawEvent<'a> {
    /// Creates an event from a stable code and potentially sensitive details.
    pub fn new(
        category: ErrorCategory,
        severity: ErrorSeverity,
        code: ErrorCode,
        surface: ExecutionSurface,
        message: &'a str,
        context: &'a [(&'a str, &'a str)],
    ) -> Self {
        Self {
            category,
            severity,
            code,
            surface,
            message,
            context,
        }
    }

    /// Drops arbitrary details and retains only approved message templates and
    /// allowlisted context values. The result is the sole persistence payload.
    pub fn sanitize(self) -> SafeEvent {
        let context = SafeContext::from_raw(self.context);
        let fingerprint = fingerprint::compute(self.category, self.code, &context);
        SafeEvent {
            category: self.category,
            severity: self.severity,
            code: self.code,
            surface: self.surface,
            message: safe_message(self.message),
            context,
            fingerprint,
            version: env!("CARGO_PKG_VERSION"),
            commit: build_commit(),
            platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        }
    }
}

/// Context containing only known keys and fixed, non-user-derived values.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SafeContext(BTreeMap<&'static str, &'static str>);

impl SafeContext {
    fn from_raw(raw: &[(&str, &str)]) -> Self {
        let mut values = BTreeMap::new();
        for &(key, value) in raw {
            if let Some((key, value)) = allowlisted_context(key, value) {
                values.insert(key, value);
            }
        }
        Self(values)
    }

    /// Returns the allowlisted key/value pairs in a stable order.
    pub fn values(&self) -> &BTreeMap<&'static str, &'static str> {
        &self.0
    }
}

/// A fully sanitized payload safe to pass to a future database or Issue sink.
///
/// Fields cannot be set directly by callers, so the fingerprint and all
/// serialized/displayed fields always derive from the same sanitization pass.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SafeEvent {
    category: ErrorCategory,
    severity: ErrorSeverity,
    code: ErrorCode,
    surface: ExecutionSurface,
    message: &'static str,
    context: SafeContext,
    fingerprint: String,
    version: &'static str,
    commit: Option<&'static str>,
    platform: String,
}

impl SafeEvent {
    /// Returns the error category.
    pub const fn category(&self) -> ErrorCategory {
        self.category
    }

    /// Returns the error severity.
    pub const fn severity(&self) -> ErrorSeverity {
        self.severity
    }

    /// Returns the stable, source-defined error code.
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns the execution surface.
    pub const fn surface(&self) -> ExecutionSurface {
        self.surface
    }

    /// Returns an approved message template, never the raw error text.
    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// Returns the allowlisted diagnostic context.
    pub fn context(&self) -> &SafeContext {
        &self.context
    }

    /// Returns the versioned SHA-256 fingerprint of category, code, and context.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Returns the Omoikane package version captured at build time.
    pub const fn version(&self) -> &'static str {
        self.version
    }

    /// Returns a validated build commit when supplied by the build environment.
    pub const fn commit(&self) -> Option<&'static str> {
        self.commit
    }

    /// Returns the build-target OS and architecture.
    pub fn platform(&self) -> &str {
        &self.platform
    }
}

fn build_commit() -> Option<&'static str> {
    option_env!("OMOIKANE_BUILD_COMMIT")
        .or(option_env!("GITHUB_SHA"))
        .filter(|value| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn safe_message(raw: &str) -> &'static str {
    // Exact, source-defined templates are useful while never copying arbitrary
    // text. All unknown messages, including those with secrets, collapse to a
    // fixed placeholder; the stable code and typed context retain the cause.
    match raw {
        "JavaScript execution failed" => "JavaScript execution failed",
        "Resource load failed" => "Resource load failed",
        "Stylesheet parse failed" => "Stylesheet parse failed",
        "Layout failed" => "Layout failed",
        "Paint failed" => "Paint failed",
        "Operation timed out" => "Operation timed out",
        _ => "Error details withheld",
    }
}

fn allowlisted_context(key: &str, value: &str) -> Option<(&'static str, &'static str)> {
    let value = match key {
        "operation" => match value {
            "parse" => "parse",
            "fetch" => "fetch",
            "execute" => "execute",
            "render" => "render",
            "navigate" => "navigate",
            "decode" => "decode",
            "connect" => "connect",
            "read" => "read",
            "write" => "write",
            _ => return None,
        },
        "resource" => match value {
            "document" => "document",
            "script" => "script",
            "stylesheet" => "stylesheet",
            "image" => "image",
            "font" => "font",
            "other" => "other",
            _ => return None,
        },
        "failure_kind" => match value {
            "timeout" => "timeout",
            "parse" => "parse",
            "network" => "network",
            "security" => "security",
            "unsupported" => "unsupported",
            "invalid-state" => "invalid-state",
            "resource-exhausted" => "resource-exhausted",
            "internal" => "internal",
            _ => return None,
        },
        "http_status_class" => match value {
            "1xx" => "1xx",
            "2xx" => "2xx",
            "3xx" => "3xx",
            "4xx" => "4xx",
            "5xx" => "5xx",
            _ => return None,
        },
        _ => return None,
    };
    Some((key_name(key), value))
}

fn key_name(key: &str) -> &'static str {
    match key {
        "operation" => "operation",
        "resource" => "resource",
        "failure_kind" => "failure_kind",
        "http_status_class" => "http_status_class",
        _ => unreachable!("only allowlisted keys reach key_name"),
    }
}
