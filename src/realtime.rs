//! Client-side realtime transport primitives.
//!
//! The WebSocket client deliberately implements RFC 6455 framing itself so the
//! browser and CDP transports share one wire format. Extensions and proxies
//! remain outside this core.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use rustls::{ClientConnection, StreamOwned};

use crate::cdp::{WebSocketFrame, WebSocketOpcode, websocket_accept_key};
use crate::http::url::UrlParseError;

/// Bounds connection establishment as well as each handshake read and write.
const WEBSOCKET_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors produced by the WebSocket client or its wire protocol.
#[derive(Debug)]
pub enum RealtimeError {
    /// Only `ws:` and `wss:` URLs are supported.
    UnsupportedUrlScheme,
    /// The WebSocket URL could not be parsed.
    Url(UrlParseError),
    /// A socket operation failed.
    Io(std::io::Error),
    /// A requested subprotocol is not an HTTP token, or was requested twice.
    InvalidProtocol,
    /// The `Origin` value cannot be serialized as a single header field.
    InvalidOrigin,
    /// The `Cookie` value cannot be serialized as a single header field.
    InvalidCookie,
    /// The host resolved only to addresses excluded by the address policy.
    NoPermittedAddress,
    /// A client nonce or frame mask could not be generated.
    Random(getrandom::Error),
    /// The server sent more than the allowed handshake header size.
    HandshakeTooLarge,
    /// The handshake response was not UTF-8.
    InvalidHandshakeEncoding(std::string::FromUtf8Error),
    /// The server did not accept the WebSocket upgrade.
    UpgradeRejected,
    /// The server's accept key did not match the request.
    InvalidAccept,
    /// The server selected a protocol that the client did not offer.
    UnrequestedProtocol,
    /// A continuation frame arrived before a message began.
    UnexpectedContinuation,
    /// A text message contained invalid UTF-8.
    InvalidTextFrame(std::string::FromUtf8Error),
    /// A message ended without a text or binary opcode.
    MissingMessageOpcode,
    /// Decoding a complete WebSocket frame failed.
    Frame(crate::cdp::CdpError),
    /// The connection closed while a frame was being read.
    ConnectionClosed,
    /// A server frame sets reserved bits without an extension.
    UnsupportedRsvBits,
    /// A server frame uses an unsupported opcode.
    UnsupportedOpcode,
    /// A server frame is masked, which only clients may do.
    MaskedServerFrame,
    /// A control frame is fragmented or has an oversized payload.
    InvalidControlFrame,
    /// A server frame cannot fit in this process's address space.
    FrameTooLarge,
}

impl std::fmt::Display for RealtimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedUrlScheme => {
                write!(f, "only ws: or wss: WebSocket URLs are supported")
            }
            Self::Url(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::InvalidProtocol => write!(f, "invalid WebSocket subprotocol"),
            Self::InvalidOrigin => write!(f, "invalid WebSocket origin"),
            Self::InvalidCookie => write!(f, "invalid WebSocket cookie header"),
            Self::NoPermittedAddress => {
                write!(f, "no permitted address for WebSocket connection")
            }
            Self::Random(error) => write!(f, "{error}"),
            Self::HandshakeTooLarge => write!(f, "WebSocket handshake is too large"),
            Self::InvalidHandshakeEncoding(_) => write!(f, "invalid handshake encoding"),
            Self::UpgradeRejected => write!(f, "server rejected WebSocket upgrade"),
            Self::InvalidAccept => write!(f, "invalid Sec-WebSocket-Accept"),
            Self::UnrequestedProtocol => {
                write!(f, "server selected an unrequested WebSocket protocol")
            }
            Self::UnexpectedContinuation => write!(f, "unexpected continuation frame"),
            Self::InvalidTextFrame(_) => write!(f, "invalid UTF-8 text frame"),
            Self::MissingMessageOpcode => write!(f, "missing WebSocket message opcode"),
            Self::Frame(error) => write!(f, "{error}"),
            Self::ConnectionClosed => write!(f, "WebSocket connection closed"),
            Self::UnsupportedRsvBits => write!(f, "WebSocket RSV bits require an extension"),
            Self::UnsupportedOpcode => write!(f, "unsupported WebSocket opcode"),
            Self::MaskedServerFrame => write!(f, "server WebSocket frames must not be masked"),
            Self::InvalidControlFrame => write!(f, "invalid fragmented control frame"),
            Self::FrameTooLarge => write!(f, "WebSocket frame is too large"),
        }
    }
}

