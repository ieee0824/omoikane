//! Request address restrictions must cover preflights and opted-in redirects.
use super::*;
use crate::http::client::HttpTransport;
use crate::http::response::HttpParseError;
use crate::test_support::http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

struct RecordingTransport {
    requests: Mutex<Vec<(HttpRequest, Option<Duration>)>>,
    responses: Mutex<VecDeque<HttpResponse>>,
}

impl RecordingTransport {
    fn new(responses: Vec<HttpResponse>) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(responses.into()),
        })
    }
}

impl HttpTransport for RecordingTransport {
    fn send(
        &self,
        request: &HttpRequest,
        timeout: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        self.requests
            .lock()
            .unwrap()
            .push((request.clone(), timeout));
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected network exchange"))
    }
}

fn response(status: u16, location: Option<&str>) -> HttpResponse {
    let mut headers = vec![
        ("Access-Control-Allow-Origin".into(), "*".into()),
        ("Access-Control-Allow-Methods".into(), "POST".into()),
        ("Access-Control-Allow-Headers".into(), "content-type".into()),
    ];
    if let Some(location) = location {
        headers.push(("Location".into(), location.to_owned()));
    }
    HttpResponse::new(status, "Fixture", headers, Vec::new())
}

fn report_request(url: &str, public_only: bool) -> HttpRequest {
    let mut request = HttpRequest::new(Method::Post, url.parse().unwrap());
    request.set_header("Content-Type", "application/reports+json");
    request.set_body(b"[]".to_vec());
    if public_only {
        request.require_public_ip();
    }
    request
}

#[test]
fn cors_redirect_taint_serializes_origin_as_null_and_checks_the_final_response() {
    const OWNER: &str = "https://app.test";
    for allow_null in [false, true] {
        let transport = RecordingTransport::new(vec![
            response(302, Some("https://first.test/step")),
            response(302, Some("https://second.test/final")),
            HttpResponse::new(
                200,
                "Fixture",
                vec![(
                    "Access-Control-Allow-Origin".into(),
                    if allow_null { "null" } else { OWNER }.into(),
                )],
                Vec::new(),
            ),
        ]);
        let mut client = Client::new();
        client.set_transport(Some(transport.clone()));
        let result = fetch_with_timeout(
            &mut client,
            HttpRequest::get("https://app.test/start").unwrap(),
            &Origin::from_url(&"https://app.test/document".parse().unwrap()),
            RequestMode::Cors,
            CredentialsMode::SameOrigin,
            RedirectMode::Follow,
            &mut PreflightCache::default(),
            None,
        );
        assert_eq!(
            result.is_ok(),
            allow_null,
            "allow_null={allow_null}: {result:?}"
        );
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].0.header("Origin"), None);
        assert_eq!(requests[1].0.header("Origin"), Some(OWNER));
        assert_eq!(requests[2].0.header("Origin"), Some("null"));
    }
}

#[test]
fn preflights_inherit_the_actual_requests_address_restriction() {
    for public_only in [false, true] {
        let transport = RecordingTransport::new(vec![response(204, None), response(200, None)]);
        let mut client = Client::new();
        client.set_transport(Some(transport.clone()));
        let timeout = Some(Duration::from_millis(25));
        let result = fetch_with_timeout(
            &mut client,
            report_request("https://collector.test/reports", public_only),
            &Origin::from_url(&"https://app.test/document".parse().unwrap()),
            RequestMode::Cors,
            CredentialsMode::SameOrigin,
            RedirectMode::Follow,
            &mut PreflightCache::default(),
            timeout,
        )
        .unwrap();
        assert_eq!(result.response.status_code(), 200);
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].0.method(), Method::Options);
        assert_eq!(requests[1].0.method(), Method::Post);
        for (request, actual_timeout) in requests.iter() {
            assert_eq!(
                request.requires_public_ip(),
                public_only,
                "{:?}",
                request.method()
            );
            assert_eq!(*actual_timeout, timeout);
        }
        assert_eq!(requests[0].0.body(), None);
        assert_eq!(requests[1].0.body(), Some(&b"[]"[..]));
    }
}

