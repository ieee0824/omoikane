use omoikane::http::{Client, HttpRequest, Method, Url};
use std::io::Write;

#[path = "support/http_fixture.rs"]
mod http_fixture;

use http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};

#[test]
fn cross_site_requests_apply_samesite_to_subresources_and_navigations() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        for expected in [None, None, Some("lax=1")] {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let cookie = request
                .lines()
                .find_map(|line| line.strip_prefix("Cookie: ").map(str::trim));
            assert_eq!(cookie, expected);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
        }
    });

    let target: Url = format!("http://{address}/resource").parse().unwrap();
    let other_site: Url = "http://localhost/".parse().unwrap();
    let mut client = Client::new();
    client
        .cookie_jar_mut()
        .add_from_header_for_url("strict=1; SameSite=Strict; Path=/", &target);
    client
        .cookie_jar_mut()
        .add_from_header_for_url("lax=1; SameSite=Lax; Path=/", &target);
    for (method, top_level) in [
        (Method::Get, false),
        (Method::Post, true),
        (Method::Get, true),
    ] {
        let mut request = HttpRequest::new(method, target.clone());
        request.set_cookie_context(other_site.clone(), top_level);
        assert_eq!(client.send(request).unwrap().status_code(), 200);
    }
    server.join();
}
