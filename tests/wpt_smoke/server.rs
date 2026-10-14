//! Local HTTP fixture server for WPT smoke cases.
use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use super::http_fixture::{
    FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback, read_request_headers,
};

pub(super) struct StaticServer {
    pub(super) base_url: String,
    stop: Arc<AtomicBool>,
    worker: Option<FixtureWorker<()>>,
}
impl StaticServer {
    pub(super) fn start(root: PathBuf) -> Self {
        let listener = bind_loopback().expect("bind WPT server");
        let address = listener.local_addr().expect("WPT server address");
        let secondary = bind_loopback().expect("bind secondary WPT server");
        let secondary_port = secondary.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = FixtureWorker::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                for listener in [&listener, &secondary] {
                    match accept_with_timeout(listener, Duration::from_millis(25)) {
                        Ok(stream) => serve(stream, &root, secondary_port),
                        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
                        Err(error) => panic!("WPT server accept: {error}"),
                    }
                }
            }
        });
        Self {
            base_url: format!("http://localhost:{}", address.port()),
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for StaticServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        drop(self.worker.take());
    }
}

fn escaped_request_body(body: &[u8]) -> Vec<u8> {
    let mut escaped = Vec::new();
    let mut index = 0;
    while index < body.len() {
        if body[index..].starts_with(b"\r\n") {
            escaped.extend_from_slice(b"\r\n");
            index += 2;
            continue;
        }
        match body[index] {
            b'\\' => escaped.extend_from_slice(b"\\\\"),
            value if value < 0x20 || value >= 0x7f => {
                escaped.extend_from_slice(format!("\\x{value:02x}").as_bytes())
            }
            value => escaped.push(value),
        }
        index += 1;
    }
    escaped
}

#[test]
fn echo_endpoint_preserves_crlf_and_escapes_request_bytes() {
    assert_eq!(
        escaped_request_body(b"a\r\n\n\0\x7f\xff\\<p>"),
        b"a\r\n\\x0a\\x00\\x7f\\xff\\\\<p>"
    );
}

