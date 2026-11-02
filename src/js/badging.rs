//! Host-owned platform boundary for the Badging API.

use super::*;
use std::sync::{Arc, Mutex};

/// Badge value passed to an installed-application backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppBadge {
    /// A platform-defined indicator without numeric contents.
    Flag,
    /// A positive numeric badge.
    Number(u64),
    /// No badge. This represents both `0` and `clearAppBadge()`.
    Clear,
}

/// Calling global used to select matching installed applications.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BadgeContext {
    /// A fully-active document within an application scope.
    Document,
    /// A worker whose registration or creator origin selects applications.
    Worker,
}

/// Owned request crossing the embedder boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BadgeRequest {
    /// Serialized tuple origin of the caller.
    pub origin: String,
    /// Calling global kind.
    pub context: BadgeContext,
    /// Requested badge state.
    pub badge: AppBadge,
}

/// Observable disposition of a platform update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BadgeDisposition {
    /// At least one matching installed application was updated.
    Applied,
    /// The origin had no matching installed application; the operation is a no-op.
    NoMatchingApplication,
    /// The host platform has no badge backend; the operation is a no-op.
    Unsupported,
}

/// Failure reported by a badge provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BadgeError {
    /// DOMException name exposed to script.
    pub name: String,
    /// Human-readable failure detail.
    pub message: String,
}

/// Embedder contract for installed-application badge updates.
pub trait BadgeProvider: Send + Sync {
    /// Updates matching installed applications using fully owned input.
    fn update_badge(&self, request: BadgeRequest) -> Result<BadgeDisposition, BadgeError>;
}

/// Deterministic provider for headless embedders and tests.
#[derive(Debug)]
pub struct DeterministicBadgeProvider {
    disposition: Mutex<BadgeDisposition>,
    requests: Mutex<Vec<BadgeRequest>>,
    next_error: Mutex<Option<BadgeError>>,
}

impl Default for DeterministicBadgeProvider {
    fn default() -> Self {
        Self {
            disposition: Mutex::new(BadgeDisposition::NoMatchingApplication),
            requests: Mutex::new(Vec::new()),
            next_error: Mutex::new(None),
        }
    }
}

impl DeterministicBadgeProvider {
    /// Sets the result returned by subsequent successful updates.
    pub fn set_disposition(&self, disposition: BadgeDisposition) {
        *self.disposition.lock().unwrap() = disposition;
    }

    /// Makes the next update fail with an explicit platform error.
    pub fn fail_next(&self, error: BadgeError) {
        *self.next_error.lock().unwrap() = Some(error);
    }

    /// Returns all owned requests observed so far.
    pub fn requests(&self) -> Vec<BadgeRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl BadgeProvider for DeterministicBadgeProvider {
    fn update_badge(&self, request: BadgeRequest) -> Result<BadgeDisposition, BadgeError> {
        self.requests.lock().unwrap().push(request);
        if let Some(error) = self.next_error.lock().unwrap().take() {
            return Err(error);
        }
        Ok(*self.disposition.lock().unwrap())
    }
}

pub(super) struct State {
    pub(super) provider: Arc<dyn BadgeProvider>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicBadgeProvider::default()),
        }
    }
}

fn top_document_id(state: &HostState, mut document_id: usize) -> Option<usize> {
    for _ in 0..=state.iframe_documents.len() {
        let Some(frame_id) = state.iframe_documents.iter().find_map(|(frame_id, entry)| {
            (entry.document.identity() == document_id).then_some(*frame_id)
        }) else {
            return Some(document_id);
        };
        document_id = owner_document_for_node(&state.get_node(frame_id)?)?.identity();
    }
    None
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    let context_kind = string_argument(args.get(1), "document", context)?;
    let badge_json = string_argument(args.get(2), "null", context)?;
    let result = with_host_state(|host| {
        let host = host.borrow();
        if !host.document_is_active(document_id) {
            return Ok(
                serde_json::json!({"error":"InvalidStateError","message":"The document is not fully active"}),
            );
        }
        let Some(DocumentSecurityOrigin::Tuple(origin)) =
            host.document_security_origins.get(&document_id)
        else {
            return Ok(
                serde_json::json!({"error":"SecurityError","message":"Badging requires a tuple origin"}),
            );
        };
        if context_kind == "document" {
            let Some(top_id) = top_document_id(&host, document_id) else {
                return Ok(
                    serde_json::json!({"error":"InvalidStateError","message":"The document has no active top-level context"}),
                );
            };
            if host.document_security_origins.get(&top_id)
                != host.document_security_origins.get(&document_id)
            {
                return Ok(
                    serde_json::json!({"error":"SecurityError","message":"Badging is restricted to the top-level origin"}),
                );
            }
        }
        let badge = match badge_json.as_str() {
            "\"flag\"" => AppBadge::Flag,
            "\"clear\"" => AppBadge::Clear,
            _ => serde_json::from_str::<u64>(&badge_json)
                .map(AppBadge::Number)
                .map_err(|_| JsNativeError::typ().with_message("Invalid badge value"))?,
        };
        let request = BadgeRequest {
            origin: origin.serialize(),
            context: if context_kind == "worker" {
                BadgeContext::Worker
            } else {
                BadgeContext::Document
            },
            badge,
        };
        match host.badging.provider.update_badge(request) {
            Ok(disposition) => Ok(serde_json::json!({"disposition": match disposition {
                BadgeDisposition::Applied => "applied",
                BadgeDisposition::NoMatchingApplication => "no-matching-application",
                BadgeDisposition::Unsupported => "unsupported",
            }})),
            Err(error) => {
                let name = match error.name.as_str() {
                    "NotAllowedError" | "OperationError" => error.name,
                    _ => "OperationError".to_owned(),
                };
                Ok(serde_json::json!({"error":name,"message":error.message}))
            }
        }
    })?;
    Ok(JsValue::from(JsString::from(result.to_string().as_str())))
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_badging"),
        3,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the provider used by subsequent badge operations.
    pub fn set_badge_provider(&mut self, provider: Arc<dyn BadgeProvider>) {
        self.host_state.borrow_mut().badging.provider = provider;
    }
}
