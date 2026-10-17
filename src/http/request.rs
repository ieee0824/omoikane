//! HTTP request construction and serialization.

use std::fmt;

use super::referrer::{ReferrerPolicy, navigation_referrer_with_policy};
use super::url::Url;

/// Returns the default `User-Agent` header sent by Omoikane HTTP requests.
pub fn default_user_agent() -> String {
    format!(
        "Mozilla/5.0 ({}) Gecko/20100101 Firefox/140.0 Omoikane/{}",
        compatibility_platform(),
        env!("CARGO_PKG_VERSION"),
    )
}

fn compatibility_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "Macintosh; Intel Mac OS X 10.15; rv:140.0",
        "windows" => "Windows NT 10.0; Win64; x64; rv:140.0",
        _ => "X11; Linux x86_64; rv:140.0",
    }
}

/// HTTP request methods.
///
/// Covers the methods most commonly needed by a headless browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Head,
    Put,
    Delete,
    Options,
    Patch,
}

impl Method {
    /// Returns the method name as an uppercase ASCII string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Head => "HEAD",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Options => "OPTIONS",
            Self::Patch => "PATCH",
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An HTTP/1.1 request.
///
/// # Examples
///
/// ```
/// use omoikane::http::{HttpRequest, Method};
///
/// let req = HttpRequest::get("http://example.com/page").unwrap();
/// assert_eq!(req.method(), Method::Get);
///
/// let bytes = req.serialize();
/// let text = String::from_utf8(bytes).unwrap();
/// assert!(text.starts_with("GET /page HTTP/1.1\r\n"));
/// ```
#[derive(Debug, Clone)]
pub struct HttpRequest {
    method: Method,
    url: Url,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    require_public_ip: bool,
    site_for_cookies: Option<Url>,
    cookie_top_level_site: Option<super::SchemefulSite>,
    top_level_navigation: bool,
    referrer_source: Option<(String, ReferrerPolicy)>,
    storage_access_eligible: bool,
    cross_site_ancestry: bool,
}

/// Owned cookie policy captured from the context that initiated a resource load.
///
/// Resource loaders may outlive a DOM borrow or run on another thread. Keeping
/// the site, activated origin, and ancestor flag together prevents a loader
/// from accidentally treating an embedded request as a top-level navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CookieRequestContext {
    site_for_cookies: Option<Url>,
    granted_origin: Option<Url>,
    top_level_site: Option<super::SchemefulSite>,
    cross_site_ancestry: bool,
}

impl CookieRequestContext {
    /// Stores the initiating site, the permitted origin, and ancestry for later requests.
    pub(crate) fn new(
        site_for_cookies: Option<Url>,
        granted_origin: Option<Url>,
        cross_site_ancestry: bool,
    ) -> Self {
        let top_level_site = site_for_cookies
            .as_ref()
            .map(super::SchemefulSite::from_url);
        Self {
            top_level_site,
            site_for_cookies,
            granted_origin,
            cross_site_ancestry,
        }
    }

    /// Overrides the real top-level site independently of cross-site cookie ancestry.
    pub(crate) fn with_top_level_site(mut self, site: Option<super::SchemefulSite>) -> Self {
        self.top_level_site = site;
        self
    }

    /// Returns the inherited SameSite context, separately from the partition key.
    pub(crate) fn cookie_site(&self) -> Option<&Url> {
        self.site_for_cookies.as_ref()
    }

    pub(crate) fn top_level_site(&self) -> Option<super::SchemefulSite> {
        self.top_level_site.clone()
    }

    /// Keeps the container's cookie site and ancestry, copying only an eligible
    /// navigating Document's activated origin. No source means no activation.
    pub(crate) fn for_navigation(mut self, source: Option<&Self>) -> Self {
        self.granted_origin = source.and_then(|source| source.granted_origin.clone());
        self
    }

    /// Keeps the creator's site and ancestry for a Worker while dropping its
    /// Document-only Storage Access grant. An unknown creator site fails closed.
    pub(crate) fn for_worker_internal(mut self) -> Self {
        self.granted_origin = None;
        self.cross_site_ancestry |= self.site_for_cookies.is_none();
        self
    }

    /// Returns whether any ancestor of the owning Document is cross-site.
    pub(crate) fn has_cross_site_ancestry(&self) -> bool {
        self.cross_site_ancestry
    }

    /// Restricts an existing shared Worker after a cross-site owner connects.
    pub(crate) fn with_cross_site_ancestry(mut self) -> Self {
        self.granted_origin = None;
        self.cross_site_ancestry = true;
        self
    }

    /// Applies the initiating context's policy to one concrete request URL.
    pub(crate) fn apply(&self, request: &mut HttpRequest) {
        let eligible = self.granted_origin.as_ref().is_some_and(|origin| {
            origin.scheme() == request.url().scheme()
                && origin.host() == request.url().host()
                && origin.port() == request.url().port()
        });
        request.set_cookie_context(
            self.site_for_cookies
                .clone()
                .unwrap_or_else(|| request.url().clone()),
            false,
        );
        request.cookie_top_level_site = self.top_level_site.clone();
        request.set_storage_access_eligible(eligible);
        request.set_cross_site_ancestry(self.cross_site_ancestry);
    }

