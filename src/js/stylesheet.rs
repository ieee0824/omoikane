//! Document-owned stylesheet resources shared by CSSOM geometry and painting.

use super::csp::{CspPolicy, InlineType, ResourceType};
use crate::dom::{Node, NodeHandle, WeakNodeHandle};
use crate::error_reporting::{
    ErrorCategory, ErrorCode, ErrorReporter, ErrorSeverity, ExecutionSurface, RawEvent,
};
use crate::http::cors::{
    self, CredentialsMode, Origin as CorsOrigin, PreflightCache, RedirectMode, RequestMode,
};
use crate::http::{Client, CookieJar, CookieRequestContext, HttpRequest, Url};
use crate::paint::stylesheet as css;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_IMPORT_DEPTH: usize = 5;

#[derive(Clone)]
struct Resource {
    text: String,
    url: Option<Url>,
    redirects: usize,
    requests: Vec<(String, usize)>,
}

/// One CSP request, response, or inline check belonging to a stylesheet load.
/// The caller delivers notifications after the loader releases its borrows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StylePolicyCheck {
    /// Current URL used for source-list matching, or `inline`.
    pub(super) reference: String,
    /// Redirect count controlling whether source-list paths are significant.
    pub(super) redirects: usize,
    /// Initial request URL, retained to avoid reporting redirect destinations.
    pub(super) report_reference: String,
    /// Clipped original inline text; empty for request/response checks.
    pub(super) sample: String,
    /// Complete source used for inline hash matching; absent for fetched CSS.
    pub(super) inline_source: Option<String>,
    /// Whether enforced policy prevented this stage from proceeding.
    pub(super) blocked: bool,
}

struct NodeSource {
    text: String,
    text_nodes: Vec<usize>,
    nonce: Option<String>,
    href: Option<String>,
    rel: Option<String>,
    base: Option<Url>,
    policy: CspPolicy,
}

impl NodeSource {
    fn matches(&self, other: &Self) -> bool {
        self.text == other.text
            && self.text_nodes == other.text_nodes
            && self.nonce == other.nonce
            && self.href == other.href
            && self.rel == other.rel
            && self.base == other.base
            // Endpoint removal after a 410 response changes delivery state,
            // not the policies that caused the original load notifications.
            && self.policy.has_same_policy_rules(&other.policy)
    }

    fn new(node: &NodeHandle, base: Option<&Url>, policy: &CspPolicy) -> Self {
        fn text_nodes(node: &NodeHandle, ids: &mut Vec<usize>) {
            for child in node.child_nodes() {
                ids.push(child.identity());
                text_nodes(&child, ids);
            }
        }
        let mut ids = Vec::new();
        text_nodes(node, &mut ids);
        Self {
            text: css::collect_text_contents(node),
            text_nodes: ids,
            nonce: node.get_attribute("nonce"),
            href: node.get_attribute("href"),
            rel: node.get_attribute("rel"),
            base: base.cloned(),
            policy: policy.clone(),
        }
    }
}

/// Retains each document's resource responses across DOM/style invalidations.
#[derive(Default)]
pub(super) struct StylesheetLoader {
    resources: HashMap<String, Option<Resource>>,
    /// Distinguishes actual source changes from resolver/viewport rebuilds.
    /// Text-node IDs detect textContent replacement even with unchanged text.
    /// Same-value CharacterData writes require a future DOM mutation revision.
    node_sources: HashMap<usize, (WeakNodeHandle, NodeSource)>,
    client: Client,
    cookie_context: Option<CookieRequestContext>,
    isolation_origin: Option<CorsOrigin>,
    embedder_policy: crate::http::corp::EmbedderPolicyValue,
    report_only_embedder_policy: crate::http::corp::EmbedderPolicyValue,
    coep_violations: Vec<cors::CorpViolation>,
    reporter: Option<(Arc<ErrorReporter>, ExecutionSurface)>,
}

impl std::fmt::Debug for StylesheetLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StylesheetLoader")
            .field("resources", &self.resources.len())
            .finish()
    }
}

impl StylesheetLoader {
    /// Drops source snapshots whose nodes have been released. Call once before
    /// a resolver rebuild rather than scanning the map for every stylesheet.
    pub(super) fn retain_live_sources(&mut self) {
        self.node_sources.retain(|_, (node, _)| node.is_alive());
    }

