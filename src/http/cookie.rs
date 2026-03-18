//! Cookie parsing and storage (RFC 6265).
//!
//! Provides [`Cookie`] for individual cookies parsed from `Set-Cookie` headers,
//! and [`CookieJar`] for storing and retrieving cookies across requests.

use std::time::{Duration, SystemTime};

use super::url::Url;

/// The `SameSite` attribute of a cookie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    /// Cookie is sent with both same-site and cross-site requests.
    None,
    /// Cookie is sent with same-site requests and top-level navigations.
    Lax,
    /// Cookie is only sent with same-site requests.
    Strict,
}

/// A parsed HTTP cookie (from a `Set-Cookie` header).
///
/// # Examples
///
/// ```
/// use omoikane::http::Cookie;
///
/// let cookie = Cookie::parse("session=abc123; Path=/; HttpOnly; Secure").unwrap();
/// assert_eq!(cookie.name(), "session");
/// assert_eq!(cookie.value(), "abc123");
/// assert_eq!(cookie.path(), Some("/"));
/// assert!(cookie.http_only());
/// assert!(cookie.secure());
/// ```
#[derive(Debug, Clone)]
pub struct Cookie {
    name: String,
    value: String,
    domain: Option<String>,
    host_only: bool,
    path: Option<String>,
    created_at: SystemTime,
    expires: Option<SystemTime>,
    max_age: Option<i64>,
    secure: bool,
    http_only: bool,
    same_site: SameSite,
}

impl Cookie {
    /// Parses a `Set-Cookie` header value into a `Cookie`.
    ///
    /// The first `name=value` pair is the cookie itself; subsequent
    /// semicolon-separated attributes set domain, path, expiry, etc.
    pub fn parse(header_value: &str) -> Option<Self> {
        let mut parts = header_value.splitn(2, ';');
        let name_value = parts.next()?.trim();

        let (name, value) = name_value.split_once('=')?;
        let name = name.trim().to_string();
        let value = value.trim().to_string();

        if name.is_empty() {
            return None;
        }

        let mut cookie = Cookie {
            name,
            value,
            domain: None,
            host_only: true,
            path: None,
            created_at: SystemTime::now(),
            expires: None,
            max_age: None,
            secure: false,
            http_only: false,
            same_site: SameSite::Lax,
        };

        // Parse attributes
        if let Some(attrs_str) = parts.next() {
            for attr in attrs_str.split(';') {
                let attr = attr.trim();
                if attr.is_empty() {
                    continue;
                }

                if let Some((attr_name, attr_value)) = attr.split_once('=') {
                    let attr_name = attr_name.trim();
                    let attr_value = attr_value.trim();
                    match attr_name.to_ascii_lowercase().as_str() {
                        "domain" => {
                            let d = attr_value.strip_prefix('.').unwrap_or(attr_value);
                            cookie.domain = Some(d.to_ascii_lowercase());
                            cookie.host_only = false;
                        }
                        "path" => {
                            cookie.path = Some(attr_value.to_string());
                        }
                        "expires" => {
                            cookie.expires = parse_http_date(attr_value);
                        }
                        "max-age" => {
                            cookie.max_age = attr_value.parse::<i64>().ok();
                        }
                        "samesite" => {
                            cookie.same_site = match attr_value.to_ascii_lowercase().as_str() {
                                "strict" => SameSite::Strict,
                                "none" => SameSite::None,
                                _ => SameSite::Lax,
                            };
                        }
                        _ => {}
                    }
                } else {
                    match attr.to_ascii_lowercase().as_str() {
                        "secure" => cookie.secure = true,
                        "httponly" => cookie.http_only = true,
                        _ => {}
                    }
                }
            }
        }

        Some(cookie)
    }

    /// Returns the cookie name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the cookie value.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns the `Domain` attribute, if set (always lowercase, no leading dot).
    pub fn domain(&self) -> Option<&str> {
        self.domain.as_deref()
    }

    /// Returns `true` if this is a host-only cookie.
    pub fn host_only(&self) -> bool {
        self.host_only
    }

