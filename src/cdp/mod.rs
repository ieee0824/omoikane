//! CDP transport primitives: WebSocket upgrade, frame handling, and JSON-RPC routing.

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use crate::dom::{Node, NodeHandle, NodeType};
use crate::html::{TreeBuilder, decode_html_response};
use crate::http::Client;
use crate::js::JsRuntime;

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

/// A queued CDP event emitted by domain operations.
#[derive(Debug, Clone, PartialEq)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
}

/// Minimal stateful CDP session spanning Page, DOM, Network, Runtime, Target, and Input.
#[derive(Debug)]
pub struct CdpSession {
    runtime: JsRuntime,
    http_client: Client,
    current_url: String,
    last_html: String,
    frame_id: String,
    next_loader_id: u64,
    next_node_id: u64,
    node_to_id: HashMap<usize, u64>,
    id_to_node: HashMap<u64, NodeHandle>,
    next_object_id: u64,
    next_browser_context_id: u64,
    browser_context_ids: Vec<String>,
    pending_events: Vec<CdpEvent>,
    last_key_event: Option<Value>,
    last_mouse_event: Option<Value>,
}

impl CdpSession {
    /// Creates a new session with an empty `about:blank` document.
    pub fn new() -> Result<Self, String> {
        let runtime = JsRuntime::new().map_err(|error| error.to_string())?;
        let mut session = Self {
            runtime,
            http_client: Client::new(),
            current_url: "about:blank".to_string(),
            last_html: String::new(),
            frame_id: "frame-0".to_string(),
            next_loader_id: 1,
            next_node_id: 1,
            node_to_id: HashMap::new(),
            id_to_node: HashMap::new(),
            next_object_id: 0,
            next_browser_context_id: 0,
            browser_context_ids: Vec::new(),
            pending_events: Vec::new(),
            last_key_event: None,
            last_mouse_event: None,
        };
        session
            .install_runtime_helpers()
            .map_err(js_error_message)?;
        session.rebuild_node_index();
        Ok(session)
    }

