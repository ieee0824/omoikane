//! Host-owned platform boundary for the Web Share API.

use super::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// One owned file supplied to a share target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareFile {
    /// File name exposed to the share target.
    pub name: String,
    /// Normalized MIME type.
    pub media_type: String,
    /// Last-modified timestamp in milliseconds.
    pub last_modified: i64,
    /// Owned file contents.
    pub bytes: Vec<u8>,
}

/// Normalized, owned data passed to the embedder share provider.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareData {
    /// Serialized caller origin.
    #[serde(default)]
    pub origin: String,
    /// Optional title.
    pub title: Option<String>,
    /// Optional text body.
    pub text: Option<String>,
    /// Parsed absolute HTTP(S) URL.
    pub url: Option<String>,
    /// Owned files.
    #[serde(default)]
    pub files: Vec<ShareFile>,
}

/// Failure reported by the host share provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShareError {
    /// DOMException name exposed to script.
    pub name: String,
    /// Human-readable failure detail.
    pub message: String,
}

/// Embedder contract for capability checks and native sharing.
pub trait ShareProvider: Send + Sync {
    /// Reports whether the platform can share the normalized payload.
    fn can_share(&self, data: &ShareData) -> bool;
    /// Starts sharing owned data with the platform.
    fn share(&self, data: ShareData) -> Result<(), ShareError>;
    /// Cancels an in-flight request owned by a retired document.
    fn cancel(&self, _document_id: usize) {}
}

/// Deterministic share provider for headless embedders and tests.
#[derive(Debug)]
pub struct DeterministicShareProvider {
    supported: Mutex<bool>,
    requests: Mutex<Vec<ShareData>>,
    next_result: Mutex<Option<Result<(), ShareError>>>,
    cancelled_documents: Mutex<Vec<usize>>,
}

impl Default for DeterministicShareProvider {
    fn default() -> Self {
        Self {
            supported: Mutex::new(true),
            requests: Mutex::new(Vec::new()),
            next_result: Mutex::new(None),
            cancelled_documents: Mutex::new(Vec::new()),
        }
    }
}

impl DeterministicShareProvider {
    /// Enables or disables platform support for otherwise valid data.
    pub fn set_supported(&self, supported: bool) {
        *self.supported.lock().unwrap() = supported;
    }

    /// Sets the result consumed by the next share request.
    pub fn respond_to_next(&self, result: Result<(), ShareError>) {
        *self.next_result.lock().unwrap() = Some(result);
    }

    /// Returns owned requests observed so far.
    pub fn requests(&self) -> Vec<ShareData> {
        self.requests.lock().unwrap().clone()
    }

    /// Returns document identifiers cancelled by lifecycle retirement.
    pub fn cancelled_documents(&self) -> Vec<usize> {
        self.cancelled_documents.lock().unwrap().clone()
    }
}

impl ShareProvider for DeterministicShareProvider {
    fn can_share(&self, _data: &ShareData) -> bool {
        *self.supported.lock().unwrap()
    }

    fn share(&self, data: ShareData) -> Result<(), ShareError> {
        self.requests.lock().unwrap().push(data);
        self.next_result.lock().unwrap().take().unwrap_or_else(|| {
            Err(ShareError {
                name: "AbortError".into(),
                message: "The user cancelled the share operation".into(),
            })
        })
    }

    fn cancel(&self, document_id: usize) {
        self.cancelled_documents.lock().unwrap().push(document_id);
    }
}

#[derive(Clone, Debug)]
struct PendingShare {
    id: u64,
    result: Result<(), ShareError>,
}

pub(super) struct State {
    provider: Arc<dyn ShareProvider>,
    pending: HashMap<usize, PendingShare>,
    next_id: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicShareProvider::default()),
            pending: HashMap::new(),
            next_id: 1,
        }
    }
}

impl State {
    pub(super) fn retire_document(&mut self, document_id: usize) {
        if self.pending.remove(&document_id).is_some() {
            self.provider.cancel(document_id);
        }
    }
}

