//! High-level HTTP client with cookie management and redirect handling.

use super::connection;
use super::cookie::CookieJar;
use super::request::{HttpRequest, Method, default_user_agent};
use super::response::{HttpParseError, HttpResponse};
use super::url::Url;

/// Maximum number of redirects to follow before aborting.
const DEFAULT_MAX_REDIRECTS: u32 = 10;

/// A high-level HTTP client that automatically manages cookies and follows
/// redirects.
///
/// # Examples
///
/// ```no_run
/// use omoikane::http::Client;
///
/// let mut client = Client::new();
/// let resp = client.get("http://example.com/").unwrap();
/// println!("Status: {}", resp.status_code());
/// ```
#[derive(Debug)]
pub struct Client {
    cookie_jar: CookieJar,
    max_redirects: u32,
    user_agent: String,
}

impl Client {
    /// Creates a new client with default settings.
    pub fn new() -> Self {
        Self {
            cookie_jar: CookieJar::new(),
            max_redirects: DEFAULT_MAX_REDIRECTS,
            user_agent: default_user_agent(),
        }
    }

    /// Sets the maximum number of redirects to follow.
    pub fn set_max_redirects(&mut self, max: u32) {
        self.max_redirects = max;
    }

    /// Returns the client-wide `User-Agent` value.
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// Sets the client-wide `User-Agent` value.
    pub fn set_user_agent(&mut self, user_agent: impl Into<String>) {
        self.user_agent = user_agent.into();
    }

    /// Returns a reference to the client's cookie jar.
    pub fn cookie_jar(&self) -> &CookieJar {
        &self.cookie_jar
    }

    /// Returns a mutable reference to the client's cookie jar.
    pub fn cookie_jar_mut(&mut self) -> &mut CookieJar {
        &mut self.cookie_jar
    }

    /// Sends a GET request to `url`, following redirects and managing cookies.
    pub fn get(&mut self, url: &str) -> Result<HttpResponse, HttpParseError> {
        let request = HttpRequest::get(url).map_err(|e| {
            HttpParseError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
        })?;
        self.send(request)
    }

