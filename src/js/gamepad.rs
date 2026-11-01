//! Owned host-provider boundary for the Gamepad API.

use super::*;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// One button value in a provider snapshot.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GamepadButtonSnapshot {
    /// Whether the button's pressed threshold is active.
    pub pressed: bool,
    /// Whether a touch-sensitive button is currently touched.
    pub touched: bool,
    /// Normalized analog value in the range from zero through one.
    pub value: f64,
}

/// One coherent gamepad sample owned by the provider boundary.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GamepadSnapshot {
    /// Provider-defined device identifier.
    pub id: String,
    /// Stable slot index for this connection lifetime.
    pub index: u32,
    /// Whether the device is currently connected.
    pub connected: bool,
    /// Monotonic sample timestamp in milliseconds.
    pub timestamp: f64,
    /// Mapping identifier, usually `standard` or the empty string.
    pub mapping: String,
    /// Normalized axis values.
    pub axes: Vec<f64>,
    /// Button values from the same provider generation.
    pub buttons: Vec<GamepadButtonSnapshot>,
    /// Haptic effect names supported by this device.
    pub haptic_effects: Vec<String>,
}

/// Validated haptic request passed to a platform adapter.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GamepadHapticRequest {
    /// Requested haptic effect name.
    pub effect: String,
    /// Effect duration in milliseconds.
    pub duration: f64,
    /// Delay before playback in milliseconds.
    pub start_delay: f64,
    /// Strong motor magnitude from zero through one.
    pub strong_magnitude: f64,
    /// Weak motor magnitude from zero through one.
    pub weak_magnitude: f64,
    /// Left trigger magnitude from zero through one.
    pub left_trigger: f64,
    /// Right trigger magnitude from zero through one.
    pub right_trigger: f64,
}

/// Result returned by a gamepad haptics backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GamepadHapticResult {
    Complete,
    Preempted,
}

impl GamepadHapticResult {
    fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Preempted => "preempted",
        }
    }
}

/// Embedder contract for polling devices and performing haptic effects.
pub trait GamepadProvider: Send + Sync {
    /// Returns one immutable generation of every currently known slot.
    fn snapshot(&self) -> Vec<GamepadSnapshot>;

    /// Reports whether the user has interacted with a connected gamepad.
    fn has_user_gesture(&self) -> bool {
        false
    }

    /// Starts a validated haptic effect for one connected slot.
    fn play_effect(
        &self,
        _index: u32,
        _request: GamepadHapticRequest,
    ) -> Result<GamepadHapticResult, String> {
        Err("Gamepad haptics are not supported".to_string())
    }

    /// Stops haptics for one connected slot.
    fn reset(&self, _index: u32) -> Result<GamepadHapticResult, String> {
        Err("Gamepad haptics are not supported".to_string())
    }
}

/// Mutable deterministic provider for headless embedders and tests.
#[derive(Debug, Default)]
pub struct DeterministicGamepadProvider {
    snapshots: Mutex<Vec<GamepadSnapshot>>,
    haptic_requests: Mutex<Vec<(u32, Option<GamepadHapticRequest>)>>,
    user_gesture: AtomicBool,
}

impl DeterministicGamepadProvider {
    /// Creates a provider with an explicit initial device generation.
    pub fn new(snapshots: Vec<GamepadSnapshot>) -> Self {
        Self {
            snapshots: Mutex::new(snapshots),
            haptic_requests: Mutex::new(Vec::new()),
            user_gesture: AtomicBool::new(false),
        }
    }

    /// Atomically replaces the generation returned by subsequent polls.
    pub fn publish(&self, snapshots: Vec<GamepadSnapshot>) {
        *self.snapshots.lock().unwrap() = snapshots;
    }

    /// Returns recorded play/reset calls in provider order.
    pub fn haptic_requests(&self) -> Vec<(u32, Option<GamepadHapticRequest>)> {
        self.haptic_requests.lock().unwrap().clone()
    }

    /// Controls the privacy gate used by `navigator.getGamepads()`.
    pub fn set_user_gesture(&self, value: bool) {
        self.user_gesture.store(value, Ordering::Release);
    }
}

impl GamepadProvider for DeterministicGamepadProvider {
    fn snapshot(&self) -> Vec<GamepadSnapshot> {
        self.snapshots.lock().unwrap().clone()
    }

