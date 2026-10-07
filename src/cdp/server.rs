//! JSON-RPC server for CDP clients: WebSocket client bookkeeping, request
//! parsing, method dispatch, and deferred responses.

use super::*;

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
pub struct DeferredResponseToken(pub(super) u64);

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
    pub(super) next_deferred_token: u64,
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

    pub(super) fn deferred_response_client(&self, token: DeferredResponseToken) -> Option<u64> {
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

        let (frame, _) = WebSocketFrame::decode_client(bytes)?;
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
