//! HTTP request construction and serialization.

use std::fmt;

use super::url::Url;

/// Returns the default `User-Agent` header sent by Omoikane HTTP requests.
pub fn default_user_agent() -> String {
    format!(
        "Omoikane/{} {}",
        env!("CARGO_PKG_VERSION"),
        target_os_name()
    )
}

fn target_os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macOS",
        "linux" => "Linux",
        "windows" => "Windows",
        other => other,
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
}

impl HttpRequest {
    /// Creates a new request with the given method and URL.
    ///
    /// Automatically adds `Host` and `User-Agent` headers derived from the URL
    /// and crate version.
    pub fn new(method: Method, url: Url) -> Self {
        let host = url.authority();
        let mut req = Self {
            method,
            url,
            headers: Vec::new(),
            body: None,
        };
        req.headers.push(("Host".to_string(), host));
        req.headers
            .push(("User-Agent".to_string(), default_user_agent()));
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

    /// Returns the first header value matching `name`, ignoring ASCII case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Appends a header. Does not check for duplicates.
    pub fn add_header(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.headers.push((name.into(), value.into()));
    }

    /// Sets a header, replacing any existing values with the same name.
    pub fn set_header(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        self.headers
            .retain(|(header_name, _)| !header_name.eq_ignore_ascii_case(&name));
        self.headers.push((name, value.into()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_request_serialization() {
        let req = HttpRequest::get("http://example.com/path?q=1").unwrap();
        let text = String::from_utf8(req.serialize()).unwrap();
        let default_user_agent = default_user_agent();

        assert!(text.starts_with("GET /path?q=1 HTTP/1.1\r\n"));
        assert!(text.contains("Host: example.com\r\n"));
        assert!(text.contains(&format!("User-Agent: {default_user_agent}\r\n")));
        assert!(text.ends_with("\r\n\r\n"));
    }

    #[test]
    fn post_request_with_body() {
        let body = b"hello=world".to_vec();
        let req = HttpRequest::post("http://example.com/submit", body.clone()).unwrap();
        let bytes = req.serialize();
        let text = String::from_utf8(bytes).unwrap();

        assert!(text.starts_with("POST /submit HTTP/1.1\r\n"));
        assert!(text.contains("Content-Length: 11\r\n"));
        assert!(text.ends_with("\r\n\r\nhello=world"));
    }

    #[test]
    fn host_header_includes_custom_port() {
        let req = HttpRequest::get("http://localhost:9090/api").unwrap();
        let text = String::from_utf8(req.serialize()).unwrap();

        assert!(text.contains("Host: localhost:9090\r\n"));
    }

    #[test]
    fn host_header_omits_default_port() {
        let req = HttpRequest::get("http://example.com:80/").unwrap();
        let text = String::from_utf8(req.serialize()).unwrap();

        assert!(text.contains("Host: example.com\r\n"));
    }

    #[test]
    fn add_custom_header() {
        let mut req = HttpRequest::get("http://example.com").unwrap();
        req.add_header("Accept", "text/html");
        let text = String::from_utf8(req.serialize()).unwrap();

        assert!(text.contains("Accept: text/html\r\n"));
    }

    #[test]
    fn set_header_replaces_existing_value_case_insensitively() {
        let mut req = HttpRequest::get("http://example.com").unwrap();
        req.set_header("user-agent", "TestAgent/1.0");
        let text = String::from_utf8(req.serialize()).unwrap();

        assert!(text.contains("user-agent: TestAgent/1.0\r\n"));
        assert_eq!(text.matches("User-Agent:").count(), 0);
        assert_eq!(text.matches("user-agent:").count(), 1);
    }

    #[test]
    fn method_display() {
        assert_eq!(Method::Get.to_string(), "GET");
        assert_eq!(Method::Post.to_string(), "POST");
        assert_eq!(Method::Delete.to_string(), "DELETE");
    }

    #[test]
    fn set_body_replaces_content_length() {
        let mut req = HttpRequest::get("http://example.com").unwrap();
        req.set_body(b"abc".to_vec());
        req.set_body(b"abcdef".to_vec());

        let cl_count = req
            .headers()
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .count();
        assert_eq!(cl_count, 1);

        let cl_val = req
            .headers()
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(cl_val, "6");
    }
}
