//! TCP/TLS connection handling for HTTP requests.

use std::collections::HashMap;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use rustls::ClientConfig;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

use super::http2;
use super::request::HttpRequest;
use super::response::{HttpParseError, HttpResponse};

/// Default timeout in seconds for both connection and read operations.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Default)]
pub(crate) struct ConnectionPool {
    http2: HashMap<String, http2::Http2Session>,
    http1: HashMap<String, http2::Http1Session>,
}

impl ConnectionPool {
    pub(crate) fn send(
        &mut self,
        request: &HttpRequest,
        insecure: bool,
    ) -> Result<HttpResponse, HttpParseError> {
        self.send_with_timeout(request, insecure, None)
    }

    pub(crate) fn send_with_timeout(
        &mut self,
        request: &HttpRequest,
        insecure: bool,
        timeout: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        if request.url().scheme() != "https" {
            let stream = connect_stream_with_timeout(request, timeout)?;
            return send_over_tcp(stream, request);
        }

        let key = format!(
            "{}|{}|{}",
            request.url().authority(),
            insecure,
            request.requires_public_ip()
        );
        if let Some(session) = self.http2.get_mut(&key) {
            session.set_timeout(timeout)?;
            match session.send_request(request) {
                Ok(response) => {
                    if std::env::var_os("OMOIKANE_LOG_HTTP").is_some() {
                        eprintln!("[omoikane][http2] reused {key}");
                    }
                    return Ok(response);
                }
                Err(error) => {
                    if std::env::var_os("OMOIKANE_LOG_HTTP").is_some() {
                        eprintln!("[omoikane][http2] reuse-failed {key}: {error}");
                    }
                    self.http2.remove(&key);
                }
            }
        }
        if let Some(session) = self.http1.get_mut(&key) {
            session.set_timeout(timeout)?;
            match session.send_request(request) {
                Ok(response) => {
                    if response.header("connection").is_some_and(|value| {
                        value.eq_ignore_ascii_case("close")
                    }) {
                        self.http1.remove(&key);
                    }
                    if std::env::var_os("OMOIKANE_LOG_HTTP").is_some() {
                        eprintln!("[omoikane][http1] reused {key}");
                    }
                    return Ok(response);
                }
                Err(error) => {
                    if std::env::var_os("OMOIKANE_LOG_HTTP").is_some() {
                        eprintln!("[omoikane][http1] reuse-failed {key}: {error}");
                    }
                    self.http1.remove(&key);
                }
            }
        }

        let stream = connect_stream_with_timeout(request, timeout)?;
        let config = if insecure {
            shared_insecure_client_config(true)
        } else {
            shared_default_client_config(true)
        };
        match http2::connect_session(stream, request, config)? {
            Ok(mut session) => {
                let response = session.send_request(request);
                if response.is_ok() {
                    if std::env::var_os("OMOIKANE_LOG_HTTP").is_some() {
                        eprintln!("[omoikane][http2] opened {key}");
                    }
                    self.http2.insert(key.clone(), session);
                }
                match response {
                    Ok(response) => Ok(response),
                    Err(HttpParseError::InvalidHeader) => {
                        self.send_new_http1(request, insecure, key, timeout)
                    }
                    Err(error) => Err(error),
                }
            }
            Err(mut session) => {
                let response = session.send_request(request)?;
                let should_keep = !response
                    .header("connection")
                    .is_some_and(|value| value.eq_ignore_ascii_case("close"));
                if should_keep {
                    if std::env::var_os("OMOIKANE_LOG_HTTP").is_some() {
                        eprintln!("[omoikane][http1] opened {key}");
                    }
                    self.http1.insert(key, session);
                }
                Ok(response)
            }
        }
    }

    fn send_new_http1(
        &mut self,
        request: &HttpRequest,
        insecure: bool,
        key: String,
        timeout: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let stream = connect_stream_with_timeout(request, timeout)?;
        let config = if insecure {
            shared_insecure_client_config(false)
        } else {
            shared_default_client_config(false)
        };
        let mut session = http2::connect_http1_session(stream, request, config)?;
        let response = session.send_request(request)?;
        let should_keep = !response
            .header("connection")
            .is_some_and(|value| value.eq_ignore_ascii_case("close"));
        if should_keep {
            if std::env::var_os("OMOIKANE_LOG_HTTP").is_some() {
                eprintln!("[omoikane][http1] opened {key}");
            }
            self.http1.insert(key, session);
        }
        Ok(response)
    }
}

/// A [`ServerCertVerifier`] that accepts any server certificate without validation.
///
/// **Security warning**: This verifier disables all certificate checks, including
/// expiry, hostname matching, and chain-of-trust. Use only in development or
/// testing environments where you explicitly accept this risk.
#[derive(Debug)]
struct InsecureCertVerifier;

