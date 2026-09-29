//! Geolocation: the deterministic position provider, its permission state,
//! and the `navigator.geolocation` request and watch lifecycle.

use super::*;

/// A deterministic position supplied by an embedder or a regression test.
///
/// The JavaScript API exposes the corresponding read-only fields through a
/// `GeolocationPosition` object.  Optional values use `None` to model the
/// nullable WebIDL members (`altitude`, `altitudeAccuracy`, `heading`, and
/// `speed`).
#[derive(Clone, Debug, PartialEq)]
pub struct GeolocationPositionData {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy: f64,
    pub altitude: Option<f64>,
    pub altitude_accuracy: Option<f64>,
    pub heading: Option<f64>,
    pub speed: Option<f64>,
    /// Unix epoch milliseconds reported as `GeolocationPosition.timestamp`.
    /// A non-finite value is replaced with the runtime's current epoch time.
    pub timestamp_ms: f64,
}

impl GeolocationPositionData {
    /// Creates a position with only the required coordinates and accuracy.
    /// Optional members default to `null`; timestamp is filled by the runtime
    /// when the position is installed.
    pub fn new(latitude: f64, longitude: f64, accuracy: f64) -> Self {
        Self {
            latitude,
            longitude,
            accuracy,
            altitude: None,
            altitude_accuracy: None,
            heading: None,
            speed: None,
            timestamp_ms: f64::NAN,
        }
    }
}

#[derive(Debug)]
pub(super) struct GeolocationRequest {
    pub(super) success: JsValue,
    pub(super) error: Option<JsValue>,
    /// `Some(id)` for `watchPosition`; `None` for one-shot requests.
    watch_id: Option<u32>,
    /// `None` represents the default infinite timeout.
    timeout_ms: Option<u64>,
    /// `None` represents an infinite maximum age.
    maximum_age_ms: Option<u64>,
    pending: bool,
    timeout_timer_id: Option<u64>,
}

#[derive(Debug)]
enum GeolocationOutcome {
    Success(GeolocationPositionData),
    Error { code: u32, message: String },
}

fn geolocation_json_number(value: f64) -> String {
    if value.is_finite() {
        value.to_string()
    } else {
        "null".to_string()
    }
}

fn geolocation_json_optional(value: Option<f64>) -> String {
    value
        .map(geolocation_json_number)
        .unwrap_or_else(|| "null".to_string())
}

fn geolocation_position_json(position: &GeolocationPositionData) -> String {
    format!(
        "{{\"coords\":{{\"latitude\":{},\"longitude\":{},\"accuracy\":{},\"altitude\":{},\"altitudeAccuracy\":{},\"heading\":{},\"speed\":{}}},\"timestamp\":{}}}",
        geolocation_json_number(position.latitude),
        geolocation_json_number(position.longitude),
        geolocation_json_number(position.accuracy),
        geolocation_json_optional(position.altitude),
        geolocation_json_optional(position.altitude_accuracy),
        geolocation_json_optional(position.heading),
        geolocation_json_optional(position.speed),
        geolocation_json_number(position.timestamp_ms),
    )
}

fn geolocation_permission_native(
    _this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    with_host_state(|state| Ok(JsValue::from(state.borrow().geolocation_permission_granted)))
}

fn geolocation_duration_argument(
    value: Option<&JsValue>,
    default: Option<u64>,
    context: &mut Context,
) -> JsResult<Option<u64>> {
    let Some(value) = value else {
        return Ok(default);
    };
    let value = value.to_number(context)?;
    if value.is_nan() || value < 0.0 {
        return Err(JsNativeError::range()
            .with_message("geolocation timeout and maximumAge must be non-negative")
            .into());
    }
    if !value.is_finite() {
        return Ok(None);
    }
    Ok(Some(value.floor().min(u64::MAX as f64) as u64))
}

/// Queues a Geolocation request according to the current permission/provider
/// snapshot.  A request with a fresh cached position is delivered immediately;
/// an unavailable provider waits for its finite timeout, while the default
/// infinite timeout deterministically reports POSITION_UNAVAILABLE.
fn geolocation_position_age_ms(state: &HostState) -> Option<u64> {
    let position = state.geolocation_position.as_ref()?;
    let now = state.performance_time_origin + state.event_loop.now_ms() as f64;
    if !position.timestamp_ms.is_finite() || now <= position.timestamp_ms {
        return Some(0);
    }
    Some((now - position.timestamp_ms).min(u64::MAX as f64) as u64)
}

fn wake_geolocation_request(state: &mut HostState, request_id: u64) {
    let timeout_timer_id = state
        .geolocation_requests
        .get(&request_id)
        .and_then(|request| request.timeout_timer_id);
    if let Some(timer_id) = timeout_timer_id {
        state.event_loop.clear_timer(timer_id);
        if let Some(request) = state.geolocation_requests.get_mut(&request_id) {
            request.timeout_timer_id = None;
            request.pending = false;
        }
    }
}

