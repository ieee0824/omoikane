//! Content Security Policy parsing and source-expression matching.
//!
//! This module intentionally implements the small enforced-policy core used by
//! the page runtime.  It keeps policy state independent from the JavaScript
//! global so a navigation can install a fresh policy before the first script
//! runs, while the matching rules remain easy to exercise in focused tests.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::dom::{Node, NodeHandle, NodeType};
use crate::http::Url;

/// A fetchable resource class covered by the CSP core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceType {
    Script,
    Style,
    Connect,
}

impl ResourceType {
    pub(crate) fn directive(self) -> &'static str {
        match self {
            Self::Script => "script-src",
            Self::Style => "style-src",
            Self::Connect => "connect-src",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Style => "style",
            Self::Connect => "connect",
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceExpression {
    None,
    SelfKeyword,
    UnsafeInline,
    Any,
    Scheme(String),
    Host {
        scheme: Option<String>,
        host: String,
        port: Option<u16>,
        path: Option<String>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ParsedPolicy {
    directives: BTreeMap<String, Vec<SourceExpression>>,
}

/// The policies attached to one Document.
///
/// Each response-header policy and each parser-initial CSP meta policy is kept
/// separately.  `allows_*` evaluates every policy, which gives the required
/// AND semantics without losing the source of a violation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CspPolicy {
    policies: Vec<ParsedPolicy>,
    base_url: Option<Url>,
}

impl CspPolicy {
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
            base_url: base_url.parse::<Url>().ok(),
        }
    }

    pub(crate) fn allows_inline(&self, resource_type: ResourceType) -> bool {
        self.policies
            .iter()
            .all(|policy| policy.allows_inline(resource_type))
    }

    pub(crate) fn allows_url(&self, resource_type: ResourceType, url: &Url) -> bool {
        self.policies
            .iter()
            .all(|policy| policy.allows_url(resource_type, &self.base_url, &ResourceUrl::from(url)))
    }

    /// Checks an HTML/network reference before it is fetched.  This accepts
    /// websocket schemes and data URLs in addition to the HTTP URL type used by
    /// the network client.
    pub(crate) fn allows_reference(&self, resource_type: ResourceType, reference: &str) -> bool {
        let Some(resource_url) = ResourceUrl::parse(reference, self.base_url.as_ref()) else {
            // With no base URL (for example, an opaque `data:` Document), a
            // relative reference cannot be checked against the policy. A
            // document with an enforced policy must fail closed here; without
            // CSP, leave the eventual resource-loader error unchanged.
            return self.policies.is_empty();
        };
        self.policies
            .iter()
            .all(|policy| policy.allows_url(resource_type, &self.base_url, &resource_url))
    }
}

impl ParsedPolicy {
    fn sources_for(&self, resource_type: ResourceType) -> Option<&[SourceExpression]> {
        self.directives
            .get(resource_type.directive())
            .or_else(|| self.directives.get("default-src"))
            .map(Vec::as_slice)
    }

    fn allows_inline(&self, resource_type: ResourceType) -> bool {
        let Some(sources) = self.sources_for(resource_type) else {
            return true;
        };
        sources
            .iter()
            .any(|source| matches!(source, SourceExpression::UnsafeInline))
    }

    fn allows_url(
        &self,
        resource_type: ResourceType,
        base_url: &Option<Url>,
        url: &ResourceUrl,
    ) -> bool {
        let Some(sources) = self.sources_for(resource_type) else {
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
            SourceExpression::UnsafeInline => false,
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
                path.as_ref().is_none_or(|prefix| {
                    prefix == "/"
                        || url.path == *prefix
                        || url.path.starts_with(&format!("{prefix}/"))
                })
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
        if reference
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
        {
            return Some(Self {
                scheme: "data".to_string(),
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
    let mut policy = ParsedPolicy::default();
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
        let sources = tokens
            .filter_map(parse_source_expression)
            .collect::<Vec<_>>();
        policy.directives.insert(name, sources);
    }
    (!policy.directives.is_empty()).then_some(policy)
}

fn parse_source_expression(token: &str) -> Option<SourceExpression> {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "'none'" => return Some(SourceExpression::None),
        "'self'" => return Some(SourceExpression::SelfKeyword),
        "'unsafe-inline'" => return Some(SourceExpression::UnsafeInline),
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

fn collect_csp_meta_contents(node: &NodeHandle, out: &mut Vec<String>) {
    if node.node_type() == NodeType::Element
        && node
            .tag_name()
            .is_some_and(|tag| tag.eq_ignore_ascii_case("meta"))
        && is_descendant_of_head(node)
        && node
            .get_attribute("http-equiv")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("content-security-policy"))
        && let Some(content) = node.get_attribute("content")
    {
        out.push(content);
    }
    for child in node.child_nodes() {
        collect_csp_meta_contents(&child, out);
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

    fn policy(text: &str, base: &str) -> CspPolicy {
        CspPolicy {
            policies: vec![parse_policy(text).unwrap()],
            base_url: base.parse().ok(),
        }
    }

    #[test]
    fn default_src_falls_back_and_self_matches_case_insensitively() {
        let policy = policy("default-src 'self'", "https://Example.test/index.html");
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
    fn inline_requires_unsafe_inline() {
        assert!(
            !policy("script-src 'self'", "https://example.test/")
                .allows_inline(ResourceType::Script)
        );
        assert!(
            policy("script-src 'unsafe-inline'", "https://example.test/")
                .allows_inline(ResourceType::Script)
        );
    }

    #[test]
    fn host_scheme_and_none_sources_match() {
        let parsed = policy(
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
            !policy("connect-src 'none'", "https://example.test/").allows_url(
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
        let policy = policy("connect-src wss:", "https://example.test/");
        assert!(policy.allows_reference(ResourceType::Connect, "wss://socket.example.test/stream"));
        assert!(!policy.allows_reference(ResourceType::Connect, "ws://socket.example.test/stream"));
    }

    #[test]
    fn host_sources_without_scheme_use_the_document_scheme() {
        let policy = policy("connect-src api.example.test", "https://example.test/");
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
        let policy = policy(
            "script-src https://example.test/CasePath",
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
    fn opaque_documents_reject_unresolvable_relative_references() {
        let policy = policy("script-src 'self'", "");
        assert!(!policy.allows_reference(ResourceType::Script, "app.js"));
        assert!(CspPolicy::default().allows_reference(ResourceType::Script, "app.js"));
    }

    #[test]
    fn invalid_host_source_ports_are_ignored() {
        assert!(parse_source_expression("example.com:").is_none());
        assert!(parse_source_expression("example.com:not-a-port").is_none());
        assert!(parse_source_expression("example.com:65536").is_none());
    }
}
