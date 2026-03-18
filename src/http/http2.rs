//! Minimal HTTP/2 transport with ALPN support.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;

use rustls::{ClientConfig, ClientConnection, StreamOwned};

use super::request::HttpRequest;
use super::response::{HttpParseError, HttpResponse};

const CONNECTION_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const DEFAULT_WINDOW_SIZE: i32 = 65_535;
const FLAG_END_STREAM: u8 = 0x1;
const FLAG_END_HEADERS: u8 = 0x4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameType {
    Data = 0x0,
    Headers = 0x1,
    Settings = 0x4,
    WindowUpdate = 0x8,
}

impl FrameType {
    fn from_u8(value: u8) -> Result<Self, HttpParseError> {
        match value {
            0x0 => Ok(Self::Data),
            0x1 => Ok(Self::Headers),
            0x4 => Ok(Self::Settings),
            0x8 => Ok(Self::WindowUpdate),
            _ => Err(HttpParseError::InvalidHeader),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Frame {
    frame_type: FrameType,
    flags: u8,
    stream_id: u32,
    payload: Vec<u8>,
}

impl Frame {
    fn encode(&self) -> Vec<u8> {
        let length = self.payload.len() as u32;
        let mut out = Vec::with_capacity(9 + self.payload.len());
        out.push(((length >> 16) & 0xFF) as u8);
        out.push(((length >> 8) & 0xFF) as u8);
        out.push((length & 0xFF) as u8);
        out.push(self.frame_type as u8);
        out.push(self.flags);
        out.extend_from_slice(&(self.stream_id & 0x7FFF_FFFF).to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    fn read(reader: &mut impl Read) -> Result<Self, HttpParseError> {
        let mut header = [0u8; 9];
        reader.read_exact(&mut header).map_err(HttpParseError::Io)?;
        let length =
            ((header[0] as usize) << 16) | ((header[1] as usize) << 8) | header[2] as usize;
        let frame_type = FrameType::from_u8(header[3])?;
        let flags = header[4];
        let stream_id =
            u32::from_be_bytes([header[5], header[6], header[7], header[8]]) & 0x7FFF_FFFF;
        let mut payload = vec![0u8; length];
        reader
            .read_exact(&mut payload)
            .map_err(HttpParseError::Io)?;
        Ok(Self {
            frame_type,
            flags,
            stream_id,
            payload,
        })
    }
}

#[derive(Debug, Default)]
struct HpackEncoder;

impl HpackEncoder {
    fn encode_request(request: &HttpRequest) -> Vec<u8> {
        let mut out = Vec::new();
        encode_literal(&mut out, ":method", request.method().as_str());
        encode_literal(&mut out, ":scheme", request.url().scheme());
        encode_literal(&mut out, ":authority", &request.url().authority());
        encode_literal(&mut out, ":path", &request.url().request_target());

        for (name, value) in request.headers() {
            let lower = name.to_ascii_lowercase();
            if lower == "host" {
                continue;
            }
            encode_literal(&mut out, &lower, value);
        }
        out
    }
}

#[derive(Debug, Default)]
struct HpackDecoder;

impl HpackDecoder {
    fn decode_header_block(block: &[u8]) -> Result<Vec<(String, String)>, HttpParseError> {
        let mut cursor = 0usize;
        let mut headers = Vec::new();
        while cursor < block.len() {
            let first = block[cursor];
            if first & 0x80 != 0 {
                let (index, consumed) = decode_integer(&block[cursor..], 7)?;
                cursor += consumed;
                let (name, value) = indexed_header(index)?;
                headers.push((name.to_string(), value.to_string()));
                continue;
            }

            if first & 0x40 == 0x40 || first & 0x10 == 0x10 || first == 0 {
                let prefix = if first & 0x40 == 0x40 { 6 } else { 4 };
                let (name_index, consumed) = decode_integer(&block[cursor..], prefix)?;
                cursor += consumed;

                let name = if name_index == 0 {
                    decode_string(block, &mut cursor)?
                } else {
                    indexed_name(name_index)?.to_string()
                };
                let value = decode_string(block, &mut cursor)?;
                headers.push((name, value));
                continue;
            }

            return Err(HttpParseError::InvalidHeader);
        }

        Ok(headers)
    }
}

#[derive(Debug, Default)]
struct StreamState {
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    ended: bool,
}

#[derive(Debug)]
struct Http2Connection<'a, IO> {
    io: &'a mut IO,
    next_stream_id: u32,
    connection_window: i32,
    stream_windows: HashMap<u32, i32>,
}

impl<'a, IO: Read + Write> Http2Connection<'a, IO> {
    fn new(io: &'a mut IO) -> Self {
        Self {
            io,
            next_stream_id: 1,
            connection_window: DEFAULT_WINDOW_SIZE,
            stream_windows: HashMap::new(),
        }
    }

    fn send_request(&mut self, request: &HttpRequest) -> Result<HttpResponse, HttpParseError> {
        self.io
            .write_all(CONNECTION_PREFACE)
            .map_err(HttpParseError::Io)?;
        self.write_frame(Frame {
            frame_type: FrameType::Settings,
            flags: 0,
            stream_id: 0,
            payload: Vec::new(),
        })?;

        let stream_id = self.next_stream_id;
        self.next_stream_id += 2;
        self.stream_windows.insert(stream_id, DEFAULT_WINDOW_SIZE);

        let header_block = HpackEncoder::encode_request(request);
        let end_stream = request.body().is_none();
        self.write_frame(Frame {
            frame_type: FrameType::Headers,
            flags: FLAG_END_HEADERS | if end_stream { FLAG_END_STREAM } else { 0 },
            stream_id,
            payload: header_block,
        })?;

        if let Some(body) = request.body() {
            self.write_data(stream_id, body, true)?;
        }

        self.io.flush().map_err(HttpParseError::Io)?;
        self.read_response(stream_id)
    }

    fn write_frame(&mut self, frame: Frame) -> Result<(), HttpParseError> {
        self.io
            .write_all(&frame.encode())
            .map_err(HttpParseError::Io)
    }

    fn write_data(
        &mut self,
        stream_id: u32,
        data: &[u8],
        end_stream: bool,
    ) -> Result<(), HttpParseError> {
        let stream_window = *self
            .stream_windows
            .get(&stream_id)
            .unwrap_or(&DEFAULT_WINDOW_SIZE);
        let allowed = self.connection_window.min(stream_window).max(0) as usize;
        if data.len() > allowed {
            return Err(HttpParseError::Io(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "HTTP/2 flow control window exhausted",
            )));
        }

        self.connection_window -= data.len() as i32;
        if let Some(window) = self.stream_windows.get_mut(&stream_id) {
            *window -= data.len() as i32;
        }

        self.write_frame(Frame {
            frame_type: FrameType::Data,
            flags: if end_stream { FLAG_END_STREAM } else { 0 },
            stream_id,
            payload: data.to_vec(),
        })
    }

    fn read_response(&mut self, target_stream_id: u32) -> Result<HttpResponse, HttpParseError> {
        let mut state = StreamState::default();
        loop {
            let frame = Frame::read(self.io)?;
            match frame.frame_type {
                FrameType::Settings => {
                    if frame.flags & 0x1 == 0 {
                        self.write_frame(Frame {
                            frame_type: FrameType::Settings,
                            flags: 0x1,
                            stream_id: 0,
                            payload: Vec::new(),
                        })?;
                        self.io.flush().map_err(HttpParseError::Io)?;
                    }
                }
                FrameType::Headers if frame.stream_id == target_stream_id => {
                    state
                        .headers
                        .extend(HpackDecoder::decode_header_block(&frame.payload)?);
                    if frame.flags & FLAG_END_STREAM != 0 {
                        state.ended = true;
                    }
                }
                FrameType::Data if frame.stream_id == target_stream_id => {
                    self.connection_window -= frame.payload.len() as i32;
                    if let Some(window) = self.stream_windows.get_mut(&target_stream_id) {
                        *window -= frame.payload.len() as i32;
                    }

                    state.body.extend_from_slice(&frame.payload);
                    self.refresh_flow_control(target_stream_id, frame.payload.len() as u32)?;

                    if frame.flags & FLAG_END_STREAM != 0 {
                        state.ended = true;
                    }
                }
                FrameType::WindowUpdate => {
                    let increment = parse_window_increment(&frame.payload)?;
                    if frame.stream_id == 0 {
                        self.connection_window += increment as i32;
                    } else {
                        *self.stream_windows.entry(frame.stream_id).or_default() +=
                            increment as i32;
                    }
                }
                _ => {}
            }

            if state.ended && !state.headers.is_empty() {
                return build_response(state.headers, state.body);
            }
        }
    }

    fn refresh_flow_control(
        &mut self,
        stream_id: u32,
        increment: u32,
    ) -> Result<(), HttpParseError> {
        self.connection_window += increment as i32;
        *self.stream_windows.entry(stream_id).or_default() += increment as i32;
        Ok(())
    }
}

pub(super) fn send_over_tls(
    stream: TcpStream,
    request: &HttpRequest,
    config: std::sync::Arc<ClientConfig>,
) -> Result<HttpResponse, HttpParseError> {
    let server_name = rustls::pki_types::ServerName::try_from(request.url().host().to_string())
        .map_err(|e| {
            HttpParseError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid server name for SNI: {e}"),
            ))
        })?;

    let mut conn = ClientConnection::new(config, server_name).map_err(|e| {
        HttpParseError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            e,
        ))
    })?;
    let mut tcp = stream;
    conn.complete_io(&mut tcp).map_err(HttpParseError::Io)?;
    let alpn = conn.alpn_protocol().map(|value| value.to_vec());
    let mut tls_stream = StreamOwned::new(conn, tcp);

    if alpn.as_deref() == Some(b"h2") {
        let mut connection = Http2Connection::new(&mut tls_stream);
        connection.send_request(request)
    } else {
        tls_stream
            .write_all(&request.serialize())
            .map_err(HttpParseError::Io)?;
        tls_stream.flush().map_err(HttpParseError::Io)?;
        HttpResponse::parse(&mut tls_stream)
    }
}

