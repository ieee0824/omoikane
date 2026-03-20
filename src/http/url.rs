//! URL parsing for HTTP(S) URLs.

use std::fmt;

/// A parsed HTTP or HTTPS URL.
///
/// Supports the format: `scheme://host[:port][/path][?query]`
///
/// # Examples
///
/// ```
/// use omoikane::http::Url;
///
/// let url: Url = "http://example.com:8080/path?key=val".parse().unwrap();
/// assert_eq!(url.scheme(), "http");
/// assert_eq!(url.host(), "example.com");
/// assert_eq!(url.port(), 8080);
/// assert_eq!(url.path(), "/path");
/// assert_eq!(url.query(), Some("key=val"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    scheme: String,
    host: String,
    port: u16,
    path: String,
    query: Option<String>,
}

impl Url {
    /// Returns the URL scheme (`"http"` or `"https"`).
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// Returns the hostname.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Returns the port number.
    ///
    /// Defaults to `80` for HTTP and `443` for HTTPS when not explicitly specified.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Returns the request path. Defaults to `"/"` when not specified.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the query string, if present (without the leading `?`).
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    /// Returns the `host:port` pair formatted for the HTTP `Host` header.
    ///
    /// Omits the port when it matches the default for the scheme.
    pub fn authority(&self) -> String {
        let default_port = default_port_for(&self.scheme);
        if self.port == default_port {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// Returns path and query combined, suitable for the HTTP request line.
    pub fn request_target(&self) -> String {
        match &self.query {
            Some(q) => format!("{}?{}", self.path, q),
            None => self.path.clone(),
        }
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}://{}{}",
            self.scheme,
            self.authority(),
            self.request_target()
        )
    }
}

/// Errors that can occur when parsing a URL string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlParseError {
    /// The scheme is missing or unsupported (only `http` and `https` are accepted).
    UnsupportedScheme,
    /// The `://` separator is missing after the scheme.
    MissingSchemeSeparator,
    /// The host portion is empty.
    EmptyHost,
    /// The port number could not be parsed as a valid `u16`.
    InvalidPort,
}

impl fmt::Display for UrlParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedScheme => write!(f, "unsupported or missing URL scheme"),
            Self::MissingSchemeSeparator => write!(f, "missing '://' in URL"),
            Self::EmptyHost => write!(f, "empty host in URL"),
            Self::InvalidPort => write!(f, "invalid port number"),
        }
    }
}

impl std::error::Error for UrlParseError {}

impl std::str::FromStr for Url {
    type Err = UrlParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // scheme
        let (scheme, rest) = s
            .split_once("://")
            .ok_or(UrlParseError::MissingSchemeSeparator)?;
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(UrlParseError::UnsupportedScheme);
        }

        // Split authority from path+query.
        // Authority ends at the first '/' or '?' (handles "http://host?q=1").
        let authority_end = rest
            .find(|c: char| c == '/' || c == '?')
            .unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        let path_and_query = if authority_end < rest.len() {
            let remainder = &rest[authority_end..];
            // If remainder starts with '?', there is no path — prepend '/'.
            if remainder.starts_with('?') {
                // We'll handle this by treating path as "/" and the rest as query below.
                remainder
            } else {
                remainder
            }
        } else {
            "/"
        };

        // host:port
        let (host, port) = if let Some(colon) = authority.rfind(':') {
            let host = &authority[..colon];
            let port_str = &authority[colon + 1..];
            let port: u16 = port_str.parse().map_err(|_| UrlParseError::InvalidPort)?;
            (host.to_string(), port)
        } else {
            (authority.to_string(), default_port_for(&scheme))
        };

        if host.is_empty() {
            return Err(UrlParseError::EmptyHost);
        }

        // path ? query
        let (path, query) = if path_and_query.starts_with('?') {
            // No explicit path, e.g. "http://example.com?x=1"
            let q = &path_and_query[1..];
            let query = if q.is_empty() {
                None
            } else {
                Some(q.to_string())
            };
            ("/".to_string(), query)
        } else {
            match path_and_query.find('?') {
                Some(i) => {
                    let q = &path_and_query[i + 1..];
                    let query = if q.is_empty() {
                        None
                    } else {
                        Some(q.to_string())
                    };
                    (path_and_query[..i].to_string(), query)
                }
                None => (path_and_query.to_string(), None),
            }
        };

        Ok(Url {
            scheme,
            host,
            port,
            path,
            query,
        })
    }
}

