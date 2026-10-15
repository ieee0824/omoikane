//! Content Security Policy parsing and source-expression matching.
//!
//! This module intentionally implements the small enforced-policy core used by
//! the page runtime.  It keeps policy state independent from the JavaScript
//! global so a navigation can install a fresh policy before the first script
//! runs, while the matching rules remain easy to exercise in focused tests.

use std::collections::BTreeMap;

use base64::Engine as _;
use serde::Serialize;
use sha2::{Digest, Sha256, Sha384, Sha512};

use crate::dom::{Node, NodeHandle, NodeType};
use crate::http::Url;

/// A fetchable resource class covered by the CSP core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceType {
    Script,
    Worker,
    Style,
    Connect,
    Font,
    Image,
}

/// Runtime references representing inline code rather than a fetch request.
pub(crate) fn is_inline_reference(reference: &str) -> bool {
    reference == "inline"
        || reference.starts_with("style-element:")
        || reference.starts_with("style-attribute:")
}

/// Snapshots an inline report sample without trimming its source text.
/// This implementation counts up to 40 Unicode scalar values.
pub(crate) fn inline_report_sample(source: &str) -> String {
    source.chars().take(40).collect()
}

impl ResourceType {
    pub(crate) fn directive(self) -> &'static str {
        match self {
            Self::Script => "script-src",
            Self::Worker => "worker-src",
            Self::Style => "style-src",
            Self::Connect => "connect-src",
            Self::Font => "font-src",
            Self::Image => "img-src",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Worker => "worker",
            Self::Style => "style",
            Self::Connect => "connect",
            Self::Font => "font",
            Self::Image => "image",
        }
    }
}

/// A violation retained by the owning Document until the embedder observes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CspViolation {
    pub(crate) document_id: usize,
    pub(crate) effective_directive: String,
    pub(crate) blocked_uri: String,
    pub(crate) resource_type: String,
    pub(crate) source: String,
}

