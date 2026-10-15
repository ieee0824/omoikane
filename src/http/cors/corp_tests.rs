use super::*;
use crate::http::{HttpParseError, HttpTransport};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct RedirectTransport {
    calls: Arc<AtomicUsize>,
}

impl HttpTransport for RedirectTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(match request.url().path() {
            "/start" => HttpResponse::new(
                302,
                "Found",
                vec![
                    ("Location".into(), "http://other.example.test/final".into()),
                    ("Cross-Origin-Resource-Policy".into(), "same-origin".into()),
                ],
                Vec::new(),
            ),
            "/final" => HttpResponse::new(200, "OK", Vec::new(), b"final".to_vec()),
            path => panic!("unexpected request: {path}"),
        })
    }
}

#[test]
fn no_cors_rejects_protected_redirect_before_following_it() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut client = Client::new();
    client.set_transport(Some(Arc::new(RedirectTransport {
        calls: Arc::clone(&calls),
    })));
    let source: Url = "http://source.example.test/page".parse().unwrap();
    let target: Url = "http://resource.example.test/start".parse().unwrap();
    let result = fetch(
        &mut client,
        HttpRequest::new(Method::Get, target),
        &Origin::from_url(&source),
        RequestMode::NoCors,
        CredentialsMode::SameOrigin,
        RedirectMode::Follow,
        &mut PreflightCache::default(),
    );
    assert!(matches!(result, Err(CorsError::CorpCheck)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn module_fetch_restricts_redirects_to_public_addresses() {
    struct ModuleRedirectTransport(Arc<Mutex<Vec<bool>>>);

    impl HttpTransport for ModuleRedirectTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            self.0.lock().unwrap().push(request.requires_public_ip());
            Ok(match request.url().path() {
                "/module.js" => HttpResponse::new(
                    302,
                    "Found",
                    vec![(
                        "Location".into(),
                        "http://other.example.test/import.js".into(),
                    )],
                    Vec::new(),
                ),
                "/import.js" => HttpResponse::new(
                    200,
                    "OK",
                    vec![(
                        "Access-Control-Allow-Origin".into(),
                        "http://source.example.test".into(),
                    )],
                    b"export {};".to_vec(),
                ),
                path => panic!("unexpected request: {path}"),
            })
        }
    }

    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut client = Client::new();
    client.set_transport(Some(Arc::new(ModuleRedirectTransport(Arc::clone(&seen)))));
    let origin = Origin::from_url(&"http://source.example.test/page".parse().unwrap());
    let response = fetch_with_embedder_policy(
        &mut client,
        HttpRequest::new(
            Method::Get,
            "http://source.example.test/module.js".parse().unwrap(),
        ),
        &origin,
        RequestMode::Cors,
        CredentialsMode::SameOrigin,
        RedirectMode::Follow,
        &mut PreflightCache::default(),
        super::super::corp::EmbedderPolicyValue::UnsafeNone,
        None,
    )
    .unwrap();
    assert_eq!(response.response.body(), b"export {};");
    assert_eq!(*seen.lock().unwrap(), [false, true]);
}

struct CredentialTransport {
    cookies: Arc<Mutex<Vec<Option<String>>>>,
}

impl HttpTransport for CredentialTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        self.cookies
            .lock()
            .unwrap()
            .push(request.header("cookie").map(str::to_owned));
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![
                (
                    "Access-Control-Allow-Origin".into(),
                    "http://a.example.test".into(),
                ),
                ("Access-Control-Allow-Credentials".into(), "true".into()),
            ],
            Vec::new(),
        ))
    }
}

#[test]
fn credentialless_omits_cookies_only_for_cross_origin_no_cors() {
    let cookies = Arc::new(Mutex::new(Vec::new()));
    let mut client = Client::new();
    client.set_transport(Some(Arc::new(CredentialTransport {
        cookies: Arc::clone(&cookies),
    })));
    client
        .cookie_jar_mut()
        .add_from_header("session=value", "b.example.test");
    let origin = Origin::from_url(&"http://a.example.test/".parse().unwrap());
    for (mode, policy) in [
        (
            RequestMode::NoCors,
            super::super::corp::EmbedderPolicyValue::UnsafeNone,
        ),
        (
            RequestMode::NoCors,
            super::super::corp::EmbedderPolicyValue::Credentialless,
        ),
        (
            RequestMode::Cors,
            super::super::corp::EmbedderPolicyValue::Credentialless,
        ),
    ] {
        let request = HttpRequest::new(
            Method::Get,
            "http://b.example.test/resource".parse().unwrap(),
        );
        let result = fetch_with_embedder_policy(
            &mut client,
            request,
            &origin,
            mode,
            CredentialsMode::Include,
            RedirectMode::Follow,
            &mut PreflightCache::default(),
            policy,
            None,
        );
        assert!(result.is_ok(), "{mode:?} {policy:?}: {result:?}");
    }
    assert_eq!(
        &*cookies.lock().unwrap(),
        &[
            Some("session=value".to_owned()),
            None,
            Some("session=value".to_owned()),
        ]
    );
}

struct RoundTripCredentialTransport {
    cookies: Arc<Mutex<Vec<Option<String>>>>,
    origins: Arc<Mutex<Vec<Option<String>>>>,
    final_allows_cors: bool,
}

