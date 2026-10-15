//! Owned cross-origin isolation policy snapshots and pure comparison steps.
//!
//! Parsing requires the caller to supply the reserved environment's secure
//! context state, including ancestor restrictions. Navigation comparisons use
//! immutable security-origin equality, including opaque-origin identities, not
//! URL strings or `document.domain`. Callers must apply the resulting decisions
//! to browsing context groups, retained WindowProxies and resource loading.
//!
//! These values neither create groups nor grant `crossOriginIsolated`. That
//! capability also requires the group's isolation mode and Permissions Policy.
//! Redirect navigation must accumulate switch decisions across responses.
//! Inherited policy containers must be copied from the creator instead of
//! reparsing absent headers on `about:blank` or `about:srcdoc`.
//!
//! Specification: <https://html.spec.whatwg.org/multipage/browsers.html>.

mod headers;
mod reports;
pub(super) use reports::{
    Disposition, coep_corp_body, coep_inheritance_body, coop_navigation_from_body,
    coop_navigation_to_body, report_envelope,
};

/// A parsed COOP value, including the internal value derived from COEP.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum OpenerPolicyValue {
    /// The default policy, which can share a group across origins.
    #[default]
    UnsafeNone,
    /// Requires matching origins and policies, except for initial popups.
    SameOriginAllowPopups,
    /// Requires matching origins and policies for related top-level contexts.
    SameOrigin,
    /// Derived from `same-origin` and a compatible COEP; not a header token.
    SameOriginPlusCoep,
    /// Severs the incoming opener while permitting initial unsafe-none popups.
    NoopenerAllowPopups,
}

impl OpenerPolicyValue {
    /// Matches COOP values using the caller's immutable security-origin check.
    pub(super) fn matches(self, response: Self, same_origin: bool) -> bool {
        if self == Self::UnsafeNone && response == Self::UnsafeNone {
            return true;
        }
        self == response && same_origin
    }

    /// Whether this active policy and a response require a new context group.
    ///
    /// The popup exceptions apply only to the initial `about:blank` document,
    /// not to later navigations of a context originally created as a popup.
    pub(super) fn requires_group_switch(
        self,
        response: Self,
        same_origin: bool,
        is_initial_about_blank: bool,
    ) -> bool {
        if is_initial_about_blank {
            if response == Self::NoopenerAllowPopups {
                return true;
            }
            if matches!(
                self,
                Self::SameOriginAllowPopups | Self::NoopenerAllowPopups
            ) && response == Self::UnsafeNone
            {
                return false;
            }
        }
        !self.matches(response, same_origin)
    }
}

/// An embedder policy value, independent of the document's opener policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum EmbedderPolicyValue {
    /// The default policy, imposing no additional COEP restrictions.
    #[default]
    UnsafeNone,
    /// Cross-origin resources must opt in through CORP or successful CORS.
    RequireCorp,
    /// Cross-origin no-CORS requests omit credentials before transmission.
    Credentialless,
}

impl EmbedderPolicyValue {
    /// Whether this value satisfies COEP's isolation compatibility check.
    ///
    /// This does not check COOP, Permissions Policy, CORP, or group isolation.
    pub(super) fn is_isolation_compatible(self) -> bool {
        matches!(self, Self::RequireCorp | Self::Credentialless)
    }
}

impl From<EmbedderPolicyValue> for crate::http::corp::EmbedderPolicyValue {
    fn from(value: EmbedderPolicyValue) -> Self {
        match value {
            EmbedderPolicyValue::UnsafeNone => Self::UnsafeNone,
            EmbedderPolicyValue::RequireCorp => Self::RequireCorp,
            EmbedderPolicyValue::Credentialless => Self::Credentialless,
        }
    }
}

/// Enforced and monitored opener policy, with owned endpoint group names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct OpenerPolicy {
    /// Policy used to enforce top-level group changes.
    pub(super) value: OpenerPolicyValue,
    /// Endpoint group from the enforced header's String `report-to` parameter.
    pub(super) reporting_endpoint: Option<String>,
    /// Policy used only when deciding whether to generate violation reports.
    pub(super) report_only_value: OpenerPolicyValue,
    /// Endpoint group from the report-only header's String parameter.
    pub(super) report_only_reporting_endpoint: Option<String>,
}

/// Navigation comparison without applying any browsing-context side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GroupSwitchDecision {
    /// Enforced COOP requires a new group.
    pub(super) enforce: bool,
    /// The report-only comparison requests reporting, not a group change.
    pub(super) report_only: bool,
}

