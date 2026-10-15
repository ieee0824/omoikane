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

/// A CSP request/response check reported after releasing the HTTP client.
struct WorkletPolicyCheck {
    reference: String,
    redirects: usize,
    initial_url: String,
}

enum WorkletFetchError {
    Network,
    Policy,
}

/// Owner metadata retained by one Worklet module graph across async imports.
#[derive(Debug, Clone)]
pub(super) struct ModuleOwner {
    pub(super) document_id: usize,
    pub(super) source: Option<security_reporting::CspSourceLocation>,
}

impl State {
    pub(super) fn owner_state(&self) -> Option<Rc<RefCell<HostState>>> {
        self.owner.clone()
    }

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
            // Fetch reports request-stage and response-stage report-only
            // violations separately, including when both check the same URL.
            checks.push(WorkletPolicyCheck {
                reference: effective.clone(),
                redirects: response.redirect_count(),
                initial_url: initial.to_string(),
            });
            if !policy.allows_reference_after_redirects(
                ResourceType::Script,
                &effective,
                response.redirect_count(),
            ) {
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
    graph: ModuleGraphContext,
) -> JsValue {
    let evaluation = {
        let mut runtime = runtime_handle.borrow_mut();
        let document = runtime.document();
        let (result, _, _) =
            runtime.eval_module_timed_with_context(source, effective_url, document, graph);
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
    let author_source = security_reporting::capture_csp_source(context);
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
            author_source.as_ref(),
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
        report_worklet_redirect_checks(
            &owner_state,
            owner_document_id,
            checks,
            author_source.as_ref(),
        );
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
            ModuleGraphContext {
                csp: policy,
                credentials: CredentialsMode::SameOrigin,
                worklet_owner: Some(ModuleOwner {
                    document_id: owner_document_id,
                    source: author_source,
                }),
            },
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

    struct GraphRequestMetadata {
        url: String,
        cookie: Option<String>,
        origin: Option<String>,
        public_only: bool,
    }

    struct GraphTransport {
        root_source: String,
        requests: Arc<Mutex<Vec<String>>>,
        reports: Arc<Mutex<Vec<serde_json::Value>>>,
        metadata: Arc<Mutex<Vec<GraphRequestMetadata>>>,
    }

    impl HttpTransport for GraphTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            self.requests
                .lock()
                .unwrap()
                .push(request.url().to_string());
            self.metadata.lock().unwrap().push(GraphRequestMetadata {
                url: request.url().to_string(),
                cookie: request.header("cookie").map(str::to_owned),
                origin: request.header("origin").map(str::to_owned),
                public_only: request.requires_public_ip(),
            });
            let source = match request.url().path() {
                "/bootstrap.js" => "registerPaint('bootstrap', class {});",
                "/root.js" => &self.root_source,
                "/parent-a.js" | "/parent-b.js" => "import 'https://other.example.test/import.js';",
                "/import.js" => "export const imported = true;",
                "/redirect.js" | "/redirect-a.js" | "/redirect-b.js" | "/missing-redirect.js" => {
                    let target = if request.url().path() == "/missing-redirect.js" {
                        "https://other.example.test/missing.js"
                    } else {
                        "https://other.example.test/import.js"
                    };
                    return Ok(HttpResponse::new(
                        302,
                        "Found",
                        vec![("Location".into(), target.into())],
                        Vec::new(),
                    ));
                }
                "/missing.js" => {
                    return Ok(HttpResponse::new(
                        404,
                        "Not Found",
                        vec![("Access-Control-Allow-Origin".into(), "*".into())],
                        Vec::new(),
                    ));
                }
                "/roundtrip.js" | "/return.js" => {
                    let target = if request.url().path() == "/roundtrip.js" {
                        "https://other.example.test/return.js"
                    } else {
                        "https://isolated.example.test/import.js"
                    };
                    return Ok(HttpResponse::new(
                        302,
                        "Found",
                        vec![
                            ("Location".into(), target.into()),
                            ("Access-Control-Allow-Origin".into(), "*".into()),
                        ],
                        Vec::new(),
                    ));
                }
                "/reports" => {
                    self.reports
                        .lock()
                        .unwrap()
                        .push(serde_json::from_slice(request.body().unwrap()).unwrap());
                    return Ok(HttpResponse::new(204, "No Content", Vec::new(), Vec::new()));
                }
                path => panic!("unexpected Worklet graph path: {path}"),
            };
            Ok(HttpResponse::new(
                200,
                "OK",
                vec![("Access-Control-Allow-Origin".into(), "*".into())],
                source.as_bytes().to_vec(),
            ))
        }
    }

