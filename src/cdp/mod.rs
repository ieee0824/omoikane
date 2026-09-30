//! CDP transport primitives: WebSocket upgrade, frame handling, and JSON-RPC routing.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use boa_engine::JsValue;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use crate::accessibility::{
    AccessibilityNode, AccessibilityProperty, AccessibilityTree, AccessibilityValue,
};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::error_reporting::{
    ErrorCategory, ErrorCode, ErrorReporter, ErrorSeverity, ExecutionSurface, RawEvent,
};
use crate::html::{TreeBuilder, decode_html_response};
use crate::http::{Client, HttpRequest, Method};
use crate::http::{HttpParseError, url::UrlParseError};
#[cfg(test)]
use crate::js::PageTaskSource;
use crate::js::{
    CompletedPageTask, FindInPageAction, FormStateRestoreMode, FormStateSnapshot,
    FullscreenTransition, JavaScriptDialog, JavaScriptDialogController, JavaScriptDialogError,
    JavaScriptDialogKind, JsRuntime, NavigationRequest, OwnedPageTask, PageTaskError,
    PointerLockTransition, StorageManager, VisitSource,
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

/// Errors produced while constructing or navigating a CDP page session.
#[derive(Debug)]
pub enum CdpSessionError {
    /// JavaScript runtime setup or document installation failed.
    JavaScript(boa_engine::JsError),
    /// The requested navigation URL was invalid.
    Url(UrlParseError),
    /// Loading the requested document failed.
    Http(HttpParseError),
    /// A page startup task completed for a retired document generation.
    StalePageTaskCompletion,
    /// A page startup task was cancelled.
    PageTaskCancelled,
    /// A page startup task exceeded its wall-clock limit.
    PageTaskTimedOut,
    /// The browser session no longer has an active page.
    Unavailable,
}

impl std::fmt::Display for CdpSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JavaScript(error) => write!(f, "{error}"),
            Self::Url(error) => write!(f, "{error}"),
            Self::Http(error) => write!(f, "{error}"),
            Self::StalePageTaskCompletion => write!(f, "stale page startup task completion"),
            Self::PageTaskCancelled => write!(f, "page startup task was cancelled"),
            Self::PageTaskTimedOut => {
                write!(f, "page startup task exceeded its wall-clock timeout")
            }
            Self::Unavailable => write!(f, "Browser session is unavailable"),
        }
    }
}

impl std::error::Error for CdpSessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::JavaScript(error) => Some(error),
            Self::Url(error) => Some(error),
            Self::Http(error) => Some(error),
            _ => None,
        }
    }
}

impl From<boa_engine::JsError> for CdpSessionError {
    fn from(error: boa_engine::JsError) -> Self {
        Self::JavaScript(error)
    }
}

impl From<UrlParseError> for CdpSessionError {
    fn from(error: UrlParseError) -> Self {
        Self::Url(error)
    }
}

impl From<HttpParseError> for CdpSessionError {
    fn from(error: HttpParseError) -> Self {
        Self::Http(error)
    }
}

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

/// Minimal stateful CDP session spanning Accessibility, Page, DOM, Network,
/// Runtime, Target, and Input.
#[derive(Debug)]
pub struct CdpSession {
    runtime: JsRuntime,
    _storage_lifetime: TabStorageLifetime,
    storage_manager: StorageManager,
    storage_session_id: u64,
    http_client: Client,
    cookie_store: Arc<Mutex<crate::http::CookieJar>>,
    current_url: String,
    last_html: String,
    frame_id: String,
    next_loader_id: u64,
    next_node_id: u64,
    node_to_id: HashMap<usize, u64>,
    id_to_node: HashMap<u64, NodeHandle>,
    next_object_id: u64,
    /// Document generation which owns each live Runtime remote-object handle.
    /// Cross-document commits clear the table; same-document history changes
    /// intentionally preserve it.
    remote_object_generations: HashMap<String, u64>,
    next_browser_context_id: u64,
    browser_context_ids: Vec<String>,
    pending_events: Vec<CdpEvent>,
    last_key_event: Option<Value>,
    last_mouse_event: Option<Value>,
    touch_target: Option<NodeHandle>,
    last_touch_position: Option<(f64, f64)>,
    touch_scroll_allowed: bool,
    mouse_pressed_target: Option<usize>,
    drag_candidate: bool,
    drag_active: bool,
    history_entries: Vec<SessionHistoryEntry>,
    history_index: usize,
    document_generation: u64,
    accessibility_enabled: bool,
    fullscreen_supported: bool,
    fullscreen_transition_allowed: bool,
    /// Host presentation state and CDP lifecycle override are independent.
    host_hidden: bool,
    lifecycle_frozen: bool,
    pending_fullscreen_transition: Option<FullscreenTransition>,
    pointer_lock_deferred: bool,
    pointer_lock_focused: bool,
    pending_pointer_release: bool,
    last_pointer_position: Option<(f64, f64)>,
    previous_input_locked: bool,
}

/// Declared after the runtime so workers and child realms finish dropping
/// before the closed tab's session storage is released.
#[derive(Debug)]
struct TabStorageLifetime {
    manager: StorageManager,
    session_id: u64,
}

impl Drop for TabStorageLifetime {
    fn drop(&mut self) {
        self.manager.remove_session(self.session_id);
    }
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
    form_state: Option<FormStateSnapshot>,
    document_generation: u64,
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

#[derive(Clone, Copy)]
enum CdpFailure {
    Request,
    Navigation,
    PageTask,
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
    pub fn new() -> Result<Self, CdpSessionError> {
        Self::with_storage_manager(StorageManager::new())
    }

    /// Creates a tab with its own session storage in a shared browser profile.
    pub fn with_storage_manager(storage_manager: StorageManager) -> Result<Self, CdpSessionError> {
        let storage_session_id = storage_manager.create_session();
        let storage_lifetime = TabStorageLifetime {
            manager: storage_manager.clone(),
            session_id: storage_session_id,
        };
        let cookie_store = Arc::new(Mutex::new(crate::http::CookieJar::new()));
        let mut runtime = JsRuntime::with_document_url_and_storage(
            TreeBuilder::parse("<html><head></head><body></body></html>").document(),
            "about:blank",
            storage_manager.clone(),
            storage_session_id,
        )
        .map_err(CdpSessionError::JavaScript)?;
        runtime.set_shared_cookie_store(Arc::clone(&cookie_store));
        let mut http_client = Client::new();
        http_client.set_shared_cookie_store(Arc::clone(&cookie_store));
        let mut session = Self {
            runtime,
            _storage_lifetime: storage_lifetime,
            storage_manager,
            storage_session_id,
            http_client,
            cookie_store,
            current_url: "about:blank".to_string(),
            last_html: String::new(),
            frame_id: "frame-0".to_string(),
            next_loader_id: 1,
            next_node_id: 1,
            node_to_id: HashMap::new(),
            id_to_node: HashMap::new(),
            next_object_id: 0,
            remote_object_generations: HashMap::new(),
            next_browser_context_id: 0,
            browser_context_ids: Vec::new(),
            pending_events: Vec::new(),
            last_key_event: None,
            last_mouse_event: None,
            touch_target: None,
            last_touch_position: None,
            touch_scroll_allowed: true,
            mouse_pressed_target: None,
            drag_candidate: false,
            drag_active: false,
            history_entries: vec![SessionHistoryEntry {
                url: "about:blank".to_string(),
                state_json: "null".to_string(),
                form_state: None,
                document_generation: 0,
            }],
            history_index: 0,
            document_generation: 0,
            accessibility_enabled: false,
            fullscreen_supported: true,
            fullscreen_transition_allowed: true,
            host_hidden: false,
            lifecycle_frozen: false,
            pending_fullscreen_transition: None,
            pointer_lock_deferred: false,
            pointer_lock_focused: true,
            pending_pointer_release: false,
            last_pointer_position: None,
            previous_input_locked: false,
        };
        session
            .install_runtime_helpers()
            .map_err(CdpSessionError::JavaScript)?;
        session.rebuild_node_index();
        Ok(session)
    }

    fn runtime_timeout(&self) -> Duration {
        self.runtime.sandbox_timeout()
    }

