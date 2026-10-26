//! Realm selection and shared browsing-context state for host input.

use super::*;

#[derive(Default)]
pub(super) struct State {
    shared: Option<JsObject>,
    dispatchers: HashMap<usize, JsValue>,
    pub(super) automation: automation::State,
}

impl State {
    pub(super) unsafe fn trace(&self, tracer: &mut Tracer) {
        unsafe { self.automation.trace(tracer) };
        if let Some(shared) = &self.shared {
            unsafe { shared.trace(tracer) };
        }
        for callback in self.dispatchers.values() {
            unsafe { callback.trace(tracer) };
        }
    }

    pub(super) fn retire_document(&mut self, document: usize) {
        self.dispatchers.remove(&document);
        self.automation.retire_document(document);
    }
}

impl JsRuntime {
    /// Supplies a value chosen by the browser's autofill UI to one HTML input.
    ///
    /// The operation runs in the input's owning Realm and does not expose a
    /// script-callable autofill primitive. It returns `false` when `control`
    /// is not a live HTML input in this browsing session.
    pub fn autofill_input(&mut self, control: &NodeHandle, value: &str) -> JsResult<bool> {
        let (node, document) = {
            let state = self.host_state.borrow();
            let Some(node) = state
                .get_node(control.identity())
                .filter(|node| node == control)
            else {
                return Ok(false);
            };
            if !node.is_html_element() || !node.has_tag_name("input") {
                return Ok(false);
            }
            let Some(document) = owner_document_for_node(&node) else {
                return Ok(false);
            };
            (node, document.identity())
        };
        self.dispatch_private_input_in_document_realm(
            document,
            "autofill",
            &[
                serde_json::Value::from(node.identity() as u64),
                serde_json::Value::from(value),
            ],
        )
        .map(|result| result.to_boolean())
    }

    /// Calls the bootstrap-private input dispatcher for one live Document.
    ///
    /// The callback was captured before author script ran and is retained in
    /// host state. Callers must derive `document` and every node identity from
    /// trusted hit testing or a previously published native-scroll target.
    pub(crate) fn dispatch_private_input_in_document_realm(
        &mut self,
        document: usize,
        kind: &str,
        arguments: &[serde_json::Value],
    ) -> JsResult<JsValue> {
        let (active, callback, main_document, main_realm) = {
            let state = self.host_state.borrow();
            (
                state.document_is_active(document),
                state.input_bridge.dispatchers.get(&document).cloned(),
                state.document.identity(),
                state.main_realm.clone(),
            )
        };
        if !active {
            return Err(JsNativeError::error()
                .with_message("input Document Realm is not active")
                .into());
        }
        let callback = callback.ok_or_else(|| {
            JsNativeError::error().with_message("private input dispatcher is unavailable")
        })?;
        let realm = if document == main_document {
            main_realm
        } else {
            self.realm_for_document(document)?
        }
        .ok_or_else(|| {
            JsNativeError::error().with_message("input Document Realm is not available")
        })?;
        let previous = self.context.enter_realm(realm);
        let _document = activate_module_document(&self.host_state, document);
        let result = self.with_active_host(|context| {
            let values = arguments
                .iter()
                .map(|value| JsValue::from_json(value, context))
                .collect::<JsResult<Vec<_>>>()?;
            let values = JsArray::from_iter(values, context);
            callback
                .as_callable()
                .expect("registered input dispatcher")
                .call(
                    &JsValue::undefined(),
                    &[js_string!(kind).into(), values.into()],
                    context,
                )
        });
        self.context.enter_realm(previous);
        result
    }

    /// Claims one exact accepted input and commits its default through the
    /// canonical non-revoking native-default setter before author delivery.
    pub(crate) fn claim_native_scroll_notification(
        &mut self,
        expected: native_scroll::AcceptedScroll,
    ) -> Result<native_scroll::OwnerNotification, native_scroll::OwnerRejection> {
        let mut state = self.host_state.borrow_mut();
        let handle = state.native_scroll.clone();
        let key = state.native_scroll_owner;
        let (current, extent) = state.native_scroll_geometry();
        let notification = handle.claim_notification_for(key, expected, current, extent)?;
        if notification.offset != current {
            state.set_window_scroll_from_native_default(
                notification.offset.x as f32,
                notification.offset.y as f32,
            );
        }
        Ok(notification)
    }
}