    /// Applies a WebSocket handshake's cookie policy. The WebSocket service
    /// may use a different port from the initiating Document on the same host.
    pub(crate) fn apply_websocket(&self, request: &mut HttpRequest) {
        self.apply(request);
        if self.granted_origin.as_ref().is_some_and(|origin| {
            origin.scheme() == request.url().scheme() && origin.host() == request.url().host()
        }) {
            request.set_storage_access_eligible(true);
        }
    }
}

impl HttpRequest {
    /// Creates a new request with the given method and URL.
    ///
    /// Automatically adds `Host`, `User-Agent`, `Accept-Language`, and `Accept-Encoding` headers
    /// derived from the URL and crate version.
    pub fn new(method: Method, url: Url) -> Self {
        let host = url.authority();
        let mut req = Self {
            method,
            url,
            headers: Vec::new(),
            body: None,
            require_public_ip: false,
            site_for_cookies: None,
            cookie_top_level_site: None,
            top_level_navigation: true,
            referrer_source: None,
            storage_access_eligible: false,
            cross_site_ancestry: false,
        };
        req.headers.push(("Host".to_string(), host));
        req.headers
            .push(("User-Agent".to_string(), default_user_agent()));
        req.headers
            .push(("Accept-Language".to_string(), "en-US,en;q=0.5".to_string()));
        req.headers
            .push(("Accept-Encoding".to_string(), "gzip".to_string()));
        req
    }

    /// Convenience constructor for a GET request.
    pub fn get(url: &str) -> Result<Self, super::url::UrlParseError> {
        let url: Url = url.parse()?;
        Ok(Self::new(Method::Get, url))
    }

    /// Convenience constructor for a POST request with a body.
    pub fn post(url: &str, body: Vec<u8>) -> Result<Self, super::url::UrlParseError> {
        let url: Url = url.parse()?;
        let mut req = Self::new(Method::Post, url);
        req.set_body(body);
        Ok(req)
    }

    /// Returns the request method.
    pub fn method(&self) -> Method {
        self.method
    }

    /// Returns the target URL.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Returns the request headers.
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// Returns the request body, if present.
    pub fn body(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }

    pub(crate) fn require_public_ip(&mut self) {
        self.require_public_ip = true;
    }

    pub(crate) fn requires_public_ip(&self) -> bool {
        self.require_public_ip
    }

    /// Sets the initiating site and whether this request navigates the top-level context.
    pub fn set_cookie_context(&mut self, site_for_cookies: Url, top_level_navigation: bool) {
        self.cookie_top_level_site = Some(super::SchemefulSite::from_url(&site_for_cookies));
        self.site_for_cookies = Some(site_for_cookies);
        self.top_level_navigation = top_level_navigation;
    }

    /// Returns the initiating site used for `SameSite` cookie selection.
    pub fn site_for_cookies(&self) -> Option<&Url> {
        self.site_for_cookies.as_ref()
    }

    /// Preserves the partition namespace when constructing a redirect request.
    pub(crate) fn copy_cookie_partition_from(&mut self, source: &Self) {
        self.cookie_top_level_site = source.cookie_top_level_site.clone();
    }

    pub(crate) fn cookie_access_context(&self, unpartitioned: bool) -> super::CookieAccessContext {
        super::CookieAccessContext {
            top_level_site: if self.top_level_navigation {
                Some(super::SchemefulSite::from_url(&self.url))
            } else {
                self.cookie_top_level_site.clone()
            },
            unpartitioned,
        }
    }

    /// Returns whether this request navigates the top-level browsing context.
    pub fn is_top_level_navigation(&self) -> bool {
        self.top_level_navigation
    }

    /// Marks a same-origin request from a Document that activated Storage Access.
    pub(crate) fn set_storage_access_eligible(&mut self, eligible: bool) {
        self.storage_access_eligible = eligible;
    }

    /// Returns whether this request may use its Document's Storage Access grant.
    pub(crate) fn storage_access_eligible(&self) -> bool {
        self.storage_access_eligible
    }

    /// Marks an embedded request whose Document has a cross-site ancestor.
    pub(crate) fn set_cross_site_ancestry(&mut self, cross_site: bool) {
        self.cross_site_ancestry = cross_site;
    }

    /// Reports whether the initiating Document has a cross-site ancestor.
    pub(crate) fn has_cross_site_ancestry(&self) -> bool {
        self.cross_site_ancestry
    }

    /// Returns the first header value matching `name`, ignoring ASCII case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Appends a header. Invalid names or values are ignored.
    /// Use [`Self::try_add_header`] when the caller needs to detect rejection.
    pub fn add_header(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.try_add_header(name, value);
    }