    /// Dispatches a CDP domain method and returns the result payload.
    pub fn dispatch(&mut self, method: &str, params: Value) -> Result<Value, JsonRpcError> {
        let result = match method {
            "Page.navigate" => self.page_navigate(&params),
            "Page.reload" => self.page_reload(),
            "Page.getFrameTree" => Ok(self.page_get_frame_tree()),
            "Page.setWebLifecycleState" => self.page_set_web_lifecycle_state(&params),
            "DOM.getDocument" => self.dom_get_document(&params),
            "DOM.getAttributes" => self.dom_get_attributes(&params),
            "DOM.querySelector" => self.dom_query_selector(&params),
            "DOM.getOuterHTML" => self.dom_get_outer_html(&params),
            "Accessibility.enable" => {
                self.accessibility_enabled = true;
                Ok(json!({}))
            }
            "Accessibility.disable" => {
                self.accessibility_enabled = false;
                Ok(json!({}))
            }
            "Accessibility.getFullAXTree" => self.accessibility_get_full_tree(&params),
            "Accessibility.getRootAXNode" => self.accessibility_get_root_node(&params),
            "Accessibility.getPartialAXTree" => self.accessibility_get_partial_tree(&params),
            "Accessibility.getAXNodeAndAncestors" => {
                self.accessibility_get_node_and_ancestors(&params)
            }
            "Accessibility.getChildAXNodes" => self.accessibility_get_child_nodes(&params),
            "Accessibility.queryAXTree" => self.accessibility_query_tree(&params),
            "Runtime.evaluate" => self.runtime_evaluate(&params),
            "Runtime.callFunctionOn" => self.runtime_call_function_on(&params),
            "Runtime.releaseObject" => self.runtime_release_object(&params),
            "Omoikane.findInPage" => self.omoikane_find_in_page(&params),
            "Target.createBrowserContext" => Ok(self.target_create_browser_context()),
            "Target.getBrowserContexts" => Ok(self.target_get_browser_contexts()),
            "Target.disposeBrowserContext" => self.target_dispose_browser_context(&params),
            "Input.dispatchKeyEvent" => self.input_dispatch_key_event(&params),
            "Input.dispatchMouseEvent" => self.input_dispatch_mouse_event(&params),
            "Input.dispatchTouchEvent" => self.input_dispatch_touch_event(&params),
            "Input.imeSetComposition" => self.input_ime_set_composition(&params),
            "Input.insertText" => self.input_insert_text(&params),
            _ => Err(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {method}"),
            }),
        };
        if result.is_err() {
            self.report_cdp_failure(if matches!(method, "Page.navigate" | "Page.reload") {
                CdpFailure::Navigation
            } else {
                CdpFailure::Request
            });
        }
        result
    }

    /// Returns and clears queued protocol events.
    pub fn drain_events(&mut self) -> Vec<CdpEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Returns the current document.
    pub fn document(&self) -> NodeHandle {
        self.runtime.document()
    }

    fn omoikane_find_in_page(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let action = match require_string(params, "action")?.as_str() {
            "start" => FindInPageAction::Start,
            "next" => FindInPageAction::Next,
            "previous" => FindInPageAction::Previous,
            "status" => FindInPageAction::Status,
            "stop" => FindInPageAction::Stop,
            _ => return Err(invalid_params("Invalid find-in-page action".to_string())),
        };
        let query = if action == FindInPageAction::Start {
            require_string(params, "query")?
        } else {
            String::new()
        };
        let result = self
            .runtime
            .find_in_page(action, &query)
            .map_err(|error| JsonRpcError {
                code: -32000,
                message: error.to_string(),
            })?;
        serde_json::to_value(result).map_err(|error| JsonRpcError {
            code: -32000,
            message: error.to_string(),
        })
    }

    /// Returns the URL of the currently loaded document.
    pub fn current_url(&self) -> &str {
        &self.current_url
    }

    /// Renders the current document using its input/CSSOM layout.
    pub(crate) fn paint_current_document(
        &mut self,
    ) -> Result<crate::paint::Canvas, crate::paint::PaintError> {
        self.runtime.paint_current_document()
    }

    /// Attaches the optional error reporter to this session's runtime and HTTP client.
    pub fn set_error_reporter(&mut self, reporter: Arc<ErrorReporter>, surface: ExecutionSurface) {
        self.runtime
            .set_error_reporter(Arc::clone(&reporter), surface);
        self.http_client.set_error_reporter(reporter, surface);
    }

    fn report_cdp_failure(&self, failure: CdpFailure) {
        let Some((reporter, surface)) = self.runtime.error_reporter_destination() else {
            return;
        };
        let (code, operation) = match failure {
            CdpFailure::Request => ("CDP_REQUEST_FAILED", "execute"),
            CdpFailure::Navigation => ("CDP_NAVIGATION_FAILED", "navigate"),
            CdpFailure::PageTask => ("CDP_PAGE_TASK_FAILED", "execute"),
        };
        reporter.report(
            RawEvent::new(
                ErrorCategory::Cdp,
                ErrorSeverity::Error,
                ErrorCode::new(code).expect("static CDP error code"),
                surface,
                "CDP operation failed",
                &[("operation", operation)],
            )
            .sanitize(),
        );
    }

    pub(crate) fn report_paint_failure(&self, code: &'static str) {
        self.runtime.report_paint_failure(code);
    }

