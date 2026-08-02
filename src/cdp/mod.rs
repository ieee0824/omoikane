//! CDP transport primitives: WebSocket upgrade, frame handling, and JSON-RPC routing.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use boa_engine::JsValue;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use crate::dom::{Node, NodeHandle, NodeType};
use crate::html::{TreeBuilder, decode_html_response};
use crate::http::{Client, HttpRequest, Method};
#[cfg(test)]
use crate::js::PageTaskSource;
use crate::js::{
    CompletedPageTask, JavaScriptDialog, JavaScriptDialogController, JavaScriptDialogError,
    JavaScriptDialogKind, JsRuntime, NavigationRequest, OwnedPageTask, PageTaskError,
    StorageManager,
};

const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Errors produced while upgrading, parsing frames, or dispatching JSON-RPC requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdpError {
    InvalidHttpRequest(&'static str),
    InvalidWebSocketFrame(&'static str),
    InvalidJsonRpc(&'static str),
    UnknownClient(u64),
    UnknownDeferredRequest(u64),
    DeferredTokenExhausted,
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
            Self::UnknownDeferredRequest(id) => write!(f, "unknown deferred request: {id}"),
            Self::DeferredTokenExhausted => write!(f, "deferred response token space exhausted"),
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

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON-RPC error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for JsonRpcError {}

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
type DeferredRpcHandler =
    Box<dyn Fn(DeferredResponseToken, &Value) -> Result<CdpMethodResult, JsonRpcError>>;

/// Opaque identifier for a JSON-RPC request whose response will be completed later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeferredResponseToken(u64);

/// Result of a deferred-capable method handler.
#[derive(Debug, Clone, PartialEq)]
pub enum CdpMethodResult {
    Complete(Value),
    Deferred,
}

struct PendingResponse {
    client_id: u64,
    request_id: Value,
}

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
    deferred_handlers: HashMap<String, DeferredRpcHandler>,
    next_deferred_token: u64,
    pending_responses: HashMap<DeferredResponseToken, PendingResponse>,
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
        let method = method.into();
        self.deferred_handlers.remove(&method);
        self.handlers.insert(method, Box::new(handler));
    }

    /// Registers a handler that may defer its JSON-RPC response.
    pub fn register_deferred_method<F>(&mut self, method: impl Into<String>, handler: F)
    where
        F: Fn(DeferredResponseToken, &Value) -> Result<CdpMethodResult, JsonRpcError> + 'static,
    {
        let method = method.into();
        self.handlers.remove(&method);
        self.deferred_handlers.insert(method, Box::new(handler));
    }

    /// Completes a previously deferred request on its original client and JSON-RPC id.
    pub fn complete_deferred_response(
        &mut self,
        token: DeferredResponseToken,
        result: Result<Value, JsonRpcError>,
    ) -> Result<(), CdpError> {
        let pending = self
            .pending_responses
            .remove(&token)
            .ok_or(CdpError::UnknownDeferredRequest(token.0))?;
        let response = match result {
            Ok(value) => JsonRpcResponse::success(pending.request_id, value),
            Err(error) => JsonRpcResponse {
                id: pending.request_id,
                result: None,
                error: Some(error),
            },
        };
        self.enqueue_response(pending.client_id, response)
    }

    /// Number of requests currently waiting for an out-of-band completion.
    pub fn pending_response_count(&self) -> usize {
        self.pending_responses.len()
    }

    fn deferred_response_client(&self, token: DeferredResponseToken) -> Option<u64> {
        self.pending_responses
            .get(&token)
            .map(|pending| pending.client_id)
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
                self.pending_responses
                    .retain(|_, pending| pending.client_id != client_id);
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

    fn enqueue_response(
        &mut self,
        client_id: u64,
        response: JsonRpcResponse,
    ) -> Result<(), CdpError> {
        let payload = serde_json::to_string(&response.to_json())
            .map_err(|_| CdpError::InvalidJsonRpc("failed to serialize response"))?;
        self.enqueue(client_id, WebSocketFrame::text(payload))
    }

    fn handle_text_frame(&mut self, client_id: u64, payload: &[u8]) -> Result<(), CdpError> {
        let request = parse_json_rpc_request(payload)?;
        let Some(id) = request.id.clone() else {
            if self.deferred_handlers.contains_key(&request.method) {
                return Err(CdpError::InvalidJsonRpc(
                    "deferred methods require a JSON-RPC id",
                ));
            }
            if let Some(handler) = self.handlers.get(&request.method) {
                handler(&request.params)
                    .map(|_| ())
                    .map_err(|_| CdpError::InvalidJsonRpc("notification handler failed"))
            } else {
                Err(CdpError::MethodNotFound(request.method))
            }?;
            return Ok(());
        };

        if self.deferred_handlers.contains_key(&request.method) {
            let token = DeferredResponseToken(self.next_deferred_token);
            self.next_deferred_token = self
                .next_deferred_token
                .checked_add(1)
                .ok_or(CdpError::DeferredTokenExhausted)?;
            let outcome = self.deferred_handlers[&request.method](token, &request.params);
            match outcome {
                Ok(CdpMethodResult::Complete(value)) => {
                    return self.enqueue_response(client_id, JsonRpcResponse::success(id, value));
                }
                Ok(CdpMethodResult::Deferred) => {
                    self.pending_responses.insert(
                        token,
                        PendingResponse {
                            client_id,
                            request_id: id,
                        },
                    );
                    return Ok(());
                }
                Err(error) => {
                    return self.enqueue_response(
                        client_id,
                        JsonRpcResponse {
                            id,
                            result: None,
                            error: Some(error),
                        },
                    );
                }
            }
        }

        let response = if let Some(handler) = self.handlers.get(&request.method) {
            match handler(&request.params) {
                Ok(result) => JsonRpcResponse::success(id, result),
                Err(error) => JsonRpcResponse {
                    id,
                    result: None,
                    error: Some(error),
                },
            }
        } else {
            JsonRpcResponse::method_not_found(id, request.method)
        };

        self.enqueue_response(client_id, response)
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SessionSettleTimings {
    pub timers: Duration,
    pub animation_frames: Duration,
}

/// Minimal stateful CDP session spanning Page, DOM, Network, Runtime, Target, and Input.
#[derive(Debug)]
pub struct CdpSession {
    runtime: JsRuntime,
    storage_manager: StorageManager,
    storage_session_id: u64,
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
    mouse_pressed_target: Option<usize>,
    drag_candidate: bool,
    drag_active: bool,
    history_entries: Vec<SessionHistoryEntry>,
    history_index: usize,
    document_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavigationCommit {
    Push,
    Replace,
    Reload,
    Traverse(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionHistoryEntry {
    url: String,
    state_json: String,
}

/// Document metadata held by the CDP host while its new runtime is owned by a
/// suspendable page-startup task.
pub(crate) struct PendingDocumentCommit {
    url: String,
    html: String,
    generation: u64,
    history_commit: NavigationCommit,
    loader_id: String,
    status: u16,
}

pub(crate) enum PreparedPageNavigation {
    Complete(Value),
    Pending {
        task: OwnedPageTask,
        commit: PendingDocumentCommit,
        response: Value,
    },
}

impl CdpSession {
    /// Creates a new session with an empty `about:blank` document.
    pub fn new() -> Result<Self, String> {
        let storage_manager = StorageManager::new();
        let storage_session_id = storage_manager.create_session();
        let runtime = JsRuntime::with_document_url_and_storage(
            TreeBuilder::parse("<html><head></head><body></body></html>").document(),
            "about:blank",
            storage_manager.clone(),
            storage_session_id,
        )
        .map_err(|error| error.to_string())?;
        let mut session = Self {
            runtime,
            storage_manager,
            storage_session_id,
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
            mouse_pressed_target: None,
            drag_candidate: false,
            drag_active: false,
            history_entries: vec![SessionHistoryEntry {
                url: "about:blank".to_string(),
                state_json: "null".to_string(),
            }],
            history_index: 0,
            document_generation: 0,
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

    /// Returns the active page's top-level Window scroll offset.
    pub(crate) fn window_scroll_offset(&self) -> (f32, f32) {
        self.runtime.window_scroll_offset()
    }

    /// Returns the URL of the currently loaded document.
    pub fn current_url(&self) -> &str {
        &self.current_url
    }

    /// Updates the active page's layout and script-visible viewport dimensions.
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.runtime.set_viewport(width as f32, height as f32);
    }

    /// Advances the active page event loop and commits script navigation.
    ///
    /// This is the browser-session lifecycle entry point for a future GUI
    /// frame pump: timer tasks and their microtasks run first, then one
    /// animation frame, then any resulting Location/History request installs
    /// the next Document.
    pub fn drive_event_loop(&mut self, elapsed_ms: u64) -> Result<(), JsonRpcError> {
        self.runtime
            .run_animation_frame(elapsed_ms)
            .map_err(js_error)?;
        self.drive_navigation_requests()
    }

    /// Settles deferred page initialization before a static render snapshot.
    pub(crate) fn settle_for_render(&mut self) -> Result<SessionSettleTimings, JsonRpcError> {
        const MAX_NAVIGATION_PASSES: usize = 8;
        const MAX_VIRTUAL_MS: u64 = 10_000;
        const TIMER_STEP_MS: u64 = 10;
        const MAX_TIMER_TASKS: usize = 100_000;
        const MAX_ANIMATION_FRAMES: usize = 8;

        let mut timings = SessionSettleTimings::default();
        for _ in 0..MAX_NAVIGATION_PASSES {
            let previous_generation = self.document_generation;
            let timers_start = Instant::now();
            self.runtime
                .run_timers(MAX_VIRTUAL_MS, TIMER_STEP_MS, MAX_TIMER_TASKS);
            timings.timers += timers_start.elapsed();
            let animation_frames_start = Instant::now();
            self.runtime.run_animation_frames(MAX_ANIMATION_FRAMES, 16);
            timings.animation_frames += animation_frames_start.elapsed();
            self.drive_navigation_requests()?;
            if self.document_generation == previous_generation {
                return Ok(timings);
            }
        }
        Err(JsonRpcError {
            code: -32000,
            message: "render navigation limit exceeded".to_string(),
        })
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
        // The first real navigation replaces the initial empty about:blank
        // entry. Later Page.navigate calls append normal session-history
        // entries.
        let commit = if self.current_url == "about:blank"
            && self.history_entries.len() == 1
            && self.history_index == 0
        {
            NavigationCommit::Replace
        } else {
            NavigationCommit::Push
        };
        let result = self.navigate_to(&url, commit)?;
        self.drive_navigation_requests()?;
        Ok(result)
    }

    pub(crate) fn prepare_page_navigate(
        &mut self,
        params: &Value,
    ) -> Result<PreparedPageNavigation, JsonRpcError> {
        let url = require_string(params, "url")?;
        let commit = if self.current_url == "about:blank"
            && self.history_entries.len() == 1
            && self.history_index == 0
        {
            NavigationCommit::Replace
        } else {
            NavigationCommit::Push
        };
        if is_fragment_only_navigation(&self.current_url, &url) {
            return self
                .navigate_to(&url, commit)
                .map(PreparedPageNavigation::Complete);
        }
        self.prepare_page_navigation_request(&url, commit)
    }

    pub(crate) fn prepare_page_reload(
        &mut self,
    ) -> Result<PreparedPageNavigation, JsonRpcError> {
        let url = self.current_url.clone();
        self.prepare_page_navigation_request(&url, NavigationCommit::Reload)
    }

    fn prepare_page_navigation_request(
        &mut self,
        url: &str,
        history_commit: NavigationCommit,
    ) -> Result<PreparedPageNavigation, JsonRpcError> {
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
        let (html, status, document_url, csp_headers) = self
            .load_page_request(url, Method::Get, None, None)
            .map_err(|message| JsonRpcError {
                code: -32000,
                message,
            })?;
        let (history_length, history_state) = self.prospective_history_state(history_commit);
        let (task, mut commit) = self
            .prepare_document_page_task(
                &document_url,
                &html,
                history_length,
                &history_state,
                &csp_headers,
            )
            .map_err(|message| JsonRpcError {
                code: -32000,
                message,
            })?;
        commit.history_commit = history_commit;
        commit.loader_id = loader_id.clone();
        commit.status = status;
        Ok(PreparedPageNavigation::Pending {
            task,
            commit,
            response: json!({ "frameId": self.frame_id, "loaderId": loader_id }),
        })
    }

    fn navigate_to(
        &mut self,
        url: &str,
        commit: NavigationCommit,
    ) -> Result<Value, JsonRpcError> {
        self.navigate_with_request(url, commit, Method::Get, None, None)
    }

    fn navigate_with_request(
        &mut self,
        url: &str,
        commit: NavigationCommit,
        method: Method,
        body: Option<Vec<u8>>,
        content_type: Option<String>,
    ) -> Result<Value, JsonRpcError> {
        let loader_id = self.next_loader_id.to_string();
        self.next_loader_id += 1;

        if method == Method::Get && commit != NavigationCommit::Reload
            && is_fragment_only_navigation(&self.current_url, url)
        {
            let previous_url = self.current_url.clone();
            self.commit_history_url(url, commit, None);
            self.current_url = url.to_string();
            self.runtime
                .eval(&format!(
                    "__omoikane_commit_same_document_navigation({url:?}, 'hashchange', {previous_url:?})"
                ))
                .and_then(|_| self.runtime.run_jobs())
                .map_err(js_error)?;
            self.sync_history_length()?;
            self.emit(
                "Page.navigatedWithinDocument",
                json!({ "frameId": self.frame_id, "url": url, "navigationType": "fragment" }),
            );
            return Ok(json!({ "frameId": self.frame_id }));
        }

        self.emit(
            "Network.requestWillBeSent",
            json!({
                "requestId": loader_id,
                "documentURL": url,
                "request": { "url": url, "method": method.as_str() },
                "type": "Document",
            }),
        );

        let (html, status, document_url, csp_headers) = self
            .load_page_request(url, method, body, content_type)
            .map_err(|message| JsonRpcError {
                code: -32000,
                message,
            })?;

        let (next_history_length, next_history_state) =
            self.prospective_history_state(commit);
        self.install_document_with_csp(
            &document_url,
            &html,
            next_history_length,
            &next_history_state,
            &csp_headers,
        )
            .map_err(|message| JsonRpcError {
                code: -32000,
                message,
            })?;

        self.commit_history_url(&document_url, commit, None);
        self.sync_history_length()?;
        if matches!(commit, NavigationCommit::Traverse(_)) {
            self.runtime
                .eval(&format!(
                    "__omoikane_commit_same_document_navigation({url:?}, 'popstate', {url:?})"
                ))
                .and_then(|_| self.runtime.run_jobs())
                .map_err(js_error)?;
        }

        self.emit(
            "Network.responseReceived",
            json!({
                "requestId": loader_id,
                "type": "Document",
                "response": {
                    "url": document_url,
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
        self.navigate_to(&url, NavigationCommit::Reload)?;
        self.drive_navigation_requests()?;
        Ok(json!({}))
    }

    /// Commits navigation requests queued by the active Runtime.
    ///
    /// Startup scripts in a newly installed Document may queue another
    /// navigation, so the new Runtime is checked again after every commit. The
    /// cap prevents a script redirect loop from blocking the embedding API.
    fn drive_navigation_requests(&mut self) -> Result<(), JsonRpcError> {
        const MAX_SCRIPT_NAVIGATIONS: usize = 32;
        for _ in 0..MAX_SCRIPT_NAVIGATIONS {
            self.runtime.run_until_idle().map_err(js_error)?;
            // Tasks record page-script failures rather than aborting the loop, so
            // a navigation is not lost to one broken script. Surface them here,
            // where document script errors are already reported.
            for error in self.runtime.take_task_errors() {
                eprintln!("[omoikane][js-error] {error}");
            }
            let Some(request) = self.runtime.take_navigation_requests().into_iter().next() else {
                return Ok(());
            };
            match request {
                NavigationRequest::Navigate { url, replace } => {
                    let previous_url = self.current_url.clone();
                    if let Err(error) = self.navigate_to(
                        &url,
                        if replace {
                            NavigationCommit::Replace
                        } else {
                            NavigationCommit::Push
                        },
                    ) {
                        self.restore_active_location(&previous_url);
                        return Err(error);
                    }
                }
                NavigationRequest::FormSubmit {
                    url,
                    method,
                    body,
                    content_type,
                } => {
                    let previous_url = self.current_url.clone();
                    let method = if method.eq_ignore_ascii_case("POST") {
                        Method::Post
                    } else {
                        Method::Get
                    };
                    if let Err(error) = self.navigate_with_request(
                        &url,
                        NavigationCommit::Push,
                        method,
                        body,
                        content_type,
                    ) {
                        self.restore_active_location(&previous_url);
                        return Err(error);
                    }
                }
                NavigationRequest::UpdateHistory {
                    url,
                    replace,
                    state_json,
                } => {
                    self.commit_history_url(
                        &url,
                        if replace {
                            NavigationCommit::Replace
                        } else {
                            NavigationCommit::Push
                        },
                        Some(state_json),
                    );
                    self.current_url = url.clone();
                    self.sync_history_length()?;
                    self.emit(
                        "Page.navigatedWithinDocument",
                        json!({ "frameId": self.frame_id, "url": url, "navigationType": "historyApi" }),
                    );
                }
                NavigationRequest::Reload => {
                    let url = self.current_url.clone();
                    self.navigate_to(&url, NavigationCommit::Reload)?;
                }
                NavigationRequest::Traverse { delta } => {
                    let target = self.history_index as i64 + i64::from(delta);
                    if target < 0 || target >= self.history_entries.len() as i64 {
                        continue;
                    }
                    let target = target as usize;
                    if target == self.history_index {
                        continue;
                    }
                    let url = self.history_entries[target].url.clone();
                    let previous_url = self.current_url.clone();
                    if let Err(error) =
                        self.navigate_to(&url, NavigationCommit::Traverse(target))
                    {
                        self.restore_active_location(&previous_url);
                        return Err(error);
                    }
                }
            }
        }
        Err(JsonRpcError {
            code: -32000,
            message: "script navigation limit exceeded".to_string(),
        })
    }

    fn commit_history_url(
        &mut self,
        url: &str,
        commit: NavigationCommit,
        state_json: Option<String>,
    ) {
        match commit {
            NavigationCommit::Push => {
                self.history_entries.truncate(self.history_index + 1);
                self.history_entries.push(SessionHistoryEntry {
                    url: url.to_string(),
                    state_json: state_json.unwrap_or_else(|| "null".to_string()),
                });
                self.history_index = self.history_entries.len() - 1;
            }
            NavigationCommit::Replace => {
                self.history_entries[self.history_index] = SessionHistoryEntry {
                    url: url.to_string(),
                    state_json: state_json.unwrap_or_else(|| "null".to_string()),
                };
            }
            NavigationCommit::Reload => {}
            NavigationCommit::Traverse(index) => self.history_index = index,
        }
    }

    fn prospective_history_state(&self, commit: NavigationCommit) -> (usize, String) {
        match commit {
            NavigationCommit::Push => (self.history_index + 2, "null".to_string()),
            NavigationCommit::Replace => (self.history_entries.len(), "null".to_string()),
            NavigationCommit::Reload => (
                self.history_entries.len(),
                self.history_entries[self.history_index].state_json.clone(),
            ),
            NavigationCommit::Traverse(index) => (
                self.history_entries.len(),
                self.history_entries[index].state_json.clone(),
            ),
        }
    }

    fn sync_history_length(&mut self) -> Result<(), JsonRpcError> {
        let state_json = &self.history_entries[self.history_index].state_json;
        self.runtime
            .eval(&format!(
                "__omoikane_sync_history({}, {state_json:?})",
                self.history_entries.len(),
            ))
            .and_then(|_| self.runtime.run_jobs())
            .map_err(js_error)
    }

    fn restore_active_location(&mut self, url: &str) {
        let _ = self
            .runtime
            .eval(&format!("__omoikane_set_location({url:?})"));
        let _ = self.runtime.run_jobs();
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

        let result = self.evaluate_expression(&expression, return_by_value)?;
        self.drive_navigation_requests()?;
        Ok(result)
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
        let result = self.evaluate_expression(&expression, return_by_value)?;
        self.drive_navigation_requests()?;
        Ok(result)
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
        let dom_type = match event_type.as_str() {
            "keyDown" | "rawKeyDown" | "keydown" => "keydown",
            "keyUp" | "keyup" => "keyup",
            "char" | "keypress" => "keypress",
            _ => {
                return Err(invalid_params(format!(
                    "Unsupported Input.dispatchKeyEvent type: {event_type}"
                )));
            }
        };
        let modifiers = params.get("modifiers").and_then(Value::as_u64).unwrap_or(0);
        let text = params.get("text").and_then(Value::as_str).unwrap_or("");
        let key = params
            .get("key")
            .and_then(Value::as_str)
            .or_else(|| (!text.is_empty()).then_some(text))
            .unwrap_or("");
        let key_code = params
            .get("windowsVirtualKeyCode")
            .or_else(|| params.get("nativeVirtualKeyCode"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let init = json!({
            "key": key,
            "text": text,
            "code": params.get("code").and_then(Value::as_str).unwrap_or(""),
            "keyCode": key_code,
            "charCode": if dom_type == "keypress" {
                text.chars().next().map(|character| character as u32).unwrap_or(0)
            } else { 0 },
            "repeat": params.get("autoRepeat").and_then(Value::as_bool).unwrap_or(false),
            "altKey": modifiers & 1 != 0,
            "ctrlKey": modifiers & 2 != 0,
            "metaKey": modifiers & 4 != 0,
            "shiftKey": modifiers & 8 != 0,
        });
        let not_canceled = self.eval_input_bool(&format!(
            "__omoikane_dispatch_keyboard_input({dom_type:?}, {init})"
        ))?;
        Ok(json!({ "defaultPrevented": !not_canceled }))
    }

    fn input_dispatch_mouse_event(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let event_type = require_string(params, "type")?;
        self.last_mouse_event = Some(params.clone());
        let dom_type = match event_type.as_str() {
            "mouseMoved" | "mousemove" => "mousemove",
            "mousePressed" | "mousedown" => "mousedown",
            "mouseReleased" | "mouseup" => "mouseup",
            "click" => "click",
            "mouseWheel" | "wheel" => "wheel",
            _ => {
                return Err(invalid_params(format!(
                    "Unsupported Input.dispatchMouseEvent type: {event_type}"
                )));
            }
        };
        let x = optional_f64(params, "x", 0.0)?;
        let y = optional_f64(params, "y", 0.0)?;
        let target = self.runtime.hit_test(x as f32, y as f32);
        let target_node = target.clone().unwrap_or_else(|| self.runtime.document());
        let target_id = target_node.identity();
        let target_node_id = self.ensure_node_id(&target_node);
        let button = mouse_button(params.get("button").and_then(Value::as_str))?;
        let modifiers = params.get("modifiers").and_then(Value::as_u64).unwrap_or(0);
        let default_buttons = if dom_type == "mousedown" { button_mask(button) } else { 0 };
        let buttons = params
            .get("buttons")
            .and_then(Value::as_u64)
            .unwrap_or(default_buttons);
        let pointer_id = params
            .get("pointerId")
            .and_then(Value::as_u64)
            .filter(|id| *id > 0)
            .unwrap_or(1);
        let (scroll_x, scroll_y) = self.runtime.window_scroll_offset();
        let mut init = json!({
            "clientX": x, "clientY": y,
            "pageX": x + scroll_x as f64, "pageY": y + scroll_y as f64,
            "screenX": x, "screenY": y,
            // CDP uses -1/"none" when no button changed, while MouseEvent.button
            // is a non-negative button index and defaults to the primary button.
            "button": button.max(0), "buttons": buttons,
            "altKey": modifiers & 1 != 0,
            "ctrlKey": modifiers & 2 != 0,
            "metaKey": modifiers & 4 != 0,
            "shiftKey": modifiers & 8 != 0,
            "detail": params.get("clickCount").and_then(Value::as_u64).unwrap_or(0),
            "pointerId": pointer_id,
        });
        if dom_type == "wheel" {
            init["deltaX"] = json!(optional_f64(params, "deltaX", 0.0)?);
            init["deltaY"] = json!(optional_f64(params, "deltaY", 0.0)?);
            init["deltaZ"] = json!(0);
            init["deltaMode"] = json!(0);
            let not_canceled = self.eval_input_bool(&format!(
                "__omoikane_dispatch_wheel_input({target_id}, {init})"
            ))?;
            return Ok(json!({
                "defaultPrevented": !not_canceled,
                "targetNodeId": target_node_id,
            }));
        }
        let not_canceled = self.eval_input_bool(&format!(
            "__omoikane_dispatch_mouse_input({target_id}, {dom_type:?}, {init}, {})",
            dom_type == "mousedown"
        ))?;
        if dom_type == "mousedown" {
            if self.drag_active {
                let _ = self.eval_input_bool(&format!(
                    "__omoikane_dispatch_drag_input(0, \"cancel\", {init})"
                ))?;
            }
            self.drag_active = false;
            self.drag_candidate = if not_canceled {
                self.eval_input_bool(&format!(
                    "__omoikane_prepare_drag_input({target_id}, {init})"
                ))?
            } else {
                false
            };
        }
        if dom_type == "mousemove"
            && buttons & button_mask(0) != 0
            && (self.drag_candidate || self.drag_active)
        {
            let active = self.eval_input_bool(&format!(
                "__omoikane_dispatch_drag_input({target_id}, \"move\", {init})"
            ))?;
            self.drag_active = active;
            if active {
                self.drag_candidate = false;
            } else if self.drag_candidate {
                // A canceled dragstart consumes the candidate and must not be
                // retried on every subsequent mousemove.
                self.drag_candidate = false;
            }
        }
        let mut click_default_prevented = false;
        let mut drag_consumed = false;
        if dom_type == "mousedown" {
            self.mouse_pressed_target = target.as_ref().map(NodeHandle::identity);
        } else if dom_type == "mouseup" {
            let was_drag = self.drag_active;
            let pressed = self.mouse_pressed_target.take();
            if was_drag || self.drag_candidate {
                drag_consumed = self.eval_input_bool(&format!(
                    "__omoikane_dispatch_drag_input({target_id}, \"end\", {init})"
                ))? || was_drag;
                self.drag_active = false;
                self.drag_candidate = false;
            }
            if !drag_consumed
                && pressed.is_some()
                && pressed == target.as_ref().map(NodeHandle::identity)
            {
                let click_not_canceled = self.eval_input_bool(&format!(
                    "__omoikane_dispatch_mouse_input({target_id}, \"click\", {init}, false)"
                ))?;
                click_default_prevented = !click_not_canceled;
            }
            let _ = self.eval_input_bool(&format!(
                "__omoikane_release_pointer_capture({})",
                pointer_id
            ))?;
        }
        Ok(json!({
            "defaultPrevented": !not_canceled,
            "clickDefaultPrevented": click_default_prevented,
            "targetNodeId": target_node_id,
        }))
    }

    fn eval_input_bool(&mut self, script: &str) -> Result<bool, JsonRpcError> {
        let not_canceled = self
            .runtime
            .eval(script)
            .and_then(|value| {
                self.runtime.run_jobs()?;
                Ok(value.as_boolean().unwrap_or(true))
            })
            .map_err(js_error)?;
        self.drive_navigation_requests()?;
        Ok(not_canceled)
    }

    fn evaluate_expression(
        &mut self,
        expression: &str,
        return_by_value: bool,
    ) -> Result<Value, JsonRpcError> {
        let value = self.runtime.eval(expression).map_err(js_error)?;
        // Runtime.evaluate is itself a user-agent task. Complete its
        // microtask checkpoint and make any host tasks (such as navigation)
        // ready before the protocol method commits them.
        self.runtime.run_until_idle().map_err(js_error)?;
        self.serialize_evaluation_value(value, return_by_value)
    }

    async fn evaluate_expression_async(
        &mut self,
        expression: &str,
        return_by_value: bool,
    ) -> Result<Value, JsonRpcError> {
        // Evaluate the requested source directly. Calling JavaScript `eval()`
        // from an async outer script would use Boa's synchronous nested-eval
        // path and reject a native-call suspension.
        let value = self.runtime.eval_async(expression).await.map_err(js_error)?;
        self.runtime.run_until_idle().map_err(js_error)?;
        let result = self.serialize_evaluation_value(value, return_by_value)?;
        self.drive_navigation_requests()?;
        Ok(result)
    }

    fn serialize_evaluation_value(
        &mut self,
        value: JsValue,
        return_by_value: bool,
    ) -> Result<Value, JsonRpcError> {
        let serialization_function = if return_by_value {
            "value => JSON.stringify({ result: __cdpSerializeValue(value) })".to_string()
        } else {
            let object_id = format!("__cdp_object_{}", self.next_object_id);
            self.next_object_id += 1;
            format!(
                "value => {{ globalThis[{object_id:?}] = value; return JSON.stringify({{ result: __cdpRemoteObject(value, {object_id:?}) }}); }}"
            )
        };
        let raw = self
            .runtime
            .call_function_with_value(&serialization_function, value)
            .map_err(js_error)?;
        // Match the synchronous Runtime.evaluate task boundary: serializer
        // getters/toJSON may enqueue microtasks that must settle before the
        // protocol response and any resulting navigation are committed.
        self.runtime.run_until_idle().map_err(js_error)?;
        let payload = raw
            .as_string()
            .ok_or(JsonRpcError {
                code: -32000,
                message: "Runtime evaluation did not return a string payload".to_string(),
            })?
            .to_std_string_escaped();
        let result = serde_json::from_str(&payload).map_err(|error| JsonRpcError {
            code: -32000,
            message: error.to_string(),
        })?;
        Ok(result)
    }

    fn load_page_request(
        &mut self,
        url: &str,
        method: Method,
        body: Option<Vec<u8>>,
        content_type: Option<String>,
    ) -> Result<(String, u16, String, Vec<String>), String> {
        if method == Method::Get {
            if url == "about:blank" {
                return Ok((
                    "<html><head></head><body></body></html>".to_string(),
                    200,
                    url.to_string(),
                    Vec::new(),
                ));
            }
            if let Some(data) = url.strip_prefix("data:text/html,") {
                return Ok((percent_decode(data), 200, url.to_string(), Vec::new()));
            }
        }
        let parsed: crate::http::url::Url = url
            .parse()
            .map_err(|error: crate::http::url::UrlParseError| error.to_string())?;
        let mut request = HttpRequest::new(method, parsed);
        if let Some(content_type) = content_type {
            request.set_header("Content-Type", content_type);
        }
        if let Some(body) = body {
            request.set_body(body);
        }
        let response = self
            .http_client
            .send(request)
            .map_err(|error| error.to_string())?;
        let effective_url = response
            .effective_url()
            .map(ToString::to_string)
            .unwrap_or_else(|| url.to_string());
        let csp_headers = response
            .headers()
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-security-policy"))
            .map(|(_, value)| value.clone())
            .collect();
        Ok((
            decode_html_response(&response),
            response.status_code(),
            effective_url,
            csp_headers,
        ))
    }

    #[cfg(test)]
    fn install_document(
        &mut self,
        url: &str,
        html: &str,
        history_length: usize,
        history_state_json: &str,
    ) -> Result<(), String> {
        self.install_document_with_csp(url, html, history_length, history_state_json, &[])
    }

    fn install_document_with_csp(
        &mut self,
        url: &str,
        html: &str,
        history_length: usize,
        history_state_json: &str,
        csp_headers: &[String],
    ) -> Result<(), String> {
        let document = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document_url_and_storage(
            document,
            url,
            self.storage_manager.clone(),
            self.storage_session_id,
        )
        .map_err(|error| error.to_string())?;
        runtime.set_user_agent(self.http_client.user_agent().to_string());
        Self::install_runtime_helpers_on(&mut runtime).map_err(js_error_message)?;
        runtime.install_csp_policy(csp_headers);
        runtime
            .eval(&format!(
                "__omoikane_sync_history({history_length}, {history_state_json:?})"
            ))
            .and_then(|_| runtime.run_jobs())
            .map_err(js_error_message)?;

        // The response and replacement Runtime are ready, so the active
        // Document can now leave without risking an unload followed by a
        // network failure that keeps the same Document alive. Listener errors
        // are reported like browser event-handler errors and do not cancel the
        // commit; cancellation policy for beforeunload dialogs belongs to the
        // future GUI integration.
        self.runtime.terminate_workers();
        let _ = self.runtime.eval(
            "window.dispatchEvent(new Event('beforeunload')); \
             window.dispatchEvent(new Event('pagehide')); \
             window.dispatchEvent(new Event('unload')); \
             if (typeof globalThis.__omoikane_permission_teardown === 'function') \
             globalThis.__omoikane_permission_teardown();",
        );
        let _ = self.runtime.run_jobs();

        let base_url = url.parse::<crate::http::Url>().ok();
        let script_errors = runtime.execute_document_scripts(base_url.as_ref());
        if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
            for error in script_errors {
                eprintln!("[omoikane][js-error] {error}");
            }
        }
        runtime
            .wire_inline_event_handlers()
            .map_err(js_error_message)?;
        runtime.fire_load().map_err(js_error_message)?;

        self.runtime = runtime;
        self.mouse_pressed_target = None;
        self.drag_candidate = false;
        self.drag_active = false;
        self.document_generation = self.document_generation.saturating_add(1);
        self.current_url = url.to_string();
        self.last_html = html.to_string();
        self.rebuild_node_index();
        Ok(())
    }

    /// Builds a replacement document and moves its runtime into a suspendable
    /// startup task without disturbing the currently committed page.
    pub(crate) fn prepare_document_page_task(
        &mut self,
        url: &str,
        html: &str,
        history_length: usize,
        history_state_json: &str,
        csp_headers: &[String],
    ) -> Result<(OwnedPageTask, PendingDocumentCommit), String> {
        let document = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document_url_and_storage(
            document,
            url,
            self.storage_manager.clone(),
            self.storage_session_id,
        )
        .map_err(|error| error.to_string())?;
        runtime.set_user_agent(self.http_client.user_agent().to_string());
        Self::install_runtime_helpers_on(&mut runtime).map_err(js_error_message)?;
        runtime.install_csp_policy(csp_headers);
        runtime
            .eval(&format!(
                "__omoikane_sync_history({history_length}, {history_state_json:?})"
            ))
            .and_then(|_| runtime.run_jobs())
            .map_err(js_error_message)?;

        let generation = self.document_generation.saturating_add(1);
        let base_url = url.parse::<crate::http::Url>().ok();
        let task = runtime.into_document_page_task(generation, base_url);
        Ok((
            task,
            PendingDocumentCommit {
                url: url.to_string(),
                html: html.to_string(),
                generation,
                history_commit: NavigationCommit::Replace,
                loader_id: String::new(),
                status: 0,
            },
        ))
    }

    /// Installs a runtime returned by a completed page-startup task.
    ///
    /// Cancelled and stale tasks leave the currently committed page unchanged.
    pub(crate) fn commit_document_page_task(
        &mut self,
        mut completed: CompletedPageTask,
        pending: PendingDocumentCommit,
    ) -> Result<(), String> {
        if completed.generation != pending.generation
            || pending.generation != self.document_generation.saturating_add(1)
        {
            return Err("stale page startup task completion".to_string());
        }
        if completed.result == Err(PageTaskError::Cancelled) {
            return Err("page startup task was cancelled".to_string());
        }

        let script_error_lines = take_page_task_script_error_lines(&mut completed);
        if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
            for line in script_error_lines {
                eprintln!("{line}");
            }
        }
        let runtime = completed.runtime;

        // Teardown is delayed until the replacement runtime has completed its
        // startup work, so cancellation or startup setup failure keeps the old
        // document intact.
        self.runtime.terminate_workers();
        let _ = self.runtime.eval(
            "window.dispatchEvent(new Event('beforeunload')); \
             window.dispatchEvent(new Event('pagehide')); \
             window.dispatchEvent(new Event('unload')); \
             if (typeof globalThis.__omoikane_permission_teardown === 'function') \
             globalThis.__omoikane_permission_teardown();",
        );
        let _ = self.runtime.run_jobs();

        self.runtime = runtime;
        self.mouse_pressed_target = None;
        self.drag_candidate = false;
        self.drag_active = false;
        self.document_generation = pending.generation;
        self.current_url = pending.url;
        self.last_html = pending.html;
        self.rebuild_node_index();
        let current_url = self.current_url.clone();
        self.commit_history_url(&current_url, pending.history_commit, None);
        self.emit(
            "Network.responseReceived",
            json!({
                "requestId": pending.loader_id,
                "type": "Document",
                "response": {
                    "url": self.current_url,
                    "status": pending.status,
                    "mimeType": "text/html",
                },
            }),
        );
        self.emit(
            "Page.frameNavigated",
            json!({ "frame": { "id": self.frame_id, "url": self.current_url, "mimeType": "text/html" } }),
        );
        self.emit(
            "Page.loadEventFired",
            json!({ "timestamp": self.next_loader_id }),
        );
        Ok(())
    }

    fn install_runtime_helpers(&mut self) -> Result<(), boa_engine::JsError> {
        Self::install_runtime_helpers_on(&mut self.runtime)
    }

    fn install_runtime_helpers_on(runtime: &mut JsRuntime) -> Result<(), boa_engine::JsError> {
        runtime.eval(
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
        runtime.run_jobs()
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
        // Node ids are session-scoped remote handles. Never reuse an id after
        // navigation: a client retaining an old Document's id must receive an
        // unknown-node error, not accidentally address a similarly positioned
        // node in the newly installed Document.
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

type SessionEvaluation = Pin<
    Box<dyn Future<Output = (CdpSession, Result<Value, JsonRpcError>)>>,
>;

struct PendingSessionEvaluation {
    token: DeferredResponseToken,
    controller: JavaScriptDialogController,
    cancelled: Rc<Cell<bool>>,
    page_url: String,
    opened: Option<JavaScriptDialog>,
    future: SessionEvaluation,
}

struct PendingPageNavigation {
    token: DeferredResponseToken,
    controller: JavaScriptDialogController,
    page_url: String,
    opened: Option<JavaScriptDialog>,
    task: Pin<Box<OwnedPageTask>>,
    commit: PendingDocumentCommit,
    response: Value,
}

enum BrowserSessionAction {
    Notify(&'static str, Value),
    Complete(DeferredResponseToken, Result<Value, JsonRpcError>),
}

struct BrowserSessionState {
    session: Option<CdpSession>,
    pending: Option<PendingSessionEvaluation>,
    pending_page: Option<PendingPageNavigation>,
    actions: Vec<BrowserSessionAction>,
}

fn page_task_script_error_lines(
    result: &Result<Vec<String>, PageTaskError>,
) -> Vec<String> {
    result
        .as_ref()
        .map(|errors| {
            errors
                .iter()
                .map(|error| format!("[omoikane][js-error] {error}"))
                .collect()
        })
        .unwrap_or_default()
}

fn take_page_task_script_error_lines(completed: &mut CompletedPageTask) -> Vec<String> {
    let mut lines = page_task_script_error_lines(&completed.result);
    lines.extend(
        completed
            .runtime
            .take_task_errors()
            .into_iter()
            .map(|error| format!("[omoikane][js-error] {error}")),
    );
    lines
}

fn pending_evaluation_busy_message(
    has_pending_evaluation: bool,
    has_pending_page: bool,
) -> Option<&'static str> {
    if has_pending_evaluation {
        Some("A Runtime.evaluate request is already pending")
    } else if has_pending_page {
        Some("Browser session is busy with a page navigation")
    } else {
        None
    }
}

impl BrowserSessionState {
    fn begin_evaluation(
        &mut self,
        token: DeferredResponseToken,
        params: &Value,
    ) -> Result<CdpMethodResult, JsonRpcError> {
        if let Some(message) = pending_evaluation_busy_message(
            self.pending.is_some(),
            self.pending_page.is_some(),
        ) {
            return Err(JsonRpcError {
                code: -32000,
                message: message.to_string(),
            });
        }
        let expression = require_string(params, "expression")?;
        let return_by_value = params
            .get("returnByValue")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let mut session = self.session.take().ok_or(JsonRpcError {
            code: -32000,
            message: "Browser session is busy".to_string(),
        })?;
        // The controller belongs only to this in-flight evaluation. It is
        // discarded together with the evaluation before a replacement Runtime
        // is installed, so dialog ids reused after navigation cannot resolve an
        // older suspension.
        let controller = session.runtime.javascript_dialog_controller();
        let page_url = session.current_url.clone();
        let cancelled = Rc::new(Cell::new(false));
        let evaluation_cancelled = Rc::clone(&cancelled);
        let future = Box::pin(async move {
            let result = {
                let mut evaluation = Box::pin(
                    session.evaluate_expression_async(&expression, return_by_value),
                );
                std::future::poll_fn(|context| {
                    if evaluation_cancelled.get() {
                        return Poll::Ready(Err(JsonRpcError {
                            code: -32000,
                            message: "JavaScript evaluation cancelled by page teardown".to_string(),
                        }));
                    }
                    evaluation.as_mut().poll(context)
                })
                .await
            };
            (session, result)
        });
        self.pending = Some(PendingSessionEvaluation {
            token,
            controller,
            cancelled,
            page_url,
            opened: None,
            future,
        });
        self.poll_evaluation();
        // Even immediate completion is flushed out-of-band so CdpServer can
        // first record the original client and request id for this token.
        Ok(CdpMethodResult::Deferred)
    }

    fn begin_page_navigation(
        &mut self,
        token: DeferredResponseToken,
        method: &str,
        params: &Value,
    ) -> Result<CdpMethodResult, JsonRpcError> {
        self.cancel_pending_dialog();
        if self.pending.is_some() || self.pending_page.is_some() {
            return Err(JsonRpcError {
                code: -32000,
                message: "Browser session is busy".to_string(),
            });
        }
        let session = self.session.as_mut().ok_or(JsonRpcError {
            code: -32000,
            message: "Browser session is unavailable".to_string(),
        })?;
        let prepared = if method == "Page.reload" {
            session.prepare_page_reload()?
        } else {
            session.prepare_page_navigate(params)?
        };
        let (task, commit, response) = match prepared {
            PreparedPageNavigation::Complete(response) => {
                return Ok(CdpMethodResult::Complete(response));
            }
            PreparedPageNavigation::Pending {
                task,
                commit,
                response,
            } => (task, commit, response),
        };
        let controller = task.dialog_controller();
        let page_url = commit.url.clone();
        self.pending_page = Some(PendingPageNavigation {
            token,
            controller,
            page_url,
            opened: None,
            task: Box::pin(task),
            commit,
            response,
        });
        self.poll_page_navigation();
        Ok(CdpMethodResult::Deferred)
    }

    fn poll_page_navigation(&mut self) {
        for _ in 0..64 {
            let Some(mut pending) = self.pending_page.take() else {
                return;
            };
            if pending.opened.is_some() {
                self.pending_page = Some(pending);
                return;
            }
            let waker: &'static Waker = Waker::noop();
            let mut context = TaskContext::from_waker(waker);
            match pending.task.as_mut().poll(&mut context) {
                Poll::Ready(completed) => {
                    let result = self
                        .session
                        .as_mut()
                        .ok_or_else(|| "Browser session is unavailable".to_string())
                        .and_then(|session| {
                            session.commit_document_page_task(completed, pending.commit)
                        })
                        .map(|()| pending.response)
                        .map_err(|message| JsonRpcError {
                            code: -32000,
                            message,
                        });
                    self.actions
                        .push(BrowserSessionAction::Complete(pending.token, result));
                    return;
                }
                Poll::Pending => {
                    if let Some(dialog) = pending.controller.pending() {
                        self.actions.push(BrowserSessionAction::Notify(
                            "Page.javascriptDialogOpening",
                            dialog_opening_params(&dialog, &pending.page_url),
                        ));
                        pending.opened = Some(dialog);
                        self.pending_page = Some(pending);
                        return;
                    }
                    self.pending_page = Some(pending);
                }
            }
        }
    }

    fn poll_evaluation(&mut self) {
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        let waker: &'static Waker = Waker::noop();
        let mut context = TaskContext::from_waker(waker);
        match pending.future.as_mut().poll(&mut context) {
            Poll::Ready((session, result)) => {
                self.session = Some(session);
                self.actions
                    .push(BrowserSessionAction::Complete(pending.token, result));
            }
            Poll::Pending => {
                if pending.opened.is_none()
                    && let Some(dialog) = pending.controller.pending()
                {
                    self.actions.push(BrowserSessionAction::Notify(
                        "Page.javascriptDialogOpening",
                        dialog_opening_params(&dialog, &pending.page_url),
                    ));
                    pending.opened = Some(dialog);
                }
                self.pending = Some(pending);
            }
        }
    }

    fn handle_dialog(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let accept = params
            .get("accept")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                invalid_params("Missing or invalid boolean parameter: accept".to_string())
            })?;
        let prompt_text = params
            .get("promptText")
            .map(|value| {
                value.as_str().map(ToString::to_string).ok_or_else(|| {
                    invalid_params("Invalid string parameter: promptText".to_string())
                })
            })
            .transpose()?;
        let controller = self
            .pending
            .as_ref()
            .map(|pending| pending.controller.clone())
            .or_else(|| {
                self.pending_page
                    .as_ref()
                    .map(|pending| pending.controller.clone())
            })
            .ok_or(JsonRpcError {
                code: -32000,
                message: "No JavaScript dialog is open".to_string(),
            })?;
        let dialog = controller.pending().ok_or(JsonRpcError {
            code: -32000,
            message: "No JavaScript dialog is open".to_string(),
        })?;
        controller
            .handle(dialog.id, accept, prompt_text.clone())
            .map_err(dialog_error)?;
        let user_input = if dialog.kind == JavaScriptDialogKind::Prompt && accept {
            prompt_text
                .or(dialog.default_prompt.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        self.actions.push(BrowserSessionAction::Notify(
            "Page.javascriptDialogClosed",
            json!({ "result": accept, "userInput": user_input }),
        ));
        if let Some(pending) = self.pending.as_mut() {
            pending.opened = None;
            self.poll_evaluation();
        } else if let Some(pending) = self.pending_page.as_mut() {
            pending.opened = None;
            self.poll_page_navigation();
        }
        Ok(json!({}))
    }

    fn cancel_pending_page(&mut self) {
        let Some(pending) = self.pending_page.as_mut() else {
            return;
        };
        if pending.controller.pending().is_some() {
            self.actions.push(BrowserSessionAction::Notify(
                "Page.javascriptDialogClosed",
                json!({ "result": false, "userInput": "" }),
            ));
        }
        pending.opened = None;
        pending.task.cancel();
        self.poll_page_navigation();
    }

    fn cancel_pending_dialog(&mut self) {
        self.cancel_pending_page();
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        if pending.controller.pending().is_some() {
            self.actions.push(BrowserSessionAction::Notify(
                "Page.javascriptDialogClosed",
                json!({ "result": false, "userInput": "" }),
            ));
        }
        pending.cancelled.set(true);
        self.poll_evaluation();
    }

}

/// CDP transport and page-session coordinator with deferred modal-dialog support.
///
/// `Runtime.evaluate` remains pending while `alert`, `confirm`, or `prompt`
/// blocks JavaScript. Other transport commands continue to be routed, and
/// navigation, reload, or owner-client disconnect dismisses the active dialog
/// before the old evaluation state is discarded.
pub struct BrowserSession {
    server: CdpServer,
    state: Rc<RefCell<BrowserSessionState>>,
    /// The first attached CDP client owns the page runtime. A secondary
    /// observer disconnect must not cancel the owner's dialogs or workers.
    owner_client_id: Option<u64>,
}

impl BrowserSession {
    pub fn new() -> Result<Self, String> {
        let state = Rc::new(RefCell::new(BrowserSessionState {
            session: Some(CdpSession::new()?),
            pending: None,
            pending_page: None,
            actions: Vec::new(),
        }));
        let mut server = CdpServer::new();

        let evaluation_state = Rc::clone(&state);
        server.register_deferred_method("Runtime.evaluate", move |token, params| {
            evaluation_state.borrow_mut().begin_evaluation(token, params)
        });
        let dialog_state = Rc::clone(&state);
        server.register_method("Page.handleJavaScriptDialog", move |params| {
            dialog_state.borrow_mut().handle_dialog(params)
        });
        for method in ["Page.navigate", "Page.reload"] {
            let method_state = Rc::clone(&state);
            server.register_deferred_method(method, move |token, params| {
                method_state
                    .borrow_mut()
                    .begin_page_navigation(token, method, params)
            });
        }
        for method in [
            "Page.getFrameTree",
            "DOM.getDocument",
            "DOM.getAttributes",
            "DOM.querySelector",
            "DOM.getOuterHTML",
            "Runtime.callFunctionOn",
            "Target.createBrowserContext",
            "Target.getBrowserContexts",
            "Target.disposeBrowserContext",
            "Input.dispatchKeyEvent",
            "Input.dispatchMouseEvent",
        ] {
            let method_state = Rc::clone(&state);
            server.register_method(method, move |params| {
                let mut state = method_state.borrow_mut();
                let session = state.session.as_mut().ok_or(JsonRpcError {
                    code: -32000,
                    message: "Page state is suspended by a JavaScript dialog".to_string(),
                })?;
                session.dispatch(method, params.clone())
            });
        }
        server.register_method("Browser.getVersion", |_| {
            Ok(json!({ "product": "Omoikane/0.1", "protocolVersion": "1.3" }))
        });

        Ok(Self {
            server,
            state,
            owner_client_id: None,
        })
    }

    pub fn accept_upgrade(&mut self, request: &str) -> Result<WebSocketUpgrade, CdpError> {
        let upgrade = self.server.accept_upgrade(request)?;
        if self.owner_client_id.is_none() {
            self.owner_client_id = Some(upgrade.client_id);
        }
        Ok(upgrade)
    }

    pub fn receive(&mut self, client_id: u64, bytes: &[u8]) -> Result<(), CdpError> {
        let owner_disconnect = WebSocketFrame::decode(bytes)
            .ok()
            .is_some_and(|(frame, _)| {
                frame.opcode == WebSocketOpcode::Close
                    && self.owner_client_id == Some(client_id)
            });
        if owner_disconnect {
            let mut state = self.state.borrow_mut();
            state.cancel_pending_dialog();
            if let Some(session) = state.session.as_mut() {
                session.runtime.terminate_workers();
                let _ = session.runtime.eval(
                    "if (typeof globalThis.__omoikane_permission_teardown === 'function') \
                     globalThis.__omoikane_permission_teardown();",
                );
            }
        }
        self.server.receive(client_id, bytes)?;
        if owner_disconnect {
            self.owner_client_id = None;
        }
        self.flush_actions()
    }

    pub fn drain_outgoing(
        &mut self,
        client_id: u64,
    ) -> Result<Vec<WebSocketFrame>, CdpError> {
        self.flush_actions()?;
        self.server.drain_outgoing(client_id)
    }

    pub fn pending_response_count(&self) -> usize {
        self.server.pending_response_count()
    }

    fn flush_actions(&mut self) -> Result<(), CdpError> {
        self.state.borrow_mut().poll_page_navigation();
        let actions = std::mem::take(&mut self.state.borrow_mut().actions);
        for action in actions {
            match action {
                BrowserSessionAction::Notify(method, params) => {
                    self.server.notify(method, params)?;
                }
                BrowserSessionAction::Complete(token, result) => {
                    // Disconnect already removed the response routing record.
                    if self.server.deferred_response_client(token).is_some() {
                        self.server.complete_deferred_response(token, result)?;
                    }
                }
            }
        }
        if let Some(session) = self.state.borrow_mut().session.as_mut() {
            for event in session.drain_events() {
                self.server.notify(&event.method, event.params)?;
            }
        }
        Ok(())
    }
}

fn dialog_opening_params(dialog: &JavaScriptDialog, page_url: &str) -> Value {
    let kind = match dialog.kind {
        JavaScriptDialogKind::Alert => "alert",
        JavaScriptDialogKind::Confirm => "confirm",
        JavaScriptDialogKind::Prompt => "prompt",
    };
    json!({
        "url": page_url,
        "type": kind,
        "message": dialog.message,
        "hasBrowserHandler": true,
        "defaultPrompt": dialog.default_prompt.clone().unwrap_or_default(),
    })
}

fn dialog_error(error: JavaScriptDialogError) -> JsonRpcError {
    JsonRpcError {
        code: -32000,
        message: error.to_string(),
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

fn invalid_params(message: String) -> JsonRpcError {
    JsonRpcError { code: -32602, message }
}

fn optional_f64(params: &Value, key: &'static str, default: f64) -> Result<f64, JsonRpcError> {
    let value = params.get(key).map(Value::as_f64).unwrap_or(Some(default));
    value.filter(|value| value.is_finite()).ok_or_else(|| {
        invalid_params(format!("Missing or invalid numeric parameter: {key}"))
    })
}

fn mouse_button(button: Option<&str>) -> Result<i32, JsonRpcError> {
    match button.unwrap_or("none") {
        "none" => Ok(-1),
        "left" => Ok(0),
        "middle" => Ok(1),
        "right" => Ok(2),
        "back" => Ok(3),
        "forward" => Ok(4),
        value => Err(invalid_params(format!("Unsupported mouse button: {value}"))),
    }
}

fn button_mask(button: i32) -> u64 {
    match button {
        0 => 1,
        1 => 4,
        2 => 2,
        3 => 8,
        4 => 16,
        _ => 0,
    }
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
        NodeType::ProcessingInstruction => {
            let data = node.data().unwrap_or_default();
            if data.is_empty() {
                format!("<?{}?>", node.node_name())
            } else {
                format!("<?{} {}?>", node.node_name(), data)
            }
        }
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

fn is_fragment_only_navigation(current: &str, target: &str) -> bool {
    let (current_base, current_fragment) = current.split_once('#').unwrap_or((current, ""));
    let (target_base, target_fragment) = target.split_once('#').unwrap_or((target, ""));
    current_base == target_base && current_fragment != target_fragment
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
    use std::cell::RefCell;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::rc::Rc;
    use std::thread;

    #[test]
    fn evaluation_busy_message_distinguishes_navigation_from_evaluation() {
        assert_eq!(
            pending_evaluation_busy_message(true, false),
            Some("A Runtime.evaluate request is already pending")
        );
        assert_eq!(
            pending_evaluation_busy_message(false, true),
            Some("Browser session is busy with a page navigation")
        );
    }

    #[test]
    fn completed_page_task_errors_are_formatted_for_script_logging() {
        assert_eq!(
            page_task_script_error_lines(&Ok(vec!["startup failed".to_string()])),
            ["[omoikane][js-error] startup failed"]
        );
        assert!(page_task_script_error_lines(&Err(PageTaskError::Cancelled)).is_empty());
    }

    #[test]
    fn completed_page_task_drains_startup_timer_errors_once() {
        let runtime = JsRuntime::new().unwrap();
        let mut task = Box::pin(runtime.into_page_task(
            1,
            vec![PageTaskSource::Classic {
                source: "setTimeout(() => { throw new Error('startup timer failed') }, 0)"
                    .to_string(),
                label: "startup".to_string(),
                script_node_id: None,
            }],
        ));
        let waker = Waker::noop();
        let mut context = TaskContext::from_waker(waker);
        let mut completed = loop {
            if let Poll::Ready(completed) = task.as_mut().poll(&mut context) {
                break completed;
            }
        };

        let lines = take_page_task_script_error_lines(&mut completed);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("[omoikane][js-error] "));
        assert!(lines[0].contains("startup timer failed"));
        assert!(take_page_task_script_error_lines(&mut completed).is_empty());
    }

    #[test]
    fn page_navigation_reports_an_unavailable_session_distinctly_from_busy() {
        let mut state = BrowserSessionState {
            session: None,
            pending: None,
            pending_page: None,
            actions: Vec::new(),
        };

        let error = state
            .begin_page_navigation(DeferredResponseToken(1), "Page.reload", &json!({}))
            .unwrap_err();
        assert_eq!(error.code, -32000);
        assert_eq!(error.message, "Browser session is unavailable");
    }

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
    fn deferred_response_keeps_processing_commands_and_preserves_client_and_id() {
        let mut server = CdpServer::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let handler_tokens = captured.clone();
        server.register_deferred_method("Runtime.evaluate", move |token, _| {
            handler_tokens.borrow_mut().push(token);
            Ok(CdpMethodResult::Deferred)
        });
        server.register_method("Page.handleJavaScriptDialog", |_| {
            Ok(json!({ "handled": true }))
        });
        let first = server.accept_upgrade(sample_upgrade_request()).unwrap();
        let second = server.accept_upgrade(sample_upgrade_request()).unwrap();

        let evaluate = WebSocketFrame::text(
            r#"{"jsonrpc":"2.0","id":"eval-1","method":"Runtime.evaluate","params":{}}"#,
        )
        .encode(true);
        server.receive(first.client_id, &evaluate).unwrap();
        assert_eq!(server.pending_response_count(), 1);
        assert!(server.drain_outgoing(first.client_id).unwrap().is_empty());

        server
            .notify(
                "Page.javascriptDialogOpening",
                json!({ "message": "Continue?", "type": "confirm" }),
            )
            .unwrap();
        for client_id in [first.client_id, second.client_id] {
            let event = server.drain_outgoing(client_id).unwrap();
            assert_eq!(event.len(), 1);
            let payload: Value = serde_json::from_str(&decode_text(&event[0])).unwrap();
            assert_eq!(payload["method"], "Page.javascriptDialogOpening");
        }

        let handle = WebSocketFrame::text(
            r#"{"jsonrpc":"2.0","id":42,"method":"Page.handleJavaScriptDialog","params":{"accept":true}}"#,
        )
        .encode(true);
        server.receive(first.client_id, &handle).unwrap();
        let immediate = server.drain_outgoing(first.client_id).unwrap();
        let payload: Value = serde_json::from_str(&decode_text(&immediate[0])).unwrap();
        assert_eq!(payload["id"], 42);
        assert_eq!(payload["result"]["handled"], true);
        assert_eq!(server.pending_response_count(), 1);

        let token = captured.borrow()[0];
        server
            .complete_deferred_response(token, Ok(json!({ "result": { "value": true } })))
            .unwrap();
        let completed = server.drain_outgoing(first.client_id).unwrap();
        let payload: Value = serde_json::from_str(&decode_text(&completed[0])).unwrap();
        assert_eq!(payload["id"], "eval-1");
        assert_eq!(payload["result"]["result"]["value"], true);
        assert_eq!(server.pending_response_count(), 0);
        assert_eq!(
            server.complete_deferred_response(token, Ok(Value::Null)),
            Err(CdpError::UnknownDeferredRequest(token.0))
        );
    }

    fn browser_request(session: &mut BrowserSession, client_id: u64, payload: &str) {
        session
            .receive(client_id, &WebSocketFrame::text(payload).encode(true))
            .unwrap();
    }

    fn browser_payloads(session: &mut BrowserSession, client_id: u64) -> Vec<Value> {
        session
            .drain_outgoing(client_id)
            .unwrap()
            .iter()
            .map(|frame| serde_json::from_str(&decode_text(frame)).unwrap())
            .collect()
    }

    #[test]
    fn browser_session_forwards_existing_dom_commands() {
        let mut session = BrowserSession::new().unwrap();
        let client = session.accept_upgrade(sample_upgrade_request()).unwrap();

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"document","method":"DOM.getDocument","params":{"depth":1}}"#,
        );
        let response = browser_payloads(&mut session, client.client_id);
        assert_eq!(response.len(), 1);
        assert_eq!(response[0]["id"], "document");
        assert_eq!(response[0]["result"]["root"]["nodeName"], "#document");
        assert!(response[0].get("error").is_none());
    }

    #[test]
    fn busy_forwarded_command_does_not_cancel_the_pending_dialog() {
        let mut session = BrowserSession::new().unwrap();
        let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"eval","method":"Runtime.evaluate","params":{"expression":"confirm('Still pending?')"}}"#,
        );
        browser_payloads(&mut session, client.client_id);

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"dom","method":"DOM.getDocument","params":{}}"#,
        );
        let busy = browser_payloads(&mut session, client.client_id);
        assert_eq!(busy.len(), 1);
        assert_eq!(busy[0]["id"], "dom");
        assert_eq!(busy[0]["error"]["code"], -32000);
        assert_eq!(session.pending_response_count(), 1);

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"handle","method":"Page.handleJavaScriptDialog","params":{"accept":true}}"#,
        );
        let completed = browser_payloads(&mut session, client.client_id);
        assert!(completed.iter().any(|value| {
            value["method"] == "Page.javascriptDialogClosed"
                && value["params"]["result"] == true
        }));
        assert!(completed.iter().any(|value| {
            value["id"] == "eval" && value["result"]["result"]["value"] == true
        }));
        assert_eq!(session.pending_response_count(), 0);
    }

    #[test]
    fn browser_session_round_trips_confirm_while_serving_another_command() {
        let mut session = BrowserSession::new().unwrap();
        let client = session.accept_upgrade(sample_upgrade_request()).unwrap();

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"eval","method":"Runtime.evaluate","params":{"expression":"confirm('Continue?')","returnByValue":true}}"#,
        );
        let opening = browser_payloads(&mut session, client.client_id);
        assert_eq!(session.pending_response_count(), 1, "{opening:?}");
        assert_eq!(opening.len(), 1);
        assert_eq!(opening[0]["method"], "Page.javascriptDialogOpening");
        assert_eq!(opening[0]["params"]["type"], "confirm");
        assert_eq!(opening[0]["params"]["message"], "Continue?");
        assert_eq!(opening[0]["params"]["defaultPrompt"], "");
        assert_eq!(opening[0]["params"]["url"], "about:blank");

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"version","method":"Browser.getVersion","params":{}}"#,
        );
        let version = browser_payloads(&mut session, client.client_id);
        assert_eq!(version[0]["id"], "version");
        assert_eq!(version[0]["result"]["product"], "Omoikane/0.1");
        assert_eq!(session.pending_response_count(), 1);

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"handle","method":"Page.handleJavaScriptDialog","params":{"accept":false}}"#,
        );
        let completed = browser_payloads(&mut session, client.client_id);
        assert_eq!(completed[0]["id"], "handle");
        assert_eq!(completed[1]["method"], "Page.javascriptDialogClosed");
        assert_eq!(completed[1]["params"]["result"], false);
        assert_eq!(completed[2]["id"], "eval");
        assert_eq!(completed[2]["result"]["result"]["value"], false);
        assert_eq!(session.pending_response_count(), 0);
    }

    #[test]
    fn browser_session_emits_each_sequential_dialog_opening() {
        let mut session = BrowserSession::new().unwrap();
        let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"eval","method":"Runtime.evaluate","params":{"expression":"alert('First'); confirm('Second')"}}"#,
        );
        let first = browser_payloads(&mut session, client.client_id);
        assert_eq!(first[0]["method"], "Page.javascriptDialogOpening");
        assert_eq!(first[0]["params"]["type"], "alert");
        assert_eq!(first[0]["params"]["message"], "First");

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"first-handle","method":"Page.handleJavaScriptDialog","params":{"accept":true}}"#,
        );
        let between = browser_payloads(&mut session, client.client_id);
        assert!(between.iter().any(|value| {
            value["method"] == "Page.javascriptDialogClosed"
                && value["params"]["result"] == true
        }));
        assert!(between.iter().any(|value| {
            value["method"] == "Page.javascriptDialogOpening"
                && value["params"]["type"] == "confirm"
                && value["params"]["message"] == "Second"
        }));
        assert_eq!(session.pending_response_count(), 1);

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"second-handle","method":"Page.handleJavaScriptDialog","params":{"accept":false}}"#,
        );
        let completed = browser_payloads(&mut session, client.client_id);
        assert!(completed.iter().any(|value| {
            value["method"] == "Page.javascriptDialogClosed"
                && value["params"]["result"] == false
        }));
        assert!(completed.iter().any(|value| {
            value["id"] == "eval" && value["result"]["result"]["value"] == false
        }));
        assert_eq!(session.pending_response_count(), 0);
    }

    #[test]
    fn resumed_async_evaluation_commits_queued_location_navigation() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for body in ["<title>Start</title>", "<title>Async next</title>"] {
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
        let mut session = BrowserSession::new().unwrap();
        let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
        let start_url = format!("http://127.0.0.1:{}/start", address.port());
        let next_url = format!("http://127.0.0.1:{}/next", address.port());
        browser_request(
            &mut session,
            client.client_id,
            &json!({
                "jsonrpc": "2.0",
                "id": "navigate",
                "method": "Page.navigate",
                "params": { "url": start_url },
            })
            .to_string(),
        );
        browser_payloads(&mut session, client.client_id);
        browser_request(
            &mut session,
            client.client_id,
            &json!({
                "jsonrpc": "2.0",
                "id": "eval",
                "method": "Runtime.evaluate",
                "params": {
                    "expression": format!(
                        "confirm('Leave?'); location.href = {next_url:?}; 'navigating'"
                    ),
                },
            })
            .to_string(),
        );
        browser_payloads(&mut session, client.client_id);
        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"handle","method":"Page.handleJavaScriptDialog","params":{"accept":true}}"#,
        );
        let completed = browser_payloads(&mut session, client.client_id);
        assert!(completed.iter().any(|value| {
            value["id"] == "eval"
                && value["result"]["result"]["value"] == "navigating"
        }), "{completed:?}");
        assert!(completed.iter().any(|value| {
            value["method"] == "Page.frameNavigated"
                && value["params"]["frame"]["url"] == next_url
        }), "{completed:?}");

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"tree","method":"Page.getFrameTree","params":{}}"#,
        );
        let tree = browser_payloads(&mut session, client.client_id);
        assert_eq!(tree[0]["result"]["frameTree"]["frame"]["url"], next_url);
        server.join().unwrap();
    }

    #[test]
    fn async_remote_objects_match_sync_serialization_for_functions_and_edge_objects() {
        for (index, (expression, return_by_value)) in [
            ("(function namedEdge(value) { return value; })", true),
            ("({ kept: 1, omitted: undefined, nested: [NaN, null] })", true),
            ("(function referenced(value) { return value; })", false),
            ("null", false),
        ]
        .into_iter()
        .enumerate()
        {
            let params = json!({
                "expression": expression,
                "returnByValue": return_by_value,
            });
            let mut direct = CdpSession::new().unwrap();
            let expected = direct.dispatch("Runtime.evaluate", params.clone()).unwrap();

            let mut browser = BrowserSession::new().unwrap();
            let client = browser.accept_upgrade(sample_upgrade_request()).unwrap();
            browser_request(
                &mut browser,
                client.client_id,
                &json!({
                    "jsonrpc": "2.0",
                    "id": index,
                    "method": "Runtime.evaluate",
                    "params": params,
                })
                .to_string(),
            );
            let response = browser_payloads(&mut browser, client.client_id);
            assert_eq!(response.len(), 1);
            assert_eq!(response[0]["result"], expected, "expression: {expression}");
        }

        let mut browser = BrowserSession::new().unwrap();
        let client = browser.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut browser,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"null","method":"Runtime.evaluate","params":{"expression":"null","returnByValue":false}}"#,
        );
        let response = browser_payloads(&mut browser, client.client_id);
        let remote = &response[0]["result"]["result"];
        assert_eq!(remote["type"], "object");
        assert_eq!(remote["subtype"], "null");
        assert_eq!(remote["value"], Value::Null);
        assert!(remote.get("objectId").is_none());
    }

    #[test]
    fn async_serializer_microtasks_settle_before_the_evaluate_response() {
        let mut browser = BrowserSession::new().unwrap();
        let client = browser.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut browser,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"serialize","method":"Runtime.evaluate","params":{"expression":"({ toJSON() { queueMicrotask(() => globalThis.serializerMicrotask = 'done'); return { ok: true }; } })","returnByValue":true}}"#,
        );
        let serialized = browser_payloads(&mut browser, client.client_id);
        assert_eq!(serialized.len(), 1);
        assert_eq!(serialized[0]["id"], "serialize");
        assert_eq!(serialized[0]["result"]["result"]["value"]["ok"], true);

        browser_request(
            &mut browser,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"observe","method":"Runtime.evaluate","params":{"expression":"globalThis.serializerMicrotask","returnByValue":true}}"#,
        );
        let observed = browser_payloads(&mut browser, client.client_id);
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0]["id"], "observe");
        assert_eq!(observed[0]["result"]["result"]["value"], "done");
    }

    #[test]
    fn sync_and_async_raw_source_execution_share_scope_completion_and_serialization() {
        let mut direct = CdpSession::new().unwrap();
        let mut browser = BrowserSession::new().unwrap();
        let client = browser.accept_upgrade(sample_upgrade_request()).unwrap();
        let cases = [
            ("var sharedVar = 7; sharedVar", true),
            ("sharedVar", true),
            ("function sharedFunction(value) { return value * 2; }", true),
            ("sharedFunction(4)", true),
            ("1; 2; 3", true),
            (
                "({ toJSON() { queueMicrotask(() => globalThis.sharedSerializerOrder = 'settled'); return { ok: true }; } })",
                true,
            ),
            ("sharedSerializerOrder", true),
            ("(function referenced(value) { return value; })", false),
        ];

        for (index, (expression, return_by_value)) in cases.into_iter().enumerate() {
            let params = json!({
                "expression": expression,
                "returnByValue": return_by_value,
            });
            let expected = direct.dispatch("Runtime.evaluate", params.clone()).unwrap();
            browser_request(
                &mut browser,
                client.client_id,
                &json!({
                    "jsonrpc": "2.0",
                    "id": index,
                    "method": "Runtime.evaluate",
                    "params": params,
                })
                .to_string(),
            );
            let response = browser_payloads(&mut browser, client.client_id);
            assert_eq!(response.len(), 1);
            assert_eq!(response[0]["result"], expected, "expression: {expression}");
        }

        assert_eq!(
            direct
                .dispatch(
                    "Runtime.evaluate",
                    json!({ "expression": "typeof sharedFunction", "returnByValue": true }),
                )
                .unwrap()["result"]["value"],
            "function"
        );
    }

    #[test]
    fn browser_session_passes_prompt_text_to_the_suspended_evaluation() {
        let mut session = BrowserSession::new().unwrap();
        let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":7,"method":"Runtime.evaluate","params":{"expression":"prompt('Name', 'Ada')","returnByValue":true}}"#,
        );
        let opening = browser_payloads(&mut session, client.client_id);
        assert_eq!(opening[0]["params"]["type"], "prompt");
        assert_eq!(opening[0]["params"]["defaultPrompt"], "Ada");

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":8,"method":"Page.handleJavaScriptDialog","params":{"accept":true,"promptText":"Grace"}}"#,
        );
        let completed = browser_payloads(&mut session, client.client_id);
        assert_eq!(completed[1]["params"]["userInput"], "Grace");
        assert_eq!(completed[2]["id"], 7);
        assert_eq!(completed[2]["result"]["result"]["value"], "Grace");
    }

    #[test]
    fn navigation_pumps_startup_and_load_dialogs_before_responding() {
        let mut session = BrowserSession::new().unwrap();
        let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"nav","method":"Page.navigate","params":{"url":"data:text/html,%3Cscript%3EglobalThis.startupOrder%3D%5B'script-before'%5D%3Balert('startup')%3BstartupOrder.push('script-after')%3BaddEventListener('load'%2C()%3D%3E%7BstartupOrder.push('load-before')%3Balert('load')%3BstartupOrder.push('load-after')%7D)%3C%2Fscript%3E"}}"#,
        );
        let opening = browser_payloads(&mut session, client.client_id);
        assert!(opening.iter().any(|value| {
            value["method"] == "Page.javascriptDialogOpening"
                && value["params"]["message"] == "startup"
        }));
        assert!(!opening.iter().any(|value| value["id"] == "nav"));

        for expected in ["startup", "load"] {
            browser_request(
                &mut session,
                client.client_id,
                &json!({
                    "jsonrpc": "2.0",
                    "id": format!("handle-{expected}"),
                    "method": "Page.handleJavaScriptDialog",
                    "params": { "accept": true },
                })
                .to_string(),
            );
            let payloads = browser_payloads(&mut session, client.client_id);
            if expected == "startup" {
                assert!(payloads.iter().any(|value| {
                    value["method"] == "Page.javascriptDialogOpening"
                        && value["params"]["message"] == "load"
                }), "{payloads:#?}");
                assert!(!payloads.iter().any(|value| value["id"] == "nav"));
            } else {
                assert!(payloads.iter().any(|value| value["id"] == "nav"));
            }
        }

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"state","method":"Runtime.evaluate","params":{"expression":"startupOrder.join(',')","returnByValue":true}}"#,
        );
        let state = browser_payloads(&mut session, client.client_id);
        assert!(state.iter().any(|value| {
            value["id"] == "state"
                && value["result"]["result"]["value"]
                    == "script-before,script-after,load-before,load-after"
        }));
    }

    #[test]
    fn navigation_pumps_module_timer_and_animation_frame_dialogs() {
        let mut session = BrowserSession::new().unwrap();
        let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"nav","method":"Page.navigate","params":{"url":"data:text/html,%3Cscript%20type%3D%22module%22%3EglobalThis.pageOrder%3D%5B'module-before'%5D%3Balert('module')%3BpageOrder.push('module-after')%3BsetTimeout(()%3D%3E%7BpageOrder.push('timer-before')%3Balert('timer')%3BpageOrder.push('timer-after')%7D%2C0)%3BrequestAnimationFrame(()%3D%3E%7BpageOrder.push('frame-before')%3Balert('frame')%3BpageOrder.push('frame-after')%7D)%3C%2Fscript%3E"}}"#,
        );
        let opening = browser_payloads(&mut session, client.client_id);
        assert!(opening.iter().any(|value| {
            value["method"] == "Page.javascriptDialogOpening"
                && value["params"]["message"] == "module"
        }));

        for (index, expected) in ["module", "timer", "frame"].into_iter().enumerate() {
            browser_request(
                &mut session,
                client.client_id,
                &json!({
                    "jsonrpc": "2.0",
                    "id": format!("handle-{expected}"),
                    "method": "Page.handleJavaScriptDialog",
                    "params": { "accept": true },
                })
                .to_string(),
            );
            let payloads = browser_payloads(&mut session, client.client_id);
            if let Some(next) = ["timer", "frame"].get(index) {
                assert!(payloads.iter().any(|value| {
                    value["method"] == "Page.javascriptDialogOpening"
                        && value["params"]["message"] == *next
                }));
            } else {
                assert!(payloads.iter().any(|value| value["id"] == "nav"));
            }
        }

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"state","method":"Runtime.evaluate","params":{"expression":"pageOrder.join(',')","returnByValue":true}}"#,
        );
        let state = browser_payloads(&mut session, client.client_id);
        assert!(state.iter().any(|value| {
            value["id"] == "state"
                && value["result"]["result"]["value"]
                    == "module-before,module-after,timer-before,timer-after,frame-before,frame-after"
        }));
    }

    #[test]
    fn replacement_navigation_cancels_a_suspended_startup_task() {
        let mut session = BrowserSession::new().unwrap();
        let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"old-nav","method":"Page.navigate","params":{"url":"data:text/html,%3Cscript%3EglobalThis.oldStartupBefore%3Dtrue%3Balert('old-startup')%3BglobalThis.oldStartupAfter%3Dtrue%3C%2Fscript%3E"}}"#,
        );
        browser_payloads(&mut session, client.client_id);
        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"new-nav","method":"Page.navigate","params":{"url":"data:text/html,%3Ctitle%3Enew%3C%2Ftitle%3E"}}"#,
        );
        let payloads = browser_payloads(&mut session, client.client_id);
        assert!(payloads.iter().any(|value| {
            value["method"] == "Page.javascriptDialogClosed"
                && value["params"]["result"] == false
        }));
        assert!(payloads.iter().any(|value| value["id"] == "old-nav" && value["error"].is_object()), "{payloads:#?}");
        assert!(payloads.iter().any(|value| value["id"] == "new-nav" && value["result"].is_object()));

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"state","method":"Runtime.evaluate","params":{"expression":"document.title + ':' + typeof oldStartupAfter","returnByValue":true}}"#,
        );
        let state = browser_payloads(&mut session, client.client_id);
        assert!(state.iter().any(|value| {
            value["id"] == "state" && value["result"]["result"]["value"] == "new:undefined"
        }));
    }

    #[test]
    fn startup_navigation_owner_disconnect_clears_task_and_token() {
        let mut session = BrowserSession::new().unwrap();
        let owner = session.accept_upgrade(sample_upgrade_request()).unwrap();
        let observer = session.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut session,
            owner.client_id,
            r#"{"jsonrpc":"2.0","id":"nav","method":"Page.navigate","params":{"url":"data:text/html,%3Cscript%3Ealert('disconnect-startup')%3BglobalThis.afterStartupDisconnect%3Dtrue%3C%2Fscript%3E"}}"#,
        );
        browser_payloads(&mut session, owner.client_id);
        browser_payloads(&mut session, observer.client_id);
        session
            .receive(
                owner.client_id,
                &WebSocketFrame {
                    fin: true,
                    opcode: WebSocketOpcode::Close,
                    payload: Vec::new(),
                }
                .encode(true),
            )
            .unwrap();
        assert_eq!(session.pending_response_count(), 0);
        let events = browser_payloads(&mut session, observer.client_id);
        assert!(events.iter().any(|value| {
            value["method"] == "Page.javascriptDialogClosed"
                && value["params"]["result"] == false
        }));
        browser_request(
            &mut session,
            observer.client_id,
            r#"{"jsonrpc":"2.0","id":"state","method":"Runtime.evaluate","params":{"expression":"location.href + ':' + typeof afterStartupDisconnect","returnByValue":true}}"#,
        );
        let state = browser_payloads(&mut session, observer.client_id);
        assert!(state.iter().any(|value| {
            value["id"] == "state"
                && value["result"]["result"]["value"] == "about:blank:undefined"
        }));
    }

    #[test]
    fn navigation_dismisses_a_dialog_and_new_runtime_dialog_ids_are_isolated() {
        let mut session = BrowserSession::new().unwrap();
        let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"old","method":"Runtime.evaluate","params":{"expression":"confirm('Old runtime')"}}"#,
        );
        browser_payloads(&mut session, client.client_id);

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"nav","method":"Page.navigate","params":{"url":"data:text/html,<title>next</title>"}}"#,
        );
        let navigation = browser_payloads(&mut session, client.client_id);
        assert!(navigation.iter().any(|value| {
            value["method"] == "Page.javascriptDialogClosed"
                && value["params"]["result"] == false
        }));
        assert!(navigation.iter().any(|value| value["id"] == "old"));
        assert!(navigation.iter().any(|value| value["id"] == "nav"));

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"new","method":"Runtime.evaluate","params":{"expression":"prompt('New runtime', 'fresh')"}}"#,
        );
        let opening = browser_payloads(&mut session, client.client_id);
        assert!(opening.iter().any(|value| {
            value["method"] == "Page.javascriptDialogOpening"
                && value["params"]["message"] == "New runtime"
        }));
        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"new-handle","method":"Page.handleJavaScriptDialog","params":{"accept":true}}"#,
        );
        let completed = browser_payloads(&mut session, client.client_id);
        assert!(completed.iter().any(|value| {
            value["id"] == "new" && value["result"]["result"]["value"] == "fresh"
        }));
    }

    #[test]
    fn reload_dismisses_a_dialog_and_replaces_the_script_state() {
        let mut session = BrowserSession::new().unwrap();
        let client = session.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"initial-nav","method":"Page.navigate","params":{"url":"data:text/html,<title>reload</title>"}}"#,
        );
        browser_payloads(&mut session, client.client_id);
        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"old-eval","method":"Runtime.evaluate","params":{"expression":"globalThis.beforeReload = 1; confirm('Reload?')"}}"#,
        );
        browser_payloads(&mut session, client.client_id);

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"reload","method":"Page.reload","params":{}}"#,
        );
        let reload = browser_payloads(&mut session, client.client_id);
        assert!(reload.iter().any(|value| {
            value["method"] == "Page.javascriptDialogClosed"
                && value["params"]["result"] == false
        }));
        assert!(reload.iter().any(|value| value["id"] == "old-eval"));
        assert!(reload.iter().any(|value| value["id"] == "reload"));

        browser_request(
            &mut session,
            client.client_id,
            r#"{"jsonrpc":"2.0","id":"check","method":"Runtime.evaluate","params":{"expression":"typeof beforeReload"}}"#,
        );
        let checked = browser_payloads(&mut session, client.client_id);
        assert!(checked.iter().any(|value| {
            value["id"] == "check"
                && value["result"]["result"]["value"] == "undefined"
        }));
    }

    #[test]
    fn owner_disconnect_dismisses_dialog_without_leaking_deferred_state() {
        let mut session = BrowserSession::new().unwrap();
        let owner = session.accept_upgrade(sample_upgrade_request()).unwrap();
        let observer = session.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut session,
            owner.client_id,
            r#"{"jsonrpc":"2.0","id":1,"method":"Runtime.evaluate","params":{"expression":"alert('bye'); globalThis.afterDisconnect = true"}}"#,
        );
        browser_payloads(&mut session, owner.client_id);
        browser_payloads(&mut session, observer.client_id);

        session
            .receive(owner.client_id, &WebSocketFrame {
                fin: true,
                opcode: WebSocketOpcode::Close,
                payload: Vec::new(),
            }.encode(true))
            .unwrap();
        assert_eq!(session.pending_response_count(), 0);
        let observer_events = browser_payloads(&mut session, observer.client_id);
        assert!(observer_events.iter().any(|value| {
            value["method"] == "Page.javascriptDialogClosed"
                && value["params"]["result"] == false
        }));

        browser_request(
            &mut session,
            observer.client_id,
            r#"{"jsonrpc":"2.0","id":2,"method":"Runtime.evaluate","params":{"expression":"typeof afterDisconnect"}}"#,
        );
        let response = browser_payloads(&mut session, observer.client_id);
        assert!(response.iter().any(|value| {
            value["id"] == 2
                && value["result"]["result"]["value"] == "undefined"
        }));
    }

    #[test]
    fn observer_disconnect_does_not_cancel_owner_dialog() {
        let mut session = BrowserSession::new().unwrap();
        let owner = session.accept_upgrade(sample_upgrade_request()).unwrap();
        let observer = session.accept_upgrade(sample_upgrade_request()).unwrap();
        browser_request(
            &mut session,
            owner.client_id,
            r#"{"jsonrpc":"2.0","id":"eval","method":"Runtime.evaluate","params":{"expression":"alert('keep pending')"}}"#,
        );
        browser_payloads(&mut session, owner.client_id);
        browser_payloads(&mut session, observer.client_id);

        session
            .receive(
                observer.client_id,
                &WebSocketFrame {
                    fin: true,
                    opcode: WebSocketOpcode::Close,
                    payload: Vec::new(),
                }
                .encode(true),
            )
            .unwrap();
        assert_eq!(session.pending_response_count(), 1);

        browser_request(
            &mut session,
            owner.client_id,
            r#"{"jsonrpc":"2.0","id":"handle","method":"Page.handleJavaScriptDialog","params":{"accept":true}}"#,
        );
        let payloads = browser_payloads(&mut session, owner.client_id);
        assert!(payloads.iter().any(|value| value["id"] == "eval"));
    }

    #[test]
    fn disconnect_drops_only_that_clients_deferred_responses() {
        let mut server = CdpServer::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let handler_tokens = captured.clone();
        server.register_deferred_method("Runtime.evaluate", move |token, _| {
            handler_tokens.borrow_mut().push(token);
            Ok(CdpMethodResult::Deferred)
        });
        let first = server.accept_upgrade(sample_upgrade_request()).unwrap();
        let second = server.accept_upgrade(sample_upgrade_request()).unwrap();

        for (client_id, id) in [(first.client_id, 1), (second.client_id, 2)] {
            let request = WebSocketFrame::text(format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"Runtime.evaluate","params":{{}}}}"#
            ))
            .encode(true);
            server.receive(client_id, &request).unwrap();
        }
        assert_eq!(server.pending_response_count(), 2);

        let close = WebSocketFrame {
            fin: true,
            opcode: WebSocketOpcode::Close,
            payload: Vec::new(),
        }
        .encode(true);
        server.receive(first.client_id, &close).unwrap();
        assert_eq!(server.pending_response_count(), 1);

        let tokens = captured.borrow();
        assert_eq!(
            server.complete_deferred_response(tokens[0], Ok(Value::Null)),
            Err(CdpError::UnknownDeferredRequest(tokens[0].0))
        );
        server
            .complete_deferred_response(
                tokens[1],
                Err(JsonRpcError {
                    code: -32001,
                    message: "dialog dismissed".to_string(),
                }),
            )
            .unwrap();
        let completed = server.drain_outgoing(second.client_id).unwrap();
        let payload: Value = serde_json::from_str(&decode_text(&completed[0])).unwrap();
        assert_eq!(payload["id"], 2);
        assert_eq!(payload["error"]["code"], -32001);
    }

    #[test]
    fn deferred_capable_method_can_complete_immediately() {
        let mut server = CdpServer::new();
        server.register_deferred_method("Runtime.evaluate", |_, params| {
            Ok(CdpMethodResult::Complete(
                json!({ "echo": params["expression"] }),
            ))
        });
        let client = server.accept_upgrade(sample_upgrade_request()).unwrap();
        let request = WebSocketFrame::text(
            r#"{"jsonrpc":"2.0","id":7,"method":"Runtime.evaluate","params":{"expression":"1 + 1"}}"#,
        )
        .encode(true);

        server.receive(client.client_id, &request).unwrap();

        let outgoing = server.drain_outgoing(client.client_id).unwrap();
        assert_eq!(outgoing.len(), 1);
        let payload: Value = serde_json::from_str(&decode_text(&outgoing[0])).unwrap();
        assert_eq!(payload["id"], 7);
        assert_eq!(payload["result"]["echo"], "1 + 1");
        assert_eq!(server.pending_response_count(), 0);
    }

    #[test]
    fn deferred_capable_method_preserves_an_immediate_error_code() {
        let mut server = CdpServer::new();
        server.register_deferred_method("Runtime.evaluate", |_, _| {
            Err(JsonRpcError {
                code: -32001,
                message: "dialog dismissed".to_string(),
            })
        });
        let client = server.accept_upgrade(sample_upgrade_request()).unwrap();
        let request = WebSocketFrame::text(
            r#"{"jsonrpc":"2.0","id":9,"method":"Runtime.evaluate","params":{}}"#,
        )
        .encode(true);

        server.receive(client.client_id, &request).unwrap();

        let outgoing = server.drain_outgoing(client.client_id).unwrap();
        assert_eq!(outgoing.len(), 1);
        let payload: Value = serde_json::from_str(&decode_text(&outgoing[0])).unwrap();
        assert_eq!(payload["id"], 9);
        assert_eq!(payload["error"]["code"], -32001);
        assert_eq!(payload["error"]["message"], "dialog dismissed");
        assert_eq!(server.pending_response_count(), 0);
    }

    #[test]
    fn deferred_token_exhaustion_does_not_reuse_a_token() {
        let mut server = CdpServer::new();
        server.next_deferred_token = u64::MAX;
        server.register_deferred_method("Runtime.evaluate", |_, _| {
            Ok(CdpMethodResult::Deferred)
        });
        let client = server.accept_upgrade(sample_upgrade_request()).unwrap();
        let request = WebSocketFrame::text(
            r#"{"jsonrpc":"2.0","id":10,"method":"Runtime.evaluate","params":{}}"#,
        )
        .encode(true);

        assert_eq!(
            server.receive(client.client_id, &request),
            Err(CdpError::DeferredTokenExhausted)
        );
        assert_eq!(server.next_deferred_token, u64::MAX);
        assert_eq!(server.pending_response_count(), 0);
        assert!(server.drain_outgoing(client.client_id).unwrap().is_empty());
    }

    #[test]
    fn later_registration_replaces_the_previous_handler_kind() {
        let mut server = CdpServer::new();
        server.register_deferred_method("Runtime.evaluate", |_, _| Ok(CdpMethodResult::Deferred));
        server.register_method("Runtime.evaluate", |_| Ok(json!({ "kind": "synchronous" })));
        let client = server.accept_upgrade(sample_upgrade_request()).unwrap();
        let request = WebSocketFrame::text(
            r#"{"jsonrpc":"2.0","id":8,"method":"Runtime.evaluate","params":{}}"#,
        )
        .encode(true);

        server.receive(client.client_id, &request).unwrap();

        let outgoing = server.drain_outgoing(client.client_id).unwrap();
        assert_eq!(outgoing.len(), 1);
        let payload: Value = serde_json::from_str(&decode_text(&outgoing[0])).unwrap();
        assert_eq!(payload["result"]["kind"], "synchronous");
        assert_eq!(server.pending_response_count(), 0);
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
    fn navigation_survives_a_dynamically_inserted_module_script_that_throws() {
        // The shape blog.piapro.net failed on: a page script inserts a
        // `type="module"` script, and that module throws. Neither the module's
        // syntax nor its exception may cost the navigation (issue #303).
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 4096];
                let size = stream.read(&mut buffer).unwrap();
                let _ = String::from_utf8_lossy(&buffer[..size]);
                let body: &[u8] = match index {
                    0 => b"<html><body><main id='content'>rendered</main><script>                           const s = document.createElement('script');                           s.type = 'module';                           s.src = '/module.js';                           document.head.appendChild(s);                           </script></body></html>",
                    _ => b"export const answer = 42; throw new Error('module boom');",
                };
                let content_type = if index == 0 { "text/html" } else { "text/javascript" };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.write_all(body).unwrap();
            }
        });
        let origin = format!("http://127.0.0.1:{}", address.port());
        let mut session = CdpSession::new().unwrap();

        session
            .dispatch("Page.navigate", json!({ "url": format!("{origin}/page") }))
            .expect("a throwing module script must not fail the navigation");

        // The document is installed and usable, not discarded.
        let content = session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "document.getElementById('content').textContent" }),
            )
            .unwrap();
        assert_eq!(content["result"]["value"], "rendered");
        assert_eq!(session.current_url(), format!("{origin}/page"));
        server.join().unwrap();
    }

    #[test]
    fn get_and_post_form_submissions_reach_http_server_and_install_documents() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let server = thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 4096];
                let size = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..size]).into_owned();
                if index > 0 {
                    sender.send(request).unwrap();
                }
                let body = match index {
                    0 => "<form id='f' action='/search?old=1' method='get'><input name='q' value='hello world'><button name='via' value='button'>Search</button></form>",
                    1 => "<form id='f' action='/submit' method='post'><input name='q' value='hello world'><button name='via' value='button'>Send</button></form>",
                    _ => "<html><body><main id='submitted'>Saved</main></body></html>",
                };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        let origin = format!("http://127.0.0.1:{}", address.port());
        let mut session = CdpSession::new().unwrap();
        session.dispatch("Page.navigate", json!({ "url": format!("{origin}/form") })).unwrap();
        session.dispatch("Runtime.evaluate", json!({ "expression": "document.getElementById('f').requestSubmit(document.querySelector('button'))" })).unwrap();
        assert_eq!(session.current_url(), format!("{origin}/search?q=hello+world&via=button"));
        session.dispatch("Runtime.evaluate", json!({ "expression": "document.getElementById('f').requestSubmit(document.querySelector('button'))" })).unwrap();
        assert_eq!(session.current_url(), format!("{origin}/submit"));
        let installed = session.dispatch("Runtime.evaluate", json!({ "expression": "document.getElementById('submitted').textContent" })).unwrap();
        assert_eq!(installed["result"]["value"], "Saved");
        let get_request = receiver.recv().unwrap();
        assert!(get_request.starts_with("GET /search?q=hello+world&via=button HTTP/1.1\r\n"));
        let post_request = receiver.recv().unwrap();
        assert!(post_request.starts_with("POST /submit HTTP/1.1\r\n"));
        assert!(post_request.contains("Content-Type: application/x-www-form-urlencoded\r\n"));
        assert!(post_request.ends_with("q=hello+world&via=button"));
        server.join().unwrap();
    }

    #[test]
    fn web_storage_survives_same_origin_document_navigation() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer).unwrap();
                let body = "<html><body></body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let origin = format!("http://127.0.0.1:{}", address.port());
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch("Page.navigate", json!({ "url": format!("{origin}/first") }))
            .unwrap();
        session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "localStorage.setItem('local', 'kept'); sessionStorage.setItem('session', 'kept')" }),
            )
            .unwrap();
        session
            .dispatch("Page.navigate", json!({ "url": format!("{origin}/second") }))
            .unwrap();
        let result = session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "localStorage.getItem('local') === 'kept' && sessionStorage.getItem('session') === 'kept'" }),
            )
            .unwrap();
        assert_eq!(result["result"]["value"], true);

        server.join().unwrap();
    }

    #[test]
    fn location_requests_install_new_documents_and_preserve_commit_semantics() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 4096];
                let size = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..size]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let body = match path {
                    "/first" => "<html><body><main id='first'></main></body></html>",
                    "/second" => r#"<html><body><main id='second'></main><script>
                        document.addEventListener('DOMContentLoaded', () => document.body.setAttribute('data-dcl', 'yes'));
                        window.addEventListener('load', () => document.body.setAttribute('data-load', 'yes'));
                    </script></body></html>"#,
                    "/third" => "<html><body><main id='third'></main></body></html>",
                    _ => "<html><body>missing</body></html>",
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let origin = format!("http://127.0.0.1:{}", address.port());
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch(
                "Page.navigate",
                json!({ "url": format!("{origin}/first") }),
            )
            .unwrap();
        session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "location.assign('/second')" }),
            )
            .unwrap();

        assert_eq!(session.current_url(), format!("{origin}/second"));
        let lifecycle = session
            .dispatch(
                "Runtime.evaluate",
                json!({
                    "expression": "location.href === document.URL && document.querySelector('#second') !== null && document.body.getAttribute('data-dcl') === 'yes' && document.body.getAttribute('data-load') === 'yes'"
                }),
            )
            .unwrap();
        assert_eq!(lifecycle["result"]["value"], true);

        let history_len_before_replace = session.history_entries.len();
        session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "location.replace('/third')" }),
            )
            .unwrap();
        assert_eq!(session.current_url(), format!("{origin}/third"));
        assert_eq!(session.history_entries.len(), history_len_before_replace);

        session.dispatch("Page.reload", json!({})).unwrap();
        assert_eq!(session.current_url(), format!("{origin}/third"));
        assert_eq!(session.history_entries.len(), history_len_before_replace);

        server.join().unwrap();
    }

    #[test]
    fn fragment_navigation_keeps_document_and_skips_network_fetch() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer).unwrap();
            let body = "<html><body><main id='persistent'></main></body></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        let url = format!("http://127.0.0.1:{}/page", address.port());
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch("Page.navigate", json!({ "url": url }))
            .unwrap();
        session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "globalThis.hashChanges = 0; addEventListener('hashchange', () => hashChanges++); location.assign('#section')" }),
            )
            .unwrap();

        assert_eq!(session.current_url(), format!("{url}#section"));
        let state = session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "document.querySelector('#persistent') !== null && hashChanges === 1 && location.hash === '#section'" }),
            )
            .unwrap();
        assert_eq!(state["result"]["value"], true);
        assert!(session
            .drain_events()
            .iter()
            .any(|event| event.method == "Page.navigatedWithinDocument"));
        server.join().unwrap();
    }

    #[test]
    fn history_state_and_traversal_are_owned_by_browser_session() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 2048];
                let size = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..size]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let body = if path == "/state" {
                    format!(
                        "<html><body><main data-path='{path}'></main><script>document.body.setAttribute('data-startup-history', history.length + ':' + history.state.page)</script></body></html>"
                    )
                } else {
                    format!("<html><body><main data-path='{path}'></main></body></html>")
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let origin = format!("http://127.0.0.1:{}", address.port());
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch(
                "Page.navigate",
                json!({ "url": format!("{origin}/start") }),
            )
            .unwrap();
        session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "history.pushState({ page: 2 }, '', '/state')" }),
            )
            .unwrap();
        assert_eq!(session.current_url(), format!("{origin}/state"));
        assert_eq!(session.history_entries.len(), 2);

        session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "history.back()" }),
            )
            .unwrap();
        assert_eq!(session.current_url(), format!("{origin}/start"));

        session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "history.forward()" }),
            )
            .unwrap();
        assert_eq!(session.current_url(), format!("{origin}/state"));
        let restored = session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "history.length === 2 && history.state.page === 2 && document.body.getAttribute('data-startup-history') === '2:2' && document.querySelector('main').getAttribute('data-path') === '/state'" }),
            )
            .unwrap();
        assert_eq!(restored["result"]["value"], true);

        server.join().unwrap();
    }

    #[test]
    fn failed_script_navigation_preserves_current_document_and_url() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer).unwrap();
            let body = "<html><body><main id='stable'></main></body></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        let stable_url = format!("http://127.0.0.1:{}/stable", address.port());
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch("Page.navigate", json!({ "url": stable_url }))
            .unwrap();
        server.join().unwrap();

        let failed = session.dispatch(
            "Runtime.evaluate",
            json!({ "expression": "location.assign('/unreachable')" }),
        );
        assert!(failed.is_err());
        assert_eq!(session.current_url(), stable_url);
        let preserved = session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "location.href === document.URL && location.href.endsWith('/stable') && document.querySelector('#stable') !== null" }),
            )
            .unwrap();
        assert_eq!(preserved["result"]["value"], true);
    }

    #[test]
    fn redirect_commits_final_url_to_document_location_and_history() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer).unwrap();
                let response = if request_index == 0 {
                    "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                } else {
                    let body = "<html><body><main id='final'></main></body></html>";
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                };
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let origin = format!("http://127.0.0.1:{}", address.port());
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch(
                "Page.navigate",
                json!({ "url": format!("{origin}/redirect") }),
            )
            .unwrap();

        assert_eq!(session.current_url(), format!("{origin}/final"));
        assert_eq!(session.history_entries[session.history_index].url, format!("{origin}/final"));
        let state = session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "location.href.endsWith('/final') && document.URL === location.href && document.querySelector('#final') !== null" }),
            )
            .unwrap();
        assert_eq!(state["result"]["value"], true);
        server.join().unwrap();
    }

    #[test]
    fn event_loop_driver_commits_timer_and_animation_frame_navigation() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 2048];
                let size = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..size]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let body = format!("<html><body><main data-path='{path}'></main></body></html>");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let origin = format!("http://127.0.0.1:{}", address.port());
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch(
                "Page.navigate",
                json!({ "url": format!("{origin}/start") }),
            )
            .unwrap();
        session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "setTimeout(() => location.assign('/timer'), 10)" }),
            )
            .unwrap();
        assert_eq!(session.current_url(), format!("{origin}/start"));
        session.drive_event_loop(10).unwrap();
        assert_eq!(session.current_url(), format!("{origin}/timer"));

        session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "requestAnimationFrame(() => location.assign('/frame'))" }),
            )
            .unwrap();
        session.drive_event_loop(16).unwrap();
        assert_eq!(session.current_url(), format!("{origin}/frame"));

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
    fn navigation_invalidates_old_document_node_ids_without_reusing_them() {
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch(
                "Page.navigate",
                json!({ "url": "data:text/html,<html><body><main id='old'></main></body></html>" }),
            )
            .unwrap();
        let old_document = session.dispatch("DOM.getDocument", json!({})).unwrap();
        let old_root_id = old_document["root"]["nodeId"].as_u64().unwrap();
        let old_main_id = session
            .dispatch(
                "DOM.querySelector",
                json!({ "nodeId": old_root_id, "selector": "#old" }),
            )
            .unwrap()["nodeId"]
            .as_u64()
            .unwrap();

        session
            .dispatch(
                "Page.navigate",
                json!({ "url": "data:text/html,<html><body><main id='new'></main></body></html>" }),
            )
            .unwrap();
        let new_document = session.dispatch("DOM.getDocument", json!({})).unwrap();
        let new_root_id = new_document["root"]["nodeId"].as_u64().unwrap();

        assert_ne!(new_root_id, old_root_id);
        assert!(session
            .dispatch("DOM.getOuterHTML", json!({ "nodeId": old_main_id }))
            .is_err());
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

    #[test]
    fn mouse_input_hit_tests_paint_order_transforms_clips_and_scroll() {
        let mut session = CdpSession::new().unwrap();
        session
            .install_document(
                "https://example.test/",
                r#"<html><head><style>
                    * { margin: 0; padding: 0; } body { height: 1000px; }
                    .box { position: absolute; width: 80px; height: 80px; }
                    #back { left: 0; top: 0; z-index: 1; }
                    #front { left: 0; top: 0; width: 50px; height: 50px; z-index: 2; }
                    #none { left: 0; top: 0; z-index: 3; pointer-events: none; }
                    #transformed { left: 0; top: 120px; width: 40px; height: 40px;
                                   transform: translateX(100px); }
                    #under { position: absolute; left: 0; top: 200px; width: 100px; height: 50px; }
                    #clip { position: absolute; left: 0; top: 200px; width: 50px; height: 50px;
                            overflow: hidden; z-index: 2; }
                    #clipped { position: absolute; left: 60px; top: 0; width: 30px; height: 30px; }
                    #scroller { position: absolute; left: 0; top: 300px; width: 100px; height: 50px;
                                overflow: hidden; }
                    #content { position: relative; height: 200px; }
                    #scrolled { position: absolute; left: 0; top: 100px; width: 60px; height: 30px; }
                    #windowScrolled { position: absolute; left: 0; top: 450px; width: 60px; height: 30px; }
                </style></head><body>
                  <div id="back" class="box"></div><div id="front" class="box"></div>
                  <div id="none" class="box"></div><div id="transformed" class="box"></div>
                  <div id="under"></div><div id="clip"><div id="clipped"></div></div>
                  <div id="scroller"><div id="content"><button id="scrolled">s</button></div></div>
                  <button id="windowScrolled">w</button>
                  <script>globalThis.hits=[]; for (const id of ['back','front','none','transformed','under','clipped','scrolled','windowScrolled']) document.getElementById(id).addEventListener('mousemove',()=>hits.push(id)); scroller.scrollTop=80;</script>
                </body></html>"#,
                1,
                "null",
            )
            .unwrap();

        for (x, y) in [(10, 10), (110, 130), (70, 210), (10, 325)] {
            session
                .dispatch(
                    "Input.dispatchMouseEvent",
                    json!({ "type": "mouseMoved", "x": x, "y": y }),
                )
                .unwrap();
        }
        session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "scrollTo(0,50)", "returnByValue": true }),
            )
            .unwrap();
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({ "type": "mouseMoved", "x": 10, "y": 410 }),
            )
            .unwrap();
        let hits = session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "hits.join(',')", "returnByValue": true }),
            )
            .unwrap();
        assert_eq!(
            hits["result"]["value"],
            "front,transformed,under,scrolled,windowScrolled"
        );
    }

    #[test]
    fn mouse_input_sequences_clicks_focus_and_reports_prevent_default() {
        let mut session = CdpSession::new().unwrap();
        session
            .install_document(
                "https://example.test/",
                r#"<html><head><style>*{margin:0} #ok,#blocked{position:absolute;width:80px;height:40px}#ok{left:0;top:0}#blocked{left:100px;top:0}</style></head><body>
                    <button id="ok">ok</button><input id="blocked">
                    <script>globalThis.events=[];globalThis.mouseFields='';globalThis.mouseBubble='';globalThis.moveFields=''; for(const type of ['mousedown','mouseup','click']) { ok.addEventListener(type,e=>{events.push('ok:'+type);if(type==='mousedown')mouseFields=[e.clientX,e.clientY,e.pageX,e.pageY,e.button,e.buttons,e.altKey,e.shiftKey,e.bubbles,e.composed].join(':')}); blocked.addEventListener(type,e=>{events.push('blocked:'+type);if(type==='mousedown')e.preventDefault()}) } ok.addEventListener('mousemove',e=>moveFields=[e.button,e.buttons,e.cancelable].join(':')); document.addEventListener('mousedown',e=>mouseBubble=e.target.id);</script>
                </body></html>"#,
                1,
                "null",
            )
            .unwrap();

        session.dispatch("Input.dispatchMouseEvent", json!({"type":"mousePressed","x":10,"y":10,"button":"left","buttons":1,"modifiers":9})).unwrap();
        session.dispatch("Input.dispatchMouseEvent", json!({"type":"mouseReleased","x":10,"y":10,"button":"left","buttons":0})).unwrap();
        session.dispatch("Input.dispatchMouseEvent", json!({"type":"mousePressed","x":10,"y":10,"button":"left","modifiers":9})).unwrap();
        session.dispatch("Input.dispatchMouseEvent", json!({"type":"mouseReleased","x":110,"y":10,"button":"left"})).unwrap();
        session.dispatch("Input.dispatchMouseEvent", json!({"type":"mouseMoved","x":10,"y":10})).unwrap();
        let prevented = session.dispatch("Input.dispatchMouseEvent", json!({"type":"mousePressed","x":110,"y":10,"button":"left"})).unwrap();
        session.dispatch("Input.dispatchMouseEvent", json!({"type":"mouseReleased","x":110,"y":10,"button":"left"})).unwrap();

        assert_eq!(prevented["defaultPrevented"], true);
        let state = session.dispatch("Runtime.evaluate", json!({
            "expression": "JSON.stringify({events,active:document.activeElement.id,mouseFields,moveFields,mouseBubble})",
            "returnByValue": true
        })).unwrap();
        assert_eq!(
            state["result"]["value"],
            r#"{"events":["ok:mousedown","ok:mouseup","ok:click","ok:mousedown","blocked:mouseup","blocked:mousedown","blocked:mouseup","blocked:click"],"active":"ok","mouseFields":"10:10:10:10:0:1:true:true:true:true","moveFields":"0:0:true","mouseBubble":"blocked"}"#
        );
    }

    #[test]
    fn mouse_input_dispatches_deterministic_drag_lifecycle_and_shared_data_transfer() {
        let mut session = CdpSession::new().unwrap();
        session
            .install_document(
                "https://example.test/",
                r#"<html><head><style>*{margin:0}#source,#target{position:absolute;top:0;width:80px;height:40px}#source{left:0}#target{left:100px}</style></head><body>
                    <div id="source" draggable="true">source</div><div id="target">target</div>
                    <script>
                      globalThis.dragEvents=[];globalThis.transfer=null;globalThis.sameTransfer=true;globalThis.clicks=0;
                      source.addEventListener('click',()=>clicks++);
                      source.addEventListener('dragstart',e=>{dragEvents.push('start:'+e.bubbles+':'+e.cancelable+':'+(e instanceof DragEvent));transfer=e.dataTransfer;e.dataTransfer.setData('text/plain','payload')});
                      source.addEventListener('drag',e=>{dragEvents.push('drag');sameTransfer=sameTransfer&&e.dataTransfer===transfer});
                      source.addEventListener('dragover',e=>{dragEvents.push('over');sameTransfer=sameTransfer&&e.dataTransfer===transfer;e.preventDefault()});
                      source.addEventListener('drop',e=>{dragEvents.push('drop:'+e.dataTransfer.getData('text/plain'));sameTransfer=sameTransfer&&e.dataTransfer===transfer});
                      target.addEventListener('dragenter',e=>{dragEvents.push('enter');sameTransfer=sameTransfer&&e.dataTransfer===transfer});
                      target.addEventListener('dragleave',e=>{dragEvents.push('leave');sameTransfer=sameTransfer&&e.dataTransfer===transfer});
                      target.addEventListener('dragover',e=>{dragEvents.push('over');sameTransfer=sameTransfer&&e.dataTransfer===transfer;e.preventDefault()});
                      target.addEventListener('drop',e=>{dragEvents.push('drop:'+e.dataTransfer.getData('text/plain'));sameTransfer=sameTransfer&&e.dataTransfer===transfer});
                      source.addEventListener('dragend',e=>{dragEvents.push('end:'+e.dataTransfer.getData('text/plain'));sameTransfer=sameTransfer&&e.dataTransfer===transfer});
                    </script>
                </body></html>"#,
                1,
                "null",
            )
            .unwrap();

        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mousePressed","x":10,"y":10,"button":"left","buttons":1}),
            )
            .unwrap();
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mouseMoved","x":110,"y":10,"button":"none","buttons":1}),
            )
            .unwrap();
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mouseMoved","x":110,"y":10,"button":"none","buttons":1}),
            )
            .unwrap();
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mouseReleased","x":10,"y":10,"button":"left","buttons":0}),
            )
            .unwrap();

        let state = session
            .dispatch(
                "Runtime.evaluate",
                json!({
                    "expression": "JSON.stringify({events:dragEvents,sameTransfer,clicks:globalThis.clicks||0})",
                    "returnByValue": true,
                }),
            )
            .unwrap();
        assert_eq!(
            state["result"]["value"],
            r#"{"events":["start:true:true:true","drag","enter","over","drag","over","leave","over","drop:payload","end:payload"],"sameTransfer":true,"clicks":0}"#
        );
    }

    #[test]
    fn canceled_dragstart_consumes_candidate_and_keeps_click_default() {
        let mut session = CdpSession::new().unwrap();
        session
            .install_document(
                "https://example.test/",
                r#"<html><head><style>*{margin:0}#source{position:absolute;left:0;top:0;width:80px;height:40px}</style></head><body>
                    <div id="source" draggable="true">source</div>
                    <script>
                      globalThis.events=[];globalThis.clicks=0;
                      source.addEventListener('dragstart',e=>{events.push('dragstart');e.preventDefault()});
                      source.addEventListener('click',()=>{events.push('click');clicks++});
                    </script>
                </body></html>"#,
                1,
                "null",
            )
            .unwrap();

        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mousePressed","x":10,"y":10,"button":"left","buttons":1}),
            )
            .unwrap();
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mouseMoved","x":20,"y":10,"button":"none","buttons":1}),
            )
            .unwrap();
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mouseMoved","x":30,"y":10,"button":"none","buttons":1}),
            )
            .unwrap();
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mouseReleased","x":10,"y":10,"button":"left","buttons":0}),
            )
            .unwrap();

        let state = session
            .dispatch(
                "Runtime.evaluate",
                json!({
                    "expression": "JSON.stringify({events,clicks})",
                    "returnByValue": true,
                }),
            )
            .unwrap();
        assert_eq!(
            state["result"]["value"],
            r#"{"events":["dragstart","click"],"clicks":1}"#
        );
    }

    #[test]
    fn mouse_hit_test_respects_pointer_events_and_svg_geometry() {
        let mut session = CdpSession::new().unwrap();
        session
            .install_document(
                "https://example.test/",
                r#"<html><head><style>*{margin:0}#back,#overlay{position:absolute;left:0;top:0;width:120px;height:80px}#back{background:gray}#overlay{background:red;pointer-events:none}#icon{position:absolute;left:130px;top:0;width:100px;height:100px;pointer-events:none}#shape{pointer-events:fill}</style></head><body>
                    <div id="back"></div><div id="overlay"></div>
                    <svg id="icon" width="100" height="100" viewBox="0 0 100 100">
                      <rect id="base" x="0" y="0" width="100" height="100" fill="red" pointer-events="none"></rect>
                      <rect id="shape" x="20" y="20" width="30" height="30" fill="blue"></rect>
                      <rect id="stroke" x="60" y="20" width="30" height="30" fill="none" stroke="green" stroke-width="4" pointer-events="stroke"></rect>
                      <g id="inherited" stroke-width="10">
                        <line id="wide" x1="10" y1="70" x2="90" y2="70" fill="none" stroke="green" pointer-events="stroke"></line>
                      </g>
                      <circle id="bounds" cx="70" cy="70" r="10" fill="none" pointer-events="bounding-box"></circle>
                    </svg>
                    <script>
                      globalThis.targets=[];
                      document.addEventListener('click',e=>targets.push(e.target.id||e.target.localName));
                    </script>
                </body></html>"#,
                1,
                "null",
            )
            .unwrap();

        for (x, y) in [(10, 10), (155, 35), (190, 35), (140, 66), (190, 60)] {
            session
                .dispatch(
                    "Input.dispatchMouseEvent",
                    json!({"type":"mousePressed","x":x,"y":y,"button":"left","buttons":1}),
                )
                .unwrap();
            session
                .dispatch(
                    "Input.dispatchMouseEvent",
                    json!({"type":"mouseReleased","x":x,"y":y,"button":"left","buttons":0}),
                )
                .unwrap();
        }
        let state = session
            .dispatch(
                "Runtime.evaluate",
                json!({
                    "expression": "JSON.stringify({targets,none:getComputedStyle(document.getElementById('overlay')).pointerEvents})",
                    "returnByValue": true,
                }),
            )
            .unwrap();
        assert_eq!(
            state["result"]["value"],
            r#"{"targets":["back","shape","stroke","wide","bounds"],"none":"none"}"#
        );
    }

    #[test]
    fn mouse_hit_test_maps_svg_children_to_the_painted_content_box() {
        let mut session = CdpSession::new().unwrap();
        session
            .install_document(
                "https://example.test/",
                r#"<html><head><style>*{margin:0}#back{position:absolute;left:0;top:0;width:240px;height:180px;background:gray}#icon{position:absolute;left:20px;top:20px;width:100px;height:100px;padding:10px;border:5px solid black;pointer-events:none}</style></head><body>
                    <div id="back"></div>
                    <svg id="icon" width="100" height="100" viewBox="0 0 100 100">
                      <rect id="shape" x="0" y="0" width="100" height="100" fill="blue" pointer-events="fill"></rect>
                    </svg>
                    <script>
                      globalThis.targets=[];
                      document.addEventListener('click',e=>targets.push(e.target.id||e.target.localName));
                    </script>
                </body></html>"#,
                1,
                "null",
            )
            .unwrap();

        let bounds = session
            .dispatch(
                "Runtime.evaluate",
                json!({
                    "expression": "(()=>{const r=document.getElementById('icon').getBoundingClientRect();return JSON.stringify([r.x,r.y,r.width,r.height])})()",
                    "returnByValue": true,
                }),
            )
            .unwrap();
        let bounds: Vec<f32> = serde_json::from_str(
            bounds["result"]["value"].as_str().expect("SVG bounds JSON"),
        )
        .unwrap();
        assert_eq!(bounds.len(), 4);
        for (x, y) in [
            (bounds[0] + 2.0, bounds[1] + bounds[3] / 2.0),
            (bounds[0] + bounds[2] / 2.0, bounds[1] + bounds[3] / 2.0),
        ] {
            session
                .dispatch(
                    "Input.dispatchMouseEvent",
                    json!({"type":"mousePressed","x":x,"y":y,"button":"left","buttons":1}),
                )
                .unwrap();
            session
                .dispatch(
                    "Input.dispatchMouseEvent",
                    json!({"type":"mouseReleased","x":x,"y":y,"button":"left","buttons":0}),
                )
                .unwrap();
        }
        let state = session
            .dispatch(
                "Runtime.evaluate",
                json!({"expression":"JSON.stringify(targets)","returnByValue":true}),
            )
            .unwrap();
        assert_eq!(state["result"]["value"], r#"["back","shape"]"#);
    }

    #[test]
    fn mouse_hit_test_targets_geometry_painted_through_svg_use() {
        let mut session = CdpSession::new().unwrap();
        session
            .install_document(
                "https://example.test/",
                r##"<html><body><svg id="icon" width="100" height="100" viewBox="0 0 100 100">
                    <defs><rect id="template" x="0" y="0" width="20" height="20" fill="blue"></rect></defs>
                    <use id="instance" href="#template" x="40" y="40" pointer-events="fill"></use>
                    <use id="blocked" href="#template" x="40" y="40" pointer-events="none"></use>
                    <script>globalThis.targets=[];document.addEventListener('click',e=>targets.push(e.target.id||e.target.localName));</script>
                </svg></body></html>"##,
                1,
                "null",
            )
            .unwrap();
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mousePressed","x":50,"y":50,"button":"left","buttons":1}),
            )
            .unwrap();
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":"mouseReleased","x":50,"y":50,"button":"left","buttons":0}),
            )
            .unwrap();
        let state = session
            .dispatch(
                "Runtime.evaluate",
                json!({"expression":"JSON.stringify(targets)","returnByValue":true}),
            )
            .unwrap();
        assert_eq!(state["result"]["value"], r#"["instance"]"#);
    }

    #[test]
    fn keyboard_input_targets_the_focused_element_with_cdp_fields() {
        let mut session = CdpSession::new().unwrap();
        session
            .install_document(
                "https://example.test/",
                r#"<html><body><input id="field"><script>
                    globalThis.keys=[]; field.focus();
                    field.addEventListener('keydown',e=>{keys.push(['field',e.type,e.key,e.code,e.keyCode,e.ctrlKey,e.shiftKey,e.bubbles,e.composed].join(':'));e.preventDefault()});
                    field.addEventListener('keyup',e=>keys.push(['field',e.type,e.key].join(':')));
                    field.addEventListener('keypress',e=>keys.push(['field',e.type,e.key,e.charCode].join(':')));
                    document.addEventListener('keydown',e=>keys.push('document:'+e.target.id));
                </script></body></html>"#,
                1,
                "null",
            )
            .unwrap();

        let down = session.dispatch("Input.dispatchKeyEvent", json!({
            "type":"keyDown","key":"A","code":"KeyA","windowsVirtualKeyCode":65,"modifiers":10
        })).unwrap();
        session.dispatch("Input.dispatchKeyEvent", json!({"type":"keyUp","key":"A","code":"KeyA"})).unwrap();
        session.dispatch("Input.dispatchKeyEvent", json!({"type":"char","text":"a","key":"a"})).unwrap();
        assert_eq!(down["defaultPrevented"], true);
        let keys = session.dispatch("Runtime.evaluate", json!({
            "expression":"keys.join('|')","returnByValue":true
        })).unwrap();
        assert_eq!(
            keys["result"]["value"],
            "field:keydown:A:KeyA:65:true:true:true:true|document:field|field:keyup:A|field:keypress:a:97"
        );
    }

    #[test]
    fn keyboard_input_edits_focused_text_control_end_to_end() {
        let mut session = CdpSession::new().unwrap();
        session
            .install_document(
                "https://example.test/",
                r#"<html><body><input id="login"><script>
                    globalThis.editEvents=[];
                    login.addEventListener('beforeinput',e=>editEvents.push(e.type+':'+e.inputType+':'+e.data));
                    login.addEventListener('input',e=>editEvents.push(e.type+':'+e.inputType+':'+e.data));
                    login.focus();
                </script></body></html>"#,
                1,
                "null",
            )
            .unwrap();

        for character in ["m", "i", "k", "u"] {
            session
                .dispatch(
                    "Input.dispatchKeyEvent",
                    json!({"type":"keyDown","key":character,"text":character}),
                )
                .unwrap();
        }
        session
            .dispatch(
                "Input.dispatchKeyEvent",
                json!({"type":"keyDown","key":"ArrowLeft"}),
            )
            .unwrap();
        session
            .dispatch(
                "Input.dispatchKeyEvent",
                json!({"type":"keyDown","key":"Backspace"}),
            )
            .unwrap();

        let state = session
            .dispatch(
                "Runtime.evaluate",
                json!({
                    "expression":"JSON.stringify({value:login.value,start:login.selectionStart,end:login.selectionEnd,events:editEvents})",
                    "returnByValue":true
                }),
            )
            .unwrap();
        assert_eq!(
            state["result"]["value"],
            r#"{"value":"miu","start":2,"end":2,"events":["beforeinput:insertText:m","input:insertText:m","beforeinput:insertText:i","input:insertText:i","beforeinput:insertText:k","input:insertText:k","beforeinput:insertText:u","input:insertText:u","beforeinput:deleteContentBackward:null","input:deleteContentBackward:null"]}"#
        );
    }
}
