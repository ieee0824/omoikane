//! `Input` domain: keyboard, mouse, touch, and IME event dispatch.

use super::*;

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

fn trace_native_target_rejection(enabled: bool, reason: &str, node: Option<usize>) {
    if enabled {
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_NATIVE_SCROLL_TARGET_POLICY",
                &json!({"reason":reason,"target":node}),
            )
        );
    }
}

impl CdpSession {
    /// Returns the numeric-only native-scroll handle owned by this session.
    pub fn native_scroll_handle(&self) -> crate::js::native_scroll::NativeScrollHandle {
        self.runtime.native_scroll_handle()
    }

    /// Resolves a conservative top-level root-scroll target from trusted hit
    /// testing and the bootstrap-private policy callback. A blank viewport
    /// point falls back to the top-level document element so root scrolling can
    /// remain compositor-driven while the page thread is busy.
    ///
    /// Cross-Document targets, pointer lock, nested scroll containers, and
    /// overscroll chaining boundaries are rejected. The returned node and Realm
    /// identities must be retained unchanged with a native input.
    pub fn native_scroll_target_at(
        &mut self,
        x: f64,
        y: f64,
    ) -> Result<Option<crate::platform_input::NativeScrollTarget>, JsonRpcError> {
        if !x.is_finite() || !y.is_finite() {
            return Ok(None);
        }
        let trace = std::env::var_os("OMOIKANE_TRACE_INPUT").is_some();
        let target = self.runtime.hit_test(x as f32, y as f32).or_else(|| {
            self.runtime
                .document()
                .child_nodes()
                .into_iter()
                .find(|node| node.tag_name().is_some())
        });
        let Some(target) = target else {
            trace_native_target_rejection(trace, "hit_test_miss", None);
            return Ok(None);
        };
        let target_id = target.identity();
        let (client_x, client_y, scroll_x, scroll_y, document) =
            self.runtime.input_position_for_node(&target, x, y);
        if document != self.runtime.document().identity() {
            trace_native_target_rejection(trace, "cross_document", Some(target_id));
            return Ok(None);
        }
        let policy = self
            .runtime
            .dispatch_private_input_in_document_realm(
                document,
                "native-scroll-eligible",
                &[json!(target_id), json!(trace)],
            )
            .map_err(js_error)?;
        let eligible = policy.as_boolean().unwrap_or(false);
        if trace && !eligible {
            let reason = policy
                .as_string()
                .map(|value| value.to_std_string_escaped())
                .unwrap_or_else(|| "unspecified".into());
            trace_native_target_rejection(true, &reason, Some(target_id));
        }
        Ok(
            eligible.then_some(crate::platform_input::NativeScrollTarget {
                document,
                node: target_id,
                cursor: (x, y),
                client: (client_x, client_y),
                page: (
                    client_x + f64::from(scroll_x),
                    client_y + f64::from(scroll_y),
                ),
            }),
        )
    }

    /// Claims one exact accepted input and commits its default once through the
    /// canonical owner setter. The caller must then deliver its notification
    /// with [`Self::dispatch_native_scroll_notification`].
    pub fn claim_native_scroll_notification(
        &mut self,
        expected: crate::js::native_scroll::AcceptedScroll,
    ) -> Result<crate::js::native_scroll::OwnerNotification, JsonRpcError> {
        self.runtime
            .claim_native_scroll_notification(expected)
            .map_err(native_scroll_owner_error)
    }

    /// Dispatches a wheel notification to an exact trusted target without
    /// running the scrolling default a second time.
    pub fn dispatch_native_scroll_notification(
        &mut self,
        target: crate::platform_input::NativeScrollTarget,
        delta: (f64, f64),
        modifiers: crate::platform_input::InputModifiers,
        buttons: u8,
    ) -> Result<(), JsonRpcError> {
        let init = json!({
            "clientX": target.client.0, "clientY": target.client.1,
            "pageX": target.page.0, "pageY": target.page.1,
            "screenX": target.cursor.0, "screenY": target.cursor.1,
            "deltaX": delta.0, "deltaY": delta.1, "deltaZ": 0,
            "deltaMode": 0, "button": 0, "buttons": buttons,
            "altKey": modifiers.alt,
            "ctrlKey": modifiers.control,
            "metaKey": modifiers.meta,
            "shiftKey": modifiers.shift,
        });
        let dispatched = self
            .runtime
            .dispatch_private_input_in_document_realm(
                target.document,
                "native-scroll-notification",
                &[json!(target.node), init],
            )
            .map_err(js_error)?
            .as_boolean()
            .unwrap_or(false);
        self.runtime.run_jobs().map_err(js_error)?;
        self.commit_pending_navigation_requests()?;
        if !dispatched {
            return Err(JsonRpcError {
                code: -32000,
                message: "native scroll target was retired before notification".to_string(),
            });
        }
        Ok(())
    }