    /// Dispatches a CDP domain method and returns the result payload.
    pub fn dispatch(&mut self, method: &str, params: Value) -> Result<Value, JsonRpcError> {
        match method {
            "Page.navigate" => self.page_navigate(&params),
            "Page.reload" => self.page_reload(),
            "Page.getFrameTree" => Ok(self.page_get_frame_tree()),
            "DOM.getDocument" => self.dom_get_document(&params),
            "DOM.getAttributes" => self.dom_get_attributes(&params),
            "DOM.querySelector" => self.dom_query_selector(&params),
            "DOM.getOuterHTML" => self.dom_get_outer_html(&params),
            "Runtime.evaluate" => self.runtime_evaluate(&params),
            "Runtime.callFunctionOn" => self.runtime_call_function_on(&params),
            "Target.createBrowserContext" => Ok(self.target_create_browser_context()),
            "Target.getBrowserContexts" => Ok(self.target_get_browser_contexts()),
            "Target.disposeBrowserContext" => self.target_dispose_browser_context(&params),
            "Input.dispatchKeyEvent" => self.input_dispatch_key_event(&params),
            "Input.dispatchMouseEvent" => self.input_dispatch_mouse_event(&params),
            _ => Err(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {method}"),
            }),
        }
    }

    /// Returns and clears queued protocol events.
    pub fn drain_events(&mut self) -> Vec<CdpEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Returns the current document.
    pub fn document(&self) -> NodeHandle {
        self.runtime.document()
    }

    /// Returns the URL of the currently loaded document.
    pub fn current_url(&self) -> &str {
        &self.current_url
    }

    /// Sets the HTTP client `User-Agent` used for subsequent navigations.
    pub fn set_user_agent(&mut self, user_agent: impl Into<String>) {
        let user_agent = user_agent.into();
        self.http_client.set_user_agent(user_agent.clone());
        self.runtime.set_user_agent(user_agent);
    }

    /// When `true`, disables TLS certificate verification for all subsequent
    /// navigations. Expired certificates, self-signed certificates, and hostname
    /// mismatches are silently accepted.
    ///
    /// **Security warning**: Only use this in development or testing environments.
    pub fn set_insecure(&mut self, insecure: bool) {
        self.http_client.set_insecure(insecure);
    }

    pub(crate) fn http_client_mut(&mut self) -> &mut Client {
        &mut self.http_client
    }

    fn page_navigate(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let url = require_string(params, "url")?;
        let loader_id = self.next_loader_id.to_string();
        self.next_loader_id += 1;

        self.emit(
            "Network.requestWillBeSent",
            json!({
                "requestId": loader_id,
                "documentURL": url,
                "request": { "url": url, "method": "GET" },
                "type": "Document",
            }),
        );

        let (html, status) = self
            .load_page_source(&url)
            .map_err(|message| JsonRpcError {
                code: -32000,
                message,
            })?;

        self.install_document(&url, &html)
            .map_err(|message| JsonRpcError {
                code: -32000,
                message,
            })?;

        self.emit(
            "Network.responseReceived",
            json!({
                "requestId": loader_id,
                "type": "Document",
                "response": {
                    "url": url,
                    "status": status,
                    "mimeType": "text/html",
                },
            }),
        );
        self.emit(
            "Page.frameNavigated",
            json!({
                "frame": {
                    "id": self.frame_id,
                    "url": self.current_url,
                    "mimeType": "text/html",
                }
            }),
        );
        self.emit(
            "Page.loadEventFired",
            json!({ "timestamp": self.next_loader_id }),
        );

        Ok(json!({
            "frameId": self.frame_id,
            "loaderId": loader_id,
        }))
    }

    fn page_reload(&mut self) -> Result<Value, JsonRpcError> {
        let url = self.current_url.clone();
        self.page_navigate(&json!({ "url": url }))?;
        Ok(json!({}))
    }

    fn page_get_frame_tree(&self) -> Value {
        json!({
            "frameTree": {
                "frame": {
                    "id": self.frame_id,
                    "url": self.current_url,
                    "mimeType": "text/html",
                }
            }
        })
    }

    fn dom_get_document(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let depth = params.get("depth").and_then(Value::as_i64).unwrap_or(-1);
        let document = self.runtime.document();
        Ok(json!({
            "root": self.serialize_node(&document, depth),
        }))
    }

    fn dom_query_selector(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let node_id = require_u64(params, "nodeId")?;
        let selector = require_string(params, "selector")?;
        let node = self.lookup_node(node_id)?;
        let result = node
            .query_selector(&selector)
            .map(|node| self.ensure_node_id(&node))
            .unwrap_or(0);
        Ok(json!({ "nodeId": result }))
    }

    fn dom_get_attributes(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let node_id = require_u64(params, "nodeId")?;
        let node = self.lookup_node(node_id)?;
        let attributes = node
            .attributes()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|(name, value)| [Value::String(name), Value::String(value)])
            .collect::<Vec<_>>();
        Ok(json!({ "attributes": attributes }))
    }

    fn dom_get_outer_html(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let node_id = require_u64(params, "nodeId")?;
        let node = self.lookup_node(node_id)?;
        Ok(json!({
            "outerHTML": serialize_outer_html(&node),
        }))
    }

    fn runtime_evaluate(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let expression = require_string(params, "expression")?;
        let return_by_value = params
            .get("returnByValue")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        self.evaluate_expression(&expression, return_by_value)
    }

    fn runtime_call_function_on(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let function_declaration = require_string(params, "functionDeclaration")?;
        let return_by_value = params
            .get("returnByValue")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let object_expr = params
            .get("objectId")
            .and_then(Value::as_str)
            .map(|id| format!("globalThis[{id:?}]"))
            .unwrap_or_else(|| "undefined".to_string());
        let argument_expr = params
            .get("arguments")
            .and_then(Value::as_array)
            .map(|arguments| {
                arguments
                    .iter()
                    .map(argument_to_js)
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default()
            .join(", ");

        let expression = format!(
            "(() => {{ const __fn = ({function_declaration}); return __fn.call({object_expr}{comma}{argument_expr}); }})()",
            comma = if argument_expr.is_empty() { "" } else { ", " }
        );
        self.evaluate_expression(&expression, return_by_value)
    }

    fn target_create_browser_context(&mut self) -> Value {
        let browser_context_id = format!("context-{}", self.next_browser_context_id);
        self.next_browser_context_id += 1;
        self.browser_context_ids.push(browser_context_id.clone());
        json!({ "browserContextId": browser_context_id })
    }

    fn target_get_browser_contexts(&self) -> Value {
        json!({ "browserContextIds": self.browser_context_ids })
    }

    fn target_dispose_browser_context(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let browser_context_id = require_string(params, "browserContextId")?;
        let original_len = self.browser_context_ids.len();
        self.browser_context_ids
            .retain(|current| current != &browser_context_id);

        if self.browser_context_ids.len() == original_len {
            return Err(JsonRpcError {
                code: -32000,
                message: format!("Unknown browser context: {browser_context_id}"),
            });
        }

        Ok(json!({}))
    }

    fn input_dispatch_key_event(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let event_type = require_string(params, "type")?;
        self.last_key_event = Some(params.clone());
        self.dispatch_document_event(&event_type)?;
        Ok(json!({}))
    }

    fn input_dispatch_mouse_event(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let event_type = require_string(params, "type")?;
        self.last_mouse_event = Some(params.clone());
        self.dispatch_document_event(&event_type)?;
        Ok(json!({}))
    }

    fn dispatch_document_event(&mut self, event_type: &str) -> Result<(), JsonRpcError> {
        self.runtime
            .eval(&format!(
                "document.dispatchEvent(new Event({event_type:?}));"
            ))
            .and_then(|_| self.runtime.run_jobs())
            .map_err(js_error)
            .map(|_| ())
    }

    fn evaluate_expression(
        &mut self,
        expression: &str,
        return_by_value: bool,
    ) -> Result<Value, JsonRpcError> {
        let script = if return_by_value {
            format!(
                "(() => {{ const __value = eval({expression:?}); return JSON.stringify({{ result: __cdpSerializeValue(__value) }}); }})()"
            )
        } else {
            let object_id = format!("__cdp_object_{}", self.next_object_id);
            self.next_object_id += 1;
            format!(
                "(() => {{ globalThis[{object_id:?}] = eval({expression:?}); return JSON.stringify({{ result: __cdpRemoteObject(globalThis[{object_id:?}], {object_id:?}) }}); }})()"
            )
        };

        let raw = self.runtime.eval(&script).map_err(js_error)?;
        let payload = raw
            .as_string()
            .ok_or(JsonRpcError {
                code: -32000,
                message: "Runtime evaluation did not return a string payload".to_string(),
            })?
            .to_std_string_escaped();
        let parsed: Value = serde_json::from_str(&payload).map_err(|error| JsonRpcError {
            code: -32000,
            message: error.to_string(),
        })?;
        Ok(parsed)
    }

    fn load_page_source(&mut self, url: &str) -> Result<(String, u16), String> {
        if url == "about:blank" {
            return Ok(("<html><head></head><body></body></html>".to_string(), 200));
        }

        if let Some(data) = url.strip_prefix("data:text/html,") {
            return Ok((percent_decode(data), 200));
        }

        let response = self
            .http_client
            .get(url)
            .map_err(|error| error.to_string())?;
        Ok((decode_html_response(&response), response.status_code()))
    }

    fn install_document(&mut self, url: &str, html: &str) -> Result<(), String> {
        let document = TreeBuilder::parse(html).document();
        self.runtime = JsRuntime::with_document(document).map_err(|error| error.to_string())?;
        self.runtime
            .set_user_agent(self.http_client.user_agent().to_string());
        self.current_url = url.to_string();
        self.last_html = html.to_string();
        self.rebuild_node_index();
        self.install_runtime_helpers().map_err(js_error_message)?;
        Ok(())
    }

    fn install_runtime_helpers(&mut self) -> Result<(), boa_engine::JsError> {
        self.runtime.eval(
            r#"
            globalThis.__cdpSerializeValue = function(value) {
              if (value === undefined) return { type: "undefined" };
              if (value === null) return { type: "object", subtype: "null", value: null };
              if (typeof value === "number") return { type: "number", value };
              if (typeof value === "string") return { type: "string", value };
              if (typeof value === "boolean") return { type: "boolean", value };
              if (typeof value === "function") return { type: "function", description: String(value) };
              return { type: "object", value: JSON.parse(JSON.stringify(value)) };
            };
            globalThis.__cdpRemoteObject = function(value, objectId) {
              if (value === null) return { type: "object", subtype: "null", value: null };
              return {
                type: typeof value === "object" ? "object" : typeof value,
                objectId,
                description: Object.prototype.toString.call(value),
              };
            };
            "#,
        )?;
        self.runtime.run_jobs()
    }

    fn emit(&mut self, method: &str, params: Value) {
        self.pending_events.push(CdpEvent {
            method: method.to_string(),
            params,
        });
    }

    fn rebuild_node_index(&mut self) {
        self.node_to_id.clear();
        self.id_to_node.clear();
        self.next_node_id = 1;
        let document = self.runtime.document();
        self.register_subtree(&document);
    }

    fn register_subtree(&mut self, node: &NodeHandle) {
        self.ensure_node_id(node);
        for child in node.child_nodes() {
            self.register_subtree(&child);
        }
    }

    fn ensure_node_id(&mut self, node: &NodeHandle) -> u64 {
        let identity = node.identity();
        if let Some(node_id) = self.node_to_id.get(&identity) {
            return *node_id;
        }

        let node_id = self.next_node_id;
        self.next_node_id += 1;
        self.node_to_id.insert(identity, node_id);
        self.id_to_node.insert(node_id, node.clone());
        node_id
    }

    fn lookup_node(&self, node_id: u64) -> Result<NodeHandle, JsonRpcError> {
        self.id_to_node.get(&node_id).cloned().ok_or(JsonRpcError {
            code: -32000,
            message: format!("Unknown node: {node_id}"),
        })
    }

    fn serialize_node(&mut self, node: &NodeHandle, depth: i64) -> Value {
        let node_id = self.ensure_node_id(node);
        let children = if depth == 0 {
            None
        } else {
            let next_depth = if depth < 0 { -1 } else { depth - 1 };
            Some(
                node.child_nodes()
                    .iter()
                    .map(|child| self.serialize_node(child, next_depth))
                    .collect::<Vec<_>>(),
            )
        };

        let local_name = match node.node_type() {
            NodeType::Element => node.tag_name().unwrap_or_default(),
            _ => String::new(),
        };
        let node_value = match node.node_type() {
            NodeType::Text | NodeType::Comment | NodeType::DocumentType => {
                node.data().unwrap_or_default()
            }
            _ => String::new(),
        };

        let mut payload = json!({
            "nodeId": node_id,
            "nodeType": cdp_node_type(node),
            "nodeName": node.node_name(),
            "localName": local_name,
            "nodeValue": node_value,
            "childNodeCount": node.child_nodes().len(),
        });

        if let Some(attributes) = node.attributes() {
            let flattened = attributes
                .into_iter()
                .flat_map(|(name, value)| [Value::String(name), Value::String(value)])
                .collect::<Vec<_>>();
            payload["attributes"] = Value::Array(flattened);
        }

        if let Some(children) = children {
            payload["children"] = Value::Array(children);
        }

        payload
    }
}

fn require_string(params: &Value, key: &'static str) -> Result<String, JsonRpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or(JsonRpcError {
            code: -32602,
            message: format!("Missing or invalid string parameter: {key}"),
        })
}

