//! Host-owned registration boundary for custom protocol handlers.

use super::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Requested mutation of the host protocol-handler registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolHandlerOperation {
    /// Add or update a handler.
    Register,
    /// Remove a matching handler.
    Unregister,
}

/// Fully normalized request passed to the host.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProtocolHandler {
    /// ASCII-lowercase handled scheme.
    pub scheme: String,
    /// Absolute same-origin HTTP(S) URL template.
    pub url_template: String,
    /// Serialized caller origin.
    pub origin: String,
}

/// Host decision for a valid registration request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolHandlerDisposition {
    /// User or automation accepted the mutation.
    Accepted,
    /// User or policy rejected the mutation without exposing that choice.
    Rejected,
    /// The embedding platform has no protocol-handler backend.
    Unsupported,
}

/// Failure reported by the host registry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolHandlerError {
    /// Human-readable failure detail.
    pub message: String,
}

/// Embedder contract for persistent, permission-controlled handler state.
pub trait ProtocolHandlerProvider: Send + Sync {
    /// Applies one validated registry mutation.
    fn update(
        &self,
        operation: ProtocolHandlerOperation,
        handler: ProtocolHandler,
    ) -> Result<ProtocolHandlerDisposition, ProtocolHandlerError>;
}

/// Deterministic provider for headless embedders and tests.
#[derive(Debug)]
pub struct DeterministicProtocolHandlerProvider {
    disposition: Mutex<ProtocolHandlerDisposition>,
    requests: Mutex<Vec<(ProtocolHandlerOperation, ProtocolHandler)>>,
    registrations: Mutex<HashSet<ProtocolHandler>>,
    next_error: Mutex<Option<ProtocolHandlerError>>,
}

impl Default for DeterministicProtocolHandlerProvider {
    fn default() -> Self {
        Self {
            disposition: Mutex::new(ProtocolHandlerDisposition::Unsupported),
            requests: Mutex::new(Vec::new()),
            registrations: Mutex::new(HashSet::new()),
            next_error: Mutex::new(None),
        }
    }
}

impl DeterministicProtocolHandlerProvider {
    /// Selects the decision returned for subsequent successful requests.
    pub fn set_disposition(&self, disposition: ProtocolHandlerDisposition) {
        *self.disposition.lock().unwrap() = disposition;
    }

    /// Makes the next host operation fail.
    pub fn fail_next(&self, error: ProtocolHandlerError) {
        *self.next_error.lock().unwrap() = Some(error);
    }

    /// Returns owned requests in observation order.
    pub fn requests(&self) -> Vec<(ProtocolHandlerOperation, ProtocolHandler)> {
        self.requests.lock().unwrap().clone()
    }

    /// Returns the currently accepted registrations.
    pub fn registrations(&self) -> HashSet<ProtocolHandler> {
        self.registrations.lock().unwrap().clone()
    }
}

impl ProtocolHandlerProvider for DeterministicProtocolHandlerProvider {
    fn update(
        &self,
        operation: ProtocolHandlerOperation,
        handler: ProtocolHandler,
    ) -> Result<ProtocolHandlerDisposition, ProtocolHandlerError> {
        self.requests
            .lock()
            .unwrap()
            .push((operation, handler.clone()));
        if let Some(error) = self.next_error.lock().unwrap().take() {
            return Err(error);
        }
        let disposition = *self.disposition.lock().unwrap();
        if disposition == ProtocolHandlerDisposition::Accepted {
            let mut registrations = self.registrations.lock().unwrap();
            match operation {
                ProtocolHandlerOperation::Register => {
                    registrations.replace(handler);
                }
                ProtocolHandlerOperation::Unregister => {
                    registrations.remove(&handler);
                }
            }
        }
        Ok(disposition)
    }
}

pub(super) struct State {
    provider: Arc<dyn ProtocolHandlerProvider>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicProtocolHandlerProvider::default()),
        }
    }
}

const SAFELISTED_SCHEMES: &[&str] = &[
    "bitcoin",
    "ftp",
    "ftps",
    "geo",
    "im",
    "irc",
    "ircs",
    "magnet",
    "mailto",
    "matrix",
    "mms",
    "news",
    "nntp",
    "openpgp4fpr",
    "sftp",
    "sip",
    "sms",
    "smsto",
    "ssh",
    "tel",
    "urn",
    "webcal",
    "wtai",
    "xmpp",
];

fn valid_scheme(scheme: &str) -> bool {
    SAFELISTED_SCHEMES.contains(&scheme)
        || scheme.strip_prefix("web+").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_lowercase())
        })
}

fn normalize(
    host: &HostState,
    document_id: usize,
    scheme: String,
    template: String,
) -> Result<ProtocolHandler, (&'static str, &'static str)> {
    let scheme = scheme.to_ascii_lowercase();
    if !valid_scheme(&scheme) {
        return Err(("SecurityError", "The scheme is not allowed"));
    }
    if !template.contains("%s") {
        return Err(("SyntaxError", "The URL template must contain %s"));
    }
    let Some(base) = host.base_url_for_document(document_id) else {
        return Err(("SecurityError", "Protocol handlers require a tuple origin"));
    };
    let parsed = url::Url::parse(&template)
        .or_else(|_| url::Url::parse(&base.to_string()).and_then(|base| base.join(&template)))
        .map_err(|_| ("SyntaxError", "The URL template is invalid"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(("SecurityError", "The handler URL must use HTTP(S)"));
    }
    let base = url::Url::parse(&base.to_string())
        .map_err(|_| ("SecurityError", "The document origin is unavailable"))?;
    if parsed.origin() != base.origin() {
        return Err(("SecurityError", "The handler URL must be same-origin"));
    }
    Ok(ProtocolHandler {
        scheme,
        url_template: parsed.to_string(),
        origin: base.origin().ascii_serialization(),
    })
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let document_id = parse_node_id(args.get(1), context)?;
    ensure_same_origin_document(context, document_id)?;
    let scheme = string_argument(args.get(2), "", context)?;
    let template = string_argument(args.get(3), "", context)?;
    let result = with_host_state(|host| {
        let host = host.borrow();
        if !host.document_is_active(document_id) {
            return Ok(
                serde_json::json!({"error":"InvalidStateError","message":"The document is not fully active"}),
            );
        }
        let handler = match normalize(&host, document_id, scheme, template) {
            Ok(handler) => handler,
            Err((name, message)) => {
                return Ok(serde_json::json!({"error":name,"message":message}));
            }
        };
        let operation = match operation.as_str() {
            "register" => ProtocolHandlerOperation::Register,
            "unregister" => ProtocolHandlerOperation::Unregister,
            _ => {
                return Err(JsNativeError::typ()
                    .with_message("Unknown protocol-handler operation")
                    .into());
            }
        };
        match host.protocol_handler.provider.update(operation, handler) {
            Ok(disposition) => Ok(serde_json::json!({"disposition": match disposition {
                ProtocolHandlerDisposition::Accepted => "accepted",
                ProtocolHandlerDisposition::Rejected => "rejected",
                ProtocolHandlerDisposition::Unsupported => "unsupported",
            }})),
            Err(error) => Ok(serde_json::json!({"error":"SecurityError","message":error.message})),
        }
    })?;
    Ok(JsValue::from(JsString::from(result.to_string().as_str())))
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_protocol_handler"),
        4,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the host registry used by subsequent protocol-handler calls.
    pub fn set_protocol_handler_provider(&mut self, provider: Arc<dyn ProtocolHandlerProvider>) {
        self.host_state.borrow_mut().protocol_handler.provider = provider;
    }
}