impl OpenerPolicy {
    /// Compares one response with the active document's COOP snapshot.
    ///
    /// Matching report-only policies suppress reports. Otherwise, compare each
    /// report-only policy against the other document's enforced policy, as in
    /// HTML's report-only COOP group-switch algorithm. This result alone does
    /// not decide whether a report is sent; the caller also checks group size.
    pub(super) fn compare_navigation(
        &self,
        response: &Self,
        same_origin: bool,
        is_initial_about_blank: bool,
    ) -> GroupSwitchDecision {
        let switches = |active: OpenerPolicyValue, response| {
            active.requires_group_switch(response, same_origin, is_initial_about_blank)
        };
        GroupSwitchDecision {
            enforce: switches(self.value, response.value),
            report_only: switches(self.report_only_value, response.report_only_value)
                && (switches(self.report_only_value, response.value)
                    || switches(self.value, response.report_only_value)),
        }
    }
}

/// Enforced and monitored embedder policy, with owned endpoint group names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct EmbedderPolicy {
    /// Policy enforced during resource fetching and child initialization.
    pub(super) value: EmbedderPolicyValue,
    /// Empty when no COEP reporting endpoint was specified.
    pub(super) reporting_endpoint: String,
    /// Monitored policy, which does not itself block responses.
    pub(super) report_only_value: EmbedderPolicyValue,
    /// Empty when no report-only COEP reporting endpoint was specified.
    pub(super) report_only_reporting_endpoint: String,
}

/// The COEP inheritance check for child navigation or dedicated worker startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EmbedderInheritanceDecision {
    /// Prevent committing the child response or running the worker script.
    pub(super) blocked: bool,
    /// Queue a report-only inheritance violation without blocking for it.
    pub(super) report_only_violation: bool,
}

impl EmbedderPolicy {
    /// Checks the parent's/owner's policy against the response's enforced COEP.
    ///
    /// This is only HTML's COEP inheritance check. Child navigation additionally
    /// requires Fetch's navigation CORP check, even when this check succeeds.
    /// Do not apply this inheritance rule to unrelated top-level navigation.
    pub(super) fn check_inheritance(
        &self,
        response: EmbedderPolicyValue,
    ) -> EmbedderInheritanceDecision {
        let incompatible_response = !response.is_isolation_compatible();
        EmbedderInheritanceDecision {
            blocked: self.value.is_isolation_compatible() && incompatible_response,
            report_only_violation: self.report_only_value.is_isolation_compatible()
                && incompatible_response,
        }
    }
}

/// The COOP/COEP values derived from one response and its reserved environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct IsolationPolicy {
    /// Top-level navigation and opener relationship policy.
    pub(super) opener: OpenerPolicy,
    /// Resource loading and child environment policy.
    pub(super) embedder: EmbedderPolicy,
}

impl IsolationPolicy {
    /// Parses repeated response fields without accessing runtime or network state.
    ///
    /// `secure_context` must describe the reserved environment, not merely its
    /// URL scheme. Header values retain HTTP field order and are combined before
    /// Structured Field parsing. Nonsecure environments receive default policies.
    pub(super) fn from_response_headers(
        headers: &[(String, String)],
        secure_context: bool,
    ) -> Self {
        if !secure_context {
            return Self::default();
        }
        let coep = headers::policy_field(headers, "cross-origin-embedder-policy");
        let coep_report =
            headers::policy_field(headers, "cross-origin-embedder-policy-report-only");
        let embedder = EmbedderPolicy {
            value: embedder_value(coep.as_ref()),
            reporting_endpoint: embedder_endpoint(coep.as_ref()),
            report_only_value: embedder_value(coep_report.as_ref()),
            report_only_reporting_endpoint: embedder_endpoint(coep_report.as_ref()),
        };
        let coop = headers::policy_field(headers, "cross-origin-opener-policy");
        let coop_report = headers::policy_field(headers, "cross-origin-opener-policy-report-only");
        let enforced_compatible = embedder.value.is_isolation_compatible();
        let monitored_compatible =
            enforced_compatible || embedder.report_only_value.is_isolation_compatible();
        let opener = OpenerPolicy {
            value: opener_value(coop.as_ref(), enforced_compatible, false),
            reporting_endpoint: coop.and_then(|item| item.reporting_endpoint),
            report_only_value: opener_value(coop_report.as_ref(), monitored_compatible, true),
            report_only_reporting_endpoint: coop_report.and_then(|item| item.reporting_endpoint),
        };
        Self { opener, embedder }
    }
}

fn embedder_value(item: Option<&headers::PolicyField>) -> EmbedderPolicyValue {
    match item.map(|item| item.token.as_str()) {
        Some("require-corp") => EmbedderPolicyValue::RequireCorp,
        Some("credentialless") => EmbedderPolicyValue::Credentialless,
        _ => EmbedderPolicyValue::UnsafeNone,
    }
}