    pub(super) fn input_dispatch_key_event(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
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
        let trusted_activation = dom_type == "keydown"
            && !matches!(key, "Escape" | "Shift" | "Control" | "Alt" | "Meta");
        let not_canceled = self.eval_input_bool_with_activation(
            &format!("__omoikane_dispatch_keyboard_input({dom_type:?}, {init})"),
            trusted_activation,
        )?;
        Ok(json!({ "defaultPrevented": !not_canceled }))
    }

    pub(super) fn input_dispatch_mouse_event(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
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
        let (client_x, client_y, scroll_x, scroll_y, target_document_id) =
            self.runtime.input_position_for_node(&target_node, x, y);
        let mut init = json!({
            "clientX": client_x, "clientY": client_y,
            "pageX": client_x + scroll_x as f64, "pageY": client_y + scroll_y as f64,
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
            let outcome = self.eval_input_number_in_document(
                target_document_id,
                &format!("__omoikane_dispatch_wheel_input({target_id}, {init})"),
            )?;
            let not_canceled = outcome & 1 != 0;
            return Ok(json!({
                "defaultPrevented": !not_canceled,
                "targetNodeId": target_node_id,
                "hostOverscrollX": outcome & 2 != 0,
                "hostOverscrollY": outcome & 4 != 0,
            }));
        }
        let not_canceled = self.eval_input_bool_with_activation(
            &format!(
                "__omoikane_dispatch_mouse_input({target_id}, {dom_type:?}, {init}, {})",
                dom_type == "mousedown"
            ),
            button == 0 && matches!(dom_type, "mousedown" | "click"),
        )?;
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
                let click_not_canceled = self.eval_input_bool_with_activation(
                    &format!(
                        "__omoikane_dispatch_mouse_input({target_id}, \"click\", {init}, false)"
                    ),
                    button == 0,
                )?;
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

    pub(super) fn input_dispatch_touch_event(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
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

    pub(super) fn input_ime_set_composition(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
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

    pub(super) fn input_insert_text(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let text = require_string(params, "text")?;
        let handled = self.eval_input_bool(&format!(
            "__omoikane_dispatch_composition_input(\"commit\", {})",
            serde_json::to_string(&text).expect("a string is JSON serializable"),
        ))?;
        Ok(json!({ "handled": handled }))
    }

    fn eval_input_bool(&mut self, script: &str) -> Result<bool, JsonRpcError> {
        self.eval_input_bool_with_activation(script, false)
    }

    fn eval_input_bool_with_activation(
        &mut self,
        script: &str,
        trusted_activation: bool,
    ) -> Result<bool, JsonRpcError> {
        let evaluate = |runtime: &mut JsRuntime| {
            let value = runtime.eval(script)?;
            runtime.run_jobs()?;
            Ok(value.as_boolean().unwrap_or(true))
        };
        let not_canceled = if trusted_activation {
            self.runtime.with_trusted_storage_access_input(evaluate)
        } else {
            evaluate(&mut self.runtime)
        }
        .map_err(js_error)?;
        self.commit_pending_navigation_requests()?;
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
        self.commit_pending_navigation_requests()?;
        Ok(outcome)
    }

    fn eval_input_number_in_document(
        &mut self,
        document_id: usize,
        script: &str,
    ) -> Result<u8, JsonRpcError> {
        let outcome = self
            .runtime
            .eval_input_in_document_realm(document_id, script)
            .and_then(|value| {
                self.runtime.run_jobs()?;
                Ok(value.as_number().unwrap_or(1.0) as u8)
            })
            .map_err(js_error)?;
        self.commit_pending_navigation_requests()?;
        Ok(outcome)
    }
}

fn native_scroll_owner_error(error: crate::js::native_scroll::OwnerRejection) -> JsonRpcError {
    JsonRpcError {
        code: -32000,
        message: format!("native scroll owner rejected input: {error:?}"),
    }
}
