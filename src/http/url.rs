//! URL parsing for HTTP(S) URLs.

use std::fmt;
use url::{Host, Url as StandardUrl};

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
    /// The URL contains another invalid component.
    InvalidUrl,
}

impl fmt::Display for UrlParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedScheme => write!(f, "unsupported or missing URL scheme"),
            Self::MissingSchemeSeparator => write!(f, "missing '://' in URL"),
            Self::EmptyHost => write!(f, "empty host in URL"),
            Self::InvalidPort => write!(f, "invalid port number"),
            Self::InvalidUrl => write!(f, "invalid URL"),
        }
    }
}

impl std::error::Error for UrlParseError {}

impl std::str::FromStr for Url {
    type Err = UrlParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // The URL parser also performs these transformations. Do them before
        // checking the scheme separator so tabs/newlines cannot bypass it.
        let input: String = s
            .trim_matches(|c: char| c <= ' ')
            .chars()
            .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
            .collect();
        let (scheme, authority_and_path) = input
            .split_once("://")
            .ok_or(UrlParseError::MissingSchemeSeparator)?;
        if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
            return Err(UrlParseError::UnsupportedScheme);
        }
        if authority_and_path.is_empty() || authority_and_path.starts_with('/') {
            return Err(UrlParseError::EmptyHost);
        }
        let parsed = StandardUrl::parse(&input).map_err(map_parse_error)?;
        Self::from_standard(parsed)
    }
}

impl Url {
    fn from_standard(parsed: StandardUrl) -> Result<Self, UrlParseError> {
        let scheme = parsed.scheme();
        if !matches!(scheme, "http" | "https") {
            return Err(UrlParseError::UnsupportedScheme);
        }
        let host = match parsed.host() {
            Some(Host::Domain(domain)) => domain.to_owned(),
            Some(Host::Ipv4(ip)) => ip.to_string(),
            Some(Host::Ipv6(ip)) => format!("[{ip}]"),
            None => return Err(UrlParseError::EmptyHost),
        };
        Ok(Self {
            scheme: scheme.to_owned(),
            host,
            port: parsed
                .port_or_known_default()
                .ok_or(UrlParseError::InvalidPort)?,
            path: parsed.path().to_owned(),
            query: parsed
                .query()
                .filter(|query| !query.is_empty())
                .map(str::to_owned),
        })
    }
}

fn map_parse_error(error: url::ParseError) -> UrlParseError {
    match error {
        url::ParseError::EmptyHost => UrlParseError::EmptyHost,
        url::ParseError::InvalidPort => UrlParseError::InvalidPort,
        _ => UrlParseError::InvalidUrl,
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
    let parsed_base = StandardUrl::parse(&base.to_string()).map_err(map_parse_error)?;
    let parsed = parsed_base.join(reference).map_err(map_parse_error)?;
    Url::from_standard(parsed)
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
        assert_eq!(url.host(), "example.com");
    }

    #[test]
    fn parses_userinfo_ipv6_and_canonical_host() {
        let url: Url = "http://user:pass@BÜCHER.Example:80/a".parse().unwrap();
        assert_eq!(url.host(), "xn--bcher-kva.example");
        assert_eq!(url.authority(), "xn--bcher-kva.example");
        assert_eq!(url.request_target(), "/a");
        assert!(!url.to_string().contains("user:pass"));

        let ipv6: Url = "http://[::1]/".parse().unwrap();
        assert_eq!(ipv6.host(), "[::1]");
        assert_eq!(ipv6.authority(), "[::1]");
        assert_eq!(ipv6.port(), 80);

        let encoded: Url = "http://%65XAMPLE.com:/".parse().unwrap();
        assert_eq!(encoded.host(), "example.com");
    }

    #[test]
    fn request_target_excludes_fragment_and_control_characters() {
        let base: Url = "http://example.com/dir/page".parse().unwrap();
        let url = resolve_url(&base, "/a.css\r\nX-Injected: 1#secret").unwrap();
        let target = url.request_target();
        assert!(!target.contains(['\r', '\n', '#']));
        assert!(target.contains("X-Injected"));
        assert!(target.contains("%20"));
        assert_eq!(
            "http://example.com/p#secret"
                .parse::<Url>()
                .unwrap()
                .request_target(),
            "/p"
        );
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

    #[test]
    fn resolve_strips_fragment() {
        let base: Url = "https://example.com/dir/page.html".parse().unwrap();

        // Fragment should be stripped before resolution
        let resolved = resolve_url(&base, "style.css#v2").unwrap();
        assert_eq!(resolved.path(), "/dir/style.css");
        assert_eq!(resolved.query(), None);

        // Absolute path with fragment
        let resolved = resolve_url(&base, "/css/style.css#v1").unwrap();
        assert_eq!(resolved.path(), "/css/style.css");

        // Query + fragment (fragment should be discarded, query preserved)
        let resolved = resolve_url(&base, "/style.css?v=2#section").unwrap();
        assert_eq!(resolved.path(), "/style.css");
        assert_eq!(resolved.query(), Some("v=2"));
    }

    #[test]
    fn resolve_rejects_non_http_schemes() {
        let base: Url = "https://example.com/page.html".parse().unwrap();

        // Non-HTTP(S) schemes with explicit ":" should be rejected per RFC 3986
        assert!(resolve_url(&base, "mailto:foo@example.com").is_err());
        assert!(resolve_url(&base, "ftp://ftp.example.com/file.css").is_err());
        assert!(resolve_url(&base, "data:,foo").is_err());
    }

    #[test]
    fn resolve_edge_case_colon_in_path() {
        let base: Url = "https://example.com/page.html".parse().unwrap();

        // Colon after "/" or "?" should be treated as path content (relative)
        // not as a scheme separator
        let resolved = resolve_url(&base, "path/to:file.css").unwrap();
        assert_eq!(resolved.path(), "/path/to:file.css");

        // But colon before "/" should be a scheme
        assert!(resolve_url(&base, "scheme:path/file.css").is_err());
    }
}
