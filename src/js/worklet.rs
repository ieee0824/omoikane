//! Worklets: the isolated WorkletGlobalScope shared by a page's Worklet
//! instances, module loading, registration bookkeeping and teardown.

use super::*;
use std::collections::BTreeMap;

/// Worklet state held by a page global and by the WorkletGlobalScope it owns.
pub(super) struct State {
    /// Each owning Document has its own global and module map, even when two
    /// Documents have the same origin. IDs keep ownership independent of realms.
    runtimes: BTreeMap<usize, WorkletRuntimeHandle>,
    documents: HashMap<u64, usize>,
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
    owner_document: usize,
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
        let document = self.module_fetch_context.as_ref()?.owner_document;
        let owner = self.owner.as_ref()?.borrow();
        Some(owner.document_cookie_request_context(document))
    }

    /// Whether this state belongs to an isolated WorkletGlobalScope.
    pub(super) fn is_child_global(&self) -> bool {
        self.id.is_some()
    }

    pub(super) fn has_pending_render_work(&self) -> bool {
        !self.terminated
            && self
                .runtimes
                .values()
                .any(|runtime| runtime.borrow().next_rendering() != crate::cdp::NextRendering::Idle)
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            runtimes: BTreeMap::new(),
            documents: HashMap::new(),
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
    let runtimes = {
        let mut state = state.borrow_mut();
        state.worklet.documents.clear();
        std::mem::take(&mut state.worklet.runtimes)
    };
    for runtime in runtimes.into_values() {
        terminate_runtime(runtime);
    }
}

fn terminate_runtime(runtime: WorkletRuntimeHandle) {
    let child_state = Rc::clone(&runtime.borrow().host_state);
    let mut child_state = child_state.borrow_mut();
    child_state.worklet.terminated = true;
    child_state.worklet.owner = None;
    child_state.worklet.module_fetch_context = None;
    child_state.worklet.modules.clear();
    child_state.worklet.registrations.clear();
}

impl State {
    pub(super) fn retire_document(&mut self, document: usize) {
        self.documents.retain(|_, owner| *owner != document);
        if let Some(runtime) = self.runtimes.remove(&document) {
            terminate_runtime(runtime);
        }
    }

    fn runtime_for_id(&self, id: u64, document: usize) -> Option<WorkletRuntimeHandle> {
        (self.documents.get(&id) == Some(&document))
            .then(|| self.runtimes.get(&document).cloned())
            .flatten()
    }
}

fn worklet_status(ok: bool, name: &str, message: &str, duplicate: bool) -> JsValue {
    let name = serde_json::to_string(name).unwrap_or_else(|_| "\"Error\"".to_string());
    let message = serde_json::to_string(message).unwrap_or_else(|_| "\"\"".to_string());
    js_string!(format!(
        "{{\"ok\":{ok},\"duplicate\":{duplicate},\"name\":{name},\"message\":{message}}}"
    ))
    .into()
}

/// Reads immutable native tags and own data properties without running getters
/// installed by an earlier module in the shared Worklet global.
fn worklet_javascript_error_details(error: &JsError) -> (String, String) {
    if let Some(native) = error.as_native() {
        return (native.kind.to_string(), native.message().to_owned());
    }
    let Some(object) = error.as_opaque().and_then(JsValue::as_object) else {
        return ("Error".into(), "Worklet module preparation failed".into());
    };
    use boa_engine::builtins::error::{Error, ErrorKind};
    let Some(data) = object.downcast_ref::<Error>() else {
        return ("Error".into(), "Worklet module preparation failed".into());
    };
    let name = [
        (ErrorKind::Syntax, "SyntaxError"),
        (ErrorKind::Type, "TypeError"),
        (ErrorKind::Reference, "ReferenceError"),
        (ErrorKind::Range, "RangeError"),
        (ErrorKind::Eval, "EvalError"),
        (ErrorKind::Uri, "URIError"),
    ]
    .into_iter()
    .find(|(kind, _)| *data == Error::new(*kind))
    .map_or("Error", |(_, name)| name);
    let message = object
        .borrow()
        .properties()
        .get(&js_string!("message").into())
        .and_then(|property| property.value().and_then(JsValue::as_string))
        .map_or_else(String::new, |value| value.to_std_string_escaped());
    (name.to_owned(), message)
}

fn worklet_javascript_error_status(error: JsError) -> JsValue {
    let (name, message) = worklet_javascript_error_details(&error);
    js_string!(
        serde_json::json!({
            "ok": false, "duplicate": false, "javascript": true,
            "name": name, "message": message,
        })
        .to_string()
    )
    .into()
}

/// Resolves with the owner's base URL before decoding or fetching any payload.
fn resolve_worklet_module_url(
    requested: &str,
    base: Option<&crate::http::Url>,
) -> Result<url::Url, url::ParseError> {
    match base {
        Some(base) => url::Url::parse(&base.to_string())?.join(requested),
        None => url::Url::parse(requested),
    }
}

fn decode_worklet_module_source(bytes: &[u8], mime: &str) -> Result<String, WorkletFetchError> {
    let essence = mime.split(';').next().unwrap_or("");
    if !is_javascript_mime_type(essence) {
        return Err(WorkletFetchError::Network);
    }
    // HTML modules use UTF-8, independent of Content-Type's charset parameter.
    let source = String::from_utf8_lossy(bytes);
    Ok(source
        .strip_prefix('\u{feff}')
        .unwrap_or(&source)
        .to_owned())
}