    /// Updates the active page's layout and script-visible viewport dimensions.
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.runtime.set_viewport(width as f32, height as f32);
    }

    /// Updates the page visibility reported to scripts by its native host.
    /// A frozen CDP lifecycle remains hidden until it is reactivated.
    pub fn set_host_visibility(&mut self, hidden: bool) {
        self.host_hidden = hidden;
        self.runtime
            .set_page_visibility(hidden || self.lifecycle_frozen);
    }

    fn page_set_web_lifecycle_state(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let state = require_string(params, "state")?;
        self.lifecycle_frozen = match state.as_str() {
            "active" => false,
            "frozen" => true,
            _ => {
                return Err(JsonRpcError {
                    code: -32602,
                    message: format!("Unsupported lifecycle state: {state}"),
                });
            }
        };
        self.runtime
            .set_page_visibility(self.host_hidden || self.lifecycle_frozen);
        Ok(json!({}))
    }

    /// Updates the visible portion of the active page for host-driven zoom or
    /// on-screen-keyboard changes. Values are expressed in CSS pixels.
    pub fn set_visual_viewport(
        &mut self,
        width: f32,
        height: f32,
        offset_left: f32,
        offset_top: f32,
        scale: f32,
    ) {
        self.runtime
            .set_visual_viewport(width, height, offset_left, offset_top, scale);
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

    /// Enables or disables fullscreen support for this tab's presentation host.
    pub fn set_fullscreen_supported(&mut self, supported: bool) {
        self.fullscreen_supported = supported;
        self.runtime.set_fullscreen_supported(supported);
        if let Some(transition) = self.runtime.take_fullscreen_transition() {
            self.pending_fullscreen_transition = Some(transition);
        }
    }

    /// Controls whether the presentation host accepts fullscreen transitions.
    pub fn set_fullscreen_transition_allowed(&mut self, allowed: bool) {
        self.fullscreen_transition_allowed = allowed;
        self.runtime.set_fullscreen_transition_allowed(allowed);
    }

    /// Takes the next native window-state transition requested by the page.
    pub fn take_fullscreen_transition(&mut self) -> Option<FullscreenTransition> {
        self.pending_fullscreen_transition
            .take()
            .or_else(|| self.runtime.take_fullscreen_transition())
    }

    /// Notifies the DOM that the native host left fullscreen independently.
    pub fn fullscreen_exited_by_host(&mut self) -> Result<bool, JsonRpcError> {
        self.runtime.exit_fullscreen_from_host().map_err(js_error)
    }

    /// Configures an external pointer-lock host, including after navigation.
    pub fn set_pointer_lock_deferred(&mut self, deferred: bool) {
        self.pointer_lock_deferred = deferred;
        self.runtime.set_pointer_lock_deferred(deferred);
    }

    /// Takes the next cursor operation requested by the page.
    pub fn take_pointer_lock_transition(&mut self) -> Option<PointerLockTransition> {
        if std::mem::take(&mut self.pending_pointer_release) {
            Some(PointerLockTransition::Release)
        } else {
            self.runtime.take_pointer_lock_transition()
        }
    }

    /// Acknowledges an actual native cursor-grab attempt.
    pub fn complete_pointer_lock_request(
        &mut self,
        id: u64,
        accepted: bool,
    ) -> Result<bool, JsonRpcError> {
        self.runtime
            .complete_pointer_lock_request(id, accepted)
            .map_err(js_error)
    }

    /// Releases pointer lock and pending requests on a host-originated exit.
    pub fn release_pointer_lock_from_host(&mut self) -> Result<(), JsonRpcError> {
        self.last_pointer_position = None;
        self.runtime
            .release_pointer_lock_from_host()
            .map_err(js_error)
    }

    /// Updates the native window focus and releases lock when focus is lost.
    pub fn set_pointer_lock_focus(&mut self, focused: bool) -> Result<(), JsonRpcError> {
        self.pointer_lock_focused = focused;
        self.last_pointer_position = None;
        self.runtime
            .set_pointer_lock_focus(focused)
            .map_err(js_error)
    }

    /// Whether native relative movement should currently be delivered.
    pub fn is_pointer_locked(&self) -> bool {
        self.runtime.pointer_lock_target().is_some()
    }

    /// Resets movement deltas after the cursor leaves the surface.
    pub fn reset_pointer_movement(&mut self) {
        self.last_pointer_position = None;
    }

    /// Clears pointer movement and hover state when the pointer leaves the view.
    pub fn pointer_left_surface(&mut self) {
        self.reset_pointer_movement();
        self.runtime.clear_pointer_hover();
    }

    pub(crate) fn http_client_mut(&mut self) -> &mut Client {
        &mut self.http_client
    }

    fn leave_fullscreen_for_navigation(&mut self) {
        let _ = self.release_pointer_lock_from_host();
        self.pending_pointer_release |= matches!(
            self.runtime.take_pointer_lock_transition(),
            Some(PointerLockTransition::Release)
        );
        let _ = self.runtime.exit_fullscreen_from_host();
        if let Some(transition) = self.runtime.take_fullscreen_transition() {
            self.pending_fullscreen_transition = Some(transition);
        }
    }

    fn dispatch_document_departure(&mut self) {
        let _ = self
            .runtime
            .eval("window.dispatchEvent(new Event('beforeunload'))");
        let _ = self.runtime.run_jobs();
        self.runtime.set_page_visibility(true);
        let _ = self.runtime.eval(
            "window.dispatchEvent(new Event('pagehide')); \
             window.dispatchEvent(new Event('unload')); \
             if (typeof globalThis.__omoikane_permission_teardown === 'function') \
             globalThis.__omoikane_permission_teardown();",
        );
        let _ = self.runtime.run_jobs();
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

    pub(crate) fn prepare_page_reload(&mut self) -> Result<PreparedPageNavigation, JsonRpcError> {
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
            .map_err(|error| JsonRpcError {
                code: -32000,
                message: error.to_string(),
            })?;
        let form_state = self.navigation_form_state(history_commit, &document_url)?;
        let (history_length, history_state) = self.prospective_history_state(history_commit);
        let (task, mut commit) = self
            .prepare_document_page_task(
                &document_url,
                &html,
                history_length,
                &history_state,
                &csp_headers,
                form_state.as_ref(),
            )
            .map_err(|error| JsonRpcError {
                code: -32000,
                message: error.to_string(),
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

    fn navigate_to(&mut self, url: &str, commit: NavigationCommit) -> Result<Value, JsonRpcError> {
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

        let same_document_navigation = match commit {
            NavigationCommit::Traverse(index) => {
                self.history_entries[index].document_generation == self.document_generation
            }
            _ => is_fragment_only_navigation(&self.current_url, url),
        };
        if method == Method::Get && commit != NavigationCommit::Reload && same_document_navigation {
            let form_state = self.navigation_form_state(commit, url)?;
            let previous_url = self.current_url.clone();
            self.commit_history_url(url, commit, None);
            self.current_url = url.to_string();
            self.runtime.commit_same_document_url(url);
            self.sync_history_length()?;
            self.runtime
                .eval(&format!(
                    "__omoikane_commit_same_document_navigation({url:?}, {previous_url:?})"
                ))
                .and_then(|_| self.runtime.run_jobs())
                .map_err(js_error)?;
            if let Some(state) = form_state {
                self.runtime
                    .restore_form_state(&state, FormStateRestoreMode::Restore)
                    .map_err(js_error)?;
            }
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
            .map_err(|error| JsonRpcError {
                code: -32000,
                message: error.to_string(),
            })?;

        let form_state = self.navigation_form_state(commit, &document_url)?;
        let (next_history_length, next_history_state) = self.prospective_history_state(commit);
        self.install_document_with_csp(
            &document_url,
            &html,
            next_history_length,
            &next_history_state,
            &csp_headers,
            form_state.as_ref(),
        )
        .map_err(|error| JsonRpcError {
            code: -32000,
            message: error.to_string(),
        })?;

        self.commit_history_url(&document_url, commit, None);
        self.sync_history_length()?;
        if matches!(commit, NavigationCommit::Traverse(_)) {
            self.runtime
                .eval(&format!(
                    "__omoikane_commit_same_document_navigation({url:?}, {url:?})"
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

    fn record_script_visit(&self, source: Option<VisitSource>, requested_url: &str) {
        let Some(source) = source else {
            return;
        };
        self.storage_manager
            .record_page_navigation(requested_url, &self.current_url, source);
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
            let Some((request, source)) = self
                .runtime
                .take_navigation_requests_with_source()
                .into_iter()
                .next()
            else {
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
                    self.record_script_visit(source, &url);
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
                    self.record_script_visit(source, &url);
                }
                NavigationRequest::UpdateHistory {
                    url,
                    replace,
                    state_json,
                } => {
                    // A same-document entry still needs a snapshot before
                    // later changes overwrite the live controls' state.
                    self.navigation_form_state(NavigationCommit::Push, &url)?;
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
                    self.runtime.commit_history_api_url(&url);
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
                    if let Err(error) = self.navigate_to(&url, NavigationCommit::Traverse(target)) {
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
                    form_state: None,
                    document_generation: self.document_generation,
                });
                self.history_index = self.history_entries.len() - 1;
            }
            NavigationCommit::Replace => {
                self.history_entries[self.history_index] = SessionHistoryEntry {
                    url: url.to_string(),
                    state_json: state_json.unwrap_or_else(|| "null".to_string()),
                    form_state: None,
                    document_generation: self.document_generation,
                };
            }
            NavigationCommit::Reload => {
                let entry = &mut self.history_entries[self.history_index];
                entry.document_generation = self.document_generation;
                if entry.url != url {
                    entry.url = url.to_string();
                    entry.form_state = None;
                }
            }
            NavigationCommit::Traverse(index) => {
                let previous_generation = self.history_entries[index].document_generation;
                self.history_index = index;
                // When a cross-Document traversal rebuilds one Document, all
                // of its same-Document history entries now share the new
                // live generation. The next back/forward between them must
                // reuse that Document rather than fetch it again.
                for entry in &mut self.history_entries {
                    if entry.document_generation == previous_generation {
                        entry.document_generation = self.document_generation;
                    }
                }
            }
        }
    }

    /// Saves values independently of the departing runtime. Only history
    /// traversal and reload may restore them, and a redirect to another URL
    /// must not receive the original document's private form state.
    fn navigation_form_state(
        &mut self,
        commit: NavigationCommit,
        document_url: &str,
    ) -> Result<Option<FormStateSnapshot>, JsonRpcError> {
        let current = self.runtime.capture_form_state().map_err(js_error)?;
        self.history_entries[self.history_index].form_state = Some(current);
        let entry = match commit {
            NavigationCommit::Reload => &self.history_entries[self.history_index],
            NavigationCommit::Traverse(index) => &self.history_entries[index],
            NavigationCommit::Push | NavigationCommit::Replace => return Ok(None),
        };
        let saved_url = entry.url.split('#').next().unwrap_or_default();
        let loaded_url = document_url.split('#').next().unwrap_or_default();
        if saved_url != loaded_url {
            return Ok(None);
        }
        Ok(entry.form_state.clone())
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

    fn accessibility_tree(&mut self) -> AccessibilityTree {
        let document = self.runtime.document();
        let title = document
            .query_selector("title")
            .map(|title| dom_text_content(&title))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| self.current_url.clone());
        let focused_identity = self.runtime.accessibility_focused_node_identity();
        let snapshot_state = self.runtime.accessibility_snapshot_state();
        AccessibilityTree::build(
            &document,
            title,
            self.document_generation,
            focused_identity,
            &snapshot_state,
            |node| self.runtime.accessibility_render_state(node),
        )
    }

    fn accessibility_inspected_node(&mut self, target: &NodeHandle) -> Option<AccessibilityNode> {
        let document = self.runtime.document();
        let focused_identity = self.runtime.accessibility_focused_node_identity();
        let snapshot_state = self.runtime.accessibility_snapshot_state();
        AccessibilityTree::build_inspected_node(
            &document,
            target,
            self.document_generation,
            focused_identity,
            &snapshot_state,
            |node| self.runtime.accessibility_render_state(node),
        )
    }

    fn accessibility_get_full_tree(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        self.validate_accessibility_frame(params)?;
        let depth = accessibility_depth(params)?;
        let tree = self.accessibility_tree();
        let mut nodes = Vec::new();
        self.serialize_ax_subtree(&tree.root, None, depth, &mut nodes);
        Ok(json!({ "nodes": nodes }))
    }

    fn accessibility_get_root_node(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        self.require_accessibility_enabled()?;
        self.validate_accessibility_frame(params)?;
        let tree = self.accessibility_tree();
        Ok(json!({
            "node": self.serialize_ax_node(&tree.root, None, false),
        }))
    }

    fn accessibility_get_partial_tree(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let target = accessibility_tree_target(self.accessibility_dom_node(params, false)?);
        let tree = self.accessibility_tree();
        let fetch_relatives = params
            .get("fetchRelatives")
            .map(Value::as_bool)
            .unwrap_or(Some(true))
            .ok_or_else(|| invalid_params("fetchRelatives must be a boolean".to_string()))?;
        let Some(path) = tree.path_to_dom_identity(target.identity()) else {
            let reason = self
                .accessibility_inspected_node(&target)
                .and_then(|node| node.ignored_reasons.first().cloned())
                .unwrap_or_else(|| "notRendered".to_string());
            let ancestor_path = fetch_relatives
                .then(|| nearest_ax_ancestor_path(&tree, &target))
                .flatten();
            let parent_id = ancestor_path
                .as_ref()
                .and_then(|path| path.last())
                .map(|parent| parent.node_id.as_str());
            let mut nodes = vec![self.serialize_synthetic_ax_node(&target, &reason, parent_id)];
            if let Some(path) = ancestor_path {
                let parent_ids = ax_parent_ids(&tree.root);
                for (index, ancestor) in path.into_iter().rev().enumerate() {
                    let parent_id = parent_ids.get(&ancestor.node_id);
                    let mut serialized =
                        self.serialize_ax_node(ancestor, parent_id.map(String::as_str), false);
                    if index == 0 {
                        let mut child_ids = serialized["childIds"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        if !child_ids.iter().any(|id| id == "0") {
                            child_ids.insert(0, Value::String("0".to_string()));
                        }
                        serialized["childIds"] = Value::Array(child_ids);
                    }
                    nodes.push(serialized);
                }
            }
            return Ok(json!({ "nodes": nodes }));
        };
        let target_ax = *path.last().expect("accessibility path contains target");
        let mut ordered = vec![target_ax];
        if fetch_relatives {
            collect_reachable_ax_children(target_ax, &mut ordered);
            if path.len() > 1 {
                for sibling in &path[path.len() - 2].children {
                    if sibling.node_id == target_ax.node_id {
                        continue;
                    }
                    ordered.push(sibling);
                    if sibling.ignored {
                        collect_reachable_ax_children(sibling, &mut ordered);
                    }
                }
            }
            ordered.extend(path[..path.len() - 1].iter().rev().copied());
        }
        let mut seen = HashSet::new();
        ordered.retain(|node| seen.insert(node.node_id.clone()));
        Ok(json!({
            "nodes": self.serialize_ax_nodes_in_order(&tree, ordered, false),
        }))
    }

    fn accessibility_get_node_and_ancestors(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
        self.require_accessibility_enabled()?;
        let target = accessibility_tree_target(self.accessibility_dom_node(params, false)?);
        let tree = self.accessibility_tree();
        let Some(path) = tree.path_to_dom_identity(target.identity()) else {
            let reason = self
                .accessibility_inspected_node(&target)
                .and_then(|node| node.ignored_reasons.first().cloned())
                .unwrap_or_else(|| "notRendered".to_string());
            let ancestor_path = nearest_ax_ancestor_path(&tree, &target);
            let parent_id = ancestor_path
                .as_ref()
                .and_then(|path| path.last())
                .map(|parent| parent.node_id.as_str());
            let mut nodes = vec![self.serialize_synthetic_ax_node(&target, &reason, parent_id)];
            if let Some(path) = ancestor_path {
                let parent_ids = ax_parent_ids(&tree.root);
                for (index, ancestor) in path.into_iter().rev().enumerate() {
                    let parent_id = parent_ids.get(&ancestor.node_id);
                    let mut serialized =
                        self.serialize_ax_node(ancestor, parent_id.map(String::as_str), false);
                    if index == 0 {
                        let mut child_ids = serialized["childIds"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        if !child_ids.iter().any(|id| id == "0") {
                            child_ids.insert(0, Value::String("0".to_string()));
                        }
                        serialized["childIds"] = Value::Array(child_ids);
                    }
                    nodes.push(serialized);
                }
            }
            return Ok(json!({
                "nodes": nodes,
            }));
        };
        Ok(json!({
            "nodes": self.serialize_ax_nodes_in_order(
                &tree,
                path.into_iter().rev().collect(),
                false,
            ),
        }))
    }

    fn accessibility_get_child_nodes(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        self.require_accessibility_enabled()?;
        self.validate_accessibility_frame(params)?;
        let node_id = require_string(params, "id")?;
        let tree = self.accessibility_tree();
        let node = tree.find_by_node_id(&node_id).ok_or(JsonRpcError {
            code: -32000,
            message: format!("Unknown accessibility node: {node_id}"),
        })?;
        let mut children = Vec::new();
        collect_reachable_ax_children(node, &mut children);
        let nodes = self.serialize_ax_nodes_in_order(&tree, children, false);
        Ok(json!({ "nodes": nodes }))
    }

    fn accessibility_query_tree(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let target = accessibility_tree_target(self.accessibility_dom_node(params, false)?);
        let accessible_name = optional_string(params, "accessibleName")?;
        let role = optional_string(params, "role")?.map(|role| role.to_ascii_lowercase());
        let full_tree = self.accessibility_tree();
        let Some(root) = full_tree.find_by_dom_identity(target.identity()).cloned() else {
            return Ok(json!({ "nodes": [] }));
        };
        let tree = AccessibilityTree { root };
        let mut candidates = Vec::new();
        collect_ax_nodes(&tree.root, &mut candidates);
        let selected = candidates
            .into_iter()
            .filter(|node| {
                accessible_name
                    .as_ref()
                    .is_none_or(|expected| node.name == *expected)
            })
            .filter(|node| {
                role.as_ref()
                    .is_none_or(|expected| node.role.eq_ignore_ascii_case(expected))
            })
            .map(|node| node.node_id.clone())
            .collect::<HashSet<_>>();
        Ok(json!({
            "nodes": self.serialize_query_ax_nodes(&tree, &full_tree, &selected),
        }))
    }

    fn require_accessibility_enabled(&self) -> Result<(), JsonRpcError> {
        if self.accessibility_enabled {
            Ok(())
        } else {
            Err(JsonRpcError {
                code: -32000,
                message: "Accessibility has not been enabled".to_string(),
            })
        }
    }

    fn validate_accessibility_frame(&self, params: &Value) -> Result<(), JsonRpcError> {
        let Some(frame_id) = params.get("frameId") else {
            return Ok(());
        };
        let frame_id = frame_id
            .as_str()
            .ok_or_else(|| invalid_params("frameId must be a string".to_string()))?;
        if frame_id == self.frame_id {
            Ok(())
        } else {
            Err(invalid_params(format!(
                "Frame with the given frameId is not found: {frame_id}"
            )))
        }
    }

    fn accessibility_dom_node(
        &mut self,
        params: &Value,
        allow_document_default: bool,
    ) -> Result<NodeHandle, JsonRpcError> {
        if let Some(object_id) = params.get("objectId") {
            let object_id = object_id.as_str().ok_or_else(|| {
                invalid_params("Accessibility objectId must be a string".to_string())
            })?;
            return self
                .runtime
                .node_for_remote_object_id(object_id)
                .ok_or(JsonRpcError {
                    code: -32000,
                    message: format!("Remote object is not a DOM node: {object_id}"),
                });
        }
        let node_id = params
            .get("nodeId")
            .or_else(|| params.get("backendNodeId"))
            .and_then(Value::as_u64);
        match node_id {
            Some(node_id) => self.lookup_node(node_id),
            None if allow_document_default => Ok(self.runtime.document()),
            None => Err(invalid_params(
                "Missing nodeId or backendNodeId".to_string(),
            )),
        }
    }

    fn serialize_ax_subtree(
        &mut self,
        node: &AccessibilityNode,
        parent_id: Option<&str>,
        depth: i64,
        output: &mut Vec<Value>,
    ) {
        output.push(self.serialize_ax_node(node, parent_id, false));
        if depth == 0 {
            return;
        }
        let next_depth = if depth < 0 { -1 } else { depth - 1 };
        for child in &node.children {
            self.serialize_ax_subtree(child, Some(&node.node_id), next_depth, output);
        }
    }

    fn serialize_ax_nodes_in_order(
        &mut self,
        tree: &AccessibilityTree,
        nodes: Vec<&AccessibilityNode>,
        force_computed_ignored: bool,
    ) -> Vec<Value> {
        let parent_ids = ax_parent_ids(&tree.root);
        nodes
            .into_iter()
            .map(|node| {
                let parent_id = parent_ids.get(&node.node_id).map(String::as_str);
                self.serialize_ax_node(node, parent_id, force_computed_ignored)
            })
            .collect()
    }

    fn serialize_query_ax_nodes(
        &mut self,
        query_tree: &AccessibilityTree,
        full_tree: &AccessibilityTree,
        selected: &HashSet<String>,
    ) -> Vec<Value> {
        let query_parent_ids = ax_parent_ids(&query_tree.root);
        let full_parent_ids = ax_parent_ids(&full_tree.root);
        query_tree
            .nodes_preorder()
            .into_iter()
            .filter(|node| selected.contains(&node.node_id))
            .map(|node| {
                let parent_id = query_parent_ids
                    .get(&node.node_id)
                    .or_else(|| full_parent_ids.get(&node.node_id))
                    .cloned()
                    .or_else(|| {
                        nearest_ax_ancestor_path(full_tree, &node.dom_node)
                            .and_then(|path| path.last().map(|parent| parent.node_id.clone()))
                    });
                self.serialize_ax_node(node, parent_id.as_deref(), true)
            })
            .collect()
    }

    fn serialize_synthetic_ax_node(
        &mut self,
        node: &NodeHandle,
        reason: &str,
        parent_id: Option<&str>,
    ) -> Value {
        let mut payload = json!({
            "nodeId": "0",
            "ignored": true,
            "ignoredReasons": [{
                "name": reason,
                "value": { "type": "boolean", "value": true },
            }],
            "role": { "type": "role", "value": "none" },
            "backendDOMNodeId": self.ensure_node_id(node),
        });
        if let Some(parent_id) = parent_id {
            payload["parentId"] = Value::String(parent_id.to_string());
        }
        payload
    }

    fn serialize_ax_node(
        &mut self,
        node: &AccessibilityNode,
        parent_id: Option<&str>,
        force_computed_ignored: bool,
    ) -> Value {
        let backend_node_id = self.ensure_node_id(&node.dom_node);
        let mut payload = json!({
            "nodeId": node.node_id,
            "ignored": node.ignored,
            "childIds": node.children.iter().map(|child| child.node_id.clone()).collect::<Vec<_>>(),
            "backendDOMNodeId": backend_node_id,
        });
        if let Some(parent_id) = parent_id {
            payload["parentId"] = Value::String(parent_id.to_string());
        } else {
            payload["frameId"] = Value::String(self.frame_id.clone());
        }
        if node.ignored && !force_computed_ignored {
            payload["role"] = json!({ "type": "role", "value": "none" });
        }
        if !node.ignored {
            payload["role"] = json!({ "type": "role", "value": node.role });
            payload["name"] = json!({ "type": "computedString", "value": node.name });
            payload["properties"] = Value::Array(
                node.properties
                    .iter()
                    .map(|property| self.serialize_ax_property(property))
                    .collect(),
            );
            if !node.description.is_empty() {
                payload["description"] = json!({
                    "type": "computedString",
                    "value": node.description,
                });
            }
            if let Some(value) = node.value.as_ref() {
                payload["value"] = self.serialize_ax_value(value, "string");
            }
        } else if force_computed_ignored {
            payload["role"] = json!({ "type": "role", "value": node.role });
            payload["name"] = json!({ "type": "computedString", "value": node.name });
        }
        if node.ignored {
            payload["ignoredReasons"] = Value::Array(
                node.ignored_reasons
                    .iter()
                    .map(|reason| {
                        json!({
                            "name": reason,
                            "value": { "type": "boolean", "value": true },
                        })
                    })
                    .collect(),
            );
        }
        payload
    }

    fn serialize_ax_property(&mut self, property: &AccessibilityProperty) -> Value {
        json!({
            "name": property.name,
            "value": self.serialize_ax_value(&property.value, "string"),
        })
    }

    fn serialize_ax_value(&mut self, value: &AccessibilityValue, string_type: &str) -> Value {
        match value {
            AccessibilityValue::Boolean(value) => {
                json!({ "type": "boolean", "value": value })
            }
            AccessibilityValue::Integer(value) => {
                json!({ "type": "integer", "value": value })
            }
            AccessibilityValue::Number(value) => json!({ "type": "number", "value": value }),
            AccessibilityValue::String(value) => {
                json!({ "type": string_type, "value": value })
            }
            AccessibilityValue::Token(value) => json!({ "type": "token", "value": value }),
            AccessibilityValue::TokenList(value) => {
                json!({ "type": "tokenList", "value": value })
            }
            AccessibilityValue::Tristate(value) => {
                json!({ "type": "tristate", "value": value })
            }
            AccessibilityValue::IdRef {
                value,
                related_nodes,
            } => json!({
                "type": "idref",
                "value": value,
                "relatedNodes": related_nodes.iter().map(|related| {
                    json!({
                        "backendDOMNodeId": self.ensure_node_id(&related.dom_node),
                        "idref": related.idref,
                        "text": related.text,
                    })
                }).collect::<Vec<_>>(),
            }),
            AccessibilityValue::IdRefList {
                value,
                related_nodes,
            } => json!({
                "type": "idrefList",
                "value": value,
                "relatedNodes": related_nodes.iter().map(|related| {
                    json!({
                        "backendDOMNodeId": self.ensure_node_id(&related.dom_node),
                        "idref": related.idref,
                        "text": related.text,
                    })
                }).collect::<Vec<_>>(),
            }),
        }
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
        let object_id = params.get("objectId").and_then(Value::as_str);
        if let Some(object_id) = object_id {
            self.validate_remote_object(object_id)?;
        }
        let this_value = match params.get("objectId").and_then(Value::as_str) {
            Some(object_id) => {
                self.runtime
                    .remote_object(object_id)
                    .ok_or_else(|| JsonRpcError {
                        code: -32000,
                        message: format!("Cannot find remote object: {object_id}"),
                    })?
            }
            None => JsValue::undefined(),
        };

        let mut arguments = Vec::new();
        for argument in params
            .get("arguments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(object_id) = argument.get("objectId").and_then(Value::as_str) {
                self.validate_remote_object(object_id)?;
                arguments.push(self.runtime.remote_object(object_id).ok_or_else(|| {
                    JsonRpcError {
                        code: -32000,
                        message: format!("Cannot find remote object: {object_id}"),
                    }
                })?);
            } else if let Some(value) = argument.get("value") {
                // The value came from serde_json, so its textual form is a
                // data literal rather than page-provided source. Parentheses
                // keep object literals from being parsed as statement blocks.
                let source = format!("({value})");
                arguments.push(self.runtime.eval(&source).map_err(js_error)?);
            } else {
                return Err(invalid_params(
                    "Each Runtime.callFunctionOn argument must include value or objectId"
                        .to_string(),
                ));
            }
        }
        let value = self
            .runtime
            .call_function_with_arguments(&function_declaration, this_value, arguments)
            .map_err(js_error)?;
        self.runtime.run_until_idle().map_err(js_error)?;
        let result = self.serialize_evaluation_value(value, return_by_value)?;
        self.drive_navigation_requests()?;
        Ok(result)
    }

    fn runtime_release_object(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let object_id = require_string(params, "objectId")?;
        self.validate_remote_object(&object_id)?;
        self.runtime.release_remote_object(&object_id);
        self.remote_object_generations.remove(&object_id);
        Ok(json!({}))
    }

    fn validate_remote_object(&self, object_id: &str) -> Result<(), JsonRpcError> {
        if self
            .remote_object_generations
            .get(object_id)
            .is_some_and(|generation| *generation == self.document_generation)
        {
            return Ok(());
        }
        Err(JsonRpcError {
            code: -32000,
            message: format!("Could not find object with given id: {object_id}"),
        })
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
            "isComposing": params.get("isComposing").and_then(Value::as_bool).unwrap_or(false),
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
        let locked_target = self.runtime.pointer_lock_target();
        let locked = locked_target.is_some();
        if self.previous_input_locked && !locked {
            self.last_pointer_position = None;
        }
        self.previous_input_locked = locked;
        let (movement_x, movement_y) = if dom_type == "mousemove" {
            let previous = self.last_pointer_position.unwrap_or((x, y));
            let delta = (
                optional_f64(params, "movementX", x - previous.0)?,
                optional_f64(params, "movementY", y - previous.1)?,
            );
            self.last_pointer_position = Some((x, y));
            delta
        } else {
            (0.0, 0.0)
        };
        self.runtime.update_pointer_position(x, y);
        let (x, y) = self.runtime.pointer_lock_position().unwrap_or((x, y));
        let target = locked_target.or_else(|| self.runtime.hit_test(x as f32, y as f32));
        let target_node = target.clone().unwrap_or_else(|| self.runtime.document());
        let target_id = target_node.identity();
        let target_node_id = self.ensure_node_id(&target_node);
        let button = mouse_button(params.get("button").and_then(Value::as_str))?;
        let modifiers = params.get("modifiers").and_then(Value::as_u64).unwrap_or(0);
        let default_buttons = if dom_type == "mousedown" {
            button_mask(button)
        } else {
            0
        };
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
            "movementX": movement_x, "movementY": movement_y,
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
            let outcome = self.eval_input_number(&format!(
                "__omoikane_dispatch_wheel_input({target_id}, {init})"
            ))?;
            let not_canceled = outcome & 1 != 0;
            return Ok(json!({
                "defaultPrevented": !not_canceled,
                "targetNodeId": target_node_id,
                "hostOverscrollX": outcome & 2 != 0,
                "hostOverscrollY": outcome & 4 != 0,
            }));
        }
        let not_canceled = self.eval_input_bool(&format!(
            "__omoikane_dispatch_mouse_input({target_id}, {dom_type:?}, {init}, {})",
            dom_type == "mousedown"
        ))?;
        if locked {
            self.drag_active = false;
            self.drag_candidate = false;
        }
        if dom_type == "mousedown" && !locked {
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

    fn input_dispatch_touch_event(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let event_type = require_string(params, "type")?;
        let dom_type = match event_type.as_str() {
            "touchStart" | "touchstart" => "touchstart",
            "touchMove" | "touchmove" => "touchmove",
            "touchEnd" | "touchend" => "touchend",
            "touchCancel" | "touchcancel" => "touchcancel",
            _ => {
                return Err(invalid_params(format!(
                    "Unsupported Input.dispatchTouchEvent type: {event_type}"
                )));
            }
        };
        let points = params
            .get("touchPoints")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_params("touchPoints must be an array".to_string()))?;
        let point = points.first();
        let position = point
            .map(|point| {
                Ok((
                    optional_f64(point, "x", 0.0)?,
                    optional_f64(point, "y", 0.0)?,
                ))
            })
            .transpose()?;
        if dom_type == "touchstart" {
            let (x, y) = position
                .ok_or_else(|| invalid_params("touchStart requires a touch point".to_string()))?;
            self.runtime.update_pointer_position(x, y);
            self.touch_target = self
                .runtime
                .hit_test(x as f32, y as f32)
                .or_else(|| Some(self.runtime.document()));
            self.last_touch_position = Some((x, y));
            self.touch_scroll_allowed = true;
        } else if self.touch_target.is_none() {
            return Err(invalid_params("touch sequence is not active".to_string()));
        }

        let target = self
            .touch_target
            .clone()
            .unwrap_or_else(|| self.runtime.document());
        let target_id = target.identity();
        let target_node_id = self.ensure_node_id(&target);
        let (delta_x, delta_y) = match (dom_type, position, self.last_touch_position) {
            ("touchmove", Some((x, y)), Some((previous_x, previous_y))) => {
                self.last_touch_position = Some((x, y));
                (previous_x - x, previous_y - y)
            }
            _ => (0.0, 0.0),
        };
        let (fallback_x, fallback_y) = self.last_touch_position.unwrap_or((0.0, 0.0));
        let touch_values = points
            .iter()
            .enumerate()
            .map(|(index, point)| {
                json!({
                    "identifier": point.get("id").and_then(Value::as_i64).unwrap_or(index as i64),
                    "clientX": point.get("x").and_then(Value::as_f64).unwrap_or(0.0),
                    "clientY": point.get("y").and_then(Value::as_f64).unwrap_or(0.0),
                })
            })
            .collect::<Vec<_>>();
        let changed_touches = if touch_values.is_empty() {
            vec![json!({ "identifier": 0, "clientX": fallback_x, "clientY": fallback_y })]
        } else {
            touch_values.clone()
        };
        let modifiers = params.get("modifiers").and_then(Value::as_u64).unwrap_or(0);
        let init = json!({
            "touches": if matches!(dom_type, "touchend" | "touchcancel") { Vec::<Value>::new() } else { touch_values },
            "changedTouches": changed_touches,
            "deltaX": delta_x,
            "deltaY": delta_y,
            "defaultAllowed": self.touch_scroll_allowed,
            "altKey": modifiers & 1 != 0,
            "ctrlKey": modifiers & 2 != 0,
            "metaKey": modifiers & 4 != 0,
            "shiftKey": modifiers & 8 != 0,
        });
        let outcome = self.eval_input_number(&format!(
            "__omoikane_dispatch_touch_input({target_id}, {dom_type:?}, {init})"
        ))?;
        if dom_type == "touchstart" && outcome & 1 == 0 {
            self.touch_scroll_allowed = false;
        }
        if matches!(dom_type, "touchend" | "touchcancel") {
            self.touch_target = None;
            self.last_touch_position = None;
            self.touch_scroll_allowed = true;
        }
        Ok(json!({
            "defaultPrevented": outcome & 1 == 0,
            "targetNodeId": target_node_id,
            "hostOverscrollX": outcome & 2 != 0,
            "hostOverscrollY": outcome & 4 != 0,
        }))
    }

    fn input_ime_set_composition(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let text = require_string(params, "text")?;
        let length = text.encode_utf16().count() as u64;
        let selection_start = require_u64(params, "selectionStart")?;
        let selection_end = require_u64(params, "selectionEnd")?;
        if selection_start > selection_end || selection_end > length {
            return Err(invalid_params(
                "Composition selection must be ordered and within the text".to_string(),
            ));
        }
        let handled = self.eval_input_bool(&format!(
            "__omoikane_dispatch_composition_input(\"set\", {}, {selection_start}, {selection_end})",
            serde_json::to_string(&text).expect("a string is JSON serializable"),
        ))?;
        Ok(json!({ "handled": handled }))
    }

    fn input_insert_text(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let text = require_string(params, "text")?;
        let handled = self.eval_input_bool(&format!(
            "__omoikane_dispatch_composition_input(\"commit\", {})",
            serde_json::to_string(&text).expect("a string is JSON serializable"),
        ))?;
        Ok(json!({ "handled": handled }))
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

    fn eval_input_number(&mut self, script: &str) -> Result<u8, JsonRpcError> {
        let outcome = self
            .runtime
            .eval(script)
            .and_then(|value| {
                self.runtime.run_jobs()?;
                Ok(value.as_number().unwrap_or(1.0) as u8)
            })
            .map_err(js_error)?;
        self.drive_navigation_requests()?;
        Ok(outcome)
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
        let value = self
            .runtime
            .eval_async(expression)
            .await
            .map_err(js_error)?;
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
        let object_id = if return_by_value {
            None
        } else {
            let object_id = format!("remote-{}", self.next_object_id);
            self.next_object_id += 1;
            Some(object_id)
        };
        let retained_value = object_id.as_ref().map(|_| value.clone());
        let serialization_function = if return_by_value {
            "value => JSON.stringify({ result: __cdpSerializeValue(value) })".to_string()
        } else {
            let object_id = object_id
                .as_deref()
                .expect("non-value serialization always has a remote object id");
            format!(
                "value => JSON.stringify({{ result: __cdpRemoteObject(value, {object_id:?}) }})"
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
        let result: Value = serde_json::from_str(&payload).map_err(|error| JsonRpcError {
            code: -32000,
            message: error.to_string(),
        })?;
        if let Some(object_id) = object_id {
            if result
                .get("result")
                .and_then(|result| result.get("objectId"))
                .is_some()
            {
                self.runtime.retain_remote_object(
                    object_id.clone(),
                    retained_value.expect("remote object value was cloned above"),
                );
                self.remote_object_generations
                    .insert(object_id, self.document_generation);
            }
        }
        Ok(result)
    }

    fn load_page_request(
        &mut self,
        url: &str,
        method: Method,
        body: Option<Vec<u8>>,
        content_type: Option<String>,
    ) -> Result<(String, u16, String, Vec<String>), CdpSessionError> {
        if method == Method::Get {
            if url == "about:blank" {
                return Ok((
                    "<html><head></head><body></body></html>".to_string(),
                    200,
                    url.to_string(),
                    Vec::new(),
                ));
            }
            if let Some(data) = crate::http::parse_data_uri(url)
                && data.mime_type.eq_ignore_ascii_case("text/html")
            {
                return Ok((
                    String::from_utf8_lossy(&data.data).into_owned(),
                    200,
                    url.to_string(),
                    Vec::new(),
                ));
            }
        }
        let parsed: crate::http::url::Url = url.parse()?;
        let mut request = HttpRequest::new(method, parsed);
        if let Ok(site) = self.current_url.parse::<crate::http::Url>() {
            request.set_cookie_context(site, true);
        }
        if let Some(content_type) = content_type {
            request.set_header("Content-Type", content_type);
        }
        if let Some(body) = body {
            request.set_body(body);
        }
        let response = self.http_client.send(request)?;
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
    ) -> Result<(), CdpSessionError> {
        self.install_document_with_csp(url, html, history_length, history_state_json, &[], None)
    }

    fn install_document_with_csp(
        &mut self,
        url: &str,
        html: &str,
        history_length: usize,
        history_state_json: &str,
        csp_headers: &[String],
        form_state: Option<&FormStateSnapshot>,
    ) -> Result<(), CdpSessionError> {
        let document = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document_url_and_storage(
            document,
            url,
            self.storage_manager.clone(),
            self.storage_session_id,
        )
        .map_err(CdpSessionError::JavaScript)?;
        if let Some((reporter, surface)) = self.runtime.error_reporter_destination() {
            runtime.set_error_reporter(reporter, surface);
        }
        runtime.set_shared_cookie_store(Arc::clone(&self.cookie_store));
        runtime.set_initial_visibility_hidden(self.host_hidden || self.lifecycle_frozen);
        runtime.set_user_agent(self.http_client.user_agent().to_string());
        runtime.set_fullscreen_supported(self.fullscreen_supported);
        runtime.set_pointer_lock_deferred(self.pointer_lock_deferred);
        runtime
            .set_pointer_lock_focus(self.pointer_lock_focused)
            .map_err(CdpSessionError::JavaScript)?;
        runtime.set_fullscreen_transition_allowed(self.fullscreen_transition_allowed);
        Self::install_runtime_helpers_on(&mut runtime).map_err(CdpSessionError::JavaScript)?;
        runtime.install_csp_policy(csp_headers);
        if let Some(state) = form_state {
            runtime
                .restore_form_state(state, FormStateRestoreMode::Restore)
                .map_err(CdpSessionError::JavaScript)?;
        }
        runtime
            .eval(&format!(
                "__omoikane_sync_history({history_length}, {history_state_json:?})"
            ))
            .and_then(|_| runtime.run_jobs())
            .map_err(CdpSessionError::JavaScript)?;

        // The response and replacement Runtime are ready, so the active
        // Document can now leave without risking an unload followed by a
        // network failure that keeps the same Document alive. Listener errors
        // are reported like browser event-handler errors and do not cancel the
        // commit; cancellation policy for beforeunload dialogs belongs to the
        // future GUI integration.
        self.leave_fullscreen_for_navigation();
        self.runtime.terminate_workers();
        self.dispatch_document_departure();

        let base_url = url.parse::<crate::http::Url>().ok();
        let script_errors = runtime.execute_document_scripts(base_url.as_ref());
        if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
            for error in script_errors {
                eprintln!("[omoikane][js-error] {error}");
            }
        }
        runtime
            .wire_inline_event_handlers()
            .map_err(CdpSessionError::JavaScript)?;
        runtime.fire_load().map_err(CdpSessionError::JavaScript)?;

        self.runtime = runtime;
        self.mouse_pressed_target = None;
        self.touch_target = None;
        self.last_touch_position = None;
        self.touch_scroll_allowed = true;
        self.drag_candidate = false;
        self.drag_active = false;
        self.document_generation = self.document_generation.saturating_add(1);
        self.remote_object_generations.clear();
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
        form_state: Option<&FormStateSnapshot>,
    ) -> Result<(OwnedPageTask, PendingDocumentCommit), CdpSessionError> {
        let document = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document_url_and_storage(
            document,
            url,
            self.storage_manager.clone(),
            self.storage_session_id,
        )
        .map_err(CdpSessionError::JavaScript)?;
        if let Some((reporter, surface)) = self.runtime.error_reporter_destination() {
            runtime.set_error_reporter(reporter, surface);
        }
        runtime.set_shared_cookie_store(Arc::clone(&self.cookie_store));
        runtime.set_initial_visibility_hidden(self.host_hidden || self.lifecycle_frozen);
        runtime.set_user_agent(self.http_client.user_agent().to_string());
        runtime.set_fullscreen_supported(self.fullscreen_supported);
        runtime.set_pointer_lock_deferred(self.pointer_lock_deferred);
        runtime
            .set_pointer_lock_focus(self.pointer_lock_focused)
            .map_err(CdpSessionError::JavaScript)?;
        runtime.set_fullscreen_transition_allowed(self.fullscreen_transition_allowed);
        Self::install_runtime_helpers_on(&mut runtime).map_err(CdpSessionError::JavaScript)?;
        runtime.install_csp_policy(csp_headers);
        if let Some(state) = form_state {
            runtime
                .restore_form_state(state, FormStateRestoreMode::Restore)
                .map_err(CdpSessionError::JavaScript)?;
        }
        runtime
            .eval(&format!(
                "__omoikane_sync_history({history_length}, {history_state_json:?})"
            ))
            .and_then(|_| runtime.run_jobs())
            .map_err(CdpSessionError::JavaScript)?;

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
    ) -> Result<(), CdpSessionError> {
        if completed.generation != pending.generation
            || pending.generation != self.document_generation.saturating_add(1)
        {
            self.report_cdp_failure(CdpFailure::PageTask);
            return Err(CdpSessionError::StalePageTaskCompletion);
        }
        match &completed.result {
            Err(PageTaskError::Cancelled) => {
                self.report_cdp_failure(CdpFailure::PageTask);
                return Err(CdpSessionError::PageTaskCancelled);
            }
            Err(PageTaskError::TimedOut) => {
                self.report_cdp_failure(CdpFailure::PageTask);
                return Err(CdpSessionError::PageTaskTimedOut);
            }
            Ok(_) => {}
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
        self.leave_fullscreen_for_navigation();
        self.runtime.terminate_workers();
        self.dispatch_document_departure();

        self.runtime = runtime;
        self.mouse_pressed_target = None;
        self.touch_target = None;
        self.last_touch_position = None;
        self.touch_scroll_allowed = true;
        self.drag_candidate = false;
        self.drag_active = false;
        self.document_generation = pending.generation;
        self.remote_object_generations.clear();
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

fn accessibility_depth(params: &Value) -> Result<i64, JsonRpcError> {
    match params.get("depth") {
        None => Ok(-1),
        Some(depth) => depth
            .as_u64()
            .and_then(|depth| i64::try_from(depth).ok())
            .ok_or_else(|| {
                invalid_params("Accessibility depth must be a non-negative integer".to_string())
            }),
    }
}

fn dom_text_content(node: &NodeHandle) -> String {
    if node.node_type() == NodeType::Text {
        return node.data().unwrap_or_default();
    }
    node.child_nodes()
        .iter()
        .map(dom_text_content)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn collect_ax_nodes<'a>(node: &'a AccessibilityNode, output: &mut Vec<&'a AccessibilityNode>) {
    output.push(node);
    for child in &node.children {
        collect_ax_nodes(child, output);
    }
}

fn collect_reachable_ax_children<'a>(
    node: &'a AccessibilityNode,
    output: &mut Vec<&'a AccessibilityNode>,
) {
    for child in &node.children {
        output.push(child);
        if child.ignored {
            collect_reachable_ax_children(child, output);
        }
    }
}

fn ax_parent_ids(root: &AccessibilityNode) -> HashMap<String, String> {
    fn collect(node: &AccessibilityNode, parent_ids: &mut HashMap<String, String>) {
        for child in &node.children {
            parent_ids.insert(child.node_id.clone(), node.node_id.clone());
            collect(child, parent_ids);
        }
    }

    let mut parent_ids = HashMap::new();
    collect(root, &mut parent_ids);
    parent_ids
}

fn accessibility_tree_target(node: NodeHandle) -> NodeHandle {
    if node.node_type() == NodeType::DocumentFragment {
        node.shadow_host().unwrap_or(node)
    } else {
        node
    }
}

fn ax_composed_parent(node: &NodeHandle) -> Option<NodeHandle> {
    if node.node_type() == NodeType::DocumentFragment {
        return node.shadow_host();
    }
    node.assigned_slot().or_else(|| {
        node.parent_node().and_then(|parent| {
            if parent.node_type() == NodeType::DocumentFragment {
                parent.shadow_host()
            } else {
                Some(parent)
            }
        })
    })
}

fn nearest_ax_ancestor_path<'a>(
    tree: &'a AccessibilityTree,
    target: &NodeHandle,
) -> Option<Vec<&'a AccessibilityNode>> {
    let mut current = ax_composed_parent(target);
    while let Some(ancestor) = current {
        if let Some(path) = tree.path_to_dom_identity(ancestor.identity()) {
            return Some(path);
        }
        current = ax_composed_parent(&ancestor);
    }
    None
}

type SessionEvaluation = Pin<Box<dyn Future<Output = (CdpSession, Result<Value, JsonRpcError>)>>>;

struct PendingSessionEvaluation {
    token: DeferredResponseToken,
    controller: JavaScriptDialogController,
    cancelled: Rc<Cell<bool>>,
    page_url: String,
    opened: Option<JavaScriptDialog>,
    timeout_deadline: Instant,
    future: SessionEvaluation,
}

struct PendingPageNavigation {
    token: DeferredResponseToken,
    controller: JavaScriptDialogController,
    page_url: String,
    opened: Option<JavaScriptDialog>,
    timeout_deadline: Instant,
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

fn deadline_after(timeout: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(timeout).unwrap_or(now)
}

fn session_evaluation(
    mut session: CdpSession,
    expression: String,
    return_by_value: bool,
    cancelled: Rc<Cell<bool>>,
    timeout_deadline: Instant,
) -> SessionEvaluation {
    Box::pin(async move {
        let result = {
            let mut evaluation =
                Box::pin(session.evaluate_expression_async(&expression, return_by_value));
            std::future::poll_fn(|context| {
                if cancelled.get() {
                    return Poll::Ready(Err(JsonRpcError {
                        code: -32000,
                        message: "JavaScript evaluation cancelled by page teardown".to_string(),
                    }));
                }
                // The request deadline is created before this future is first
                // polled. The runtime starts its own clock later, so relying
                // on it alone can leave a suspended request pending when
                // wait_for_outgoing wakes at the earlier request deadline.
                if Instant::now() >= timeout_deadline {
                    return Poll::Ready(Err(evaluation_timeout_error()));
                }
                let result = evaluation.as_mut().poll(context);
                if Instant::now() >= timeout_deadline {
                    Poll::Ready(Err(evaluation_timeout_error()))
                } else {
                    result
                }
            })
            .await
        };
        (session, result)
    })
}

fn evaluation_timeout_error() -> JsonRpcError {
    js_error(
        boa_engine::JsNativeError::runtime_limit()
            .with_message(boa_engine::vm::WALL_CLOCK_TIMEOUT_MESSAGE)
            .into(),
    )
}

fn page_task_script_error_lines(result: &Result<Vec<String>, PageTaskError>) -> Vec<String> {
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
        if let Some(message) =
            pending_evaluation_busy_message(self.pending.is_some(), self.pending_page.is_some())
        {
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
        let session = self.session.take().ok_or(JsonRpcError {
            code: -32000,
            message: "Browser session is busy".to_string(),
        })?;
        // The controller belongs only to this in-flight evaluation. It is
        // discarded together with the evaluation before a replacement Runtime
        // is installed, so dialog ids reused after navigation cannot resolve an
        // older suspension.
        let controller = session.runtime.javascript_dialog_controller();
        let page_url = session.current_url.clone();
        let timeout_deadline = deadline_after(session.runtime_timeout());
        let cancelled = Rc::new(Cell::new(false));
        let future = session_evaluation(
            session,
            expression,
            return_by_value,
            Rc::clone(&cancelled),
            timeout_deadline,
        );
        self.pending = Some(PendingSessionEvaluation {
            token,
            controller,
            cancelled,
            page_url,
            opened: None,
            timeout_deadline,
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
        let timeout_deadline = deadline_after(session.runtime_timeout());
        let prepared = if method == "Page.reload" {
            session.prepare_page_reload()
        } else {
            session.prepare_page_navigate(params)
        };
        if prepared.is_err() {
            session.report_cdp_failure(CdpFailure::Navigation);
        }
        let prepared = prepared?;
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
            timeout_deadline,
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
            let waker: &'static Waker = Waker::noop();
            let mut context = TaskContext::from_waker(waker);
            match pending.task.as_mut().poll(&mut context) {
                Poll::Ready(completed) => {
                    if pending.opened.take().is_some() {
                        self.actions.push(BrowserSessionAction::Notify(
                            "Page.javascriptDialogClosed",
                            json!({ "result": false, "userInput": "" }),
                        ));
                    }
                    let result = self
                        .session
                        .as_mut()
                        .ok_or(CdpSessionError::Unavailable)
                        .and_then(|session| {
                            session.commit_document_page_task(completed, pending.commit)
                        })
                        .map(|()| pending.response)
                        .map_err(|error| JsonRpcError {
                            code: -32000,
                            message: error.to_string(),
                        });
                    self.actions
                        .push(BrowserSessionAction::Complete(pending.token, result));
                    return;
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
                if result.is_err() {
                    session.report_cdp_failure(CdpFailure::Request);
                }
                self.session = Some(session);
                if pending.opened.take().is_some() {
                    self.actions.push(BrowserSessionAction::Notify(
                        "Page.javascriptDialogClosed",
                        json!({ "result": false, "userInput": "" }),
                    ));
                }
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

    fn next_pending_timeout_deadline(&self) -> Option<Instant> {
        self.pending
            .as_ref()
            .map(|pending| pending.timeout_deadline)
            .into_iter()
            .chain(
                self.pending_page
                    .as_ref()
                    .map(|pending| pending.timeout_deadline),
            )
            .min()
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
    pub fn new() -> Result<Self, CdpSessionError> {
        let state = Rc::new(RefCell::new(BrowserSessionState {
            session: Some(CdpSession::new()?),
            pending: None,
            pending_page: None,
            actions: Vec::new(),
        }));
        let mut server = CdpServer::new();

        let evaluation_state = Rc::clone(&state);
        server.register_deferred_method("Runtime.evaluate", move |token, params| {
            evaluation_state
                .borrow_mut()
                .begin_evaluation(token, params)
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
            "Accessibility.enable",
            "Accessibility.disable",
            "Accessibility.getFullAXTree",
            "Accessibility.getRootAXNode",
            "Accessibility.getPartialAXTree",
            "Accessibility.getAXNodeAndAncestors",
            "Accessibility.getChildAXNodes",
            "Accessibility.queryAXTree",
            "Runtime.callFunctionOn",
            "Target.createBrowserContext",
            "Target.getBrowserContexts",
            "Target.disposeBrowserContext",
            "Input.dispatchKeyEvent",
            "Input.dispatchMouseEvent",
            "Input.dispatchTouchEvent",
            "Input.imeSetComposition",
            "Input.insertText",
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
                frame.opcode == WebSocketOpcode::Close && self.owner_client_id == Some(client_id)
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

    pub fn drain_outgoing(&mut self, client_id: u64) -> Result<Vec<WebSocketFrame>, CdpError> {
        self.flush_actions()?;
        self.server.drain_outgoing(client_id)
    }

    /// Waits for queued output or the next pending timeout deadline, then drains
    /// the current outbound frames for a client.
    pub fn wait_for_outgoing(&mut self, client_id: u64) -> Result<Vec<WebSocketFrame>, CdpError> {
        self.flush_actions()?;
        let mut outgoing = self.server.drain_outgoing(client_id)?;
        if !outgoing.is_empty() || self.server.pending_response_count() == 0 {
            return Ok(outgoing);
        }
        let deadline = { self.state.borrow().next_pending_timeout_deadline() };
        if let Some(deadline) = deadline {
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            self.flush_actions()?;
            outgoing.extend(self.server.drain_outgoing(client_id)?);
        }
        Ok(outgoing)
    }

    pub fn pending_response_count(&self) -> usize {
        self.server.pending_response_count()
    }

    fn flush_actions(&mut self) -> Result<(), CdpError> {
        self.state.borrow_mut().poll_evaluation();
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

fn optional_string(params: &Value, key: &'static str) -> Result<Option<String>, JsonRpcError> {
    match params.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .map(|value| Some(value.to_string()))
            .ok_or_else(|| invalid_params(format!("Invalid string parameter: {key}"))),
    }
}

fn require_u64(params: &Value, key: &'static str) -> Result<u64, JsonRpcError> {
    params.get(key).and_then(Value::as_u64).ok_or(JsonRpcError {
        code: -32602,
        message: format!("Missing or invalid numeric parameter: {key}"),
    })
}

fn invalid_params(message: String) -> JsonRpcError {
    JsonRpcError {
        code: -32602,
        message,
    }
}

fn optional_f64(params: &Value, key: &'static str, default: f64) -> Result<f64, JsonRpcError> {
    let value = params.get(key).map(Value::as_f64).unwrap_or(Some(default));
    value
        .filter(|value| value.is_finite())
        .ok_or_else(|| invalid_params(format!("Missing or invalid numeric parameter: {key}")))
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

fn cdp_node_type(node: &NodeHandle) -> u8 {
    if node.is_cdata_section() {
        return 4;
    }
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
        NodeType::Text if node.is_cdata_section() => {
            format!("<![CDATA[{}]]>", node.data().unwrap_or_default())
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

#[cfg(test)]
mod tests;