    pub(super) fn set_isolation(
        &mut self,
        origin: CorsOrigin,
        embedder_policy: crate::http::corp::EmbedderPolicyValue,
        report_only_embedder_policy: crate::http::corp::EmbedderPolicyValue,
    ) {
        if self.isolation_origin.as_ref() != Some(&origin)
            || self.embedder_policy != embedder_policy
            || self.report_only_embedder_policy != report_only_embedder_policy
        {
            self.resources.clear();
        }
        self.isolation_origin = Some(origin);
        self.embedder_policy = embedder_policy;
        self.report_only_embedder_policy = report_only_embedder_policy;
    }

    /// Takes COEP violations recorded during external stylesheet and import fetches.
    pub(super) fn take_coep_violations(&mut self) -> Vec<cors::CorpViolation> {
        std::mem::take(&mut self.coep_violations)
    }

    pub(super) fn set_error_reporter(
        &mut self,
        reporter: Option<(Arc<ErrorReporter>, ExecutionSurface)>,
    ) {
        if let Some((instance, surface)) = &reporter {
            self.client
                .set_error_reporter(Arc::clone(instance), *surface);
        }
        self.reporter = reporter;
    }

    fn report_fetch_failure(&self) {
        let Some((reporter, surface)) = &self.reporter else {
            return;
        };
        reporter.report(
            RawEvent::new(
                ErrorCategory::Css,
                ErrorSeverity::Error,
                ErrorCode::new("CSS_STYLESHEET_FETCH_FAILED").expect("static code"),
                *surface,
                "Resource load failed",
                &[("operation", "fetch"), ("resource", "stylesheet")],
            )
            .sanitize(),
        );
    }

    pub(super) fn cached_import(
        &self,
        href: &str,
        base: Option<&Url>,
        document_base: Option<&Url>,
    ) -> Option<(String, String)> {
        let key = if href.starts_with("data:") {
            href.to_string()
        } else {
            css::resolve_relative_stylesheet_url(base?, href, document_base)?.to_string()
        };
        let resource = self.resources.get(&key)?.as_ref()?;
        let resolved = resource
            .url
            .as_ref()
            .map_or_else(|| key.clone(), ToString::to_string);
        Some((resource.text.clone(), resolved))
    }

    pub(super) fn load_node(
        &mut self,
        node: &NodeHandle,
        base: Option<&Url>,
        policy: &CspPolicy,
        cookie_context: CookieRequestContext,
        cookies: Arc<Mutex<CookieJar>>,
    ) -> (String, Vec<StylePolicyCheck>) {
        self.client.set_shared_cookie_store(cookies);
        if self.cookie_context.as_ref() != Some(&cookie_context) {
            self.resources.clear();
            self.node_sources.clear();
        }
        self.cookie_context = Some(cookie_context);
        let mut checks = Vec::new();
        if node.get_attribute("disabled").is_some() {
            self.node_sources.remove(&node.identity());
            return (String::new(), checks);
        }
        let source = NodeSource::new(node, base, policy);
        let unchanged = self
            .node_sources
            .get(&node.identity())
            .is_some_and(|(previous, input)| {
                previous.upgrade().as_ref() == Some(node) && input.matches(&source)
            });
        let mut output = Vec::new();
        let mut active = HashSet::new();
        if node.tag_name().as_deref() == Some("style") {
            let nonce = node.get_attribute("nonce");
            let blocked = !policy.allows_inline_source_with_nonce(
                InlineType::Style,
                &source.text,
                nonce.as_deref(),
            );
            checks.push(StylePolicyCheck {
                reference: "inline".into(),
                redirects: 0,
                report_reference: "inline".into(),
                sample: super::csp::inline_report_sample(&source.text),
                inline_source: Some(source.text.clone()),
                blocked,
            });
            if !blocked {
                let resource = Resource {
                    text: css::collect_text_contents(node),
                    url: base.cloned(),
                    redirects: 0,
                    requests: Vec::new(),
                };
                self.expand(
                    resource,
                    base,
                    policy,
                    0,
                    &mut active,
                    &mut output,
                    &mut checks,
                );
            }
        } else if node.get_attribute("rel").is_some_and(|rel| {
            rel.split_whitespace()
                .any(|word| word.eq_ignore_ascii_case("stylesheet"))
        }) {
            if let Some(href) = node.get_attribute("href") {
                if let Some(resource) = self.fetch(href.trim(), base, base, policy, &mut checks) {
                    self.expand(
                        resource,
                        base,
                        policy,
                        0,
                        &mut active,
                        &mut output,
                        &mut checks,
                    );
                }
            }
        }
        let mut text = output.join("\n");
        if !text.is_empty()
            && let Some(media) = node
                .get_attribute("media")
                .filter(|media| !media.trim().is_empty())
        {
            text = format!("@media {media} {{\n{text}\n}}");
        }
        self.node_sources
            .insert(node.identity(), (node.downgrade(), source));
        // Enforcement and cached-response validation have still run above.
        // Only notifications are suppressed for a resolver-only rebuild.
        if unchanged {
            checks.clear();
        }
        (text, checks)
    }