    fn has_user_gesture(&self) -> bool {
        self.user_gesture.load(Ordering::Acquire)
    }

    fn play_effect(
        &self,
        index: u32,
        request: GamepadHapticRequest,
    ) -> Result<GamepadHapticResult, String> {
        self.haptic_requests
            .lock()
            .unwrap()
            .push((index, Some(request)));
        Ok(GamepadHapticResult::Complete)
    }

    fn reset(&self, index: u32) -> Result<GamepadHapticResult, String> {
        self.haptic_requests.lock().unwrap().push((index, None));
        Ok(GamepadHapticResult::Complete)
    }
}

pub(super) struct State {
    provider: Arc<dyn GamepadProvider>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicGamepadProvider::default()),
        }
    }
}

fn finite_unit(value: f64, name: &str) -> JsResult<f64> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(JsNativeError::range()
            .with_message(format!("{name} must be between 0 and 1"))
            .into())
    }
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    with_host_state(|host| {
        let host = host.borrow();
        match operation.as_str() {
            "snapshot" => {
                let document_id = parse_node_id(args.get(1), context)?;
                ensure_same_origin_document(context, document_id)?;
                if !host.document_is_active(document_id) {
                    return Ok(JsValue::from(js_string!("[]")));
                }
                if !host.feature_enabled_in_document(
                    document_id,
                    permissions_policy::PolicyFeature::Gamepad,
                ) {
                    return Err(JsNativeError::error()
                        .with_message(
                            "SecurityError: Gamepad access is disallowed by Permissions Policy",
                        )
                        .into());
                }
                if !host.gamepad.provider.has_user_gesture() {
                    return Ok(JsValue::from(js_string!("[]")));
                }
                let snapshots = host.gamepad.provider.snapshot();
                let text = serde_json::to_string(&snapshots).map_err(|_| {
                    JsNativeError::typ().with_message("Invalid Gamepad provider snapshot")
                })?;
                Ok(JsValue::from(JsString::from(text.as_str())))
            }
            "play" => {
                let index = args
                    .get(2)
                    .map(|value| value.to_u32(context))
                    .transpose()?
                    .unwrap_or_default();
                let text = string_argument(args.get(3), "", context)?;
                let mut request: GamepadHapticRequest =
                    serde_json::from_str(&text).map_err(|_| {
                        JsNativeError::typ().with_message("Invalid Gamepad haptic request")
                    })?;
                if !request.duration.is_finite()
                    || request.duration < 0.0
                    || !request.start_delay.is_finite()
                    || request.start_delay < 0.0
                {
                    return Err(JsNativeError::range()
                        .with_message("Haptic timing must be finite and non-negative")
                        .into());
                }
                request.strong_magnitude =
                    finite_unit(request.strong_magnitude, "strongMagnitude")?;
                request.weak_magnitude = finite_unit(request.weak_magnitude, "weakMagnitude")?;
                request.left_trigger = finite_unit(request.left_trigger, "leftTrigger")?;
                request.right_trigger = finite_unit(request.right_trigger, "rightTrigger")?;
                host.gamepad
                    .provider
                    .play_effect(index, request)
                    .map(|result| JsValue::from(JsString::from(result.as_str())))
                    .map_err(|message| JsNativeError::error().with_message(message).into())
            }
            "reset" => {
                let index = args
                    .get(2)
                    .map(|value| value.to_u32(context))
                    .transpose()?
                    .unwrap_or_default();
                host.gamepad
                    .provider
                    .reset(index)
                    .map(|result| JsValue::from(JsString::from(result.as_str())))
                    .map_err(|message| JsNativeError::error().with_message(message).into())
            }
            _ => Err(JsNativeError::typ()
                .with_message("Unknown Gamepad provider operation")
                .into()),
        }
    })
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_gamepad"),
        4,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the provider used by subsequent Gamepad polls and haptic calls.
    pub fn set_gamepad_provider(&mut self, provider: Arc<dyn GamepadProvider>) {
        self.host_state.borrow_mut().gamepad.provider = provider;
    }

    /// Polls the provider and dispatches any resulting connection transitions.
    pub fn poll_gamepads(&mut self) -> JsResult<()> {
        self.eval("__omoikane_poll_gamepads(true)")?;
        Ok(())
    }
}