fn require_u64(params: &Value, key: &'static str) -> Result<u64, JsonRpcError> {
    params.get(key).and_then(Value::as_u64).ok_or(JsonRpcError {
        code: -32602,
        message: format!("Missing or invalid numeric parameter: {key}"),
    })
}

fn argument_to_js(argument: &Value) -> Result<String, JsonRpcError> {
    if let Some(object_id) = argument.get("objectId").and_then(Value::as_str) {
        return Ok(format!("globalThis[{object_id:?}]"));
    }

    if let Some(value) = argument.get("value") {
        return Ok(value.to_string());
    }

    Err(JsonRpcError {
        code: -32602,
        message: "Each Runtime.callFunctionOn argument must include value or objectId".to_string(),
    })
}

fn percent_decode(input: &str) -> String {
    let mut output = String::new();
    let bytes = input.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3])
                    && let Ok(value) = u8::from_str_radix(hex, 16) {
                        output.push(value as char);
                        index += 3;
                        continue;
                    }
                output.push('%');
                index += 1;
            }
            b'+' => {
                output.push(' ');
                index += 1;
            }
            byte => {
                output.push(byte as char);
                index += 1;
            }
        }
    }
    output
}

fn cdp_node_type(node: &NodeHandle) -> u8 {
    match node.node_type() {
        NodeType::Element => 1,
        NodeType::Text => 3,
        NodeType::ProcessingInstruction => 7,
        NodeType::Comment => 8,
        NodeType::Document => 9,
        NodeType::DocumentType => 10,
        NodeType::DocumentFragment => 11,
    }
}

