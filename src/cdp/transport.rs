//! WebSocket transport for the CDP endpoint: the HTTP upgrade handshake and
//! RFC 6455 frame encoding and decoding.

use super::*;

const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Maximum payload accepted in a CDP WebSocket frame (16 MiB).
const MAX_WEBSOCKET_PAYLOAD: usize = 16 * 1024 * 1024;

/// A parsed HTTP upgrade request for the WebSocket handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSocketUpgradeRequest {
    pub path: String,
    pub websocket_key: String,
}

/// Successful handshake result for a new WebSocket client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSocketUpgrade {
    pub client_id: u64,
    pub response: String,
}

/// WebSocket opcodes used by the CDP transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSocketOpcode {
    Continuation,
    Text,
    Binary,
    Close,
    Ping,
    Pong,
}

impl WebSocketOpcode {
    fn from_u8(value: u8) -> Result<Self, CdpError> {
        match value {
            0x0 => Ok(Self::Continuation),
            0x1 => Ok(Self::Text),
            0x2 => Ok(Self::Binary),
            0x8 => Ok(Self::Close),
            0x9 => Ok(Self::Ping),
            0xA => Ok(Self::Pong),
            _ => Err(CdpError::InvalidWebSocketFrame("unsupported opcode")),
        }
    }

    pub(crate) fn as_u8(self) -> u8 {
        match self {
            Self::Continuation => 0x0,
            Self::Text => 0x1,
            Self::Binary => 0x2,
            Self::Close => 0x8,
            Self::Ping => 0x9,
            Self::Pong => 0xA,
        }
    }
}

/// A single WebSocket frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSocketFrame {
    pub fin: bool,
    pub opcode: WebSocketOpcode,
    pub payload: Vec<u8>,
}

impl WebSocketFrame {
    /// Creates a text frame.
    pub fn text(payload: impl Into<String>) -> Self {
        Self {
            fin: true,
            opcode: WebSocketOpcode::Text,
            payload: payload.into().into_bytes(),
        }
    }

    /// Creates a pong frame.
    pub fn pong(payload: Vec<u8>) -> Self {
        Self {
            fin: true,
            opcode: WebSocketOpcode::Pong,
            payload,
        }
    }

    /// Encodes the frame to bytes. Client frames should be masked.
    pub fn encode(&self, masked: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        let first = if self.fin { 0x80 } else { 0 } | self.opcode.as_u8();
        bytes.push(first);

        let payload_len = self.payload.len();
        let mut second = if masked { 0x80 } else { 0 };
        match payload_len {
            0..=125 => {
                second |= payload_len as u8;
                bytes.push(second);
            }
            126..=65535 => {
                second |= 126;
                bytes.push(second);
                bytes.extend_from_slice(&(payload_len as u16).to_be_bytes());
            }
            _ => {
                second |= 127;
                bytes.push(second);
                bytes.extend_from_slice(&(payload_len as u64).to_be_bytes());
            }
        }

        if masked {
            let mask = [0x12, 0x34, 0x56, 0x78];
            bytes.extend_from_slice(&mask);
            for (index, byte) in self.payload.iter().enumerate() {
                bytes.push(*byte ^ mask[index % 4]);
            }
        } else {
            bytes.extend_from_slice(&self.payload);
        }

        bytes
    }

    /// Decodes a single frame from bytes and returns the frame plus the consumed length.
    /// Payloads above 16 MiB are rejected before allocating the decoded payload.
    pub fn decode(bytes: &[u8]) -> Result<(Self, usize), CdpError> {
        if bytes.len() < 2 {
            return Err(CdpError::InvalidWebSocketFrame("frame too short"));
        }

        let fin = bytes[0] & 0x80 != 0;
        let opcode = WebSocketOpcode::from_u8(bytes[0] & 0x0F)?;
        let masked = bytes[1] & 0x80 != 0;
        let mut cursor = 2usize;

        let payload_len = match bytes[1] & 0x7F {
            len @ 0..=125 => len as usize,
            126 => {
                if bytes.len() < cursor + 2 {
                    return Err(CdpError::InvalidWebSocketFrame("missing extended length"));
                }
                let mut len_bytes = [0u8; 2];
                len_bytes.copy_from_slice(&bytes[cursor..cursor + 2]);
                cursor += 2;
                u16::from_be_bytes(len_bytes) as usize
            }
            127 => {
                if bytes.len() < cursor + 8 {
                    return Err(CdpError::InvalidWebSocketFrame("missing extended length"));
                }
                let mut len_bytes = [0u8; 8];
                len_bytes.copy_from_slice(&bytes[cursor..cursor + 8]);
                cursor += 8;
                let length = u64::from_be_bytes(len_bytes);
                if length >> 63 != 0 {
                    return Err(CdpError::InvalidWebSocketFrame("invalid 64-bit length"));
                }
                usize::try_from(length)
                    .map_err(|_| CdpError::InvalidWebSocketFrame("length out of range"))?
            }
            _ => unreachable!(),
        };

        if payload_len > MAX_WEBSOCKET_PAYLOAD {
            return Err(CdpError::InvalidWebSocketFrame("payload limit exceeded"));
        }

        let mask = if masked {
            if bytes.len() < cursor + 4 {
                return Err(CdpError::InvalidWebSocketFrame("missing mask"));
            }
            let mut mask = [0u8; 4];
            mask.copy_from_slice(&bytes[cursor..cursor + 4]);
            cursor += 4;
            Some(mask)
        } else {
            None
        };

        let end = cursor
            .checked_add(payload_len)
            .ok_or(CdpError::InvalidWebSocketFrame("length overflow"))?;
        if bytes.len() < end {
            return Err(CdpError::InvalidWebSocketFrame("payload truncated"));
        }

        let mut payload = bytes[cursor..end].to_vec();
        if let Some(mask) = mask {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }

        Ok((
            Self {
                fin,
                opcode,
                payload,
            },
            end,
        ))
    }

