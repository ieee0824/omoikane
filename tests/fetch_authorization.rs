#[path = "support/http_fixture.rs"]
mod http_fixture;

use http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};
use omoikane::http::cors::{
    self, CredentialsMode, Origin, PreflightCache, RedirectMode, RequestMode,
};
use omoikane::http::{Client, HttpRequest, Method, Url};
use std::io::Write;
use std::net::TcpStream;

const PAGE_ORIGIN: &str = "http://127.0.0.1:1";

fn read_request(stream: &mut TcpStream) -> String {
    read_request_headers(stream, READ_TIMEOUT)
        .unwrap()
        .to_ascii_lowercase()
}

fn respond(stream: &mut TcpStream, status: &str, extra: &str) {
    write!(stream, "HTTP/1.1 {status}\r\nAccess-Control-Allow-Origin: {PAGE_ORIGIN}\r\nAccess-Control-Allow-Credentials: true\r\nAccess-Control-Allow-Methods: POST\r\nAccess-Control-Allow-Headers: authorization\r\nContent-Length: 0\r\nConnection: close\r\n{extra}\r\n").unwrap();
}

fn request(url: Url) -> HttpRequest {
    let mut request = HttpRequest::new(Method::Post, url);
    request.set_header("Authorization", "Bearer synthetic-test-token");
    request
}

#[test]
fn explicit_authorization_is_independent_of_automatic_cookie_credentials() {
    for credentials in [
        CredentialsMode::Omit,
        CredentialsMode::SameOrigin,
        CredentialsMode::Include,
    ] {
        let listener = bind_loopback().unwrap();
        let url: Url = format!("http://{}/activate", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let server = FixtureWorker::spawn(move || {
            let mut preflight = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let preflight_request = read_request(&mut preflight);
            respond(&mut preflight, "204 No Content", "");
            let mut actual = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let actual_request = read_request(&mut actual);
            respond(&mut actual, "200 OK", "Set-Cookie: after=1; Path=/\r\n");
            (preflight_request, actual_request)
        });
        let mut client = Client::new();
        client
            .cookie_jar_mut()
            .add_from_header_for_url("before=1; Path=/", &url);
        let origin = Origin::from_url(&format!("{PAGE_ORIGIN}/").parse().unwrap());
        let response = cors::fetch(
            &mut client,
            request(url.clone()),
            &origin,
            RequestMode::Cors,
            credentials,
            RedirectMode::Follow,
            &mut PreflightCache::default(),
        )
        .unwrap();
        assert_eq!(response.response.status_code(), 200);
        let (preflight, actual) = server.join();
        assert!(preflight.starts_with("options "));
        assert!(preflight.contains("access-control-request-headers: authorization\r\n"));
        assert!(!preflight.contains("\r\nauthorization:"));
        assert!(!preflight.contains("\r\ncookie:"));
        assert!(
            actual.contains("\r\nauthorization: bearer synthetic-test-token\r\n"),
            "{credentials:?}: {actual}"
        );
        let include = credentials == CredentialsMode::Include;
        assert_eq!(actual.contains("\r\ncookie: before=1\r\n"), include);
        assert_eq!(
            client
                .cookie_jar()
                .cookie_header(&url)
                .unwrap()
                .contains("after=1"),
            include
        );
    }
}

#[test]
fn a_redirect_to_another_origin_still_removes_explicit_authorization() {
    let first = bind_loopback().unwrap();
    let next = bind_loopback().unwrap();
    let first_url: Url = format!("http://{}/start", first.local_addr().unwrap())
        .parse()
        .unwrap();
    let next_url = format!("http://{}/destination", next.local_addr().unwrap());
    let server = FixtureWorker::spawn(move || {
        let mut preflight = accept_with_timeout(&first, ACCEPT_TIMEOUT).unwrap();
        assert!(read_request(&mut preflight).starts_with("options "));
        respond(&mut preflight, "204 No Content", "");
        let mut initial = accept_with_timeout(&first, ACCEPT_TIMEOUT).unwrap();
        let initial_request = read_request(&mut initial);
        respond(
            &mut initial,
            "307 Temporary Redirect",
            &format!("Location: {next_url}\r\n"),
        );
        let mut redirected = accept_with_timeout(&next, ACCEPT_TIMEOUT).unwrap();
        let redirected_request = read_request(&mut redirected);
        respond(&mut redirected, "200 OK", "");
        (initial_request, redirected_request)
    });
    let origin = Origin::from_url(&format!("{PAGE_ORIGIN}/").parse().unwrap());
    let response = cors::fetch(
        &mut Client::new(),
        request(first_url),
        &origin,
        RequestMode::Cors,
        CredentialsMode::Include,
        RedirectMode::Follow,
        &mut PreflightCache::default(),
    )
    .unwrap();
    assert_eq!(response.response.status_code(), 200);
    let (initial, redirected) = server.join();
    assert!(initial.contains("\r\nauthorization: bearer synthetic-test-token\r\n"));
    assert!(redirected.starts_with("post /destination "));
    assert!(!redirected.contains("\r\nauthorization:"));
}