fn embedder_endpoint(item: Option<&headers::PolicyField>) -> String {
    item.filter(|item| embedder_value(Some(item)).is_isolation_compatible())
        .and_then(|item| item.reporting_endpoint.clone())
        .unwrap_or_default()
}

fn opener_value(
    item: Option<&headers::PolicyField>,
    compatible_coep: bool,
    report_only: bool,
) -> OpenerPolicyValue {
    match item.map(|item| item.token.as_str()) {
        Some("same-origin") if compatible_coep => OpenerPolicyValue::SameOriginPlusCoep,
        Some("same-origin") => OpenerPolicyValue::SameOrigin,
        Some("same-origin-allow-popups") => OpenerPolicyValue::SameOriginAllowPopups,
        Some("noopener-allow-popups") if !report_only => OpenerPolicyValue::NoopenerAllowPopups,
        _ => OpenerPolicyValue::UnsafeNone,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(fields: &[(&str, &str)], secure: bool) -> IsolationPolicy {
        let headers = fields
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<Vec<_>>();
        IsolationPolicy::from_response_headers(&headers, secure)
    }

    #[test]
    fn nonsecure_reserved_environment_ignores_all_isolation_fields() {
        let fields = [
            (
                "Cross-Origin-Opener-Policy",
                "same-origin; report-to=\"coop\"",
            ),
            (
                "Cross-Origin-Embedder-Policy",
                "require-corp; report-to=\"coep\"",
            ),
            ("Cross-Origin-Opener-Policy-Report-Only", "same-origin"),
            ("Cross-Origin-Embedder-Policy-Report-Only", "credentialless"),
        ];
        assert_eq!(parse(&fields, false), IsolationPolicy::default());
        let policy = parse(&fields, true);
        assert_eq!(policy.opener.value, OpenerPolicyValue::SameOriginPlusCoep);
        assert_eq!(policy.opener.reporting_endpoint.as_deref(), Some("coop"));
        assert_eq!(policy.embedder.reporting_endpoint, "coep");
    }

    #[test]
    fn report_only_coep_upgrades_only_report_only_coop() {
        let policy = parse(
            &[
                ("Cross-Origin-Opener-Policy", "same-origin"),
                ("Cross-Origin-Opener-Policy-Report-Only", "same-origin"),
                ("Cross-Origin-Embedder-Policy-Report-Only", "credentialless"),
            ],
            true,
        );
        assert_eq!(policy.opener.value, OpenerPolicyValue::SameOrigin);
        assert_eq!(
            policy.opener.report_only_value,
            OpenerPolicyValue::SameOriginPlusCoep
        );
        assert_eq!(policy.embedder.value, EmbedderPolicyValue::UnsafeNone);
    }

    #[test]
    fn enforced_coep_also_upgrades_report_only_coop() {
        for coep in ["require-corp", "credentialless"] {
            let policy = parse(
                &[
                    ("cross-origin-opener-policy-report-only", "same-origin"),
                    ("cross-origin-embedder-policy", coep),
                ],
                true,
            );
            assert_eq!(policy.opener.value, OpenerPolicyValue::UnsafeNone);
            assert_eq!(
                policy.opener.report_only_value,
                OpenerPolicyValue::SameOriginPlusCoep
            );
        }
    }

    #[test]
    fn malformed_fields_default_independently_without_erasing_valid_monitoring() {
        let policy = parse(
            &[
                ("cross-origin-opener-policy", "same-origin"),
                ("Cross-Origin-Opener-Policy", "same-origin"),
                ("cross-origin-embedder-policy", "require-corp;"),
                ("cross-origin-embedder-policy-report-only", "require-corp"),
            ],
            true,
        );
        assert_eq!(policy.opener, OpenerPolicy::default());
        assert_eq!(policy.embedder.value, EmbedderPolicyValue::UnsafeNone);
        assert_eq!(
            policy.embedder.report_only_value,
            EmbedderPolicyValue::RequireCorp
        );
    }

    #[test]
    fn opener_tokens_and_endpoint_types_follow_the_processing_model() {
        for (token, expected) in [
            ("unsafe-none", OpenerPolicyValue::UnsafeNone),
            ("same-origin", OpenerPolicyValue::SameOrigin),
            (
                "same-origin-allow-popups",
                OpenerPolicyValue::SameOriginAllowPopups,
            ),
            (
                "noopener-allow-popups",
                OpenerPolicyValue::NoopenerAllowPopups,
            ),
            ("same-origin-plus-COEP", OpenerPolicyValue::UnsafeNone),
            ("Same-origin", OpenerPolicyValue::UnsafeNone),
        ] {
            let policy = parse(&[("cross-origin-opener-policy", token)], true);
            assert_eq!(policy.opener.value, expected, "{token}");
        }
        let policy = parse(
            &[
                (
                    "cross-origin-opener-policy-report-only",
                    "noopener-allow-popups; report-to=\"r\"",
                ),
                (
                    "cross-origin-embedder-policy",
                    "unsafe-none; report-to=\"ignored\"",
                ),
            ],
            true,
        );
        assert_eq!(
            policy.opener.report_only_value,
            OpenerPolicyValue::UnsafeNone
        );
        assert_eq!(
            policy.opener.report_only_reporting_endpoint.as_deref(),
            Some("r")
        );
        assert_eq!(policy.embedder.reporting_endpoint, "");
    }

    #[test]
    fn unsafe_none_matches_across_origins_but_restrictive_policies_do_not() {
        use OpenerPolicyValue::*;
        assert!(UnsafeNone.matches(UnsafeNone, false));
        for value in [
            SameOrigin,
            SameOriginAllowPopups,
            SameOriginPlusCoep,
            NoopenerAllowPopups,
        ] {
            assert!(value.matches(value, true));
            assert!(!value.matches(value, false));
            assert!(!value.matches(UnsafeNone, true));
            assert!(!UnsafeNone.matches(value, true));
        }
        assert!(!SameOrigin.matches(SameOriginPlusCoep, true));
    }

    #[test]
    fn initial_popup_exceptions_do_not_apply_to_later_navigation() {
        use OpenerPolicyValue::*;
        for active in [SameOriginAllowPopups, NoopenerAllowPopups] {
            for same_origin in [false, true] {
                assert!(!active.requires_group_switch(UnsafeNone, same_origin, true));
                assert!(active.requires_group_switch(UnsafeNone, same_origin, false));
            }
        }
        assert!(SameOrigin.requires_group_switch(UnsafeNone, true, true));
        assert!(NoopenerAllowPopups.requires_group_switch(NoopenerAllowPopups, true, true));
        assert!(!NoopenerAllowPopups.requires_group_switch(NoopenerAllowPopups, true, false));
    }

    #[test]
    fn matching_monitoring_policies_suppress_report_only_group_switches() {
        let active = OpenerPolicy {
            report_only_value: OpenerPolicyValue::SameOrigin,
            ..OpenerPolicy::default()
        };
        assert_eq!(
            active.compare_navigation(&active, true, false),
            GroupSwitchDecision {
                enforce: false,
                report_only: false
            }
        );
        assert_eq!(
            active.compare_navigation(&OpenerPolicy::default(), true, false),
            GroupSwitchDecision {
                enforce: false,
                report_only: true
            }
        );
        let enforced = OpenerPolicy {
            value: OpenerPolicyValue::SameOrigin,
            ..OpenerPolicy::default()
        };
        assert_eq!(
            OpenerPolicy::default().compare_navigation(&enforced, true, false),
            GroupSwitchDecision {
                enforce: true,
                report_only: false
            }
        );
    }

    #[test]
    fn report_only_comparison_checks_both_enforced_monitoring_pairs() {
        let active = OpenerPolicy {
            value: OpenerPolicyValue::SameOrigin,
            report_only_value: OpenerPolicyValue::SameOriginAllowPopups,
            ..OpenerPolicy::default()
        };
        let response = OpenerPolicy {
            value: active.report_only_value,
            report_only_value: active.value,
            ..OpenerPolicy::default()
        };
        // The monitored policies differ, but each matches the opposite
        // enforced policy; neither mixed comparison requests reporting.
        assert_eq!(
            active.compare_navigation(&response, true, false),
            GroupSwitchDecision {
                enforce: true,
                report_only: false
            }
        );
    }

    #[test]
    fn either_compatible_child_policy_satisfies_coep_inheritance() {
        use EmbedderPolicyValue::*;
        let parent = EmbedderPolicy {
            value: RequireCorp,
            report_only_value: Credentialless,
            ..EmbedderPolicy::default()
        };
        assert_eq!(
            parent.check_inheritance(UnsafeNone),
            EmbedderInheritanceDecision {
                blocked: true,
                report_only_violation: true
            }
        );
        for child in [RequireCorp, Credentialless] {
            assert_eq!(
                parent.check_inheritance(child),
                EmbedderInheritanceDecision {
                    blocked: false,
                    report_only_violation: false
                }
            );
        }
        let monitoring = EmbedderPolicy {
            value: UnsafeNone,
            ..parent
        };
        assert_eq!(
            monitoring.check_inheritance(UnsafeNone),
            EmbedderInheritanceDecision {
                blocked: false,
                report_only_violation: true
            }
        );
    }
}