/// Resolves a potentially relative URL reference against a base URL.
///
/// Handles absolute URLs (returned as-is), protocol-relative (`//host/path`),
/// absolute paths (`/path`), and relative paths (`path`, `../path`).
///
/// # Examples
///
/// ```
/// use omoikane::http::Url;
/// use omoikane::http::url::resolve_url;
///
/// let base: Url = "https://example.com/dir/page.html".parse().unwrap();
/// assert_eq!(
///     resolve_url(&base, "/css/style.css").unwrap().to_string(),
///     "https://example.com/css/style.css",
/// );
/// assert_eq!(
///     resolve_url(&base, "other.css").unwrap().to_string(),
///     "https://example.com/dir/other.css",
/// );
/// ```
pub fn resolve_url(base: &Url, reference: &str) -> Result<Url, UrlParseError> {
    let reference = reference.trim();

    // Absolute HTTP(S) URL — parse directly.
    if reference.starts_with("http://") || reference.starts_with("https://") {
        return reference.parse();
    }

    // References with an explicit HTTP(S) scheme but missing "//" (e.g. "http:foo")
    // are not supported by our URL type; let the parser return an error instead of
    // treating them as relative.
    if let Some(colon_idx) = reference.find(':') {
        let scheme = &reference[..colon_idx];
        if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
            return reference.parse();
        }
    }

    // Protocol-relative URL (e.g. "//cdn.example.com/style.css").
    if let Some(rest) = reference.strip_prefix("//") {
        return format!("{}://{}", base.scheme, rest).parse();
    }

    // Absolute path (e.g. "/css/style.css").
    if reference.starts_with('/') {
        let (raw_path, query) = split_path_query(reference);
        let normalized = normalize_path(&raw_path);
        return Ok(Url {
            scheme: base.scheme.clone(),
            host: base.host.clone(),
            port: base.port,
            path: normalized,
            query,
        });
    }

    // Relative path (e.g. "style.css" or "../style.css").
    let base_dir = match base.path.rfind('/') {
        Some(i) => &base.path[..=i],
        None => "/",
    };
    let merged = format!("{}{}", base_dir, reference);
    let (raw_path, query) = split_path_query(&merged);
    let normalized = normalize_path(&raw_path);
    Ok(Url {
        scheme: base.scheme.clone(),
        host: base.host.clone(),
        port: base.port,
        path: normalized,
        query,
    })
}

/// Splits a path-and-query string into `(path, Option<query>)`.
fn split_path_query(s: &str) -> (String, Option<String>) {
    match s.find('?') {
        Some(i) => {
            let q = &s[i + 1..];
            let query = if q.is_empty() { None } else { Some(q.to_string()) };
            (s[..i].to_string(), query)
        }
        None => (s.to_string(), None),
    }
}

/// Removes `.` and `..` segments from an absolute path (RFC 3986 §5.2.4).
fn normalize_path(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "." => {}
            ".." => { segments.pop(); }
            s => segments.push(s),
        }
    }
    let result = segments.join("/");
    if result.starts_with('/') {
        result
    } else {
        format!("/{}", result)
    }
}