impl std::error::Error for RealtimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Url(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Random(error) => Some(error),
            Self::InvalidHandshakeEncoding(error) | Self::InvalidTextFrame(error) => Some(error),
            Self::Frame(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for RealtimeError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<UrlParseError> for RealtimeError {
    fn from(error: UrlParseError) -> Self {
        Self::Url(error)
    }
}

/// A message read from a WebSocket connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebSocketMessage {
    Text(String),
    Binary(Vec<u8>),
    Close { code: u16, reason: String },
}

/// Which resolved peer addresses a WebSocket connection may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebSocketAddressPolicy {
    /// Any resolved address, including loopback and private networks.
    Any,
    /// Only addresses accepted by [`crate::http::is_public_ip`].
    PublicOnly,
}

/// Converts a WebSocket URL into the HTTP-equivalent handshake URL.
pub(crate) fn websocket_http_url(url: &str) -> Result<crate::http::Url, RealtimeError> {
    let http_url = if let Some(rest) = url.strip_prefix("ws://") {
        format!("http://{rest}")
    } else if let Some(rest) = url.strip_prefix("wss://") {
        format!("https://{rest}")
    } else {
        return Err(RealtimeError::UnsupportedUrlScheme);
    };
    Ok(http_url.parse::<crate::http::Url>()?)
}

/// Rejects subprotocol lists that are not unique RFC 6455 tokens, so no value
/// can terminate or extend the `Sec-WebSocket-Protocol` field.
fn validate_protocols(protocols: &[String]) -> Result<(), RealtimeError> {
    for (index, protocol) in protocols.iter().enumerate() {
        if !crate::http::is_http_token(protocol) || protocols[..index].contains(protocol) {
            return Err(RealtimeError::InvalidProtocol);
        }
    }
    Ok(())
}

/// Resolves `url` and connects to the first address allowed by `policy`.
fn connect_stream(
    url: &crate::http::Url,
    policy: WebSocketAddressPolicy,
) -> Result<TcpStream, RealtimeError> {
    let addresses: Vec<SocketAddr> = (url.host(), url.port())
        .to_socket_addrs()?
        .filter(|address| {
            policy == WebSocketAddressPolicy::Any || crate::http::is_public_ip(address.ip())
        })
        .collect();
    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, WEBSOCKET_IO_TIMEOUT) {
            Ok(stream) => {
                stream.set_read_timeout(Some(WEBSOCKET_IO_TIMEOUT))?;
                stream.set_write_timeout(Some(WEBSOCKET_IO_TIMEOUT))?;
                return Ok(stream);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.map_or(RealtimeError::NoPermittedAddress, RealtimeError::Io))
}

fn connect_websocket_stream(
    url: &crate::http::Url,
    policy: WebSocketAddressPolicy,
    endpoint: Option<std::net::SocketAddr>,
    tls_config: Option<Arc<rustls::ClientConfig>>,
) -> Result<WebSocketStream, RealtimeError> {
    let mut tcp = if let Some(endpoint) = endpoint {
        let stream = TcpStream::connect_timeout(&endpoint, WEBSOCKET_IO_TIMEOUT)?;
        stream.set_read_timeout(Some(WEBSOCKET_IO_TIMEOUT))?;
        stream.set_write_timeout(Some(WEBSOCKET_IO_TIMEOUT))?;
        stream
    } else {
        connect_stream(url, policy)?
    };
    if url.scheme() != "https" {
        return Ok(WebSocketStream::Plain(tcp));
    }
    let server_name =
        rustls::pki_types::ServerName::try_from(url.host().trim_matches(['[', ']']).to_string())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut tls = ClientConnection::new(
        tls_config.unwrap_or_else(crate::http::websocket_tls_config),
        server_name,
    )
    .map_err(io::Error::other)?;
    tls.complete_io(&mut tcp)?;
    Ok(WebSocketStream::Tls {
        shared: Arc::new(Mutex::new(StreamOwned::new(tls, tcp))),
        retry_read_timeout: false,
    })
}

enum WebSocketStream {
    Plain(TcpStream),
    Tls {
        shared: Arc<Mutex<StreamOwned<ClientConnection, TcpStream>>>,
        retry_read_timeout: bool,
    },
}

