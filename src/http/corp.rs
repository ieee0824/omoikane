//! Cross-Origin-Resource-Policy decisions for opaque fetch responses.
//!
//! This module implements Fetch's internal CORP check. Callers decide when a
//! response is opaque and whether an embedder policy violation needs reporting.

use super::Url;
use super::cors::Origin;

/// The COEP value supplied by the requesting environment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum EmbedderPolicyValue {
    /// No additional COEP restriction.
    #[default]
    UnsafeNone,
    /// Responses without an explicit CORP value require matching origins.
    RequireCorp,
    /// Uncredentialed subresources may omit an explicit CORP value.
    Credentialless,
}

/// The combined Fetch CORP and COEP decision for one response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PolicyCheck {
    /// The response cannot be used by this request.
    pub(crate) blocked: bool,
    /// The report-only COEP would block this response.
    pub(crate) report_only_violation: bool,
    /// The enforced COEP blocks this response.
    pub(crate) enforced_violation: bool,
}

/// Checks explicit CORP first, then monitored and enforced COEP values.
///
/// An explicit CORP denial is unrelated to COEP and must not produce a COEP
/// violation report. The caller queues reports before acting on `blocked`.
pub(crate) fn check_response(
    origin: &Origin,
    response_url: &Url,
    headers: &[(String, String)],
    enforced: EmbedderPolicyValue,
    report_only: EmbedderPolicyValue,
    request_includes_credentials: bool,
    for_navigation: bool,
) -> PolicyCheck {
    if !allows_response(
        origin,
        response_url,
        headers,
        EmbedderPolicyValue::UnsafeNone,
        request_includes_credentials,
        for_navigation,
    ) {
        return PolicyCheck {
            blocked: true,
            report_only_violation: false,
            enforced_violation: false,
        };
    }
    let report_only_violation = !allows_response(
        origin,
        response_url,
        headers,
        report_only,
        request_includes_credentials,
        for_navigation,
    );
    let enforced_violation = !allows_response(
        origin,
        response_url,
        headers,
        enforced,
        request_includes_credentials,
        for_navigation,
    );
    PolicyCheck {
        blocked: enforced_violation,
        report_only_violation,
        enforced_violation,
    }
}

/// Checks a response's CORP value and the requesting environment's COEP value.
///
/// `response_url` is the URL of this response, including a redirect response.
/// `request_includes_credentials` records the request's effective credentials
/// mode for this hop. Only nested navigations pass `for_navigation = true`.
pub(crate) fn allows_response(
    origin: &Origin,
    response_url: &Url,
    headers: &[(String, String)],
    embedder_policy: EmbedderPolicyValue,
    request_includes_credentials: bool,
    for_navigation: bool,
) -> bool {
    if for_navigation && embedder_policy == EmbedderPolicyValue::UnsafeNone {
        return true;
    }

    let policy = resource_policy(headers);
    let effective_policy = match (policy, embedder_policy) {
        (Some(policy), _) => Some(policy),
        (None, EmbedderPolicyValue::UnsafeNone) => None,
        (None, EmbedderPolicyValue::RequireCorp) => Some(ResourcePolicy::SameOrigin),
        (None, EmbedderPolicyValue::Credentialless)
            if request_includes_credentials || for_navigation =>
        {
            Some(ResourcePolicy::SameOrigin)
        }
        (None, EmbedderPolicyValue::Credentialless) => None,
    };

    match effective_policy {
        None | Some(ResourcePolicy::CrossOrigin) => true,
        Some(ResourcePolicy::SameOrigin) => origin.is_same_origin(response_url),
        Some(ResourcePolicy::SameSite) => origin.is_same_site_for_corp(response_url),
    }
}

#[derive(Clone, Copy)]
enum ResourcePolicy {
    SameOrigin,
    SameSite,
    CrossOrigin,
}

