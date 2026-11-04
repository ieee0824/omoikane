//! Synchronous browser input primitives shared by trusted host entry points.

use super::*;

/// Owns the client used by the WindowProxy group check during synchronous input.
/// Its state is restored before the caller drains jobs, including on unwind.
struct HostInputClientScope {
    host: Rc<RefCell<HostState>>,
    previous: Option<usize>,
}

impl HostInputClientScope {
    fn new(host: &Rc<RefCell<HostState>>, document: usize) -> Self {
        let previous = host
            .borrow_mut()
            .input_bridge
            .host_client_document
            .replace(document);
        Self {
            host: Rc::clone(host),
            previous,
        }
    }
}

impl Drop for HostInputClientScope {
    fn drop(&mut self) {
        self.host.borrow_mut().input_bridge.host_client_document = self.previous;
    }
}

/// Selects a client for `window_proxy_global_native`'s group check only.
/// With no author frame, the active synchronous input scope supplies its client.
/// An explicit author Realm without an identity fails closed; origin checks
/// remain independent of this selection.
pub(super) fn window_access_client_document(context: &Context) -> JsResult<Option<usize>> {
    if let Some(realm) = author_script_realm(context) {
        return Ok(realm
            .host_defined()
            .get::<ModuleDocumentId>()
            .map(|id| id.0));
    }
    with_host_state(|host| {
        let state = host.borrow();
        Ok(state
            .input_bridge
            .host_client_document
            .filter(|document| state.document_is_active(*document)))
    })
}

/// The eight input operations whose callbacks are captured during bootstrap.
#[derive(Clone, Copy)]
pub(crate) enum HostInputKind {
    Keyboard,
    Mouse,
    Wheel,
    Touch,
    Composition,
    PrepareDrag,
    Drag,
    ReleasePointerCapture,
}

impl HostInputKind {
    fn dispatcher_name(self) -> &'static str {
        match self {
            Self::Keyboard => "keyboard",
            Self::Mouse => "mouse",
            Self::Wheel => "wheel",
            Self::Touch => "touch",
            Self::Composition => "composition",
            Self::PrepareDrag => "prepare-drag",
            Self::Drag => "drag",
            Self::ReleasePointerCapture => "release-pointer-capture",
        }
    }
}

impl JsRuntime {
    /// Returns the already loaded Document of a connected iframe in this runtime.
    ///
    /// Returns `None` for foreign nodes, non-iframes, disconnected or retired
    /// frames, and frames without an existing Document record. This read-only
    /// lookup does not load resources, initialize a Realm or execute JavaScript.
    /// The returned owned handle is a snapshot: navigation does not retarget it.
    /// `dispatch_host_input` rechecks whether that Document is still active.
    pub fn frame_content_document(&self, frame: &NodeHandle) -> Option<NodeHandle> {
        let state = self.host_state.borrow();
        if !frame.is_html_element()
            || !frame.has_tag_name("iframe")
            || !state
                .get_node(frame.identity())
                .is_some_and(|known| known == *frame)
            || !document_root_for_node(frame)
                .is_some_and(|document| state.document_is_active(document.identity()))
        {
            return None;
        }
        state
            .iframe_documents
            .get(&frame.identity())
            .filter(|entry| state.document_is_active(entry.document.identity()))
            .map(|entry| entry.document.clone())
    }

    /// Delivers one owned input command in the explicitly selected Document Realm.
    ///
    /// Rejects foreign or retired nodes, a non-Document entry and non-finite
    /// numeric data before delivering input. Targets may belong to another live
    /// Document in this runtime; the existing input forwarding policy applies.
    /// Returns the dispatcher's boolean or numeric result, including cancellation
    /// and scroll-default flags. Listener errors retain existing event semantics.
    /// Clears synchronous dialog metadata on success or error. Does not drain
    /// jobs, commit navigation or grant a host Storage Access activation scope.
    pub fn dispatch_host_input(
        &mut self,
        entry_document: &NodeHandle,
        command: host_input_types::HostInputCommand,
    ) -> JsResult<JsValue> {
        let result = (|| {
            let prepared = {
                let state = self.host_state.borrow();
                let id = entry_document.identity();
                if entry_document.node_type() != NodeType::Document
                    || !state.document_is_active(id)
                    || !state
                        .get_node(id)
                        .is_some_and(|node| node == *entry_document)
                {
                    return Err(JsNativeError::typ()
                        .with_message("host input entry must be a live Document in this runtime")
                        .into());
                }
                host_input_command::prepare(&state, &command)?
            };
            let document = entry_document.identity();
            let _client = HostInputClientScope::new(&self.host_state, document);
            self.dispatch_private_input_with_related_target(
                document,
                prepared.kind.dispatcher_name(),
                &prepared.arguments,
                prepared.related_target,
            )
        })();
        self.finish_sync_js_boundary(result)
    }

