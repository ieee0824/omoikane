//! Local HTTP fixture server for WPT smoke cases.
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

pub(super) struct StaticServer {
    pub(super) base_url: String,
}
impl StaticServer {
    pub(super) fn start(root: PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind WPT server");
        let address = listener.local_addr().expect("WPT server address");
        let root = Arc::new(root);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &root);
            }
        });
        Self {
            base_url: format!("http://{address}"),
        }
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

fn serve(mut stream: TcpStream, root: &Path) {
    let mut request_line = String::new();
    let mut request_content_type = String::new();
    let mut content_length = 0;
    let mut body = Vec::new();
    {
        let mut reader = BufReader::new(&stream);
        if reader.read_line(&mut request_line).is_err() {
            return;
        }
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).is_err() {
                return;
            }
            if header == "\r\n" || header.is_empty() {
                break;
            }
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
        if reader.read_exact(&mut body).is_err() {
            return;
        }
    }
    let target = request_line.split_whitespace().nth(1).unwrap_or("/");
    let path = target.split("?").next().unwrap_or("/");
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
add_result_callback(test => globalThis.__wpt_results.push({name:String(test.name),status:Number(test.status),message:String(test.message||"")}));
add_completion_callback((tests,status) => { globalThis.__wpt_harness_status=Number(status.status); globalThis.__wpt_complete=true; });
"#;
        respond(&mut stream, 200, "text/javascript; charset=utf-8", body);
        return;
    }
    if path == "/resources/testdriver-vendor.js" {
        // The pinned visibility-state WPT asks the browser to minimize and
        // restore its window. Queue these requests for the Rust test runner,
        // which owns the page's host visibility state.
        let body = br#"
globalThis.__wpt_window_commands = [];
test_driver_internal.minimize_window = () => new Promise(resolve => {
  __wpt_window_commands.push({hidden: true, resolve});
});
test_driver_internal.set_window_rect = () => new Promise(resolve => {
  __wpt_window_commands.push({hidden: false, resolve});
});
"#;
        respond(&mut stream, 200, "text/javascript; charset=utf-8", body);
        return;
    }
    let relative = path.trim_start_matches("/");
    if relative.split("/").any(|part| part == "..") {
        respond(&mut stream, 403, "text/plain", b"forbidden");
        return;
    }
    let file = root.join(relative);
    match fs::read(&file) {
        Ok(body) => respond(&mut stream, 200, content_type(&file), &body),
        Err(_) => respond(&mut stream, 404, "text/plain", b"not found"),
    }
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = if status == 200 { "OK" } else { "Error" };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}
fn content_type(path: &Path) -> &str {
    match path.extension().and_then(|value| value.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
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
        let server = StaticServer::start(root.clone());
        assert!(server.base_url.starts_with("http://127.0.0.1:"));

        let (header, body) = get(&server, "/hello.html?cache=1");
        assert_eq!(
            header,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 5\r\nConnection: close"
        );
        assert_eq!(body, b"hello");

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
            ("page.html", "text/html; charset=utf-8"),
            ("page.htm", "text/html; charset=utf-8"),
            ("script.js", "text/javascript; charset=utf-8"),
            ("style.css", "text/css; charset=utf-8"),
            ("data.json", "application/json"),
            ("image.bin", "application/octet-stream"),
        ] {
            assert_eq!(content_type(Path::new(path)), expected);
        }
    }
}
