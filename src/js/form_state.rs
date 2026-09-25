//! Opaque form state exchanged with navigation and autofill hosts.

use super::*;

/// Persisted state of form-associated custom elements in one Document.
///
/// This owns serialized strings, files and entry lists, never JavaScript or DOM
/// objects from the departing runtime. Hosts may serialize it with Serde and
/// restore it after recreating the same document.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct FormStateSnapshot(String);

/// The user-agent operation delivering a saved custom control state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormStateRestoreMode {
    /// Restore persisted state while recreating a session history entry.
    Restore,
    /// Supply a value selected by the host's autofill UI.
    Autocomplete,
}

#[derive(Default)]
pub(super) struct State {
    handlers: HashMap<usize, (JsValue, JsValue)>,
    pending: HashMap<usize, FormStateSnapshot>,
}

impl State {
    pub(super) unsafe fn trace(&self, tracer: &mut Tracer) {
        for (capture, restore) in self.handlers.values() {
            unsafe { capture.trace(tracer) };
            unsafe { restore.trace(tracer) };
        }
    }

    pub(super) fn retire_document(&mut self, document: usize) {
        self.handlers.remove(&document);
        self.pending.remove(&document);
    }
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_register_form_state"),
        3,
        NativeFunction::from_copy_closure(|_, args, context| {
            let document = parse_node_id(args.first(), context)?;
            ensure_same_origin_document(context, document)?;
            let capture = args.get(1).filter(|value| value.is_callable()).cloned();
            let restore = args.get(2).filter(|value| value.is_callable()).cloned();
            let (Some(capture), Some(restore)) = (capture, restore) else {
                return Err(JsNativeError::typ()
                    .with_message("Form state handlers required")
                    .into());
            };
            with_host_state(|state| {
                state
                    .borrow_mut()
                    .form_state
                    .handlers
                    .insert(document, (capture, restore));
                // The Realm is still being bootstrapped. Its restore task must
                // be queued after the host installs the live Realm identity.

                Ok(JsValue::undefined())
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_capture_iframe_form_state"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let id = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, id)?;
            with_host_state(|host| {
                let Some((document, url, name)) = ({
                    let state = host.borrow();
                    state.iframe_documents.get(&id).map(|entry| {
                        (
                            entry.document.identity(),
                            entry.document_url.clone(),
                            state
                                .browsing_context_names
                                .get(&id)
                                .cloned()
                                .unwrap_or_default(),
                        )
                    })
                }) else {
                    return Ok(JsValue::null());
                };
                let form_state = capture_document(host, document, context)?;
                let saved = IframeHistoryState {
                    form_state,
                    url,
                    name,
                };
                let json = serde_json::to_string(&saved)
                    .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
                Ok(js_string!(json).into())
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_restore_iframe_form_state"),
        4,
        NativeFunction::from_copy_closure(|_, args, context| {
            let id = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, id)?;
            let json = args
                .get(1)
                .cloned()
                .unwrap_or_default()
                .to_string(context)?
                .to_std_string_escaped();
            let saved: IframeHistoryState = serde_json::from_str(&json)
                .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
            let history_url = args
                .get(2)
                .cloned()
                .unwrap_or_default()
                .to_string(context)?
                .to_std_string_escaped();
            let restore_name = args.get(3).is_some_and(JsValue::to_boolean);
            with_host_state(|host| {
                let restore = {
                    let mut state = host.borrow_mut();
                    let Some(frame) = state.get_node(id) else {
                        return Ok(JsValue::undefined());
                    };
                    if !state.node_is_in_active_document(&frame) {
                        return Ok(JsValue::undefined());
                    }
                    let document = state
                        .iframe_content_document(&frame)
                        .map_err(|error| JsNativeError::typ().with_message(error))?;
                    let actual_url = &state
                        .iframe_documents
                        .get(&id)
                        .expect("loaded iframe")
                        .document_url;
                    // A redirect must not deliver the previous document's state
                    // to a different resource. Same-document history URLs and
                    // the original srcdoc resource are both legitimate matches.
                    if without_fragment(actual_url) != without_fragment(&saved.url)
                        && without_fragment(actual_url) != without_fragment(&history_url)
                    {
                        return Ok(JsValue::undefined());
                    }
                    if restore_name {
                        state.browsing_context_names.insert(id, saved.name);
                    }
                    let restore = state
                        .form_state
                        .handlers
                        .get(&document.identity())
                        .map(|handlers| handlers.1.clone());
                    if restore.is_none() {
                        state
                            .form_state
                            .pending
                            .insert(document.identity(), saved.form_state.clone());
                    }
                    restore
                };
                if let Some(restore) = restore {
                    deliver_restore(&restore, &saved.form_state, "restore", context)?;
                }
                Ok(JsValue::undefined())
            })
        }),
    )?;
    Ok(())
}

pub(super) fn restore_pending_document(
    host: &Rc<RefCell<HostState>>,
    document: usize,
    context: &mut Context,
) -> JsResult<()> {
    let pending = {
        let mut state = host.borrow_mut();
        let callback = state
            .form_state
            .handlers
            .get(&document)
            .map(|handlers| handlers.1.clone());
        callback.and_then(|callback| {
            state
                .form_state
                .pending
                .remove(&document)
                .map(|snapshot| (callback, snapshot))
        })
    };
    if let Some((callback, snapshot)) = pending {
        deliver_restore(&callback, &snapshot, "restore", context)?;
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct IframeHistoryState {
    form_state: FormStateSnapshot,
    url: String,
    name: String,
}

fn without_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

fn capture_document(
    host: &Rc<RefCell<HostState>>,
    document: usize,
    context: &mut Context,
) -> JsResult<FormStateSnapshot> {
    let callback = host
        .borrow()
        .form_state
        .handlers
        .get(&document)
        .map(|handlers| handlers.0.clone());
    let Some(callback) = callback else {
        return Ok(FormStateSnapshot("[]".to_string()));
    };
    let value = callback.as_callable().expect("capture callback").call(
        &JsValue::undefined(),
        &[],
        context,
    )?;
    let json = value
        .as_string()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid captured form state"))?
        .to_std_string_escaped();
    Ok(FormStateSnapshot(json))
}

fn deliver_restore(
    callback: &JsValue,
    snapshot: &FormStateSnapshot,
    mode: &str,
    context: &mut Context,
) -> JsResult<()> {
    callback.as_callable().expect("restore callback").call(
        &JsValue::undefined(),
        &[
            js_string!(snapshot.0.as_str()).into(),
            js_string!(mode).into(),
        ],
        context,
    )?;
    Ok(())
}

impl JsRuntime {
    /// Captures custom controls' restoration state independently of their
    /// submission values. Controls with null state are omitted.
    pub fn capture_form_state(&mut self) -> JsResult<FormStateSnapshot> {
        let document = self.document().identity();
        let callback = self
            .host_state
            .borrow()
            .form_state
            .handlers
            .get(&document)
            .map(|handlers| handlers.0.clone());
        let Some(callback) = callback else {
            return Ok(FormStateSnapshot("[]".to_string()));
        };
        self.with_active_host(|context| {
            let value = callback
                .as_callable()
                .expect("registered capture callback")
                .call(&JsValue::undefined(), &[], context)?;
            let json = value
                .as_string()
                .ok_or_else(|| JsNativeError::typ().with_message("Invalid captured form state"))?
                .to_std_string_escaped();
            Ok(FormStateSnapshot(json))
        })
    }

    /// Schedules restoration into matching controls in this document.
    ///
    /// Call before executing document scripts for history restoration. A late
    /// custom-element definition can consume its saved state after upgrade.
    /// User callbacks run as DOM tasks, so hosts should drive the normal event
    /// loop (or an asynchronous page task) to deliver them.
    pub fn restore_form_state(
        &mut self,
        snapshot: &FormStateSnapshot,
        mode: FormStateRestoreMode,
    ) -> JsResult<()> {
        let document = self.document().identity();
        let callback = self
            .host_state
            .borrow()
            .form_state
            .handlers
            .get(&document)
            .map(|handlers| handlers.1.clone());
        let Some(callback) = callback else {
            return Ok(());
        };
        let mode = match mode {
            FormStateRestoreMode::Restore => "restore",
            FormStateRestoreMode::Autocomplete => "autocomplete",
        };
        self.with_active_host(|context| {
            callback
                .as_callable()
                .expect("registered restore callback")
                .call(
                    &JsValue::undefined(),
                    &[
                        JsValue::from(JsString::from(snapshot.0.as_str())),
                        JsValue::from(JsString::from(mode)),
                    ],
                    context,
                )?;
            Ok(())
        })
    }
}