    /// Delivers one host-validated input primitive in an active Document Realm.
    ///
    /// The caller owns validation of the document, node identities and event
    /// fields. Arguments are data, never JavaScript source. This synchronous
    /// boundary clears rejected dialog metadata on success and error, as eval
    /// did. It does not drain jobs, commit navigation or confer Storage Access
    /// activation; the caller retains those boundaries around each primitive.
    /// The callback's WindowProxy group lookup uses the scoped host client only
    /// when no author frame supplies one. The client scope ends before return.
    pub(crate) fn dispatch_host_input_primitive(
        &mut self,
        document: usize,
        kind: HostInputKind,
        arguments: &[serde_json::Value],
    ) -> JsResult<JsValue> {
        let _client = HostInputClientScope::new(&self.host_state, document);
        let result = self.dispatch_private_input_in_document_realm(
            document,
            kind.dispatcher_name(),
            arguments,
        );
        self.finish_sync_js_boundary(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_client_is_cleared_after_success_and_rejected_dialog() {
        let mut runtime = JsRuntime::new().unwrap();
        let document = runtime.document().identity();
        runtime
            .dispatch_host_input_primitive(
                document,
                HostInputKind::Keyboard,
                &[
                    serde_json::json!("keydown"),
                    serde_json::json!({"key":"Shift"}),
                ],
            )
            .unwrap();
        assert!(
            runtime
                .host_state
                .borrow()
                .input_bridge
                .host_client_document
                .is_none()
        );
        runtime
            .eval("Date.now = () => alert('host input boundary')")
            .unwrap();
        assert!(
            runtime
                .dispatch_host_input_primitive(
                    document,
                    HostInputKind::Keyboard,
                    &[serde_json::json!("keydown"), serde_json::json!({"key":"x"})],
                )
                .is_err()
        );
        assert!(
            runtime
                .host_state
                .borrow()
                .input_bridge
                .host_client_document
                .is_none()
        );
        assert_eq!(runtime.pending_javascript_dialog(), None);
    }

    #[test]
    fn input_client_does_not_replace_an_explicit_author_realm() {
        let mut runtime = JsRuntime::new().unwrap();
        let document = runtime.document().identity();
        for identity in [None, Some(123usize)] {
            let realm = runtime.context.create_realm().unwrap();
            if let Some(identity) = identity {
                realm.host_defined_mut().insert(ModuleDocumentId(identity));
            }
            let previous = runtime.context.enter_realm(realm);
            runtime
                .context
                .register_global_builtin_callable(
                    js_string!("inputClientProbe"),
                    0,
                    NativeFunction::from_copy_closure(|_, _, context| {
                        Ok(window_access_client_document(context)?
                            .map(|id| JsValue::from(id as f64))
                            .unwrap_or_else(JsValue::undefined))
                    }),
                )
                .unwrap();
            let scope = HostInputClientScope::new(&runtime.host_state, document);
            let result = runtime.eval("inputClientProbe()").unwrap();
            runtime.context.enter_realm(previous);
            drop(scope);
            match identity {
                Some(identity) => assert_eq!(result.as_number(), Some(identity as f64)),
                None => assert!(result.is_undefined()),
            }
            assert!(
                runtime
                    .host_state
                    .borrow()
                    .input_bridge
                    .host_client_document
                    .is_none()
            );
        }
    }

    #[test]
    fn input_client_scope_restores_nested_and_panicking_calls() {
        let mut runtime = JsRuntime::new().unwrap();
        let document = runtime.document().identity();
        let outer = HostInputClientScope::new(&runtime.host_state, document);
        {
            let _inner = HostInputClientScope::new(&runtime.host_state, usize::MAX);
            assert_eq!(
                runtime
                    .with_active_host(|context| window_access_client_document(context))
                    .unwrap(),
                None
            );
        }
        assert_eq!(
            runtime
                .with_active_host(|context| window_access_client_document(context))
                .unwrap(),
            Some(document)
        );
        let host = Rc::clone(&runtime.host_state);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _inner = HostInputClientScope::new(&host, usize::MAX);
            panic!("test input client unwinding");
        }));
        assert!(result.is_err());
        assert_eq!(
            runtime
                .with_active_host(|context| window_access_client_document(context))
                .unwrap(),
            Some(document)
        );
        drop(outer);
        assert_eq!(
            runtime
                .with_active_host(|context| window_access_client_document(context))
                .unwrap(),
            None
        );
    }

    fn assert_bound_listener_denied(separate_group: bool) {
        let sandbox = if separate_group {
            ""
        } else {
            "sandbox='allow-scripts'"
        };
        let document = crate::html::TreeBuilder::parse(&format!(
            "<html><body><iframe id='frame' {sandbox} \
             srcdoc='<body><div id=target></div></body>'></iframe></body></html>",
        ))
        .document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://input.test/").unwrap();
        runtime.eval("globalThis.inputFrame = document.getElementById('frame'); void inputFrame.contentWindow;").unwrap();
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
        runtime
            .eval_in_document_realm(
                child_id,
                "Object.defineProperty(globalThis, 'privateInputProbe', {configurable:true, get() { \
                document.getElementById('target').setAttribute('data-read', 'yes'); return 42; \
             }});",
            )
            .unwrap();
        runtime.eval(
            "globalThis.inputClientErrors = []; \
             addEventListener('error', event => {inputClientErrors.push(event.error.name); event.preventDefault()}); \
             document.body.addEventListener('keydown', \
               Reflect.get.bind(null, inputFrame.contentWindow, 'privateInputProbe')); \
             document.body.tabIndex = 0; document.body.focus();",
        ).unwrap();
        if separate_group {
            // Isolate group policy from origin policy without navigating
            // or replacing the callback and proxy saved by the listener.
            let mut state = runtime.host_state.borrow_mut();
            let group = state.next_document_group_id;
            state.next_document_group_id += 1;
            state.document_group_ids.insert(child_id, group);
        }
        let main = runtime.document().identity();
        {
            let state = runtime.host_state.borrow();
            let main_origin = state.document_security_origins.get(&main).unwrap();
            let child_origin = state.document_security_origins.get(&child_id).unwrap();
            let main_group = state.document_group_ids.get(&main).unwrap();
            let child_group = state.document_group_ids.get(&child_id).unwrap();
            assert_eq!(main_origin == child_origin, separate_group);
            assert_eq!(main_group == child_group, !separate_group);
        }
        runtime
            .dispatch_host_input_primitive(
                main,
                HostInputKind::Keyboard,
                &[
                    serde_json::json!("keydown"),
                    serde_json::json!({"key":"Shift"}),
                ],
            )
            .unwrap();
        assert!(
            runtime
                .host_state
                .borrow()
                .input_bridge
                .host_client_document
                .is_none()
        );
        runtime.run_jobs().unwrap();
        assert_eq!(
            child
                .query_selector("#target")
                .unwrap()
                .get_attribute("data-read"),
            None,
            "a bound native listener crossed the origin/group boundary",
        );
        assert_eq!(
            runtime
                .eval("JSON.stringify(inputClientErrors)")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "[\"SecurityError\"]",
        );
        runtime
            .eval("document.body.dispatchEvent(new KeyboardEvent('keydown', {key:'Shift'}))")
            .unwrap();
        runtime.run_jobs().unwrap();
        assert_eq!(
            child
                .query_selector("#target")
                .unwrap()
                .get_attribute("data-read"),
            None
        );
        assert_eq!(
            runtime
                .eval("JSON.stringify(inputClientErrors)")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "[\"SecurityError\",\"SecurityError\"]",
        );
        assert!(
            runtime
                .host_state
                .borrow()
                .input_bridge
                .host_client_document
                .is_none()
        );
    }

    #[test]
    fn bound_native_listener_cannot_cross_opaque_origin_in_same_group() {
        assert_bound_listener_denied(false);
    }

    #[test]
    fn bound_native_listener_cannot_cross_group_with_same_origin() {
        assert_bound_listener_denied(true);
    }

    #[test]
    fn host_client_fallback_rechecks_document_retirement() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                "globalThis.retiringFrame = document.createElement('iframe'); \
             document.body.appendChild(retiringFrame); void retiringFrame.contentDocument;",
            )
            .unwrap();
        let child = runtime
            .host_state
            .borrow()
            .iframe_documents
            .values()
            .next()
            .unwrap()
            .document
            .identity();
        let scope = HostInputClientScope::new(&runtime.host_state, child);
        assert_eq!(
            runtime
                .with_active_host(|context| window_access_client_document(context))
                .unwrap(),
            Some(child)
        );
        runtime.eval("retiringFrame.srcdoc = '<p>new generation</p>'; void retiringFrame.contentDocument").unwrap();
        assert!(!runtime.host_state.borrow().document_is_active(child));
        assert_eq!(
            runtime
                .with_active_host(|context| window_access_client_document(context))
                .unwrap(),
            None
        );
        drop(scope);
        assert!(
            runtime
                .host_state
                .borrow()
                .input_bridge
                .host_client_document
                .is_none()
        );
    }
}
