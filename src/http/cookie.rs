//! Cookie parsing and storage (RFC 6265).
//!
//! Provides [`Cookie`] for individual cookies parsed from `Set-Cookie` headers,
//! and [`CookieJar`] for storing and retrieving cookies across requests.

use std::net::IpAddr;
use std::time::{Duration, SystemTime};

use super::request::Method;
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
    received_at: SystemTime,
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
            received_at: SystemTime::now(),
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

            if let Ok(elapsed) = now.duration_since(self.received_at) {
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
        if let Some(cookie_path) = &self.path
            && !path_matches(url.path(), cookie_path)
        {
            return false;
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
        self.add_with_context(header_value, origin_url, origin_url, true, true);
    }

    /// Stores a response cookie with the request's site and navigation context.
    pub fn add_from_header_for_request(
        &mut self,
        header_value: &str,
        origin_url: &Url,
        site_for_cookies: &Url,
        top_level_navigation: bool,
    ) {
        self.add_with_context(
            header_value,
            origin_url,
            site_for_cookies,
            top_level_navigation,
            true,
        );
    }

    /// Stores a cookie set by `document.cookie`, without HTTP-only privileges.
    pub fn add_from_document(&mut self, value: &str, document_url: &Url, site_for_cookies: &Url) {
        self.add_with_context(value, document_url, site_for_cookies, false, false);
    }

    fn add_with_context(
        &mut self,
        header_value: &str,
        origin_url: &Url,
        site_for_cookies: &Url,
        top_level_navigation: bool,
        from_http: bool,
    ) {
        if let Some(mut cookie) = Cookie::parse(header_value) {
            let origin_domain = origin_url.host().to_ascii_lowercase();

            if let Some(domain) = &cookie.domain {
                if domain.is_empty() || !domain_matches(&origin_domain, domain) {
                    return;
                }
                if is_public_suffix(domain) {
                    if domain != &origin_domain {
                        return;
                    }
                    cookie.host_only = true;
                } else {
                    cookie.host_only = false;
                }
            } else {
                cookie.host_only = true;
            }
            if cookie.host_only {
                cookie.domain = Some(origin_domain.clone());
            }

            if cookie
                .path
                .as_deref()
                .is_none_or(|path| !path.starts_with('/'))
            {
                cookie.path = Some(default_path(origin_url.path()));
            }

            let secure_origin = origin_url.scheme() == "https";
            if cookie.secure && !secure_origin {
                return;
            }
            if !from_http && cookie.http_only {
                return;
            }
            if cookie.same_site == SameSite::None && !cookie.secure {
                return;
            }
            if !same_site(origin_url, site_for_cookies)
                && cookie.same_site != SameSite::None
                && (!from_http || !top_level_navigation)
            {
                return;
            }
            let lower_name = cookie.name.to_ascii_lowercase();
            if lower_name.starts_with("__secure-") && !cookie.secure {
                return;
            }
            if lower_name.starts_with("__host-")
                && (!cookie.secure
                    || !cookie.host_only
                    || cookie.path.as_deref() != Some("/")
                    || !header_value.split(';').skip(1).any(|attribute| {
                        attribute
                            .trim()
                            .split_once('=')
                            .is_some_and(|(name, value)| {
                                name.trim().eq_ignore_ascii_case("path") && value.trim() == "/"
                            })
                    }))
            {
                return;
            }

            if !secure_origin
                && !cookie.secure
                && self.cookies.iter().any(|(old, _)| {
                    old.secure
                        && old.name == cookie.name
                        && old
                            .domain
                            .as_deref()
                            .zip(cookie.domain.as_deref())
                            .is_some_and(|(a, b)| domain_matches(a, b) || domain_matches(b, a))
                        && old
                            .path
                            .as_deref()
                            .zip(cookie.path.as_deref())
                            .is_some_and(|(old, new)| path_matches(new, old))
                })
            {
                return;
            }

            // Remove existing cookie with same name+domain+path
            let name = cookie.name.clone();
            let domain = cookie.domain.clone();
            let path = cookie.path.clone();
            if let Some((old, _)) = self
                .cookies
                .iter()
                .find(|(old, _)| old.name == name && old.domain == domain && old.path == path)
            {
                if !from_http && old.http_only {
                    return;
                }
                cookie.created_at = old.created_at;
            }
            self.cookies
                .retain(|(c, _)| !(c.name == name && c.domain == domain && c.path == path));

            if !cookie.is_expired(SystemTime::now()) {
                self.cookies.push((cookie, origin_domain));
            }
        }
    }

    /// Builds the `Cookie` header value for a request to `url`.
    ///
    /// Returns `None` if no cookies match.
    pub fn cookie_header(&self, url: &Url) -> Option<String> {
        self.cookie_header_for_request(url, url, true, Method::Get)
    }

    /// Selects cookies using the initiating site's URL and navigation context.
    /// Cross-site `Strict` cookies are excluded; `Lax` cookies are sent only
    /// with safe top-level navigations.
    pub fn cookie_header_for_request(
        &self,
        url: &Url,
        site_for_cookies: &Url,
        top_level_navigation: bool,
        method: Method,
    ) -> Option<String> {
        self.cookie_string(url, site_for_cookies, top_level_navigation, method, false)
    }

    /// Returns the cookies visible to script for this document.
    pub fn document_cookie(&self, url: &Url, site_for_cookies: &Url) -> String {
        self.cookie_string(url, site_for_cookies, false, Method::Get, true)
            .unwrap_or_default()
    }

    fn cookie_string(
        &self,
        url: &Url,
        site_for_cookies: &Url,
        top_level_navigation: bool,
        method: Method,
        non_http: bool,
    ) -> Option<String> {
        let now = SystemTime::now();
        let same_site = same_site(url, site_for_cookies);
        let safe_navigation =
            top_level_navigation && matches!(method, Method::Get | Method::Head | Method::Options);
        let mut cookies: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|(c, _)| {
                !c.is_expired(now)
                    && c.matches_url(url)
                    && (!non_http || !c.http_only)
                    && (same_site
                        || c.same_site == SameSite::None
                        || (!non_http && c.same_site == SameSite::Lax && safe_navigation))
            })
            .map(|(c, _)| c)
            .collect();
        cookies.sort_by(|a, b| {
            b.path
                .as_deref()
                .unwrap_or("/")
                .len()
                .cmp(&a.path.as_deref().unwrap_or("/").len())
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        let pairs: Vec<String> = cookies
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
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

    if host.trim_matches(['[', ']']).parse::<IpAddr>().is_ok() {
        return false;
    }

    // host ends with ".domain"
    host.ends_with(&format!(".{}", domain))
}

fn is_public_suffix(domain: &str) -> bool {
    domain.parse::<IpAddr>().is_err() && psl::suffix_str(domain) == Some(domain)
}

fn same_site(a: &Url, b: &Url) -> bool {
    if a.scheme() != b.scheme() {
        return false;
    }
    let a_host = a.host().trim_matches(['[', ']']);
    let b_host = b.host().trim_matches(['[', ']']);
    let a_site = psl::domain_str(a_host).unwrap_or(a_host);
    let b_site = psl::domain_str(b_host).unwrap_or(b_host);
    a_site.eq_ignore_ascii_case(b_site)
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

/// Parses a cookie expiry date using the token order from RFC 6265 §5.1.1.
fn parse_http_date(s: &str) -> Option<SystemTime> {
    let mut day = None;
    let mut month = None;
    let mut year = None;
    let mut time = None;
    for token in s.split(|c: char| !c.is_ascii_alphanumeric() && c != ':') {
        if token.is_empty() {
            continue;
        }
        if time.is_none() {
            let parts: Vec<_> = token.split(':').collect();
            if parts.len() == 3
                && parts.iter().all(|part| !part.is_empty() && part.len() <= 2)
                && let (Ok(h), Ok(m), Ok(sec)) = (
                    parts[0].parse::<u8>(),
                    parts[1].parse::<u8>(),
                    parts[2].parse::<u8>(),
                )
            {
                time = Some((h, m, sec));
                continue;
            }
        }
        if day.is_none()
            && (1..=2).contains(&token.len())
            && token.bytes().all(|b| b.is_ascii_digit())
        {
            day = token.parse::<u8>().ok();
            continue;
        }
        if month.is_none() && token.len() >= 3 {
            month = match token[..3].to_ascii_lowercase().as_str() {
                "jan" => Some(1u8),
                "feb" => Some(2),
                "mar" => Some(3),
                "apr" => Some(4),
                "may" => Some(5),
                "jun" => Some(6),
                "jul" => Some(7),
                "aug" => Some(8),
                "sep" => Some(9),
                "oct" => Some(10),
                "nov" => Some(11),
                "dec" => Some(12),
                _ => None,
            };
            if month.is_some() {
                continue;
            }
        }
        if year.is_none()
            && (2..=4).contains(&token.len())
            && token.bytes().all(|b| b.is_ascii_digit())
        {
            year = token.parse::<i64>().ok();
        }
    }
    let day = day?;
    let month = month?;
    let mut year = year?;
    let (hour, min, sec) = time?;
    if (0..=69).contains(&year) {
        year += 2000;
    } else if (70..=99).contains(&year) {
        year += 1900;
    }
    if year < 1601 || hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    let month_days = [
        31,
        if is_leap_year(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > month_days[(month - 1) as usize] {
        return None;
    }

    // Civil date to days since 1970-01-01, with constant work for any year.
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let year_of_era_day = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + year_of_era_day - 719_468;
    let seconds = days * 86_400 + i64::from(hour) * 3600 + i64::from(min) * 60 + i64::from(sec);
    if seconds >= 0 {
        SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64))
    } else {
        SystemTime::UNIX_EPOCH.checked_sub(Duration::from_secs(seconds.unsigned_abs()))
    }
}

fn is_leap_year(y: i64) -> bool {
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
        let c = Cookie::parse("a=b; Expires=Thu, 01 Jan 2099 00:00:00 GMT").unwrap();
        assert!(c.expires().is_some());
        assert!(!c.is_expired(SystemTime::now()));
    }

    #[test]
    fn parse_cookie_expired_in_past() {
        let c = Cookie::parse("a=b; Expires=Thu, 01 Jan 1970 00:00:01 GMT").unwrap();
        assert!(c.is_expired(SystemTime::now()));
    }

    #[test]
    fn parse_cookie_max_age_zero_is_expired() {
        let c = Cookie::parse("a=b; Max-Age=0").unwrap();
        assert!(c.is_expired(SystemTime::now()));
    }

    #[test]
    fn max_age_takes_precedence_over_expires() {
        let c = Cookie::parse("a=b; Max-Age=3600; Expires=Thu, 01 Jan 1970 00:00:01 GMT").unwrap();
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
        let origin: Url = "https://example.com/".parse().unwrap();
        jar.add_from_header_for_url("s=secret; Secure; Domain=example.com; Path=/", &origin);

        let http_url: Url = "http://example.com/".parse().unwrap();
        assert_eq!(jar.cookie_header(&http_url), None);

        let https_url: Url = "https://example.com/".parse().unwrap();
        assert_eq!(jar.cookie_header(&https_url), Some("s=secret".to_string()));
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

    #[test]
    fn jar_rejects_public_suffix_and_ip_suffix_domains() {
        let mut jar = CookieJar::new();
        let origin: Url = "https://example.com/".parse().unwrap();
        jar.add_from_header_for_url("bad=1; Domain=com", &origin);
        let jp_origin: Url = "https://shop.example.co.jp/".parse().unwrap();
        jar.add_from_header_for_url("bad=2; Domain=co.jp", &jp_origin);
        let ip_origin: Url = "http://10.0.0.1/".parse().unwrap();
        jar.add_from_header_for_url("bad=3; Domain=0.1", &ip_origin);
        assert!(jar.is_empty());

        jar.add_from_header_for_url("good=1; Domain=example.com", &origin);
        assert_eq!(jar.cookie_header(&origin), Some("good=1".to_string()));
    }

    #[test]
    fn jar_enforces_secure_and_prefix_constraints() {
        let mut jar = CookieJar::new();
        let http: Url = "http://example.com/".parse().unwrap();
        let https: Url = "https://example.com/".parse().unwrap();
        jar.add_from_header_for_url("secure=1; Secure", &http);
        jar.add_from_header_for_url("__Secure-bad=1", &https);
        jar.add_from_header_for_url("__Host-bad=1; Secure; Path=/; Domain=example.com", &https);
        jar.add_from_header_for_url("none=1; SameSite=None", &https);
        assert!(jar.is_empty());

        jar.add_from_header_for_url("__Secure-good=1; Secure", &https);
        jar.add_from_header_for_url("__Host-good=1; Secure; Path=/", &https);
        assert_eq!(
            jar.cookie_header(&https),
            Some("__Secure-good=1; __Host-good=1".to_string())
        );
    }

    #[test]
    fn insecure_response_cannot_overlay_secure_cookie() {
        let mut jar = CookieJar::new();
        let https: Url = "https://example.com/login".parse().unwrap();
        let http: Url = "http://example.com/login".parse().unwrap();
        jar.add_from_header_for_url("session=safe; Secure; Path=/login", &https);
        jar.add_from_header_for_url("session=attacker; Path=/login", &http);
        assert_eq!(jar.cookie_header(&https), Some("session=safe".to_string()));
    }

    #[test]
    fn invalid_path_defaults_and_longer_paths_are_sent_first() {
        let mut jar = CookieJar::new();
        let origin: Url = "https://example.com/docs/page".parse().unwrap();
        jar.add_from_header_for_url("root=1; Path=/", &origin);
        jar.add_from_header_for_url("default=1; Path=relative", &origin);
        let nested: Url = "https://example.com/docs/next".parse().unwrap();
        assert_eq!(
            jar.cookie_header(&nested),
            Some("default=1; root=1".to_string())
        );
        let outside: Url = "https://example.com/other".parse().unwrap();
        assert_eq!(jar.cookie_header(&outside), Some("root=1".to_string()));
    }

    #[test]
    fn samesite_filters_cross_site_subresources_and_post_navigation() {
        let mut jar = CookieJar::new();
        let target: Url = "https://auth.example.com/account".parse().unwrap();
        let other_site: Url = "https://other.test/".parse().unwrap();
        jar.add_from_header_for_url("strict=1; SameSite=Strict; Path=/", &target);
        jar.add_from_header_for_url("lax=1; SameSite=Lax; Path=/", &target);
        jar.add_from_header_for_url("none=1; SameSite=None; Secure; Path=/", &target);

        assert_eq!(
            jar.cookie_header_for_request(&target, &other_site, false, Method::Get),
            Some("none=1".to_string())
        );
        assert_eq!(
            jar.cookie_header_for_request(&target, &other_site, true, Method::Post),
            Some("none=1".to_string())
        );
        assert_eq!(
            jar.cookie_header_for_request(&target, &other_site, true, Method::Get),
            Some("lax=1; none=1".to_string())
        );
        assert_eq!(
            jar.cookie_header_for_request(&target, &target, false, Method::Post),
            Some("strict=1; lax=1; none=1".to_string())
        );
    }

    #[test]
    fn document_cookie_hides_httponly_and_cannot_replace_it() {
        let mut jar = CookieJar::new();
        let document: Url = "https://example.com/account/page".parse().unwrap();
        jar.add_from_header_for_url("session=server; HttpOnly; Path=/", &document);
        jar.add_from_document("session=script; Path=/", &document, &document);
        jar.add_from_document(
            "session=domain-script; Domain=example.com; Path=/",
            &document,
            &document,
        );
        jar.add_from_document("theme=dark; Path=/account", &document, &document);
        assert_eq!(jar.document_cookie(&document, &document), "theme=dark");
        assert_eq!(
            jar.cookie_header(&document),
            Some("theme=dark; session=server".to_string())
        );
    }

    #[test]
    fn replacement_preserves_creation_order_but_renews_max_age() {
        let mut jar = CookieJar::new();
        let url: Url = "https://example.com/".parse().unwrap();
        jar.add_from_header_for_url("session=old; Max-Age=3600", &url);
        let old_creation = SystemTime::now() - Duration::from_secs(7200);
        jar.cookies[0].0.created_at = old_creation;
        jar.cookies[0].0.received_at = old_creation;

        jar.add_from_header_for_url("session=new; Max-Age=3600", &url);
        assert_eq!(jar.cookie_header(&url), Some("session=new".to_string()));
        assert_eq!(jar.cookies[0].0.created_at, old_creation);
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

    #[test]
    fn invalid_cookie_dates_do_not_panic_or_loop() {
        for value in [
            "Thu, 00 Jan 2030 00:00:00 GMT",
            "Thu, 01 Jan 999999999999 00:00:00 GMT",
            "Thu, 01 Jan 2030 18446744073709551615:00:00 GMT",
            "Thu, 29 Feb 2023 00:00:00 GMT",
            "Thu, 01 Jan 2030 24:00:00 GMT",
        ] {
            assert!(parse_http_date(value).is_none(), "{value}");
        }
    }

    #[test]
    fn cookie_date_accepts_common_formats_and_leap_days() {
        let expected = parse_http_date("Wed, 09 Jun 2021 10:18:14 GMT").unwrap();
        assert_eq!(
            parse_http_date("Wed, 09-Jun-2021 10:18:14 GMT"),
            Some(expected)
        );
        assert_eq!(parse_http_date("09 Jun 21 10:18:14 GMT"), Some(expected));
        assert!(parse_http_date("Thu, 29 Feb 2024 00:00:00 GMT").is_some());
    }
}