    /// Returns the `Path` attribute, if set.
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// Returns the `Expires` attribute, if set.
    pub fn expires(&self) -> Option<SystemTime> {
        self.expires
    }

    /// Returns the `Max-Age` attribute in seconds, if set.
    pub fn max_age(&self) -> Option<i64> {
        self.max_age
    }

    /// Returns `true` if the `Secure` flag is set.
    pub fn secure(&self) -> bool {
        self.secure
    }

    /// Returns `true` if the `HttpOnly` flag is set.
    pub fn http_only(&self) -> bool {
        self.http_only
    }

    /// Returns the `SameSite` attribute.
    pub fn same_site(&self) -> SameSite {
        self.same_site
    }

    /// Returns `true` if this cookie has expired relative to `now`.
    fn is_expired(&self, now: SystemTime) -> bool {
        if let Some(max_age) = self.max_age {
            if max_age <= 0 {
                return true;
            }

            if let Ok(elapsed) = now.duration_since(self.created_at) {
                return elapsed >= Duration::from_secs(max_age as u64);
            }

            return false;
        }
        if let Some(expires) = self.expires {
            return now > expires;
        }
        false
    }

    /// Returns `true` if this cookie should be sent with a request to `url`.
    fn matches_url(&self, url: &Url) -> bool {
        if let Some(domain) = &self.domain {
            let matches = if self.host_only {
                url.host().eq_ignore_ascii_case(domain)
            } else {
                domain_matches(url.host(), domain)
            };

            if !matches {
                return false;
            }
        }

        // Path matching
        if let Some(cookie_path) = &self.path {
            if !path_matches(url.path(), cookie_path) {
                return false;
            }
        }

        // Secure flag: only send over HTTPS
        if self.secure && url.scheme() != "https" {
            return false;
        }

        true
    }
}

/// A jar that stores cookies and selects matching ones for outgoing requests.
///
/// # Examples
///
/// ```
/// use omoikane::http::CookieJar;
///
/// let mut jar = CookieJar::new();
/// jar.add_from_header("session=abc; Path=/; Domain=example.com", "example.com");
///
/// let url = "http://example.com/page".parse().unwrap();
/// let header = jar.cookie_header(&url);
/// assert_eq!(header, Some("session=abc".to_string()));
/// ```
#[derive(Debug, Clone)]
pub struct CookieJar {
    cookies: Vec<(Cookie, String)>, // (cookie, origin_domain)
}

impl CookieJar {
    /// Creates an empty cookie jar.
    pub fn new() -> Self {
        Self {
            cookies: Vec::new(),
        }
    }

    /// Parses a `Set-Cookie` header value and stores the cookie.
    ///
    /// `origin_domain` is the domain of the server that set the cookie,
    /// used for domain validation.
    pub fn add_from_header(&mut self, header_value: &str, origin_domain: &str) {
        let origin_url: Url = format!("http://{origin_domain}/")
            .parse()
            .expect("origin domain should be a valid URL host");
        self.add_from_header_for_url(header_value, &origin_url);
    }

    /// Parses a `Set-Cookie` header value and stores the cookie for `origin_url`.
    pub fn add_from_header_for_url(&mut self, header_value: &str, origin_url: &Url) {
        if let Some(mut cookie) = Cookie::parse(header_value) {
            let origin_domain = origin_url.host().to_ascii_lowercase();

            if cookie.domain.is_none() {
                cookie.domain = Some(origin_domain.clone());
                cookie.host_only = true;
            } else {
                cookie.host_only = false;
            }

            if let Some(domain) = &cookie.domain {
                if !domain_matches(&origin_domain, domain) {
                    return; // Reject: server can't set cookie for unrelated domain
                }
            }

            if cookie.path.is_none() {
                cookie.path = Some(default_path(origin_url.path()));
            }

            // Remove existing cookie with same name+domain+path
            let name = cookie.name.clone();
            let domain = cookie.domain.clone();
            let path = cookie.path.clone();
            self.cookies.retain(|(c, _)| {
                !(c.name == name && c.domain == domain && c.path == path)
            });

            self.cookies.push((cookie, origin_domain));
        }
    }

