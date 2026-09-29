//! Worklets: the isolated WorkletGlobalScope shared by a page's Worklet
//! instances, module loading, registration bookkeeping and teardown.

use super::*;

/// The deterministic Worklet runtime owned by a page global. Worklet modules
/// execute in a separate Boa realm; only module metadata crosses back to the
/// page, so no `JsValue` from the isolated global can leak into the Window.
pub(super) type WorkletRuntimeHandle = Rc<RefCell<JsRuntime>>;

/// Tears down the isolated WorkletGlobalScope owned by `state`. Taking the
/// handle before borrowing the child realm breaks the owner/child cycle even
/// when navigation drops the page while a module task is pending.
pub(super) fn terminate_worklet_runtime(state: &Rc<RefCell<HostState>>) {
    let runtime = state.borrow_mut().worklet_runtime.take();
    let Some(runtime) = runtime else {
        return;
    };
    let child_state = Rc::clone(&runtime.borrow().host_state);
    let mut child_state = child_state.borrow_mut();
    child_state.worklet_terminated = true;
    child_state.worklet_owner = None;
    child_state.worklet_modules.clear();
    child_state.worklet_registrations.clear();
}

fn worklet_status(ok: bool, name: &str, message: &str, duplicate: bool) -> JsValue {
    let name = serde_json::to_string(name).unwrap_or_else(|_| "\"Error\"".to_string());
    let message = serde_json::to_string(message).unwrap_or_else(|_| "\"\"".to_string());
    js_string!(format!(
        "{{\"ok\":{ok},\"duplicate\":{duplicate},\"name\":{name},\"message\":{message}}}"
    ))
    .into()
}

fn worklet_error_name(error: &str) -> &'static str {
    for name in [
        "InvalidModificationError",
        "InvalidStateError",
        "DataCloneError",
        "SecurityError",
        "NotAllowedError",
        "OperationError",
        "NetworkError",
        "AbortError",
        "AggregateError",
        "EvalError",
        "RangeError",
        "ReferenceError",
        "SyntaxError",
        "TypeError",
        "URIError",
        "Error",
    ] {
        if error.starts_with(name) || error.contains(&format!("name: \"{name}\"")) {
            return name;
        }
    }
    "OperationError"
}

fn create_worklet_native(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let id = with_host_state(|state| {
        let mut state = state.borrow_mut();
        let id = state.next_worklet_id;
        state.next_worklet_id = state.next_worklet_id.saturating_add(1);
        Ok(id.to_string())
    })?;
    Ok(js_string!(id).into())
}

