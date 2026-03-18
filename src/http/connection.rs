//! TCP connection handling for HTTP requests.

use std::io::{self, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use super::request::HttpRequest;
use super::response::{HttpParseError, HttpResponse};

/// Default read timeout in seconds.
const DEFAULT_READ_TIMEOUT_SECS: u64 = 30;

/// Sends an HTTP request over a new TCP connection and returns the response.
///
/// Opens a TCP connection to the host and port specified in the request URL,
/// writes the serialized request, and parses the response.
///
/// Only `http` URLs are supported. `https` URLs will return an error until
/// TLS support is implemented.
///
/// # Errors
///
/// Returns an error if the scheme is `https`, the connection cannot be
/// established, the request cannot be sent, or the response cannot be parsed.
///
/// # Examples
///
/// ```no_run
/// use omoikane::http::{HttpRequest, send};
///
/// let req = HttpRequest::get("http://example.com/").unwrap();
/// let resp = send(&req).unwrap();
/// println!("Status: {}", resp.status_code());
/// ```
pub fn send(request: &HttpRequest) -> Result<HttpResponse, HttpParseError> {
    let url = request.url();

    if url.scheme() == "https" {
        return Err(HttpParseError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            "HTTPS is not yet supported; see issue 001-2",
        )));
    }

    let addr = format!("{}:{}", url.host(), url.port());
    let socket_addr = addr
        .to_socket_addrs()
        .map_err(HttpParseError::Io)?
        .next()
        .ok_or_else(|| {
            HttpParseError::Io(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                format!("could not resolve address: {addr}"),
            ))
        })?;

    let mut stream =
        TcpStream::connect_timeout(&socket_addr, Duration::from_secs(DEFAULT_READ_TIMEOUT_SECS))
            .map_err(HttpParseError::Io)?;

    stream
        .set_read_timeout(Some(Duration::from_secs(DEFAULT_READ_TIMEOUT_SECS)))
        .map_err(HttpParseError::Io)?;

    stream
        .write_all(&request.serialize())
        .map_err(HttpParseError::Io)?;
    stream.flush().map_err(HttpParseError::Io)?;

    HttpResponse::parse(&mut stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    /// Starts a local TCP server that reads an HTTP request, validates it,
    /// and responds with a fixed 200 OK response. Returns the port.
    fn start_test_server(
        expected_path: &str,
        expected_host: Option<&str>,
        response_body: &str,
    ) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let expected_path = expected_path.to_string();
        let expected_host = expected_host.map(|s| s.to_string());
        let response_body = response_body.to_string();

        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);

            // Read request line
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();

            // Validate request line contains expected path
            assert!(
                request_line.contains(&expected_path),
                "expected path '{}' in request line '{}'",
                expected_path,
                request_line.trim()
            );

            // Read headers, find Host
            let mut host_value = None;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    break;
                }
                if let Some((name, value)) = trimmed.split_once(':') {
                    if name.trim().eq_ignore_ascii_case("host") {
                        host_value = Some(value.trim().to_string());
                    }
                }
            }

            // Verify Host header value
            let host_value = host_value.expect("Host header missing");
            if let Some(expected) = &expected_host {
                assert_eq!(
                    &host_value, expected,
                    "Host header value mismatch"
                );
            }

            // Send response
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            use std::io::Write;
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        });

        port
    }

    #[test]
    fn send_get_to_local_server() {
        let port = start_test_server("/hello", None, "world");

        let url = format!("http://127.0.0.1:{}/hello", port);
        let req = HttpRequest::get(&url).unwrap();
        let resp = send(&req).unwrap();

        assert_eq!(resp.status_code(), 200);
        assert_eq!(resp.body(), b"world");
    }

    #[test]
    fn send_verifies_host_header_value() {
        // Port is dynamic, so we start the server first, then build the expected host.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected_host = format!("127.0.0.1:{}", port);

        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);

            // Skip request line
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();

            // Read headers, verify Host value
            let mut host_value = None;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    break;
                }
                if let Some((name, value)) = trimmed.split_once(':') {
                    if name.trim().eq_ignore_ascii_case("host") {
                        host_value = Some(value.trim().to_string());
                    }
                }
            }

            assert_eq!(
                host_value.expect("Host header missing"),
                expected_host,
                "Host header value mismatch"
            );

            // Send response
            let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
            use std::io::Write;
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        });

        let url = format!("http://127.0.0.1:{}/", port);
        let req = HttpRequest::get(&url).unwrap();
        let resp = send(&req).unwrap();

        assert_eq!(resp.status_code(), 200);
        assert_eq!(resp.body(), b"ok");
    }

    #[test]
    fn send_rejects_https() {
        let req = HttpRequest::get("https://example.com/").unwrap();
        let err = send(&req);
        assert!(err.is_err());
    }
}