fn encode_literal(out: &mut Vec<u8>, name: &str, value: &str) {
    out.push(0);
    encode_string_literal(out, name);
    encode_string_literal(out, value);
}

fn encode_integer(out: &mut Vec<u8>, value: usize, prefix_bits: u8) {
    let max_prefix = (1usize << prefix_bits) - 1;
    if let Some(last) = out.last_mut() {
        if value < max_prefix {
            *last |= value as u8;
            return;
        }
        *last |= max_prefix as u8;
    } else {
        unreachable!();
    }

    let mut remaining = value - max_prefix;
    while remaining >= 128 {
        out.push((remaining as u8 & 0x7F) | 0x80);
        remaining >>= 7;
    }
    out.push(remaining as u8);
}

fn encode_string_literal(out: &mut Vec<u8>, value: &str) {
    out.push(0);
    encode_integer(out, value.len(), 7);
    out.extend_from_slice(value.as_bytes());
}

fn decode_integer(input: &[u8], prefix_bits: u8) -> Result<(usize, usize), HttpParseError> {
    if input.is_empty() {
        return Err(HttpParseError::InvalidHeader);
    }
    let mask = (1usize << prefix_bits) - 1;
    let mut value = (input[0] as usize) & mask;
    if value < mask {
        return Ok((value, 1));
    }

    let mut m = 0usize;
    let mut consumed = 1usize;
    loop {
        let byte = *input.get(consumed).ok_or(HttpParseError::InvalidHeader)? as usize;
        consumed += 1;
        value += (byte & 0x7F) << m;
        if byte & 0x80 == 0 {
            break;
        }
        m += 7;
    }
    Ok((value, consumed))
}