fn error_json(error: ShareError) -> serde_json::Value {
    let name = match error.name.as_str() {
        "AbortError" | "DataError" | "NotAllowedError" | "OperationError" => error.name,
        _ => "OperationError".to_owned(),
    };
    serde_json::json!({"error": name, "message": error.message})
}

fn unavailable(name: &str, message: &str) -> serde_json::Value {
    serde_json::json!({"error": name, "message": message})
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let document_id = parse_node_id(args.get(1), context)?;
    ensure_same_origin_document(context, document_id)?;
    let json = string_argument(args.get(2), "{}", context)?;
    let result = with_host_state(|host| {
        let mut host = host.borrow_mut();
        if !host.document_is_active(document_id) {
            return Ok(unavailable(
                "InvalidStateError",
                "The document is not fully active",
            ));
        }
        if !host
            .feature_enabled_in_document(document_id, permissions_policy::PolicyFeature::WebShare)
        {
            return Ok(unavailable(
                "NotAllowedError",
                "Web Share is disabled by Permissions Policy",
            ));
        }
        let Some(url) = host.base_url_for_document(document_id) else {
            return Ok(unavailable(
                "NotAllowedError",
                "Web Share requires a tuple origin",
            ));
        };
        match operation.as_str() {
            "preflight" => {
                if host.web_share.pending.contains_key(&document_id) {
                    Ok(unavailable(
                        "InvalidStateError",
                        "A share operation is already in progress",
                    ))
                } else {
                    Ok(serde_json::json!({"ok": true}))
                }
            }
            "canShare" => {
                let mut data: ShareData = serde_json::from_str(&json)
                    .map_err(|_| JsNativeError::typ().with_message("Invalid share data"))?;
                data.origin = format!("{}://{}", url.scheme(), url.authority());
                Ok(serde_json::json!({"supported": host.web_share.provider.can_share(&data)}))
            }
            "begin" => {
                if host.web_share.pending.contains_key(&document_id) {
                    return Ok(unavailable(
                        "InvalidStateError",
                        "A share operation is already in progress",
                    ));
                }
                let mut data: ShareData = serde_json::from_str(&json)
                    .map_err(|_| JsNativeError::typ().with_message("Invalid share data"))?;
                data.origin = format!("{}://{}", url.scheme(), url.authority());
                if !host.web_share.provider.can_share(&data) {
                    return Ok(unavailable(
                        "DataError",
                        "The platform cannot share this data",
                    ));
                }
                let result = host.web_share.provider.share(data);
                let id = host.web_share.next_id;
                host.web_share.next_id = host.web_share.next_id.saturating_add(1);
                host.web_share
                    .pending
                    .insert(document_id, PendingShare { id, result });
                Ok(serde_json::json!({"id": id}))
            }
            "complete" => {
                let id = serde_json::from_str::<serde_json::Value>(&json)
                    .ok()
                    .and_then(|value| value.get("id")?.as_u64())
                    .unwrap_or_default();
                let Some(pending) = host.web_share.pending.remove(&document_id) else {
                    return Ok(unavailable(
                        "AbortError",
                        "The share operation was cancelled",
                    ));
                };
                if pending.id != id {
                    host.web_share.pending.insert(document_id, pending);
                    return Ok(unavailable(
                        "AbortError",
                        "The share operation is no longer current",
                    ));
                }
                match pending.result {
                    Ok(()) => Ok(serde_json::json!({"ok": true})),
                    Err(error) => Ok(error_json(error)),
                }
            }
            _ => Err(JsNativeError::typ()
                .with_message("Unknown Web Share operation")
                .into()),
        }
    })?;
    Ok(JsValue::from(JsString::from(result.to_string().as_str())))
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_web_share"),
        3,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the provider used by subsequent Web Share requests.
    pub fn set_share_provider(&mut self, provider: Arc<dyn ShareProvider>) {
        let mut host = self.host_state.borrow_mut();
        let documents = host.web_share.pending.keys().copied().collect::<Vec<_>>();
        for document in documents {
            host.web_share.retire_document(document);
        }
        host.web_share.provider = provider;
    }
}