    struct GraphFixture {
        runtime: JsRuntime,
        requests: Arc<Mutex<Vec<String>>>,
        reports: Arc<Mutex<Vec<serde_json::Value>>>,
        metadata: Arc<Mutex<Vec<GraphRequestMetadata>>>,
    }

    impl GraphFixture {
        fn new(policy: &str, report_only: bool, source: &str) -> Self {
            let mut runtime = JsRuntime::with_document_and_url(
                blank_html_document(),
                "https://isolated.example.test/page",
            )
            .unwrap();
            runtime.install_document_response_headers(&[
                (
                    if report_only {
                        "Content-Security-Policy-Report-Only"
                    } else {
                        "Content-Security-Policy"
                    }
                    .into(),
                    policy.into(),
                ),
                ("Reporting-Endpoints".into(), "graph=\"/reports\"".into()),
            ]);
            let requests = Arc::new(Mutex::new(Vec::new()));
            let reports = Arc::new(Mutex::new(Vec::new()));
            let metadata = Arc::new(Mutex::new(Vec::new()));
            let transport = Arc::new(GraphTransport {
                root_source: source.into(),
                requests: Arc::clone(&requests),
                reports: Arc::clone(&reports),
                metadata: Arc::clone(&metadata),
            });
            runtime.set_http_resource_transport(Some(transport.clone()));
            runtime
                .eval("CSS.paintWorklet.addModule('/bootstrap.js');")
                .unwrap();
            runtime.run_until_idle().unwrap();
            let child = runtime.host_state.borrow().worklet.runtime.clone().unwrap();
            {
                let child = child.borrow();
                let cookies = child.host_state.borrow().module_fetch_cookie_store();
                *child.module_loader.fetch_pool.borrow_mut() =
                    Some(ModuleFetchPool::new_for_test(cookies, transport).unwrap());
            }
            Self {
                runtime,
                requests,
                reports,
                metadata,
            }
        }

        fn load(&mut self) {
            self.runtime
                .eval(
                    "globalThis.workletResult = 'pending'; globalThis.workletEvents = []; \
                 globalThis.workletReports = []; \
                 document.addEventListener('securitypolicyviolation', event => \
                   workletEvents.push([event.target === document, event.effectiveDirective, \
                     event.blockedURI, event.disposition])); \
                 new ReportingObserver(batch => workletReports.push(...batch)).observe(); \
                 CSS.paintWorklet.addModule('/root.js').then( \
                   () => workletResult = 'loaded', error => workletResult = error.name);",
                )
                .unwrap();
            self.runtime.run_until_idle().unwrap();
            security_reporting::wait_for_report_delivery(&mut self.runtime);
        }