fn resource_policy(headers: &[(String, String)]) -> Option<ResourcePolicy> {
    let mut values = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("cross-origin-resource-policy"))
        .map(|(_, value)| value.trim_matches([' ', '\t']));
    let value = values.next()?;
    // Fetch combines repeated fields with commas. No combined value matches
    // CORP's three case-sensitive tokens.
    if values.next().is_some() {
        return None;
    }
    match value {
        "same-origin" => Some(ResourcePolicy::SameOrigin),
        "same-site" => Some(ResourcePolicy::SameSite),
        "cross-origin" => Some(ResourcePolicy::CrossOrigin),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coep_reports_only_violations_added_by_embedder_policy() {
        let owner: Url = "https://owner.example.test/page".parse().unwrap();
        let resource: Url = "https://resource.example.test/image".parse().unwrap();
        let origin = Origin::from_url(&owner);
        let no_headers = [];
        let monitored = check_response(
            &origin,
            &resource,
            &no_headers,
            EmbedderPolicyValue::UnsafeNone,
            EmbedderPolicyValue::RequireCorp,
            true,
            false,
        );
        assert_eq!(
            monitored,
            PolicyCheck {
                blocked: false,
                report_only_violation: true,
                enforced_violation: false,
            }
        );
        let enforced = check_response(
            &origin,
            &resource,
            &no_headers,
            EmbedderPolicyValue::RequireCorp,
            EmbedderPolicyValue::RequireCorp,
            true,
            false,
        );
        assert!(enforced.blocked && enforced.report_only_violation && enforced.enforced_violation);
        let explicitly_denied = check_response(
            &origin,
            &resource,
            &[("Cross-Origin-Resource-Policy".into(), "same-origin".into())],
            EmbedderPolicyValue::RequireCorp,
            EmbedderPolicyValue::RequireCorp,
            true,
            false,
        );
        assert!(explicitly_denied.blocked);
        assert!(!explicitly_denied.report_only_violation);
        assert!(!explicitly_denied.enforced_violation);
    }

    fn check(
        source: &str,
        target: &str,
        fields: &[(&str, &str)],
        embedder: EmbedderPolicyValue,
        credentials: bool,
        navigation: bool,
    ) -> bool {
        let source: Url = source.parse().unwrap();
        let target: Url = target.parse().unwrap();
        let headers = fields
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect::<Vec<_>>();
        allows_response(
            &Origin::from_url(&source),
            &target,
            &headers,
            embedder,
            credentials,
            navigation,
        )
    }

    #[test]
    fn explicit_policy_uses_response_origin_and_case_sensitive_tokens() {
        use EmbedderPolicyValue::UnsafeNone;
        let source = "https://a.example.test/page";
        let target = "https://b.example.test/resource";
        assert!(!check(
            source,
            target,
            &[("Cross-Origin-Resource-Policy", "same-origin")],
            UnsafeNone,
            false,
            false
        ));
        assert!(check(
            source,
            target,
            &[("Cross-Origin-Resource-Policy", "same-site")],
            UnsafeNone,
            false,
            false
        ));
        assert!(check(
            source,
            target,
            &[("Cross-Origin-Resource-Policy", "cross-origin")],
            UnsafeNone,
            false,
            false
        ));
        for value in ["Same-Site", "same-origin, same-site"] {
            assert!(check(
                source,
                target,
                &[("Cross-Origin-Resource-Policy", value)],
                UnsafeNone,
                false,
                false
            ));
        }
        assert!(check(
            source,
            target,
            &[
                ("Cross-Origin-Resource-Policy", "same-origin"),
                ("cross-origin-resource-policy", "same-site"),
            ],
            UnsafeNone,
            false,
            false
        ));
    }

    #[test]
    fn same_site_uses_schemeless_site_with_https_response_guard() {
        use EmbedderPolicyValue::UnsafeNone;
        let fields = &[("Cross-Origin-Resource-Policy", "same-site")];
        assert!(check(
            "https://a.example.co.uk/",
            "http://b.example.co.uk/",
            fields,
            UnsafeNone,
            false,
            false
        ));
        assert!(!check(
            "http://a.example.co.uk/",
            "https://b.example.co.uk/",
            fields,
            UnsafeNone,
            false,
            false
        ));
        assert!(!check(
            "https://a.example.co.uk/",
            "https://other.co.uk/",
            fields,
            UnsafeNone,
            false,
            false
        ));
        let target: Url = "https://a.example.co.uk/resource".parse().unwrap();
        let headers = vec![("Cross-Origin-Resource-Policy".into(), "same-site".into())];
        assert!(!allows_response(
            &Origin::opaque(),
            &target,
            &headers,
            UnsafeNone,
            false,
            false
        ));
    }

    #[test]
    fn coep_fallback_depends_on_credential_and_navigation_state() {
        use EmbedderPolicyValue::*;
        let source = "https://a.example.test/";
        let target = "https://b.example.test/";
        assert!(check(source, target, &[], UnsafeNone, true, false));
        assert!(!check(source, target, &[], RequireCorp, false, false));
        assert!(check(source, target, &[], Credentialless, false, false));
        assert!(!check(source, target, &[], Credentialless, true, false));
        assert!(!check(source, target, &[], Credentialless, false, true));
        assert!(check(
            source,
            target,
            &[("Cross-Origin-Resource-Policy", "cross-origin")],
            RequireCorp,
            true,
            false
        ));
        assert!(check(
            source,
            target,
            &[("Cross-Origin-Resource-Policy", "same-origin")],
            UnsafeNone,
            true,
            true
        ));
    }
}