impl std::fmt::Debug for WebSocketStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plain(stream) => f.debug_tuple("Plain").field(stream).finish(),
            Self::Tls { .. } => f.write_str("Tls(..)"),
        }
    }
}

impl WebSocketStream {
    fn try_clone(&self) -> io::Result<Self> {
        match self {
            Self::Plain(stream) => Ok(Self::Plain(stream.try_clone()?)),
            Self::Tls {
                shared,
                retry_read_timeout,
            } => Ok(Self::Tls {
                shared: Arc::clone(shared),
                retry_read_timeout: *retry_read_timeout,
            }),
        }
    }

    fn enable_tls_read_poll(&mut self) -> io::Result<()> {
        if let Self::Tls {
            shared,
            retry_read_timeout,
        } = self
        {
            shared
                .lock()
                .map_err(|_| io::Error::other("WebSocket TLS state poisoned"))?
                .sock
                .set_read_timeout(Some(Duration::from_millis(20)))?;
            *retry_read_timeout = true;
        }
        Ok(())
    }
}

impl Read for WebSocketStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buf),
            Self::Tls {
                shared,
                retry_read_timeout,
            } => loop {
                let result = shared
                    .lock()
                    .map_err(|_| io::Error::other("WebSocket TLS state poisoned"))?
                    .read(buf);
                match result {
                    Err(error)
                        if *retry_read_timeout
                            && matches!(
                                error.kind(),
                                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                            ) =>
                    {
                        std::thread::yield_now();
                    }
                    other => return other,
                }
            },
        }
    }
}

impl Write for WebSocketStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buf),
            Self::Tls { shared, .. } => shared
                .lock()
                .map_err(|_| io::Error::other("WebSocket TLS state poisoned"))?
                .write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls { shared, .. } => shared
                .lock()
                .map_err(|_| io::Error::other("WebSocket TLS state poisoned"))?
                .flush(),
        }
    }
}

/// A connected RFC 6455 client using masked client frames.
#[derive(Debug)]
pub struct WebSocketClient {
    stream: WebSocketStream,
    protocol: String,
    read_buffer: Vec<u8>,
}

impl WebSocketClient {
    /// Connects to a `ws:` URL and validates the server handshake.
    ///
    /// Each subprotocol must be a unique HTTP token and `origin` must be a
    /// valid header value; otherwise no connection is attempted. Any resolved
    /// address may be used, and connecting, reading, and writing are bounded
    /// by a fixed timeout.
    pub fn connect(
        url: &str,
        protocols: &[String],
        origin: Option<&str>,
    ) -> Result<Self, RealtimeError> {
        Self::connect_with_policy(url, protocols, origin, WebSocketAddressPolicy::Any)
    }

    /// Like [`Self::connect`], but only connects to addresses allowed by `policy`.
    pub(crate) fn connect_with_policy(
        url: &str,
        protocols: &[String],
        origin: Option<&str>,
        policy: WebSocketAddressPolicy,
    ) -> Result<Self, RealtimeError> {
        Self::connect_with_cookie_policy(url, protocols, origin, None, policy)
    }

    /// Connects with a cookie selected for the initiating Document or Worker.
    pub(crate) fn connect_with_cookie_policy(
        url: &str,
        protocols: &[String],
        origin: Option<&str>,
        cookie: Option<&str>,
        policy: WebSocketAddressPolicy,
    ) -> Result<Self, RealtimeError> {
        Self::connect_with_tls_config(url, protocols, origin, cookie, policy, None)
    }

    /// Uses a caller-supplied trust configuration for a WebSocket TLS fixture.
    pub(crate) fn connect_with_tls_config(
        url: &str,
        protocols: &[String],
        origin: Option<&str>,
        cookie: Option<&str>,
        policy: WebSocketAddressPolicy,
        tls_config: Option<Arc<rustls::ClientConfig>>,
    ) -> Result<Self, RealtimeError> {
        Self::connect_with_route(url, protocols, origin, cookie, policy, None, tls_config)
    }