    /// Appends a valid header and reports whether it was accepted.
    pub fn try_add_header(&mut self, name: impl Into<String>, value: impl Into<String>) -> bool {
        let name = name.into();
        let value = value.into();
        if !is_valid_header(&name, &value) {
            return false;
        }
        self.headers.push((name, value));
        true
    }

    /// Sets a header, replacing any existing values with the same name.
    /// Invalid names or values are ignored.
    pub fn set_header(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.try_set_header(name, value);
    }

    /// Sets a valid header and reports whether it was accepted.
    pub fn try_set_header(&mut self, name: impl Into<String>, value: impl Into<String>) -> bool {
        let name = name.into();
        let value = value.into();
        if !is_valid_header(&name, &value) {
            return false;
        }
        self.headers
            .retain(|(header_name, _)| !header_name.eq_ignore_ascii_case(&name));
        self.headers.push((name, value));
        true
    }

    /// Removes every header whose name matches `name`, ignoring ASCII case.
    pub fn remove_header(&mut self, name: &str) {
        self.headers
            .retain(|(header_name, _)| !header_name.eq_ignore_ascii_case(name));
    }

    /// Captures the navigation initiator for redirect-time policy updates.
    pub(crate) fn set_referrer_source(&mut self, source: String, policy: ReferrerPolicy) {
        let value = navigation_referrer_with_policy(&source, &self.url.to_string(), policy);
        self.referrer_source = Some((source, policy));
        self.remove_header("Referer");
        if !value.is_empty() {
            self.set_header("Referer", value);
        }
    }

    pub(crate) fn referrer_source(&self) -> Option<(&str, ReferrerPolicy)> {
        self.referrer_source
            .as_ref()
            .map(|(source, policy)| (source.as_str(), *policy))
    }

    /// Sets the request body and automatically sets the `Content-Length` header.
    pub fn set_body(&mut self, body: Vec<u8>) {
        // Remove any existing Content-Length
        self.headers
            .retain(|(k, _)| !k.eq_ignore_ascii_case("content-length"));
        self.headers
            .push(("Content-Length".to_string(), body.len().to_string()));
        self.body = Some(body);
    }

    /// Serializes the request into bytes ready to be sent over a TCP connection.
    ///
    /// Produces an HTTP/1.1 request message consisting of the request line,
    /// headers, and optional body.
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Request line
        let line = format!(
            "{} {} HTTP/1.1\r\n",
            self.method.as_str(),
            self.url.request_target(),
        );
        buf.extend_from_slice(line.as_bytes());

        // Headers
        for (name, value) in &self.headers {
            let header = format!("{}: {}\r\n", name, value);
            buf.extend_from_slice(header.as_bytes());
        }

        // End of headers
        buf.extend_from_slice(b"\r\n");

        // Body
        if let Some(body) = &self.body {
            buf.extend_from_slice(body);
        }

        buf
    }
}

/// Validates an HTTP field before it can be serialized into a request.
pub(crate) fn is_valid_header(name: &str, value: &str) -> bool {
    is_http_token(name)
        && value
            .bytes()
            .all(|byte| byte == b'\t' || byte >= b' ' && byte != 0x7f)
}

/// Returns whether `value` is an RFC 9110 `token` (one or more `tchar`s).
pub(crate) fn is_http_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// Returns whether script is forbidden to set this request field.
pub(crate) fn is_forbidden_request_header(name: &str, value: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if name.starts_with("proxy-") || name.starts_with("sec-") {
        return true;
    }
    if [
        "accept-charset",
        "accept-encoding",
        "access-control-request-headers",
        "access-control-request-method",
        "connection",
        "content-length",
        "cookie",
        "cookie2",
        "date",
        "dnt",
        "expect",
        "host",
        "keep-alive",
        "origin",
        "referer",
        "set-cookie",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "via",
    ]
    .contains(&name.as_str())
    {
        return true;
    }
    [
        "x-http-method",
        "x-http-method-override",
        "x-method-override",
    ]
    .contains(&name.as_str())
        && value.split(',').any(|method| {
            ["CONNECT", "TRACE", "TRACK"].contains(&method.trim().to_ascii_uppercase().as_str())
        })
}

/// Whether a caller-supplied header survives an HTTP redirect.
pub(crate) fn copy_header_on_redirect(name: &str, preserve_body: bool, same_origin: bool) -> bool {
    if ["host", "cookie", "content-length", "origin"]
        .iter()
        .any(|internal| name.eq_ignore_ascii_case(internal))
    {
        return false;
    }
    if name.eq_ignore_ascii_case("referer") {
        // The generic HTTP client does not know the original source policy.
        // Preserve the value within the same origin; avoid forwarding a full
        // URL to another origin without recomputing it for that destination.
        return same_origin;
    }
    if !same_origin
        && ["authorization", "proxy-authorization"]
            .iter()
            .any(|credential| name.eq_ignore_ascii_case(credential))
    {
        return false;
    }
    preserve_body
        || ![
            "content-encoding",
            "content-language",
            "content-location",
            "content-type",
        ]
        .iter()
        .any(|body_header| name.eq_ignore_ascii_case(body_header))
}
