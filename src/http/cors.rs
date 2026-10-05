//! Fetch origin, CORS, credentials, preflight, and redirect policy.

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

use super::client::{is_redirect, redirect_method, resolve_redirect_url};
use super::request::copy_header_on_redirect;
use super::{Client, HttpRequest, HttpResponse, Method, Url};

const MAX_REDIRECTS: usize = 10;

/// A tuple origin used by Fetch's same-origin and CORS checks.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    tuple: Option<(String, String, u16)>,
}

impl Origin {
    pub fn from_url(url: &Url) -> Self {
        Self {
            tuple: Some((
                url.scheme().to_ascii_lowercase(),
                url.host().to_ascii_lowercase(),
                url.port(),
            )),
        }
    }

    pub fn opaque() -> Self {
        Self { tuple: None }
    }

    pub fn serialize(&self) -> String {
        let Some((scheme, host, port)) = &self.tuple else {
            return "null".to_string();
        };
        let default_port = (scheme == "http" && *port == 80) || (scheme == "https" && *port == 443);
        if default_port {
            format!("{scheme}://{host}")
        } else {
            format!("{scheme}://{host}:{port}")
        }
    }

    pub fn is_same_origin(&self, url: &Url) -> bool {
        self.tuple.is_some() && *self == Self::from_url(url)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestMode {
    SameOrigin,
    Cors,
    NoCors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialsMode {
    Omit,
    SameOrigin,
    Include,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectMode {
    Follow,
    Error,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseType {
    Basic,
    Cors,
    Opaque,
    OpaqueRedirect,
}

/// Successful CORS preflight results, keyed by the request properties that
/// the preflight approved and valid until the response's `max-age` expires.
#[derive(Debug, Default)]
pub struct PreflightCache {
    entries: HashMap<PreflightCacheKey, Instant>,
}

/// Identifies one approved preflight.  A cached entry is reused only for the
/// same origins, method, unsafe header set, and credentials mode.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PreflightCacheKey {
    /// Origin of the document that issued the request (the `Origin` header).
    request_origin: Origin,
    /// Origin of the URL the preflight was sent to.
    target_origin: Origin,
    /// Request method as sent in `Access-Control-Request-Method`.
    method: String,
    /// Sorted, lowercase, deduplicated CORS-unsafe request header names.
    unsafe_header_names: Vec<String>,
    /// Whether the request was made with credentials mode `include`.
    credentialed: bool,
}

impl PreflightCache {
    /// Drops entries that expired at `now` and reports whether `key` remains.
    fn contains_fresh(&mut self, key: &PreflightCacheKey, now: Instant) -> bool {
        self.entries.retain(|_, expires| *expires > now);
        self.entries.contains_key(key)
    }

    fn insert(&mut self, key: PreflightCacheKey, expires: Instant) {
        self.entries.insert(key, expires);
    }
}

#[derive(Debug)]
pub struct FetchResponse {
    pub response: HttpResponse,
    pub response_type: ResponseType,
    pub redirected: bool,
}

#[derive(Debug)]
pub enum CorsError {
    Network(String),
    Timeout,
    SameOriginMode,
    CorsCheck,
    Preflight,
    Redirect,
    TooManyRedirects,
}

impl fmt::Display for CorsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(error) => formatter.write_str(error),
            Self::Timeout => formatter.write_str("network request timed out"),
            Self::SameOriginMode => {
                formatter.write_str("cross-origin request blocked by same-origin mode")
            }
            Self::CorsCheck => formatter.write_str("CORS response check failed"),
            Self::Preflight => formatter.write_str("CORS preflight failed"),
            Self::Redirect => formatter.write_str("redirect disallowed by request redirect mode"),
            Self::TooManyRedirects => formatter.write_str("too many redirects"),
        }
    }
}

impl std::error::Error for CorsError {}

pub fn fetch(
    client: &mut Client,
    request: HttpRequest,
    origin: &Origin,
    mode: RequestMode,
    credentials: CredentialsMode,
    redirect_mode: RedirectMode,
    cache: &mut PreflightCache,
) -> Result<FetchResponse, CorsError> {
    fetch_with_timeout(
        client,
        request,
        origin,
        mode,
        credentials,
        redirect_mode,
        cache,
        None,
    )
}

/// Fetches a resource with an optional transport timeout.  The timeout is
/// applied to each network hop (including CORS preflight); callers that need
/// the ordinary Fetch behavior should use [`fetch`] instead.
pub fn fetch_with_timeout(
    client: &mut Client,
    mut request: HttpRequest,
    origin: &Origin,
    mode: RequestMode,
    credentials: CredentialsMode,
    redirect_mode: RedirectMode,
    cache: &mut PreflightCache,
    timeout: Option<Duration>,
) -> Result<FetchResponse, CorsError> {
    let mut redirected = false;
    let mut cross_origin_seen = false;
    for redirect_count in 0..=MAX_REDIRECTS {
        let cross_origin = !origin.is_same_origin(request.url());
        cross_origin_seen |= cross_origin;
        if cross_origin && mode == RequestMode::SameOrigin {
            return Err(CorsError::SameOriginMode);
        }
        if cross_origin && mode == RequestMode::NoCors {
            for name in cors_unsafe_header_names(&request) {
                request.remove_header(&name);
            }
        }
        if mode == RequestMode::Cors
            && (cross_origin || !matches!(request.method(), Method::Get | Method::Head))
        {
            request.set_header("Origin", origin.serialize());
        }
        if cross_origin && mode == RequestMode::Cors {
            ensure_preflight(client, &request, origin, credentials, cache, timeout)?;
        }

        let send_credentials = match credentials {
            CredentialsMode::Omit => false,
            CredentialsMode::SameOrigin => !cross_origin,
            CredentialsMode::Include => true,
        };
        // Credentials mode controls automatic credentials such as cookies.
        // An Authorization header explicitly supplied by the caller remains
        // part of the request, including a credentialless CORS request.
        // Cross-origin redirects remove it below as required by Fetch.
        let mut response = client
            .send_once_with_timeout(request.clone(), send_credentials, timeout)
            .map_err(|error| {
                if error.is_timeout() {
                    CorsError::Timeout
                } else {
                    CorsError::Network(error.to_string())
                }
            })?;

        if is_redirect(response.status_code()) {
            if cross_origin && mode == RequestMode::Cors {
                cors_check(&response, origin, credentials)?;
            }
            match redirect_mode {
                RedirectMode::Error => return Err(CorsError::Redirect),
                RedirectMode::Manual => {
                    response.set_effective_url(request.url().clone());
                    response.set_redirect_count(redirect_count);
                    return Ok(FetchResponse {
                        response,
                        response_type: ResponseType::OpaqueRedirect,
                        redirected: false,
                    });
                }
                RedirectMode::Follow => {}
            }
            if redirect_count == MAX_REDIRECTS {
                return Err(CorsError::TooManyRedirects);
            }
            let location = response.header("location").ok_or(CorsError::Redirect)?;
            let next_url = resolve_redirect_url(request.url(), location)
                .map_err(|error| CorsError::Network(error.to_string()))?;
            let previous_origin = Origin::from_url(request.url());
            let next_origin = Origin::from_url(&next_url);
            let method = redirect_method(response.status_code(), request.method());
            let preserve_body = method == request.method();
            let mut next = HttpRequest::new(method, next_url);
            if request.requires_public_ip() {
                next.require_public_ip();
            }
            if let Some(site) = request.site_for_cookies() {
                next.set_cookie_context(site.clone(), request.is_top_level_navigation());
            }
            for (name, value) in request.headers() {
                if copy_header_on_redirect(name, preserve_body, previous_origin == next_origin) {
                    next.add_header(name.clone(), value.clone());
                }
            }
            if preserve_body && let Some(body) = request.body() {
                next.set_body(body.to_vec());
            }
            request = next;
            redirected = true;
            continue;
        }

        if cross_origin && mode == RequestMode::Cors {
            cors_check(&response, origin, credentials)?;
        }
        let response_type = if cross_origin_seen && mode == RequestMode::NoCors {
            ResponseType::Opaque
        } else if cross_origin_seen && mode == RequestMode::Cors {
            ResponseType::Cors
        } else {
            ResponseType::Basic
        };
        response.set_effective_url(request.url().clone());
        response.set_redirect_count(redirect_count);
        return Ok(FetchResponse {
            response,
            response_type,
            redirected,
        });
    }
    Err(CorsError::TooManyRedirects)
}

fn ensure_preflight(
    client: &mut Client,
    request: &HttpRequest,
    origin: &Origin,
    credentials: CredentialsMode,
    cache: &mut PreflightCache,
    timeout: Option<Duration>,
) -> Result<(), CorsError> {
    let unsafe_headers = cors_unsafe_header_names(request);
    if is_cors_safelisted_method(request.method()) && unsafe_headers.is_empty() {
        return Ok(());
    }
    let method = request.method().as_str();
    let key = PreflightCacheKey {
        request_origin: origin.clone(),
        target_origin: Origin::from_url(request.url()),
        method: method.to_string(),
        unsafe_header_names: unsafe_headers,
        credentialed: credentials == CredentialsMode::Include,
    };
    let now = Instant::now();
    if cache.contains_fresh(&key, now) {
        return Ok(());
    }

    let mut preflight = HttpRequest::new(Method::Options, request.url().clone());
    preflight.set_header("Origin", origin.serialize());
    preflight.set_header("Access-Control-Request-Method", method);
    if !key.unsafe_header_names.is_empty() {
        preflight.set_header(
            "Access-Control-Request-Headers",
            key.unsafe_header_names.join(", "),
        );
    }
    let response = client
        .send_once_with_timeout(preflight, false, timeout)
        .map_err(|error| {
            if error.is_timeout() {
                CorsError::Timeout
            } else {
                CorsError::Network(error.to_string())
            }
        })?;
    validate_preflight_response(
        &response,
        origin,
        credentials,
        method,
        &key.unsafe_header_names,
    )?;
    cache.insert(key, now + preflight_max_age(&response));
    Ok(())
}

/// Checks a preflight response against the actual request it approves.
///
/// Every failed stage reports [`CorsError::Preflight`]: a non-2xx status, a
/// failed CORS check of the preflight response, a method missing from
/// `Access-Control-Allow-Methods`, or any unsafe header missing from
/// `Access-Control-Allow-Headers`.
fn validate_preflight_response(
    response: &HttpResponse,
    origin: &Origin,
    credentials: CredentialsMode,
    method: &str,
    unsafe_header_names: &[String],
) -> Result<(), CorsError> {
    if !(200..300).contains(&response.status_code()) {
        return Err(CorsError::Preflight);
    }
    cors_check(response, origin, credentials).map_err(|_| CorsError::Preflight)?;
    let allowance = PreflightAllowance::from_response(response);
    let credentialed = credentials == CredentialsMode::Include;
    if !allowance.allows_method(method, credentialed)
        || !allowance.allows_headers(unsafe_header_names, credentialed)
    {
        return Err(CorsError::Preflight);
    }
    Ok(())
}

/// Methods and headers listed by a preflight response.
struct PreflightAllowance {
    methods: Vec<String>,
    headers: Vec<String>,
}

impl PreflightAllowance {
    fn from_response(response: &HttpResponse) -> Self {
        Self {
            methods: header_tokens(response, "access-control-allow-methods"),
            headers: header_tokens(response, "access-control-allow-headers"),
        }
    }

