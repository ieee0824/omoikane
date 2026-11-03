//! Document-owned callbacks exported only by the trusted bootstrap module.

use super::*;

#[cfg(test)]
mod observation;

#[derive(Default)]
pub(super) struct DocumentCallbacks {
    stylesheet_flush: HashMap<usize, JsValue>,
    #[cfg(test)]
    stylesheet_flush_entries: HashMap<usize, u64>,
}

impl DocumentCallbacks {
    pub(super) fn remove(&mut self, document_id: usize) {
        self.stylesheet_flush.remove(&document_id);
        #[cfg(test)]
        self.stylesheet_flush_entries.remove(&document_id);
    }

    #[cfg(test)]
    pub(super) fn stylesheet_flush_entries(&self, document_id: usize) -> u64 {
        self.stylesheet_flush_entries
            .get(&document_id)
            .copied()
            .unwrap_or(0)
    }

    pub(super) unsafe fn trace(&self, tracer: &mut Tracer) {
        for callback in self.stylesheet_flush.values() {
            unsafe { callback.trace(tracer) };
        }
    }
}

pub(super) fn capture(
    host_state: &Rc<RefCell<HostState>>,
    namespace: &JsObject,
    document_id: usize,
    context: &mut Context,
) -> JsResult<()> {
    let callback = namespace.get(js_string!("__omoikane_flush_stylesheets"), context)?;
    if callback.as_callable().is_none() {
        return Err(JsNativeError::error()
            .with_message("stylesheet flush callback is unavailable")
            .into());
    }
    host_state
        .borrow_mut()
        .host_callbacks
        .stylesheet_flush
        .insert(document_id, callback);
    Ok(())
}

impl JsRuntime {
    /// Commits batched CSSOM changes through the active document's private callback.
    #[cfg_attr(test, track_caller)]
    pub(crate) fn flush_stylesheets(&mut self) -> JsResult<JsValue> {
        let document_id = self.document().identity();
        let result = self.flush_stylesheets_for_document(document_id);
        let result = self.finish_sync_js_boundary(result);
        #[cfg(test)]
        if result.is_ok() {
            observation::record(document_id, std::panic::Location::caller());
        }
        result
    }

    /// Discards modal metadata after a synchronous callback cancels suspension.
    fn finish_sync_js_boundary(&mut self, result: JsResult<JsValue>) -> JsResult<JsValue> {
        self.host_state.borrow_mut().pending_javascript_dialog = None;
        result
    }

    // The document identity is supplied exclusively by host-owned state.
    pub(super) fn flush_stylesheets_for_document(
        &mut self,
        document_id: usize,
    ) -> JsResult<JsValue> {
        let callback = {
            let state = self.host_state.borrow();
            if !state.document_is_active(document_id) {
                return Err(JsNativeError::error()
                    .with_message("stylesheet Document is not active")
                    .into());
            }
            state
                .host_callbacks
                .stylesheet_flush
                .get(&document_id)
                .cloned()
        }
        .and_then(|value| value.as_callable())
        .ok_or_else(|| JsNativeError::error().with_message("stylesheet callback is unavailable"))?;
        let realm = async_realm::document_realm(self, document_id)?;
        self.with_active_host(|context| {
            let mut context = async_realm::ContextScope::new(context, Some(realm));
            callback.call(&JsValue::undefined(), &[], &mut context)
        })
    }
}

