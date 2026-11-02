//! Host-owned state and platform boundary for the Screen Orientation API.

use super::*;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// A concrete screen orientation exposed to script.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenOrientationType {
    PortraitPrimary,
    PortraitSecondary,
    LandscapePrimary,
    LandscapeSecondary,
}

/// One coherent orientation sample supplied by a presentation host.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenOrientationSnapshot {
    /// Concrete orientation type.
    pub orientation_type: ScreenOrientationType,
    /// Clockwise rotation from the natural orientation, in degrees.
    pub angle: u16,
}

impl Default for ScreenOrientationSnapshot {
    fn default() -> Self {
        Self {
            orientation_type: ScreenOrientationType::LandscapePrimary,
            angle: 0,
        }
    }
}

/// Failure reported by a platform orientation-lock adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScreenOrientationError {
    /// The host cannot satisfy the requested orientation.
    NotSupported(String),
    /// The host failed while applying the request.
    Operation(String),
}

/// Embedder contract for reading and changing display orientation.
pub trait ScreenOrientationProvider: Send + Sync {
    /// Returns the host's latest coherent display sample.
    fn snapshot(&self) -> ScreenOrientationSnapshot;
    /// Applies a validated WebIDL orientation lock value.
    fn lock(&self, orientation: &str) -> Result<ScreenOrientationSnapshot, ScreenOrientationError>;
    /// Removes the active orientation lock and returns the resulting sample.
    fn unlock(&self) -> Result<ScreenOrientationSnapshot, ScreenOrientationError>;
}

/// Mutable provider used by headless embedders and deterministic tests.
#[derive(Debug)]
pub struct DeterministicScreenOrientationProvider {
    natural: ScreenOrientationSnapshot,
    current: Mutex<ScreenOrientationSnapshot>,
    next_error: Mutex<Option<ScreenOrientationError>>,
}

impl DeterministicScreenOrientationProvider {
    /// Creates a provider with an explicit natural orientation.
    pub fn new(natural: ScreenOrientationSnapshot) -> Self {
        Self {
            natural,
            current: Mutex::new(natural),
            next_error: Mutex::new(None),
        }
    }
    /// Publishes a physical display-orientation change.
    pub fn publish(&self, snapshot: ScreenOrientationSnapshot) {
        *self.current.lock().unwrap() = snapshot;
    }
    /// Makes the next lock or unlock operation fail deterministically.
    pub fn fail_next(&self, error: ScreenOrientationError) {
        *self.next_error.lock().unwrap() = Some(error);
    }
    fn requested_snapshot(&self, request: &str) -> ScreenOrientationSnapshot {
        let current = *self.current.lock().unwrap();
        let orientation_type = match request {
            "natural" => self.natural.orientation_type,
            "portrait"
                if matches!(
                    current.orientation_type,
                    ScreenOrientationType::PortraitPrimary
                        | ScreenOrientationType::PortraitSecondary
                ) =>
            {
                current.orientation_type
            }
            "portrait" => ScreenOrientationType::PortraitPrimary,
            "landscape"
                if matches!(
                    current.orientation_type,
                    ScreenOrientationType::LandscapePrimary
                        | ScreenOrientationType::LandscapeSecondary
                ) =>
            {
                current.orientation_type
            }
            "landscape" | "any" => ScreenOrientationType::LandscapePrimary,
            "portrait-primary" => ScreenOrientationType::PortraitPrimary,
            "portrait-secondary" => ScreenOrientationType::PortraitSecondary,
            "landscape-primary" => ScreenOrientationType::LandscapePrimary,
            "landscape-secondary" => ScreenOrientationType::LandscapeSecondary,
            _ => current.orientation_type,
        };
        let natural_portrait = matches!(
            self.natural.orientation_type,
            ScreenOrientationType::PortraitPrimary | ScreenOrientationType::PortraitSecondary
        );
        let angle = match (natural_portrait, orientation_type) {
            (true, ScreenOrientationType::PortraitPrimary)
            | (false, ScreenOrientationType::LandscapePrimary) => 0,
            (true, ScreenOrientationType::LandscapePrimary)
            | (false, ScreenOrientationType::PortraitPrimary) => 90,
            (true, ScreenOrientationType::PortraitSecondary)
            | (false, ScreenOrientationType::LandscapeSecondary) => 180,
            (true, ScreenOrientationType::LandscapeSecondary)
            | (false, ScreenOrientationType::PortraitSecondary) => 270,
        };
        ScreenOrientationSnapshot {
            orientation_type,
            angle,
        }
    }
}

impl Default for DeterministicScreenOrientationProvider {
    fn default() -> Self {
        Self::new(ScreenOrientationSnapshot::default())
    }
}

