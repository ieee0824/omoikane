//! Host-owned platform boundary for the Screen Wake Lock API.

use super::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Failure reported while acquiring a platform screen wake lock.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WakeLockError {
    /// Human-readable platform failure message.
    pub message: String,
}

/// Embedder contract for holding the platform screen awake.
pub trait WakeLockProvider: Send + Sync {
    /// Acquires the platform lock for the first live sentinel in a document.
    fn acquire(&self) -> Result<(), WakeLockError>;
    /// Releases a previously acquired platform lock.
    fn release(&self);
}

/// Deterministic provider for headless embedders and tests.
#[derive(Debug, Default)]
pub struct DeterministicWakeLockProvider {
    held: Mutex<usize>,
    next_error: Mutex<Option<WakeLockError>>,
}

impl DeterministicWakeLockProvider {
    /// Makes the next acquisition fail.
    pub fn fail_next(&self, message: impl Into<String>) {
        *self.next_error.lock().unwrap() = Some(WakeLockError {
            message: message.into(),
        });
    }

    /// Returns the number of document-level platform locks currently held.
    pub fn held_count(&self) -> usize {
        *self.held.lock().unwrap()
    }
}

impl WakeLockProvider for DeterministicWakeLockProvider {
    fn acquire(&self) -> Result<(), WakeLockError> {
        if let Some(error) = self.next_error.lock().unwrap().take() {
            return Err(error);
        }
        *self.held.lock().unwrap() += 1;
        Ok(())
    }

    fn release(&self) {
        let mut held = self.held.lock().unwrap();
        *held = held.saturating_sub(1);
    }
}

pub(super) struct State {
    provider: Arc<dyn WakeLockProvider>,
    document_sentinels: HashMap<usize, usize>,
    permission_granted: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicWakeLockProvider::default()),
            document_sentinels: HashMap::new(),
            permission_granted: true,
        }
    }
}

impl State {
    pub(super) fn set_permission(&mut self, granted: bool) {
        self.permission_granted = granted;
    }

    pub(super) fn release_document(&mut self, document_id: usize) {
        if self.document_sentinels.remove(&document_id).is_some() {
            self.provider.release();
        }
    }
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let document_id = parse_node_id(args.get(1), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|host| {
        let mut host = host.borrow_mut();
        match operation.as_str() {
            "permission" => Ok(JsValue::from(if host.wake_lock.permission_granted {
                js_string!("granted")
            } else {
                js_string!("denied")
            })),
            "request" => {
                if !host.document_is_active(document_id)
                    || host.page_hidden
                    || !host.wake_lock.permission_granted
                    || !host.feature_enabled_in_document(
                        document_id,
                        permissions_policy::PolicyFeature::ScreenWakeLock,
                    )
                {
                    return Ok(JsValue::from(js_string!("NotAllowedError")));
                }
                let current = host
                    .wake_lock
                    .document_sentinels
                    .get(&document_id)
                    .copied()
                    .unwrap_or_default();
                if current == 0 {
                    if let Err(error) = host.wake_lock.provider.acquire() {
                        return Ok(JsValue::from(JsString::from(error.message.as_str())));
                    }
                }
                host.wake_lock
                    .document_sentinels
                    .insert(document_id, current + 1);
                Ok(JsValue::from(js_string!("ok")))
            }
            "release" => {
                let current = host
                    .wake_lock
                    .document_sentinels
                    .get(&document_id)
                    .copied()
                    .unwrap_or_default();
                if current <= 1 {
                    host.wake_lock.release_document(document_id);
                } else {
                    host.wake_lock
                        .document_sentinels
                        .insert(document_id, current - 1);
                }
                Ok(JsValue::undefined())
            }
            _ => Err(JsNativeError::typ()
                .with_message("Unknown Screen Wake Lock operation")
                .into()),
        }
    })
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_wake_lock"),
        2,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the provider used by subsequent screen wake-lock requests.
    pub fn set_wake_lock_provider(&mut self, provider: Arc<dyn WakeLockProvider>) {
        let mut state = self.host_state.borrow_mut();
        let documents = state
            .wake_lock
            .document_sentinels
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for document in documents {
            state.wake_lock.release_document(document);
        }
        state.wake_lock.provider = provider;
    }

    /// Revokes all live screen wake locks and dispatches their release events.
    pub fn revoke_wake_locks(&mut self) {
        let documents = self.media_document_ids();
        for document_id in documents {
            self.host_state
                .borrow_mut()
                .wake_lock
                .release_document(document_id);
            if self.host_state.borrow().document_is_active(document_id) {
                let result =
                    self.eval_in_document_realm(document_id, "__omoikane_release_wake_locks()");
                self.record_error_from("wake lock revocation", result);
            }
        }
    }
}