fn schedule_geolocation_request(state: &mut HostState, request_id: u64) {
    let Some((pending, maximum_age_ms, timeout_ms)) = state
        .geolocation_requests
        .get(&request_id)
        .map(|request| (request.pending, request.maximum_age_ms, request.timeout_ms))
    else {
        return;
    };
    if pending {
        return;
    }

    let fresh_position = geolocation_position_age_ms(state)
        .is_some_and(|age| maximum_age_ms.is_none_or(|maximum_age| age <= maximum_age));
    let immediate = !state.geolocation_permission_granted
        || fresh_position
        || timeout_ms.is_none()
        || timeout_ms == Some(0);
    if let Some(request) = state.geolocation_requests.get_mut(&request_id) {
        request.pending = true;
    } else {
        return;
    }
    if immediate {
        state.event_loop.enqueue_geolocation(request_id);
        return;
    }
    let timeout = timeout_ms.unwrap_or(0);
    let timer_id = state.event_loop.schedule_timer(
        TimerPayload::GeolocationTimeout { request_id },
        timeout,
        false,
        None,
    );
    if let Some(request) = state.geolocation_requests.get_mut(&request_id) {
        request.timeout_timer_id = Some(timer_id);
    } else {
        state.event_loop.clear_timer(timer_id);
    }
}

fn geolocation_request_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let success = args.first().cloned().unwrap_or_default();
    if !success.is_callable() {
        return Err(JsNativeError::typ()
            .with_message("Geolocation success callback must be callable")
            .into());
    }
    let error = args.get(1).cloned().filter(|value| value.is_callable());
    let watch_id = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| JsValue::from(-1))
        .to_number(context)?;
    let watch_id = if watch_id.is_finite() && watch_id >= 0.0 {
        Some(watch_id.min(u32::MAX as f64) as u32)
    } else {
        None
    };
    // JavaScript normalizes the options, but parse again at the native boundary
    // so embedders cannot bypass range validation by calling the private hook.
    let timeout_ms = geolocation_duration_argument(args.get(3), None, context)?;
    let maximum_age_ms = geolocation_duration_argument(args.get(4), Some(0), context)?;

    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let request_id = state.next_geolocation_request_id;
        state.next_geolocation_request_id = state.next_geolocation_request_id.saturating_add(1);
        state.geolocation_requests.insert(
            request_id,
            GeolocationRequest {
                success,
                error,
                watch_id,
                timeout_ms,
                maximum_age_ms,
                pending: false,
                timeout_timer_id: None,
            },
        );
        schedule_geolocation_request(&mut state, request_id);
        Ok(JsValue::from(request_id as f64))
    })
}

fn geolocation_clear_watch_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let watch_id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)?;
    if !watch_id.is_finite() || watch_id < 0.0 {
        return Ok(JsValue::undefined());
    }
    let watch_id = watch_id.min(u32::MAX as f64) as u32;
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let request_ids: Vec<_> = state
            .geolocation_requests
            .iter()
            .filter_map(|(request_id, request)| {
                (request.watch_id == Some(watch_id)).then_some(*request_id)
            })
            .collect();
        for request_id in request_ids {
            if let Some(request) = state.geolocation_requests.remove(&request_id)
                && let Some(timer_id) = request.timeout_timer_id
            {
                state.event_loop.clear_timer(timer_id);
            }
        }
        Ok(JsValue::undefined())
    })
}

/// Registers the private geolocation hooks used by the DOM bootstrap.
pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    for (name, length, function) in [
        (
            js_string!("__omoikane_geolocation_permission"),
            0,
            NativeFunction::from_copy_closure(geolocation_permission_native),
        ),
        (
            js_string!("__omoikane_geolocation_request"),
            5,
            NativeFunction::from_copy_closure(geolocation_request_native),
        ),
        (
            js_string!("__omoikane_geolocation_clear_watch"),
            1,
            NativeFunction::from_copy_closure(geolocation_clear_watch_native),
        ),
    ] {
        register_private_builtin_callable(context, bindings, name, length, function)?;
    }
    Ok(())
}

impl JsRuntime {
    /// Sets or replaces the deterministic geolocation provider position.
    ///
    /// A position is delivered asynchronously to all active watches.  The
    /// position remains cached for `maximumAge` checks on subsequent requests;
    /// calling this method again refreshes that cache age.
    pub fn set_geolocation_position(&mut self, mut position: GeolocationPositionData) {
        let mut state = self.host_state.borrow_mut();
        if !position.latitude.is_finite()
            || !position.longitude.is_finite()
            || !position.accuracy.is_finite()
            || position.accuracy < 0.0
        {
            state.geolocation_position = None;
            return;
        }
        if !position.timestamp_ms.is_finite() || position.timestamp_ms < 0.0 {
            position.timestamp_ms =
                state.performance_time_origin + state.event_loop.now_ms() as f64;
        }
        state.geolocation_position = Some(position);
        let request_ids: Vec<_> = state
            .geolocation_requests
            .iter()
            .filter_map(|(request_id, request)| {
                (!request.pending || request.timeout_timer_id.is_some()).then_some(*request_id)
            })
            .collect();
        for request_id in request_ids {
            wake_geolocation_request(&mut state, request_id);
            schedule_geolocation_request(&mut state, request_id);
        }
    }