/// Loads and evaluates one Worklet module in the owner page's isolated
/// WorkletGlobalScope. The JavaScript wrapper converts this status record into
/// the asynchronous `addModule()` Promise lifecycle.
fn worklet_add_module_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = worker_id_argument(args, context)?;
    let requested_url = string_argument(args.get(1), "", context)?;
    with_host_state(|owner_state| {
        let (owner_url, base_url, storage, session_id, user_agent, secure) = {
            let state = owner_state.borrow();
            (
                state
                    .base_url
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| state.location_href.clone()),
                state.base_url.clone(),
                state.storage_manager.clone(),
                state.storage_session_id,
                state.navigator_user_agent.clone(),
                host_is_secure_context(&state),
            )
        };
        if !secure {
            return Ok(worklet_status(
                false,
                "NotAllowedError",
                "Worklet modules require a secure context",
                false,
            ));
        }
        let resolved_url = match resolve_worker_url(&requested_url, &owner_url, base_url.as_ref()) {
            Ok(url) => url,
            Err(error) => {
                return Ok(worklet_status(
                    false,
                    "SecurityError",
                    &error.to_string(),
                    false,
                ));
            }
        };
        // A module map hit is resolved entirely from the deterministic cache:
        // do not refetch a resource that has already executed successfully.
        // This also ensures a repeated `addModule()` call cannot observe a
        // changing network response after the first successful registration.
        if let Some(runtime) = owner_state.borrow().worklet_runtime.clone() {
            let runtime_ref = runtime.borrow();
            let worklet_state = runtime_ref.host_state.borrow();
            if worklet_state.worklet_terminated {
                return Ok(worklet_status(
                    false,
                    "InvalidStateError",
                    "WorkletGlobalScope has been torn down",
                    false,
                ));
            }
            if worklet_state.worklet_modules.contains(&resolved_url) {
                return Ok(worklet_status(true, "", "", true));
            }
        }
        // Fetch before constructing the isolated realm. This keeps failed
        // addModule() calls side-effect free and makes retries deterministic.
        let fetched = {
            let mut state = owner_state.borrow_mut();
            fetch_script_resource_with_client(
                &requested_url,
                base_url.as_ref(),
                &mut state.http_client,
            )
        };
        let (effective_url, source, _) = match fetched {
            Some(value) => value,
            None => {
                return Ok(worklet_status(
                    false,
                    "TypeError",
                    &format!("failed to fetch Worklet module: {requested_url}"),
                    false,
                ));
            }
        };
        // A redirect must not turn a same-origin module into a cross-origin
        // script. Data URLs are intentionally retained for deterministic
        // inline tests, matching the existing Worker implementation.
        if !effective_url.starts_with("data:") {
            let owner = match owner_url.parse::<crate::http::Url>() {
                Ok(url) => url,
                Err(_) => {
                    return Ok(worklet_status(
                        false,
                        "SecurityError",
                        "Worklet owner has no origin",
                        false,
                    ));
                }
            };
            let effective = match effective_url.parse::<crate::http::Url>() {
                Ok(url) => url,
                Err(_) => {
                    return Ok(worklet_status(
                        false,
                        "TypeError",
                        "Worklet module URL is invalid",
                        false,
                    ));
                }
            };
            if !same_origin_url(&owner, &effective) {
                return Ok(worklet_status(
                    false,
                    "SecurityError",
                    "Worklet module must be same-origin",
                    false,
                ));
            }
        }

        let runtime_handle = if let Some(runtime) = owner_state.borrow().worklet_runtime.clone() {
            runtime
        } else {
            let mut runtime = match JsRuntime::with_document_url_and_storage(
                blank_html_document(),
                &owner_url,
                storage,
                session_id,
            ) {
                Ok(runtime) => runtime,
                Err(error) => {
                    return Ok(worklet_status(
                        false,
                        "OperationError",
                        &error.to_string(),
                        false,
                    ));
                }
            };
            runtime.set_user_agent(user_agent);
            let worklet_state = Rc::clone(&runtime.host_state);
            {
                let mut state = worklet_state.borrow_mut();
                state.worklet_owner = Some(Rc::clone(owner_state));
                state.worklet_id = Some(id);
                state.worklet_terminated = false;
            }
            if let Err(error) = runtime.eval(&format!(
                "__omoikane_install_worklet_global({effective_url:?}, {id:?})"
            )) {
                return Ok(worklet_status(
                    false,
                    "OperationError",
                    &error.to_string(),
                    false,
                ));
            }
            let runtime = Rc::new(RefCell::new(runtime));
            owner_state.borrow_mut().worklet_runtime = Some(Rc::clone(&runtime));
            runtime
        };

        {
            let runtime = runtime_handle.borrow();
            let state = runtime.host_state.borrow();
            if state.worklet_terminated {
                return Ok(worklet_status(
                    false,
                    "InvalidStateError",
                    "WorkletGlobalScope has been torn down",
                    false,
                ));
            }
            if state.worklet_modules.contains(&resolved_url) {
                return Ok(worklet_status(true, "", "", true));
            }
        }

        let evaluation = {
            let mut runtime = runtime_handle.borrow_mut();
            let document = runtime.document();
            let (result, _, _) = runtime.eval_module_timed(&source, &effective_url, document);
            result
        };
        if let Err(error) = evaluation {
            let message = error.to_string();
            return Ok(worklet_status(
                false,
                worklet_error_name(&message),
                &message,
                false,
            ));
        }
        runtime_handle
            .borrow()
            .host_state
            .borrow_mut()
            .worklet_modules
            .insert(resolved_url);
        Ok(worklet_status(true, "", "", false))
    })
}

fn worklet_register_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = worker_id_argument(args, context)?;
    let name = string_argument(args.get(1), "", context)?;
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        if state.worklet_id != Some(id) || state.worklet_terminated {
            return Ok(JsValue::from(false));
        }
        state.worklet_registrations.insert(name);
        Ok(JsValue::from(true))
    })
}