    /// Sends an [`HttpRequest`], following redirects and managing cookies.
    ///
    /// Cookies from the jar are attached to the request, and `Set-Cookie`
    /// headers in the response are stored in the jar. Redirects (301, 302,
    /// 303, 307, 308) are followed automatically up to
    /// [`Client::set_max_redirects`].
    pub fn send(&mut self, mut request: HttpRequest) -> Result<HttpResponse, HttpParseError> {
        let mut redirects_remaining = self.max_redirects;
        let built_in_user_agent = default_user_agent();

        loop {
            let should_apply_client_user_agent = request
                .header("user-agent")
                .is_none_or(|value| value == built_in_user_agent);
            if should_apply_client_user_agent {
                request.set_header("User-Agent", self.user_agent.clone());
            }

            // Attach cookies
            if let Some(cookie_header) = self.cookie_jar.cookie_header(request.url()) {
                request.add_header("Cookie", cookie_header);
            }

            let response = connection::send(&request)?;

            // Store Set-Cookie headers
            let origin = request.url().clone();
            for (name, value) in response.headers() {
                if name.eq_ignore_ascii_case("set-cookie") {
                    self.cookie_jar.add_from_header_for_url(value, &origin);
                }
            }

            // Check for redirect
            if !is_redirect(response.status_code()) {
                return Ok(response);
            }

            if redirects_remaining == 0 {
                return Err(HttpParseError::TooManyRedirects);
            }
            redirects_remaining -= 1;

            // Get redirect target from Location header
            let location = response
                .header("location")
                .ok_or(HttpParseError::MissingLocation)?;

            let new_url = resolve_redirect_url(request.url(), location)?;

            // Determine method for redirect
            let new_method = redirect_method(response.status_code(), request.method());

            let mut new_request = HttpRequest::new(new_method, new_url);

            // Preserve headers (except Host, which is set by HttpRequest::new)
            for (name, value) in request.headers() {
                if !name.eq_ignore_ascii_case("host")
                    && !name.eq_ignore_ascii_case("cookie")
                    && !name.eq_ignore_ascii_case("content-length")
                {
                    new_request.add_header(name.clone(), value.clone());
                }
            }

            if matches!(response.status_code(), 307 | 308) {
                if let Some(body) = request.body() {
                    new_request.set_body(body.to_vec());
                }
            }

            request = new_request;
        }
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `true` if the status code indicates a redirect.
fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Determines the HTTP method to use after a redirect.
///
/// - 301, 302, 303: change to GET (per common browser behavior)
/// - 307, 308: preserve the original method
fn redirect_method(status: u16, original: Method) -> Method {
    match status {
        301 | 302 | 303 => Method::Get,
        307 | 308 => original,
        _ => original,
    }
}

/// Resolves a `Location` header value against the current URL.
///
/// Handles both absolute URLs and relative paths.
fn resolve_redirect_url(base: &Url, location: &str) -> Result<Url, HttpParseError> {
    // Try absolute URL first
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.parse::<Url>().map_err(|e| {
            HttpParseError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
        });
    }

    // Relative path — resolve against base URL
    let new_path = if location.starts_with('/') {
        location.to_string()
    } else {
        // Relative to current path directory
        let base_path = base.path();
        let dir = match base_path.rfind('/') {
            Some(i) => &base_path[..=i],
            None => "/",
        };
        format!("{}{}", dir, location)
    };

    // Split path and query from new_path
    let (path, query) = if let Some(i) = new_path.find('?') {
        let q = &new_path[i + 1..];
        let query = if q.is_empty() { None } else { Some(q) };
        (&new_path[..i], query)
    } else {
        (new_path.as_str(), None)
    };

    // Construct new URL string and parse
    let new_url_str = match query {
        Some(q) => format!(
            "{}://{}:{}{path}?{q}",
            base.scheme(),
            base.host(),
            base.port()
        ),
        None => format!("{}://{}:{}{path}", base.scheme(), base.host(), base.port()),
    };

    new_url_str
        .parse::<Url>()
        .map_err(|e| HttpParseError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- redirect_method tests ---

    #[test]
    fn redirect_301_becomes_get() {
        assert_eq!(redirect_method(301, Method::Post), Method::Get);
    }

    #[test]
    fn redirect_302_becomes_get() {
        assert_eq!(redirect_method(302, Method::Post), Method::Get);
    }

    #[test]
    fn redirect_303_becomes_get() {
        assert_eq!(redirect_method(303, Method::Post), Method::Get);
    }

    #[test]
    fn redirect_307_preserves_method() {
        assert_eq!(redirect_method(307, Method::Post), Method::Post);
    }

    #[test]
    fn redirect_308_preserves_method() {
        assert_eq!(redirect_method(308, Method::Post), Method::Post);
    }

    #[test]
    fn redirect_307_preserves_request_body() {
        let original = HttpRequest::post("http://example.com/upload", b"hello".to_vec()).unwrap();
        let mut redirected = HttpRequest::new(
            redirect_method(307, original.method()),
            "http://example.com/next".parse().unwrap(),
        );

        if let Some(body) = original.body() {
            redirected.set_body(body.to_vec());
        }

        assert_eq!(redirected.method(), Method::Post);
        assert_eq!(redirected.body(), Some(&b"hello"[..]));
    }

    // --- resolve_redirect_url tests ---

    #[test]
    fn resolve_absolute_url() {
        let base: Url = "http://a.com/page".parse().unwrap();
        let resolved = resolve_redirect_url(&base, "http://b.com/other").unwrap();
        assert_eq!(resolved.host(), "b.com");
        assert_eq!(resolved.path(), "/other");
    }

    #[test]
    fn resolve_absolute_path() {
        let base: Url = "http://example.com/old/page".parse().unwrap();
        let resolved = resolve_redirect_url(&base, "/new/page").unwrap();
        assert_eq!(resolved.host(), "example.com");
        assert_eq!(resolved.path(), "/new/page");
    }

    #[test]
    fn resolve_relative_path() {
        let base: Url = "http://example.com/dir/page".parse().unwrap();
        let resolved = resolve_redirect_url(&base, "other").unwrap();
        assert_eq!(resolved.path(), "/dir/other");
    }

    #[test]
    fn resolve_with_query() {
        let base: Url = "http://example.com/page".parse().unwrap();
        let resolved = resolve_redirect_url(&base, "/new?q=1").unwrap();
        assert_eq!(resolved.path(), "/new");
        assert_eq!(resolved.query(), Some("q=1"));
    }

    // --- is_redirect tests ---

    #[test]
    fn is_redirect_true_for_3xx() {
        assert!(is_redirect(301));
        assert!(is_redirect(302));
        assert!(is_redirect(303));
        assert!(is_redirect(307));
        assert!(is_redirect(308));
    }

    #[test]
    fn is_redirect_false_for_200() {
        assert!(!is_redirect(200));
    }

    // --- Integration tests with local TCP server ---

    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn client_follows_redirect() {
        // Server: first request returns 302 -> /final, second returns 200.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            // First request: 302 redirect
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            // Consume headers
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
            }

            let resp =
                format!("HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\n\r\n");
            stream.write_all(resp.as_bytes()).unwrap();
            stream.flush().unwrap();
            drop(stream);

            // Second request: 200 OK
            let (mut stream2, _) = listener.accept().unwrap();
            let mut reader2 = BufReader::new(&stream2);
            let mut line2 = String::new();
            reader2.read_line(&mut line2).unwrap();
            assert!(
                line2.contains("/final"),
                "expected /final, got: {}",
                line2.trim()
            );
            // Consume headers
            loop {
                let mut h = String::new();
                reader2.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
            }

            let resp2 = "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ndone";
            stream2.write_all(resp2.as_bytes()).unwrap();
            stream2.flush().unwrap();
        });

        let mut client = Client::new();
        let url = format!("http://127.0.0.1:{}/start", port);
        let resp = client.get(&url).unwrap();