impl ServerCertVerifier for InsecureCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let provider = rustls::crypto::CryptoProvider::get_default()
            .map(|p| p.signature_verification_algorithms)
            .unwrap_or_else(|| {
                rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms
            });
        rustls::crypto::verify_tls12_signature(message, cert, dss, &provider)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let provider = rustls::crypto::CryptoProvider::get_default()
            .map(|p| p.signature_verification_algorithms)
            .unwrap_or_else(|| {
                rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms
            });
        rustls::crypto::verify_tls13_signature(message, cert, dss, &provider)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::CryptoProvider::get_default()
            .map(|p| p.signature_verification_algorithms.supported_schemes())
            .unwrap_or_else(|| {
                rustls::crypto::aws_lc_rs::default_provider()
                    .signature_verification_algorithms
                    .supported_schemes()
            })
    }
}

/// Builds a [`ClientConfig`] that skips all TLS certificate verification.
fn insecure_client_config(enable_http2: bool) -> ClientConfig {
    let mut config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InsecureCertVerifier))
        .with_no_client_auth();
    config.alpn_protocols = if enable_http2 {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    };
    config
}

fn shared_insecure_client_config(enable_http2: bool) -> Arc<ClientConfig> {
    static HTTP2: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    static HTTP1: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    let config = if enable_http2 { &HTTP2 } else { &HTTP1 };
    Arc::clone(config.get_or_init(|| Arc::new(insecure_client_config(enable_http2))))
}

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
    send_with_options(request, false)
}

/// Sends an HTTP request like [`send`], but optionally skips TLS certificate verification.
///
/// When `insecure` is `true`, expired, self-signed, or otherwise invalid certificates
/// are accepted. **Use only in development and testing.**
pub fn send_with_options(
    request: &HttpRequest,
    insecure: bool,
) -> Result<HttpResponse, HttpParseError> {
    let url = request.url();
    if url.scheme() == "https" {
        send_https_with_fallback(
            request,
            || connect_stream(request),
            |enable_http2| {
                if insecure {
                    shared_insecure_client_config(enable_http2)
                } else {
                    shared_default_client_config(enable_http2)
                }
            },
        )
    } else {
        let stream = connect_stream(request)?;
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

fn connect_stream(request: &HttpRequest) -> Result<TcpStream, HttpParseError> {
    connect_stream_with_timeout(request, None)
}

fn connect_stream_with_timeout(
    request: &HttpRequest,
    timeout: Option<Duration>,
) -> Result<TcpStream, HttpParseError> {
    let url = request.url();
    let addr = format!("{}:{}", url.host(), url.port());
    let socket_addrs = addr.to_socket_addrs().map_err(HttpParseError::Io)?;
    let socket_addrs = socket_addrs
        .filter(|address| !request.requires_public_ip() || is_public_ip(address.ip()))
        .collect::<Vec<_>>();
    if socket_addrs.is_empty() {
        return Err(HttpParseError::Io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("no permitted address for: {addr}"),
        )));
    }

    let mut last_error = None;
    for socket_addr in socket_addrs {
        match connect_socket_with_timeout(socket_addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    Err(HttpParseError::Io(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("could not connect: {addr}"),
        )
    })))
}

