use omoikane::http::{Client, HttpRequest, Method, Url};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

#[test]
fn cross_site_requests_apply_samesite_to_subresources_and_navigations() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for expected in [None, None, Some("lax=1")] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut cookie = None;
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.strip_prefix("Cookie: ") {
                    cookie = Some(value.trim().to_string());
                }
            }
            assert_eq!(cookie.as_deref(), expected);
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
    server.join().unwrap();
}
