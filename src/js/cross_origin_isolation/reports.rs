//! Reporting API bodies for COOP/COEP violations.
//!
//! These constructors only transform explicit inputs. The caller owns endpoint
//! selection, document lifetime, observer queuing, and HTTP delivery.

use super::OpenerPolicyValue;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::js) enum Disposition {
    Enforce,
    Reporting,
}

impl Disposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Enforce => "enforce",
            Self::Reporting => "reporting",
        }
    }
}

/// Removes URL credentials, fragments, and non-HTTP payloads before reporting.
pub(in crate::js) fn sanitize_report_url(input: &str) -> String {
    let Ok(mut url) = url::Url::parse(input) else {
        return String::new();
    };
    if !matches!(url.scheme(), "http" | "https") {
        return url.scheme().to_owned();
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_fragment(None);
    url.to_string()
}

/// Builds the common Reporting API envelope for an isolation violation.
///
/// The caller supplies the owning Document's already sanitized creation URL
/// and user agent at the point of violation. HTTP delivery snapshots its
/// endpoint separately.
pub(in crate::js) fn report_envelope(
    report_type: &str,
    sanitized_document_url: &str,
    user_agent: &str,
    body: Value,
) -> Value {
    json!({
        "type": report_type,
        "url": sanitized_document_url,
        "age": 0,
        "user_agent": user_agent,
        "body": body,
    })
}

/// Builds the HTML COEP child-navigation or worker-initialization report body.
pub(in crate::js) fn coep_inheritance_body(
    response_url: &str,
    worker_initialization: bool,
    disposition: Disposition,
) -> Value {
    json!({
        "type": if worker_initialization { "worker initialization" } else { "navigation" },
        "blockedURL": sanitize_report_url(response_url),
        "disposition": disposition.as_str(),
    })
}

/// Builds the Fetch COEP CORP report body for an opaque response.
pub(in crate::js) fn coep_corp_body(
    initial_response_url: &str,
    destination: &str,
    disposition: Disposition,
) -> Value {
    json!({
        "type": "corp",
        "blockedURL": sanitize_report_url(initial_response_url),
        "destination": destination,
        "disposition": disposition.as_str(),
    })
}

pub(in crate::js) fn policy_name(policy: OpenerPolicyValue) -> &'static str {
    match policy {
        OpenerPolicyValue::UnsafeNone => "unsafe-none",
        OpenerPolicyValue::SameOriginAllowPopups => "same-origin-allow-popups",
        OpenerPolicyValue::SameOrigin => "same-origin",
        OpenerPolicyValue::SameOriginPlusCoep => "same-origin-plus-COEP",
        OpenerPolicyValue::NoopenerAllowPopups => "noopener-allow-popups",
    }
}

/// Builds the COOP report for the incoming response's endpoint.
pub(in crate::js) fn coop_navigation_to_body(
    policy: OpenerPolicyValue,
    previous_response_url: &str,
    same_origin: bool,
    referrer: &str,
    disposition: Disposition,
) -> Value {
    json!({
        "type": "navigation-to-response",
        "effectivePolicy": policy_name(policy),
        "previousResponseURL": same_origin.then(|| sanitize_report_url(previous_response_url)),
        "referrer": sanitize_report_url(referrer),
        "disposition": disposition.as_str(),
    })
}

/// Builds the COOP report for the outgoing response's endpoint.
pub(in crate::js) fn coop_navigation_from_body(
    policy: OpenerPolicyValue,
    next_response_url: &str,
    same_origin: bool,
    is_navigation_source: bool,
    disposition: Disposition,
) -> Value {
    json!({
        "type": "navigation-from-response",
        "effectivePolicy": policy_name(policy),
        "nextResponseURL": (same_origin || is_navigation_source)
            .then(|| sanitize_report_url(next_response_url)),
        "disposition": disposition.as_str(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coop_reports_hide_cross_origin_navigation_urls_and_url_credentials() {
        let to = coop_navigation_to_body(
            OpenerPolicyValue::SameOriginPlusCoep,
            "https://user:password@old.example/secret#fragment",
            false,
            "https://user:password@old.example/referrer#fragment",
            Disposition::Reporting,
        );
        assert!(to["previousResponseURL"].is_null());
        assert_eq!(to["referrer"], "https://old.example/referrer");
        assert_eq!(to["effectivePolicy"], "same-origin-plus-COEP");
        let from = coop_navigation_from_body(
            OpenerPolicyValue::SameOrigin,
            "https://user:password@new.example/private#fragment",
            false,
            false,
            Disposition::Enforce,
        );
        assert!(from["nextResponseURL"].is_null());
        let source = coop_navigation_from_body(
            OpenerPolicyValue::SameOrigin,
            "https://user:password@new.example/private#fragment",
            false,
            true,
            Disposition::Enforce,
        );
        assert_eq!(source["nextResponseURL"], "https://new.example/private");
    }

    #[test]
    fn coep_reports_distinguish_inheritance_and_corp_dispositions() {
        let navigation = coep_inheritance_body(
            "https://u:p@resource.example/child#secret",
            false,
            Disposition::Reporting,
        );
        assert_eq!(navigation["type"], "navigation");
        assert_eq!(navigation["blockedURL"], "https://resource.example/child");
        assert_eq!(navigation["disposition"], "reporting");
        let worker = coep_inheritance_body(
            "https://resource.example/worker",
            true,
            Disposition::Enforce,
        );
        assert_eq!(worker["type"], "worker initialization");
        let corp = coep_corp_body(
            "https://resource.example/script",
            "script",
            Disposition::Enforce,
        );
        assert_eq!(corp["type"], "corp");
        assert_eq!(corp["destination"], "script");
        assert_eq!(sanitize_report_url("/path?token=private#fragment"), "");
    }

    #[test]
    fn isolation_envelope_uses_the_owner_document_and_redacts_credentials() {
        let report = report_envelope(
            "coep",
            &sanitize_report_url("https://user:password@owner.example/document#secret"),
            "Omoikane-Test",
            coep_corp_body(
                "https://resource.example/script",
                "script",
                Disposition::Reporting,
            ),
        );
        assert_eq!(report["url"], "https://owner.example/document");
        assert_eq!(report["user_agent"], "Omoikane-Test");
        assert_eq!(report["age"], 0);
        assert_eq!(
            report["body"]["blockedURL"],
            "https://resource.example/script"
        );
        assert_eq!(sanitize_report_url("data:text/plain,secret"), "data");
    }
}