/// Browser-internal focus traversal returns identities, never foreign DOM
/// wrappers. Each document's input dispatcher applies its own part of a move.
pub(super) fn focus_chain_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document = parse_node_id(args.first(), context)?;
    let chain = with_host_state(|host| {
        let state = host.borrow();
        let mut current = document;
        let mut chain = Vec::new();
        for _ in 0..=state.iframe_documents.len() {
            if !state.document_is_active(current) {
                return Ok(None);
            }
            if current == state.document.identity()
                || state
                    .auxiliary_contexts
                    .values()
                    .any(|entry| entry.document.identity() == current)
            {
                chain.push((current, None, None));
                return Ok(Some(chain));
            }
            let Some((&frame, _)) = state
                .iframe_documents
                .iter()
                .find(|(_, entry)| entry.document.identity() == current)
            else {
                return Ok(None);
            };
            let Some(parent) = state
                .get_node(frame)
                .and_then(|node| owner_document_for_node(&node))
            else {
                return Ok(None);
            };
            chain.push((current, Some(parent.identity()), Some(frame)));
            current = parent.identity();
        }
        Ok(None)
    })?;
    Ok(chain.map_or(JsValue::null(), |chain| {
        js_string!(serde_json::to_string(&chain).expect("focus identity chain")).into()
    }))
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_input_state"),
        0,
        NativeFunction::from_copy_closure(|_, _, _| {
            with_host_state(|state| {
                let mut state = state.borrow_mut();
                Ok(state
                    .input_bridge
                    .shared
                    .get_or_insert_with(JsObject::with_null_proto)
                    .clone()
                    .into())
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_register_input_dispatcher"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let callback = args
                .first()
                .filter(|v| v.is_callable())
                .cloned()
                .ok_or_else(|| JsNativeError::typ().with_message("input dispatcher required"))?;
            let document = current_iframe_realm(context)
                .map(|(_, doc)| doc)
                .or_else(|| {
                    context
                        .realm()
                        .host_defined()
                        .get::<ModuleDocumentId>()
                        .map(|id| id.0)
                });
            with_host_state(|state| {
                let mut state = state.borrow_mut();
                let document = document.unwrap_or_else(|| state.document.identity());
                state.input_bridge.dispatchers.insert(document, callback);
                Ok(JsValue::undefined())
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_forward_input"),
        3,
        NativeFunction::from_copy_closure(|_, args, context| {
            let document = parse_node_id(args.first(), context)?;
            let caller = context
                .realm()
                .host_defined()
                .get::<ModuleDocumentId>()
                .map(|id| id.0);
            let (active, callback) = with_host_state(|state| {
                let state = state.borrow();
                let caller = caller.unwrap_or_else(|| state.document.identity());
                let active = state.document_is_active(document) && state.document_is_active(caller);
                Ok((
                    active,
                    if caller == document || !active {
                        None
                    } else {
                        state.input_bridge.dispatchers.get(&document).cloned()
                    },
                ))
            })?;
            if !active {
                return Ok(JsValue::from(true));
            }
            let Some(callback) = callback else {
                return Ok(JsValue::undefined());
            };
            // Input listeners may suspend in a modal host call. Preserve the
            // VM's continuation chain instead of nesting a synchronous call.
            context.call_with_native_continuation(
                &callback.as_callable().unwrap(),
                &JsValue::undefined(),
                &args[1..],
                NativeCallContinuation::from_copy_closure_with_captures(|result, (), _| result, ()),
            )
        }),
    )?;
    Ok(())
}

/// Proves that a rendered main-document ancestor cannot consume the requested
/// user-scroll axes. Read owned layout geometry rather than author-overridable
/// Element metrics; unknown geometry keeps the input on the canonical owner.
pub(super) fn native_scroll_axes_have_no_range(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let (Some(check_x), Some(check_y)) = (
        args.get(1).and_then(JsValue::as_boolean),
        args.get(2).and_then(JsValue::as_boolean),
    ) else {
        return Ok(JsValue::from(false));
    };
    if !check_x && !check_y {
        return Ok(JsValue::from(false));
    }
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let Some(node) = state.get_node(node_id) else {
            return Ok(JsValue::from(false));
        };
        if document_root_for_node(&node) != Some(state.document.clone()) {
            return Ok(JsValue::from(false));
        }
        state.ensure_content_visibility_geometry(&node);
        let Some(layout) = state
            .layout_root
            .as_ref()
            .and_then(|root| find_layout_box(root, &node))
        else {
            return Ok(JsValue::from(false));
        };
        let (max_x, max_y) = layout.max_scroll_offset();
        let rect = layout.dimensions.content;
        let finite = [rect.x, rect.y, rect.width, rect.height, max_x, max_y]
            .into_iter()
            .all(f32::is_finite);
        let empty = finite && (!check_x || max_x == 0.0) && (!check_y || max_y == 0.0);
        if std::env::var_os("OMOIKANE_TRACE_INPUT").is_some() {
            eprintln!(
                "{}",
                crate::diagnostics::json_record(
                    "OMOIKANE_NATIVE_SCROLL_ANCESTOR_RANGE",
                    &serde_json::json!({"node":node_id,
                        "tag":node.tag_name().map(|tag|tag.chars().take(48).collect::<String>()),
                        "axes":[check_x,check_y],"maximum":[max_x,max_y],"empty":empty}),
                )
            );
        }
        Ok(JsValue::from(empty))
    })
}
