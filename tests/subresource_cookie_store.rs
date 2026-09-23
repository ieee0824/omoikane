//! Subresources must use the browsing session's Cookie store.
use omoikane::{cdp::CdpSession, frame::render_browser_frame, html::TreeBuilder, js::JsRuntime};
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

fn accept_with_timeout(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for resource request"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept failed: {error}"),
        }
    }
}

fn read_request(stream: &TcpStream) -> (String, Option<String>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let path = line.split_whitespace().nth(1).unwrap().to_string();
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
    (path, cookie)
}

#[test]
fn stylesheet_shares_navigation_and_document_cookies() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let mut stream = accept_with_timeout(&listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let (path, cookie) = read_request(&stream);
            let (body, extra) = match path.as_str() {
                "/page" => (
                    "<html><head><link rel='stylesheet' href='/style.css'></head><body>hello</body></html>",
                    "Set-Cookie: page=1; Path=/\r\n",
                ),
                "/style.css" => (
                    "body { color: rgb(1, 2, 3) }",
                    "Set-Cookie: style=2; Path=/\r\nContent-Type: text/css\r\n",
                ),
                _ => panic!("unexpected request: {path}"),
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{}",
                body.len(),
                extra,
                body
            )
            .unwrap();
            requests.push((path, cookie));
        }
        requests
    });

    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url": format!("http://{address}/page")}),
        )
        .unwrap();
    let result = session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression": "getComputedStyle(document.body).color + '|' + document.cookie"}),
        )
        .unwrap();
    let requests = server.join().unwrap();
    assert_eq!(requests[0], ("/page".to_string(), None));
    assert_eq!(
        requests[1],
        ("/style.css".to_string(), Some("page=1".to_string()))
    );
    assert_eq!(result["result"]["value"], "rgb(1, 2, 3)|page=1; style=2");
}

#[test]
fn image_shares_navigation_and_document_cookies() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[255, 0, 0, 255])
            .unwrap();
    }
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let mut stream = accept_with_timeout(&listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let (path, cookie) = read_request(&stream);
            match path.as_str() {
                "/page" => {
                    let body = "<html><body><img src='/image.png'></body></html>";
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nSet-Cookie: page=1; Path=/\r\nConnection: close\r\n\r\n{}",
                        body.len(), body,
                    )
                    .unwrap();
                }
                "/image.png" => {
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nSet-Cookie: image=2; Path=/\r\nConnection: close\r\n\r\n",
                        png.len(),
                    )
                    .unwrap();
                    stream.write_all(&png).unwrap();
                }
                _ => panic!("unexpected request: {path}"),
            }
            requests.push((path, cookie));
        }
        requests
    });

    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url": format!("http://{address}/page")}),
        )
        .unwrap();
    render_browser_frame(&mut session, 100, 100, 16).unwrap();
    let cookies = session
        .dispatch("Runtime.evaluate", json!({"expression": "document.cookie"}))
        .unwrap();
    let requests = server.join().unwrap();
    assert_eq!(requests[0], ("/page".to_string(), None));
    assert_eq!(
        requests[1],
        ("/image.png".to_string(), Some("page=1".to_string()))
    );
    assert_eq!(cookies["result"]["value"], "page=1; image=2");
}

#[test]
fn module_shares_document_cookies_in_both_directions() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = accept_with_timeout(&listener);
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let request = read_request(&stream);
        let body = "globalThis.moduleCookie = document.cookie;";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nSet-Cookie: module=2; Path=/\r\nConnection: close\r\n\r\n{}",
            body.len(), body,
        )
        .unwrap();
        request
    });

    let url = format!("http://{address}/page");
    let document = TreeBuilder::parse(
        "<html><body><script type='module' src='/module.js'></script></body></html>",
    )
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, &url).unwrap();
    runtime.eval("document.cookie = 'page=1; Path=/'").unwrap();
    let errors = runtime.execute_document_scripts(Some(&url.parse().unwrap()));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        server.join().unwrap(),
        ("/module.js".to_string(), Some("page=1".to_string()))
    );
    assert_eq!(
        runtime
            .eval("document.cookie === 'page=1; module=2' && moduleCookie === 'page=1; module=2'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn image_cache_does_not_cross_browsing_sessions() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[255, 0, 0, 255])
            .unwrap();
    }
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for session_number in 1..=2 {
            for _ in 0..2 {
                let mut stream = accept_with_timeout(&listener);
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let request = read_request(&stream);
                match request.0.as_str() {
                    "/page" => {
                        let body = "<html><body><img src='/image.png'></body></html>";
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nSet-Cookie: session={session_number}; Path=/\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    "/image.png" => {
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", png.len()).unwrap();
                        stream.write_all(&png).unwrap();
                    }
                    _ => panic!("unexpected request: {}", request.0),
                }
                requests.push(request);
            }
        }
        requests
    });
    for _ in 0..2 {
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch(
                "Page.navigate",
                json!({"url": format!("http://{address}/page")}),
            )
            .unwrap();
        render_browser_frame(&mut session, 100, 100, 16).unwrap();
    }
    let requests = server.join().unwrap();
    assert_eq!(requests[0], ("/page".to_string(), None));
    assert_eq!(
        requests[1],
        ("/image.png".to_string(), Some("session=1".to_string()))
    );
    assert_eq!(requests[2], ("/page".to_string(), None));
    assert_eq!(
        requests[3],
        ("/image.png".to_string(), Some("session=2".to_string()))
    );
}