    /// Builds the `Cookie` header value for a request to `url`.
    ///
    /// Returns `None` if no cookies match.
    pub fn cookie_header(&self, url: &Url) -> Option<String> {
        let now = SystemTime::now();
        let pairs: Vec<String> = self
            .cookies
            .iter()
            .filter(|(c, _)| !c.is_expired(now) && c.matches_url(url))
            .map(|(c, _)| format!("{}={}", c.name, c.value))
            .collect();

        if pairs.is_empty() {
            None
        } else {
            Some(pairs.join("; "))
        }
    }

    /// Returns the number of stored cookies (including expired ones).
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    /// Returns `true` if the jar is empty.
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }
}

impl Default for CookieJar {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `true` if `host` domain-matches `domain` per RFC 6265 §5.1.3.
fn domain_matches(host: &str, domain: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let domain = domain.to_ascii_lowercase();

    if host == domain {
        return true;
    }

    // host ends with ".domain"
    host.ends_with(&format!(".{}", domain))
}

/// Returns `true` if `request_path` path-matches `cookie_path` per RFC 6265 §5.1.4.
fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }

    if request_path.starts_with(cookie_path) {
        // cookie_path ends with '/' or the next char in request_path is '/'
        if cookie_path.ends_with('/') {
            return true;
        }
        if request_path.as_bytes().get(cookie_path.len()) == Some(&b'/') {
            return true;
        }
    }

    false
}

fn default_path(request_path: &str) -> String {
    if !request_path.starts_with('/') {
        return "/".to_string();
    }

    match request_path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => request_path[..index].to_string(),
    }
}

/// Parses a subset of HTTP-date formats (RFC 7231 §7.1.1.1).
///
/// Supports the preferred format: `Thu, 01 Dec 2025 00:00:00 GMT`.
fn parse_http_date(s: &str) -> Option<SystemTime> {
    // Minimal parser for "Day, DD Mon YYYY HH:MM:SS GMT"
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }

    // Find date parts - skip day name
    let (day, mon, year, time) = if parts[0].ends_with(',') {
        // "Thu, 01 Dec 2025 00:00:00 GMT"
        if parts.len() < 5 {
            return None;
        }
        (parts[1], parts[2], parts[3], parts[4])
    } else {
        return None;
    };

    let day: u64 = day.parse().ok()?;
    let month = match mon.to_ascii_lowercase().as_str() {
        "jan" => 1u64,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let year: u64 = year.parse().ok()?;

    let time_parts: Vec<&str> = time.split(':').collect();
    if time_parts.len() != 3 {
        return None;
    }
    let hour: u64 = time_parts[0].parse().ok()?;
    let min: u64 = time_parts[1].parse().ok()?;
    let sec: u64 = time_parts[2].parse().ok()?;

    // Convert to seconds since UNIX_EPOCH (simplified, not accounting for leap seconds)
    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }
    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += month_days[m as usize];
        if m == 2 && is_leap_year(year) {
            days += 1;
        }
    }
    days += day - 1;

    let secs = days * 86400 + hour * 3600 + min * 60 + sec;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