    /// Dials an endpoint selected by an explicit transport while preserving
    /// the URL host for the HTTP Host field and verified TLS server name.
    pub(crate) fn connect_with_route(
        url: &str,
        protocols: &[String],
        origin: Option<&str>,
        cookie: Option<&str>,
        policy: WebSocketAddressPolicy,
        endpoint: Option<std::net::SocketAddr>,
        tls_config: Option<Arc<rustls::ClientConfig>>,
    ) -> Result<Self, RealtimeError> {
        let parsed = websocket_http_url(url)?;
        validate_protocols(protocols)?;
        if origin.is_some_and(|origin| !crate::http::is_valid_header("Origin", origin)) {
            return Err(RealtimeError::InvalidOrigin);
        }
        if cookie.is_some_and(|cookie| !crate::http::is_valid_header("Cookie", cookie)) {
            return Err(RealtimeError::InvalidCookie);
        }
        let mut stream = connect_websocket_stream(&parsed, policy, endpoint, tls_config)?;
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(RealtimeError::Random)?;
        let key = base64::engine::general_purpose::STANDARD.encode(nonce);
        let mut request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n",
            parsed.request_target(),
            parsed.authority(),
            key
        );
        if !protocols.is_empty() {
            request.push_str(&format!(
                "Sec-WebSocket-Protocol: {}\r\n",
                protocols.join(", ")
            ));
        }
        if let Some(origin) = origin {
            request.push_str(&format!("Origin: {origin}\r\n"));
        }
        if let Some(cookie) = cookie {
            request.push_str(&format!("Cookie: {cookie}\r\n"));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes())?;
        stream.flush()?;