#[test]
fn fetch_and_navigation_share_response_cookies() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for path in ["/page", "/fetch", "/after"] {
            let mut stream = accept_with_timeout(&listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let request = read_request(&stream);
            assert_eq!(request.0, path);
            let extra = match path {
                "/page" => "Set-Cookie: page=1; Path=/\r\n",
                "/fetch" => "Set-Cookie: fetched=2; Path=/\r\n",
                _ => "",
            };
            let body = "<html><body>cookie page</body></html>";
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}", body.len()).unwrap();
            requests.push(request);
        }
        requests
    });
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url": format!("http://{address}/page")}),
        )
        .unwrap();
    session
        .dispatch("Runtime.evaluate", json!({"expression": "fetch('/fetch')"}))
        .unwrap();
    let cookies = session
        .dispatch("Runtime.evaluate", json!({"expression": "document.cookie"}))
        .unwrap();
    assert_eq!(cookies["result"]["value"], "page=1; fetched=2");
    session
        .dispatch(
            "Page.navigate",
            json!({"url": format!("http://{address}/after")}),
        )
        .unwrap();
    let requests = server.join().unwrap();
    assert_eq!(requests[0], ("/page".to_string(), None));
    assert_eq!(
        requests[1],
        ("/fetch".to_string(), Some("page=1".to_string()))
    );
    assert_eq!(
        requests[2],
        ("/after".to_string(), Some("page=1; fetched=2".to_string()))
    );
}

#[test]
fn cross_site_iframe_and_image_exclude_strict_and_lax_but_top_navigation_sends_lax() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[255, 0, 0, 255])
            .unwrap();
    }
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..6 {
            let mut stream = accept_with_timeout(&listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let request = read_request(&stream);
            let (body, extra) = match request.0.as_str() {
                "/seed" => (b"<html><body><img src='/image.png'></body></html>".as_slice(), "Set-Cookie: strict=1; SameSite=Strict; Path=/\r\nSet-Cookie: lax=2; SameSite=Lax; Path=/\r\nContent-Type: text/html\r\n"),
                "/parent" => (b"<html><body><iframe src='http://127.0.0.1:PORT/child'></iframe><img src='http://127.0.0.1:PORT/image.png'></body></html>".as_slice(), "Content-Type: text/html\r\n"),
                "/child" | "/final" => (b"<html></html>".as_slice(), "Content-Type: text/html\r\n"),
                "/image.png" => (png.as_slice(), "Content-Type: image/png\r\n"),
                _ => panic!("unexpected request: {}", request.0),
            };
            let body = if request.0 == "/parent" {
                String::from_utf8_lossy(body)
                    .replace("PORT", &address.port().to_string())
                    .into_bytes()
            } else {
                body.to_vec()
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
            let done = request.0 == "/final";
            requests.push(request);
            if done {
                break;
            }
        }
        requests
    });
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url": format!("http://{address}/seed")}),
        )
        .unwrap();
    render_browser_frame(&mut session, 100, 100, 16).unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url": format!("http://localhost:{}/parent", address.port())}),
        )
        .unwrap();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression": "document.querySelector('iframe').contentDocument === null"}),
        )
        .unwrap();
    render_browser_frame(&mut session, 100, 100, 16).unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url": format!("http://{address}/final")}),
        )
        .unwrap();
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 6, "{requests:?}");
    assert_eq!(requests[0], ("/seed".to_string(), None));
    assert_eq!(
        requests[1],
        (
            "/image.png".to_string(),
            Some("strict=1; lax=2".to_string())
        )
    );
    assert_eq!(requests[2], ("/parent".to_string(), None));
    assert!(
        requests[3..5].contains(&("/child".to_string(), None)),
        "{requests:?}"
    );
    assert!(
        requests[3..5].contains(&("/image.png".to_string(), None)),
        "{requests:?}"
    );
    assert_eq!(
        requests[5],
        ("/final".to_string(), Some("lax=2".to_string()))
    );
}
