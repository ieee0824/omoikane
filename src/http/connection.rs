//! TCP/TLS connection handling for HTTP requests.

use std::io::{self, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use rustls::ClientConfig;

use super::http2;
use super::request::HttpRequest;
use super::response::{HttpParseError, HttpResponse};

/// Default timeout in seconds for both connection and read operations.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Sends an HTTP request over a new TCP connection and returns the response.
///
/// For `http` URLs, uses a plain TCP connection. For `https` URLs, wraps the
/// connection with TLS using rustls, with certificate verification against
/// Mozilla's root certificate store and SNI support.
///
/// # Errors
///
/// Returns an error if the connection cannot be established, TLS handshake
/// fails, the request cannot be sent, or the response cannot be parsed.
///
/// # Examples
///
/// ```no_run
/// use omoikane::http::{HttpRequest, send};
///
/// // Plain HTTP
/// let req = HttpRequest::get("http://example.com/").unwrap();
/// let resp = send(&req).unwrap();
/// println!("Status: {}", resp.status_code());
///
/// // HTTPS with TLS
/// let req = HttpRequest::get("https://example.com/").unwrap();
/// let resp = send(&req).unwrap();
/// println!("Status: {}", resp.status_code());
/// ```
pub fn send(request: &HttpRequest) -> Result<HttpResponse, HttpParseError> {
    let url = request.url();
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

    let stream =
        TcpStream::connect_timeout(&socket_addr, Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .map_err(HttpParseError::Io)?;

    stream
        .set_read_timeout(Some(Duration::from_secs(DEFAULT_TIMEOUT_SECS)))
        .map_err(HttpParseError::Io)?;

    if url.scheme() == "https" {
        let config = default_client_config();
        send_over_tls_with_config(stream, request, Arc::new(config))
    } else {
        send_over_tcp(stream, request)
    }
}

fn send_over_tcp(
    mut stream: TcpStream,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpParseError> {
    stream
        .write_all(&request.serialize())
        .map_err(HttpParseError::Io)?;
    stream.flush().map_err(HttpParseError::Io)?;
    HttpResponse::parse(&mut stream)
}

fn default_client_config() -> ClientConfig {
    let root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let mut config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    config
}

fn send_over_tls_with_config(
    stream: TcpStream,
    request: &HttpRequest,
    config: Arc<ClientConfig>,
) -> Result<HttpResponse, HttpParseError> {
    http2::send_over_tls(stream, request, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::ServerName;
    use rustls::{ClientConnection, StreamOwned};
    use std::io::{BufRead, BufReader, Read};
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
                assert_eq!(&host_value, expected, "Host header value mismatch");
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
    fn tls_rejects_invalid_server_name() {
        // IP addresses cannot be used as SNI server names with rustls
        let req = HttpRequest::get("https://127.0.0.1/").unwrap();
        let result = send(&req);
        assert!(result.is_err());
    }

    // --- TLS tests using a local rustls server with rcgen certificates ---

    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

    /// Generates a self-signed certificate for the given hostname using rcgen.
    /// Returns (certificate DER, private key DER).
    fn generate_test_cert(hostname: &str) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
        let mut params = rcgen::CertificateParams::new(vec![hostname.to_string()]).unwrap();
        params.distinguished_name = rcgen::DistinguishedName::new();
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let key_der = key_pair.serialize_der();
        let cert = params.self_signed(&key_pair).unwrap();
        let cert_der = cert.der().clone();
        (
            cert_der,
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
        )
    }

    /// Starts a local TLS server that accepts one connection, reads an HTTP
    /// request, and responds with a fixed 200 OK response. Returns the port
    /// and the CA certificate DER (for client trust).
    fn start_tls_test_server(
        hostname: &str,
        response_body: &str,
    ) -> (u16, CertificateDer<'static>) {
        let (cert_der, key_der) = generate_test_cert(hostname);
        let ca_cert = cert_der.clone();

        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let response_body = response_body.to_string();

        std::thread::spawn(move || {
            let (tcp_stream, _) = listener.accept().unwrap();
            let conn = rustls::ServerConnection::new(Arc::new(server_config)).unwrap();
            let mut tls_stream = StreamOwned::new(conn, tcp_stream);

            // Read request (consume until \r\n\r\n)
            let mut buf = vec![0u8; 4096];
            let _ = tls_stream.read(&mut buf);

            // Send response
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            tls_stream.write_all(response.as_bytes()).unwrap();
            tls_stream.flush().unwrap();
        });

        (port, ca_cert)
    }

    /// Helper: connect to the local TLS server while exercising the same
    /// request/TLS path as production code, but with a test-specific root store.
    fn send_to_local_tls_server_with_config(
        request: &HttpRequest,
        port: u16,
        ca_cert: &CertificateDer<'_>,
    ) -> Result<HttpResponse, HttpParseError> {
        let addr: std::net::SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .map_err(HttpParseError::Io)?;
        stream
            .set_read_timeout(Some(Duration::from_secs(DEFAULT_TIMEOUT_SECS)))
            .map_err(HttpParseError::Io)?;

        let mut root_store = rustls::RootCertStore::empty();
        root_store.add(ca_cert.clone()).unwrap();

        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        send_over_tls_with_config(stream, request, Arc::new(config))
    }

    #[test]
    fn tls_https_success_with_trusted_cert() {
        let (port, ca_cert) = start_tls_test_server("localhost", "tls-ok");

        let url = format!("https://localhost:{}/", port);
        let req = HttpRequest::get(&url).unwrap();
        let resp = send_to_local_tls_server_with_config(&req, port, &ca_cert).unwrap();

        assert_eq!(resp.status_code(), 200);
        assert_eq!(resp.body(), b"tls-ok");
    }

    #[test]
    fn tls_rejects_untrusted_self_signed_cert() {
        // Start a server with a self-signed cert, but connect using the
        // default Mozilla root store — the cert won't be trusted.
        let (port, _ca_cert) = start_tls_test_server("localhost", "should-not-reach");

        // Connect directly to 127.0.0.1 to avoid DNS issues, using default roots.
        let addr: std::net::SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();

        let root_store =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let server_name = ServerName::try_from("localhost".to_string()).unwrap();
        let conn = ClientConnection::new(Arc::new(config), server_name).unwrap();
        let mut tls_stream = StreamOwned::new(conn, stream);

        // The TLS handshake should fail during write (certificate not trusted).
        let req = HttpRequest::get(&format!("https://localhost:{}/", port)).unwrap();
        let result = tls_stream.write_all(&req.serialize());

        assert!(result.is_err(), "should reject untrusted self-signed cert");
    }

    #[test]
    fn tls_rejects_hostname_mismatch() {
        // Certificate is issued for "correct-host.test", but we connect
        // using "localhost" — hostname verification should fail.
        let (port, ca_cert) = start_tls_test_server("correct-host.test", "should-not-reach");

        let url = format!("https://localhost:{}/", port);
        let req = HttpRequest::get(&url).unwrap();
        let result = send_to_local_tls_server_with_config(&req, port, &ca_cert);

        assert!(result.is_err(), "should reject hostname mismatch");
    }
}