fn serve(mut stream: TcpStream, root: &Path, secondary_port: u16) {
    let Ok(headers) = read_request_headers(&mut stream, READ_TIMEOUT) else {
        return;
    };
    let request_line = headers.lines().next().unwrap_or_default();
    let mut request_content_type = String::new();
    let mut content_length = 0;
    let mut body = Vec::new();
    for header in headers.lines().skip(1) {
        if let Some((key, value)) = header.split_once(':') {
            if key.eq_ignore_ascii_case("content-length") {
                let Ok(length) = value.trim().parse::<usize>() else {
                    return;
                };
                content_length = length;
            } else if key.eq_ignore_ascii_case("content-type") {
                request_content_type = value.trim().to_string();
            }
        }
    }
    body.resize(content_length, 0);
    if stream.set_read_timeout(Some(READ_TIMEOUT)).is_err() || stream.read_exact(&mut body).is_err()
    {
        return;
    }
    let target = request_line.split_whitespace().nth(1).unwrap_or("/");
    let path = target.split("?").next().unwrap_or("/");
    if super::cross_origin::serve(&mut stream, root, target, secondary_port) {
        return;
    }
    if path == "/FileAPI/file/resources/echo-content-escaped.py" {
        // Equivalent to the pinned wptserve endpoint: echo actual request
        // bytes, escaping controls/non-ASCII/backslashes and preserving CRLF.
        let escaped = escaped_request_body(&body);
        let method = request_line.split_whitespace().next().unwrap_or("GET");
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Length: {}\r\nX-Request-Method: {method}\r\nX-Request-Content-Length: {content_length}\r\nX-Request-Content-Type: {request_content_type}\r\nConnection: close\r\n\r\n",
            escaped.len()
        );
        let _ = stream.write_all(&escaped);
        return;
    }
    if path == "/common/sab.js" {
        // Upstream's buffer factory uses WebAssembly.Memory only to discover
        // the SharedArrayBuffer constructor in browsers that hide its global.
        // Omoikane exposes the real constructor but has no WebAssembly.Memory.
        // Adapt constructor discovery, retaining actual shared buffers and all
        // original test assertions (including transfer/detachment assertions).
        let body = br#"
const createBuffer = (type, length, opts) => {
  if (type === "ArrayBuffer") return new ArrayBuffer(length, opts);
  if (type === "SharedArrayBuffer") return new SharedArrayBuffer(length, opts);
  throw new Error("type has to be ArrayBuffer or SharedArrayBuffer");
};
"#;
        respond(&mut stream, 200, "text/javascript; charset=utf-8", body);
        return;
    }
    if path == "/resources/testharnessreport.js" {
        // This runner consumes result callbacks rather than the interactive
        // HTML report. Disable that report before tests start so its DOM-heavy
        // rendering cannot consume a page callback's execution budget.
        let body = br#"
setup({output:false});
globalThis.__wpt_results = [];
globalThis.__wpt_harness_status = -1;
globalThis.__wpt_complete = false;
globalThis.__wpt_allow_uncaught_exception = false;
if (typeof add_start_callback === 'function') {
  add_start_callback(properties => { globalThis.__wpt_allow_uncaught_exception = properties.allow_uncaught_exception === true; });
}
add_result_callback(test => globalThis.__wpt_results.push({name:String(test.name),status:Number(test.status),message:String(test.message||"")}));
add_completion_callback((tests,status) => { globalThis.__wpt_harness_status=Number(status.status); globalThis.__wpt_harness_message=String(status.message||""); globalThis.__wpt_complete=true; });
"#;
        respond(&mut stream, 200, "text/javascript; charset=utf-8", body);
        return;
    }
    if path == "/resources/testdriver-vendor.js" {
        respond(
            &mut stream,
            200,
            "text/javascript; charset=utf-8",
            super::testdriver::VENDOR_SCRIPT.as_bytes(),
        );
        return;
    }
    // wptserve's canonical rewrite supports the historical IDL parser URL.
    let path = if path == "/resources/WebIDLParser.js" {
        "/resources/webidl2/lib/webidl2.js"
    } else {
        path
    };
    let relative = path.trim_start_matches("/");
    if relative.split("/").any(|part| part == "..") {
        respond(&mut stream, 403, "text/plain", b"forbidden");
        return;
    }
    let file = root.join(relative);
    match fs::read(&file) {
        Ok(body) => match fixture_headers(&file, target) {
            Ok(headers) => {
                respond_with_headers(&mut stream, 200, content_type(&file), &body, &headers)
            }
            Err(_) => respond(&mut stream, 500, "text/plain", b"invalid static headers"),
        },
        Err(_) => respond(&mut stream, 404, "text/plain", b"not found"),
    }
}

/// Reads static `.headers`/`.sub.headers` sidecars and `pipe=header(...)`
/// stages. Repeated fields retain their order and remain separate.
fn fixture_headers(file: &Path, target: &str) -> io::Result<Vec<(String, String)>> {
    let mut headers = Vec::new();
    for suffix in [".headers", ".sub.headers"] {
        let mut sidecar_name = file.as_os_str().to_os_string();
        sidecar_name.push(suffix);
        let source = match fs::read_to_string(PathBuf::from(sidecar_name)) {
            Ok(source) => source,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for line in source
            .lines()
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .filter(|line| !line.is_empty() && !line.trim_start().starts_with('#'))
        {
            headers.push(parse_fixture_header(line)?);
        }
        break;
    }
    if let Some((_, query)) = target.split_once('?') {
        for (name, value) in url::form_urlencoded::parse(query.as_bytes()) {
            if name != "pipe" {
                continue;
            }
            for stage in value.split('|') {
                let Some(argument) = stage
                    .trim()
                    .strip_prefix("header(")
                    .and_then(|stage| stage.strip_suffix(')'))
                else {
                    continue;
                };
                let Some((name, value)) = argument.split_once(',') else {
                    continue;
                };
                let value = value
                    .strip_suffix(",True")
                    .or_else(|| value.strip_suffix(",False"))
                    .unwrap_or(value);
                // wptserve permits encoded ASCII whitespace in a CSP header
                // stage. Fold controls to spaces before writing HTTP headers
                // so they separate directive tokens without injecting fields.
                let value = value
                    .chars()
                    .map(|ch| match ch {
                        '\t' | '\n' | '\x0c' | '\r' => ' ',
                        _ => ch,
                    })
                    .collect::<String>();
                if let Ok(header) = parse_fixture_header(&format!("{name}: {value}")) {
                    headers.push(header);
                }
            }
        }
    }
    Ok(headers)
}

fn parse_fixture_header(line: &str) -> io::Result<(String, String)> {
    let (name, value) = line
        .split_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid static header"))?;
    let name = name.trim();
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || value.bytes().any(|byte| byte == b'\r' || byte == b'\n')
        || name.eq_ignore_ascii_case("content-length")
        || name.eq_ignore_ascii_case("connection")
        || name.eq_ignore_ascii_case("transfer-encoding")
        || name.eq_ignore_ascii_case("trailer")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid static header",
        ));
    }
    Ok((name.to_owned(), value.trim().to_owned()))
}