    fn fetch(
        &mut self,
        href: &str,
        base: Option<&Url>,
        document_base: Option<&Url>,
        policy: &CspPolicy,
        checks: &mut Vec<StylePolicyCheck>,
    ) -> Option<Resource> {
        if href.is_empty() {
            return None;
        }
        let is_data = href
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"));
        let url = if is_data {
            None
        } else {
            let Some(url) = base
                .and_then(|base| css::resolve_relative_stylesheet_url(base, href, document_base))
            else {
                self.report_fetch_failure();
                return None;
            };
            Some(url)
        };
        let key = url
            .as_ref()
            .map_or_else(|| href.to_owned(), ToString::to_string);
        if !check_reference(policy, &key, 0, &key, checks) {
            return None;
        }
        if let Some(cached) = self.resources.get(&key) {
            return cached
                .as_ref()
                .filter(|resource| check_cached_resource(resource, policy, &key, checks))
                .cloned();
        }
        let check_start = checks.len();
        let resource = if let Some(url) = url {
            self.fetch_network(url, document_base, policy, &key, checks)
        } else {
            self.decode_data(href, policy, &key, checks)
        };
        let policy_blocked = checks[check_start..].iter().any(|check| check.blocked);
        if !policy_blocked {
            if resource.is_none() {
                self.report_fetch_failure();
            }
            self.resources.insert(key, resource.clone());
        }
        resource
    }

    fn fetch_network(
        &mut self,
        mut url: Url,
        document_base: Option<&Url>,
        policy: &CspPolicy,
        original: &str,
        checks: &mut Vec<StylePolicyCheck>,
    ) -> Option<Resource> {
        let mut requests = vec![(original.to_owned(), 0)];
        let mut redirects = 0;
        let public_only = !document_base.is_some_and(|base| same_origin(base, &url));
        let mut crossed_origin = false;
        loop {
            let mut request = HttpRequest::get(&url.to_string()).ok()?;
            if public_only || !document_base.is_some_and(|base| same_origin(base, &url)) {
                request.require_public_ip();
            }
            if let Some(cookie_context) = self.cookie_context.as_ref() {
                cookie_context.apply(&mut request);
                if crossed_origin {
                    request.set_storage_access_eligible(false);
                }
            }
            let origin = self.isolation_origin.clone().unwrap_or_else(|| {
                document_base
                    .map(CorsOrigin::from_url)
                    .unwrap_or_else(CorsOrigin::opaque)
            });
            let mut violations = Vec::new();
            // Each CSS redirect is fetched separately so CSP can reject the
            // next URL before transmission. Preserve credentialless taint
            // across those calls, including a redirect back to our origin.
            let credentials = if crossed_origin
                && self.embedder_policy == crate::http::corp::EmbedderPolicyValue::Credentialless
            {
                CredentialsMode::Omit
            } else {
                CredentialsMode::Include
            };
            let fetched = cors::fetch_with_embedder_policy_reporting(
                &mut self.client,
                request,
                &origin,
                RequestMode::NoCors,
                credentials,
                RedirectMode::Manual,
                &mut PreflightCache::default(),
                self.embedder_policy,
                self.report_only_embedder_policy,
                &mut violations,
                None,
            );
            self.coep_violations
                .extend(violations.into_iter().map(|mut violation| {
                    violation.blocked_url = original.to_owned();
                    violation
                }));
            let response = fetched.ok()?.response;
            if matches!(response.status_code(), 301 | 302 | 303 | 307 | 308) {
                if redirects == 10 {
                    return None;
                }
                let next =
                    crate::http::url::resolve_url(&url, response.header("location")?).ok()?;
                crossed_origin |= !same_origin(&url, &next);
                url = next;
                redirects += 1;
                let reference = url.to_string();
                if !check_reference(policy, &reference, redirects, original, checks) {
                    return None;
                }
                requests.push((reference, redirects));
                continue;
            }
            let effective = response.effective_url().cloned().unwrap_or(url);
            if !check_reference(policy, &effective.to_string(), redirects, original, checks) {
                return None;
            }
            let mime = response
                .header("content-type")
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim();
            if response.status_code() != 200
                || !mime.eq_ignore_ascii_case("text/css")
                || response.body().len() > MAX_BYTES
            {
                return None;
            }
            return Some(Resource {
                text: std::str::from_utf8(response.body()).ok()?.to_owned(),
                url: Some(effective),
                redirects,
                requests,
            });
        }
    }