fn check_worklet_module_response(
    policy: &CspPolicy,
    checks: &mut Vec<WorkletPolicyCheck>,
    reference: &str,
    redirects: usize,
    initial: &str,
) -> Result<(), WorkletFetchError> {
    checks.push(WorkletPolicyCheck {
        reference: reference.to_owned(),
        redirects,
        initial_url: initial.to_owned(),
    });
    if policy.allows_reference_after_redirects(ResourceType::Script, reference, redirects) {
        Ok(())
    } else {
        Err(WorkletFetchError::Policy)
    }
}

fn fetch_worklet_data_module(
    initial: &url::Url,
    policy: &CspPolicy,
    checks: &mut Vec<WorkletPolicyCheck>,
) -> Result<(String, String), WorkletFetchError> {
    let mut payload_url = initial.clone();
    payload_url.set_fragment(None);
    let data =
        crate::http::parse_data_uri(payload_url.as_str()).ok_or(WorkletFetchError::Network)?;
    check_worklet_module_response(policy, checks, initial.as_str(), 0, initial.as_str())?;
    let source = decode_worklet_module_source(&data.data, &data.mime_type)?;
    Ok((initial.to_string(), source))
}

/// Fetches with module-script CORS credentials and pre-I/O redirect checks.
fn fetch_worklet_http_module(
    state: &mut HostState,
    initial: &url::Url,
    base: Option<&crate::http::Url>,
    cookie_context: &crate::http::CookieRequestContext,
    origin: &CorsOrigin,
    credentials: CredentialsMode,
    policy: &CspPolicy,
    checks: &mut Vec<WorkletPolicyCheck>,
) -> Result<(String, String), WorkletFetchError> {
    let request_url = initial
        .as_str()
        .parse::<crate::http::Url>()
        .map_err(|_| WorkletFetchError::Network)?;
    let mut request = HttpRequest::new(Method::Get, request_url.clone());
    if requires_public_fetch(&request_url, base) {
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
            policy.allows_reference_after_redirects(ResourceType::Script, &reference, redirects)
        };
        crate::http::cors::fetch_with_timeout_restricting_redirects_checked(
            &mut state.http_client,
            request,
            origin,
            RequestMode::Cors,
            credentials,
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
    let effective = if response.redirect_count() == 0 {
        initial.to_string()
    } else {
        response
            .effective_url()
            .map_or_else(|| request_url.to_string(), ToString::to_string)
    };
    check_worklet_module_response(
        policy,
        checks,
        &effective,
        response.redirect_count(),
        initial.as_str(),
    )?;
    if !(200..=299).contains(&response.status_code()) {
        state.http_client.report_resource_failure();
        return Err(WorkletFetchError::Network);
    }
    let source = decode_worklet_module_source(
        response.body(),
        response.header("content-type").unwrap_or(""),
    )
    .inspect_err(|_| state.http_client.report_resource_failure())?;
    Ok((effective, source))
}

fn fetch_worklet_module_source(
    state: &mut HostState,
    requested: &str,
    base: Option<&crate::http::Url>,
    cookie_context: &crate::http::CookieRequestContext,
    origin: &CorsOrigin,
    credentials: CredentialsMode,
    policy: &CspPolicy,
    checks: &mut Vec<WorkletPolicyCheck>,
) -> Result<(String, String), WorkletFetchError> {
    let initial = url::Url::parse(requested).map_err(|_| WorkletFetchError::Network)?;
    match initial.scheme() {
        "data" => fetch_worklet_data_module(&initial, policy, checks).inspect_err(|error| {
            if matches!(error, WorkletFetchError::Network) {
                state.http_client.report_resource_failure();
            }
        }),
        "http" | "https" => fetch_worklet_http_module(
            state,
            &initial,
            base,
            cookie_context,
            origin,
            credentials,
            policy,
            checks,
        ),
        _ => Err(WorkletFetchError::Network),
    }
}

fn create_worklet_native(_: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = with_host_state(|state| {
        let mut state = state.borrow_mut();
        let document = context
            .realm()
            .host_defined()
            .get::<ModuleDocumentId>()
            .map(|owner| owner.0)
            .unwrap_or_else(|| state.document.identity());
        let id = state.worklet.next_id;
        state.worklet.documents.insert(id, document);
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
        state.http_client = owner.http_client.fork_for_worker();
        state.worklet.module_fetch_context = Some(ModuleFetchContext {
            cookies: Arc::clone(&owner.cookie_store),
            owner_document: config.owner_document_id,
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
        state
            .worklet
            .runtimes
            .insert(config.owner_document_id, Rc::clone(&runtime));
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

fn load_worklet_dependencies(runtime: &mut JsRuntime, module: &Module) -> Result<(), JsValue> {
    let promise = runtime.with_active_host_value(|context| module.load(context));
    // The host, rather than author code, owns this intermediate promise.
    promise.mark_handled();
    runtime
        .run_jobs()
        .map_err(|error| worklet_status(false, "OperationError", &error.to_string(), false))?;
    match promise.state() {
        PromiseState::Fulfilled(_) => Ok(()),
        PromiseState::Rejected(value) => {
            let fetch_failed = value.as_object().is_some_and(|object| {
                runtime
                    .module_loader
                    .worklet_fetch_errors
                    .borrow()
                    .contains_key(&object)
            });
            if fetch_failed {
                Err(worklet_status(
                    false,
                    "AbortError",
                    "Failed to fetch Worklet module graph",
                    false,
                ))
            } else {
                Err(worklet_javascript_error_status(JsError::from_opaque(value)))
            }
        }
        PromiseState::Pending => Err(worklet_status(
            false,
            "OperationError",
            "Worklet module loading remained pending",
            false,
        )),
    }
}

fn report_worklet_evaluation_error(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let error = JsError::from_opaque(args.first().cloned().unwrap_or_else(JsValue::undefined));
    script_errors::report_exception(context, &error)?;
    Ok(JsValue::undefined())
}

fn evaluate_worklet_module(
    runtime_handle: &WorkletRuntimeHandle,
    source: &str,
    effective_url: &str,
    resolved_url: String,
    graph: ModuleGraphContext,
) -> JsValue {
    let mut runtime = runtime_handle.borrow_mut();
    let document = runtime.document();
    runtime
        .module_loader
        .set_context_for_module_graph(document.identity(), effective_url, graph);
    let module = match Module::parse_with_host_defined(
        Source::from_reader(source.as_bytes(), Some(Path::new(effective_url))),
        None,
        security_reporting::module_csp_metadata(&resolved_url),
        &mut runtime.context,
    ) {
        Ok(module) => module,
        Err(error) => return worklet_javascript_error_status(error),
    };
    let _document = activate_module_document(&runtime.host_state, document.identity());
    if let Err(status) = load_worklet_dependencies(&mut runtime, &module) {
        return status;
    }
    if let Err(error) = runtime.with_active_host(|context| module.link(context)) {
        return worklet_javascript_error_status(error);
    }
    runtime.with_active_host_value(|context| {
        let evaluation = module.evaluate(context);
        let report = NativeFunction::from_fn_ptr(report_worklet_evaluation_error)
            .to_js_function(context.realm());
        evaluation.catch(report, context).mark_handled();
    });
    // HTML resolves addModule after starting evaluation. Runtime exceptions
    // are reported in the Worklet global, including later async rejections.
    if let Err(error) = runtime.run_jobs() {
        return worklet_status(false, "OperationError", &error.to_string(), false);
    }
    runtime.host_state.borrow_mut().pending_javascript_dialog = None;
    runtime
        .host_state
        .borrow_mut()
        .worklet
        .modules
        .insert(resolved_url);
    worklet_status(true, "", "", false)
}

struct ModuleRequestOwner {
    config: WorkletRuntimeConfig,
    base_url: Option<crate::http::Url>,
    secure: bool,
    cookie_context: crate::http::CookieRequestContext,
    origin: CorsOrigin,
}

fn module_request_owner(
    state: &HostState,
    context: &Context,
    id: u64,
) -> JsResult<ModuleRequestOwner> {
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
    Ok(ModuleRequestOwner {
        config: WorkletRuntimeConfig {
            owner_url,
            storage: state.storage_manager.clone(),
            session_id: state.storage_session_id,
            user_agent: state.navigator_user_agent.clone(),
            owner_document_id: document_id,
            effective_url: String::new(),
            id,
        },
        base_url,
        secure: document_is_secure_context(state, document_id),
        cookie_context: state.document_cookie_request_context(document_id),
        origin: script_errors::document_fetch_origin(state, document_id),
    })
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
    let credentials = match string_argument(args.get(2), "same-origin", context)?.as_str() {
        "omit" => CredentialsMode::Omit,
        "include" => CredentialsMode::Include,
        "same-origin" => CredentialsMode::SameOrigin,
        _ => {
            return Err(JsNativeError::typ()
                .with_message("Unsupported Worklet credentials mode")
                .into());
        }
    };
    let author_source = security_reporting::capture_csp_source(context);
    with_host_state(|owner_state| {
        let ModuleRequestOwner {
            config,
            base_url,
            secure,
            cookie_context,
            origin,
        } = module_request_owner(&owner_state.borrow(), context, id)?;
        let owner_document_id = config.owner_document_id;
        if !secure {
            return Ok(worklet_status(
                false,
                "NotAllowedError",
                "Worklet modules require a secure context",
                false,
            ));
        }
        if owner_state.borrow().worklet.documents.get(&id) != Some(&owner_document_id) {
            return Ok(worklet_status(
                false,
                "InvalidStateError",
                "Worklet owner Document is no longer active",
                false,
            ));
        }
        let resolved_url = match resolve_worklet_module_url(&requested_url, base_url.as_ref()) {
            Ok(url) => url.to_string(),
            Err(_) => {
                return Ok(worklet_status(
                    false,
                    "SyntaxError",
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
        // do not refetch a resource whose graph was already prepared.
        // This also ensures a repeated `addModule()` call cannot observe a
        // changing network response after the first addModule completion.
        if let Some(runtime) = owner_state
            .borrow()
            .worklet
            .runtimes
            .get(&owner_document_id)
            .cloned()
        {
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
                &resolved_url,
                base_url.as_ref(),
                &cookie_context,
                &origin,
                credentials,
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
                    "AbortError",
                    &format!("failed to fetch Worklet module: {requested_url}"),
                    false,
                ));
            }
        };
        let runtime_handle = if let Some(runtime) = owner_state
            .borrow()
            .worklet
            .runtimes
            .get(&owner_document_id)
            .cloned()
        {
            runtime
        } else {
            match create_worklet_runtime(
                &owner_state,
                WorkletRuntimeConfig {
                    effective_url: effective_url.clone(),
                    ..config
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
                credentials,
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
        let document = owner_state
            .borrow()
            .csp_document_for_context(context)?
            .identity();
        let runtime = owner_state.borrow().worklet.runtime_for_id(id, document);
        let Some(runtime) = runtime else {
            return Ok(js_string!("[]").into());
        };
        let runtime_ref = runtime.borrow();
        let state = runtime_ref.host_state.borrow();
        if state.worklet.terminated {
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
        let document = owner_state
            .borrow()
            .csp_document_for_context(context)?
            .identity();
        let runtime = owner_state.borrow().worklet.runtime_for_id(id, document);
        let Some(runtime) = runtime else {
            return Ok(JsValue::from(0));
        };
        let runtime_ref = runtime.borrow();
        let state = runtime_ref.host_state.borrow();
        if state.worklet.terminated {
            return Ok(JsValue::from(0));
        }
        Ok(JsValue::from(state.worklet.modules.len() as u32))
    })
}

fn worklet_process_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = worker_id_argument(args, context)?;
    let request = string_argument(args.get(1), "{}", context)?;
    with_host_state(|owner_state| {
        let document = owner_state
            .borrow()
            .csp_document_for_context(context)?
            .identity();
        let runtime = owner_state.borrow().worklet.runtime_for_id(id, document);
        let Some(runtime) = runtime else {
            return Ok(js_string!(r#"{"ok":false}"#).into());
        };
        let expression = format!(
            "__omoikane_audio_process({})",
            serde_json::to_string(&request).unwrap_or_else(|_| "\"{}\"".to_string())
        );
        let mut runtime = runtime.borrow_mut();
        let value = runtime.eval(&expression).map_err(|error| {
            JsNativeError::error().with_message(format!("AudioWorklet processor failed: {error}"))
        })?;
        let encoded = value
            .to_string(&mut runtime.context)?
            .to_std_string_escaped();
        Ok(js_string!(encoded).into())
    })
}

fn worklet_teardown_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = worker_id_argument(args, context)?;
    with_host_state(|owner_state| {
        let mut state = owner_state.borrow_mut();
        let document = state.csp_document_for_context(context)?.identity();
        if state.worklet.documents.get(&id) != Some(&document) {
            return Ok(JsValue::from(false));
        }
        state.worklet.retire_document(document);
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
            3,
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
            js_string!("__omoikane_worklet_process"),
            2,
            NativeFunction::from_copy_closure(worklet_process_native),
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
        let runtimes: Vec<_> = self
            .host_state
            .borrow()
            .worklet
            .runtimes
            .values()
            .cloned()
            .collect();
        for runtime in runtimes {
            let runtime = runtime.borrow_mut();
            let mut state = runtime.host_state.borrow_mut();
            if !state.worklet.terminated {
                state.event_loop.advance(elapsed_ms);
            }
        }
    }

    /// Pumps each Document's isolated Worklet global with the page clock.
    pub(super) fn run_worklet_background_tasks(&mut self) {
        if self.host_state.borrow().worklet.is_child_global() {
            return;
        }
        let runtimes: Vec<_> = self
            .host_state
            .borrow()
            .worklet
            .runtimes
            .iter()
            .map(|(document, runtime)| (*document, Rc::clone(runtime)))
            .collect();
        for (document, runtime) in runtimes {
            let (result, errors, terminated) = {
                let mut runtime = runtime.borrow_mut();
                if runtime.host_state.borrow().worklet.terminated {
                    continue;
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
                self.host_state
                    .borrow_mut()
                    .worklet
                    .retire_document(document);
            }
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
                vec![
                    ("Access-Control-Allow-Origin".into(), "*".into()),
                    ("Content-Type".into(), "text/javascript".into()),
                ],
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
                   () => workletResult = 'loaded', error => { workletResult = error.name; globalThis.workletErrorIsDom=error instanceof DOMException; globalThis.workletErrorIsNative=error instanceof Error; });",
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
             globalThis.verifiedDataExport=value;",
        );
        fixture.load();
        let child = fixture
            .runtime
            .host_state
            .borrow()
            .worklet
            .single_test_runtime()
            .unwrap();
        assert!(
            child
                .borrow_mut()
                .eval("verifiedDataExport===42")
                .unwrap()
                .to_boolean()
        );
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
             globalThis.verifiedDataExport=value;",
        );
        fixture.load();
        let child = fixture
            .runtime
            .host_state
            .borrow()
            .worklet
            .single_test_runtime()
            .unwrap();
        assert!(
            child
                .borrow_mut()
                .eval("verifiedDataExport===42")
                .unwrap()
                .to_boolean()
        );
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
             globalThis.verifiedDataExport=value;",
        );
        fixture.load();
        let child = fixture
            .runtime
            .host_state
            .borrow()
            .worklet
            .single_test_runtime()
            .unwrap();
        assert!(
            child
                .borrow_mut()
                .eval("verifiedDataExport===42")
                .unwrap()
                .to_boolean()
        );
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
             globalThis.verifiedDataIdentity=a.id===b.id && a.id!==c.id && dataRuns===2 && \
                a.url.endsWith('#one') && c.url.endsWith('#two');"
        );
        let mut fixture = GraphFixture::new("script-src-elem 'self' data:", false, &source);
        fixture.load();
        let child = fixture
            .runtime
            .host_state
            .borrow()
            .worklet
            .single_test_runtime()
            .unwrap();
        assert!(
            child
                .borrow_mut()
                .eval("verifiedDataIdentity===true")
                .unwrap()
                .to_boolean()
        );
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
            .single_test_runtime()
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
    fn worklet_graph_same_scheme_relative_import_uses_the_owner_origin() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self'",
            false,
            "import {imported} from 'https:dir/../import.js';globalThis.ownerImportValue=imported;",
        );
        fixture.load();
        assert!(
            fixture
                .runtime
                .eval("workletResult==='loaded' && workletEvents.length===0")
                .unwrap()
                .to_boolean()
        );
        let child = fixture
            .runtime
            .host_state
            .borrow()
            .worklet
            .single_test_runtime()
            .unwrap();
        assert!(
            child
                .borrow_mut()
                .eval("ownerImportValue===true")
                .unwrap()
                .to_boolean()
        );
        assert_eq!(
            *fixture.requests.lock().unwrap(),
            vec![
                "https://isolated.example.test/bootstrap.js",
                "https://isolated.example.test/root.js",
                "https://isolated.example.test/import.js"
            ]
        );
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
    fn worklet_graph_loading_failures_reject_with_abort_error() {
        for (policy, source) in [
            (
                "script-src-elem 'self'",
                "import 'https://other.example.test/import.js';",
            ),
            ("script-src-elem 'self'", "import './missing.js';"),
            (
                "script-src-elem 'self' data:",
                "import 'data:text/plain,export%20const%20value%3D42';",
            ),
        ] {
            let mut fixture = GraphFixture::new(policy, false, source);
            fixture.load();
            let actual = fixture
                .runtime
                .eval("workletResult")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped();
            assert_eq!(actual, "AbortError", "module: {source}");
        }
    }

    #[test]
    fn worklet_graph_parse_link_and_resolution_errors_keep_native_kind() {
        for (source, expected) in [
            ("import 'data:text/javascript,export%20%3D';", "SyntaxError"),
            ("const =", "SyntaxError"),
            (
                "import {missing} from 'data:text/javascript,export%20const%20value%3D42';",
                "SyntaxError",
            ),
            (
                "import 'data:text/javascript,import%20%27./import.js%27;';",
                "TypeError",
            ),
        ] {
            let mut fixture = GraphFixture::new("script-src-elem 'self' data:", false, source);
            fixture.load();
            let actual = fixture
                .runtime
                .eval("workletResult")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped();
            assert_eq!(actual, expected, "module: {source}");
            assert!(
                fixture
                    .runtime
                    .eval("!workletErrorIsDom && workletErrorIsNative")
                    .unwrap()
                    .to_boolean()
            );
        }
    }

    #[test]
    fn worklet_root_http_and_cors_failures_reject_with_abort_error() {
        for url in ["/missing.js", "https://other.example.test/missing.js"] {
            let mut fixture = GraphFixture::new("script-src-elem *", false, "");
            fixture
                .runtime
                .eval(&format!(
                    "globalThis.rootResult='pending';CSS.paintWorklet.addModule({}).then( \
                 ()=>rootResult='loaded',error=>rootResult=error.name);",
                    serde_json::to_string(url).unwrap(),
                ))
                .unwrap();
            fixture.runtime.run_until_idle().unwrap();
            let actual = fixture
                .runtime
                .eval("rootResult")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped();
            assert_eq!(actual, "AbortError", "module: {url}");
            assert!(
                fixture
                    .requests
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|request| request.ends_with(url.trim_start_matches('/')))
            );
        }
    }

    #[test]
    fn worklet_runtime_exceptions_are_reported_without_rejecting_add_module() {
        for source in [
            "throw new TypeError('author failure');",
            "throw new SyntaxError('author failure');",
            "throw {name:'AbortError',message:'author failure'};",
            "throw 'author failure';",
            "await Promise.reject(new TypeError('author failure'));",
        ] {
            let mut fixture = GraphFixture::new("script-src-elem 'self'", false, source);
            let child = fixture
                .runtime
                .host_state
                .borrow()
                .worklet
                .single_test_runtime()
                .unwrap();
            child.borrow_mut().eval("globalThis.runtimeErrors=[];globalThis.runtimeRejections=[];addEventListener('error',e=>{runtimeErrors.push(e.message);e.preventDefault()});addEventListener('unhandledrejection',e=>runtimeRejections.push(e.reason));").unwrap();
            fixture.load();
            assert!(
                fixture
                    .runtime
                    .eval("workletResult==='loaded'")
                    .unwrap()
                    .to_boolean(),
                "{source}"
            );
            assert!(child.borrow_mut().eval("runtimeErrors.length===1 && runtimeErrors[0]==='author failure' && runtimeRejections.length===0").unwrap().to_boolean(), "{source}");
        }
    }

    #[test]
    fn worklet_pending_evaluation_resolves_add_module_and_reports_later_rejection() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self'",
            false,
            "await new Promise((_, reject)=>{globalThis.rejectEvaluation=reject});",
        );
        fixture.load();
        assert!(
            fixture
                .runtime
                .eval("workletResult==='loaded'")
                .unwrap()
                .to_boolean()
        );
        let child = fixture
            .runtime
            .host_state
            .borrow()
            .worklet
            .single_test_runtime()
            .unwrap();
        child.borrow_mut().eval("globalThis.runtimeErrors=[];addEventListener('error',e=>{runtimeErrors.push(e.message);e.preventDefault()});rejectEvaluation(new Error('late failure'));").unwrap();
        fixture.runtime.run_until_idle().unwrap();
        assert!(
            child
                .borrow_mut()
                .eval("runtimeErrors.join(',')==='late failure'")
                .unwrap()
                .to_boolean()
        );
    }

    #[test]
    fn worklet_cached_fetch_failure_remains_abort_error_after_collection() {
        let mut fixture =
            GraphFixture::new("script-src-elem 'self'", false, "import './missing.js';");
        fixture.load();
        boa_gc::force_collect();
        fixture.runtime.eval("globalThis.retry='pending';CSS.paintWorklet.addModule('/root.js').catch(e=>retry=e.name);").unwrap();
        fixture.runtime.run_until_idle().unwrap();
        assert!(
            fixture
                .runtime
                .eval("retry==='AbortError'")
                .unwrap()
                .to_boolean()
        );
    }

    #[test]
    fn worklet_preparation_errors_ignore_mutated_error_prototypes_and_constructors() {
        let mut fixture = GraphFixture::new(
            "script-src-elem 'self' data:",
            false,
            "import 'data:text/javascript,export%20%3D';",
        );
        let child = fixture
            .runtime
            .host_state
            .borrow()
            .worklet
            .single_test_runtime()
            .unwrap();
        child.borrow_mut().eval("globalThis.errorGetterCalls=0;for(const key of ['cause','constructor'])Object.defineProperty(Error.prototype,key,{get(){errorGetterCalls++;throw new Error('must not read getter')}});").unwrap();
        fixture.runtime.eval("globalThis.OriginalSyntaxError=SyntaxError;globalThis.SyntaxError=function(){throw new Error('must not call author constructor')};").unwrap();
        fixture.load();
        assert!(
            fixture
                .runtime
                .eval("workletResult==='SyntaxError' && workletErrorIsNative && !workletErrorIsDom")
                .unwrap()
                .to_boolean()
        );
        assert!(
            child
                .borrow_mut()
                .eval("errorGetterCalls===0")
                .unwrap()
                .to_boolean()
        );
    }

    #[test]
    fn worklet_fetch_failure_tags_are_released_with_the_document() {
        let mut fixture =
            GraphFixture::new("script-src-elem 'self'", false, "import './missing.js';");
        fixture.load();
        let child = fixture
            .runtime
            .host_state
            .borrow()
            .worklet
            .single_test_runtime()
            .unwrap();
        let child = child.borrow();
        let document_id = child.document().identity();
        assert!(!child.module_loader.worklet_fetch_errors.borrow().is_empty());
        child
            .module_loader
            .clear_csp_context_for_document(document_id);
        assert!(child.module_loader.worklet_fetch_errors.borrow().is_empty());
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
        let child = {
            let state = fixture.runtime.host_state.borrow();
            assert_eq!(state.iframe_documents.len(), 1);
            let document = state
                .iframe_documents
                .values()
                .next()
                .unwrap()
                .document
                .identity();
            state.worklet.runtimes.get(&document).cloned().unwrap()
        };
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

    struct RootModuleTransport {
        status: u16,
        mime: Option<&'static str>,
        body: Vec<u8>,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl HttpTransport for RootModuleTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            self.requests
                .lock()
                .unwrap()
                .push(request.url().to_string());
            let headers = self
                .mime
                .map(|mime| vec![("Content-Type".into(), mime.into())])
                .unwrap_or_default();
            Ok(HttpResponse::new(
                self.status,
                "Fixture",
                headers,
                self.body.clone(),
            ))
        }
    }

    fn root_module_fixture(
        status: u16,
        mime: Option<&'static str>,
        body: &[u8],
    ) -> (JsRuntime, Arc<Mutex<Vec<String>>>) {
        let mut runtime = JsRuntime::with_document_and_url(
            blank_html_document(),
            "https://isolated.example.test/page",
        )
        .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        runtime.set_http_resource_transport(Some(Arc::new(RootModuleTransport {
            status,
            mime,
            body: body.to_vec(),
            requests: Arc::clone(&requests),
        })));
        (runtime, requests)
    }

    fn load_root_module(runtime: &mut JsRuntime, url: &str) {
        runtime.eval(&format!("globalThis.rootResult='pending';CSS.paintWorklet.addModule({}).then(()=>rootResult='loaded',e=>{{rootResult=e.name;globalThis.rootErrorIsDom=e instanceof DOMException;}});", serde_json::to_string(url).unwrap())).unwrap();
        runtime.run_until_idle().unwrap();
    }

    #[test]
    fn worklet_root_url_syntax_is_distinct_from_data_decode_and_scheme_failure() {
        for (url, expected) in [
            ("http://[", "SyntaxError"),
            ("data:text/javascript;base64,%%%", "AbortError"),
            ("about:blank", "AbortError"),
            ("file:///module.js", "AbortError"),
        ] {
            let (mut runtime, requests) = root_module_fixture(
                200,
                Some("text/javascript"),
                b"registerPaint('unexpected',class{});",
            );
            load_root_module(&mut runtime, url);
            assert_eq!(
                runtime
                    .eval("rootResult")
                    .unwrap()
                    .as_string()
                    .unwrap()
                    .to_std_string_escaped(),
                expected,
                "{url}"
            );
            assert!(
                runtime
                    .eval("rootErrorIsDom && CSS.paintWorklet.moduleCount===0")
                    .unwrap()
                    .to_boolean()
            );
            assert!(requests.lock().unwrap().is_empty(), "{url}");
        }
    }

    #[test]
    fn worklet_root_http_requires_a_javascript_mime_type() {
        for mime in [
            None,
            Some("text/plain"),
            Some("application/json"),
            Some("text/javascript, text/plain"),
        ] {
            let (mut runtime, _) =
                root_module_fixture(200, mime, b"registerPaint('unexpected',class{});");
            load_root_module(&mut runtime, "/module.js");
            assert!(runtime.eval("rootResult==='AbortError' && rootErrorIsDom && CSS.paintWorklet.moduleCount===0").unwrap().to_boolean(), "{mime:?}");
            assert!(runtime.host_state.borrow().worklet.runtimes.is_empty());
        }
    }

    #[test]
    fn worklet_root_accepts_ok_statuses_and_mime_parameters() {
        for status in [200, 201, 204, 299] {
            let (mut runtime, _) =
                root_module_fixture(status, Some("Text/JavaScript; charset=windows-1252"), b"");
            load_root_module(&mut runtime, "/module.js");
            assert!(
                runtime
                    .eval("rootResult==='loaded' && CSS.paintWorklet.moduleCount===1")
                    .unwrap()
                    .to_boolean(),
                "{status}"
            );
        }
    }

    #[test]
    fn worklet_root_decodes_utf8_with_replacement() {
        let (mut runtime, _) = root_module_fixture(
            200,
            Some("text/javascript; charset=windows-1252"),
            b"globalThis.decoded='\xff';registerPaint('decoded',class{});",
        );
        load_root_module(&mut runtime, "/module.js");
        assert!(runtime.eval("rootResult==='loaded' && CSS.paintWorklet.registeredNames.join(',')==='decoded'").unwrap().to_boolean());
        let child = runtime
            .host_state
            .borrow()
            .worklet
            .runtimes
            .get(&runtime.document().identity())
            .cloned()
            .unwrap();
        assert_eq!(
            child
                .borrow_mut()
                .eval("decoded")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "\u{fffd}"
        );
    }

    #[test]
    fn worklet_root_data_decodes_invalid_utf8_with_replacement() {
        let (mut runtime, requests) = root_module_fixture(200, Some("text/javascript"), b"");
        load_root_module(
            &mut runtime,
            "data:text/javascript,globalThis.decoded%3D%27%FF%27;registerPaint('decoded',class{});",
        );
        assert!(runtime.eval("rootResult==='loaded' && CSS.paintWorklet.registeredNames.join(',')==='decoded'").unwrap().to_boolean());
        let child = runtime
            .host_state
            .borrow()
            .worklet
            .runtimes
            .get(&runtime.document().identity())
            .cloned()
            .unwrap();
        assert_eq!(
            child
                .borrow_mut()
                .eval("decoded")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "\u{fffd}"
        );
        assert!(requests.lock().unwrap().is_empty());
    }

    #[test]
    fn worklet_root_utf8_bom_does_not_remove_bom_inside_the_script() {
        let (mut runtime, _) = root_module_fixture(
            200,
            Some("application/javascript"),
            b"\xef\xbb\xbfglobalThis.bomText='\xef\xbb\xbf';registerPaint('bom',class{});",
        );
        load_root_module(&mut runtime, "/module.js");
        assert!(
            runtime
                .eval("rootResult==='loaded' && CSS.paintWorklet.registeredNames.join(',')==='bom'")
                .unwrap()
                .to_boolean()
        );
        let child = runtime
            .host_state
            .borrow()
            .worklet
            .runtimes
            .get(&runtime.document().identity())
            .cloned()
            .unwrap();
        assert!(
            child
                .borrow_mut()
                .eval("bomText.length===1 && bomText.charCodeAt(0)===65279")
                .unwrap()
                .to_boolean()
        );
    }

    #[test]
    fn worklet_root_standard_resolution_preserves_fragment_identity() {
        let source = b"globalThis.rootRuns=(globalThis.rootRuns||0)+1;globalThis.rootUrls=globalThis.rootUrls||[];rootUrls.push(import.meta.url);registerPaint('root-'+rootRuns,class{});";
        let (mut runtime, requests) = root_module_fixture(200, Some("text/javascript"), source);
        for url in [
            "https:dir/../module.js?version=1#one",
            "./module.js?version=1#one",
            "./module.js?version=1#two",
        ] {
            load_root_module(&mut runtime, url);
        }
        assert!(runtime.eval("rootResult==='loaded' && CSS.paintWorklet.moduleCount===2 && CSS.paintWorklet.registeredNames.join(',')==='root-1,root-2'").unwrap().to_boolean());
        assert_eq!(
            *requests.lock().unwrap(),
            vec!["https://isolated.example.test/module.js?version=1"; 2]
        );
        let child = runtime
            .host_state
            .borrow()
            .worklet
            .runtimes
            .get(&runtime.document().identity())
            .cloned()
            .unwrap();
        assert!(child.borrow_mut().eval("JSON.stringify(rootUrls)===JSON.stringify(['https://isolated.example.test/module.js?version=1#one','https://isolated.example.test/module.js?version=1#two'])").unwrap().to_boolean());
    }

    #[test]
    fn worklet_root_data_fragment_is_excluded_from_source_and_retained_in_identity() {
        let (mut runtime, requests) = root_module_fixture(200, Some("text/javascript"), b"");
        let data = "data:text/javascript,globalThis.rootRuns=(globalThis.rootRuns||0)+1;globalThis.rootUrls=globalThis.rootUrls||[];rootUrls.push(import.meta.url);registerPaint('data-'+rootRuns,class{});";
        for fragment in ["one", "one", "two"] {
            load_root_module(&mut runtime, &format!("{data}#{fragment}"));
        }
        assert!(runtime.eval("rootResult==='loaded' && CSS.paintWorklet.moduleCount===2 && CSS.paintWorklet.registeredNames.join(',')==='data-1,data-2'").unwrap().to_boolean());
        assert!(requests.lock().unwrap().is_empty());
        let child = runtime
            .host_state
            .borrow()
            .worklet
            .runtimes
            .get(&runtime.document().identity())
            .cloned()
            .unwrap();
        assert!(child.borrow_mut().eval("rootUrls.length===2 && rootUrls[0].endsWith('#one') && rootUrls[1].endsWith('#two')").unwrap().to_boolean());
    }

    #[test]
    fn worklet_root_decodes_unpadded_base64_data() {
        let (mut runtime, requests) = root_module_fixture(200, Some("text/javascript"), b"");
        load_root_module(
            &mut runtime,
            "data:text/javascript;base64,Z2xvYmFsVGhpcy51bnBhZGRlZD00Mjs",
        );
        assert!(runtime.eval("rootResult==='loaded'").unwrap().to_boolean());
        let child = runtime
            .host_state
            .borrow()
            .worklet
            .runtimes
            .get(&runtime.document().identity())
            .cloned()
            .unwrap();
        assert!(
            child
                .borrow_mut()
                .eval("unpadded===42")
                .unwrap()
                .to_boolean()
        );
        assert!(requests.lock().unwrap().is_empty());
    }

    #[test]
    fn worklet_root_data_report_only_checks_both_fetch_stages() {
        let (mut runtime, requests) = root_module_fixture(200, Some("text/javascript"), b"");
        runtime.install_document_response_headers(&[(
            "Content-Security-Policy-Report-Only".into(),
            "script-src-elem 'self'; report-to observations".into(),
        )]);
        runtime.eval("globalThis.rootEvents=[];globalThis.rootReports=[];document.addEventListener('securitypolicyviolation',e=>rootEvents.push([e.target===document,e.blockedURI,e.disposition]));new ReportingObserver(batch=>rootReports.push(...batch)).observe();").unwrap();
        load_root_module(
            &mut runtime,
            "data:text/javascript,registerPaint('data',class{});",
        );
        security_reporting::wait_for_report_delivery(&mut runtime);
        assert!(runtime.eval("rootResult==='loaded' && CSS.paintWorklet.registeredNames.join(',')==='data' && JSON.stringify(rootEvents)==='[[true,\"data\",\"report\"],[true,\"data\",\"report\"]]' && rootReports.length===2 && rootReports.every(r=>r.body.blockedURL==='data')").unwrap().to_boolean());
        assert!(requests.lock().unwrap().is_empty());
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
                   () => workletResult = 'loaded', error => { workletResult = error.name; globalThis.workletErrorIsDom=error instanceof DOMException; globalThis.workletErrorIsNative=error instanceof Error; });",
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
                        vec![
                            ("Content-Type".into(), "text/javascript".into()),
                            (
                                "Access-Control-Allow-Origin".into(),
                                "https://isolated.example.test".into(),
                            ),
                        ],
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
                   () => workletResult = 'loaded', error => { workletResult = error.name; globalThis.workletErrorIsDom=error instanceof DOMException; globalThis.workletErrorIsNative=error instanceof Error; });",
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
                        vec![
                            ("Content-Type".into(), "text/javascript".into()),
                            (
                                "Access-Control-Allow-Origin".into(),
                                "https://isolated.example.test".into(),
                            ),
                        ],
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
                   () => workletResult = 'loaded', error => { workletResult = error.name; globalThis.workletErrorIsDom=error instanceof DOMException; globalThis.workletErrorIsNative=error instanceof Error; });",
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
                        vec![
                            ("Content-Type".into(), "text/javascript".into()),
                            (
                                "Access-Control-Allow-Origin".into(),
                                "https://isolated.example.test".into(),
                            ),
                        ],
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
        let worklet = runtime
            .host_state
            .borrow()
            .worklet
            .runtimes
            .get(&runtime.document().identity())
            .cloned()
            .unwrap();
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
            .document_cookie_request_context(runtime.document().identity());
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
                    vec![
                        ("Content-Type".into(), "text/javascript".into()),
                        (
                            "Access-Control-Allow-Origin".into(),
                            "https://isolated.example.test".into(),
                        ),
                    ],
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
        let worklet = runtime
            .host_state
            .borrow()
            .worklet
            .runtimes
            .get(&runtime.document().identity())
            .cloned()
            .unwrap();
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

#[cfg(test)]
#[path = "worklet_document_tests.rs"]
mod document_tests;

#[cfg(test)]
impl State {
    fn single_test_runtime(&self) -> Option<WorkletRuntimeHandle> {
        assert_eq!(
            self.runtimes.len(),
            1,
            "fixture must own one Worklet runtime"
        );
        self.runtimes.values().next().cloned()
    }
}
