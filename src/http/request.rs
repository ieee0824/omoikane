//! HTTP request construction and serialization.

use std::fmt;

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
    top_level_navigation: bool,
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
            top_level_navigation: true,
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
        self.site_for_cookies = Some(site_for_cookies);
        self.top_level_navigation = top_level_navigation;
    }

    /// Returns the initiating site used for `SameSite` cookie selection.
    pub fn site_for_cookies(&self) -> Option<&Url> {
        self.site_for_cookies.as_ref()
    }

    /// Returns whether this request navigates the top-level browsing context.
    pub fn is_top_level_navigation(&self) -> bool {
        self.top_level_navigation
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
    !name.is_empty()
        && name.bytes().all(|byte| {
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
        && value
            .bytes()
            .all(|byte| byte == b'\t' || byte >= b' ' && byte != 0x7f)
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