        let mut response = Vec::new();
        let mut byte = [0u8; 1];
        while !response.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte)?;
            response.push(byte[0]);
            if response.len() > 32 * 1024 {
                return Err(RealtimeError::HandshakeTooLarge);
            }
        }
        let response =
            String::from_utf8(response).map_err(RealtimeError::InvalidHandshakeEncoding)?;
        let mut lines = response.split("\r\n");
        if !lines
            .next()
            .is_some_and(|line| line.starts_with("HTTP/1.1 101 "))
        {
            return Err(RealtimeError::UpgradeRejected);
        }
        let mut accept = None;
        let mut protocol = String::new();
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("sec-websocket-accept") {
                    accept = Some(value.trim());
                }
                if name.eq_ignore_ascii_case("sec-websocket-protocol") {
                    protocol = value.trim().to_string();
                }
            }
        }
        let expected_accept = websocket_accept_key(&key);
        if accept != Some(expected_accept.as_str()) {
            return Err(RealtimeError::InvalidAccept);
        }
        if !protocol.is_empty() && !protocols.iter().any(|candidate| candidate == &protocol) {
            return Err(RealtimeError::UnrequestedProtocol);
        }
        stream.enable_tls_read_poll()?;
        Ok(Self {
            stream,
            protocol,
            read_buffer: Vec::new(),
        })
    }

    /// Returns the negotiated subprotocol, or an empty string.
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    /// Sends one text or binary message as a masked client frame.
    pub fn send(&mut self, payload: Vec<u8>, binary: bool) -> Result<(), RealtimeError> {
        let frame = WebSocketFrame {
            fin: true,
            opcode: if binary {
                WebSocketOpcode::Binary
            } else {
                WebSocketOpcode::Text
            },
            payload,
        };
        self.write_client_frame(&frame)
    }

    /// Reads one complete message, joining continuation frames and answering ping.
    pub fn read_message(&mut self) -> Result<WebSocketMessage, RealtimeError> {
        let mut opcode = None;
        let mut payload = Vec::new();
        loop {
            let frame = self.read_frame()?;
            match frame.opcode {
                WebSocketOpcode::Ping => {
                    let pong = WebSocketFrame {
                        fin: true,
                        opcode: WebSocketOpcode::Pong,
                        payload: frame.payload,
                    };
                    self.write_client_frame(&pong)?;
                }
                WebSocketOpcode::Pong => {}
                WebSocketOpcode::Close => {
                    let code = frame
                        .payload
                        .get(..2)
                        .map(|v| u16::from_be_bytes([v[0], v[1]]))
                        .unwrap_or(1005);
                    let reason =
                        String::from_utf8_lossy(frame.payload.get(2..).unwrap_or_default())
                            .into_owned();
                    return Ok(WebSocketMessage::Close { code, reason });
                }
                WebSocketOpcode::Text | WebSocketOpcode::Binary => {
                    opcode = Some(frame.opcode);
                    payload.extend(frame.payload);
                    if frame.fin {
                        break;
                    }
                }
                WebSocketOpcode::Continuation => {
                    if opcode.is_none() {
                        return Err(RealtimeError::UnexpectedContinuation);
                    }
                    payload.extend(frame.payload);
                    if frame.fin {
                        break;
                    }
                }
            }
        }
        match opcode {
            Some(WebSocketOpcode::Text) => String::from_utf8(payload)
                .map(WebSocketMessage::Text)
                .map_err(RealtimeError::InvalidTextFrame),
            Some(WebSocketOpcode::Binary) => Ok(WebSocketMessage::Binary(payload)),
            _ => Err(RealtimeError::MissingMessageOpcode),
        }
    }

    /// Starts the close handshake with a masked close frame.
    pub fn close(&mut self, code: u16, reason: &str) -> Result<(), RealtimeError> {
        let mut payload = code.to_be_bytes().to_vec();
        payload.extend_from_slice(reason.as_bytes());
        let frame = WebSocketFrame {
            fin: true,
            opcode: WebSocketOpcode::Close,
            payload,
        };
        self.write_client_frame(&frame)
    }

    /// Clones the underlying socket for an independent background reader.
    pub fn try_clone(&self) -> Result<Self, RealtimeError> {
        Ok(Self {
            stream: self.stream.try_clone()?,
            protocol: self.protocol.clone(),
            read_buffer: Vec::new(),
        })
    }

    fn write_client_frame(&mut self, frame: &WebSocketFrame) -> Result<(), RealtimeError> {
        let mut mask = [0u8; 4];
        getrandom::fill(&mut mask).map_err(RealtimeError::Random)?;
        let payload_len = frame.payload.len();
        let mut bytes = vec![if frame.fin { 0x80 } else { 0 } | frame.opcode.as_u8()];
        match payload_len {
            0..=125 => bytes.push(0x80 | payload_len as u8),
            126..=65535 => {
                bytes.push(0x80 | 126);
                bytes.extend_from_slice(&(payload_len as u16).to_be_bytes());
            }
            _ => {
                bytes.push(0x80 | 127);
                bytes.extend_from_slice(&(payload_len as u64).to_be_bytes());
            }
        }
        bytes.extend_from_slice(&mask);
        bytes.extend(
            frame
                .payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % 4]),
        );
        self.stream.write_all(&bytes)?;
        Ok(())
    }

    fn read_frame(&mut self) -> Result<WebSocketFrame, RealtimeError> {
        loop {
            if let Some(expected) = server_frame_length(&self.read_buffer)?
                && self.read_buffer.len() >= expected
            {
                match WebSocketFrame::decode(&self.read_buffer) {
                    Ok((frame, consumed)) => {
                        self.read_buffer.drain(..consumed);
                        return Ok(frame);
                    }
                    Err(error) => return Err(RealtimeError::Frame(error)),
                }
            }
            let mut chunk = [0u8; 4096];
            let count = self.stream.read(&mut chunk)?;
            if count == 0 {
                return Err(RealtimeError::ConnectionClosed);
            }
            self.read_buffer.extend_from_slice(&chunk[..count]);
        }
    }
}

fn server_frame_length(bytes: &[u8]) -> Result<Option<usize>, RealtimeError> {
    if bytes.len() < 2 {
        return Ok(None);
    }
    if bytes[0] & 0x70 != 0 {
        return Err(RealtimeError::UnsupportedRsvBits);
    }
    let opcode = bytes[0] & 0x0f;
    if !matches!(opcode, 0 | 1 | 2 | 8 | 9 | 10) {
        return Err(RealtimeError::UnsupportedOpcode);
    }
    if bytes[1] & 0x80 != 0 {
        return Err(RealtimeError::MaskedServerFrame);
    }
    let mut cursor = 2;
    let length = match bytes[1] & 0x7f {
        value @ 0..=125 => value as usize,
        126 => {
            if bytes.len() < 4 {
                return Ok(None);
            }
            cursor = 4;
            u16::from_be_bytes([bytes[2], bytes[3]]) as usize
        }
        127 => {
            if bytes.len() < 10 {
                return Ok(None);
            }
            cursor = 10;
            let mut raw = [0; 8];
            raw.copy_from_slice(&bytes[2..10]);
            u64::from_be_bytes(raw)
                .try_into()
                .map_err(|_| RealtimeError::FrameTooLarge)?
        }
        _ => unreachable!(),
    };
    if opcode >= 8 && (bytes[0] & 0x80 == 0 || length > 125) {
        return Err(RealtimeError::InvalidControlFrame);
    }
    Ok(Some(cursor + length))
}