        assert_eq!(resp.status_code(), 200);
        assert_eq!(resp.body(), b"done");
    }

    #[test]
    fn client_stores_cookies_across_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            // First request: set cookie
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
            }

            let resp =
                "HTTP/1.1 200 OK\r\nSet-Cookie: token=xyz; Path=/\r\nContent-Length: 2\r\n\r\nok";
            stream.write_all(resp.as_bytes()).unwrap();
            stream.flush().unwrap();
            drop(stream);

            // Second request: verify cookie is sent
            let (mut stream2, _) = listener.accept().unwrap();
            let mut reader2 = BufReader::new(&stream2);
            let mut line2 = String::new();
            reader2.read_line(&mut line2).unwrap();

            let mut cookie_header = None;
            loop {
                let mut h = String::new();
                reader2.read_line(&mut h).unwrap();
                let trimmed = h.trim().to_string();
                if trimmed.is_empty() {
                    break;
                }
                if trimmed.to_ascii_lowercase().starts_with("cookie:") {
                    cookie_header = Some(trimmed);
                }
            }

            assert!(
                cookie_header
                    .as_ref()
                    .map(|h| h.contains("token=xyz"))
                    .unwrap_or(false),
                "expected Cookie header with token=xyz, got: {:?}",
                cookie_header
            );

            let resp2 = "HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\ncookied";
            stream2.write_all(resp2.as_bytes()).unwrap();
            stream2.flush().unwrap();
        });

        let mut client = Client::new();

        // First request — receives Set-Cookie
        let url = format!("http://127.0.0.1:{}/first", port);
        let resp1 = client.get(&url).unwrap();
        assert_eq!(resp1.status_code(), 200);

        // Second request — should send Cookie
        let url2 = format!("http://127.0.0.1:{}/second", port);
        let resp2 = client.get(&url2).unwrap();
        assert_eq!(resp2.status_code(), 200);
        assert_eq!(resp2.body(), b"cookied");
    }

    #[test]
    fn client_detects_redirect_loop() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            // Always respond with redirect to same URL
            for _ in 0..=DEFAULT_MAX_REDIRECTS {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                }

                let resp =
                    format!("HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\n\r\n");
                stream.write_all(resp.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
        });

        let mut client = Client::new();
        let url = format!("http://127.0.0.1:{}/loop", port);
        let result = client.get(&url);

        assert!(result.is_err());
    }

    #[test]
    fn client_uses_default_user_agent() {
        let default_user_agent = default_user_agent();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();

            let mut user_agent = None;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let trimmed = header.trim().to_string();
                if trimmed.is_empty() {
                    break;
                }
                if let Some((name, value)) = trimmed.split_once(':') {
                    if name.trim().eq_ignore_ascii_case("user-agent") {
                        user_agent = Some(value.trim().to_string());
                    }
                }
            }

            assert_eq!(user_agent.as_deref(), Some(default_user_agent.as_str()));

            let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
            stream.write_all(resp.as_bytes()).unwrap();
            stream.flush().unwrap();
        });

        let mut client = Client::new();
        let url = format!("http://127.0.0.1:{port}/ua");
        let resp = client.get(&url).unwrap();
        assert_eq!(resp.status_code(), 200);
    }

    #[test]
    fn client_can_override_default_user_agent() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            for expected_path in ["/start", "/final"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(
                    line.contains(expected_path),
                    "unexpected path: {}",
                    line.trim()
                );

                let mut user_agent = None;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    let trimmed = header.trim().to_string();
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = trimmed.split_once(':') {
                        if name.trim().eq_ignore_ascii_case("user-agent") {
                            user_agent = Some(value.trim().to_string());
                        }
                    }
                }

                assert_eq!(user_agent.as_deref(), Some("CustomAgent/1.0"));

                let resp = if expected_path == "/start" {
                    "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\n\r\n"
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"
                };
                stream.write_all(resp.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
        });

        let mut client = Client::new();
        client.set_user_agent("CustomAgent/1.0");

        let url = format!("http://127.0.0.1:{port}/start");
        let resp = client.get(&url).unwrap();
        assert_eq!(resp.status_code(), 200);
    }

    #[test]
    fn explicit_request_user_agent_wins_over_client_default() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();

            let mut user_agent = None;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let trimmed = header.trim().to_string();
                if trimmed.is_empty() {
                    break;
                }
                if let Some((name, value)) = trimmed.split_once(':') {
                    if name.trim().eq_ignore_ascii_case("user-agent") {
                        user_agent = Some(value.trim().to_string());
                    }
                }
            }

            assert_eq!(user_agent.as_deref(), Some("RequestAgent/2.0"));

            let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
            stream.write_all(resp.as_bytes()).unwrap();
            stream.flush().unwrap();
        });

        let mut client = Client::new();
        client.set_user_agent("ClientAgent/1.0");

        let mut request = HttpRequest::get(&format!("http://127.0.0.1:{port}/ua")).unwrap();
        request.set_header("User-Agent", "RequestAgent/2.0");

        let resp = client.send(request).unwrap();
        assert_eq!(resp.status_code(), 200);
    }
}