    fn decode_data(
        &self,
        href: &str,
        policy: &CspPolicy,
        original: &str,
        checks: &mut Vec<StylePolicyCheck>,
    ) -> Option<Resource> {
        let data = crate::http::parse_data_uri(href)?;
        if !check_reference(policy, href, 0, original, checks) {
            return None;
        }
        if !data.mime_type.eq_ignore_ascii_case("text/css") || data.data.len() > MAX_BYTES {
            return None;
        }
        Some(Resource {
            text: String::from_utf8(data.data).ok()?,
            url: None,
            redirects: 0,
            requests: vec![(original.to_owned(), 0)],
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn expand(
        &mut self,
        resource: Resource,
        document_base: Option<&Url>,
        policy: &CspPolicy,
        depth: usize,
        active: &mut HashSet<String>,
        output: &mut Vec<String>,
        checks: &mut Vec<StylePolicyCheck>,
    ) {
        if depth < MAX_IMPORT_DEPTH {
            let directives = css::extract_import_directives(&resource.text);
            if !directives.is_empty() {
                let chars: Vec<char> = resource.text.chars().collect();
                let mut cursor = 0usize;
                for directive in directives {
                    let preceding: String = chars[cursor..directive.start].iter().collect();
                    if !preceding.trim().is_empty() {
                        output.push(css::resolve_stylesheet_asset_urls(
                            preceding,
                            resource.url.as_ref(),
                        ));
                    }
                    cursor = directive.end;

                    let mut imported = Vec::new();
                    let supports_matches = directive
                        .supports
                        .as_deref()
                        .is_none_or(crate::css::supports_condition_matches);
                    if supports_matches
                        && let Some(import) = self.fetch(
                            &directive.href,
                            resource.url.as_ref().or(document_base),
                            document_base,
                            policy,
                            checks,
                        )
                    {
                        let key = import
                            .url
                            .as_ref()
                            .map_or_else(|| directive.href.clone(), ToString::to_string);
                        if active.insert(key.clone()) {
                            self.expand(
                                import,
                                document_base,
                                policy,
                                depth + 1,
                                active,
                                &mut imported,
                                checks,
                            );
                            active.remove(&key);
                        }
                    }
                    css::append_imported_stylesheets(
                        output,
                        imported,
                        directive.layer,
                        directive.supports.as_deref(),
                        directive.media.as_deref(),
                    );
                }
                let trailing: String = chars[cursor..].iter().collect();
                if !trailing.trim().is_empty() {
                    output.push(css::resolve_stylesheet_asset_urls(
                        trailing,
                        resource.url.as_ref(),
                    ));
                }
                return;
            }
        }
        output.push(css::resolve_stylesheet_asset_urls(
            resource.text,
            resource.url.as_ref(),
        ));
    }
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme() && a.host() == b.host() && a.port() == b.port()
}

fn check_reference(
    policy: &CspPolicy,
    reference: &str,
    redirects: usize,
    report_reference: &str,
    checks: &mut Vec<StylePolicyCheck>,
) -> bool {
    let blocked =
        !policy.allows_reference_after_redirects(ResourceType::Style, reference, redirects);
    checks.push(StylePolicyCheck {
        reference: reference.to_owned(),
        redirects,
        report_reference: report_reference.to_owned(),
        sample: String::new(),
        inline_source: None,
        blocked,
    });
    !blocked
}

fn check_cached_resource(
    resource: &Resource,
    policy: &CspPolicy,
    original: &str,
    checks: &mut Vec<StylePolicyCheck>,
) -> bool {
    // The caller already checked the first request. Retain intermediate hops
    // so a policy change cannot allow a cached response through a denied host.
    for (reference, redirects) in resource.requests.iter().skip(1) {
        if !check_reference(policy, reference, *redirects, original, checks) {
            return false;
        }
    }
    let reference = resource
        .url
        .as_ref()
        .map_or_else(|| original.to_owned(), ToString::to_string);
    check_reference(policy, &reference, resource.redirects, original, checks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::TreeBuilder;
    use crate::http::{HttpParseError, HttpResponse, HttpTransport};
    use crate::test_support::http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    use std::time::Duration;

    #[test]
    fn credentialless_stylesheet_redirect_back_to_origin_keeps_cookies_omitted() {
        struct RedirectTransport(Arc<Mutex<Vec<(String, Option<String>)>>>);

        impl HttpTransport for RedirectTransport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<HttpResponse, HttpParseError> {
                self.0.lock().unwrap().push((
                    request.url().to_string(),
                    request.header("cookie").map(str::to_owned),
                ));
                Ok(match request.url().path() {
                    "/entry.css" => HttpResponse::new(
                        302,
                        "Found",
                        vec![(
                            "Location".into(),
                            "https://resource.example.test/jump.css".into(),
                        )],
                        Vec::new(),
                    ),
                    "/jump.css" => HttpResponse::new(
                        302,
                        "Found",
                        vec![(
                            "Location".into(),
                            "https://owner.example.test/final.css".into(),
                        )],
                        Vec::new(),
                    ),
                    "/final.css" => HttpResponse::new(
                        200,
                        "OK",
                        vec![("Content-Type".into(), "text/css".into())],
                        b"body { color: red }".to_vec(),
                    ),
                    path => panic!("unexpected stylesheet path: {path}"),
                })
            }
        }

        let base: Url = "https://owner.example.test/document".parse().unwrap();
        let document = TreeBuilder::parse("<link rel=stylesheet href='/entry.css'>").document();
        let link = document.query_selector("link").unwrap();
        let cookies = Arc::new(Mutex::new(CookieJar::new()));
        cookies
            .lock()
            .unwrap()
            .add_from_header_for_url("session=secret; Secure; SameSite=None", &base);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut loader = StylesheetLoader::default();
        loader
            .client
            .set_transport(Some(Arc::new(RedirectTransport(Arc::clone(&seen)))));
        loader.set_isolation(
            CorsOrigin::from_url(&base),
            crate::http::corp::EmbedderPolicyValue::Credentialless,
            crate::http::corp::EmbedderPolicyValue::UnsafeNone,
        );
        let (css, checks) = loader.load_node(
            &link,
            Some(&base),
            &CspPolicy::default(),
            CookieRequestContext::new(Some(base.clone()), None, false),
            cookies,
        );
        assert!(css.contains("color: red"));
        assert!(checks.iter().all(|check| !check.blocked));
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            [
                (
                    "https://owner.example.test/entry.css".to_owned(),
                    Some("session=secret".to_owned()),
                ),
                ("https://resource.example.test/jump.css".to_owned(), None),
                ("https://owner.example.test/final.css".to_owned(), None),
            ]
        );
    }

    fn load(
        loader: &mut StylesheetLoader,
        node: &NodeHandle,
        policy: &CspPolicy,
    ) -> (String, Vec<StylePolicyCheck>) {
        let base = "https://example.test/document".parse().unwrap();
        loader.load_node(
            node,
            Some(&base),
            policy,
            CookieRequestContext::new(Some(base.clone()), None, false),
            Arc::default(),
        )
    }

    fn policy_with_reporting_endpoint(
        document: &NodeHandle,
        enforced: &str,
        report_only: &str,
    ) -> CspPolicy {
        CspPolicy::from_response_headers_and_document(
            &[
                ("Content-Security-Policy".into(), enforced.into()),
                (
                    "Content-Security-Policy-Report-Only".into(),
                    report_only.into(),
                ),
                (
                    "Reporting-Endpoints".into(),
                    "csp=\"https://example.test/reports\"".into(),
                ),
            ],
            document,
            "https://example.test/document",
        )
    }

    #[test]
    fn removing_a_reporting_endpoint_does_not_repeat_stylesheet_notifications() {
        let document = TreeBuilder::parse(
            "<style>body { color: red }</style>\
             <link rel=stylesheet href='data:text/css,body%7Bcolor:red%7D'>",
        )
        .document();
        let nodes = [
            document.query_selector("style").unwrap(),
            document.query_selector("link").unwrap(),
        ];
        let mut policy = policy_with_reporting_endpoint(
            &document,
            "style-src 'unsafe-inline' data:",
            "style-src 'none'; report-to csp",
        );
        let mut loader = StylesheetLoader::default();
        let first_loads: Vec<_> = nodes
            .iter()
            .map(|node| load(&mut loader, node, &policy))
            .collect();
        assert_eq!(first_loads[0].1.len(), 1);
        assert_eq!(first_loads[1].1.len(), 2);
        assert!(
            first_loads.iter().all(|(css, checks)| {
                !css.is_empty() && checks.iter().all(|check| !check.blocked)
            })
        );
        assert!(policy.reporting_endpoint("csp").is_some());
        let original_policy = policy.clone();
        // This is the delivery-state update made when the endpoint returns 410.
        policy.remove_reporting_endpoint("csp");
        assert!(policy.reporting_endpoint("csp").is_none());
        assert_ne!(policy, original_policy);
        for (node, (first_css, _)) in nodes.iter().zip(first_loads) {
            let (css, checks) = load(&mut loader, node, &policy);
            assert_eq!(css, first_css);
            assert!(checks.is_empty());
        }
    }

    #[test]
    fn policy_rule_changes_notify_and_enforce_against_a_cached_stylesheet() {
        let document =
            TreeBuilder::parse("<link rel=stylesheet href='data:text/css,body%7Bcolor:red%7D'>")
                .document();
        let link = document.query_selector("link").unwrap();
        let reference = link.get_attribute("href").unwrap();
        let allowed = policy_with_reporting_endpoint(
            &document,
            "style-src data:",
            "style-src data:; report-to csp",
        );
        let mut loader = StylesheetLoader::default();
        let (first_css, first_checks) = load(&mut loader, &link, &allowed);
        assert!(!first_css.is_empty());
        assert_eq!(first_checks.len(), 2);
        assert!(
            allowed
                .report_only_policy_texts(ResourceType::Style, &reference)
                .is_empty()
        );
        let monitored = policy_with_reporting_endpoint(
            &document,
            "style-src data:",
            "style-src 'none'; report-to csp",
        );
        assert_eq!(
            monitored
                .report_only_policy_texts(ResourceType::Style, &reference)
                .len(),
            1
        );
        let (css, checks) = load(&mut loader, &link, &monitored);
        assert_eq!(css, first_css);
        assert_eq!(checks.len(), 2);
        assert!(checks.iter().all(|check| !check.blocked));

        let mut denied = policy_with_reporting_endpoint(
            &document,
            "style-src 'none'",
            "style-src 'none'; report-to csp",
        );
        let (css, checks) = load(&mut loader, &link, &denied);
        assert!(css.is_empty());
        assert_eq!(checks.len(), 1);
        assert!(checks[0].blocked);
        denied.remove_reporting_endpoint("csp");
        let (css, checks) = load(&mut loader, &link, &denied);
        assert!(css.is_empty());
        assert!(checks.is_empty());
        let (css, checks) = load(&mut loader, &link, &allowed);
        assert_eq!(css, first_css);
        assert_eq!(checks.len(), 2);
        assert!(checks.iter().all(|check| !check.blocked));
    }

    #[test]
    fn inline_checks_follow_source_replacement_without_repeating_for_style_rebuilds() {
        let document = TreeBuilder::parse("<style>body { color: red }</style>").document();
        let style = document.query_selector("style").unwrap();
        let mut loader = StylesheetLoader::default();
        let policy = CspPolicy::default();
        let (css, checks) = load(&mut loader, &style, &policy);
        assert!(css.contains("color: red"));
        assert_eq!(
            checks,
            vec![StylePolicyCheck {
                reference: "inline".into(),
                redirects: 0,
                report_reference: "inline".into(),
                sample: "body { color: red }".into(),
                inline_source: Some("body { color: red }".into()),
                blocked: false,
            }]
        );
        style.set_attribute("media", "all");
        let (css, checks) = load(&mut loader, &style, &policy);
        assert!(css.starts_with("@media all"));
        assert!(checks.is_empty());
        let old = style.child_nodes()[0].clone();
        style.remove_child(&old).unwrap();
        style.append_child(NodeHandle::text("body { color: red }"));
        assert_eq!(load(&mut loader, &style, &policy).1.len(), 1);
        let denied = CspPolicy::from_headers_and_document(
            &["style-src 'none'".into()],
            &document,
            "https://example.test/document",
        );
        let (css, checks) = load(&mut loader, &style, &denied);
        assert!(css.is_empty());
        assert!(checks[0].blocked);
        let (css, checks) = load(&mut loader, &style, &denied);
        assert!(css.is_empty());
        assert!(checks.is_empty());
    }

    #[test]
    fn each_new_link_checks_cached_data_responses_while_rebuilds_remain_silent() {
        let document = TreeBuilder::parse(
            "<link id=a rel=stylesheet href='data:text/css,body%7Bcolor:red%7D'>\
             <link id=b rel=stylesheet href='data:text/css,body%7Bcolor:red%7D'>",
        )
        .document();
        let first = document.query_selector("#a").unwrap();
        let second = document.query_selector("#b").unwrap();
        let mut loader = StylesheetLoader::default();
        let policy = CspPolicy::default();
        for node in [&first, &second] {
            let (css, checks) = load(&mut loader, node, &policy);
            assert!(css.contains("color:red"));
            assert_eq!(checks.len(), 2);
            assert!(
                checks
                    .iter()
                    .all(|check| !check.blocked && check.redirects == 0)
            );
            assert!(load(&mut loader, node, &policy).1.is_empty());
        }
    }

    #[test]
    fn cached_stylesheet_checks_keep_redirect_hosts_and_the_original_reporting_url() {
        let original = "https://example.test/allowed/start.css";
        let resource = Resource {
            text: "body { color: red }".into(),
            url: Some("https://example.test/final.css".parse().unwrap()),
            redirects: 2,
            requests: vec![
                (original.into(), 0),
                ("https://other.test/middle.css".into(), 1),
                ("https://example.test/final.css".into(), 2),
            ],
        };
        let document = TreeBuilder::parse("<body>").document();
        let policy = CspPolicy::from_headers_and_document(
            &["style-src https://example.test/allowed/".into()],
            &document,
            "https://example.test/document",
        );
        let mut checks = Vec::new();
        assert!(!check_cached_resource(
            &resource,
            &policy,
            original,
            &mut checks
        ));
        assert_eq!(
            checks,
            vec![StylePolicyCheck {
                reference: "https://other.test/middle.css".into(),
                redirects: 1,
                report_reference: original.into(),
                sample: String::new(),
                inline_source: None,
                blocked: true,
            }]
        );
        let resource = Resource {
            requests: vec![(original.into(), 0)],
            ..resource
        };
        checks.clear();
        assert!(check_cached_resource(
            &resource,
            &policy,
            original,
            &mut checks
        ));
        assert_eq!(checks[0].reference, "https://example.test/final.css");
        assert_eq!(checks[0].redirects, 2);
        assert!(!checks[0].blocked);
    }

    #[test]
    fn stylesheet_uses_top_level_site_for_samesite_cookies() {
        let listener = bind_loopback().unwrap();
        let target: Url = format!("http://{}/style.css", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let server = FixtureWorker::spawn(move || {
            let mut cookies = Vec::new();
            for _ in 0..2 {
                let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
                let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                let cookie = request
                    .lines()
                    .find_map(|line| line.strip_prefix("Cookie: "))
                    .map(|value| value.trim().to_string());
                cookies.push(cookie);
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/css\r\nContent-Length: 19\r\nConnection: close\r\n\r\nbody { color: red }").unwrap();
            }
            cookies
        });
        let store = Arc::new(Mutex::new(CookieJar::new()));
        for header in [
            "strict=1; SameSite=Strict; Path=/",
            "lax=2; SameSite=Lax; Path=/",
        ] {
            store
                .lock()
                .unwrap()
                .add_from_header_for_url(header, &target);
        }
        let document = TreeBuilder::parse(&format!(
            "<html><head><link rel='stylesheet' href='{}'></head></html>",
            target
        ))
        .document();
        let link = document.query_selector("link").unwrap();
        let other: Url = "http://localhost/".parse().unwrap();
        for site in [&other, &target] {
            let mut loader = StylesheetLoader::default();
            let (css, checks) = loader.load_node(
                &link,
                Some(&target),
                &CspPolicy::default(),
                CookieRequestContext::new(Some(site.clone()), None, false),
                Arc::clone(&store),
            );
            assert!(checks.iter().all(|check| !check.blocked));
            assert_eq!(css, "body { color: red }");
        }
        assert_eq!(server.join(), [None, Some("strict=1; lax=2".to_string())]);
    }

    #[test]
    fn third_party_stylesheet_sends_and_stores_cookies_only_with_document_access() {
        let listener = bind_loopback().unwrap();
        let address = listener.local_addr().unwrap();
        let target: Url = format!("http://{address}/style.css").parse().unwrap();
        let server = FixtureWorker::spawn(move || {
            let mut cookies = Vec::new();
            for phase in ["pre", "post"] {
                for (resource, body) in [
                    ("style.css", "@import url(import.css);"),
                    ("import.css", "body { color: red }"),
                ] {
                    let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
                    let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                    assert!(request.starts_with(&format!("GET /{resource} HTTP/1.1")));
                    cookies.push(
                        request
                            .lines()
                            .find_map(|line| line.strip_prefix("Cookie: "))
                            .map(str::trim)
                            .map(str::to_owned),
                    );
                    let cookie_name = resource.split('.').next().unwrap();
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/css\r\nContent-Length: {}\r\nSet-Cookie: {phase}_{cookie_name}=1; Path=/; Secure; SameSite=None\r\nConnection: close\r\n\r\n{body}",
                        body.len(),
                    )
                    .unwrap();
                }
            }
            cookies
        });
        let store = Arc::new(Mutex::new(CookieJar::new()));
        store
            .lock()
            .unwrap()
            .add_from_header_for_url("session=pre; Path=/; Secure; SameSite=None", &target);
        let top: Url = format!("http://localhost:{}/", address.port())
            .parse()
            .unwrap();
        let mut loader = StylesheetLoader::default();
        for phase in ["pre", "post"] {
            let document = TreeBuilder::parse(&format!(
                "<html><head><link rel='stylesheet' href='http://{address}/style.css'></head></html>"
            ))
            .document();
            let link = document.query_selector("link").unwrap();
            let granted = (phase == "post").then(|| target.clone());
            let (css, checks) = loader.load_node(
                &link,
                Some(&target),
                &CspPolicy::default(),
                CookieRequestContext::new(Some(top.clone()), granted, false),
                Arc::clone(&store),
            );
            assert!(checks.iter().all(|check| !check.blocked));
            assert!(css.contains("body { color: red }"));
        }
        let requests = server.join();
        assert_eq!(requests[0], None);
        assert_eq!(requests[1], None);
        assert_eq!(requests[2], Some("session=pre".to_string()));
        assert!(requests[3].as_deref().is_some_and(
            |cookie| cookie.contains("session=pre") && cookie.contains("post_style=1")
        ));
        let cookies = store.lock().unwrap().document_cookie(&target, &target);
        assert!(!cookies.contains("pre_style=1"));
        assert!(!cookies.contains("pre_import=1"));
        assert!(cookies.contains("post_style=1"));
        assert!(cookies.contains("post_import=1"));
    }
}
