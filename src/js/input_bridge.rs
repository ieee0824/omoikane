//! Realm selection and shared browsing-context state for host input.

use super::*;

#[derive(Default)]
pub(super) struct State {
    shared: Option<JsObject>,
    dispatchers: HashMap<usize, JsValue>,
}

impl State {
    pub(super) unsafe fn trace(&self, tracer: &mut Tracer) {
        if let Some(shared) = &self.shared {
            unsafe { shared.trace(tracer) };
        }
        for callback in self.dispatchers.values() {
            unsafe { callback.trace(tracer) };
        }
    }

    pub(super) fn retire_document(&mut self, document: usize) {
        self.dispatchers.remove(&document);
    }
}

pub(super) fn register(context: &mut Context) -> JsResult<()> {
    context.register_global_builtin_callable(
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
    context.register_global_builtin_callable(
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
    context.register_global_builtin_callable(
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
