//! Worklets: the isolated WorkletGlobalScope shared by a page's Worklet
//! instances, module loading, registration bookkeeping and teardown.

use super::*;

/// Worklet state held by a page global and by the WorkletGlobalScope it owns.
pub(super) struct State {
    /// One isolated WorkletGlobalScope shared by Worklet instances in this
    /// browsing context (including `CSS.paintWorklet`). The runtime is lazily
    /// constructed on the first `addModule()` call.
    runtime: Option<WorkletRuntimeHandle>,
    /// Origin of the single runtime. A different-origin Document must not
    /// evaluate into or inspect an existing WorkletGlobalScope.
    runtime_origin: Option<StorageOrigin>,
    next_id: u64,
    /// Worklet globals point back to their owning page only through this
    /// control-plane handle. It is cleared during teardown so the cycle does
    /// not keep a navigated page alive.
    owner: Option<Rc<RefCell<HostState>>>,
    /// Module graph requests use the owner's cookie jar and partition context;
    /// these are kept separate from the Worklet global's other network APIs.
    module_fetch_context: Option<ModuleFetchContext>,
    id: Option<u64>,
    terminated: bool,
    modules: HashSet<String>,
    registrations: HashSet<String>,
}

struct ModuleFetchContext {
    cookies: Arc<Mutex<crate::http::CookieJar>>,
    cookie_context: crate::http::CookieRequestContext,
}

/// A redirect request checked during I/O and reported after releasing the HTTP client.
struct WorkletPolicyCheck {
    reference: String,
    redirects: usize,
    initial_url: String,
}

enum WorkletFetchError {
    Network,
    Policy,
}

impl State {
    pub(super) fn module_cookie_store(&self) -> Option<Arc<Mutex<crate::http::CookieJar>>> {
        self.module_fetch_context
            .as_ref()
            .map(|context| Arc::clone(&context.cookies))
    }

    pub(super) fn module_cookie_context(&self) -> Option<crate::http::CookieRequestContext> {
        self.module_fetch_context
            .as_ref()
            .map(|context| context.cookie_context.clone())
    }

    /// Whether this state belongs to an isolated WorkletGlobalScope.
    pub(super) fn is_child_global(&self) -> bool {
        self.id.is_some()
    }

    pub(super) fn has_pending_render_work(&self) -> bool {
        !self.terminated
            && self.runtime.as_ref().is_some_and(|runtime| {
                runtime.borrow().next_rendering() != crate::cdp::NextRendering::Idle
            })
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            runtime: None,
            runtime_origin: None,
            next_id: 1,
            owner: None,
            module_fetch_context: None,
            id: None,
            terminated: false,
            modules: HashSet::new(),
            registrations: HashSet::new(),
        }
    }
}

/// The deterministic Worklet runtime owned by a page global. Worklet modules
/// execute in a separate Boa realm; only module metadata crosses back to the
/// page, so no `JsValue` from the isolated global can leak into the Window.
type WorkletRuntimeHandle = Rc<RefCell<JsRuntime>>;

