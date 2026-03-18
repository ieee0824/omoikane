//! CDP transport primitives: WebSocket upgrade, frame handling, and JSON-RPC routing.

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Errors produced while upgrading, parsing frames, or dispatching JSON-RPC requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdpError {
    InvalidHttpRequest(&'static str),
    InvalidWebSocketFrame(&'static str),
    InvalidJsonRpc(&'static str),
    UnknownClient(u64),
    MethodNotFound(String),
}

impl std::fmt::Display for CdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHttpRequest(message) => write!(f, "invalid HTTP request: {message}"),
            Self::InvalidWebSocketFrame(message) => {
                write!(f, "invalid WebSocket frame: {message}")
            }
            Self::InvalidJsonRpc(message) => write!(f, "invalid JSON-RPC message: {message}"),
            Self::UnknownClient(id) => write!(f, "unknown client: {id}"),
            Self::MethodNotFound(method) => write!(f, "method not found: {method}"),
        }
    }
}

impl std::error::Error for CdpError {}

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

    fn as_u8(self) -> u8 {
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
                u64::from_be_bytes(len_bytes) as usize
            }
            _ => unreachable!(),
        };

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

        if bytes.len() < cursor + payload_len {
            return Err(CdpError::InvalidWebSocketFrame("payload truncated"));
        }

        let mut payload = bytes[cursor..cursor + payload_len].to_vec();
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
            cursor + payload_len,
        ))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonRpcRequest {
    pub id: Option<Value>,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonRpcResponse {
    pub id: Value,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

impl JsonRpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    fn method_not_found(id: Value, method: String) -> Self {
        Self {
            id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {method}"),
            }),
        }
    }

    fn invalid_params(id: Value, message: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: message.into(),
            }),
        }
    }

    fn to_json(&self) -> Value {
        match &self.error {
            Some(error) => json!({
                "jsonrpc": "2.0",
                "id": self.id,
                "error": {
                    "code": error.code,
                    "message": error.message,
                }
            }),
            None => json!({
                "jsonrpc": "2.0",
                "id": self.id,
                "result": self.result.clone().unwrap_or(Value::Null),
            }),
        }
    }
}

type RpcHandler = Box<dyn Fn(&Value) -> Result<Value, JsonRpcError>>;

#[derive(Default)]
struct ClientState {
    outgoing: Vec<WebSocketFrame>,
}

/// Minimal multi-client WebSocket + JSON-RPC server state for CDP transport.
#[derive(Default)]
pub struct CdpServer {
    next_client_id: u64,
    clients: HashMap<u64, ClientState>,
    handlers: HashMap<String, RpcHandler>,
}

impl CdpServer {
    /// Creates an empty server.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a JSON-RPC method handler.
    pub fn register_method<F>(&mut self, method: impl Into<String>, handler: F)
    where
        F: Fn(&Value) -> Result<Value, JsonRpcError> + 'static,
    {
        self.handlers.insert(method.into(), Box::new(handler));
    }

    /// Accepts a WebSocket upgrade request and allocates a client id.
    pub fn accept_upgrade(&mut self, request: &str) -> Result<WebSocketUpgrade, CdpError> {
        let request = parse_upgrade_request(request)?;
        let client_id = self.next_client_id;
        self.next_client_id += 1;
        self.clients.insert(client_id, ClientState::default());

        Ok(WebSocketUpgrade {
            client_id,
            response: format!(
                "HTTP/1.1 101 Switching Protocols\r\n\
                 Upgrade: websocket\r\n\
                 Connection: Upgrade\r\n\
                 Sec-WebSocket-Accept: {}\r\n\r\n",
                websocket_accept_key(&request.websocket_key)
            ),
        })
    }