fn serialize_outer_html(node: &NodeHandle) -> String {
    match node.node_type() {
        NodeType::Document => node
            .child_nodes()
            .iter()
            .map(serialize_outer_html)
            .collect::<Vec<_>>()
            .join(""),
        NodeType::Element => {
            let tag_name = node.tag_name().unwrap_or_default();
            let attributes = node
                .attributes()
                .unwrap_or_default()
                .into_iter()
                .map(|(name, value)| format!(r#" {name}="{}""#, escape_html(&value)))
                .collect::<Vec<_>>()
                .join("");
            let children = node
                .child_nodes()
                .iter()
                .map(serialize_outer_html)
                .collect::<Vec<_>>()
                .join("");
            format!("<{tag_name}{attributes}>{children}</{tag_name}>")
        }
        NodeType::Text => escape_html(&node.data().unwrap_or_default()),
        NodeType::Comment => format!("<!--{}-->", node.data().unwrap_or_default()),
        NodeType::ProcessingInstruction => format!(
            "<?{} {}?>",
            node.node_name(),
            node.data().unwrap_or_default()
        ),
        NodeType::DocumentType => format!("<!DOCTYPE {}>", node.data().unwrap_or_default()),
        NodeType::DocumentFragment => node
            .child_nodes()
            .iter()
            .map(serialize_outer_html)
            .collect::<Vec<_>>()
            .join(""),
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn js_error(error: boa_engine::JsError) -> JsonRpcError {
    JsonRpcError {
        code: -32000,
        message: error.to_string(),
    }
}

fn js_error_message(error: boa_engine::JsError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

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

    #[test]
    fn page_domain_navigates_reloads_and_emits_network_events() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let body = "<html><body><main id=\"app\">Hello</main></body></html>";
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let mut session = CdpSession::new().unwrap();
        let url = format!("http://127.0.0.1:{}/", address.port());
        let navigate = session
            .dispatch("Page.navigate", json!({ "url": url.clone() }))
            .unwrap();
        assert_eq!(navigate["frameId"], "frame-0");

        let frame_tree = session.dispatch("Page.getFrameTree", json!({})).unwrap();
        assert_eq!(frame_tree["frameTree"]["frame"]["url"], url);

        session.dispatch("Page.reload", json!({})).unwrap();

        let events = session.drain_events();
        assert!(
            events
                .iter()
                .any(|event| event.method == "Network.requestWillBeSent")
        );
        assert!(
            events
                .iter()
                .any(|event| event.method == "Network.responseReceived")
        );
        assert!(
            events
                .iter()
                .any(|event| event.method == "Page.loadEventFired")
        );

        server.join().unwrap();
    }

    #[test]
    fn dom_domain_exposes_document_query_attributes_and_outer_html() {
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch(
                "Page.navigate",
                json!({ "url": "data:text/html,<html><head><meta property=\"og:image\" content=\"https://example.com/image.jpg\"></head><body><main id=\"app\"><p>Hello</p></main></body></html>" }),
            )
            .unwrap();

        let document = session.dispatch("DOM.getDocument", json!({})).unwrap();
        let root_id = document["root"]["nodeId"].as_u64().unwrap();
        let queried = session
            .dispatch(
                "DOM.querySelector",
                json!({ "nodeId": root_id, "selector": "#app" }),
            )
            .unwrap();
        let app_id = queried["nodeId"].as_u64().unwrap();
        let meta = session
            .dispatch(
                "DOM.querySelector",
                json!({ "nodeId": root_id, "selector": r#"meta[property="og:image"]"# }),
            )
            .unwrap();
        let meta_id = meta["nodeId"].as_u64().unwrap();
        let attributes = session
            .dispatch("DOM.getAttributes", json!({ "nodeId": meta_id }))
            .unwrap();
        let html = session
            .dispatch("DOM.getOuterHTML", json!({ "nodeId": app_id }))
            .unwrap();

        assert_eq!(document["root"]["nodeName"], "#document");
        assert!(app_id > 0);
        assert!(meta_id > 0);
        assert_eq!(
            attributes["attributes"],
            json!([
                "content",
                "https://example.com/image.jpg",
                "property",
                "og:image"
            ])
        );
        assert_eq!(html["outerHTML"], "<main id=\"app\"><p>Hello</p></main>");
    }

    #[test]
    fn runtime_domain_evaluates_and_calls_functions_on_remote_objects() {
        let mut session = CdpSession::new().unwrap();

        let value = session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "21 * 2", "returnByValue": true }),
            )
            .unwrap();
        assert_eq!(value["result"]["value"], 42);

        let object = session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "({ count: 2 })", "returnByValue": false }),
            )
            .unwrap();
        let object_id = object["result"]["objectId"].as_str().unwrap().to_string();

        let called = session
            .dispatch(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": "function(multiplier) { return this.count * multiplier; }",
                    "arguments": [{ "value": 3 }],
                    "returnByValue": true,
                }),
            )
            .unwrap();

        assert_eq!(called["result"]["value"], 6);
    }

    #[test]
    fn target_and_input_domains_manage_contexts_and_dispatch_dom_events() {
        let mut session = CdpSession::new().unwrap();

        let first = session
            .dispatch("Target.createBrowserContext", json!({}))
            .unwrap();
        let second = session
            .dispatch("Target.createBrowserContext", json!({}))
            .unwrap();
        let contexts = session
            .dispatch("Target.getBrowserContexts", json!({}))
            .unwrap();
        assert_eq!(contexts["browserContextIds"].as_array().unwrap().len(), 2);

        session
            .dispatch(
                "Runtime.evaluate",
                json!({
                    "expression": "globalThis.keyCount = 0; globalThis.mouseCount = 0; document.addEventListener('keydown', () => { globalThis.keyCount += 1; }); document.addEventListener('click', () => { globalThis.mouseCount += 1; }); 0",
                    "returnByValue": true
                }),
            )
            .unwrap();
        session
            .dispatch("Input.dispatchKeyEvent", json!({ "type": "keydown" }))
            .unwrap();
        session
            .dispatch("Input.dispatchMouseEvent", json!({ "type": "click" }))
            .unwrap();

        let counts = session
            .dispatch(
                "Runtime.evaluate",
                json!({
                    "expression": "JSON.stringify({ keyCount: globalThis.keyCount, mouseCount: globalThis.mouseCount })",
                    "returnByValue": true
                }),
            )
            .unwrap();
        assert_eq!(
            counts["result"]["value"],
            "{\"keyCount\":1,\"mouseCount\":1}"
        );

        session
            .dispatch(
                "Target.disposeBrowserContext",
                json!({ "browserContextId": first["browserContextId"] }),
            )
            .unwrap();
        let remaining = session
            .dispatch("Target.getBrowserContexts", json!({}))
            .unwrap();
        assert_eq!(remaining["browserContextIds"].as_array().unwrap().len(), 1);
        assert_eq!(
            remaining["browserContextIds"][0],
            second["browserContextId"].clone()
        );
    }
}