/// An owned violation descriptor for one policy and its selected source list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicyViolation {
    pub(crate) serialized: String,
    pub(crate) report_sample: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceExpression {
    None,
    SelfKeyword,
    UnsafeInline,
    UnsafeHashes,
    StrictDynamic,
    Nonce(String),
    Hash(HashAlgorithm, String),
    UnsafeEval,
    TrustedTypesEval,
    ReportSample,
    Any,
    Scheme(String),
    Host {
        scheme: Option<String>,
        host: String,
        port: Option<u16>,
        path: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HashAlgorithm {
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlgorithm {
    fn digest(self, source: &str) -> Vec<u8> {
        match self {
            Self::Sha256 => Sha256::digest(source.as_bytes()).to_vec(),
            Self::Sha384 => Sha384::digest(source.as_bytes()).to_vec(),
            Self::Sha512 => Sha512::digest(source.as_bytes()).to_vec(),
        }
    }
}

/// The inline sink selects both its directive fallback and hash/nonce rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InlineType {
    Script,
    DynamicScript,
    ScriptAttribute,
    Style,
    StyleAttribute,
    Navigation,
}

impl InlineType {
    fn resource_type(self) -> ResourceType {
        match self {
            Self::Script | Self::DynamicScript | Self::ScriptAttribute | Self::Navigation => {
                ResourceType::Script
            }
            Self::Style | Self::StyleAttribute => ResourceType::Style,
        }
    }

    fn is_attribute(self) -> bool {
        matches!(self, Self::ScriptAttribute | Self::StyleAttribute)
    }

    fn permits_nonce(self) -> bool {
        matches!(self, Self::Script | Self::DynamicScript | Self::Style)
    }

    fn requires_unsafe_hashes(self) -> bool {
        matches!(
            self,
            Self::ScriptAttribute | Self::StyleAttribute | Self::Navigation
        )
    }

    fn is_script(self) -> bool {
        matches!(
            self,
            Self::Script | Self::DynamicScript | Self::ScriptAttribute | Self::Navigation
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ParsedPolicy {
    directives: BTreeMap<String, Vec<SourceExpression>>,
    trusted_types: Option<TrustedTypesDirective>,
    require_trusted_types_for_script: bool,
    serialized: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TrustedTypesDirective {
    names: Vec<String>,
    wildcard: bool,
    allow_duplicates: bool,
}

impl TrustedTypesDirective {
    fn allows_policy(&self, name: &str, already_created: bool) -> bool {
        (!already_created || self.allow_duplicates)
            && (self.wildcard || self.names.iter().any(|allowed| allowed == name))
    }
}

/// The policies attached to one Document.
///
/// Each response-header policy and each parser-initial CSP meta policy is kept
/// separately.  `allows_*` evaluates every policy, which gives the required
/// AND semantics without losing the source of a violation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CspPolicy {
    policies: Vec<ParsedPolicy>,
    report_only: Vec<ParsedPolicy>,
    reporting_endpoints: BTreeMap<String, String>,
    base_url: Option<Url>,
}

impl CspPolicy {
    /// Returns enforced and report-only policies rejecting a policy name.
    pub(crate) fn trusted_type_policy_creation_violations(
        &self,
        name: &str,
        already_created: bool,
    ) -> (Vec<PolicyViolation>, Vec<PolicyViolation>) {
        let violations = |policies: &[ParsedPolicy]| {
            policies
                .iter()
                .filter(|policy| {
                    policy
                        .trusted_types
                        .as_ref()
                        .is_some_and(|directive| !directive.allows_policy(name, already_created))
                })
                .map(|policy| PolicyViolation {
                    serialized: policy.serialized.clone(),
                    report_sample: true,
                })
                .collect()
        };
        (violations(&self.policies), violations(&self.report_only))
    }

    /// Returns enforced and report-only policies requiring trusted script sinks.
    pub(crate) fn trusted_type_sink_violations(
        &self,
    ) -> (Vec<PolicyViolation>, Vec<PolicyViolation>) {
        let violations = |policies: &[ParsedPolicy]| {
            policies
                .iter()
                .filter(|policy| policy.require_trusted_types_for_script)
                .map(|policy| PolicyViolation {
                    serialized: policy.serialized.clone(),
                    report_sample: true,
                })
                .collect()
        };
        (violations(&self.policies), violations(&self.report_only))
    }

    /// Returns policies that disallow compiling strings as JavaScript.
    pub(crate) fn script_compilation_violations(
        &self,
    ) -> (Vec<PolicyViolation>, Vec<PolicyViolation>) {
        // CSP's dynamic-code check asks whether this global requires Trusted
        // Types once, outside the loop over individual source-list policies.
        let trusted_types_required = self
            .policies
            .iter()
            .any(|policy| policy.require_trusted_types_for_script);
        let violations = |policies: &[ParsedPolicy]| {
            policies
                .iter()
                .filter_map(|policy| {
                    let sources = policy
                        .directives
                        .get("script-src")
                        .or_else(|| policy.directives.get("default-src"))?;
                    let allowed = sources.contains(&SourceExpression::UnsafeEval)
                        || (trusted_types_required
                            && sources.contains(&SourceExpression::TrustedTypesEval));
                    (!allowed).then(|| PolicyViolation {
                        serialized: policy.serialized.clone(),
                        report_sample: sources.contains(&SourceExpression::ReportSample),
                    })
                })
                .collect()
        };
        (violations(&self.policies), violations(&self.report_only))
    }

    /// Returns CSP policies rejecting an inline event-handler attribute.
    /// These attributes use `script-src-attr` fallback, not `unsafe-eval`.
    pub(crate) fn script_attribute_violations(
        &self,
        source: &str,
    ) -> (Vec<PolicyViolation>, Vec<PolicyViolation>) {
        self.inline_policy_violations(InlineType::ScriptAttribute, source, None)
    }

    /// Returns policies whose inline script check rejects a javascript: URL.
    /// The URL is executed as inline code, so its source is checked using the
    /// script element directive (and its ordinary fallback chain).
    pub(crate) fn javascript_navigation_violations(
        &self,
        full_url: &str,
    ) -> (Vec<PolicyViolation>, Vec<PolicyViolation>) {
        self.inline_policy_violations(InlineType::Navigation, full_url, None)
    }

    /// Returns the policies rejecting this exact UTF-8 inline source.
    /// Navigation hashes use the serialized full javascript: URL, while
    /// attribute hashes require an explicit unsafe-hashes keyword.
    pub(crate) fn inline_policy_violations(
        &self,
        kind: InlineType,
        source: &str,
        nonce: Option<&str>,
    ) -> (Vec<PolicyViolation>, Vec<PolicyViolation>) {
        let violations = |policies: &[ParsedPolicy]| {
            policies
                .iter()
                .filter(|policy| !policy.allows_inline_source(kind, Some(source), nonce))
                .map(|policy| PolicyViolation {
                    serialized: policy.serialized.clone(),
                    report_sample: policy
                        .sources_for(kind.resource_type(), kind.is_attribute())
                        .is_some_and(|sources| sources.contains(&SourceExpression::ReportSample)),
                })
                .collect()
        };
        (violations(&self.policies), violations(&self.report_only))
    }

    /// Compares the rules that affect resource checks and violation reports.
    /// Endpoint availability changes independently after report delivery.
    pub(crate) fn has_same_policy_rules(&self, other: &Self) -> bool {
        self.policies == other.policies
            && self.report_only == other.report_only
            && self.base_url == other.base_url
    }

    pub(crate) fn from_response_headers_and_document(
        headers: &[(String, String)],
        document: &NodeHandle,
        base_url: &str,
    ) -> Self {
        let values = |name: &str| {
            headers
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
                .collect::<Vec<_>>()
        };
        let mut policy =
            Self::from_headers_and_document(&values("content-security-policy"), document, base_url)
                .with_report_only_headers(&values("content-security-policy-report-only"));
        policy.reporting_endpoints = super::security_reporting::parse_reporting_endpoints(
            &values("reporting-endpoints").join(", "),
            base_url,
        );
        policy
    }
    pub(crate) fn reporting_endpoint(&self, name: &str) -> Option<String> {
        self.reporting_endpoints.get(name).cloned()
    }
    pub(crate) fn remove_reporting_endpoint(&mut self, name: &str) {
        self.reporting_endpoints.remove(name);
    }

    /// Copies CSP into a new global without inheriting its reporting endpoints.
    /// Reporting endpoint lists belong to each Window or Worker global and
    /// are initialized from that global's response headers.
    pub(crate) fn inherited_for_new_global(&self) -> Self {
        let mut inherited = self.clone();
        inherited.reporting_endpoints.clear();
        inherited
    }

    /// Keeps inherited directives while giving an opaque Document no tuple
    /// origin for `'self'` matching.
    pub(crate) fn inherited_for_opaque_document(&self) -> Self {
        let mut inherited = self.inherited_for_new_global();
        inherited.base_url = None;
        inherited
    }

    /// Adds response report-only policies without affecting enforcement.
    pub(crate) fn with_report_only_headers(mut self, headers: &[String]) -> Self {
        self.report_only
            .extend(headers.iter().filter_map(|header| parse_policy(header)));
        self
    }

    /// Reports violations of monitoring policies without authorizing a block.
    pub(crate) fn report_only_policy_texts(
        &self,
        resource_type: ResourceType,
        reference: &str,
    ) -> Vec<PolicyViolation> {
        self.report_only_policy_texts_after_redirects(resource_type, reference, 0)
    }

    /// Reports monitoring-policy violations after applying redirect matching.
    pub(crate) fn report_only_policy_texts_after_redirects(
        &self,
        resource_type: ResourceType,
        reference: &str,
        redirects: usize,
    ) -> Vec<PolicyViolation> {
        self.violating_policy_texts(&self.report_only, resource_type, reference, redirects, None)
    }

    /// Describes each policy blocking this resource, including sample opt-in.
    /// Evaluation only depends on this snapshot and the explicit resource.
    pub(crate) fn blocking_policy_texts_after_redirects(
        &self,
        resource_type: ResourceType,
        reference: &str,
        redirects: usize,
    ) -> Vec<PolicyViolation> {
        self.violating_policy_texts(&self.policies, resource_type, reference, redirects, None)
    }

    /// Reports only script policies that reject both the URL/inline source
    /// and the script element's nonce.
    pub(crate) fn script_policy_violations_with_nonce(
        &self,
        reference: &str,
        redirects: usize,
        nonce: Option<&str>,
        report_only: bool,
    ) -> Vec<PolicyViolation> {
        let policies = if report_only {
            &self.report_only
        } else {
            &self.policies
        };
        self.violating_policy_texts(policies, ResourceType::Script, reference, redirects, nonce)
    }

    fn violating_policy_texts(
        &self,
        policies: &[ParsedPolicy],
        resource_type: ResourceType,
        reference: &str,
        redirects: usize,
        nonce: Option<&str>,
    ) -> Vec<PolicyViolation> {
        let inline = is_inline_reference(reference);
        let attribute =
            resource_type == ResourceType::Style && reference.starts_with("style-attribute:");
        let url = ResourceUrl::parse(reference, self.base_url.as_ref());
        policies
            .iter()
            .filter_map(|policy| {
                let violated = if policy.sources_for(resource_type, attribute).is_none() {
                    false
                } else if resource_type == ResourceType::Script && policy.allows_script_nonce(nonce)
                {
                    false
                } else if inline {
                    !policy.allows_inline(resource_type, attribute)
                } else {
                    url.as_ref().is_none_or(|url| {
                        !policy.allows_url(resource_type, &self.base_url, url, redirects)
                    })
                };
                violated.then(|| PolicyViolation {
                    serialized: policy.serialized.clone(),
                    report_sample: inline
                        && policy
                            .sources_for(resource_type, attribute)
                            .is_some_and(|sources| {
                                sources.contains(&SourceExpression::ReportSample)
                            }),
                })
            })
            .collect()
    }

    pub(crate) fn from_headers_and_document(
        headers: &[String],
        document: &NodeHandle,
        base_url: &str,
    ) -> Self {
        let mut policies = headers
            .iter()
            .filter_map(|header| parse_policy(header))
            .collect::<Vec<_>>();

        // A CSP meta element only contributes when it uses the exact
        // http-equiv value.  The policy is captured once during navigation;
        // later insertion of a meta element cannot retroactively change it.
        let mut meta_contents = Vec::new();
        collect_csp_meta_contents(document, &mut meta_contents);
        policies.extend(
            meta_contents
                .iter()
                .filter_map(|content| parse_policy(content)),
        );

        Self {
            policies,
            report_only: Vec::new(),
            reporting_endpoints: BTreeMap::new(),
            base_url: base_url.parse::<Url>().ok(),
        }
    }

    /// Limits parser-time enforcement to meta policies before this script.
    /// Response policies and inherited policies occupy the prefix; this
    /// Document's parsed meta policies occupy the suffix in document order.
    pub(crate) fn for_parser_script(&self, document: &NodeHandle, script: &NodeHandle) -> Self {
        let mut counts = ParserMetaCounts::default();
        count_parser_meta_policies(document, script.identity(), &mut counts);
        let mut policy = self.clone();
        if counts.found_script && counts.total <= policy.policies.len() {
            policy
                .policies
                .truncate(policy.policies.len() - counts.total + counts.before_script);
        }
        policy
    }

    /// Changes only parser-time enforcement, preserving live endpoint state.
    pub(crate) fn use_parser_policies_from(&mut self, policy: &Self) {
        self.policies.clone_from(&policy.policies);
    }

    /// Adds this srcdoc Document's policies to an inherited snapshot. The
    /// parent and sibling policy lists remain independent.
    pub(crate) fn with_document_meta(&self, document: &NodeHandle, base_url: &str) -> Self {
        let own = Self::from_headers_and_document(&[], document, base_url);
        let mut combined = self.clone();
        combined.policies.extend(own.policies);
        if combined.base_url.is_none() {
            combined.base_url = own.base_url;
        }
        combined
    }

    pub(crate) fn allows_inline(&self, resource_type: ResourceType) -> bool {
        self.policies
            .iter()
            .all(|policy| policy.allows_inline(resource_type, false))
    }

    /// Checks the complete inline text against every enforcing policy.
    pub(crate) fn allows_inline_source(&self, kind: InlineType, source: &str) -> bool {
        self.allows_inline_source_with_nonce(kind, source, None)
    }

    /// Only inline script and style elements can use an element nonce.
    pub(crate) fn allows_inline_source_with_nonce(
        &self,
        kind: InlineType,
        source: &str,
        nonce: Option<&str>,
    ) -> bool {
        self.policies
            .iter()
            .all(|policy| policy.allows_inline_source(kind, Some(source), nonce))
    }

    /// Checks inline attributes using attribute-specific directive fallbacks.
    pub(crate) fn allows_inline_attribute(&self, resource_type: ResourceType) -> bool {
        self.policies
            .iter()
            .all(|policy| policy.allows_inline(resource_type, true))
    }

    pub(crate) fn allows_url(&self, resource_type: ResourceType, url: &Url) -> bool {
        self.allows_url_after_redirects(resource_type, url, 0)
    }

    pub(crate) fn allows_url_after_redirects(
        &self,
        resource_type: ResourceType,
        url: &Url,
        redirects: usize,
    ) -> bool {
        self.policies.iter().all(|policy| {
            policy.allows_url(
                resource_type,
                &self.base_url,
                &ResourceUrl::from(url),
                redirects,
            )
        })
    }

    /// Checks an HTML/network reference before it is fetched.  This accepts
    /// websocket schemes and data URLs in addition to the HTTP URL type used by
    /// the network client.
    pub(crate) fn allows_reference(&self, resource_type: ResourceType, reference: &str) -> bool {
        self.allows_reference_after_redirects(resource_type, reference, 0)
    }

    pub(crate) fn allows_reference_after_redirects(
        &self,
        resource_type: ResourceType,
        reference: &str,
        redirects: usize,
    ) -> bool {
        let Some(resource_url) = ResourceUrl::parse(reference, self.base_url.as_ref()) else {
            // With no base URL (for example, an opaque `data:` Document), a
            // relative reference cannot be checked against the policy. A
            // document with an enforced policy must fail closed here; without
            // CSP, leave the eventual resource-loader error unchanged.
            return self.policies.is_empty();
        };
        self.policies.iter().all(|policy| {
            policy.allows_url(resource_type, &self.base_url, &resource_url, redirects)
        })
    }
}

impl ParsedPolicy {
    fn sources_for(
        &self,
        resource_type: ResourceType,
        attribute: bool,
    ) -> Option<&[SourceExpression]> {
        let fallbacks: &[&str] = match (resource_type, attribute) {
            (ResourceType::Script, false) => &["script-src-elem", "script-src", "default-src"],
            (ResourceType::Script, true) => &["script-src-attr", "script-src", "default-src"],
            (ResourceType::Worker, _) => &["worker-src", "child-src", "script-src", "default-src"],
            (ResourceType::Style, false) => &["style-src-elem", "style-src", "default-src"],
            (ResourceType::Style, true) => &["style-src-attr", "style-src", "default-src"],
            (ResourceType::Connect, _) => &["connect-src", "default-src"],
            (ResourceType::Font, _) => &["font-src", "default-src"],
            (ResourceType::Image, _) => &["img-src", "default-src"],
        };
        fallbacks
            .iter()
            .find_map(|name| self.directives.get(*name))
            .map(Vec::as_slice)
    }

    fn allows_inline(&self, resource_type: ResourceType, attribute: bool) -> bool {
        let kind = match (resource_type, attribute) {
            (ResourceType::Script, false) => InlineType::Script,
            (ResourceType::Script, true) => InlineType::ScriptAttribute,
            (ResourceType::Style, false) => InlineType::Style,
            (ResourceType::Style, true) => InlineType::StyleAttribute,
            _ => return false,
        };
        self.allows_inline_source(kind, None, None)
    }

    fn allows_inline_source(
        &self,
        kind: InlineType,
        source: Option<&str>,
        nonce: Option<&str>,
    ) -> bool {
        let Some(sources) = self.sources_for(kind.resource_type(), kind.is_attribute()) else {
            return true;
        };
        let overrides_unsafe_inline = sources.iter().any(|expression| {
            matches!(
                expression,
                SourceExpression::Nonce(_) | SourceExpression::Hash(..)
            ) || (kind.is_script() && matches!(expression, SourceExpression::StrictDynamic))
        });
        if !overrides_unsafe_inline && sources.contains(&SourceExpression::UnsafeInline) {
            return true;
        }
        if kind.permits_nonce()
            && nonce.is_some_and(|nonce| {
                sources.iter().any(|expression| {
                    matches!(expression, SourceExpression::Nonce(expected) if expected == nonce)
                })
            })
        {
            return true;
        }
        if kind == InlineType::DynamicScript && sources.contains(&SourceExpression::StrictDynamic) {
            return true;
        }
        let Some(source) = source else {
            return false;
        };
        if kind.requires_unsafe_hashes() && !sources.contains(&SourceExpression::UnsafeHashes) {
            return false;
        }
        sources.iter().any(|expression| match expression {
            SourceExpression::Hash(algorithm, expected) => {
                base64::engine::general_purpose::STANDARD.encode(algorithm.digest(source))
                    == *expected
            }
            _ => false,
        })
    }

    fn allows_script_nonce(&self, nonce: Option<&str>) -> bool {
        let Some(nonce) = nonce.filter(|nonce| !nonce.is_empty()) else {
            return false;
        };
        self.sources_for(ResourceType::Script, false)
            .is_some_and(|sources| {
                sources.iter().any(|source| {
                    matches!(source, SourceExpression::Nonce(expected) if expected == nonce)
                })
            })
    }

    fn allows_url(
        &self,
        resource_type: ResourceType,
        base_url: &Option<Url>,
        url: &ResourceUrl,
        redirects: usize,
    ) -> bool {
        let Some(sources) = self.sources_for(resource_type, false) else {
            return true;
        };

        // `'none'` is only an absolute deny when it is the sole source
        // expression.  With other expressions present it is ignored per CSP.
        let has_non_none = sources
            .iter()
            .any(|source| !matches!(source, SourceExpression::None));
        if !has_non_none {
            return false;
        }

        sources.iter().any(|source| match source {
            SourceExpression::None => false,
            SourceExpression::UnsafeInline
            | SourceExpression::UnsafeHashes
            | SourceExpression::StrictDynamic
            | SourceExpression::Nonce(_)
            | SourceExpression::Hash(..)
            | SourceExpression::UnsafeEval
            | SourceExpression::TrustedTypesEval
            | SourceExpression::ReportSample => false,
            SourceExpression::Any => matches!(url.scheme.as_str(), "http" | "https" | "ws" | "wss"),
            SourceExpression::SelfKeyword => base_url
                .as_ref()
                .map(ResourceUrl::from)
                .is_some_and(|base| base.same_origin(url)),
            SourceExpression::Scheme(scheme) => scheme_matches(scheme, &url.scheme),
            SourceExpression::Host {
                scheme,
                host,
                port,
                path,
            } => {
                let expected_scheme = scheme.clone().or_else(|| {
                    base_url
                        .as_ref()
                        .map(ResourceUrl::from)
                        .map(|base| base.scheme)
                });
                if expected_scheme
                    .as_deref()
                    .is_some_and(|expected| !scheme_matches(expected, &url.scheme))
                {
                    return false;
                }
                if !host_matches(host, &url.host) {
                    return false;
                }
                if port.is_some_and(|expected| expected != url.port) {
                    return false;
                }
                // CSP3 ignores a host source's path after a redirect, while
                // retaining its scheme, host and port restrictions above.
                redirects > 0
                    || path
                        .as_ref()
                        .is_none_or(|source| path_part_matches(source, &url.path))
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResourceUrl {
    scheme: String,
    host: String,
    port: u16,
    path: String,
}

impl ResourceUrl {
    fn from(url: &Url) -> Self {
        Self {
            scheme: url.scheme().to_ascii_lowercase(),
            host: url.host().to_ascii_lowercase(),
            port: url.port(),
            path: url.path().to_string(),
        }
    }

    fn parse(reference: &str, base_url: Option<&Url>) -> Option<Self> {
        let reference = reference.trim();
        if let Some(scheme) = reference.get(..5).and_then(|prefix| {
            ["data:", "blob:"]
                .into_iter()
                .find(|scheme| prefix.eq_ignore_ascii_case(scheme))
        }) {
            return Some(Self {
                scheme: scheme.trim_end_matches(':').to_string(),
                host: String::new(),
                port: 0,
                path: String::new(),
            });
        }

        let lower_reference = reference.to_ascii_lowercase();
        let original_scheme = if lower_reference.starts_with("ws://") {
            Some("ws")
        } else if lower_reference.starts_with("wss://") {
            Some("wss")
        } else {
            None
        };
        let normalized = match original_scheme {
            Some("ws") => format!("http{}", &reference[2..]),
            Some("wss") => format!("https{}", &reference[3..]),
            _ => reference.to_string(),
        };
        let normalized_lower = normalized.to_ascii_lowercase();
        let url = if normalized_lower.starts_with("http://")
            || normalized_lower.starts_with("https://")
        {
            normalized.parse::<Url>().ok()?
        } else {
            let base = base_url?;
            crate::http::url::resolve_url(base, &normalized).ok()?
        };
        let mut resource = Self::from(&url);
        if let Some(scheme) = original_scheme {
            resource.scheme = scheme.to_string();
        }
        Some(resource)
    }

    fn same_origin(&self, other: &Self) -> bool {
        origin_scheme_matches(&self.scheme, &other.scheme)
            && self.host.eq_ignore_ascii_case(&other.host)
            && self.port == other.port
    }
}

/// CSP3 path-part matching splits before percent-decoding. An encoded slash
/// belongs to one component and cannot turn into a directory boundary.
fn path_part_matches(source: &str, target: &str) -> bool {
    if source.is_empty() || (source == "/" && target.is_empty()) {
        return true;
    }
    let exact = !source.ends_with('/');
    let mut source_parts: Vec<_> = source.split('/').collect();
    let target_parts: Vec<_> = target.split('/').collect();
    if source_parts.len() > target_parts.len()
        || (exact && source_parts.len() != target_parts.len())
    {
        return false;
    }
    if !exact {
        source_parts.pop();
    }
    source_parts.iter().zip(&target_parts).all(|(left, right)| {
        crate::http::percent_decode_bytes(left) == crate::http::percent_decode_bytes(right)
    })
}

fn scheme_matches(expected: &str, actual: &str) -> bool {
    expected.eq_ignore_ascii_case(actual)
        || (expected.eq_ignore_ascii_case("http") && actual.eq_ignore_ascii_case("ws"))
        || (expected.eq_ignore_ascii_case("https") && actual.eq_ignore_ascii_case("wss"))
}

fn origin_scheme_matches(left: &str, right: &str) -> bool {
    scheme_matches(left, right) || scheme_matches(right, left)
}

fn host_matches(expected: &str, actual: &str) -> bool {
    let expected = expected.to_ascii_lowercase();
    let actual = actual.to_ascii_lowercase();
    if let Some(suffix) = expected.strip_prefix("*.") {
        actual != suffix && actual.ends_with(&format!(".{suffix}"))
    } else {
        expected == actual
    }
}

fn parse_policy(input: &str) -> Option<ParsedPolicy> {
    let mut policy = ParsedPolicy {
        serialized: input.trim().to_owned(),
        ..ParsedPolicy::default()
    };
    for directive in input.split(';') {
        let mut tokens = directive.split_ascii_whitespace();
        let Some(name) = tokens.next() else {
            continue;
        };
        let name = name.to_ascii_lowercase();
        if policy.directives.contains_key(&name) {
            // Duplicate directives are ignored after the first occurrence.
            continue;
        }
        let values = tokens.collect::<Vec<_>>();
        if name == "trusted-types" {
            policy.trusted_types = Some(parse_trusted_types_directive(&values));
        } else if name == "require-trusted-types-for" {
            policy.require_trusted_types_for_script = values.contains(&"'script'");
        }
        let sources = values
            .into_iter()
            .filter_map(parse_source_expression)
            .collect();
        policy.directives.insert(name, sources);
    }
    (!policy.directives.is_empty()).then_some(policy)
}

fn parse_trusted_types_directive(values: &[&str]) -> TrustedTypesDirective {
    let mut directive = TrustedTypesDirective::default();
    for value in values {
        match *value {
            "*" => directive.wildcard = true,
            "'allow-duplicates'" => directive.allow_duplicates = true,
            "'none'" => {}
            name if !name.is_empty()
                && name.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric()
                        || matches!(byte, b'-' | b'#' | b'=' | b'_' | b'/' | b'@' | b'.' | b'%')
                }) =>
            {
                directive.names.push(name.to_owned())
            }
            _ => {}
        }
    }
    directive
}

fn parse_source_expression(token: &str) -> Option<SourceExpression> {
    if let Some(nonce) = token
        .strip_prefix("'nonce-")
        .and_then(|value| value.strip_suffix('\''))
        && let core = nonce.trim_end_matches('=')
        && !core.is_empty()
        && nonce.len() - core.len() <= 2
        && core
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'-' | b'_'))
    {
        return Some(SourceExpression::Nonce(nonce.to_owned()));
    }
    if let Some(hash) = parse_hash_source(token) {
        return Some(hash);
    }
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "'none'" => return Some(SourceExpression::None),
        "'self'" => return Some(SourceExpression::SelfKeyword),
        "'unsafe-inline'" => return Some(SourceExpression::UnsafeInline),
        "'unsafe-hashes'" => return Some(SourceExpression::UnsafeHashes),
        "'strict-dynamic'" => return Some(SourceExpression::StrictDynamic),
        "'unsafe-eval'" => return Some(SourceExpression::UnsafeEval),
        "'trusted-types-eval'" => return Some(SourceExpression::TrustedTypesEval),
        "'report-sample'" => {
            // CSP's sample opt-in uses exact source-list membership, unlike
            // the explicitly case-insensitive unsafe-inline matching.
            return (token == "'report-sample'").then_some(SourceExpression::ReportSample);
        }
        "*" => return Some(SourceExpression::Any),
        _ => {}
    }

    if let Some(scheme) = lower.strip_suffix(':')
        && !scheme.is_empty()
        && !scheme.contains('/')
        // A dotted token ending in ':' is a malformed host:port expression
        // (for example, `example.com:`), not a useful scheme source.
        && !scheme.contains('.')
    {
        return Some(SourceExpression::Scheme(scheme.to_string()));
    }

    // Keep the original spelling for the path component: host and scheme
    // matching are ASCII case-insensitive, but CSP host-source paths are
    // case-sensitive.
    let (scheme, authority_and_path) = if let Some(index) = lower.find("://") {
        (
            Some(token[..index].to_ascii_lowercase()),
            &token[index + 3..],
        )
    } else if lower.starts_with("//") {
        (None, &token[2..])
    } else {
        (None, token)
    };
    let (authority, path) = authority_and_path
        .split_once('/')
        .map_or((authority_and_path, None), |(authority, path)| {
            (authority, Some(format!("/{path}")))
        });
    if authority.is_empty() {
        return None;
    }
    let authority_lower = authority.to_ascii_lowercase();
    let (host, port) = if let Some(bracketed) = authority_lower.strip_prefix('[') {
        let end = bracketed.find(']')?;
        let host = &bracketed[..end];
        let rest = &bracketed[end + 1..];
        let port = match rest {
            "" => None,
            rest if rest.starts_with(':') => Some(rest[1..].parse::<u16>().ok()?),
            _ => return None,
        };
        (host.to_string(), port)
    } else if let Some((host, port)) = authority_lower.rsplit_once(':') {
        if host.is_empty() || port.is_empty() {
            return None;
        }
        (host.to_string(), Some(port.parse::<u16>().ok()?))
    } else {
        (authority_lower, None)
    };
    Some(SourceExpression::Host {
        scheme,
        host,
        port,
        path,
    })
}

fn parse_hash_source(token: &str) -> Option<SourceExpression> {
    let (algorithm, value) = token
        .strip_prefix('\'')?
        .strip_suffix('\'')?
        .split_once('-')?;
    let algorithm = match algorithm.to_ascii_lowercase().as_str() {
        "sha256" => HashAlgorithm::Sha256,
        "sha384" => HashAlgorithm::Sha384,
        "sha512" => HashAlgorithm::Sha512,
        _ => return None,
    };
    let unpadded = value.trim_end_matches('=');
    if unpadded.is_empty()
        || value.len() - unpadded.len() > 2
        || !unpadded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'-' | b'_'))
    {
        return None;
    }
    // CSP3 compares the canonical padded Base64 digest with this spelling
    // after only alphabet normalization. Valid grammar, including omitted
    // padding or an undecodable value, still suppresses 'unsafe-inline'.
    Some(SourceExpression::Hash(
        algorithm,
        value.replace('-', "+").replace('_', "/"),
    ))
}

fn csp_meta_content(node: &NodeHandle) -> Option<String> {
    if node.node_type() == NodeType::Element
        && node
            .tag_name()
            .is_some_and(|tag| tag.eq_ignore_ascii_case("meta"))
        && is_descendant_of_head(node)
        && node
            .get_attribute("http-equiv")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("content-security-policy"))
    {
        node.get_attribute("content")
    } else {
        None
    }
}

fn collect_csp_meta_contents(node: &NodeHandle, out: &mut Vec<String>) {
    if let Some(content) = csp_meta_content(node) {
        out.push(content);
    }
    for child in node.child_nodes() {
        collect_csp_meta_contents(&child, out);
    }
}

#[derive(Default)]
struct ParserMetaCounts {
    total: usize,
    before_script: usize,
    found_script: bool,
}

fn count_parser_meta_policies(node: &NodeHandle, script_id: usize, counts: &mut ParserMetaCounts) {
    if node.identity() == script_id {
        counts.found_script = true;
    }
    if csp_meta_content(node).is_some_and(|content| parse_policy(&content).is_some()) {
        counts.total += 1;
        if !counts.found_script {
            counts.before_script += 1;
        }
    }
    for child in node.child_nodes() {
        count_parser_meta_policies(&child, script_id, counts);
    }
}

fn is_descendant_of_head(node: &NodeHandle) -> bool {
    let mut ancestor = node.parent_node();
    while let Some(current) = ancestor {
        if current
            .tag_name()
            .is_some_and(|tag| tag.eq_ignore_ascii_case("head"))
        {
            return true;
        }
        ancestor = current.parent_node();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::TreeBuilder;

    fn make_policy(text: &str, base: &str) -> CspPolicy {
        CspPolicy {
            policies: vec![parse_policy(text).unwrap()],
            base_url: base.parse().ok(),
            report_only: Vec::new(),
            reporting_endpoints: BTreeMap::new(),
        }
    }

    #[test]
    fn trusted_types_directives_keep_names_and_dispositions_separate() {
        let policy = make_policy(
            "trusted-types App default; require-trusted-types-for 'script'; script-src 'self'",
            "https://example.test/",
        )
        .with_report_only_headers(&[
            "trusted-types Monitor; require-trusted-types-for 'script'".into(),
        ]);
        let (blocked, reported) = policy.trusted_type_policy_creation_violations("App", false);
        assert!(blocked.is_empty());
        assert_eq!(reported.len(), 1);
        let (blocked, reported) = policy.trusted_type_policy_creation_violations("app", false);
        assert_eq!((blocked.len(), reported.len()), (1, 1));
        let (blocked, reported) = policy.trusted_type_policy_creation_violations("App", true);
        assert_eq!((blocked.len(), reported.len()), (1, 1));
        let (blocked, reported) = policy.trusted_type_sink_violations();
        assert_eq!((blocked.len(), reported.len()), (1, 1));
    }

    #[test]
    fn trusted_types_wildcard_and_duplicates_follow_their_own_grammar() {
        let policy = make_policy(
            "trusted-types * 'allow-duplicates'",
            "https://example.test/",
        );
        assert!(
            policy
                .trusted_type_policy_creation_violations("any-name", true)
                .0
                .is_empty()
        );
        let policy = make_policy("trusted-types 'none'", "https://example.test/");
        assert_eq!(
            policy
                .trusted_type_policy_creation_violations("default", false)
                .0
                .len(),
            1
        );
        let policy = make_policy(
            "require-trusted-types-for 'invalid'; trusted-types Good@name bad!name",
            "https://example.test/",
        );
        assert!(policy.trusted_type_sink_violations().0.is_empty());
        assert!(
            policy
                .trusted_type_policy_creation_violations("Good@name", false)
                .0
                .is_empty()
        );
        assert_eq!(
            policy
                .trusted_type_policy_creation_violations("bad!name", false)
                .0
                .len(),
            1
        );
    }

    #[test]
    fn dynamic_compilation_uses_script_src_and_trusted_types_eval_only_with_requirement() {
        let base = "https://example.test/";
        let policy = make_policy(
            "default-src 'self'; script-src 'trusted-types-eval' 'report-sample'",
            base,
        );
        let (blocked, reported) = policy.script_compilation_violations();
        assert_eq!((blocked.len(), reported.len()), (1, 0));
        assert!(blocked[0].report_sample);

        let policy = make_policy(
            "default-src 'self'; script-src 'trusted-types-eval'; require-trusted-types-for 'script'",
            base,
        );
        assert!(policy.script_compilation_violations().0.is_empty());

        let mut policy = make_policy("require-trusted-types-for 'script'", base);
        policy
            .policies
            .push(parse_policy("script-src 'trusted-types-eval'").unwrap());
        assert!(policy.script_compilation_violations().0.is_empty());
        let policy = policy.with_report_only_headers(&["script-src 'trusted-types-eval'".into()]);
        assert!(policy.script_compilation_violations().1.is_empty());

        let policy = make_policy("default-src 'unsafe-eval'", base)
            .with_report_only_headers(&["script-src 'self' 'report-sample'".into()]);
        let (blocked, reported) = policy.script_compilation_violations();
        assert!(blocked.is_empty());
        assert_eq!(reported.len(), 1);
        assert!(reported[0].report_sample);
    }

    #[test]
    fn inline_handler_uses_script_src_attr_instead_of_dynamic_code_permission() {
        let policy = make_policy(
            "script-src 'unsafe-eval'; script-src-attr 'none' 'report-sample'",
            "https://example.test/",
        );
        let (blocked, reported) = policy.script_attribute_violations("doSubmit()");
        assert_eq!((blocked.len(), reported.len()), (1, 0));
        assert!(blocked[0].report_sample);
        assert!(policy.script_compilation_violations().0.is_empty());

        let policy = make_policy("script-src 'unsafe-inline'", "https://example.test/");
        assert!(
            policy
                .script_attribute_violations("doSubmit()")
                .0
                .is_empty()
        );
        assert_eq!(policy.script_compilation_violations().0.len(), 1);
    }

    #[test]
    fn default_src_falls_back_and_self_matches_case_insensitively() {
        let policy = make_policy("default-src 'self'", "https://Example.test/index.html");
        assert!(policy.allows_url(
            ResourceType::Script,
            &"https://example.test/a.js".parse().unwrap()
        ));
        assert!(!policy.allows_url(
            ResourceType::Script,
            &"https://other.test/a.js".parse().unwrap()
        ));
        assert!(policy.allows_url(
            ResourceType::Connect,
            &"https://EXAMPLE.TEST/api".parse().unwrap()
        ));
    }

    #[test]
    fn worker_src_uses_its_own_fallback_chain_not_script_src_elem() {
        let base = "https://example.test/page";
        let allowed = "https://example.test/worker.js";
        for text in [
            "worker-src 'none'; child-src 'self'; script-src 'self'",
            "child-src 'none'; script-src 'self'",
            "script-src 'none'; script-src-elem 'self'",
            "default-src 'none'; script-src-elem 'self'",
        ] {
            assert!(
                !make_policy(text, base).allows_reference(ResourceType::Worker, allowed),
                "{text}"
            );
        }
        for text in [
            "worker-src 'self'; child-src 'none'",
            "child-src 'self'; script-src 'none'",
            "script-src 'self'; default-src 'none'",
            "script-src 'self'; script-src-elem 'none'",
            "default-src 'self'",
        ] {
            assert!(
                make_policy(text, base).allows_reference(ResourceType::Worker, allowed),
                "{text}"
            );
        }
    }

    #[test]
    fn inline_requires_unsafe_inline() {
        assert!(
            !make_policy("script-src 'self'", "https://example.test/")
                .allows_inline(ResourceType::Script)
        );
        assert!(
            make_policy("script-src 'unsafe-inline'", "https://example.test/")
                .allows_inline(ResourceType::Script)
        );
    }

    #[test]
    fn script_nonce_allows_matching_elements_and_rejects_unsafe_inline_fallback() {
        let policy = make_policy(
            "script-src 'nonce-AbC+_=' 'unsafe-inline'",
            "https://example.test/",
        );
        for reference in ["inline", "https://other.test/script.js"] {
            assert!(
                policy
                    .script_policy_violations_with_nonce(reference, 0, Some("AbC+_="), false)
                    .is_empty()
            );
            assert_eq!(
                policy
                    .script_policy_violations_with_nonce(reference, 0, Some("abc+_="), false)
                    .len(),
                1
            );
            assert_eq!(
                policy
                    .script_policy_violations_with_nonce(reference, 0, None, false)
                    .len(),
                1
            );
        }
    }

    #[test]
    fn malformed_nonce_sources_do_not_disable_unsafe_inline() {
        for malformed in ["a=b", "a===", "=", "a b"] {
            let policy = make_policy(
                &format!("script-src 'nonce-{malformed}' 'unsafe-inline'"),
                "https://example.test/",
            );
            assert!(policy.allows_inline(ResourceType::Script), "{malformed}");
        }
    }

    #[test]
    fn inline_hash_sources_match_exact_utf8_and_canonical_base64_spelling() {
        use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};

        let source = "π();";
        for (name, digest) in [
            ("sha256", HashAlgorithm::Sha256.digest(source)),
            ("sha384", HashAlgorithm::Sha384.digest(source)),
            ("sha512", HashAlgorithm::Sha512.digest(source)),
        ] {
            for encoded in [STANDARD.encode(&digest), URL_SAFE.encode(&digest)] {
                let policy = make_policy(
                    &format!("script-src '{name}-{encoded}'"),
                    "https://example.test/",
                );
                assert!(policy.allows_inline_source(InlineType::Script, source));
                assert!(!policy.allows_inline_source(InlineType::Script, "π(); "));
                assert!(!policy.allows_inline_source(InlineType::Script, "λ();"));
            }
            let unpadded = URL_SAFE_NO_PAD.encode(&digest);
            let policy = make_policy(
                &format!("script-src 'unsafe-inline' '{name}-{unpadded}'"),
                "https://example.test/",
            );
            assert_eq!(
                policy.allows_inline_source(InlineType::Script, source),
                name == "sha384",
                "{name}: the canonical SHA-384 spelling needs no padding"
            );
            assert!(!policy.allows_inline_source(InlineType::Script, "other()"));
        }
    }

    #[test]
    fn malformed_hashes_cannot_suppress_or_authorize_unsafe_inline() {
        for invalid in [
            "'sha256-%%%'",
            "'sha256-abc=def'",
            "'sha384-'",
            "'sha512-AAAA==='",
            "'sha1-AAAA'",
        ] {
            let policy = make_policy(
                &format!("script-src 'unsafe-inline' {invalid}"),
                "https://example.test/",
            );
            assert!(
                policy.allows_inline_source(InlineType::Script, "anything"),
                "{invalid}"
            );
        }
    }

    #[test]
    fn syntactically_valid_nonmatching_hashes_still_suppress_unsafe_inline() {
        for hash in ["'sha256-Zg=='", "'sha256-A'", "'sha256-AA'"] {
            let policy = make_policy(
                &format!("script-src 'unsafe-inline' {hash}"),
                "https://example.test/",
            );
            assert!(
                !policy.allows_inline_source(InlineType::Script, "anything"),
                "{hash}"
            );
        }
    }

    #[test]
    fn unsafe_hashes_only_unlocks_matching_attribute_and_navigation_sources() {
        let source = "doSubmit()";
        let digest =
            base64::engine::general_purpose::STANDARD.encode(HashAlgorithm::Sha256.digest(source));
        let source_list = format!("'sha256-{digest}'");
        let policy = make_policy(
            &format!("script-src {source_list}"),
            "https://example.test/",
        );
        assert!(policy.allows_inline_source(InlineType::Script, source));
        assert_eq!(policy.script_attribute_violations(source).0.len(), 1);
        assert_eq!(policy.javascript_navigation_violations(source).0.len(), 1);

        let policy = make_policy(
            &format!("script-src 'unsafe-hashes' {source_list}"),
            "https://example.test/",
        );
        assert!(policy.script_attribute_violations(source).0.is_empty());
        assert_eq!(policy.script_attribute_violations("doSubmit();").0.len(), 1);
        assert!(policy.javascript_navigation_violations(source).0.is_empty());

        let style_policy = make_policy(
            &format!("style-src-attr 'unsafe-hashes' {source_list}"),
            "https://example.test/",
        );
        assert!(style_policy.allows_inline_source(InlineType::StyleAttribute, source));
        assert!(!style_policy.allows_inline_source(InlineType::StyleAttribute, "doSubmit();"));
    }

    #[test]
    fn navigation_hash_uses_full_url_and_selected_directive_per_policy() {
        let url = "javascript:navigated();";
        let digest =
            base64::engine::general_purpose::STANDARD.encode(HashAlgorithm::Sha256.digest(url));
        let source = format!("'unsafe-hashes' 'sha256-{digest}'");
        let policy = make_policy(
            &format!("script-src {source}; script-src-elem 'none'"),
            "https://example.test/",
        );
        assert_eq!(policy.javascript_navigation_violations(url).0.len(), 1);
        let policy = make_policy(&format!("default-src {source}"), "https://example.test/")
            .with_report_only_headers(&["script-src-elem 'none' 'report-sample'".into()]);
        let (enforced, reported) = policy.javascript_navigation_violations(url);
        assert!(enforced.is_empty());
        assert_eq!(reported.len(), 1);
        assert!(reported[0].report_sample);
        assert_eq!(
            policy
                .javascript_navigation_violations("navigated();")
                .0
                .len(),
            1
        );
    }

    #[test]
    fn hashes_nonces_and_strict_dynamic_override_unsafe_inline() {
        let source = "run()";
        let digest =
            base64::engine::general_purpose::STANDARD.encode(HashAlgorithm::Sha256.digest(source));
        for keyword in [
            format!("'sha256-{digest}'"),
            "'nonce-abc'".into(),
            "'strict-dynamic'".into(),
        ] {
            let policy = make_policy(
                &format!("script-src 'unsafe-inline' {keyword}"),
                "https://example.test/",
            );
            assert!(
                !policy.allows_inline_source(InlineType::Script, "other()"),
                "{keyword}"
            );
        }
        let policy = make_policy(
            "script-src 'unsafe-inline' 'nonce-abc'",
            "https://example.test/",
        );
        assert_eq!(
            policy
                .javascript_navigation_violations("javascript:run()")
                .0
                .len(),
            1
        );
        let policy = make_policy("script-src 'strict-dynamic'", "https://example.test/");
        assert!(!policy.allows_inline_source(InlineType::Script, source));
        assert!(policy.allows_inline_source(InlineType::DynamicScript, source));
        assert!(!policy.allows_inline_source(InlineType::ScriptAttribute, source));
    }

    #[test]
    fn style_element_nonce_does_not_authorize_style_attribute() {
        let policy = make_policy(
            "style-src-elem 'nonce-abc' 'unsafe-inline'; style-src-attr 'nonce-abc' 'unsafe-inline'",
            "https://example.test/",
        );
        assert!(policy.allows_inline_source_with_nonce(
            InlineType::Style,
            "#target { color: red; }",
            Some("abc")
        ));
        assert!(!policy.allows_inline_source(InlineType::Style, "#target { color: red; }"));
        assert!(!policy.allows_inline_source_with_nonce(
            InlineType::StyleAttribute,
            "color: red;",
            Some("abc")
        ));
    }

    #[test]
    fn report_sample_does_not_authorize_inline_or_external_resources() {
        for sources in [
            "'report-sample'",
            "'none' 'report-sample'",
            "'REPORT-SAMPLE'",
        ] {
            let policy = make_policy(&format!("script-src {sources}"), "https://example.test/");
            assert!(!policy.allows_inline(ResourceType::Script), "{sources}");
            assert!(
                !policy.allows_inline_attribute(ResourceType::Script),
                "{sources}"
            );
            assert!(
                !policy.allows_reference(ResourceType::Script, "https://example.test/app.js"),
                "{sources}"
            );
        }
        let policy = make_policy(
            "script-src 'unsafe-inline' 'report-sample'",
            "https://example.test/",
        );
        assert!(policy.allows_inline(ResourceType::Script));
        assert!(!policy.allows_reference(ResourceType::Script, "https://example.test/app.js"));
    }

    #[test]
    fn element_and_attribute_directives_use_separate_fallbacks() {
        for (resource, directive) in [
            (ResourceType::Script, "script-src"),
            (ResourceType::Style, "style-src"),
        ] {
            let elements_blocked = make_policy(
                &format!("{directive} 'unsafe-inline'; {directive}-elem 'none'"),
                "https://example.test/",
            );
            assert!(!elements_blocked.allows_inline(resource), "{directive}");
            assert!(
                elements_blocked.allows_inline_attribute(resource),
                "{directive}"
            );

            let attributes_blocked = make_policy(
                &format!("{directive} 'unsafe-inline'; {directive}-attr 'none'"),
                "https://example.test/",
            );
            assert!(attributes_blocked.allows_inline(resource), "{directive}");
            assert!(
                !attributes_blocked.allows_inline_attribute(resource),
                "{directive}"
            );

            let element_source = make_policy(
                &format!("{directive} 'none'; {directive}-elem https://example.test/allowed/"),
                "https://example.test/",
            );
            assert!(element_source.allows_reference(resource, "/allowed/resource"));
            assert!(!element_source.allows_reference(resource, "/other/resource"));
            assert!(element_source.allows_reference_after_redirects(
                resource,
                "/other/resource",
                1
            ));
            assert!(!element_source.allows_reference_after_redirects(
                resource,
                "https://other.test/allowed/resource",
                1
            ));
        }
    }

    #[test]
    fn inline_samples_use_only_the_selected_element_source_list() {
        for (resource, directive, reference) in [
            (ResourceType::Script, "script-src", "inline"),
            (ResourceType::Style, "style-src", "style-element:7"),
        ] {
            for (text, expected_sample) in [
                (format!("{directive} 'report-sample'"), true),
                ("default-src 'report-sample'".to_string(), true),
                (
                    format!("default-src 'report-sample'; {directive} 'none'"),
                    false,
                ),
                (
                    format!("{directive} 'report-sample'; {directive}-elem 'none'"),
                    false,
                ),
                (
                    format!("{directive} 'none'; {directive}-elem 'report-sample'"),
                    true,
                ),
                (
                    format!("{directive} 'report-sample'; {directive}-elem"),
                    false,
                ),
                (
                    format!("{directive} 'none'; {directive}-attr 'report-sample'"),
                    false,
                ),
            ] {
                let policy = make_policy(&text, "https://example.test/");
                assert_eq!(
                    policy.blocking_policy_texts_after_redirects(resource, reference, 0),
                    vec![PolicyViolation {
                        serialized: text.clone(),
                        report_sample: expected_sample,
                    }],
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn style_attribute_samples_use_the_attribute_source_list() {
        for (text, expected_sample) in [
            ("style-src 'none'; style-src-attr 'report-sample'", true),
            ("style-src 'report-sample'; style-src-elem 'none'", true),
            ("default-src 'report-sample'", true),
            ("style-src 'report-sample'; style-src-attr 'none'", false),
            ("style-src 'report-sample'; style-src-attr", false),
            ("style-src 'none'; style-src-elem 'report-sample'", false),
        ] {
            let policy = make_policy(text, "https://example.test/");
            assert!(
                !policy.allows_inline_attribute(ResourceType::Style),
                "{text}"
            );
            assert_eq!(
                policy.blocking_policy_texts_after_redirects(
                    ResourceType::Style,
                    "style-attribute:7",
                    0
                ),
                vec![PolicyViolation {
                    serialized: text.to_string(),
                    report_sample: expected_sample,
                }],
                "{text}"
            );
        }
    }

    #[test]
    fn each_violating_policy_keeps_its_own_sample_opt_in() {
        let document = TreeBuilder::parse("<html><head></head><body></body></html>").document();
        let policy = CspPolicy::from_headers_and_document(
            &[
                "script-src 'report-sample'".to_string(),
                "script-src 'none'".to_string(),
                "script-src 'unsafe-inline' 'report-sample'".to_string(),
            ],
            &document,
            "https://example.test/",
        )
        .with_report_only_headers(&[
            "script-src 'none'; report-to plain".to_string(),
            "script-src 'report-sample'; report-to sampled".to_string(),
            "script-src 'unsafe-inline'; report-to allowed".to_string(),
        ]);
        assert_eq!(
            policy.blocking_policy_texts_after_redirects(ResourceType::Script, "inline", 0),
            vec![
                PolicyViolation {
                    serialized: "script-src 'report-sample'".to_string(),
                    report_sample: true,
                },
                PolicyViolation {
                    serialized: "script-src 'none'".to_string(),
                    report_sample: false,
                },
            ]
        );
        assert_eq!(
            policy.report_only_policy_texts(ResourceType::Script, "inline"),
            vec![
                PolicyViolation {
                    serialized: "script-src 'none'; report-to plain".to_string(),
                    report_sample: false,
                },
                PolicyViolation {
                    serialized: "script-src 'report-sample'; report-to sampled".to_string(),
                    report_sample: true,
                },
            ]
        );
    }

    #[test]
    fn report_sample_opt_in_requires_the_exact_token() {
        // The parser lowercases directive names, while CSP's inline sample
        // check uses exact membership in the original directive value.
        for (token, expected_sample) in [
            ("'report-sample'", true),
            ("'REPORT-SAMPLE'", false),
            ("'Report-Sample'", false),
            ("report-sample", false),
        ] {
            let text = format!("SCRIPT-SRC {token}");
            let policy = make_policy(&text, "https://example.test/");
            assert_eq!(
                policy.blocking_policy_texts_after_redirects(ResourceType::Script, "inline", 0),
                vec![PolicyViolation {
                    serialized: text,
                    report_sample: expected_sample,
                }]
            );
        }
    }

    #[test]
    fn external_violations_never_opt_in_to_inline_samples() {
        let text = "default-src 'report-sample'";
        let policy = make_policy(text, "https://example.test/")
            .with_report_only_headers(&[text.to_string()]);
        for resource in [
            ResourceType::Script,
            ResourceType::Style,
            ResourceType::Image,
        ] {
            for (reference, redirects) in [
                ("https://example.test/resource", 0),
                ("/resource", 0),
                ("https://other.test/resource", 1),
            ] {
                let expected = vec![PolicyViolation {
                    serialized: text.to_string(),
                    report_sample: false,
                }];
                assert_eq!(
                    policy.blocking_policy_texts_after_redirects(resource, reference, redirects),
                    expected
                );
                assert_eq!(
                    policy.report_only_policy_texts_after_redirects(resource, reference, redirects),
                    expected
                );
            }
        }
    }

    #[test]
    fn inline_report_samples_preserve_whitespace_and_unicode_boundaries() {
        assert_eq!(inline_report_sample(""), "");
        assert_eq!(inline_report_sample(" \n\t source \n"), " \n\t source \n");
        assert_eq!(inline_report_sample(&"a".repeat(39)), "a".repeat(39));
        assert_eq!(inline_report_sample(&"a".repeat(40)), "a".repeat(40));
        assert_eq!(inline_report_sample(&"a".repeat(41)), "a".repeat(40));
        let prefix = format!(" \n{}", "あ".repeat(37));
        assert_eq!(
            inline_report_sample(&format!("{prefix}😀z")),
            format!("{prefix}😀")
        );
    }

    #[test]
    fn host_scheme_and_none_sources_match() {
        let parsed = make_policy(
            "connect-src https://api.example.test:8443 https:",
            "https://example.test/",
        );
        assert!(parsed.allows_url(
            ResourceType::Connect,
            &"https://api.example.test:8443/v1".parse().unwrap()
        ));
        assert!(parsed.allows_url(
            ResourceType::Connect,
            &"https://other.test/v1".parse().unwrap()
        ));
        assert!(
            !make_policy("connect-src 'none'", "https://example.test/").allows_url(
                ResourceType::Connect,
                &"https://example.test/api".parse().unwrap()
            )
        );
    }

    #[test]
    fn multiple_policies_are_anded_and_meta_is_captured() {
        let document = TreeBuilder::parse(
            r#"<html><head><meta http-equiv="Content-Security-Policy" content="script-src 'none'"></head><body></body></html>"#,
        )
        .document();
        let policy = CspPolicy::from_headers_and_document(
            &["script-src 'self'".to_string()],
            &document,
            "https://example.test/",
        );
        assert!(!policy.allows_url(
            ResourceType::Script,
            &"https://example.test/a.js".parse().unwrap()
        ));
        assert!(!policy.allows_inline(ResourceType::Script));
    }

    #[test]
    fn websocket_scheme_is_checked_against_its_transport_source() {
        let policy = make_policy("connect-src wss:", "https://example.test/");
        assert!(policy.allows_reference(ResourceType::Connect, "wss://socket.example.test/stream"));
        assert!(!policy.allows_reference(ResourceType::Connect, "ws://socket.example.test/stream"));
    }

    #[test]
    fn unrelated_policy_does_not_report_an_unparsed_connect_reference() {
        let document = TreeBuilder::parse("<html><head></head><body></body></html>").document();
        let policy = CspPolicy::from_headers_and_document(
            &["trusted-types Allowed".into(), "connect-src 'none'".into()],
            &document,
            "https://example.test/",
        );
        assert_eq!(
            policy.blocking_policy_texts_after_redirects(
                ResourceType::Connect,
                "ws:/common/blank.html",
                0,
            ),
            vec![PolicyViolation {
                serialized: "connect-src 'none'".into(),
                report_sample: false,
            }],
        );
    }

    #[test]
    fn host_sources_without_scheme_use_the_document_scheme() {
        let policy = make_policy("connect-src api.example.test", "https://example.test/");
        assert!(policy.allows_reference(ResourceType::Connect, "https://api.example.test/v1"));
        assert!(!policy.allows_reference(ResourceType::Connect, "http://api.example.test/v1"));
    }

    #[test]
    fn meta_csp_is_only_collected_from_head() {
        let document = TreeBuilder::parse(
            r#"<html><head></head><body><meta http-equiv="Content-Security-Policy" content="script-src 'none'"></body></html>"#,
        )
        .document();
        let policy = CspPolicy::from_headers_and_document(&[], &document, "https://example.test/");
        assert!(policy.allows_url(
            ResourceType::Script,
            &"https://example.test/app.js".parse().unwrap()
        ));
    }

    #[test]
    fn bracketed_ipv6_host_sources_keep_the_full_authority() {
        let parsed = parse_source_expression("[::1]:8443").unwrap();
        assert_eq!(
            parsed,
            SourceExpression::Host {
                scheme: None,
                host: "::1".to_string(),
                port: Some(8443),
                path: None,
            }
        );
    }

    #[test]
    fn host_source_paths_keep_case_sensitive_matching() {
        let policy = make_policy(
            "script-src https://example.test/CasePath/",
            "https://example.test/",
        );
        assert!(
            policy.allows_reference(ResourceType::Script, "https://example.test/CasePath/app.js")
        );
        assert!(
            !policy.allows_reference(ResourceType::Script, "https://example.test/casepath/app.js")
        );
    }

    #[test]
    fn path_source_directory_prefix_and_exact_file_are_distinct() {
        for (source, path, allowed) in [
            ("/allowed/", "/allowed/file.js", true),
            ("/allowed/", "/allowed/deeper/file.js", true),
            ("/allowed/", "/allowed", false),
            ("/allowed/", "/allowed-suffix/file.js", false),
            ("/allowed", "/allowed", true),
            ("/allowed", "/allowed/file.js", false),
            ("/allowed.js", "/allowed.js?query=ignored", true),
            ("/allowed.js", "/allowed.js/child", false),
            ("/", "/any/path", true),
        ] {
            let policy = make_policy(
                &format!("script-src https://example.test{source}"),
                "https://example.test/",
            );
            assert_eq!(
                policy
                    .allows_reference(ResourceType::Script, &format!("https://example.test{path}")),
                allowed,
                "source={source}, path={path}"
            );
        }
    }

    #[test]
    fn path_source_percent_decoding_keeps_segment_boundaries_and_case() {
        for (source, path, allowed) in [
            ("/%61llowed/", "/allowed/file.js", true),
            ("/allowed/", "/%61llowed/file.js", true),
            ("/caf%C3%A9.js", "/caf%c3%a9.js", true),
            ("/a%2Fb.js", "/a%2fb.js", true),
            ("/a%2Fb.js", "/a/b.js", false),
            ("/a/b.js", "/a%2Fb.js", false),
            ("/%252F", "/%2F", false),
            ("/Case/", "/case/file.js", false),
            ("/literal%xx", "/literal%xx", true),
            ("/%FF", "/%FE", false),
        ] {
            let policy = make_policy(
                &format!("script-src https://example.test{source}"),
                "https://example.test/",
            );
            assert_eq!(
                policy
                    .allows_reference(ResourceType::Script, &format!("https://example.test{path}")),
                allowed,
                "source={source}, path={path}"
            );
        }
    }

    #[test]
    fn redirects_skip_paths_but_still_check_scheme_host_and_port() {
        let policy = make_policy(
            "script-src https://example.test:8443/entry.js",
            "https://example.test/",
        );
        for (url, count, allowed) in [
            ("https://example.test:8443/other.js", 0, false),
            ("https://example.test:8443/other.js", 1, true),
            ("https://example.test:8443/other.js", 3, true),
            ("https://other.test:8443/entry.js", 1, false),
            ("https://example.test:9443/entry.js", 1, false),
            ("http://example.test:8443/entry.js", 1, false),
        ] {
            assert_eq!(
                policy.allows_url_after_redirects(
                    ResourceType::Script,
                    &url.parse().unwrap(),
                    count
                ),
                allowed,
                "{url} after {count} redirects"
            );
        }
    }

    #[test]
    fn opaque_documents_reject_unresolvable_relative_references() {
        let policy = make_policy("script-src 'self'", "");
        assert!(!policy.allows_reference(ResourceType::Script, "app.js"));
        assert!(CspPolicy::default().allows_reference(ResourceType::Script, "app.js"));
    }

    #[test]
    fn invalid_host_source_ports_are_ignored() {
        assert!(parse_source_expression("example.com:").is_none());
        assert!(parse_source_expression("example.com:not-a-port").is_none());
        assert!(parse_source_expression("example.com:65536").is_none());
    }

    #[test]
    fn image_sources_use_img_src_before_default_src() {
        let fallback = make_policy("default-src 'none'", "https://example.test/page");
        assert!(!fallback.allows_reference(ResourceType::Image, "https://example.test/image.png"));
        let image_policy = make_policy(
            "default-src 'none'; img-src 'self'",
            "https://example.test/page",
        );
        assert!(
            image_policy.allows_reference(ResourceType::Image, "https://example.test/image.png")
        );
        assert!(
            !image_policy.allows_reference(ResourceType::Image, "https://other.test/image.png")
        );
        assert!(
            !image_policy.allows_reference(ResourceType::Script, "https://example.test/image.png")
        );
    }

    #[test]
    fn image_source_schemes_require_explicit_data_and_blob_permission() {
        let policy = make_policy("img-src 'self' data: blob:", "https://example.test/page");
        assert!(policy.allows_reference(ResourceType::Image, "data:image/png;base64,aGVsbG8="));
        assert!(policy.allows_reference(ResourceType::Image, "blob:https://example.test/id"));
        let denied = make_policy("img-src 'none'", "https://example.test/page");
        assert!(!denied.allows_reference(ResourceType::Image, "data:image/png;base64,aGVsbG8="));
        assert!(!denied.allows_reference(ResourceType::Image, "blob:https://example.test/id"));
    }

    #[test]
    fn redirected_image_sources_skip_paths_but_keep_origin_boundaries() {
        let policy = make_policy(
            "img-src https://example.test:8443/allowed/",
            "https://example.test/page",
        );
        let changed_path = "https://example.test:8443/elsewhere/image.png"
            .parse()
            .unwrap();
        assert!(!policy.allows_url(ResourceType::Image, &changed_path));
        assert!(policy.allows_url_after_redirects(ResourceType::Image, &changed_path, 1));
        let other_origin = "https://other.test:8443/allowed/image.png".parse().unwrap();
        assert!(!policy.allows_url_after_redirects(ResourceType::Image, &other_origin, 1));
    }

    #[test]
    fn image_report_only_violations_do_not_block_an_enforcing_allowlist() {
        let policy = make_policy("img-src 'self'", "https://example.test/page")
            .with_report_only_headers(&["img-src 'none'".into()]);
        let reference = "https://example.test/image.png";
        assert!(policy.allows_reference(ResourceType::Image, reference));
        assert!(
            policy
                .blocking_policy_texts_after_redirects(ResourceType::Image, reference, 0)
                .is_empty()
        );
        assert_eq!(
            policy.report_only_policy_texts(ResourceType::Image, reference),
            vec![PolicyViolation {
                serialized: "img-src 'none'".into(),
                report_sample: false,
            }]
        );
        assert!(!policy.allows_reference(ResourceType::Image, "https://other.test/image.png"));
    }
}