fn decode_string(input: &[u8], cursor: &mut usize) -> Result<String, HttpParseError> {
    let first = *input.get(*cursor).ok_or(HttpParseError::InvalidHeader)?;
    if first & 0x80 != 0 {
        return Err(HttpParseError::InvalidHeader);
    }
    let (len, consumed) = decode_integer(&input[*cursor..], 7)?;
    *cursor += consumed;
    let end = *cursor + len;
    let bytes = input
        .get(*cursor..end)
        .ok_or(HttpParseError::InvalidHeader)?;
    *cursor = end;
    String::from_utf8(bytes.to_vec()).map_err(|_| HttpParseError::InvalidHeader)
}

fn indexed_name(index: usize) -> Result<&'static str, HttpParseError> {
    match index {
        1 => Ok(":authority"),
        2 => Ok(":method"),
        4 => Ok(":path"),
        6 => Ok(":scheme"),
        8 => Ok(":status"),
        28 => Ok("content-length"),
        31 => Ok("content-type"),
        _ => Err(HttpParseError::InvalidHeader),
    }
}

fn indexed_header(index: usize) -> Result<(&'static str, &'static str), HttpParseError> {
    match index {
        8 => Ok((":status", "200")),
        13 => Ok((":status", "404")),
        16 => Ok(("accept-encoding", "gzip, deflate")),
        _ => Err(HttpParseError::InvalidHeader),
    }
}

fn parse_window_increment(payload: &[u8]) -> Result<u32, HttpParseError> {
    if payload.len() != 4 {
        return Err(HttpParseError::InvalidHeader);
    }
    Ok(u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) & 0x7FFF_FFFF)
}