    /// Removes the deterministic provider position. Active watches stay alive
    /// and report `POSITION_UNAVAILABLE` (or timeout) on their next request.
    pub fn clear_geolocation_position(&mut self) {
        let mut state = self.host_state.borrow_mut();
        state.geolocation_position = None;
    }

    /// Sets the permission result used by future and active geolocation
    /// requests.  A denied state is delivered through the normal geolocation
    /// task source, never synchronously from the setter.
    pub fn set_geolocation_permission(&mut self, granted: bool) {
        let changed = {
            let mut state = self.host_state.borrow_mut();
            let changed = state.geolocation_permission_granted != granted;
            state.geolocation_permission_granted = granted;
            let request_ids: Vec<_> = state
                .geolocation_requests
                .iter()
                .filter_map(|(request_id, request)| {
                    (!request.pending || request.timeout_timer_id.is_some()).then_some(*request_id)
                })
                .collect();
            for request_id in request_ids {
                wake_geolocation_request(&mut state, request_id);
                schedule_geolocation_request(&mut state, request_id);
            }
            changed
        };
        if changed {
            self.notify_permission_change(
                "geolocation",
                if granted { "granted" } else { "denied" },
            );
        }
    }

    pub(super) fn run_geolocation_delivery(
        &mut self,
        request_id: u64,
        timed_out: bool,
    ) -> JsResult<()> {
        let (success, error, watch_id, maximum_age_ms, timeout_ms, pending) = {
            let state = self.host_state.borrow();
            let Some(request) = state.geolocation_requests.get(&request_id) else {
                return Ok(());
            };
            (
                request.success.clone(),
                request.error.clone(),
                request.watch_id,
                request.maximum_age_ms,
                request.timeout_ms,
                request.pending,
            )
        };
        if !pending {
            return Ok(());
        }
        let (outcome, timeout_timer_id) = {
            let mut state = self.host_state.borrow_mut();
            let fresh_position = geolocation_position_age_ms(&state)
                .is_some_and(|age| maximum_age_ms.is_none_or(|maximum_age| age <= maximum_age));
            let outcome = if !state.geolocation_permission_granted {
                GeolocationOutcome::Error {
                    code: 1,
                    message: "User denied Geolocation.".to_string(),
                }
            } else if fresh_position {
                GeolocationOutcome::Success(
                    state.geolocation_position.clone().expect("fresh position"),
                )
            } else if timed_out || timeout_ms == Some(0) {
                GeolocationOutcome::Error {
                    code: 3,
                    message: "The location request timed out.".to_string(),
                }
            } else {
                GeolocationOutcome::Error {
                    code: 2,
                    message: "Unable to determine the user's location.".to_string(),
                }
            };
            let timeout_timer_id =
                state
                    .geolocation_requests
                    .get_mut(&request_id)
                    .and_then(|request| {
                        request.pending = false;
                        request.timeout_timer_id.take()
                    });
            if watch_id.is_none() {
                state.geolocation_requests.remove(&request_id);
            }
            (outcome, timeout_timer_id)
        };

        if let Some(timer_id) = timeout_timer_id {
            self.host_state
                .borrow_mut()
                .event_loop
                .clear_timer(timer_id);
        }
        let payload = match &outcome {
            GeolocationOutcome::Success(position) => geolocation_position_json(position),
            GeolocationOutcome::Error { .. } => "null".to_string(),
        };
        let (status, code, message) = match outcome {
            GeolocationOutcome::Success(_) => ("success", 0_u32, String::new()),
            GeolocationOutcome::Error { code, message } => ("error", code, message),
        };
        let global = self.context.global_object();
        global.set(
            js_string!("__omoikane_geolocation_callback"),
            success,
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_geolocation_error_callback"),
            error.unwrap_or_else(JsValue::undefined),
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_geolocation_status"),
            JsValue::from(js_string!(status)),
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_geolocation_payload"),
            JsValue::from(js_string!(payload)),
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_geolocation_error_code"),
            JsValue::from(code as f64),
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_geolocation_error_message"),
            JsValue::from(js_string!(message)),
            true,
            &mut self.context,
        )?;
        let result = self.eval("__omoikane_dispatch_geolocation_task()");
        for name in [
            "__omoikane_geolocation_callback",
            "__omoikane_geolocation_error_callback",
            "__omoikane_geolocation_status",
            "__omoikane_geolocation_payload",
            "__omoikane_geolocation_error_code",
            "__omoikane_geolocation_error_message",
        ] {
            let _ = global.set(
                js_string!(name),
                JsValue::undefined(),
                true,
                &mut self.context,
            );
        }
        self.record_error_from("geolocation", result);
        Ok(())
    }
}
