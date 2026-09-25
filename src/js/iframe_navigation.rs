//! Route child Window navigation to the owning iframe's history controller.

use super::*;

#[derive(Default)]
pub(super) struct State {
    owners: HashMap<usize, JsValue>,
}

impl State {
    pub(super) fn owner(&self, document: usize) -> Option<JsValue> {
        self.owners.get(&document).cloned()
    }

    pub(super) unsafe fn trace(&self, tracer: &mut Tracer) {
        for callback in self.owners.values() {
            unsafe { callback.trace(tracer) };
        }
    }

    pub(super) fn retire_document(&mut self, document: usize) {
        self.owners.remove(&document);
    }
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_register_iframe_navigation"),
        2,
        NativeFunction::from_copy_closure(|_, args, context| {
            let document = parse_node_id(args.first(), context)?;
            ensure_same_origin_document(context, document)?;
            let callback = args
                .get(1)
                .filter(|value| value.is_callable())
                .cloned()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("iframe navigation handler required")
                })?;
            with_host_state(|host| {
                host.borrow_mut()
                    .iframe_navigation
                    .owners
                    .insert(document, callback);
                Ok(JsValue::undefined())
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_child_navigation"),
        4,
        NativeFunction::from_copy_closure(|_, args, context| {
            let document = parse_node_id(args.first(), context)?;
            with_host_state(|host| {
                let (frame, callback, document_url, event_state) = {
                    let mut state = host.borrow_mut();
                    let frame = state
                        .iframe_documents
                        .iter()
                        .find_map(|(frame, entry)| {
                            (entry.document.identity() == document).then_some(*frame)
                        })
                        .ok_or_else(|| {
                            JsNativeError::typ().with_message("Document is no longer active")
                        })?;
                    let node = state
                        .get_node(frame)
                        .filter(|node| state.node_is_in_active_document(node))
                        .ok_or_else(|| {
                            JsNativeError::typ().with_message("iframe is no longer active")
                        })?;
                    // Resolving the facade may commit a src/srcdoc change.
                    // Recheck after that lazy load, before forwarding any
                    // operation from the departing Document to its replacement.
                    let active = state
                        .iframe_content_document(&node)
                        .map_err(|error| JsNativeError::typ().with_message(error))?;
                    if active.identity() != document {
                        return Err(JsNativeError::typ()
                            .with_message("Document is no longer active")
                            .into());
                    }
                    let owner = owner_document_for_node(&node).ok_or_else(|| {
                        JsNativeError::typ().with_message("iframe has no owner document")
                    })?;
                    let callback = state
                        .iframe_navigation
                        .owners
                        .get(&owner.identity())
                        .cloned()
                        .ok_or_else(|| {
                            JsNativeError::typ().with_message("iframe owner is no longer active")
                        })?;
                    let document_url =
                        state.document_urls.get(&document).cloned().ok_or_else(|| {
                            JsNativeError::typ().with_message("Document URL is no longer active")
                        })?;
                    let event_state = state.shared_event_state_for_node(document);
                    (frame, callback, document_url, event_state)
                };
                let mut forwarded =
                    vec![JsValue::from(frame as f64), JsValue::from(document as f64)];
                forwarded.extend_from_slice(&args[1..]);
                forwarded.push(JsValue::from(js_string!(document_url.as_str())));
                forwarded.push(event_state.map_or_else(JsValue::null, JsValue::from));
                callback
                    .as_callable()
                    .expect("registered navigation handler")
                    .call(&JsValue::undefined(), &forwarded, context)
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_iframe_document_url"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let frame = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, frame)?;
            with_host_state(|host| {
                Ok(host
                    .borrow()
                    .iframe_documents
                    .get(&frame)
                    .map_or_else(JsValue::null, |entry| {
                        js_string!(entry.document_url.as_str()).into()
                    }))
            })
        }),
    )?;
    Ok(())
}
