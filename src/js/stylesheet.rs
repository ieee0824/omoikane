//! Document-owned stylesheet resources shared by CSSOM geometry and painting.

use super::csp::{CspPolicy, ResourceType};
use crate::dom::NodeHandle;
use crate::error_reporting::{
    ErrorCategory, ErrorCode, ErrorReporter, ErrorSeverity, ExecutionSurface, RawEvent,
};
use crate::http::{Client, CookieJar, HttpRequest, Url};
use crate::paint::{DataUri, image::parse_data_uri, stylesheet as css};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_IMPORT_DEPTH: usize = 5;

#[derive(Clone)]
struct Resource {
    text: String,
    url: Option<Url>,
    redirects: usize,
}

/// Retains each document's resource responses across DOM/style invalidations.
#[derive(Default)]
pub(super) struct StylesheetLoader {
    resources: HashMap<String, Option<Resource>>,
    client: Client,
    site_for_cookies: Option<Url>,
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
    pub(super) fn set_error_reporter(
        &mut self,
        reporter: Option<(Arc<ErrorReporter>, ExecutionSurface)>,
    ) {
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
        site_for_cookies: Option<&Url>,
        cookies: Arc<Mutex<CookieJar>>,
    ) -> (String, Vec<String>) {
        self.client.set_shared_cookie_store(cookies);
        self.site_for_cookies = site_for_cookies.cloned();
        let mut blocked = Vec::new();
        if node.get_attribute("disabled").is_some() {
            return (String::new(), blocked);
        }
        let mut output = Vec::new();
        let mut active = HashSet::new();
        if node.tag_name().as_deref() == Some("style") {
            if policy.allows_inline(ResourceType::Style) {
                let resource = Resource {
                    text: css::collect_text_contents(node),
                    url: base.cloned(),
                    redirects: 0,
                };
                self.expand(
                    resource,
                    base,
                    policy,
                    0,
                    &mut active,
                    &mut output,
                    &mut blocked,
                );
            } else {
                blocked.push(format!("style-element:{}", node.identity()));
            }
        } else if node.get_attribute("rel").is_some_and(|rel| {
            rel.split_whitespace()
                .any(|word| word.eq_ignore_ascii_case("stylesheet"))
        }) {
            if let Some(href) = node.get_attribute("href") {
                if let Some(resource) = self.fetch(href.trim(), base, base, policy, &mut blocked) {
                    self.expand(
                        resource,
                        base,
                        policy,
                        0,
                        &mut active,
                        &mut output,
                        &mut blocked,
                    );
                }
            }
        }
        let mut text = output.join("\n");
        if let Some(media) = node
            .get_attribute("media")
            .filter(|media| !media.trim().is_empty())
        {
            text = format!("@media {media} {{\n{text}\n}}");
        }
        (text, blocked)
    }

    fn fetch(
        &mut self,
        href: &str,
        base: Option<&Url>,
        document_base: Option<&Url>,
        policy: &CspPolicy,
        blocked: &mut Vec<String>,
    ) -> Option<Resource> {
        if href.is_empty() {
            return None;
        }
        let url = if href.starts_with("data:") {
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
            .map_or_else(|| href.to_string(), ToString::to_string);
        if !policy.allows_reference(ResourceType::Style, &key) {
            blocked.push(key);
            return None;
        }
        if let Some(cached) = self.resources.get(&key) {
            if let Some(resource) = cached {
                if let Some(url) = &resource.url {
                    if !policy.allows_url_after_redirects(
                        ResourceType::Style,
                        url,
                        resource.redirects,
                    ) {
                        blocked.push(url.to_string());
                        return None;
                    }
                }
            }
            return cached.clone();
        }
        let blocked_before = blocked.len();
        let resource = if let Some(url) = url {
            let same_origin = document_base.is_some_and(|base| {
                base.scheme() == url.scheme()
                    && base.host() == url.host()
                    && base.port() == url.port()
            });
            let response = HttpRequest::get(&key).ok().and_then(|mut request| {
                if !same_origin {
                    request.require_public_ip();
                }
                if let Some(site) = self.site_for_cookies.as_ref().or(document_base) {
                    request.set_cookie_context(site.clone(), false);
                }
                self.client.send(request).ok()
            });
            response.and_then(|response| {
                let effective = response.effective_url().cloned().unwrap_or(url);
                if !policy.allows_url_after_redirects(
                    ResourceType::Style,
                    &effective,
                    response.redirect_count(),
                ) {
                    blocked.push(effective.to_string());
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
                Some(Resource {
                    text: std::str::from_utf8(response.body()).ok()?.to_string(),
                    url: Some(effective),
                    redirects: response.redirect_count(),
                })
            })
        } else if href.starts_with("data:text/css") {
            parse_data_uri(href).ok().and_then(|data| {
                let text = match data {
                    DataUri::Text { data, .. } => data,
                    DataUri::Binary { data, .. } => String::from_utf8(data).ok()?,
                };
                (text.len() <= MAX_BYTES).then_some(Resource {
                    text,
                    url: None,
                    redirects: 0,
                })
            })
        } else {
            None
        };
        if resource.is_none() && blocked.len() == blocked_before {
            self.report_fetch_failure();
        }
        self.resources.insert(key, resource.clone());
        resource
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
        blocked: &mut Vec<String>,
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
                            blocked,
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
                                blocked,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::TreeBuilder;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn stylesheet_uses_top_level_site_for_samesite_cookies() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target: Url = format!("http://{}/style.css", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let server = std::thread::spawn(move || {
            let mut cookies = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut cookie = None;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.strip_prefix("Cookie: ") {
                        cookie = Some(value.trim().to_string());
                    }
                }
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
            let (css, blocked) = loader.load_node(
                &link,
                Some(&target),
                &CspPolicy::default(),
                Some(site),
                Arc::clone(&store),
            );
            assert!(blocked.is_empty());
            assert_eq!(css, "body { color: red }");
        }
        assert_eq!(
            server.join().unwrap(),
            [None, Some("strict=1; lax=2".to_string())]
        );
    }
}