/// Parses one complete Server-Sent Events response body.
pub fn parse_event_stream(input: &str) -> Vec<(String, String, String, Option<u64>)> {
    let mut events = Vec::new();
    let mut data = Vec::new();
    let mut event_type = String::new();
    let mut last_id = String::new();
    let mut retry = None;
    for line in input
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .chain(std::iter::once(""))
    {
        if line.is_empty() {
            if !data.is_empty() {
                events.push((
                    if event_type.is_empty() {
                        "message".into()
                    } else {
                        event_type.clone()
                    },
                    data.join("\n"),
                    last_id.clone(),
                    retry,
                ));
            }
            data.clear();
            event_type.clear();
            retry = None;
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line
            .split_once(':')
            .map(|(f, v)| (f, v.strip_prefix(' ').unwrap_or(v)))
            .unwrap_or((line, ""));
        match field {
            "data" => data.push(value.to_string()),
            "event" => event_type = value.to_string(),
            "id" if !value.contains('\0') => last_id = value.to_string(),
            "retry" => retry = value.parse().ok(),
            _ => {}
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers, read_request_headers_from,
    };
    use std::net::TcpListener;

    #[test]
    fn event_stream_parses_multiline_type_id_and_retry() {
        assert_eq!(
            parse_event_stream("id: 7\nevent: update\ndata: one\ndata: two\nretry: 25\n\n"),
            vec![("update".into(), "one\ntwo".into(), "7".into(), Some(25))]
        );
    }

    #[test]
    fn malformed_websocket_frame_is_rejected_without_waiting_for_more_bytes() {
        let invalid_opcode = server_frame_length(&[0x83, 0]).unwrap_err();
        assert!(matches!(invalid_opcode, RealtimeError::UnsupportedOpcode));
        assert_eq!(invalid_opcode.to_string(), "unsupported WebSocket opcode");
        let invalid_control = server_frame_length(&[0x89, 126, 0, 126]).unwrap_err();
        assert!(matches!(
            invalid_control,
            RealtimeError::InvalidControlFrame
        ));
        assert_eq!(
            invalid_control.to_string(),
            "invalid fragmented control frame"
        );
    }

    #[test]
    fn typed_realtime_errors_keep_visible_messages_and_sources() {
        let unsupported = WebSocketClient::connect("ftp://example.test", &[], None).unwrap_err();
        assert!(matches!(unsupported, RealtimeError::UnsupportedUrlScheme));
        assert_eq!(
            unsupported.to_string(),
            "only ws: or wss: WebSocket URLs are supported"
        );

        let invalid_url = WebSocketClient::connect("ws://", &[], None).unwrap_err();
        assert!(matches!(invalid_url, RealtimeError::Url(_)));
        assert_eq!(invalid_url.to_string(), "empty host in URL");
        assert!(std::error::Error::source(&invalid_url).is_some());

        let invalid_text =
            RealtimeError::InvalidTextFrame(String::from_utf8(vec![0xff]).unwrap_err());
        assert_eq!(invalid_text.to_string(), "invalid UTF-8 text frame");
        assert!(std::error::Error::source(&invalid_text).is_some());
    }

    /// Binds a listener that must never see a connection during the test.
    fn untouched_listener() -> (TcpListener, String) {
        let listener = bind_loopback().unwrap();
        let url = format!("ws://{}/socket", listener.local_addr().unwrap());
        (listener, url)
    }

    fn assert_no_connection(listener: &TcpListener) {
        let error = listener.accept().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn invalid_subprotocols_are_rejected_before_connecting() {
        let (listener, url) = untouched_listener();
        for protocols in [
            vec!["chat\r\nX-Injected: 1".to_string()],
            vec!["chat\nX-Injected: 1".to_string()],
            vec!["two words".to_string()],
            vec!["a,b".to_string()],
            vec![String::new()],
            vec!["chat".to_string(), "chat".to_string()],
        ] {
            let error = WebSocketClient::connect(&url, &protocols, None).unwrap_err();
            assert!(
                matches!(error, RealtimeError::InvalidProtocol),
                "{protocols:?}: {error}"
            );
        }
        assert_no_connection(&listener);
    }

    #[test]
    fn invalid_origin_is_rejected_before_connecting() {
        let (listener, url) = untouched_listener();
        let error = WebSocketClient::connect(&url, &[], Some("http://a.test\r\nX-Injected: 1"))
            .unwrap_err();
        assert!(matches!(error, RealtimeError::InvalidOrigin));
        assert_no_connection(&listener);
    }

    #[test]
    fn invalid_cookie_is_rejected_before_connecting() {
        let (listener, url) = untouched_listener();
        let error = WebSocketClient::connect_with_cookie_policy(
            &url,
            &[],
            Some("http://example.test"),
            Some("session=value\r\nX-Injected: 1"),
            WebSocketAddressPolicy::Any,
        )
        .unwrap_err();
        assert!(matches!(error, RealtimeError::InvalidCookie));
        assert_no_connection(&listener);
    }

    #[test]
    fn wss_uses_one_verified_tls_session_for_concurrent_read_and_write() {
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
        use rustls::{RootCertStore, ServerConfig, ServerConnection};

        let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let certificate = params.self_signed(&key_pair).unwrap().der().clone();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
        let server_config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![certificate.clone()], key)
                .unwrap(),
        );
        let mut roots = RootCertStore::empty();
        roots.add(certificate).unwrap();
        let client_config = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let listener = bind_loopback().unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = FixtureWorker::spawn(move || {
            let tcp = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            tcp.set_read_timeout(Some(READ_TIMEOUT)).unwrap();
            tcp.set_write_timeout(Some(READ_TIMEOUT)).unwrap();
            let mut stream = StreamOwned::new(ServerConnection::new(server_config).unwrap(), tcp);
            let request =
                read_request_headers_from(&mut stream, READ_TIMEOUT, |stream, remaining| {
                    stream.sock.set_read_timeout(Some(remaining))
                })
                .unwrap();
            let key = request
                .lines()
                .find_map(|line| line.strip_prefix("Sec-WebSocket-Key: "))
                .unwrap();
            write!(stream, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", websocket_accept_key(key)).unwrap();
            stream.flush().unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
                if let Ok((frame, consumed)) = WebSocketFrame::decode(&bytes)
                    && consumed == bytes.len()
                {
                    assert_eq!(frame.payload, b"ping");
                    break;
                }
            }
            stream
                .write_all(&WebSocketFrame::text("pong").encode(false))
                .unwrap();
            stream.flush().unwrap();
            request
        });

        let mut client = WebSocketClient::connect_with_tls_config(
            &format!("wss://localhost:{port}/socket"),
            &[],
            Some("https://localhost"),
            Some("session=secure"),
            WebSocketAddressPolicy::Any,
            Some(client_config),
        )
        .unwrap();
        let mut reader = client.try_clone().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || sender.send(reader.read_message()).unwrap());
        client.send(b"ping".to_vec(), false).unwrap();
        assert!(matches!(
            receiver.recv_timeout(READ_TIMEOUT).unwrap().unwrap(),
            WebSocketMessage::Text(text) if text == "pong"
        ));
        let request = server.join();
        assert!(request.contains("Origin: https://localhost\r\n"));
        assert!(request.contains("Cookie: session=secure\r\n"));
    }

    #[test]
    fn wss_rejects_untrusted_and_wrong_host_certificates() {
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
        use rustls::{RootCertStore, ServerConfig, ServerConnection};

        let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let certificate = params.self_signed(&key_pair).unwrap().der().clone();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
        let server_config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![certificate.clone()], key)
                .unwrap(),
        );
        let mut roots = RootCertStore::empty();
        roots.add(certificate).unwrap();
        let trusted = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let listener = bind_loopback().unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = FixtureWorker::spawn(move || {
            for _ in 0..2 {
                let mut tcp = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
                tcp.set_read_timeout(Some(READ_TIMEOUT)).unwrap();
                let mut tls = ServerConnection::new(Arc::clone(&server_config)).unwrap();
                let _ = tls.complete_io(&mut tcp);
            }
        });

        let untrusted = WebSocketClient::connect_with_tls_config(
            &format!("wss://localhost:{port}/socket"),
            &[],
            None,
            None,
            WebSocketAddressPolicy::Any,
            None,
        )
        .unwrap_err();
        assert!(matches!(untrusted, RealtimeError::Io(_)));
        let wrong_host = WebSocketClient::connect_with_tls_config(
            &format!("wss://127.0.0.1:{port}/socket"),
            &[],
            None,
            None,
            WebSocketAddressPolicy::Any,
            Some(trusted),
        )
        .unwrap_err();
        assert!(matches!(wrong_host, RealtimeError::Io(_)));
        server.join();
    }

    #[test]
    fn public_only_policy_rejects_loopback_before_connecting() {
        let (listener, url) = untouched_listener();
        let error = WebSocketClient::connect_with_policy(
            &url,
            &[],
            None,
            WebSocketAddressPolicy::PublicOnly,
        )
        .unwrap_err();
        assert!(matches!(error, RealtimeError::NoPermittedAddress));
        assert_eq!(
            error.to_string(),
            "no permitted address for WebSocket connection"
        );
        assert_no_connection(&listener);
    }

    fn read_frame(stream: &mut TcpStream) -> (WebSocketFrame, Vec<u8>) {
        let mut bytes = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            stream.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
            if let Ok((frame, consumed)) = WebSocketFrame::decode(&bytes)
                && consumed == bytes.len()
            {
                return (frame, bytes);
            }
        }
    }

    #[test]
    fn websocket_handshake_masking_fragment_ping_and_close_round_trip() {
        let listener = bind_loopback().unwrap();
        let address = listener.local_addr().unwrap();
        let server = FixtureWorker::spawn(move || {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let key = request
                .lines()
                .find_map(|line| line.strip_prefix("Sec-WebSocket-Key: "))
                .unwrap();
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(key)
                    .unwrap()
                    .len(),
                16
            );
            assert!(request.contains("Sec-WebSocket-Protocol: chat"));
            assert!(request.contains("Origin: http://example.test"));
            write!(stream, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\nSec-WebSocket-Protocol: chat\r\n\r\n", websocket_accept_key(key)).unwrap();

            let (sent, sent_wire) = read_frame(&mut stream);
            assert_ne!(sent_wire[1] & 0x80, 0, "client frames must be masked");
            assert_eq!(sent, WebSocketFrame::text("hello"));
            stream
                .write_all(
                    &WebSocketFrame {
                        fin: false,
                        opcode: WebSocketOpcode::Text,
                        payload: b"hel".to_vec(),
                    }
                    .encode(false),
                )
                .unwrap();
            stream
                .write_all(
                    &WebSocketFrame {
                        fin: true,
                        opcode: WebSocketOpcode::Ping,
                        payload: b"?".to_vec(),
                    }
                    .encode(false),
                )
                .unwrap();
            stream
                .write_all(
                    &WebSocketFrame {
                        fin: true,
                        opcode: WebSocketOpcode::Continuation,
                        payload: b"lo".to_vec(),
                    }
                    .encode(false),
                )
                .unwrap();
            let (pong, wire) = read_frame(&mut stream);
            assert_ne!(wire[1] & 0x80, 0);
            assert_eq!(pong.opcode, WebSocketOpcode::Pong);
            assert_eq!(pong.payload, b"?");
            assert_ne!(
                &sent_wire[2..6],
                &wire[2..6],
                "every frame needs a fresh mask"
            );
            let (close, _) = read_frame(&mut stream);
            assert_eq!(close.opcode, WebSocketOpcode::Close);
            assert_eq!(&close.payload[..2], &1000u16.to_be_bytes());
            stream.write_all(&close.encode(false)).unwrap();
        });

        let mut client = WebSocketClient::connect(
            &format!("ws://{address}/echo"),
            &["chat".into()],
            Some("http://example.test"),
        )
        .unwrap();
        assert_eq!(client.protocol(), "chat");
        client.send(b"hello".to_vec(), false).unwrap();
        assert_eq!(
            client.read_message().unwrap(),
            WebSocketMessage::Text("hello".into())
        );
        client.close(1000, "done").unwrap();
        assert_eq!(
            client.read_message().unwrap(),
            WebSocketMessage::Close {
                code: 1000,
                reason: "done".into()
            }
        );
        server.join();
    }
}
