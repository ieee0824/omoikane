//! Idle periods for `requestIdleCallback` (Issue #1293).
//!
//! Script owns each Document's idle callback list. The host decides when an
//! idle period starts: after a task pump leaves every task queue empty, at
//! most once per pump, so a callback that reposts itself waits for the next
//! pump instead of spinning. Each callback runs as its own turn followed by a
//! microtask checkpoint.

use super::*;

/// The longest idle period, from requestIdleCallback §5.
pub(super) const MAX_IDLE_PERIOD_MS: f64 = 50.0;

/// The frame interval that bounds an idle period while an animation frame
/// callback waits.
const FRAME_INTERVAL_MS: f64 = 1000.0 / 60.0;

/// How long an embedder waits before pumping again for waiting idle
/// callbacks: one frame at 60Hz.
pub(super) const IDLE_PERIOD_INTERVAL_MS: u64 = 16;

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_request_idle_period"),
        0,
        NativeFunction::from_fn_ptr(request_idle_period_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_idle_budget"),
        0,
        NativeFunction::from_fn_ptr(idle_budget_native),
    )
}

/// `__omoikane_idle_budget()` returns how long an idle period starting now
/// may last, in milliseconds. The page clock stands still while script
/// runs, so the value also bounds a running period after script schedules a
/// timer or an animation frame.
fn idle_budget_native(
    _this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    with_host_state(|state| {
        let state = state.borrow();
        Ok(JsValue::from(idle_budget_ms(
            state.event_loop.next_timer_delay_ms(),
            state.event_loop.has_pending_animation_frames(),
        )))
    })
}

/// `__omoikane_request_idle_period()` asks the host to start an idle period
/// for the calling Document, which has idle callbacks waiting.
fn request_idle_period_native(
    _this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = context
        .realm()
        .host_defined()
        .get::<ModuleDocumentId>()
        .map(|owner| owner.0)
        .or_else(active_document_id);
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        if let Some(document_id) = document_id.filter(|id| state.document_is_active(*id)) {
            state.idle_period_requests.insert(document_id);
        }
        Ok(JsValue::undefined())
    })
}

/// The idle period budget in milliseconds: the time until the next timer is
/// due or, with a waiting animation frame callback, the next frame, capped
/// at [`MAX_IDLE_PERIOD_MS`].
pub(super) fn idle_budget_ms(next_timer_delay_ms: Option<u64>, frame_pending: bool) -> f64 {
    let timer = next_timer_delay_ms.map_or(f64::INFINITY, |delay| delay as f64);
    let frame = if frame_pending {
        FRAME_INTERVAL_MS
    } else {
        f64::INFINITY
    };
    MAX_IDLE_PERIOD_MS.min(timer).min(frame)
}

impl JsRuntime {
    /// Starts one idle period for every Document with waiting idle callbacks.
    /// Returns whether any callback ran.
    pub(super) fn run_idle_period(&mut self) -> JsResult<bool> {
        let documents = {
            let mut state = self.host_state.borrow_mut();
            if state.idle_period_requests.is_empty() {
                return Ok(false);
            }
            std::mem::take(&mut state.idle_period_requests)
        };
        let mut ran = false;
        let mut first_error = None;
        for document_id in documents {
            let mut method = "start";
            loop {
                let turn = (method == "step").then(|| self.begin_task_turn());
                let hook_result = self.call_idle_period_hook(document_id, method);
                let jobs_result = if method == "step" {
                    self.run_jobs_for_document(Some(document_id))
                } else {
                    Ok(())
                };
                if let Some(turn) = turn {
                    self.end_task_turn(turn);
                }
                if let Err(error) = jobs_result {
                    first_error.get_or_insert(error);
                }
                let more = match hook_result {
                    Ok(more) => more,
                    Err(error) => {
                        first_error.get_or_insert(error);
                        break;
                    }
                };
                if method == "step" {
                    ran = true;
                }
                if !more {
                    break;
                }
                method = "step";
            }
        }
        first_error.map_or(Ok(ran), Err)
    }

    /// Calls `method` on `document_id`'s exported idle period hook in its
    /// Realm. Returns the hook's boolean result: whether a callback is ready.
    fn call_idle_period_hook(&mut self, document_id: usize, method: &str) -> JsResult<bool> {
        let hook = self
            .host_state
            .borrow()
            .idle_period_hooks
            .get(&document_id)
            .cloned();
        let Some(hook) = hook else {
            return Ok(false);
        };
        let is_main_document = document_id == self.document().identity();
        let realm = if is_main_document {
            self.host_state.borrow().main_realm.clone()
        } else {
            self.existing_realm_for_document(document_id)
        };
        let Some(realm) = realm else {
            return Ok(false);
        };
        if !is_main_document && !self.iframe_realm_is_live(document_id, &realm) {
            return Ok(false);
        }
        let old_realm = self.context.enter_realm(realm);
        let _document = activate_module_document(&self.host_state, document_id);
        let result = self.with_active_host(|context| {
            let function = hook
                .as_object()
                .ok_or_else(|| JsNativeError::typ().with_message("idle period hook"))?
                .get(js_string!(method), context)?;
            let function = function
                .as_callable()
                .ok_or_else(|| JsNativeError::typ().with_message("idle period hook"))?;
            Ok(function.call(&hook, &[], context)?.to_boolean())
        });
        self.context.enter_realm(old_realm);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_budget_is_capped_by_the_next_timer_and_frame() {
        assert_eq!(idle_budget_ms(None, false), 50.0);
        assert_eq!(idle_budget_ms(Some(10), false), 10.0);
        assert_eq!(idle_budget_ms(Some(500), false), 50.0);
        assert_eq!(idle_budget_ms(Some(0), false), 0.0);
        assert_eq!(idle_budget_ms(None, true), 1000.0 / 60.0);
        assert_eq!(idle_budget_ms(Some(5), true), 5.0);
    }
}