#[test]
fn opted_in_redirects_restrict_foreign_origins_and_keep_the_restriction() {
    for initial_public_only in [false, true] {
        let transport = RecordingTransport::new(vec![
            response(307, Some("/step")),
            response(307, Some("https://collector.test/receive")),
            response(204, None),
            response(307, Some("https://app.test/final")),
            response(204, None),
            response(200, None),
        ]);
        let mut client = Client::new();
        client.set_transport(Some(transport.clone()));
        let result = fetch_with_timeout_restricting_redirects(
            &mut client,
            report_request("https://app.test/reports", initial_public_only),
            &Origin::from_url(&"https://app.test/document".parse().unwrap()),
            RequestMode::Cors,
            CredentialsMode::SameOrigin,
            RedirectMode::Follow,
            &mut PreflightCache::default(),
            None,
        )
        .unwrap();
        assert!(result.redirected);
        assert_eq!(result.response.redirect_count(), 3);
        assert_eq!(
            result.response.effective_url().unwrap().to_string(),
            "https://app.test/final"
        );
        let requests = transport.requests.lock().unwrap();
        let observed = requests
            .iter()
            .map(|(request, _)| {
                (
                    request.method(),
                    request.url().to_string(),
                    request.requires_public_ip(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            observed,
            vec![
                (
                    Method::Post,
                    "https://app.test/reports".into(),
                    initial_public_only
                ),
                (
                    Method::Post,
                    "https://app.test/step".into(),
                    initial_public_only
                ),
                (
                    Method::Options,
                    "https://collector.test/receive".into(),
                    true
                ),
                (Method::Post, "https://collector.test/receive".into(), true),
                (Method::Options, "https://app.test/final".into(), true),
                (Method::Post, "https://app.test/final".into(), true),
            ]
        );
        assert_eq!(requests[4].0.header("Origin"), Some("null"));
        assert_eq!(requests[5].0.header("Origin"), Some("null"));
        assert!(
            requests
                .iter()
                .filter(|(request, _)| request.method() == Method::Post)
                .all(|(request, _)| request.body() == Some(&b"[]"[..]))
        );
    }
}

#[test]
fn ordinary_fetch_preserves_explicitly_unrestricted_redirects() {
    let transport = RecordingTransport::new(vec![
        response(307, Some("https://collector.test/receive")),
        response(204, None),
        response(200, None),
    ]);
    let mut client = Client::new();
    client.set_transport(Some(transport.clone()));
    let result = fetch(
        &mut client,
        report_request("https://app.test/reports", false),
        &Origin::from_url(&"https://app.test/document".parse().unwrap()),
        RequestMode::Cors,
        CredentialsMode::SameOrigin,
        RedirectMode::Follow,
        &mut PreflightCache::default(),
    )
    .unwrap();
    assert!(result.redirected);
    assert_eq!(result.response.redirect_count(), 1);
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].0.method(), Method::Post);
    assert_eq!(requests[1].0.method(), Method::Options);
    assert_eq!(requests[2].0.method(), Method::Post);
    assert!(
        requests
            .iter()
            .all(|(request, _)| !request.requires_public_ip())
    );
}

#[test]
fn guarded_preflight_never_connects_to_a_private_endpoint() {
    let collector = bind_loopback().unwrap();
    let url = format!("http://{}/reports", collector.local_addr().unwrap());
    let result = fetch_with_timeout(
        &mut Client::new(),
        report_request(&url, true),
        &Origin::from_url(&"https://app.test/document".parse().unwrap()),
        RequestMode::Cors,
        CredentialsMode::SameOrigin,
        RedirectMode::Follow,
        &mut PreflightCache::default(),
        Some(Duration::from_millis(100)),
    );
    assert!(
        matches!(result, Err(CorsError::Network(ref error)) if error.contains("no permitted address")),
        "guard must reject before sending OPTIONS: {result:?}"
    );
    assert!(matches!(
        collector.accept(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn opted_in_redirect_never_connects_to_a_foreign_private_endpoint() {
    let initial = bind_loopback().unwrap();
    let collector = bind_loopback().unwrap();
    let initial_url = format!("http://{}/reports", initial.local_addr().unwrap());
    let collector_url = format!("http://{}/collect", collector.local_addr().unwrap());
    let worker = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&initial, ACCEPT_TIMEOUT).unwrap();
        let headers = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        assert!(headers.starts_with("POST /reports "));
        let mut body = [0; 2];
        stream.read_exact(&mut body).unwrap();
        assert_eq!(&body, b"[]");
        write!(
            stream,
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {collector_url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
    });
    let result = fetch_with_timeout_restricting_redirects(
        &mut Client::new(),
        report_request(&initial_url, false),
        &Origin::from_url(&initial_url.parse().unwrap()),
        RequestMode::Cors,
        CredentialsMode::SameOrigin,
        RedirectMode::Follow,
        &mut PreflightCache::default(),
        Some(Duration::from_secs(1)),
    );
    worker.join();
    assert!(
        matches!(result, Err(CorsError::Network(ref error)) if error.contains("no permitted address")),
        "initial local request is allowed, foreign redirect must be rejected: {result:?}"
    );
    assert!(matches!(
        collector.accept(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));
}
