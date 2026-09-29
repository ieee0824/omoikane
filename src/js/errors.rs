//! Typed failures at the boundary between the JS runtime and its embedder.

use boa_engine::{JsError, JsValue};

use crate::http::HttpParseError;
use crate::http::url::UrlParseError;

/// Errors while preparing an embedded browsing context.
#[derive(Debug)]
pub(super) enum JsHostError {
    /// A monotonically allocated identifier was exhausted.
    IdSpaceExhausted(&'static str),
    /// The iframe's owner document is no longer active.
    InactiveIframeOwner,
    /// The auxiliary browsing context has already closed.
    AuxiliaryContextClosed,
    /// The auxiliary browsing context closed during navigation.
    AuxiliaryContextClosedDuringNavigation,
    /// A form submission URL was invalid.
    Url(UrlParseError),
    /// Fetching a form submission failed.
    Http(HttpParseError),
}

impl std::fmt::Display for JsHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IdSpaceExhausted(label) => write!(f, "{label} id space exhausted"),
            Self::InactiveIframeOwner => write!(f, "iframe owner document is no longer active"),
            Self::AuxiliaryContextClosed => write!(f, "auxiliary context is closed"),
            Self::AuxiliaryContextClosedDuringNavigation => {
                write!(f, "auxiliary context was closed")
            }
            Self::Url(error) => write!(f, "{error}"),
            Self::Http(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for JsHostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Url(error) => Some(error),
            Self::Http(error) => Some(error),
            _ => None,
        }
    }
}

impl From<UrlParseError> for JsHostError {
    fn from(error: UrlParseError) -> Self {
        Self::Url(error)
    }
}

impl From<HttpParseError> for JsHostError {
    fn from(error: HttpParseError) -> Self {
        Self::Http(error)
    }
}

/// A JavaScript evaluation failure retained until the embedder chooses how to report it.
#[derive(Debug)]
pub enum JsEvaluationError {
    /// Parsing, compiling, executing, or draining jobs failed in Boa.
    JavaScript(JsError),
    /// A module's evaluation promise was rejected with this JS value.
    ModuleRejected(JsValue),
    /// A module's evaluation promise did not settle synchronously.
    ModulePending,
    /// The script's owning Document Realm was retired.
    DocumentRealmRetired,
}

impl std::fmt::Display for JsEvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JavaScript(error) => write!(f, "{error}"),
            Self::ModuleRejected(value) => write!(f, "{}", value.display()),
            Self::ModulePending => write!(f, "module evaluation remained pending"),
            Self::DocumentRealmRetired => write!(f, "script document Realm is no longer live"),
        }
    }
}

impl std::error::Error for JsEvaluationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::JavaScript(error) => Some(error),
            _ => None,
        }
    }
}

impl From<JsError> for JsEvaluationError {
    fn from(error: JsError) -> Self {
        Self::JavaScript(error)
    }
}

/// Errors while serializing a page-search request or decoding its result.
#[derive(Debug)]
pub enum FindInPageError {
    /// The action or query could not be serialized as JSON.
    Serialize(serde_json::Error),
    /// The page-search script failed in Boa.
    JavaScript(JsError),
    /// The page-search script returned a value other than a JSON string.
    MissingJsonString,
    /// The returned JSON did not describe a page-search result.
    Deserialize(serde_json::Error),
}

impl std::fmt::Display for FindInPageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize(error) | Self::Deserialize(error) => write!(f, "{error}"),
            Self::JavaScript(error) => write!(f, "{error}"),
            Self::MissingJsonString => write!(f, "page search did not return a JSON string"),
        }
    }
}

impl std::error::Error for FindInPageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) | Self::Deserialize(error) => Some(error),
            Self::JavaScript(error) => Some(error),
            Self::MissingJsonString => None,
        }
    }
}

/// An invalid embedder-controlled Notification permission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationPermissionError {
    /// The supplied permission is not one of the three supported values.
    Invalid(String),
}

impl std::fmt::Display for NotificationPermissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(permission) => write!(
                f,
                "invalid notification permission {permission:?}; expected one of default, granted, denied"
            ),
        }
    }
}

impl std::error::Error for NotificationPermissionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::js::{FindInPageAction, JsRuntime, take_monotonic_id};

    #[test]
    fn host_errors_retain_categories_sources_and_messages() {
        let mut next = u64::MAX;
        let error = take_monotonic_id(&mut next, "iframe browsing context").unwrap_err();
        assert!(matches!(error, JsHostError::IdSpaceExhausted(_)));
        assert_eq!(
            error.to_string(),
            "iframe browsing context id space exhausted"
        );
        assert_eq!(next, u64::MAX);

        let error = JsHostError::InactiveIframeOwner;
        assert_eq!(
            error.to_string(),
            "iframe owner document is no longer active"
        );
        let error = JsHostError::AuxiliaryContextClosed;
        assert_eq!(error.to_string(), "auxiliary context is closed");
        let error = JsHostError::AuxiliaryContextClosedDuringNavigation;
        assert_eq!(error.to_string(), "auxiliary context was closed");

        let error = JsHostError::Url("not-a-url".parse::<crate::http::Url>().unwrap_err());
        assert_eq!(error.to_string(), "missing '://' in URL");
        assert!(std::error::Error::source(&error).is_some());

        let error = JsHostError::Http(HttpParseError::InvalidHeader);
        assert_eq!(error.to_string(), "invalid HTTP header");
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn evaluation_errors_preserve_boa_messages_and_sources() {
        let mut runtime = JsRuntime::new().unwrap();
        let original = runtime.eval("missingFunction()").unwrap_err().to_string();
        let error = runtime.eval_safe("missingFunction()").unwrap_err();
        assert!(matches!(error, JsEvaluationError::JavaScript(_)));
        assert_eq!(error.to_string(), original);
        assert!(std::error::Error::source(&error).is_some());

        assert_eq!(
            JsEvaluationError::ModulePending.to_string(),
            "module evaluation remained pending"
        );
        assert_eq!(
            JsEvaluationError::DocumentRealmRetired.to_string(),
            "script document Realm is no longer live"
        );
        let rejection = JsValue::from(123);
        assert_eq!(
            JsEvaluationError::ModuleRejected(rejection.clone()).to_string(),
            rejection.display().to_string()
        );
    }

    #[test]
    fn page_search_and_permission_errors_keep_visible_messages() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .find_in_page(FindInPageAction::Start, "needle")
            .unwrap();
        let original = runtime.eval("missingFunction()").unwrap_err();
        let expected = original.to_string();
        let error = FindInPageError::JavaScript(original);
        assert!(matches!(error, FindInPageError::JavaScript(_)));
        assert_eq!(error.to_string(), expected);
        assert!(std::error::Error::source(&error).is_some());

        let error = FindInPageError::MissingJsonString;
        assert_eq!(
            error.to_string(),
            "page search did not return a JSON string"
        );
        let error = FindInPageError::Deserialize(
            serde_json::from_str::<serde_json::Value>("not json").unwrap_err(),
        );
        assert!(std::error::Error::source(&error).is_some());

        let error = runtime.set_notification_permission("maybe").unwrap_err();
        assert!(matches!(error, NotificationPermissionError::Invalid(_)));
        assert_eq!(
            error.to_string(),
            "invalid notification permission \"maybe\"; expected one of default, granted, denied"
        );
    }
}