fn default_port_for(scheme: &str) -> u16 {
    match scheme {
        "https" => 443,
        _ => 80,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_http_url() {
        let url: Url = "http://example.com".parse().unwrap();
        assert_eq!(url.scheme(), "http");
        assert_eq!(url.host(), "example.com");
        assert_eq!(url.port(), 80);
        assert_eq!(url.path(), "/");
        assert_eq!(url.query(), None);
    }

    #[test]
    fn parse_https_with_default_port() {
        let url: Url = "https://secure.example.com/login".parse().unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.port(), 443);
        assert_eq!(url.path(), "/login");
    }

    #[test]
    fn parse_custom_port() {
        let url: Url = "http://localhost:8080/api".parse().unwrap();
        assert_eq!(url.host(), "localhost");
        assert_eq!(url.port(), 8080);
        assert_eq!(url.path(), "/api");
    }

    #[test]
    fn parse_with_query() {
        let url: Url = "http://example.com/search?q=rust&page=1".parse().unwrap();
        assert_eq!(url.path(), "/search");
        assert_eq!(url.query(), Some("q=rust&page=1"));
    }

    #[test]
    fn parse_trailing_question_mark() {
        let url: Url = "http://example.com/path?".parse().unwrap();
        assert_eq!(url.path(), "/path");
        assert_eq!(url.query(), None);
    }

    #[test]
    fn authority_omits_default_port() {
        let url: Url = "http://example.com".parse().unwrap();
        assert_eq!(url.authority(), "example.com");
    }

    #[test]
    fn authority_includes_custom_port() {
        let url: Url = "http://example.com:9090".parse().unwrap();
        assert_eq!(url.authority(), "example.com:9090");
    }

    #[test]
    fn request_target_with_query() {
        let url: Url = "http://example.com/path?q=1".parse().unwrap();
        assert_eq!(url.request_target(), "/path?q=1");
    }

    #[test]
    fn request_target_without_query() {
        let url: Url = "http://example.com/path".parse().unwrap();
        assert_eq!(url.request_target(), "/path");
    }

    #[test]
    fn parse_no_path_with_query() {
        let url: Url = "http://example.com?x=1".parse().unwrap();
        assert_eq!(url.host(), "example.com");
        assert_eq!(url.path(), "/");
        assert_eq!(url.query(), Some("x=1"));
    }

    #[test]
    fn parse_no_path_with_query_and_port() {
        let url: Url = "http://example.com:8080?x=1&y=2".parse().unwrap();
        assert_eq!(url.host(), "example.com");
        assert_eq!(url.port(), 8080);
        assert_eq!(url.path(), "/");
        assert_eq!(url.query(), Some("x=1&y=2"));
    }

    #[test]
    fn display_roundtrip() {
        let input = "http://example.com:8080/path?q=1";
        let url: Url = input.parse().unwrap();
        assert_eq!(url.to_string(), input);
    }

    #[test]
    fn error_missing_scheme_separator() {
        let err = "http:example.com".parse::<Url>().unwrap_err();
        assert_eq!(err, UrlParseError::MissingSchemeSeparator);
    }

    #[test]
    fn error_unsupported_scheme() {
        let err = "ftp://example.com".parse::<Url>().unwrap_err();
        assert_eq!(err, UrlParseError::UnsupportedScheme);
    }

    #[test]
    fn error_empty_host() {
        let err = "http:///path".parse::<Url>().unwrap_err();
        assert_eq!(err, UrlParseError::EmptyHost);
    }

    #[test]
    fn error_invalid_port() {
        let err = "http://example.com:notaport/path"
            .parse::<Url>()
            .unwrap_err();
        assert_eq!(err, UrlParseError::InvalidPort);
    }

    #[test]
    fn case_insensitive_scheme() {
        let url: Url = "HTTP://EXAMPLE.COM".parse().unwrap();
        assert_eq!(url.scheme(), "http");
    }

    #[test]
    fn resolve_absolute_url() {
        let base: Url = "https://example.com/dir/page.html".parse().unwrap();
        let resolved = resolve_url(&base, "http://other.com/style.css").unwrap();
        assert_eq!(resolved.to_string(), "http://other.com/style.css");
    }

    #[test]
    fn resolve_protocol_relative() {
        let base: Url = "https://example.com/dir/page.html".parse().unwrap();
        let resolved = resolve_url(&base, "//cdn.example.com/style.css").unwrap();
        assert_eq!(resolved.to_string(), "https://cdn.example.com/style.css");
    }

    #[test]
    fn resolve_absolute_path() {
        let base: Url = "https://example.com/dir/page.html".parse().unwrap();
        let resolved = resolve_url(&base, "/css/style.css").unwrap();
        assert_eq!(resolved.to_string(), "https://example.com/css/style.css");
    }

    #[test]
    fn resolve_relative_path() {
        let base: Url = "https://example.com/dir/page.html".parse().unwrap();
        let resolved = resolve_url(&base, "other.css").unwrap();
        assert_eq!(resolved.to_string(), "https://example.com/dir/other.css");
    }

    #[test]
    fn resolve_parent_relative_path() {
        let base: Url = "https://example.com/a/b/page.html".parse().unwrap();
        let resolved = resolve_url(&base, "../style.css").unwrap();
        assert_eq!(resolved.to_string(), "https://example.com/a/style.css");
    }

    #[test]
    fn resolve_with_query() {
        let base: Url = "https://example.com/dir/page.html".parse().unwrap();
        let resolved = resolve_url(&base, "/style.css?v=1").unwrap();
        assert_eq!(resolved.to_string(), "https://example.com/style.css?v=1");
    }
}