    /// Returns the number of currently connected clients.
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// Processes a single incoming frame from a client.
    pub fn receive(&mut self, client_id: u64, bytes: &[u8]) -> Result<(), CdpError> {
        if !self.clients.contains_key(&client_id) {
            return Err(CdpError::UnknownClient(client_id));
        }

        let (frame, _) = WebSocketFrame::decode(bytes)?;
        match frame.opcode {
            WebSocketOpcode::Text => self.handle_text_frame(client_id, &frame.payload),
            WebSocketOpcode::Ping => {
                self.enqueue(client_id, WebSocketFrame::pong(frame.payload))?;
                Ok(())
            }
            WebSocketOpcode::Close => {
                self.clients.remove(&client_id);
                Ok(())
            }
            _ => Err(CdpError::InvalidWebSocketFrame(
                "only text, ping, and close are supported",
            )),
        }
    }

    /// Broadcasts a JSON-RPC notification to every connected client.
    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), CdpError> {
        let payload = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .map_err(|_| CdpError::InvalidJsonRpc("failed to serialize notification"))?;

        for client in self.clients.values_mut() {
            client.outgoing.push(WebSocketFrame::text(payload.clone()));
        }

        Ok(())
    }

    /// Returns and clears all queued server frames for a client.
    pub fn drain_outgoing(&mut self, client_id: u64) -> Result<Vec<WebSocketFrame>, CdpError> {
        let client = self
            .clients
            .get_mut(&client_id)
            .ok_or(CdpError::UnknownClient(client_id))?;
        Ok(std::mem::take(&mut client.outgoing))
    }

    fn enqueue(&mut self, client_id: u64, frame: WebSocketFrame) -> Result<(), CdpError> {
        let client = self
            .clients
            .get_mut(&client_id)
            .ok_or(CdpError::UnknownClient(client_id))?;
        client.outgoing.push(frame);
        Ok(())
    }

    fn handle_text_frame(&mut self, client_id: u64, payload: &[u8]) -> Result<(), CdpError> {
        let request = parse_json_rpc_request(payload)?;
        let Some(id) = request.id.clone() else {
            if let Some(handler) = self.handlers.get(&request.method) {
                handler(&request.params)
                    .map(|_| ())
                    .map_err(|_| CdpError::InvalidJsonRpc("notification handler failed"))
            } else {
                Err(CdpError::MethodNotFound(request.method))
            }?;
            return Ok(());
        };

        let response = if let Some(handler) = self.handlers.get(&request.method) {
            match handler(&request.params) {
                Ok(result) => JsonRpcResponse::success(id, result),
                Err(error) => JsonRpcResponse::invalid_params(id, error.message),
            }
        } else {
            JsonRpcResponse::method_not_found(id, request.method)
        };

        let payload = serde_json::to_string(&response.to_json())
            .map_err(|_| CdpError::InvalidJsonRpc("failed to serialize response"))?;
        self.enqueue(client_id, WebSocketFrame::text(payload))
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

fn parse_json_rpc_request(payload: &[u8]) -> Result<JsonRpcRequest, CdpError> {
    let value: Value = serde_json::from_slice(payload)
        .map_err(|_| CdpError::InvalidJsonRpc("message must be valid JSON"))?;

    let method = value
        .get("method")
        .and_then(Value::as_str)
        .ok_or(CdpError::InvalidJsonRpc("missing method"))?;

    Ok(JsonRpcRequest {
        id: value.get("id").cloned(),
        method: method.to_string(),
        params: value.get("params").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_upgrade_request() -> &'static str {
        "GET /devtools/browser HTTP/1.1\r\n\
         Host: localhost:9222\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    }

    fn decode_text(frame: &WebSocketFrame) -> String {
        String::from_utf8(frame.payload.clone()).unwrap()
    }

    #[test]
    fn computes_websocket_accept_key_from_rfc_example() {
        assert_eq!(
            websocket_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn parses_websocket_upgrade_request() {
        let request = parse_upgrade_request(sample_upgrade_request()).unwrap();

        assert_eq!(request.path, "/devtools/browser");
        assert_eq!(request.websocket_key, "dGhlIHNhbXBsZSBub25jZQ==");
    }

    #[test]
    fn encodes_and_decodes_masked_text_frames() {
        let original = WebSocketFrame::text("Browser.getVersion");
        let encoded = original.encode(true);
        let (decoded, consumed) = WebSocketFrame::decode(&encoded).unwrap();

        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded, original);
    }

    #[test]
    fn upgrades_connections_and_dispatches_json_rpc_requests() {
        let mut server = CdpServer::new();
        server.register_method("Browser.getVersion", |_| {
            Ok(json!({
                "product": "Omoikane/0.1",
                "protocolVersion": "1.3"
            }))
        });

        let upgrade = server.accept_upgrade(sample_upgrade_request()).unwrap();
        assert_eq!(upgrade.client_id, 0);
        assert!(upgrade.response.contains("101 Switching Protocols"));

        let request = WebSocketFrame::text(
            r#"{"jsonrpc":"2.0","id":1,"method":"Browser.getVersion","params":{}}"#,
        )
        .encode(true);
        server.receive(upgrade.client_id, &request).unwrap();

        let outgoing = server.drain_outgoing(upgrade.client_id).unwrap();
        assert_eq!(outgoing.len(), 1);
        let payload: Value = serde_json::from_str(&decode_text(&outgoing[0])).unwrap();
        assert_eq!(payload["id"], 1);
        assert_eq!(payload["result"]["product"], "Omoikane/0.1");
    }

    #[test]
    fn returns_method_not_found_for_unknown_calls() {
        let mut server = CdpServer::new();
        let upgrade = server.accept_upgrade(sample_upgrade_request()).unwrap();

        let request =
            WebSocketFrame::text(r#"{"jsonrpc":"2.0","id":7,"method":"Page.enable","params":{}}"#)
                .encode(true);
        server.receive(upgrade.client_id, &request).unwrap();

        let outgoing = server.drain_outgoing(upgrade.client_id).unwrap();
        let payload: Value = serde_json::from_str(&decode_text(&outgoing[0])).unwrap();
        assert_eq!(payload["error"]["code"], -32601);
        assert_eq!(payload["id"], 7);
    }

    #[test]
    fn broadcasts_notifications_to_multiple_clients() {
        let mut server = CdpServer::new();
        let first = server.accept_upgrade(sample_upgrade_request()).unwrap();
        let second = server.accept_upgrade(sample_upgrade_request()).unwrap();

        server
            .notify("Page.loadEventFired", json!({ "timestamp": 1.25 }))
            .unwrap();

        assert_eq!(server.client_count(), 2);

        let first_payload: Value = serde_json::from_str(&decode_text(
            &server.drain_outgoing(first.client_id).unwrap()[0],
        ))
        .unwrap();
        let second_payload: Value = serde_json::from_str(&decode_text(
            &server.drain_outgoing(second.client_id).unwrap()[0],
        ))
        .unwrap();

        assert_eq!(first_payload["method"], "Page.loadEventFired");
        assert_eq!(second_payload["params"]["timestamp"], 1.25);
    }

    #[test]
    fn responds_to_ping_and_removes_closed_clients() {
        let mut server = CdpServer::new();
        let upgrade = server.accept_upgrade(sample_upgrade_request()).unwrap();

        let ping = WebSocketFrame {
            fin: true,
            opcode: WebSocketOpcode::Ping,
            payload: b"hi".to_vec(),
        }
        .encode(true);
        server.receive(upgrade.client_id, &ping).unwrap();

        let outgoing = server.drain_outgoing(upgrade.client_id).unwrap();
        assert_eq!(outgoing[0], WebSocketFrame::pong(b"hi".to_vec()));

        let close = WebSocketFrame {
            fin: true,
            opcode: WebSocketOpcode::Close,
            payload: Vec::new(),
        }
        .encode(true);
        server.receive(upgrade.client_id, &close).unwrap();

        assert_eq!(server.client_count(), 0);
    }
}