        fn assert_no_import_request(&self) {
            assert!(
                self.requests
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|url| { url != "https://other.example.test/import.js" })
            );
        }
    }

    #[test]
    fn worklet_graph_evaluates_allowed_data_module_without_http() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self' data:",
            false,
            "import {value} from 'data:text/javascript,export%20const%20value%3D42'; \
             if(value!==42)throw new Error('wrong data export');",
        );
        fixture.load();
        assert!(
            fixture
                .runtime
                .eval("workletResult==='loaded' && workletEvents.length===0",)
                .unwrap()
                .to_boolean()
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn worklet_graph_reports_data_module_block_to_its_owner() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self'",
            false,
            "import 'data:text/javascript,export%20const%20value%3D42';",
        );
        fixture.load();
        assert!(
            fixture
                .runtime
                .eval(
                    "workletResult!=='pending' && workletResult!=='loaded' && \
             JSON.stringify(workletEvents)==='[[true,\"script-src-elem\",\"data\",\"enforce\"]]'",
                )
                .unwrap()
                .to_boolean()
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn worklet_graph_report_only_data_module_runs_and_reports_both_stages() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self'; report-to graph",
            true,
            "import {value} from 'data:text/javascript,export%20const%20value%3D42'; \
             if(value!==42)throw new Error('wrong data export');",
        );
        fixture.load();
        assert!(fixture.runtime.eval(
            "workletResult==='loaded' && workletEvents.length===2 && \
             workletEvents.every(event => JSON.stringify(event)=== \
               '[true,\"script-src-elem\",\"data\",\"report\"]') && \
             workletReports.length===2 && workletReports.every(report => report.body.blockedURL==='data')",
        ).unwrap().to_boolean());
        assert_eq!(fixture.reports.lock().unwrap().len(), 2);
    }

    #[test]
    fn worklet_graph_decodes_base64_data_module() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self' data:",
            false,
            "import {value} from 'data:text/javascript;base64,ZXhwb3J0IGNvbnN0IHZhbHVlPTQyOw=='; \
             if(value!==42)throw new Error('wrong base64 export');",
        );
        fixture.load();
        assert!(
            fixture
                .runtime
                .eval("workletResult==='loaded'")
                .unwrap()
                .to_boolean()
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn worklet_graph_data_fragment_is_identity_without_becoming_source_text() {
        let data = "data:text/javascript,globalThis.dataRuns=(globalThis.dataRuns||0)+1;export const id={};export const url=import.meta.url;";
        let first = serde_json::to_string(&format!("{data}#one")).unwrap();
        let second = serde_json::to_string(&format!("{data}#two")).unwrap();
        let source = format!(
            "import * as a from {first}; import * as b from {first}; import * as c from {second}; \
             if(a.id!==b.id || a.id===c.id || dataRuns!==2 || \
                !a.url.endsWith('#one') || !c.url.endsWith('#two'))throw new Error('wrong module identity');"
        );
        let mut fixture = GraphFixture::new("script-src-elem 'self' data:", false, &source);
        fixture.load();
        assert!(
            fixture
                .runtime
                .eval("workletResult==='loaded'")
                .unwrap()
                .to_boolean()
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn worklet_graph_rejects_data_module_with_non_javascript_mime() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self' data:",
            false,
            "import 'data:text/plain,globalThis.wrongMimeRan%3Dtrue';",
        );
        fixture.load();
        assert!(fixture.runtime.eval(
            "workletResult!=='pending' && workletResult!=='loaded' && workletEvents.length===0",
        ).unwrap().to_boolean());
        let child = fixture
            .runtime
            .host_state
            .borrow()
            .worklet
            .runtime
            .clone()
            .unwrap();
        assert!(
            child
                .borrow_mut()
                .eval("typeof wrongMimeRan==='undefined'")
                .unwrap()
                .to_boolean()
        );
    }

    #[test]
    fn worklet_graph_data_module_does_not_resolve_relative_import_against_document() {
        for specifier in [
            "./import.js",
            "../import.js",
            "/import.js",
            "//other.example.test/import.js",
        ] {
            let data = format!("data:text/javascript,import%20%27{specifier}%27;");
            let source = format!("import {};", serde_json::to_string(&data).unwrap());
            let mut fixture = GraphFixture::new("script-src-elem 'self' data:", false, &source);
            fixture.load();
            assert!(fixture.runtime.eval(
                "workletResult!=='pending' && workletResult!=='loaded' && workletEvents.length===0",
            ).unwrap().to_boolean(), "relative import: {specifier}");
            assert_eq!(
                fixture.requests.lock().unwrap().len(),
                2,
                "relative import: {specifier}"
            );
        }
    }

    #[test]
    fn worklet_graph_data_module_keeps_owner_csp_for_absolute_http_import() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self' data:",
            false,
            "import 'data:text/javascript,import%20%27https://other.example.test/import.js%27;';",
        );
        fixture.load();
        fixture.assert_no_import_request();
        assert!(fixture.runtime.eval(
            "workletResult!=='pending' && workletResult!=='loaded' && \
             JSON.stringify(workletEvents)==='[[true,\"script-src-elem\",\"https://other.example.test/import.js\",\"enforce\"]]'",
        ).unwrap().to_boolean());
    }

    #[test]
    fn worklet_graph_data_module_keeps_http_owner_cookies_origin_and_address_policy() {
        let mut fixture = GraphFixture::new(
            "script-src-elem * data:",
            false,
            "import 'data:text/javascript,import%20%27https://isolated.example.test/import.js%27;import%20%27https://other.example.test/import.js%27;';",
        );
        fixture
            .runtime
            .eval("document.cookie='session=owner';")
            .unwrap();
        fixture.load();
        assert!(
            fixture
                .runtime
                .eval("workletResult==='loaded'")
                .unwrap()
                .to_boolean()
        );
        let metadata = fixture.metadata.lock().unwrap();
        let owner = metadata
            .iter()
            .find(|request| request.url == "https://isolated.example.test/import.js")
            .unwrap();
        assert_eq!(owner.cookie.as_deref(), Some("session=owner"));
        assert!(!owner.public_only);
        let cross = metadata
            .iter()
            .find(|request| request.url == "https://other.example.test/import.js")
            .unwrap();
        assert_eq!(cross.cookie, None);
        assert_eq!(
            cross.origin.as_deref(),
            Some("https://isolated.example.test")
        );
        assert!(cross.public_only);
    }

    #[test]
    fn owner_csp_blocks_worklet_graph_import_before_request() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self'",
            false,
            "import 'https://other.example.test/import.js'; registerPaint('root', class {});",
        );
        fixture.load();
        assert_eq!(
            *fixture.requests.lock().unwrap(),
            vec![
                "https://isolated.example.test/bootstrap.js",
                "https://isolated.example.test/root.js",
            ]
        );
        assert!(
            fixture
                .runtime
                .eval(
                    "workletResult !== 'pending' && workletResult !== 'loaded' && \
             workletEvents.length === 1 && workletEvents[0][0] && \
             workletEvents[0][1] === 'script-src-elem' && workletEvents[0][3] === 'enforce'"
                )
                .unwrap()
                .to_boolean()
        );
    }

    #[test]
    fn owner_csp_blocks_worklet_graph_redirect_before_target_request() {
        let mut fixture =
            GraphFixture::new("script-src-elem 'self'", false, "import './redirect.js';");
        fixture.load();
        fixture.assert_no_import_request();
        assert_eq!(fixture.requests.lock().unwrap().len(), 3);
        assert!(fixture.runtime.eval(
            "workletResult !== 'pending' && workletResult !== 'loaded' && \
             JSON.stringify(workletEvents) === \
             '[[true,\"script-src-elem\",\"https://isolated.example.test/redirect.js\",\"enforce\"]]'"
        ).unwrap().to_boolean());
    }

    #[test]
    fn report_only_worklet_graph_redirect_notifies_owner_and_sends_initial_url() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self'; report-to graph",
            true,
            "import './redirect.js';",
        );
        fixture.load();
        assert!(fixture.runtime.eval(
            "workletResult === 'loaded' && \
             workletEvents.length === 2 && workletEvents.every(event => \
               JSON.stringify(event) === \
               '[true,\"script-src-elem\",\"https://isolated.example.test/redirect.js\",\"report\"]') && \
             workletReports.length === 2 && workletReports.every(report => \
               report.body.blockedURL === 'https://isolated.example.test/redirect.js')"
        ).unwrap().to_boolean());
        let reports = fixture.reports.lock().unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!(
            reports[0][0]["body"]["blockedURL"],
            "https://isolated.example.test/redirect.js"
        );
        assert_eq!(reports[0][0]["body"]["disposition"], "report");
    }

    #[test]
    fn report_only_worklet_graph_observes_intermediate_cross_origin_redirect() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self'; report-to graph",
            true,
            "import './roundtrip.js';",
        );
        fixture.load();
        assert!(fixture.runtime.eval(
            "workletResult === 'loaded' && \
             JSON.stringify(workletEvents) === \
             '[[true,\"script-src-elem\",\"https://isolated.example.test/roundtrip.js\",\"report\"]]' && \
             workletReports.length === 1",
        ).unwrap().to_boolean());
        assert_eq!(
            *fixture.requests.lock().unwrap(),
            vec![
                "https://isolated.example.test/bootstrap.js",
                "https://isolated.example.test/root.js",
                "https://isolated.example.test/roundtrip.js",
                "https://other.example.test/return.js",
                "https://isolated.example.test/import.js",
                "https://isolated.example.test/reports",
            ]
        );
        assert_eq!(fixture.reports.lock().unwrap().len(), 1);
    }

    #[test]
    fn report_only_worklet_graph_reports_a_redirect_even_when_target_returns_404() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self'; report-to graph",
            true,
            "import './missing-redirect.js';",
        );
        fixture.load();
        assert!(fixture.runtime.eval(
            "workletResult !== 'pending' && workletResult !== 'loaded' && \
             workletEvents.length === 2 && workletEvents.every(event => \
               JSON.stringify(event) === \
               '[true,\"script-src-elem\",\"https://isolated.example.test/missing-redirect.js\",\"report\"]') && \
             workletReports.length === 2",
        ).unwrap().to_boolean());
        assert_eq!(fixture.reports.lock().unwrap().len(), 2);
    }

    #[test]
    fn worker_src_does_not_block_worklet_graph_imports() {
        let mut fixture = GraphFixture::new(
            "worker-src 'none'; script-src-elem *",
            false,
            "import 'https://other.example.test/import.js';",
        );
        fixture.load();
        assert!(
            fixture
                .runtime
                .eval("workletResult === 'loaded' && workletEvents.length === 0")
                .unwrap()
                .to_boolean()
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 3);
    }

    #[test]
    fn shared_worklet_graph_import_has_one_request_and_one_report_per_stage() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self'; report-to graph",
            true,
            "import './parent-a.js'; import './parent-b.js';",
        );
        fixture.load();
        assert!(
            fixture
                .runtime
                .eval(
                    "workletResult === 'loaded' && workletEvents.length === 2 && \
             workletEvents.every(event => event[0]) && workletReports.length === 2"
                )
                .unwrap()
                .to_boolean()
        );
        assert_eq!(
            fixture
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|url| { url.as_str() == "https://other.example.test/import.js" })
                .count(),
            1
        );
        assert_eq!(fixture.reports.lock().unwrap().len(), 2);
    }

    #[test]
    fn distinct_worklet_requests_to_one_redirect_target_keep_both_owner_reports() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self'; report-to graph",
            true,
            "import './redirect-a.js'; import './redirect-b.js';",
        );
        fixture.load();
        assert!(fixture.runtime.eval(
            "workletResult === 'loaded' && workletEvents.length === 4 && \
             workletEvents.every(event => event[0] && event[3] === 'report') && \
             JSON.stringify(workletEvents.map(event => event[2]).sort()) === \
             '[\"https://isolated.example.test/redirect-a.js\",\"https://isolated.example.test/redirect-a.js\",\"https://isolated.example.test/redirect-b.js\",\"https://isolated.example.test/redirect-b.js\"]' && \
             workletReports.length === 4",
        ).unwrap().to_boolean());
        assert_eq!(fixture.reports.lock().unwrap().len(), 4);
    }

    #[test]
    fn worklet_graph_violation_retains_the_author_call_site() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self'; report-to graph",
            false,
            "import 'https://other.example.test/import.js';",
        );
        fixture
            .runtime
            .eval(
                "globalThis.sourceEvents=[]; document.addEventListener('securitypolicyviolation', \
             e => sourceEvents.push([e.sourceFile,e.lineNumber,e.columnNumber]));",
            )
            .unwrap();
        let document_id = fixture.runtime.document().identity();
        fixture
            .runtime
            .eval_script_in_document_realm(
                document_id,
                0,
                "\nCSS.paintWorklet.addModule('/root.js').catch(()=>{});",
                "https://isolated.example.test/caller.js",
                "https://isolated.example.test/caller.js",
                false,
                (11, 1),
                false,
            )
            .unwrap();
        fixture.runtime.run_until_idle().unwrap();
        security_reporting::wait_for_report_delivery(&mut fixture.runtime);
        let observed = fixture
            .runtime
            .eval("JSON.stringify(sourceEvents)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            observed,
            // The method name `addModule` starts at column 18 of this call.
            "[[\"https://isolated.example.test/caller.js\",12,18]]"
        );
        let reports = fixture.reports.lock().unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(
            reports[0][0]["body"]["sourceFile"],
            "https://isolated.example.test/caller.js"
        );
        assert_eq!(reports[0][0]["body"]["lineNumber"], 12);
        assert_eq!(reports[0][0]["body"]["columnNumber"], 18);
    }

    fn create_graph_owner_iframe(runtime: &mut JsRuntime) {
        runtime
            .eval(
                "globalThis.graphFrame=document.createElement('iframe'); \
             graphFrame.srcdoc=\"<meta http-equiv='Content-Security-Policy' \
               content=\\\"script-src-elem 'self'\\\"><body></body>\"; \
             document.body.appendChild(graphFrame);",
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
    }

    #[test]
    fn worklet_graph_uses_iframe_policy_and_notifies_only_its_document() {
        let mut fixture = GraphFixture::new(
            "script-src-elem *",
            false,
            "import 'https://other.example.test/import.js';",
        );
        create_graph_owner_iframe(&mut fixture.runtime);
        fixture
            .runtime
            .eval(
                "globalThis.topEvents=0; \
             document.addEventListener('securitypolicyviolation',()=>topEvents++); \
             graphFrame.contentWindow.eval(\"globalThis.childEvents=[]; \
               document.addEventListener('securitypolicyviolation',e=> \
                 childEvents.push([e.target===document,e.effectiveDirective,e.disposition])); \
               CSS.paintWorklet.addModule('/root.js').catch(()=>{});\");",
            )
            .unwrap();
        fixture.runtime.run_until_idle().unwrap();
        fixture.assert_no_import_request();
        assert!(
            fixture
                .runtime
                .eval(
                    "topEvents===0 && JSON.stringify(graphFrame.contentWindow.childEvents) === \
             '[[true,\"script-src-elem\",\"enforce\"]]'",
                )
                .unwrap()
                .to_boolean()
        );
    }

    #[test]
    fn discarded_iframe_cannot_start_an_import_from_its_retained_worklet_graph() {
        let mut fixture = GraphFixture::new(
            "script-src-elem *",
            false,
            "globalThis.loadLater=()=>import('https://other.example.test/import.js');",
        );
        create_graph_owner_iframe(&mut fixture.runtime);
        fixture
            .runtime
            .eval("graphFrame.contentWindow.eval(\"CSS.paintWorklet.addModule('/root.js');\");")
            .unwrap();
        fixture.runtime.run_until_idle().unwrap();
        let child = fixture
            .runtime
            .host_state
            .borrow()
            .worklet
            .runtime
            .clone()
            .unwrap();
        fixture.runtime.eval("graphFrame.remove();").unwrap();
        fixture.runtime.run_until_idle().unwrap();
        let mut child = child.borrow_mut();
        child
            .eval(
                "globalThis.laterResult='pending';loadLater().then( \
             ()=>laterResult='loaded',error=>laterResult=error.name);",
            )
            .unwrap();
        child.run_until_idle().unwrap();
        fixture.assert_no_import_request();
        assert!(
            child
                .eval("laterResult !== 'pending' && laterResult !== 'loaded'",)
                .unwrap()
                .to_boolean()
        );
    }

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
                "https://isolated.example.test/reports",
            ]
        );
        assert!(
            runtime
                .eval(
                    "workletResult === 'loaded' && \
                     workletEvents.length === 2 && workletEvents.every(event => \
                       JSON.stringify(event) === \
                       '[true,\"script-src-elem\",\"https://isolated.example.test/entry.js\",\"report\"]') && \
                     workletReports.length === 2 && workletReports.every(report => \
                       report.body.effectiveDirective === 'script-src-elem' && \
                       report.body.disposition === 'report')"
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