// In fully initialized Window realms, flushStyleSheets has exactly one call to
// this private native entry before its CSS work. This test-only observation
// does not cover pre-validation bootstrap or invalid node/origin arguments.
#[cfg(test)]
pub(super) fn record_stylesheet_flush(node_id: usize) -> JsResult<()> {
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        let document_id = state
            .get_node(node_id)
            .and_then(|node| {
                document_root_for_node(&node).or_else(|| state.node_lifetime_owner(node_id))
            })
            .map(|document| document.identity());
        if let Some(document_id) =
            document_id.filter(|id| state.host_callbacks.stylesheet_flush.contains_key(id))
        {
            *state
                .host_callbacks
                .stylesheet_flush_entries
                .entry(document_id)
                .or_default() += 1;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::TreeBuilder;

    fn runtime() -> JsRuntime {
        JsRuntime::with_document(
            TreeBuilder::parse("<html><body><iframe id='f'></iframe></body></html>").document(),
        )
        .unwrap()
    }

    fn assert_registered(runtime: &JsRuntime, document_id: usize, expected: bool) {
        let state = runtime.host_state.borrow();
        assert_eq!(
            state
                .host_callbacks
                .stylesheet_flush
                .contains_key(&document_id),
            expected
        );
        if !expected {
            assert!(
                !state
                    .host_callbacks
                    .stylesheet_flush_entries
                    .contains_key(&document_id)
            );
        }
    }

    #[test]
    fn child_callback_uses_its_document_and_is_removed_on_retirement() {
        let mut runtime = runtime();
        runtime.eval("var f = document.getElementById('f'); f.srcdoc = '<style>body { color:red }</style>';").unwrap();
        runtime.run_until_idle().unwrap();
        runtime
            .eval("f.contentWindow.eval('void document.fonts.size')")
            .unwrap();
        let main_id = runtime.document().identity();
        let child = runtime
            .host_state
            .borrow()
            .iframe_documents
            .values()
            .next()
            .unwrap()
            .document
            .clone();
        let child_id = child.identity();
        let (main_before, child_before) = {
            let state = runtime.host_state.borrow();
            assert!(state.host_callbacks.stylesheet_flush.contains_key(&main_id));
            assert!(
                state
                    .host_callbacks
                    .stylesheet_flush
                    .contains_key(&child_id)
            );
            (
                state.host_callbacks.stylesheet_flush_entries(main_id),
                state.host_callbacks.stylesheet_flush_entries(child_id),
            )
        };
        boa_gc::force_collect();
        runtime.flush_stylesheets_for_document(child_id).unwrap();
        {
            let state = runtime.host_state.borrow();
            assert_eq!(
                state.host_callbacks.stylesheet_flush_entries(main_id),
                main_before
            );
            assert_eq!(
                state.host_callbacks.stylesheet_flush_entries(child_id),
                child_before + 1
            );
        }
        runtime
            .eval("f.srcdoc = '<style>body { color:blue }</style>'")
            .unwrap();
        runtime.run_until_idle().unwrap();
        runtime
            .eval("f.contentWindow.eval('void document.fonts.size')")
            .unwrap();
        let replacement = runtime
            .host_state
            .borrow()
            .iframe_documents
            .values()
            .next()
            .unwrap()
            .document
            .clone();
        let replacement_id = replacement.identity();
        assert_ne!(replacement_id, child_id);
        assert_registered(&runtime, child_id, false);
        assert_registered(&runtime, replacement_id, true);
        assert!(runtime.flush_stylesheets_for_document(child_id).is_err());
        runtime
            .flush_stylesheets_for_document(replacement_id)
            .unwrap();
        runtime.eval("f.remove()").unwrap();
        runtime.run_until_idle().unwrap();
        assert_registered(&runtime, replacement_id, false);
        assert_registered(&runtime, main_id, true);
        assert!(
            runtime
                .flush_stylesheets_for_document(replacement_id)
                .is_err()
        );
        // Retain the old DOM identity through retirement; its callback must not
        // survive merely because an author or host still owns the Document.
        assert_eq!(child.identity(), child_id);
    }

    #[test]
    fn synchronous_stylesheet_flush_discards_rejected_dialog_state() {
        let mut runtime = runtime();
        runtime.eval("globalThis.savedArrayFrom = Array.from; globalThis.flushDialogReached = false; Array.from = function() { Array.from = savedArrayFrom; flushDialogReached = true; alert('stylesheet flush boundary'); };").unwrap();
        let (result, calls) =
            JsRuntime::observe_stylesheet_host_flushes(|| runtime.flush_stylesheets());
        // Capture before any follow-up eval: eval itself clears modal
        // metadata and would otherwise hide a leak from the direct host call.
        let pending = runtime.pending_javascript_dialog();
        runtime
            .eval("Array.from = savedArrayFrom; delete globalThis.savedArrayFrom;")
            .unwrap();
        assert_eq!(
            runtime.eval("flushDialogReached").unwrap().as_boolean(),
            Some(true)
        );
        // The shim and hook do not catch alert rejection; it must propagate.
        assert!(result.is_err());
        assert!(
            calls.is_empty(),
            "failed flush observed as successful: {calls:?}"
        );
        assert_eq!(
            pending, None,
            "synchronous flush retained a canceled dialog"
        );
        runtime.flush_stylesheets().unwrap();
        assert_eq!(runtime.eval("6 * 7").unwrap().as_number(), Some(42.0));
    }

    #[test]
    fn runtime_drop_releases_callbacks_before_context_teardown() {
        let mut first = runtime();
        first.flush_stylesheets().unwrap();
        let host = first.host_state.clone();
        assert!(!host.borrow().host_callbacks.stylesheet_flush.is_empty());
        drop(first);
        assert!(host.borrow().host_callbacks.stylesheet_flush.is_empty());
        assert!(
            host.borrow()
                .host_callbacks
                .stylesheet_flush_entries
                .is_empty()
        );
        drop(host);
        boa_gc::force_collect();
        let mut second = runtime();
        second.flush_stylesheets().unwrap();
        drop(second);
        boa_gc::force_collect();
    }
}