pub(super) fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    respond_with_headers(stream, status, content_type, body, &[]);
}

fn respond_with_headers(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    headers: &[(String, String)],
) {
    let reason = if status == 200 { "OK" } else { "Error" };
    let mut header = format!("HTTP/1.1 {status} {reason}\r\n");
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        header.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    for (name, value) in headers {
        header.push_str(&format!("{name}: {value}\r\n"));
    }
    header.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}
fn content_type(path: &Path) -> &str {
    match path.extension().and_then(|value| value.to_str()) {
        // Match wptserve's default MIME mapping: an HTML fixture's encoding
        // declaration must not be overridden by an invented transport charset.
        Some("html" | "htm") => "text/html",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn request(server: &StaticServer, bytes: &[u8]) -> (String, Vec<u8>) {
        let address = server.base_url.strip_prefix("http://").unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(bytes).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let separator = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("HTTP response header terminator");
        (
            String::from_utf8(response[..separator].to_vec()).unwrap(),
            response[separator + 4..].to_vec(),
        )
    }

    fn get(server: &StaticServer, path: &str) -> (String, Vec<u8>) {
        request(
            server,
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
        )
    }

    #[test]
    fn serves_static_files_and_reports_missing_or_forbidden_paths() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "omoikane-wpt-smoke-server-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("hello.html"), b"hello").unwrap();
        fs::write(root.join("policy.html"), b"<p>controlled</p>").unwrap();
        fs::write(
            root.join("policy.html.headers"),
            b"Permissions-Policy: fullscreen=()\r\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("resources/webidl2/lib")).unwrap();
        fs::write(
            root.join("resources/webidl2/lib/webidl2.js"),
            b"var WebIDL2 = {};",
        )
        .unwrap();
        let server = StaticServer::start(root.clone());
        assert!(server.base_url.starts_with("http://localhost:"));

        let (header, body) = get(&server, "/hello.html?cache=1");
        assert_eq!(
            header,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 5\r\nConnection: close"
        );
        assert_eq!(body, b"hello");
        let (header, body) = get(&server, "/policy.html");
        assert!(header.contains("Permissions-Policy: fullscreen=()\r\n"));
        assert_eq!(body, b"<p>controlled</p>");
        let (header, body) = get(&server, "/hello.html?pipe=header(Content-Length,0)");
        assert_eq!(header.matches("Content-Length: ").count(), 1);
        assert!(header.contains("Content-Length: 5\r\n"));
        assert_eq!(body, b"hello");
        let (header, body) = get(&server, "/resources/WebIDLParser.js");
        assert!(header.starts_with("HTTP/1.1 200 OK"));
        assert!(header.contains("Content-Type: text/javascript"));
        assert_eq!(body, b"var WebIDL2 = {};");

        for invalid in [
            "Content-Length: 0\n",
            "Transfer-Encoding: chunked\n",
            "Trailer: X-Test\n",
        ] {
            fs::write(root.join("policy.html.headers"), invalid).unwrap();
            let (header, body) = get(&server, "/policy.html");
            assert!(header.starts_with("HTTP/1.1 500 Error\r\n"));
            assert_eq!(body, b"invalid static headers");
        }

        for (path, expected_header, expected_body) in [
            (
                "/missing.html",
                "HTTP/1.1 404 Error\r\nContent-Type: text/plain\r\nContent-Length: 9\r\nConnection: close",
                b"not found".as_slice(),
            ),
            (
                "/../hello.html",
                "HTTP/1.1 403 Error\r\nContent-Type: text/plain\r\nContent-Length: 9\r\nConnection: close",
                b"forbidden".as_slice(),
            ),
        ] {
            let (header, body) = get(&server, path);
            assert_eq!(header, expected_header, "{path}");
            assert_eq!(body, expected_body, "{path}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn serves_sidecar_and_pipe_csp_headers_as_separate_fields() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "omoikane-wpt-smoke-csp-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("page.html"), b"<body>page</body>").unwrap();
        fs::write(
            root.join("page.html.headers"),
            "Content-Security-Policy-Report-Only: require-trusted-types-for 'script'\n",
        )
        .unwrap();
        fs::write(root.join("worker.js"), b"postMessage('loaded');").unwrap();
        fs::write(
            root.join("worker.js.sub.headers"),
            "Content-Security-Policy: script-src 'none'\n",
        )
        .unwrap();
        let server = StaticServer::start(root.clone());
        let (header, body) = get(
            &server,
            "/page.html?pipe=header(Content-Security-Policy,connect-src%20%27none%27)%7Cheader(Content-Security-Policy,require-trusted-types-for%20%27script%27,True)",
        );
        assert_eq!(body, b"<body>page</body>");
        assert!(header.contains(
            "Content-Security-Policy-Report-Only: require-trusted-types-for 'script'\r\n"
        ));
        assert!(header.contains("Content-Security-Policy: connect-src 'none'\r\n"));
        assert!(header.contains("Content-Security-Policy: require-trusted-types-for 'script'\r\n"));
        assert_eq!(header.matches("Content-Security-Policy: ").count(), 2);
        let (header, body) = get(&server, "/worker.js");
        assert_eq!(body, b"postMessage('loaded');");
        assert!(header.contains("Content-Security-Policy: script-src 'none'\r\n"));
        let (header, _) = get(
            &server,
            "/page.html?pipe=header(Content-Security-Policy,require-trusted-types-for%20%27script%27%09%27script%27%0A%27script%27%0C%27script%27%0D%27script%27%20%27script%27,True)",
        );
        assert!(header.contains("Content-Security-Policy: require-trusted-types-for 'script' 'script' 'script' 'script' 'script' 'script'\r\n"));
        assert_eq!(header.matches("Content-Security-Policy: ").count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn serves_special_script_endpoints() {
        let server = StaticServer::start(PathBuf::from("tests/wpt"));
        for (path, markers) in [
            (
                "/common/sab.js",
                [
                    "new SharedArrayBuffer(length, opts)",
                    "new ArrayBuffer(length, opts)",
                ],
            ),
            (
                "/resources/testharnessreport.js",
                ["setup({output:false})", "add_completion_callback"],
            ),
            (
                "/resources/testdriver-vendor.js",
                [
                    "test_driver_internal.minimize_window",
                    "test_driver_internal.set_window_rect",
                ],
            ),
        ] {
            let (header, body) = get(&server, path);
            assert_eq!(
                header,
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/javascript; charset=utf-8\r\nContent-Length: {}\r\nConnection: close",
                    body.len()
                ),
                "{path}"
            );
            let script = String::from_utf8(body).unwrap();
            for marker in markers {
                assert!(script.contains(marker), "{path}: missing {marker}");
            }
        }
    }

    #[test]
    fn echo_endpoint_preserves_request_headers_and_body() {
        let server = StaticServer::start(PathBuf::new());
        let body = b"a\r\n\n\0\x7f\xff\\<p>";
        let mut bytes = format!(
            "POST /FileAPI/file/resources/echo-content-escaped.py?check=1 HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        bytes.extend_from_slice(body);
        let (header, echoed) = request(&server, &bytes);
        let expected = b"a\r\n\\x0a\\x00\\x7f\\xff\\\\<p>";
        assert_eq!(echoed, expected);
        assert_eq!(
            header,
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Length: {}\r\nX-Request-Method: POST\r\nX-Request-Content-Length: {}\r\nX-Request-Content-Type: text/plain\r\nConnection: close",
                expected.len(),
                body.len()
            )
        );
    }

    #[test]
    fn content_types_match_fixture_extensions() {
        for (path, expected) in [
            ("page.html", "text/html"),
            ("page.htm", "text/html"),
            ("script.js", "text/javascript; charset=utf-8"),
            ("style.css", "text/css; charset=utf-8"),
            ("data.json", "application/json"),
            ("image.bin", "application/octet-stream"),
        ] {
            assert_eq!(content_type(Path::new(path)), expected);
        }
    }
}
