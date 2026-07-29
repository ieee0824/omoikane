//! Client-side realtime transport primitives.
//!
//! The WebSocket client deliberately implements RFC 6455 framing itself so the
//! browser and CDP transports share one wire format. TLS (`wss:`), extensions,
//! and proxies remain outside this core.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use base64::Engine as _;

use crate::cdp::{WebSocketFrame, WebSocketOpcode, websocket_accept_key};

/// A message read from a WebSocket connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebSocketMessage {
    Text(String),
    Binary(Vec<u8>),
    Close { code: u16, reason: String },
}

/// A connected RFC 6455 client using masked client frames.
#[derive(Debug)]
pub struct WebSocketClient {
    stream: TcpStream,
    protocol: String,
    read_buffer: Vec<u8>,
}

impl WebSocketClient {
    /// Connects to a `ws:` URL and validates the server handshake.
    pub fn connect(url: &str, protocols: &[String], origin: Option<&str>) -> Result<Self, String> {
        let http_url = url
            .strip_prefix("ws://")
            .map(|rest| format!("http://{rest}"))
            .ok_or_else(|| "only ws: WebSocket URLs are supported".to_string())?;
        let parsed = http_url
            .parse::<crate::http::Url>()
            .map_err(|error| error.to_string())?;
        let mut stream = TcpStream::connect((parsed.host(), parsed.port()))
            .map_err(|error| error.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|error| error.to_string())?;
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
        request.push_str("\r\n");
        stream
            .write_all(request.as_bytes())
            .map_err(|e| e.to_string())?;

        let mut response = Vec::new();
        let mut byte = [0u8; 1];
        while !response.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).map_err(|e| e.to_string())?;
            response.push(byte[0]);
            if response.len() > 32 * 1024 {
                return Err("WebSocket handshake is too large".into());
            }
        }
        let response = String::from_utf8(response).map_err(|_| "invalid handshake encoding")?;
        let mut lines = response.split("\r\n");
        if !lines
            .next()
            .is_some_and(|line| line.starts_with("HTTP/1.1 101 "))
        {
            return Err("server rejected WebSocket upgrade".into());
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
            return Err("invalid Sec-WebSocket-Accept".into());
        }
        if !protocol.is_empty() && !protocols.iter().any(|candidate| candidate == &protocol) {
            return Err("server selected an unrequested WebSocket protocol".into());
        }
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
    pub fn send(&mut self, payload: Vec<u8>, binary: bool) -> Result<(), String> {
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
    pub fn read_message(&mut self) -> Result<WebSocketMessage, String> {
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
                        return Err("unexpected continuation frame".into());
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
                .map_err(|_| "invalid UTF-8 text frame".into()),
            Some(WebSocketOpcode::Binary) => Ok(WebSocketMessage::Binary(payload)),
            _ => Err("missing WebSocket message opcode".into()),
        }
    }

    /// Starts the close handshake with a masked close frame.
    pub fn close(&mut self, code: u16, reason: &str) -> Result<(), String> {
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
    pub fn try_clone(&self) -> Result<Self, String> {
        Ok(Self {
            stream: self.stream.try_clone().map_err(|error| error.to_string())?,
            protocol: self.protocol.clone(),
            read_buffer: Vec::new(),
        })
    }

    fn write_client_frame(&mut self, frame: &WebSocketFrame) -> Result<(), String> {
        let mut mask = [0u8; 4];
        getrandom::fill(&mut mask).map_err(|error| error.to_string())?;
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
        self.stream
            .write_all(&bytes)
            .map_err(|error| error.to_string())
    }

    fn read_frame(&mut self) -> Result<WebSocketFrame, String> {
        loop {
            if let Some(expected) = server_frame_length(&self.read_buffer)?
                && self.read_buffer.len() >= expected
            {
                match WebSocketFrame::decode(&self.read_buffer) {
                    Ok((frame, consumed)) => {
                        self.read_buffer.drain(..consumed);
                        return Ok(frame);
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
            let mut chunk = [0u8; 4096];
            let count = self.stream.read(&mut chunk).map_err(|e| e.to_string())?;
            if count == 0 {
                return Err("WebSocket connection closed".into());
            }
            self.read_buffer.extend_from_slice(&chunk[..count]);
        }
    }
}

fn server_frame_length(bytes: &[u8]) -> Result<Option<usize>, String> {
    if bytes.len() < 2 {
        return Ok(None);
    }
    if bytes[0] & 0x70 != 0 {
        return Err("WebSocket RSV bits require an extension".into());
    }
    let opcode = bytes[0] & 0x0f;
    if !matches!(opcode, 0 | 1 | 2 | 8 | 9 | 10) {
        return Err("unsupported WebSocket opcode".into());
    }
    if bytes[1] & 0x80 != 0 {
        return Err("server WebSocket frames must not be masked".into());
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
                .map_err(|_| "WebSocket frame is too large")?
        }
        _ => unreachable!(),
    };
    if opcode >= 8 && (bytes[0] & 0x80 == 0 || length > 125) {
        return Err("invalid fragmented control frame".into());
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
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn event_stream_parses_multiline_type_id_and_retry() {
        assert_eq!(
            parse_event_stream("id: 7\nevent: update\ndata: one\ndata: two\nretry: 25\n\n"),
            vec![("update".into(), "one\ntwo".into(), "7".into(), Some(25))]
        );
    }

    #[test]
    fn malformed_websocket_frame_is_rejected_without_waiting_for_more_bytes() {
        assert_eq!(
            server_frame_length(&[0x83, 0]).unwrap_err(),
            "unsupported WebSocket opcode"
        );
        assert_eq!(
            server_frame_length(&[0x89, 126, 0, 126]).unwrap_err(),
            "invalid fragmented control frame"
        );
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let request = String::from_utf8(request).unwrap();
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
        server.join().unwrap();
    }
}