impl ScreenOrientationProvider for DeterministicScreenOrientationProvider {
    fn snapshot(&self) -> ScreenOrientationSnapshot {
        *self.current.lock().unwrap()
    }
    fn lock(&self, orientation: &str) -> Result<ScreenOrientationSnapshot, ScreenOrientationError> {
        if let Some(error) = self.next_error.lock().unwrap().take() {
            return Err(error);
        }
        let snapshot = self.requested_snapshot(orientation);
        *self.current.lock().unwrap() = snapshot;
        Ok(snapshot)
    }
    fn unlock(&self) -> Result<ScreenOrientationSnapshot, ScreenOrientationError> {
        if let Some(error) = self.next_error.lock().unwrap().take() {
            return Err(error);
        }
        *self.current.lock().unwrap() = self.natural;
        Ok(self.natural)
    }
}

pub(super) struct State {
    provider: Arc<dyn ScreenOrientationProvider>,
    next_request: u32,
    pending_request: Option<(u32, usize)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicScreenOrientationProvider::default()),
            next_request: 1,
            pending_request: None,
        }
    }
}

fn result_value(result: Result<ScreenOrientationSnapshot, ScreenOrientationError>) -> JsValue {
    let value = match result {
        Ok(snapshot) => serde_json::json!({"snapshot":snapshot}),
        Err(ScreenOrientationError::NotSupported(message)) => {
            serde_json::json!({"error":"NotSupportedError", "message":message})
        }
        Err(ScreenOrientationError::Operation(message)) => {
            serde_json::json!({"error":"OperationError", "message":message})
        }
    };
    JsValue::from(JsString::from(value.to_string().as_str()))
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let document_id = parse_node_id(args.get(1), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|host| {
        let mut host = host.borrow_mut();
        if !host.document_is_active(document_id) {
            let error = if operation == "commit" {
                "AbortError"
            } else {
                "InvalidStateError"
            };
            return Ok(JsValue::from(JsString::from(
                format!(r#"{{"error":"{error}"}}"#).as_str(),
            )));
        }
        if operation == "snapshot" {
            return Ok(result_value(Ok(host
                .screen_orientation
                .provider
                .snapshot())));
        }
        if host.page_hidden {
            return Ok(JsValue::from(js_string!(r#"{"error":"SecurityError"}"#)));
        }
        let sandbox = host
            .document_sandbox
            .get(&document_id)
            .copied()
            .unwrap_or_default();
        if sandbox.active && !sandbox.allow_orientation_lock {
            return Ok(JsValue::from(js_string!(r#"{"error":"SecurityError"}"#)));
        }
        match operation.as_str() {
            "prepare" => {
                let token = host.screen_orientation.next_request;
                host.screen_orientation.next_request = token.wrapping_add(1).max(1);
                host.screen_orientation.pending_request = Some((token, document_id));
                Ok(JsValue::from(token as f64))
            }
            "commit" => {
                let token = args
                    .get(2)
                    .map(|value| value.to_u32(context))
                    .transpose()?
                    .unwrap_or_default();
                if host.screen_orientation.pending_request != Some((token, document_id)) {
                    return Ok(JsValue::from(js_string!(r#"{"error":"AbortError"}"#)));
                }
                host.screen_orientation.pending_request = None;
                if host.fullscreen_element(document_id).is_none() {
                    return Ok(JsValue::from(js_string!(r#"{"error":"SecurityError"}"#)));
                }
                let request = string_argument(args.get(3), "", context)?;
                Ok(result_value(
                    host.screen_orientation.provider.lock(&request),
                ))
            }
            "unlock" => {
                host.screen_orientation.pending_request = None;
                Ok(result_value(host.screen_orientation.provider.unlock()))
            }
            _ => Err(JsNativeError::typ()
                .with_message("Unknown Screen Orientation operation")
                .into()),
        }
    })
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_screen_orientation"),
        4,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the provider used by subsequent orientation reads and locks.
    pub fn set_screen_orientation_provider(
        &mut self,
        provider: Arc<dyn ScreenOrientationProvider>,
    ) {
        self.host_state.borrow_mut().screen_orientation.provider = provider;
    }
    /// Polls the provider and reports a physical orientation change to live realms.
    pub fn refresh_screen_orientation(&mut self) -> JsResult<()> {
        let snapshot = self
            .host_state
            .borrow()
            .screen_orientation
            .provider
            .snapshot();
        let json = serde_json::to_string(&snapshot).map_err(|_| {
            JsNativeError::typ().with_message("Invalid Screen Orientation snapshot")
        })?;
        for document_id in self.media_document_ids() {
            if self.host_state.borrow().document_is_active(document_id) {
                self.eval_in_document_realm(
                    document_id,
                    &format!("__omoikane_update_screen_orientation({json})"),
                )?;
            }
        }
        Ok(())
    }
}
