//! Fetch origin, CORS, credentials, preflight, and redirect policy.

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

use super::client::{is_redirect, redirect_method, resolve_redirect_url};
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

#[derive(Debug, Default)]
pub struct PreflightCache {
    entries: HashMap<(Origin, Origin, String, Vec<String>, bool), Instant>,
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
    mut request: HttpRequest,
    origin: &Origin,
    mode: RequestMode,
    credentials: CredentialsMode,
    redirect_mode: RedirectMode,
    cache: &mut PreflightCache,
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
            ensure_preflight(client, &request, origin, credentials, cache)?;
        }

        let send_credentials = match credentials {
            CredentialsMode::Omit => false,
            CredentialsMode::SameOrigin => !cross_origin,
            CredentialsMode::Include => true,
        };
        let mut outbound = request.clone();
        if !send_credentials {
            outbound.remove_header("authorization");
        }
        let mut response = client
            .send_once(outbound, send_credentials)
            .map_err(|error| CorsError::Network(error.to_string()))?;

        if is_redirect(response.status_code()) {
            if cross_origin && mode == RequestMode::Cors {
                cors_check(&response, origin, credentials)?;
            }
            match redirect_mode {
                RedirectMode::Error => return Err(CorsError::Redirect),
                RedirectMode::Manual => {
                    response.set_effective_url(request.url().clone());
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
            let preserve_body = matches!(response.status_code(), 307 | 308);
            let mut next = HttpRequest::new(method, next_url);
            if request.requires_public_ip() {
                next.require_public_ip();
            }
            for (name, value) in request.headers() {
                if !is_redirect_internal_header(name)
                    && (preserve_body || !is_request_body_header(name))
                    && !(previous_origin != next_origin
                        && name.eq_ignore_ascii_case("authorization"))
                {
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
) -> Result<(), CorsError> {
    let unsafe_headers = cors_unsafe_header_names(request);
    if is_cors_safelisted_method(request.method()) && unsafe_headers.is_empty() {
        return Ok(());
    }
    let target = Origin::from_url(request.url());
    let credentialed = credentials == CredentialsMode::Include;
    let key = (
        origin.clone(),
        target,
        request.method().as_str().to_string(),
        unsafe_headers.clone(),
        credentialed,
    );
    let now = Instant::now();
    cache.entries.retain(|_, expires| *expires > now);
    if cache.entries.contains_key(&key) {
        return Ok(());
    }

    let mut preflight = HttpRequest::new(Method::Options, request.url().clone());
    preflight.set_header("Origin", origin.serialize());
    preflight.set_header("Access-Control-Request-Method", request.method().as_str());
    if !unsafe_headers.is_empty() {
        preflight.set_header("Access-Control-Request-Headers", unsafe_headers.join(", "));
    }
    let response = client
        .send_once(preflight, false)
        .map_err(|error| CorsError::Network(error.to_string()))?;
    if !(200..300).contains(&response.status_code())
        || cors_check(&response, origin, credentials).is_err()
        || !header_tokens(&response, "access-control-allow-methods")
            .iter()
            .any(|method| {
                (method == "*" && !credentialed)
                    || method.eq_ignore_ascii_case(request.method().as_str())
            })
        || !unsafe_headers.iter().all(|name| {
            header_tokens(&response, "access-control-allow-headers")
                .iter()
                .any(|allowed| {
                    (allowed == "*" && !credentialed) || allowed.eq_ignore_ascii_case(name)
                })
        })
    {
        return Err(CorsError::Preflight);
    }
    let max_age = response
        .header("access-control-max-age")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(5)
        .min(86_400);
    cache
        .entries
        .insert(key, now + Duration::from_secs(max_age));
    Ok(())
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

fn is_redirect_internal_header(name: &str) -> bool {
    ["host", "cookie", "content-length", "origin"]
        .iter()
        .any(|ignored| name.eq_ignore_ascii_case(ignored))
}

fn is_request_body_header(name: &str) -> bool {
    [
        "content-encoding",
        "content-language",
        "content-location",
        "content-type",
    ]
    .iter()
    .any(|header| name.eq_ignore_ascii_case(header))
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
}