    fn allows_method(&self, method: &str, credentialed: bool) -> bool {
        self.methods
            .iter()
            .any(|allowed| allowed_token_matches(allowed, method, credentialed))
    }

    /// Every unsafe header name must be listed; an empty set is allowed.
    fn allows_headers(&self, unsafe_header_names: &[String], credentialed: bool) -> bool {
        unsafe_header_names.iter().all(|name| {
            self.headers
                .iter()
                .any(|allowed| allowed_token_matches(allowed, name, credentialed))
        })
    }
}

/// Matches an allow-list token case-insensitively.  The `*` wildcard matches
/// any value only for requests without credentials.
fn allowed_token_matches(allowed: &str, value: &str, credentialed: bool) -> bool {
    (allowed == "*" && !credentialed) || allowed.eq_ignore_ascii_case(value)
}

/// Cache lifetime from `Access-Control-Max-Age`: 5 seconds when absent or
/// invalid, capped at 24 hours.
fn preflight_max_age(response: &HttpResponse) -> Duration {
    let seconds = response
        .header("access-control-max-age")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(5)
        .min(86_400);
    Duration::from_secs(seconds)
}

fn cors_check(
    response: &HttpResponse,
    origin: &Origin,
    credentials: CredentialsMode,
) -> Result<(), CorsError> {
    let allow_origin = response
        .header("access-control-allow-origin")
        .ok_or(CorsError::CorsCheck)?;
    if allow_origin != origin.serialize()
        && !(allow_origin == "*" && credentials != CredentialsMode::Include)
    {
        return Err(CorsError::CorsCheck);
    }
    if credentials == CredentialsMode::Include
        && !response
            .header("access-control-allow-credentials")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    {
        return Err(CorsError::CorsCheck);
    }
    Ok(())
}

pub fn exposed_response_headers(
    response: &HttpResponse,
    response_type: ResponseType,
    credentials: CredentialsMode,
) -> Vec<(String, String)> {
    if matches!(
        response_type,
        ResponseType::Opaque | ResponseType::OpaqueRedirect
    ) {
        return Vec::new();
    }
    let exposed = header_tokens(response, "access-control-expose-headers");
    response
        .headers()
        .iter()
        .filter(|(name, _)| {
            !name.eq_ignore_ascii_case("set-cookie")
                && !name.eq_ignore_ascii_case("set-cookie2")
                && (response_type == ResponseType::Basic
                    || is_cors_safelisted_response_header(name)
                    || exposed.iter().any(|allowed| {
                        allowed.eq_ignore_ascii_case(name)
                            || (allowed == "*" && credentials != CredentialsMode::Include)
                    }))
        })
        .cloned()
        .collect()
}

fn is_cors_safelisted_method(method: Method) -> bool {
    matches!(method, Method::Get | Method::Head | Method::Post)
}

fn cors_unsafe_header_names(request: &HttpRequest) -> Vec<String> {
    let mut names = request
        .headers()
        .iter()
        .filter(|(name, value)| {
            !is_automatic_header(name) && !is_cors_safelisted_header(name, value)
        })
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

fn is_automatic_header(name: &str) -> bool {
    [
        "host",
        "user-agent",
        "accept-language",
        "accept-encoding",
        "content-length",
        "origin",
    ]
    .iter()
    .any(|automatic| name.eq_ignore_ascii_case(automatic))
}

fn is_cors_safelisted_header(name: &str, value: &str) -> bool {
    if name.eq_ignore_ascii_case("accept")
        || name.eq_ignore_ascii_case("accept-language")
        || name.eq_ignore_ascii_case("content-language")
    {
        return value.len() <= 128;
    }
    if name.eq_ignore_ascii_case("content-type") {
        let mime = value.split(';').next().unwrap_or("").trim();
        return [
            "application/x-www-form-urlencoded",
            "multipart/form-data",
            "text/plain",
        ]
        .iter()
        .any(|allowed| mime.eq_ignore_ascii_case(allowed));
    }
    false
}

fn is_cors_safelisted_response_header(name: &str) -> bool {
    [
        "cache-control",
        "content-language",
        "content-length",
        "content-type",
        "expires",
        "last-modified",
        "pragma",
    ]
    .iter()
    .any(|allowed| name.eq_ignore_ascii_case(allowed))
}

fn header_tokens(response: &HttpResponse, name: &str) -> Vec<String> {
    response
        .headers()
        .iter()
        .filter(|(header, _)| header.eq_ignore_ascii_case(name))
        .flat_map(|(_, value)| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_include_scheme_host_and_effective_port() {
        let http: Url = "http://example.com/a".parse().unwrap();
        let https: Url = "https://example.com/a".parse().unwrap();
        let other_port: Url = "http://example.com:81/a".parse().unwrap();
        let origin = Origin::from_url(&http);
        assert!(origin.is_same_origin(&http));
        assert!(!origin.is_same_origin(&https));
        assert!(!origin.is_same_origin(&other_port));
        assert_eq!(origin.serialize(), "http://example.com");
        assert_eq!(Origin::opaque().serialize(), "null");
    }

    #[test]
    fn unsafe_headers_and_methods_require_preflight() {
        let url: Url = "http://api.example/data".parse().unwrap();
        let mut simple = HttpRequest::new(Method::Post, url.clone());
        simple.set_header("Content-Type", "text/plain;charset=UTF-8");
        assert!(cors_unsafe_header_names(&simple).is_empty());
        let mut unsafe_request = HttpRequest::new(Method::Put, url);
        unsafe_request.set_header("X-Token", "yes");
        assert_eq!(cors_unsafe_header_names(&unsafe_request), vec!["x-token"]);
        assert!(!is_cors_safelisted_method(unsafe_request.method()));
    }

    #[test]
    fn wildcard_origin_is_rejected_for_credentialed_responses() {
        let response = HttpResponse::new(
            200,
            "OK",
            vec![("Access-Control-Allow-Origin".into(), "*".into())],
            Vec::new(),
        );
        let origin = Origin::from_url(&"https://app.example/".parse().unwrap());
        assert!(cors_check(&response, &origin, CredentialsMode::Omit).is_ok());
        assert!(cors_check(&response, &origin, CredentialsMode::Include).is_err());
    }

    fn app_origin() -> Origin {
        Origin::from_url(&"https://app.example/".parse().unwrap())
    }

    fn preflight_response(status: u16, headers: &[(&str, &str)]) -> HttpResponse {
        HttpResponse::new(
            status,
            "OK",
            headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            Vec::new(),
        )
    }

    fn validate(
        response: &HttpResponse,
        credentials: CredentialsMode,
        method: &str,
        headers: &[&str],
    ) -> Result<(), CorsError> {
        let headers = headers
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        validate_preflight_response(response, &app_origin(), credentials, method, &headers)
    }

    #[test]
    fn preflight_wildcards_apply_only_without_credentials() {
        let response = preflight_response(
            204,
            &[
                ("Access-Control-Allow-Origin", "https://app.example"),
                ("Access-Control-Allow-Credentials", "true"),
                ("Access-Control-Allow-Methods", "*"),
                ("Access-Control-Allow-Headers", "*"),
            ],
        );
        assert!(validate(&response, CredentialsMode::Omit, "PUT", &["x-token"]).is_ok());
        assert!(validate(&response, CredentialsMode::SameOrigin, "PUT", &["x-token"]).is_ok());
        assert!(matches!(
            validate(&response, CredentialsMode::Include, "PUT", &[]),
            Err(CorsError::Preflight)
        ));
        let explicit_method = preflight_response(
            204,
            &[
                ("Access-Control-Allow-Origin", "https://app.example"),
                ("Access-Control-Allow-Credentials", "true"),
                ("Access-Control-Allow-Methods", "put"),
                ("Access-Control-Allow-Headers", "*"),
            ],
        );
        assert!(validate(&explicit_method, CredentialsMode::Include, "PUT", &[]).is_ok());
        assert!(matches!(
            validate(
                &explicit_method,
                CredentialsMode::Include,
                "PUT",
                &["x-token"]
            ),
            Err(CorsError::Preflight)
        ));
    }

    #[test]
    fn preflight_explicit_allow_lists_match_case_insensitively_with_credentials() {
        let response = preflight_response(
            200,
            &[
                ("Access-Control-Allow-Origin", "https://app.example"),
                ("Access-Control-Allow-Credentials", "TRUE"),
                ("Access-Control-Allow-Methods", "GET, Put"),
                ("Access-Control-Allow-Headers", "X-Token"),
            ],
        );
        assert!(validate(&response, CredentialsMode::Include, "PUT", &["x-token"]).is_ok());
        assert!(validate(&response, CredentialsMode::Omit, "PUT", &["x-token"]).is_ok());
    }

    #[test]
    fn preflight_rejects_unlisted_method_or_header() {
        let response = preflight_response(
            204,
            &[
                ("Access-Control-Allow-Origin", "*"),
                ("Access-Control-Allow-Methods", "GET, POST"),
                ("Access-Control-Allow-Headers", "x-token"),
            ],
        );
        assert!(matches!(
            validate(&response, CredentialsMode::Omit, "DELETE", &[]),
            Err(CorsError::Preflight)
        ));
        assert!(matches!(
            validate(&response, CredentialsMode::Omit, "POST", &["x-other"]),
            Err(CorsError::Preflight)
        ));
        let no_methods = preflight_response(204, &[("Access-Control-Allow-Origin", "*")]);
        assert!(matches!(
            validate(&no_methods, CredentialsMode::Omit, "POST", &[]),
            Err(CorsError::Preflight)
        ));
    }

    #[test]
    fn preflight_requires_every_unsafe_header_across_header_lines() {
        let response = preflight_response(
            204,
            &[
                ("Access-Control-Allow-Origin", "*"),
                ("Access-Control-Allow-Methods", "PUT"),
                ("Access-Control-Allow-Headers", "x-a, , x-b"),
                ("access-control-allow-headers", "x-c"),
            ],
        );
        assert!(
            validate(
                &response,
                CredentialsMode::Omit,
                "PUT",
                &["x-a", "x-b", "x-c"]
            )
            .is_ok()
        );
        assert!(matches!(
            validate(&response, CredentialsMode::Omit, "PUT", &["x-a", "x-d"]),
            Err(CorsError::Preflight)
        ));
    }

    #[test]
    fn preflight_rejects_failed_status_or_cors_check_as_preflight_error() {
        let allow = [
            ("Access-Control-Allow-Origin", "*"),
            ("Access-Control-Allow-Methods", "PUT"),
        ];
        assert!(matches!(
            validate(
                &preflight_response(403, &allow),
                CredentialsMode::Omit,
                "PUT",
                &[]
            ),
            Err(CorsError::Preflight)
        ));
        let other_origin = preflight_response(
            204,
            &[
                ("Access-Control-Allow-Origin", "https://other.example"),
                ("Access-Control-Allow-Methods", "PUT"),
            ],
        );
        assert!(matches!(
            validate(&other_origin, CredentialsMode::Omit, "PUT", &[]),
            Err(CorsError::Preflight)
        ));
        let missing_credentials = preflight_response(
            204,
            &[
                ("Access-Control-Allow-Origin", "https://app.example"),
                ("Access-Control-Allow-Methods", "PUT"),
            ],
        );
        assert!(validate(&missing_credentials, CredentialsMode::Omit, "PUT", &[]).is_ok());
        assert!(matches!(
            validate(&missing_credentials, CredentialsMode::Include, "PUT", &[]),
            Err(CorsError::Preflight)
        ));
    }

    #[test]
    fn preflight_max_age_defaults_and_is_capped() {
        let max_age = |value: Option<&str>| {
            let headers = value
                .map(|value| vec![("Access-Control-Max-Age", value)])
                .unwrap_or_default();
            preflight_max_age(&preflight_response(204, &headers))
        };
        assert_eq!(max_age(None), Duration::from_secs(5));
        assert_eq!(max_age(Some("invalid")), Duration::from_secs(5));
        assert_eq!(max_age(Some("600")), Duration::from_secs(600));
        assert_eq!(max_age(Some("999999")), Duration::from_secs(86_400));
    }

    #[test]
    fn preflight_cache_distinguishes_credentials_and_expires_entries() {
        let target = Origin::from_url(&"https://api.example/".parse().unwrap());
        let key = PreflightCacheKey {
            request_origin: app_origin(),
            target_origin: target,
            method: "PUT".into(),
            unsafe_header_names: vec!["x-token".into()],
            credentialed: false,
        };
        let credentialed_key = PreflightCacheKey {
            credentialed: true,
            ..key.clone()
        };
        let now = Instant::now();
        let mut cache = PreflightCache::default();
        cache.insert(key.clone(), now + Duration::from_secs(5));
        assert!(cache.contains_fresh(&key, now));
        assert!(!cache.contains_fresh(&credentialed_key, now));
        assert!(!cache.contains_fresh(&key, now + Duration::from_secs(5)));
        assert!(cache.entries.is_empty());
    }
}