/// Tears down the isolated WorkletGlobalScope owned by `state`. Taking the
/// handle before borrowing the child realm breaks the owner/child cycle even
/// when navigation drops the page while a module task is pending.
pub(super) fn terminate_worklet_runtime(state: &Rc<RefCell<HostState>>) {
    let runtime = {
        let mut state = state.borrow_mut();
        state.worklet.runtime_origin = None;
        state.worklet.runtime.take()
    };
    let Some(runtime) = runtime else {
        return;
    };
    let child_state = Rc::clone(&runtime.borrow().host_state);
    let mut child_state = child_state.borrow_mut();
    child_state.worklet.terminated = true;
    child_state.worklet.owner = None;
    child_state.worklet.module_fetch_context = None;
    child_state.worklet.modules.clear();
    child_state.worklet.registrations.clear();
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

/// Fetches a Worklet module with module-script CORS credentials. A redirected
/// cross-origin request omits cookies, and public-address checks remain active
/// after leaving the initial origin.
fn fetch_worklet_module_source(
    state: &mut HostState,
    requested: &str,
    base: Option<&crate::http::Url>,
    cookie_context: &crate::http::CookieRequestContext,
    origin: &CorsOrigin,
    policy: &CspPolicy,
    checks: &mut Vec<WorkletPolicyCheck>,
) -> Result<(String, String), WorkletFetchError> {
    match resolve_resource_ref(requested, base).ok_or(WorkletFetchError::Network)? {
        ResolvedResource::Data { .. } => fetch_script_resource_with_client_in_context(
            requested,
            base,
            Some(cookie_context),
            &mut state.http_client,
        )
        .map(|(url, source, _, _)| (url, source))
        .ok_or(WorkletFetchError::Network),
        ResolvedResource::Url(initial) => {
            let initial = initial
                .parse::<crate::http::Url>()
                .map_err(|_| WorkletFetchError::Network)?;
            let mut request = HttpRequest::new(Method::Get, initial.clone());
            if requires_public_fetch(&initial, base) {
                request.require_public_ip();
            }
            cookie_context.apply(&mut request);
            let fetched = {
                let mut check_redirect = |url: &crate::http::Url, redirects: usize| {
                    let reference = url.to_string();
                    checks.push(WorkletPolicyCheck {
                        reference: reference.clone(),
                        redirects,
                        initial_url: initial.to_string(),
                    });
                    policy.allows_reference_after_redirects(
                        ResourceType::Script,
                        &reference,
                        redirects,
                    )
                };
                crate::http::cors::fetch_with_timeout_restricting_redirects_checked(
                    &mut state.http_client,
                    request,
                    origin,
                    RequestMode::Cors,
                    CredentialsMode::SameOrigin,
                    RedirectMode::Follow,
                    &mut PreflightCache::default(),
                    None,
                    &mut check_redirect,
                )
            };
            let response = fetched
                .map_err(|error| match error {
                    crate::http::cors::CorsError::RequestPolicy => WorkletFetchError::Policy,
                    _ => WorkletFetchError::Network,
                })?
                .response;
            let effective = response
                .effective_url()
                .map_or_else(|| initial.to_string(), ToString::to_string);
            if !policy.allows_reference_after_redirects(
                ResourceType::Script,
                &effective,
                response.redirect_count(),
            ) {
                if !checks.iter().any(|check| {
                    check.reference == effective && check.redirects == response.redirect_count()
                }) {
                    checks.push(WorkletPolicyCheck {
                        reference: effective,
                        redirects: response.redirect_count(),
                        initial_url: initial.to_string(),
                    });
                }
                return Err(WorkletFetchError::Policy);
            }
            if response.status_code() != 200 {
                state.http_client.report_resource_failure();
                return Err(WorkletFetchError::Network);
            }
            let source = match std::str::from_utf8(response.body()) {
                Ok(source) => source.to_owned(),
                Err(_) => {
                    state.http_client.report_resource_failure();
                    return Err(WorkletFetchError::Network);
                }
            };
            Ok((effective, source))
        }
    }
}

fn create_worklet_native(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let id = with_host_state(|state| {
        let mut state = state.borrow_mut();
        let id = state.worklet.next_id;
        state.worklet.next_id = state.worklet.next_id.saturating_add(1);
        Ok(id.to_string())
    })?;
    Ok(js_string!(id).into())
}

struct WorkletRuntimeConfig {
    owner_url: String,
    storage: StorageManager,
    session_id: u64,
    user_agent: String,
    owner_document_id: usize,
    owner_origin: Option<StorageOrigin>,
    effective_url: String,
    id: u64,
}

fn create_worklet_runtime(
    owner_state: &Rc<RefCell<HostState>>,
    config: WorkletRuntimeConfig,
) -> Result<WorkletRuntimeHandle, JsValue> {
    let mut runtime = JsRuntime::with_document_url_and_storage(
        blank_html_document(),
        &config.owner_url,
        config.storage,
        config.session_id,
    )
    .map_err(|error| worklet_status(false, "OperationError", &error.to_string(), false))?;
    runtime.set_user_agent(config.user_agent);
    let worklet_state = Rc::clone(&runtime.host_state);
    {
        let owner = owner_state.borrow();
        let mut state = worklet_state.borrow_mut();
        state.worklet.module_fetch_context = Some(ModuleFetchContext {
            cookies: Arc::clone(&owner.cookie_store),
            cookie_context: owner
                .document_cookie_request_context(config.owner_document_id)
                .for_worker_internal(),
        });
        let mut isolation = IsolationPolicy::default();
        isolation.embedder = owner
            .isolation_policy_for_document(config.owner_document_id)
            .embedder;
        let worklet_document_id = state.document.identity();
        state
            .document_isolation_policies
            .insert(worklet_document_id, isolation);
        state.worklet.owner = Some(Rc::clone(owner_state));
        state.worklet.id = Some(config.id);
        state.worklet.terminated = false;
    }
    runtime
        .eval(&format!(
            "__omoikane_install_worklet_global({:?}, {:?})",
            config.effective_url, config.id
        ))
        .map_err(|error| worklet_status(false, "OperationError", &error.to_string(), false))?;
    let runtime = Rc::new(RefCell::new(runtime));
    {
        let mut state = owner_state.borrow_mut();
        state.worklet.runtime_origin = config.owner_origin;
        state.worklet.runtime = Some(Rc::clone(&runtime));
    }
    Ok(runtime)
}

fn check_initial_worklet_policy(
    owner_state: &Rc<RefCell<HostState>>,
    document_id: usize,
    resolved_url: &str,
    source: Option<&security_reporting::CspSourceLocation>,
) -> Result<CspPolicy, JsValue> {
    let mut state = owner_state.borrow_mut();
    let Some(document) = state.get_node(document_id) else {
        return Err(worklet_status(
            false,
            "InvalidStateError",
            "Worklet owner Document is no longer active",
            false,
        ));
    };
    let policy = state.csp_policy_for_document(&document);
    if !state.csp_allows_response_for_document_with_source(
        &document,
        ResourceType::Script,
        resolved_url,
        0,
        resolved_url,
        source,
    ) {
        return Err(worklet_status(
            false,
            "AbortError",
            "Worklet module blocked by Content Security Policy",
            false,
        ));
    }
    Ok(policy)
}

fn report_worklet_redirect_checks(
    owner_state: &Rc<RefCell<HostState>>,
    document_id: usize,
    checks: Vec<WorkletPolicyCheck>,
    source: Option<&security_reporting::CspSourceLocation>,
) {
    if checks.is_empty() {
        return;
    }
    let mut state = owner_state.borrow_mut();
    if let Some(document) = state.get_node(document_id) {
        for check in checks {
            state.csp_allows_response_for_document_with_source(
                &document,
                ResourceType::Script,
                &check.reference,
                check.redirects,
                &check.initial_url,
                source,
            );
        }
    }
}

fn evaluate_worklet_module(
    runtime_handle: &WorkletRuntimeHandle,
    source: &str,
    effective_url: &str,
    resolved_url: String,
) -> JsValue {
    let evaluation = {
        let mut runtime = runtime_handle.borrow_mut();
        let document = runtime.document();
        let (result, _, _) = runtime.eval_module_timed(
            source,
            effective_url,
            document,
            crate::http::cors::CredentialsMode::SameOrigin,
        );
        result
    };
    if let Err(error) = evaluation {
        let message = error.to_string();
        return worklet_status(false, worklet_error_name(&message), &message, false);
    }
    runtime_handle
        .borrow()
        .host_state
        .borrow_mut()
        .worklet
        .modules
        .insert(resolved_url);
    worklet_status(true, "", "", false)
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
    let source = security_reporting::capture_csp_source(context);
    with_host_state(|owner_state| {
        let (
            owner_url,
            base_url,
            storage,
            session_id,
            user_agent,
            secure,
            cookie_context,
            origin,
            owner_document_id,
        ) = {
            let state = owner_state.borrow();
            let document_id = state.csp_document_for_context(context)?.identity();
            let base_url = state.base_url_for_document(document_id);
            let owner_url = if document_id == state.document.identity() {
                base_url.as_ref().map(ToString::to_string)
            } else {
                state
                    .document_origins
                    .get(&document_id)
                    .and_then(Option::as_ref)
                    .map(StorageOrigin::serialize)
            }
            .unwrap_or_default();
            (
                owner_url,
                base_url,
                state.storage_manager.clone(),
                state.storage_session_id,
                state.navigator_user_agent.clone(),
                document_is_secure_context(&state, document_id),
                state.document_cookie_request_context(document_id),
                script_errors::document_fetch_origin(&state, document_id),
                document_id,
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
        let owner_origin = StorageOrigin::from_url(&owner_url);
        if owner_state.borrow().worklet.runtime.is_some()
            && owner_state.borrow().worklet.runtime_origin != owner_origin
        {
            return Ok(worklet_status(
                false,
                "SecurityError",
                "Worklet modules require the existing runtime's origin",
                false,
            ));
        }
        let resolved_url = match resolve_resource_ref(&requested_url, base_url.as_ref()) {
            Some(ResolvedResource::Data { .. }) => requested_url.clone(),
            Some(ResolvedResource::Url(url)) => url,
            None => {
                return Ok(worklet_status(
                    false,
                    "SecurityError",
                    "Worklet module URL cannot be resolved",
                    false,
                ));
            }
        };
        let policy = match check_initial_worklet_policy(
            &owner_state,
            owner_document_id,
            &resolved_url,
            source.as_ref(),
        ) {
            Ok(policy) => policy,
            Err(status) => return Ok(status),
        };
        // A module map hit is resolved entirely from the deterministic cache:
        // do not refetch a resource that has already executed successfully.
        // This also ensures a repeated `addModule()` call cannot observe a
        // changing network response after the first successful registration.
        if let Some(runtime) = owner_state.borrow().worklet.runtime.clone() {
            let runtime_ref = runtime.borrow();
            let worklet_state = runtime_ref.host_state.borrow();
            if worklet_state.worklet.terminated {
                return Ok(worklet_status(
                    false,
                    "InvalidStateError",
                    "WorkletGlobalScope has been torn down",
                    false,
                ));
            }
            if worklet_state.worklet.modules.contains(&resolved_url) {
                return Ok(worklet_status(true, "", "", true));
            }
        }
        // Fetch before constructing the isolated realm. This keeps failed
        // addModule() calls side-effect free and makes retries deterministic.
        let mut checks = Vec::new();
        let fetched = {
            let mut state = owner_state.borrow_mut();
            fetch_worklet_module_source(
                &mut state,
                &requested_url,
                base_url.as_ref(),
                &cookie_context,
                &origin,
                &policy,
                &mut checks,
            )
        };
        report_worklet_redirect_checks(&owner_state, owner_document_id, checks, source.as_ref());
        let (effective_url, source) = match fetched {
            Ok(value) => value,
            Err(WorkletFetchError::Policy) => {
                return Ok(worklet_status(
                    false,
                    "AbortError",
                    "Worklet module blocked by Content Security Policy",
                    false,
                ));
            }
            Err(WorkletFetchError::Network) => {
                return Ok(worklet_status(
                    false,
                    "TypeError",
                    &format!("failed to fetch Worklet module: {requested_url}"),
                    false,
                ));
            }
        };
        let runtime_handle = if let Some(runtime) = owner_state.borrow().worklet.runtime.clone() {
            runtime
        } else {
            match create_worklet_runtime(
                &owner_state,
                WorkletRuntimeConfig {
                    owner_url,
                    storage,
                    session_id,
                    user_agent,
                    owner_document_id,
                    owner_origin,
                    effective_url: effective_url.clone(),
                    id,
                },
            ) {
                Ok(runtime) => runtime,
                Err(status) => return Ok(status),
            }
        };

        {
            let runtime = runtime_handle.borrow();
            let state = runtime.host_state.borrow();
            if state.worklet.terminated {
                return Ok(worklet_status(
                    false,
                    "InvalidStateError",
                    "WorkletGlobalScope has been torn down",
                    false,
                ));
            }
            if state.worklet.modules.contains(&resolved_url) {
                return Ok(worklet_status(true, "", "", true));
            }
        }

        Ok(evaluate_worklet_module(
            &runtime_handle,
            &source,
            &effective_url,
            resolved_url,
        ))
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
        if state.worklet.id != Some(id) || state.worklet.terminated {
            return Ok(JsValue::from(false));
        }
        state.worklet.registrations.insert(name);
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
        let runtime = owner_state.borrow().worklet.runtime.clone();
        let Some(runtime) = runtime else {
            return Ok(js_string!("[]").into());
        };
        let runtime_ref = runtime.borrow();
        let state = runtime_ref.host_state.borrow();
        if state.worklet.id != Some(id) || state.worklet.terminated {
            return Ok(js_string!("[]").into());
        }
        let mut names: Vec<_> = state.worklet.registrations.iter().cloned().collect();
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
        let runtime = owner_state.borrow().worklet.runtime.clone();
        let Some(runtime) = runtime else {
            return Ok(JsValue::from(0));
        };
        let runtime_ref = runtime.borrow();
        let state = runtime_ref.host_state.borrow();
        if state.worklet.id != Some(id) || state.worklet.terminated {
            return Ok(JsValue::from(0));
        }
        Ok(JsValue::from(state.worklet.modules.len() as u32))
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
            let Some(runtime) = state.worklet.runtime.take() else {
                return Ok(JsValue::from(false));
            };
            if runtime.borrow().host_state.borrow().worklet.id != Some(id) {
                state.worklet.runtime = Some(runtime);
                return Ok(JsValue::from(false));
            }
            runtime
        };
        {
            let worklet_state = Rc::clone(&runtime.borrow().host_state);
            let mut state = worklet_state.borrow_mut();
            state.worklet.terminated = true;
            state.worklet.owner = None;
            state.worklet.module_fetch_context = None;
            state.worklet.modules.clear();
            state.worklet.registrations.clear();
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
        let Some(runtime) = self.host_state.borrow().worklet.runtime.clone() else {
            return;
        };
        if runtime.borrow().host_state.borrow().worklet.terminated {
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
        if self.host_state.borrow().worklet.id.is_some() {
            return;
        }
        let Some(runtime) = self.host_state.borrow().worklet.runtime.clone() else {
            return;
        };
        let (result, errors, terminated) = {
            let mut runtime = runtime.borrow_mut();
            if runtime.host_state.borrow().worklet.terminated {
                return;
            }
            let result = runtime.run_until_idle();
            let errors = runtime.take_task_errors();
            let terminated = runtime.host_state.borrow().worklet.terminated;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{HttpParseError, HttpResponse, HttpTransport};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn owner_script_src_elem_blocks_worklet_before_request() {
        struct CountingTransport(Arc<AtomicUsize>);

        impl HttpTransport for CountingTransport {
            fn send(
                &self,
                _: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<HttpResponse, HttpParseError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(HttpResponse::new(
                    200,
                    "OK",
                    Vec::new(),
                    b"registerPaint('blocked', class {});".to_vec(),
                ))
            }
        }

        let mut runtime = JsRuntime::with_document_and_url(
            blank_html_document(),
            "https://isolated.example.test/page",
        )
        .unwrap();
        runtime.install_csp_policy(&["script-src-elem 'none'; worker-src *".into()]);
        let requests = Arc::new(AtomicUsize::new(0));
        runtime
            .set_http_resource_transport(Some(Arc::new(CountingTransport(Arc::clone(&requests)))));
        runtime
            .eval(
                "globalThis.workletResult = 'pending'; \
                 CSS.paintWorklet.addModule('/blocked.js').then( \
                   () => workletResult = 'loaded', error => workletResult = error.name);",
            )
            .unwrap();
        runtime.run_until_idle().unwrap();

        assert_eq!(requests.load(Ordering::SeqCst), 0);
        assert_eq!(
            runtime
                .eval("workletResult")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "AbortError"
        );
    }

    #[test]
    fn owner_csp_blocks_worklet_redirect_before_target_request() {
        struct RedirectTransport(Arc<Mutex<Vec<String>>>);

        impl HttpTransport for RedirectTransport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<HttpResponse, HttpParseError> {
                self.0.lock().unwrap().push(request.url().to_string());
                Ok(match request.url().path() {
                    "/entry.js" => HttpResponse::new(
                        302,
                        "Found",
                        vec![(
                            "Location".into(),
                            "https://other.example.test/blocked.js".into(),
                        )],
                        Vec::new(),
                    ),
                    "/blocked.js" => HttpResponse::new(
                        200,
                        "OK",
                        vec![(
                            "Access-Control-Allow-Origin".into(),
                            "https://isolated.example.test".into(),
                        )],
                        b"registerPaint('blocked', class {});".to_vec(),
                    ),
                    path => panic!("unexpected Worklet module path: {path}"),
                })
            }
        }

        let mut runtime = JsRuntime::with_document_and_url(
            blank_html_document(),
            "https://isolated.example.test/page",
        )
        .unwrap();
        runtime.install_csp_policy(&["script-src-elem 'self'".into()]);
        let requests = Arc::new(Mutex::new(Vec::new()));
        runtime
            .set_http_resource_transport(Some(Arc::new(RedirectTransport(Arc::clone(&requests)))));
        runtime
            .eval(
                "globalThis.workletResult = 'pending'; \
                 CSS.paintWorklet.addModule('/entry.js').then( \
                   () => workletResult = 'loaded', error => workletResult = error.name);",
            )
            .unwrap();
        runtime.run_until_idle().unwrap();

        assert_eq!(
            *requests.lock().unwrap(),
            vec!["https://isolated.example.test/entry.js"]
        );
        assert_eq!(
            runtime
                .eval("workletResult")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "AbortError"
        );
    }

    #[test]
    fn owner_report_only_csp_allows_worklet_redirect_and_notifies_document() {
        struct RedirectTransport(Arc<Mutex<Vec<String>>>);

        impl HttpTransport for RedirectTransport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<HttpResponse, HttpParseError> {
                self.0.lock().unwrap().push(request.url().to_string());
                Ok(match request.url().path() {
                    "/entry.js" => HttpResponse::new(
                        302,
                        "Found",
                        vec![(
                            "Location".into(),
                            "https://other.example.test/module.js".into(),
                        )],
                        Vec::new(),
                    ),
                    "/module.js" => HttpResponse::new(
                        200,
                        "OK",
                        vec![(
                            "Access-Control-Allow-Origin".into(),
                            "https://isolated.example.test".into(),
                        )],
                        b"registerPaint('allowed', class {});".to_vec(),
                    ),
                    "/reports" => HttpResponse::new(204, "No Content", Vec::new(), Vec::new()),
                    path => panic!("unexpected Worklet module path: {path}"),
                })
            }
        }

        let mut runtime = JsRuntime::with_document_and_url(
            blank_html_document(),
            "https://isolated.example.test/page",
        )
        .unwrap();
        runtime.install_document_response_headers(&[
            (
                "Content-Security-Policy-Report-Only".into(),
                "script-src-elem 'self'; report-to observations".into(),
            ),
            (
                "Reporting-Endpoints".into(),
                "observations=\"https://isolated.example.test/reports\"".into(),
            ),
        ]);
        let requests = Arc::new(Mutex::new(Vec::new()));
        runtime
            .set_http_resource_transport(Some(Arc::new(RedirectTransport(Arc::clone(&requests)))));
        runtime
            .eval(
                "globalThis.workletResult = 'pending'; globalThis.workletEvents = []; \
                 globalThis.workletReports = []; \
                 document.addEventListener('securitypolicyviolation', event => \
                   workletEvents.push([event.target === document, event.effectiveDirective, \
                     event.blockedURI, event.disposition])); \
                 new ReportingObserver(batch => workletReports.push(...batch)).observe(); \
                 CSS.paintWorklet.addModule('/entry.js').then( \
                   () => workletResult = 'loaded', error => workletResult = error.name);",
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
        security_reporting::wait_for_report_delivery(&mut runtime);

        assert_eq!(
            *requests.lock().unwrap(),
            vec![
                "https://isolated.example.test/entry.js",
                "https://other.example.test/module.js",
                "https://isolated.example.test/reports",
            ]
        );
        assert!(
            runtime
                .eval(
                    "workletResult === 'loaded' && \
                     JSON.stringify(workletEvents) === \
                       '[[true,\"script-src-elem\",\"https://isolated.example.test/entry.js\",\"report\"]]' && \
                     workletReports.length === 1 && \
                     workletReports[0].body.effectiveDirective === 'script-src-elem' && \
                     workletReports[0].body.disposition === 'report'"
                )
                .unwrap()
                .to_boolean(),
            "events: {:?}; reports: {:?}",
            runtime.eval("JSON.stringify(workletEvents)").unwrap(),
            runtime.eval("JSON.stringify(workletReports)").unwrap()
        );
    }

    #[test]
    fn worklet_cross_origin_redirect_uses_cors_without_credentials() {
        struct RedirectTransport(Arc<Mutex<Vec<(String, Option<String>, bool)>>>);

        impl HttpTransport for RedirectTransport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<HttpResponse, HttpParseError> {
                self.0.lock().unwrap().push((
                    request.url().to_string(),
                    request.header("cookie").map(str::to_owned),
                    request.requires_public_ip(),
                ));
                Ok(match request.url().path() {
                    "/entry.js" => HttpResponse::new(
                        302,
                        "Found",
                        vec![(
                            "Location".into(),
                            "https://other.example.test/module.js".into(),
                        )],
                        Vec::new(),
                    ),
                    "/module.js" => HttpResponse::new(
                        200,
                        "OK",
                        vec![(
                            "Access-Control-Allow-Origin".into(),
                            "https://isolated.example.test".into(),
                        )],
                        b"registerPaint('test', class {});".to_vec(),
                    ),
                    path => panic!("unexpected Worklet module path: {path}"),
                })
            }
        }

        let owner_url: crate::http::Url = "https://isolated.example.test/page".parse().unwrap();
        let other_url: crate::http::Url = "https://other.example.test/module.js".parse().unwrap();
        let mut runtime =
            JsRuntime::with_document_and_url(blank_html_document(), &owner_url.to_string())
                .unwrap();
        let store = Arc::clone(&runtime.host_state.borrow().cookie_store);
        {
            let mut store = store.lock().unwrap();
            store.add_from_header_for_url("owner=secret; Secure; SameSite=None", &owner_url);
            store.add_from_header_for_url("other=secret; Secure; SameSite=None", &other_url);
        }
        let seen = Arc::new(Mutex::new(Vec::new()));
        runtime.set_http_resource_transport(Some(Arc::new(RedirectTransport(Arc::clone(&seen)))));
        runtime
            .eval(
                "globalThis.workletResult='pending'; \
                 CSS.paintWorklet.addModule('/entry.js').then( \
                   ()=>workletResult='loaded', error=>workletResult=error.name);",
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime
                .eval("workletResult")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "loaded"
        );
        assert_eq!(
            runtime
                .eval("CSS.paintWorklet.moduleCount")
                .unwrap()
                .as_number(),
            Some(1.0)
        );
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].1.as_deref(), Some("owner=secret"));
        assert!(!seen[0].2);
        assert_eq!(seen[1].1, None);
        assert!(seen[1].2);
        let worklet = runtime.host_state.borrow().worklet.runtime.clone().unwrap();
        let worklet_state = Rc::clone(&worklet.borrow().host_state);
        let worklet_state = worklet_state.borrow();
        assert!(!Arc::ptr_eq(&worklet_state.cookie_store, &store));
        assert!(Arc::ptr_eq(
            &worklet_state.worklet.module_cookie_store().unwrap(),
            &store
        ));
        let owner_context = runtime
            .host_state
            .borrow()
            .document_cookie_request_context(runtime.document().identity())
            .for_worker_internal();
        assert_eq!(
            worklet_state.worklet.module_cookie_context(),
            Some(owner_context)
        );
        assert_eq!(
            worklet_state
                .module_fetch_isolation_for_document(worklet_state.document.identity())
                .embedder,
            crate::http::corp::EmbedderPolicyValue::UnsafeNone
        );
    }

    #[test]
    fn worklet_accepts_cors_approved_cross_origin_module_url() {
        struct CrossOriginTransport(Arc<Mutex<Vec<(Option<String>, bool)>>>);

        impl HttpTransport for CrossOriginTransport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<HttpResponse, HttpParseError> {
                assert_eq!(
                    request.url().to_string(),
                    "https://other.example.test/module.js"
                );
                self.0.lock().unwrap().push((
                    request.header("cookie").map(str::to_owned),
                    request.requires_public_ip(),
                ));
                Ok(HttpResponse::new(
                    200,
                    "OK",
                    vec![(
                        "Access-Control-Allow-Origin".into(),
                        "https://isolated.example.test".into(),
                    )],
                    b"registerPaint('direct', class {});".to_vec(),
                ))
            }
        }

        let module_url: crate::http::Url = "https://other.example.test/module.js".parse().unwrap();
        let mut runtime = JsRuntime::with_document_and_url(
            blank_html_document(),
            "https://isolated.example.test/page",
        )
        .unwrap();
        runtime.install_isolation_policy(&[(
            "Cross-Origin-Embedder-Policy".into(),
            "require-corp".into(),
        )]);
        runtime.install_csp_policy(&["worker-src 'none'; script-src-elem *".into()]);
        let cookie_store = Arc::clone(&runtime.host_state.borrow().cookie_store);
        cookie_store
            .lock()
            .unwrap()
            .add_from_header_for_url("other=secret; Secure; SameSite=None", &module_url);
        let seen = Arc::new(Mutex::new(Vec::new()));
        runtime
            .set_http_resource_transport(Some(Arc::new(CrossOriginTransport(Arc::clone(&seen)))));
        runtime
            .eval(
                "globalThis.workletResult='pending'; \
                 CSS.paintWorklet.addModule('https://other.example.test/module.js').then( \
                   ()=>workletResult='loaded', error=>workletResult=error.name);",
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime
                .eval("workletResult")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "loaded"
        );
        assert_eq!(*seen.lock().unwrap(), vec![(None, true)]);
        let worklet = runtime.host_state.borrow().worklet.runtime.clone().unwrap();
        let worklet_state = Rc::clone(&worklet.borrow().host_state);
        let worklet_state = worklet_state.borrow();
        assert_eq!(
            worklet_state
                .isolation_policy_for_document(worklet_state.document.identity())
                .embedder
                .value,
            cross_origin_isolation::EmbedderPolicyValue::RequireCorp
        );
    }
}