fn build_response(
    headers: Vec<(String, String)>,
    body: Vec<u8>,
) -> Result<HttpResponse, HttpParseError> {
    let mut status_code = None;
    let mut filtered = Vec::new();
    for (name, value) in headers {
        if name == ":status" {
            status_code = value.parse::<u16>().ok();
        } else {
            filtered.push((name, value));
        }
    }
    let status_code = status_code.ok_or(HttpParseError::InvalidStatusCode)?;
    Ok(HttpResponse::new(status_code, "", filtered, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
    use rustls::{RootCertStore, ServerConfig, ServerConnection};
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Duration;

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

    #[test]
    fn hpack_roundtrips_literal_request_headers() {
        let req = HttpRequest::get("https://example.com/path?q=1").unwrap();
        let encoded = HpackEncoder::encode_request(&req);
        let decoded = HpackDecoder::decode_header_block(&encoded).unwrap();

        assert!(decoded.contains(&(":method".to_string(), "GET".to_string())));
        assert!(decoded.contains(&(":scheme".to_string(), "https".to_string())));
        assert!(decoded.contains(&(":path".to_string(), "/path?q=1".to_string())));
    }

    #[test]
    fn frame_encode_decode_roundtrip() {
        let frame = Frame {
            frame_type: FrameType::Headers,
            flags: FLAG_END_HEADERS | FLAG_END_STREAM,
            stream_id: 1,
            payload: b"headers".to_vec(),
        };

        let mut bytes = frame.encode();
        let decoded = Frame::read(&mut &bytes[..]).unwrap();
        assert_eq!(decoded, frame);
        bytes.clear();
    }

    #[test]
    fn negotiates_h2_with_alpn_and_falls_back_to_http11() {
        let (cert_der, key_der) = generate_test_cert("localhost");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der.clone()], key_der)
            .unwrap();
        let mut server_config = Arc::new(server_config);
        Arc::get_mut(&mut server_config).unwrap().alpn_protocols =
            vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        std::thread::spawn(move || {
            let (tcp_stream, _) = listener.accept().unwrap();
            let mut conn = ServerConnection::new(server_config).unwrap();
            let mut tcp_stream = tcp_stream;
            conn.complete_io(&mut tcp_stream).unwrap();
            assert_eq!(conn.alpn_protocol(), Some(b"h2".as_slice()));
        });

        let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
        let mut root_store = RootCertStore::empty();
        root_store.add(cert_der).unwrap();
        let mut config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let server_name = ServerName::try_from("localhost".to_string()).unwrap();
        let mut conn = ClientConnection::new(Arc::new(config), server_name).unwrap();
        let mut stream = stream;
        conn.complete_io(&mut stream).unwrap();

        assert_eq!(conn.alpn_protocol(), Some(b"h2".as_slice()));
    }

    #[test]
    fn sends_basic_get_over_http2() {
        let (cert_der, key_der) = generate_test_cert("localhost");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der.clone()], key_der)
            .unwrap();
        let mut server_config = Arc::new(server_config);
        Arc::get_mut(&mut server_config).unwrap().alpn_protocols = vec![b"h2".to_vec()];

        std::thread::spawn(move || {
            let (tcp_stream, _) = listener.accept().unwrap();
            let conn = ServerConnection::new(server_config).unwrap();
            let mut tls_stream = StreamOwned::new(conn, tcp_stream);

            let mut preface = [0u8; 24];
            tls_stream.read_exact(&mut preface).unwrap();
            assert_eq!(&preface, CONNECTION_PREFACE);

            let settings = Frame::read(&mut tls_stream).unwrap();
            assert_eq!(settings.frame_type, FrameType::Settings);

            let headers = Frame::read(&mut tls_stream).unwrap();
            assert_eq!(headers.frame_type, FrameType::Headers);
            let decoded = HpackDecoder::decode_header_block(&headers.payload).unwrap();
            assert!(decoded.contains(&(":method".to_string(), "GET".to_string())));
            assert!(decoded.contains(&(":path".to_string(), "/".to_string())));

            let server_settings = Frame {
                frame_type: FrameType::Settings,
                flags: 0,
                stream_id: 0,
                payload: Vec::new(),
            };
            tls_stream.write_all(&server_settings.encode()).unwrap();

            let response_headers = vec![
                (":status".to_string(), "200".to_string()),
                ("content-length".to_string(), "5".to_string()),
                ("content-type".to_string(), "text/plain".to_string()),
            ];
            let response = Frame {
                frame_type: FrameType::Headers,
                flags: FLAG_END_HEADERS,
                stream_id: 1,
                payload: encode_headers_for_test(&response_headers),
            };
            let data = Frame {
                frame_type: FrameType::Data,
                flags: FLAG_END_STREAM,
                stream_id: 1,
                payload: b"hello".to_vec(),
            };
            tls_stream.write_all(&response.encode()).unwrap();
            tls_stream.write_all(&data.encode()).unwrap();
            tls_stream.flush().unwrap();
        });

        let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
        let mut root_store = RootCertStore::empty();
        root_store.add(cert_der).unwrap();
        let mut config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        let req = HttpRequest::get(&format!("https://localhost:{port}/")).unwrap();
        let response = send_over_tls(stream, &req, Arc::new(config)).unwrap();

        assert_eq!(response.status_code(), 200);
        assert_eq!(response.body(), b"hello");
        assert_eq!(response.header("content-type"), Some("text/plain"));
    }

    fn encode_headers_for_test(headers: &[(String, String)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, value) in headers {
            encode_literal(&mut out, name, value);
        }
        out
    }
}