fn is_leap_year(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Cookie::parse tests ---

    #[test]
    fn parse_simple_cookie() {
        let c = Cookie::parse("name=value").unwrap();
        assert_eq!(c.name(), "name");
        assert_eq!(c.value(), "value");
        assert_eq!(c.domain(), None);
        assert!(c.host_only());
        assert_eq!(c.path(), None);
        assert!(!c.secure());
        assert!(!c.http_only());
    }

    #[test]
    fn parse_cookie_with_all_attributes() {
        let c = Cookie::parse(
            "id=42; Domain=example.com; Path=/api; Secure; HttpOnly; SameSite=Strict; Max-Age=3600",
        )
        .unwrap();
        assert_eq!(c.name(), "id");
        assert_eq!(c.value(), "42");
        assert_eq!(c.domain(), Some("example.com"));
        assert!(!c.host_only());
        assert_eq!(c.path(), Some("/api"));
        assert!(c.secure());
        assert!(c.http_only());
        assert_eq!(c.same_site(), SameSite::Strict);
        assert_eq!(c.max_age(), Some(3600));
    }

    #[test]
    fn parse_cookie_domain_leading_dot_stripped() {
        let c = Cookie::parse("a=b; Domain=.example.com").unwrap();
        assert_eq!(c.domain(), Some("example.com"));
    }

    #[test]
    fn parse_cookie_expires() {
        let c =
            Cookie::parse("a=b; Expires=Thu, 01 Jan 2099 00:00:00 GMT").unwrap();
        assert!(c.expires().is_some());
        assert!(!c.is_expired(SystemTime::now()));
    }

    #[test]
    fn parse_cookie_expired_in_past() {
        let c =
            Cookie::parse("a=b; Expires=Thu, 01 Jan 1970 00:00:01 GMT").unwrap();
        assert!(c.is_expired(SystemTime::now()));
    }

    #[test]
    fn parse_cookie_max_age_zero_is_expired() {
        let c = Cookie::parse("a=b; Max-Age=0").unwrap();
        assert!(c.is_expired(SystemTime::now()));
    }

    #[test]
    fn max_age_takes_precedence_over_expires() {
        let c = Cookie::parse(
            "a=b; Max-Age=3600; Expires=Thu, 01 Jan 1970 00:00:01 GMT",
        )
        .unwrap();
        assert!(!c.is_expired(SystemTime::now()));
    }

    #[test]
    fn parse_cookie_samesite_none() {
        let c = Cookie::parse("a=b; SameSite=None").unwrap();
        assert_eq!(c.same_site(), SameSite::None);
    }

    #[test]
    fn parse_cookie_samesite_default_lax() {
        let c = Cookie::parse("a=b").unwrap();
        assert_eq!(c.same_site(), SameSite::Lax);
    }

    #[test]
    fn parse_empty_name_returns_none() {
        assert!(Cookie::parse("=value").is_none());
    }

    #[test]
    fn parse_no_equals_returns_none() {
        assert!(Cookie::parse("justanamenovalue").is_none());
    }

    // --- domain_matches tests ---

    #[test]
    fn domain_matches_exact() {
        assert!(domain_matches("example.com", "example.com"));
    }

    #[test]
    fn domain_matches_subdomain() {
        assert!(domain_matches("sub.example.com", "example.com"));
    }

    #[test]
    fn domain_does_not_match_different() {
        assert!(!domain_matches("other.com", "example.com"));
    }

    #[test]
    fn domain_does_not_match_suffix() {
        // "notexample.com" should NOT match "example.com"
        assert!(!domain_matches("notexample.com", "example.com"));
    }

    #[test]
    fn default_path_uses_parent_directory() {
        assert_eq!(default_path("/docs/page.html"), "/docs");
        assert_eq!(default_path("/docs/"), "/docs");
        assert_eq!(default_path("/"), "/");
    }

    // --- path_matches tests ---

    #[test]
    fn path_matches_exact() {
        assert!(path_matches("/api", "/api"));
    }

    #[test]
    fn path_matches_subpath() {
        assert!(path_matches("/api/users", "/api"));
    }

    #[test]
    fn path_matches_trailing_slash() {
        assert!(path_matches("/api/users", "/api/"));
    }

    #[test]
    fn path_does_not_match_partial() {
        assert!(!path_matches("/api2", "/api"));
    }

    #[test]
    fn path_matches_root() {
        assert!(path_matches("/anything", "/"));
    }

    // --- CookieJar tests ---

    #[test]
    fn jar_add_and_retrieve() {
        let mut jar = CookieJar::new();
        jar.add_from_header("session=abc; Path=/; Domain=example.com", "example.com");

        let url: Url = "http://example.com/page".parse().unwrap();
        assert_eq!(jar.cookie_header(&url), Some("session=abc".to_string()));
    }

    #[test]
    fn jar_multiple_cookies() {
        let mut jar = CookieJar::new();
        jar.add_from_header("a=1; Path=/; Domain=example.com", "example.com");
        jar.add_from_header("b=2; Path=/; Domain=example.com", "example.com");

        let url: Url = "http://example.com/".parse().unwrap();
        let header = jar.cookie_header(&url).unwrap();
        assert!(header.contains("a=1"));
        assert!(header.contains("b=2"));
    }

    #[test]
    fn jar_replaces_same_cookie() {
        let mut jar = CookieJar::new();
        jar.add_from_header("a=1; Path=/; Domain=example.com", "example.com");
        jar.add_from_header("a=2; Path=/; Domain=example.com", "example.com");

        assert_eq!(jar.len(), 1);
        let url: Url = "http://example.com/".parse().unwrap();
        assert_eq!(jar.cookie_header(&url), Some("a=2".to_string()));
    }

    #[test]
    fn jar_domain_mismatch_rejected() {
        let mut jar = CookieJar::new();
        // Server at a.com tries to set cookie for b.com — should be rejected
        jar.add_from_header("evil=1; Domain=b.com", "a.com");
        assert!(jar.is_empty());
    }

    #[test]
    fn jar_subdomain_receives_parent_cookie() {
        let mut jar = CookieJar::new();
        jar.add_from_header("a=1; Domain=example.com; Path=/", "example.com");

        let url: Url = "http://sub.example.com/".parse().unwrap();
        assert_eq!(jar.cookie_header(&url), Some("a=1".to_string()));
    }

    #[test]
    fn jar_secure_cookie_not_sent_over_http() {
        let mut jar = CookieJar::new();
        jar.add_from_header("s=secret; Secure; Domain=example.com; Path=/", "example.com");

        let http_url: Url = "http://example.com/".parse().unwrap();
        assert_eq!(jar.cookie_header(&http_url), None);

        let https_url: Url = "https://example.com/".parse().unwrap();
        assert_eq!(
            jar.cookie_header(&https_url),
            Some("s=secret".to_string())
        );
    }

    #[test]
    fn jar_path_scoping() {
        let mut jar = CookieJar::new();
        jar.add_from_header("a=1; Path=/api; Domain=example.com", "example.com");

        let match_url: Url = "http://example.com/api/users".parse().unwrap();
        assert!(jar.cookie_header(&match_url).is_some());

        let no_match_url: Url = "http://example.com/other".parse().unwrap();
        assert!(jar.cookie_header(&no_match_url).is_none());
    }

    #[test]
    fn jar_expired_cookies_not_sent() {
        let mut jar = CookieJar::new();
        jar.add_from_header(
            "old=1; Expires=Thu, 01 Jan 1970 00:00:01 GMT; Domain=example.com; Path=/",
            "example.com",
        );

        let url: Url = "http://example.com/".parse().unwrap();
        assert_eq!(jar.cookie_header(&url), None);
    }

    #[test]
    fn jar_no_domain_defaults_to_origin() {
        let mut jar = CookieJar::new();
        let origin: Url = "http://example.com/account/login".parse().unwrap();
        jar.add_from_header_for_url("a=1", &origin);

        let url: Url = "http://example.com/account/profile".parse().unwrap();
        assert!(jar.cookie_header(&url).is_some());

        let subdomain: Url = "http://www.example.com/".parse().unwrap();
        assert_eq!(jar.cookie_header(&subdomain), None);
    }

    #[test]
    fn jar_default_path_uses_origin_directory() {
        let mut jar = CookieJar::new();
        let origin: Url = "http://example.com/docs/page.html".parse().unwrap();
        jar.add_from_header_for_url("theme=dark", &origin);

        let nested: Url = "http://example.com/docs/chapter-1".parse().unwrap();
        assert_eq!(jar.cookie_header(&nested), Some("theme=dark".to_string()));

        let outside: Url = "http://example.com/home".parse().unwrap();
        assert_eq!(jar.cookie_header(&outside), None);
    }

    // --- parse_http_date tests ---

    #[test]
    fn parse_http_date_valid() {
        let t = parse_http_date("Thu, 01 Jan 2099 00:00:00 GMT");
        assert!(t.is_some());
        assert!(t.unwrap() > SystemTime::now());
    }

    #[test]
    fn parse_http_date_epoch() {
        let t = parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT").unwrap();
        assert_eq!(t, SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn parse_http_date_invalid() {
        assert!(parse_http_date("not a date").is_none());
    }
}