impl HttpTransport for RoundTripCredentialTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        self.cookies
            .lock()
            .unwrap()
            .push(request.header("cookie").map(str::to_owned));
        self.origins
            .lock()
            .unwrap()
            .push(request.header("origin").map(str::to_owned));
        let location = match request.url().path() {
            "/start" => Some("http://b.example.test/middle"),
            "/middle" => Some("http://a.example.test/final"),
            "/final" => None,
            path => panic!("unexpected request: {path}"),
        };
        Ok(match location {
            Some(location) => HttpResponse::new(
                302,
                "Found",
                vec![
                    ("Location".into(), location.into()),
                    (
                        "Access-Control-Allow-Origin".into(),
                        "http://a.example.test".into(),
                    ),
                ],
                Vec::new(),
            ),
            None => HttpResponse::new(
                200,
                "OK",
                self.final_allows_cors
                    .then(|| vec![("Access-Control-Allow-Origin".into(), "null".into())])
                    .unwrap_or_default(),
                b"final".to_vec(),
            ),
        })
    }
}

#[test]
fn credentialless_redirect_taint_keeps_cookies_off_return_to_same_origin() {
    let cookies = Arc::new(Mutex::new(Vec::new()));
    let origins = Arc::new(Mutex::new(Vec::new()));
    let mut client = Client::new();
    client.set_transport(Some(Arc::new(RoundTripCredentialTransport {
        cookies: Arc::clone(&cookies),
        origins: Arc::clone(&origins),
        final_allows_cors: true,
    })));
    client
        .cookie_jar_mut()
        .add_from_header("source=value", "a.example.test");
    client
        .cookie_jar_mut()
        .add_from_header("foreign=value", "b.example.test");
    let origin = Origin::from_url(&"http://a.example.test/page".parse().unwrap());
    let result = fetch_with_embedder_policy(
        &mut client,
        HttpRequest::new(Method::Get, "http://a.example.test/start".parse().unwrap()),
        &origin,
        RequestMode::NoCors,
        CredentialsMode::Include,
        RedirectMode::Follow,
        &mut PreflightCache::default(),
        super::super::corp::EmbedderPolicyValue::Credentialless,
        None,
    );
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        &*cookies.lock().unwrap(),
        &[Some("source=value".to_owned()), None, None]
    );
    assert_eq!(&*origins.lock().unwrap(), &[None, None, None]);
}

#[test]
fn same_origin_credentials_stay_off_after_cors_redirect_returns_home() {
    let cookies = Arc::new(Mutex::new(Vec::new()));
    let origins = Arc::new(Mutex::new(Vec::new()));
    let mut client = Client::new();
    client.set_transport(Some(Arc::new(RoundTripCredentialTransport {
        cookies: Arc::clone(&cookies),
        origins: Arc::clone(&origins),
        final_allows_cors: true,
    })));
    client
        .cookie_jar_mut()
        .add_from_header("source=value", "a.example.test");
    client
        .cookie_jar_mut()
        .add_from_header("foreign=value", "b.example.test");
    let origin = Origin::from_url(&"http://a.example.test/page".parse().unwrap());
    let result = fetch(
        &mut client,
        HttpRequest::new(Method::Get, "http://a.example.test/start".parse().unwrap()),
        &origin,
        RequestMode::Cors,
        CredentialsMode::SameOrigin,
        RedirectMode::Follow,
        &mut PreflightCache::default(),
    );
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        &*cookies.lock().unwrap(),
        &[Some("source=value".to_owned()), None, None]
    );
    assert_eq!(
        &*origins.lock().unwrap(),
        &[
            None,
            Some("http://a.example.test".to_owned()),
            Some("null".to_owned()),
        ]
    );
}

#[test]
fn cors_redirect_back_to_owner_still_requires_cors_on_final_response() {
    let cookies = Arc::new(Mutex::new(Vec::new()));
    let origins = Arc::new(Mutex::new(Vec::new()));
    let mut client = Client::new();
    client.set_transport(Some(Arc::new(RoundTripCredentialTransport {
        cookies,
        origins,
        final_allows_cors: false,
    })));
    let origin = Origin::from_url(&"http://a.example.test/page".parse().unwrap());
    let result = fetch(
        &mut client,
        HttpRequest::new(Method::Get, "http://a.example.test/start".parse().unwrap()),
        &origin,
        RequestMode::Cors,
        CredentialsMode::SameOrigin,
        RedirectMode::Follow,
        &mut PreflightCache::default(),
    );
    assert!(matches!(result, Err(CorsError::CorsCheck)));
}

#[test]
fn monitored_coep_reports_redirect_violation_without_blocking_or_leaking_target() {
    struct Transport;

    impl HttpTransport for Transport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            Ok(match request.url().path() {
                "/start" => HttpResponse::new(
                    302,
                    "Found",
                    vec![(
                        "Location".into(),
                        "http://other.example.test/private".into(),
                    )],
                    Vec::new(),
                ),
                "/private" => HttpResponse::new(200, "OK", Vec::new(), b"visible".to_vec()),
                path => panic!("unexpected request: {path}"),
            })
        }
    }

    let mut client = Client::new();
    client.set_transport(Some(Arc::new(Transport)));
    let origin = Origin::from_url(&"http://source.example.test/page".parse().unwrap());
    let mut violations = Vec::new();
    let fetched = fetch_with_embedder_policy_reporting(
        &mut client,
        HttpRequest::new(
            Method::Get,
            "http://source.example.test/start".parse().unwrap(),
        ),
        &origin,
        RequestMode::NoCors,
        CredentialsMode::SameOrigin,
        RedirectMode::Follow,
        &mut PreflightCache::default(),
        super::super::corp::EmbedderPolicyValue::UnsafeNone,
        super::super::corp::EmbedderPolicyValue::RequireCorp,
        &mut violations,
        None,
    )
    .unwrap();
    assert_eq!(fetched.response.body(), b"visible");
    assert_eq!(
        violations,
        vec![CorpViolation {
            blocked_url: "http://source.example.test/start".into(),
            report_only: true,
            enforced: false,
        }]
    );
}