fn connect_socket_with_timeout(
    socket_addr: SocketAddr,
    timeout: Option<Duration>,
) -> io::Result<TcpStream> {
    let timeout = timeout.unwrap_or(Duration::from_secs(DEFAULT_TIMEOUT_SECS));
    let stream = TcpStream::connect_timeout(&socket_addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(stream)
}

pub(crate) fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => {
            if let Some(ipv4) = ip.to_ipv4_mapped() {
                return is_public_ipv4(ipv4);
            }
            let segments = ip.segments();
            !ip.is_unspecified()
                && !ip.is_loopback()
                && !ip.is_multicast()
                && (segments[0] & 0xfe00) != 0xfc00
                && (segments[0] & 0xffc0) != 0xfe80
                && (segments[0] & 0xffc0) != 0xfec0
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        }
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn default_client_config(enable_http2: bool) -> ClientConfig {
    let root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let mut config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    config.alpn_protocols = if enable_http2 {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    };
    config
}

fn shared_default_client_config(enable_http2: bool) -> Arc<ClientConfig> {
    static HTTP2: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    static HTTP1: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    let config = if enable_http2 { &HTTP2 } else { &HTTP1 };
    Arc::clone(config.get_or_init(|| Arc::new(default_client_config(enable_http2))))
}

fn send_over_tls_with_config(
    stream: TcpStream,
    request: &HttpRequest,
    config: Arc<ClientConfig>,
) -> Result<HttpResponse, HttpParseError> {
    http2::send_over_tls(stream, request, config)
}

fn send_https_with_fallback<Connect, Config>(
    request: &HttpRequest,
    connect: Connect,
    config: Config,
) -> Result<HttpResponse, HttpParseError>
where
    Connect: Fn() -> Result<TcpStream, HttpParseError>,
    Config: Fn(bool) -> Arc<ClientConfig>,
{
    let stream = connect()?;
    match send_over_tls_with_config(stream, request, config(true)) {
        Ok(response) => Ok(response),
        Err(HttpParseError::InvalidHeader) => {
            let stream = connect()?;
            send_over_tls_with_config(stream, request, config(false))
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::ServerName;
    use rustls::{ClientConnection, StreamOwned};
    use std::io::{BufRead, BufReader, Read};
    use std::net::TcpListener;

    #[test]
    fn tls_configs_are_shared_by_transport_mode() {
        let http2 = shared_default_client_config(true);
        let same_http2 = shared_default_client_config(true);
        let http1 = shared_default_client_config(false);

        assert!(Arc::ptr_eq(&http2, &same_http2));
        assert!(!Arc::ptr_eq(&http2, &http1));
        assert_eq!(http2.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
        assert_eq!(http1.alpn_protocols, vec![b"http/1.1".to_vec()]);
    }

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
    ///
    /// The server listens on `[::1]:0` so that `localhost` (which resolves to
    /// `::1` on macOS) can connect via `send_with_options`.
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

        let listener = TcpListener::bind("[::1]:0").unwrap();
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

    #[test]
    fn connection_pool_reuses_http11_tls_connection() {
        let (cert_der, key_der) = generate_test_cert("localhost");
        let listener = TcpListener::bind("[::1]:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .unwrap();
        let mut server_config = Arc::new(server_config);
        Arc::get_mut(&mut server_config).unwrap().alpn_protocols = vec![b"http/1.1".to_vec()];

        let server = std::thread::spawn(move || {
            let (tcp_stream, _) = listener.accept().unwrap();
            let conn = rustls::ServerConnection::new(server_config).unwrap();
            let mut tls_stream = StreamOwned::new(conn, tcp_stream);

            for expected_path in ["/first", "/second"] {
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0u8; 1];
                    tls_stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with(&format!("GET {expected_path} HTTP/1.1\r\n")));
                tls_stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .unwrap();
                tls_stream.flush().unwrap();
            }
        });

        let mut pool = ConnectionPool::default();
        for path in ["first", "second"] {
            let request = HttpRequest::get(&format!("https://localhost:{port}/{path}")).unwrap();
            let response = pool.send(&request, true).unwrap();
            assert_eq!(response.body(), b"ok");
        }
        server.join().unwrap();
    }

    /// Helper: connect to the local TLS server while exercising the same
    /// request/TLS path as production code, but with a test-specific root store.
    fn send_to_local_tls_server_with_config(
        request: &HttpRequest,
        port: u16,
        ca_cert: &CertificateDer<'_>,
    ) -> Result<HttpResponse, HttpParseError> {
        let addr: std::net::SocketAddr = format!("[::1]:{}", port).parse().unwrap();

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

        // Connect directly to [::1] to match the test server's bind address.
        let addr: std::net::SocketAddr = format!("[::1]:{}", port).parse().unwrap();
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

    #[test]
    fn tls_falls_back_to_http11_when_http2_header_decode_fails() {
        let (cert_der, key_der) = generate_test_cert("localhost");
        let listener = TcpListener::bind("[::1]:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der.clone()], key_der)
            .unwrap();
        let mut server_config = Arc::new(server_config);
        Arc::get_mut(&mut server_config).unwrap().alpn_protocols =
            vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        std::thread::spawn(move || {
            // First connection negotiates h2 and returns an unsupported frame type,
            // which the client currently treats as InvalidHeader.
            let (tcp_stream, _) = listener.accept().unwrap();
            let conn = rustls::ServerConnection::new(server_config.clone()).unwrap();
            let mut tls_stream = StreamOwned::new(conn, tcp_stream);

            let mut preface = [0u8; 24];
            tls_stream.read_exact(&mut preface).unwrap();
            let mut frame_header = [0u8; 9];
            tls_stream.read_exact(&mut frame_header).unwrap();
            let settings_len = ((frame_header[0] as usize) << 16)
                | ((frame_header[1] as usize) << 8)
                | frame_header[2] as usize;
            let mut settings_payload = vec![0u8; settings_len];
            tls_stream.read_exact(&mut settings_payload).unwrap();

            let invalid_frame = [
                0u8, 0u8, 0u8,  // payload len
                0xFF, // unknown frame type
                0u8,  // flags
                0u8, 0u8, 0u8, 0u8, // stream id
            ];
            tls_stream.write_all(&invalid_frame).unwrap();
            tls_stream.flush().unwrap();
            drop(tls_stream);

            // Second connection negotiates HTTP/1.1 and returns a valid response.
            let (tcp_stream, _) = listener.accept().unwrap();
            let conn = rustls::ServerConnection::new(server_config).unwrap();
            let mut tls_stream = StreamOwned::new(conn, tcp_stream);

            let mut request_bytes = vec![0u8; 4096];
            let _ = tls_stream.read(&mut request_bytes).unwrap();

            let response = "HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nfallback";
            tls_stream.write_all(response.as_bytes()).unwrap();
            tls_stream.flush().unwrap();
        });

        let addr: std::net::SocketAddr = format!("[::1]:{port}").parse().unwrap();
        let req = HttpRequest::get(&format!("https://localhost:{port}/")).unwrap();
        let mut root_store = rustls::RootCertStore::empty();
        root_store.add(cert_der).unwrap();

        let response = send_https_with_fallback(
            &req,
            || {
                let stream =
                    TcpStream::connect_timeout(&addr, Duration::from_secs(DEFAULT_TIMEOUT_SECS))
                        .map_err(HttpParseError::Io)?;
                stream
                    .set_read_timeout(Some(Duration::from_secs(DEFAULT_TIMEOUT_SECS)))
                    .map_err(HttpParseError::Io)?;
                Ok(stream)
            },
            |enable_http2| {
                let mut config = ClientConfig::builder()
                    .with_root_certificates(root_store.clone())
                    .with_no_client_auth();
                config.alpn_protocols = if enable_http2 {
                    vec![b"h2".to_vec(), b"http/1.1".to_vec()]
                } else {
                    vec![b"http/1.1".to_vec()]
                };
                Arc::new(config)
            },
        )
        .unwrap();

        assert_eq!(response.status_code(), 200);
        assert_eq!(response.body(), b"fallback");
    }

    /// Helper: connect to a TLS server with the insecure verifier (no trusted roots required).
    fn send_insecure_to_local_tls_server(
        request: &HttpRequest,
        _port: u16,
    ) -> Result<HttpResponse, HttpParseError> {
        send_with_options(request, true)
    }

    #[test]
    fn insecure_mode_accepts_self_signed_cert() {
        // Self-signed cert is not in the Mozilla root store, but insecure mode
        // should still succeed.
        let (port, _ca_cert) = start_tls_test_server("localhost", "insecure-ok");

        let url = format!("https://localhost:{}/", port);
        let req = HttpRequest::get(&url).unwrap();
        let resp = send_insecure_to_local_tls_server(&req, port).unwrap();

        assert_eq!(resp.status_code(), 200);
        assert_eq!(resp.body(), b"insecure-ok");
    }

    #[test]
    fn insecure_mode_accepts_hostname_mismatch() {
        // Certificate is for "correct-host.test" but we connect as "localhost".
        // In insecure mode this should be accepted.
        let (port, _ca_cert) = start_tls_test_server("correct-host.test", "mismatch-ok");

        let url = format!("https://localhost:{}/", port);
        let req = HttpRequest::get(&url).unwrap();
        let resp = send_insecure_to_local_tls_server(&req, port).unwrap();

        assert_eq!(resp.status_code(), 200);
        assert_eq!(resp.body(), b"mismatch-ok");
    }

    #[test]
    fn default_mode_still_rejects_untrusted_cert_after_insecure_added() {
        // Ensure the default (secure) path is not affected by the insecure code path.
        let (port, _ca_cert) = start_tls_test_server("localhost", "should-not-reach");

        let addr: std::net::SocketAddr = format!("[::1]:{}", port).parse().unwrap();
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

        let req = HttpRequest::get(&format!("https://localhost:{}/", port)).unwrap();
        let result = tls_stream.write_all(&req.serialize());

        assert!(
            result.is_err(),
            "secure mode should still reject untrusted cert"
        );
    }

    #[test]
    fn public_ip_filter_rejects_site_local_ipv6() {
        assert!(!is_public_ip("fec0::1".parse().unwrap()));
        assert!(!is_public_ip("feff::1".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn restricted_request_rejects_the_resolved_loopback_address() {
        let mut request = HttpRequest::get("http://127.0.0.1:9/").unwrap();
        request.require_public_ip();
        let error = connect_stream(&request).unwrap_err();
        assert!(
            matches!(error, HttpParseError::Io(ref error) if error.kind() == io::ErrorKind::PermissionDenied)
        );
    }
}