fn worklet_registered_names_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = worker_id_argument(args, context)?;
    with_host_state(|owner_state| {
        let runtime = owner_state.borrow().worklet_runtime.clone();
        let Some(runtime) = runtime else {
            return Ok(js_string!("[]").into());
        };
        let runtime_ref = runtime.borrow();
        let state = runtime_ref.host_state.borrow();
        if state.worklet_id != Some(id) || state.worklet_terminated {
            return Ok(js_string!("[]").into());
        }
        let mut names: Vec<_> = state.worklet_registrations.iter().cloned().collect();
        names.sort();
        let json = serde_json::to_string(&names).unwrap_or_else(|_| "[]".to_string());
        Ok(js_string!(json).into())
    })
}

fn worklet_module_count_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = worker_id_argument(args, context)?;
    with_host_state(|owner_state| {
        let runtime = owner_state.borrow().worklet_runtime.clone();
        let Some(runtime) = runtime else {
            return Ok(JsValue::from(0));
        };
        let runtime_ref = runtime.borrow();
        let state = runtime_ref.host_state.borrow();
        if state.worklet_id != Some(id) || state.worklet_terminated {
            return Ok(JsValue::from(0));
        }
        Ok(JsValue::from(state.worklet_modules.len() as u32))
    })
}

fn worklet_teardown_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = worker_id_argument(args, context)?;
    with_host_state(|owner_state| {
        let runtime = {
            let mut state = owner_state.borrow_mut();
            let Some(runtime) = state.worklet_runtime.take() else {
                return Ok(JsValue::from(false));
            };
            if runtime.borrow().host_state.borrow().worklet_id != Some(id) {
                state.worklet_runtime = Some(runtime);
                return Ok(JsValue::from(false));
            }
            runtime
        };
        {
            let worklet_state = Rc::clone(&runtime.borrow().host_state);
            let mut state = worklet_state.borrow_mut();
            state.worklet_terminated = true;
            state.worklet_owner = None;
            state.worklet_modules.clear();
            state.worklet_registrations.clear();
        }
        Ok(JsValue::from(true))
    })
}

/// Registers the private Worklet hooks used by the DOM bootstrap.
pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    for (name, length, function) in [
        (
            js_string!("__omoikane_create_worklet"),
            0,
            NativeFunction::from_copy_closure(create_worklet_native),
        ),
        (
            js_string!("__omoikane_worklet_add_module"),
            2,
            NativeFunction::from_copy_closure(worklet_add_module_native),
        ),
        (
            js_string!("__omoikane_worklet_register"),
            2,
            NativeFunction::from_copy_closure(worklet_register_native),
        ),
        (
            js_string!("__omoikane_worklet_registered_names"),
            1,
            NativeFunction::from_copy_closure(worklet_registered_names_native),
        ),
        (
            js_string!("__omoikane_worklet_module_count"),
            1,
            NativeFunction::from_copy_closure(worklet_module_count_native),
        ),
        (
            js_string!("__omoikane_worklet_teardown"),
            1,
            NativeFunction::from_copy_closure(worklet_teardown_native),
        ),
    ] {
        register_private_builtin_callable(context, bindings, name, length, function)?;
    }
    Ok(())
}

impl JsRuntime {
    pub(super) fn advance_worklet_clocks(&mut self, elapsed_ms: u64) {
        let Some(runtime) = self.host_state.borrow().worklet_runtime.clone() else {
            return;
        };
        if runtime.borrow().host_state.borrow().worklet_terminated {
            return;
        }
        runtime
            .borrow_mut()
            .host_state
            .borrow_mut()
            .event_loop
            .advance(elapsed_ms);
    }

    /// Pumps the page-owned WorkletGlobalScope between page tasks. Worklet
    /// timers and posted microtasks stay in the isolated realm, but their
    /// deterministic clock advances with the owner page's clock.
    pub(super) fn run_worklet_background_tasks(&mut self) {
        if self.host_state.borrow().worklet_id.is_some() {
            return;
        }
        let Some(runtime) = self.host_state.borrow().worklet_runtime.clone() else {
            return;
        };
        let (result, errors, terminated) = {
            let mut runtime = runtime.borrow_mut();
            if runtime.host_state.borrow().worklet_terminated {
                return;
            }
            let result = runtime.run_until_idle();
            let errors = runtime.take_task_errors();
            let terminated = runtime.host_state.borrow().worklet_terminated;
            (result, errors, terminated)
        };
        if let Err(error) = result {
            self.record_task_error(format!("[worklet] {error}"));
        }
        for error in errors {
            self.record_task_error(format!("[worklet] {error}"));
        }
        if terminated {
            terminate_worklet_runtime(&self.host_state);
        }
    }
}