    /// Decodes a client frame, enforcing RFC 6455 masking requirements.
    pub fn decode_client(bytes: &[u8]) -> Result<(Self, usize), CdpError> {
        if bytes.get(1).is_some_and(|byte| byte & 0x80 == 0) {
            return Err(CdpError::InvalidWebSocketFrame(
                "client frame must be masked",
            ));
        }
        Self::decode(bytes)
    }
}

/// Computes the `Sec-WebSocket-Accept` value for a client key.
pub fn websocket_accept_key(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WEBSOCKET_GUID.as_bytes());
    BASE64_STANDARD.encode(hasher.finalize())
}

/// Parses a raw HTTP upgrade request into the fields needed for a WebSocket handshake.
pub fn parse_upgrade_request(request: &str) -> Result<WebSocketUpgradeRequest, CdpError> {
    let mut lines = request.split("\r\n");
    let request_line = lines
        .next()
        .ok_or(CdpError::InvalidHttpRequest("missing request line"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or_default();

    if method != "GET" || path.is_empty() || version != "HTTP/1.1" {
        return Err(CdpError::InvalidHttpRequest("invalid request line"));
    }

    let mut headers = HashMap::new();
    for line in lines.take_while(|line| !line.is_empty()) {
        let Some((name, value)) = line.split_once(':') else {
            return Err(CdpError::InvalidHttpRequest("malformed header"));
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }

    let upgrade = headers
        .get("upgrade")
        .map(|value| value.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    let connection = headers
        .get("connection")
        .map(|value| value.to_ascii_lowercase().contains("upgrade"))
        .unwrap_or(false);
    let key = headers
        .get("sec-websocket-key")
        .cloned()
        .ok_or(CdpError::InvalidHttpRequest("missing Sec-WebSocket-Key"))?;
    let version = headers
        .get("sec-websocket-version")
        .map(String::as_str)
        .unwrap_or_default();

    if !upgrade || !connection {
        return Err(CdpError::InvalidHttpRequest(
            "missing websocket upgrade headers",
        ));
    }
    if version != "13" {
        return Err(CdpError::InvalidHttpRequest(
            "unsupported Sec-WebSocket-Version",
        ));
    }

    Ok(WebSocketUpgradeRequest {
        path: path.to_string(),
        websocket_key: key,
    })
}

#[cfg(test)]
mod boundary_tests {
    use super::*;

    #[test]
    fn client_mask_is_required_but_server_frames_remain_decodable() {
        let frame = WebSocketFrame::text("hello");
        assert!(WebSocketFrame::decode_client(&frame.encode(false)).is_err());
        assert_eq!(
            WebSocketFrame::decode(&frame.encode(false)).unwrap().0,
            frame
        );
        assert_eq!(
            WebSocketFrame::decode_client(&frame.encode(true))
                .unwrap()
                .0,
            frame
        );
    }

    #[test]
    fn payload_limit_is_checked_before_reading_payload() {
        for length in [MAX_WEBSOCKET_PAYLOAD, MAX_WEBSOCKET_PAYLOAD + 1] {
            let mut header = vec![0x82, 127];
            header.extend_from_slice(&(length as u64).to_be_bytes());
            let error = WebSocketFrame::decode(&header).unwrap_err();
            let expected = if length == MAX_WEBSOCKET_PAYLOAD {
                "payload truncated"
            } else {
                "payload limit exceeded"
            };
            assert!(
                matches!(error, CdpError::InvalidWebSocketFrame(message) if message == expected)
            );
        }
    }

    #[test]
    fn maximum_allowed_payload_round_trips() {
        let frame = WebSocketFrame {
            fin: true,
            opcode: WebSocketOpcode::Binary,
            payload: vec![42; MAX_WEBSOCKET_PAYLOAD],
        };
        let encoded = frame.encode(true);
        let (decoded, consumed) = WebSocketFrame::decode_client(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded, frame);
    }
}
