//! Classic ServiceWorker startup using the isolated Worker runtime.
use super::*;

enum ServiceWorkerStartResult {
    Started(u64),
    Unchanged,
    LoadFailure,
    SecurityFailure,
}

/// Owns rollback of a newly reserved registration while author startup runs.
struct PendingRegistration {
    jar: Arc<Mutex<crate::http::CookieJar>>,
    scope: String,
    top_level_site: crate::http::SchemefulSite,
    id: u64,
    fresh: bool,
    committed: bool,
}

impl PendingRegistration {
    fn reserve(
        owner: &Rc<RefCell<HostState>>,
        scope: &str,
        top_level_site: crate::http::SchemefulSite,
    ) -> Self {
        let jar = Arc::clone(&owner.borrow().cookie_store);
        let (fresh, id) = {
            let mut cookies = jar.lock().unwrap();
            let fresh = cookies
                .service_worker_registration_id(scope, &top_level_site)
                .is_none();
            let id = cookies.register_service_worker_scope(scope, &top_level_site);
            (fresh, id)
        };
        Self {
            jar,
            scope: scope.to_owned(),
            top_level_site,
            id,
            fresh,
            committed: false,
        }
    }
}

impl Drop for PendingRegistration {
    fn drop(&mut self) {
        if self.fresh && !self.committed {
            let mut cookies = self.jar.lock().unwrap();
            if cookies.service_worker_registration_id(&self.scope, &self.top_level_site)
                == Some(self.id)
            {
                cookies.clear_service_worker_subscriptions(&self.scope, &self.top_level_site);
            }
        }
    }
}

/// Starts a classic ServiceWorker and returns its private runtime id.
/// A null result indicates a network or evaluation failure; "security" marks
/// response MIME or scope validation failure for the DOM binding.
pub(super) fn start_service_worker_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let url = string_argument(args.first(), "", context)?;
    let scope = string_argument(args.get(1), "", context)?;
    let document_id = parse_node_id(args.get(2), context)?;
    ensure_same_origin_node(context, document_id)?;
    let previous = args
        .get(3)
        .filter(|value| !value.is_null_or_undefined())
        .map(|value| {
            value
                .to_string(context)
                .map(|value| value.to_std_string_escaped().parse::<u64>().ok())
        })
        .transpose()?
        .flatten();
    let source = security_reporting::capture_csp_source(context);
    with_host_state(|owner| {
        if previous
            .is_some_and(|id| !update_registration_is_current(owner, id, &scope, document_id))
        {
            return Ok(js_string!("stale").into());
        }
        start_service_worker(
            Rc::clone(owner),
            &url,
            &scope,
            document_id,
            source.as_ref(),
            previous,
        )
        .map(|result| match result {
            ServiceWorkerStartResult::Unchanged => js_string!("unchanged").into(),
            ServiceWorkerStartResult::Started(id) => js_string!(id.to_string()).into(),
            ServiceWorkerStartResult::LoadFailure => JsValue::null(),
            ServiceWorkerStartResult::SecurityFailure => js_string!("security").into(),
        })
    })
}

/// Validates the registration generation before any update network request.
fn update_registration_is_current(
    owner: &Rc<RefCell<HostState>>,
    id: u64,
    scope: &str,
    document_id: usize,
) -> bool {
    let (entry, jar, top_level_site) = {
        let owner = owner.borrow();
        let Some(entry) = owner.workers.get(&id).cloned() else {
            return false;
        };
        let top = owner
            .storage_access_top_level_site(document_id)
            .or_else(|| {
                scope
                    .parse::<crate::http::Url>()
                    .ok()
                    .map(|url| crate::http::SchemefulSite::from_url(&url))
            });
        (entry, Arc::clone(&owner.cookie_store), top)
    };
    let worker = entry.borrow();
    let state = worker.runtime.host_state.borrow();
    if worker.terminated
        || state.worker_terminated
        || state.worker_service_worker_scope.as_deref() != Some(scope)
        || state.worker_service_worker_top_level_site != top_level_site
    {
        return false;
    }
    let Some(top) = top_level_site else {
        return false;
    };
    let Some(generation) = state.worker_service_worker_registration_id else {
        return false;
    };
    jar.lock()
        .unwrap()
        .service_worker_registration_id(scope, &top)
        == Some(generation)
}

fn start_service_worker(
    owner: Rc<RefCell<HostState>>,
    url: &str,
    scope: &str,
    document_id: usize,
    source: Option<&security_reporting::CspSourceLocation>,
    previous: Option<u64>,
) -> JsResult<ServiceWorkerStartResult> {
    let fetched = fetch_dedicated_worker(&owner, url, document_id, source, true)?;
    let Some(script) = fetched.loaded.as_ref() else {
        return Ok(ServiceWorkerStartResult::LoadFailure);
    };
    // The ServiceWorker loader rejects redirects before fetching their target.
    // Keep this identity check as a guard against future loader changes.
    if fetched.worker_url != fetched.initial_url {
        return Ok(ServiceWorkerStartResult::LoadFailure);
    }
    if !script
        .content_type
        .as_deref()
        .and_then(|value| value.split(';').next())
        .is_some_and(is_javascript_mime_type)
    {
        return Ok(ServiceWorkerStartResult::SecurityFailure);
    }
    if let Err(error) = check_service_worker_scope(
        &fetched.worker_url,
        scope,
        script.service_worker_allowed.as_deref(),
    ) {
        return Ok(error);
    }

    let PreparedDedicatedWorker {
        id,
        url: effective_url,
        csp_requested_url,
        source,
        mut runtime,
    } = initialize_dedicated_worker(&owner, fetched)?;
    let source = source.expect("accepted ServiceWorker fetch retains its script source");

    if let Some(previous) = previous {
        match prepare_update_imports(&owner, previous, &source, &mut runtime) {
            Ok(true) => return Ok(ServiceWorkerStartResult::Unchanged),
            Ok(false) => {}
            Err(error) => return Ok(error),
        }
    }

    let Ok(scope_url) = scope.parse::<crate::http::Url>() else {
        return Ok(ServiceWorkerStartResult::LoadFailure);
    };
    let top_level_site = owner
        .borrow()
        .storage_access_top_level_site(document_id)
        .unwrap_or_else(|| crate::http::SchemefulSite::from_url(&scope_url));

    let mut registration = PendingRegistration::reserve(&owner, scope, top_level_site.clone());
    {
        let mut state = runtime.host_state.borrow_mut();
        state.worker_service_worker_registration_id = Some(registration.id);
        state.worker_service_worker_scope = Some(scope.to_owned());
        state.service_worker_main_source = Some(source.clone());
        state.worker_service_worker_top_level_site = Some(top_level_site.clone());
    }
    let encoded_scope = serde_json::to_string(scope).expect("scope URLs are serializable");
    runtime.eval(&format!(
        "__omoikane_install_service_worker_global({encoded_scope})"
    ))?;
    let evaluated =
        evaluate_initial_script(&mut runtime, &source, &effective_url, &csp_requested_url)?;
    if !evaluated || runtime.is_terminated_worker() {
        return Ok(ServiceWorkerStartResult::LoadFailure);
    }
    dispatch_lifecycle(&mut runtime, "install")?;
    if runtime.is_terminated_worker() {
        return Ok(ServiceWorkerStartResult::LoadFailure);
    }

    let outgoing = std::mem::take(&mut runtime.host_state.borrow_mut().worker_startup_outgoing);
    let entry = Rc::new(RefCell::new(WorkerRuntime {
        runtime,
        owner_state: Rc::clone(&owner),
        owner_object: None,
        outgoing,
        startup_error: None,
        service_worker: true,
        terminated: false,
    }));
    registration.committed = true;
    owner.borrow_mut().workers.insert(id, entry);
    Ok(ServiceWorkerStartResult::Started(id))
}

fn prepare_update_imports(
    owner: &Rc<RefCell<HostState>>,
    previous: u64,
    source: &str,
    runtime: &mut JsRuntime,
) -> Result<bool, ServiceWorkerStartResult> {
    let previous = owner
        .borrow()
        .workers
        .get(&previous)
        .cloned()
        .ok_or(ServiceWorkerStartResult::LoadFailure)?;
    let (main, imports) = {
        let previous = previous.borrow();
        let state = previous.runtime.host_state.borrow();
        (
            state.service_worker_main_source.clone(),
            state.service_worker_imports.clone(),
        )
    };
    // A changed main script determines its own imports. Removed old resources
    // must not make an otherwise valid new worker fail to install.
    if main.as_deref() != Some(source) {
        return Ok(false);
    }
    let resources = worker_imports::refetch_service_worker_imports(
        &mut runtime.host_state.borrow_mut(),
        &imports,
    );
    let unchanged = main.as_deref() == Some(source)
        && resources.iter().all(|(url, resource)| {
            resource.as_ref().is_none_or(|resource| {
                imports
                    .get(url)
                    .is_some_and(|old| old.source == resource.source)
            })
        });
    runtime.host_state.borrow_mut().service_worker_import_cache = resources;
    Ok(unchanged)
}

fn evaluate_initial_script(
    runtime: &mut JsRuntime,
    source: &str,
    effective_url: &str,
    requested_url: &str,
) -> JsResult<bool> {
    runtime.with_active_host(|context| {
        match script_errors::evaluate_worker_initial_script(
            context,
            source,
            effective_url,
            requested_url,
        )? {
            script_errors::WorkerScriptOutcome::Completed => Ok(true),
            script_errors::WorkerScriptOutcome::ParseFailure => Ok(false),
            script_errors::WorkerScriptOutcome::Exception(error) => {
                script_errors::report_worker_startup_exception(context, &error)?;
                Ok(false)
            }
        }
    })
}

fn check_service_worker_scope(
    script: &str,
    scope: &str,
    allowed: Option<&str>,
) -> Result<(), ServiceWorkerStartResult> {
    let script = url::Url::parse(script).map_err(|_| ServiceWorkerStartResult::LoadFailure)?;
    let scope = url::Url::parse(scope).map_err(|_| ServiceWorkerStartResult::LoadFailure)?;
    let max_scope = script
        .join(allowed.unwrap_or("./"))
        .map_err(|_| ServiceWorkerStartResult::LoadFailure)?;
    if max_scope.origin() != script.origin()
        || scope.origin() != script.origin()
        || !scope.path().starts_with(max_scope.path())
    {
        return Err(ServiceWorkerStartResult::SecurityFailure);
    }
    Ok(())
}

/// Dispatches activate in the worker realm before its client state becomes activated.
/// Returns false if the worker terminates or fails during dispatch.
pub(super) fn dispatch_service_worker_activate_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = worker_id_argument(args, context)?;
    with_host_state(|owner| {
        // Keep the entry outside the owner map during dispatch so host
        // termination never reborrows this WorkerRuntime.
        let entry = owner.borrow_mut().workers.remove(&id);
        let Some(entry) = entry else {
            return Ok(JsValue::from(false));
        };
        let mut worker = entry.borrow_mut();
        if !worker.service_worker {
            drop(worker);
            owner.borrow_mut().workers.insert(id, entry);
            return Ok(JsValue::from(false));
        }
        if worker.terminated || worker.runtime.is_terminated_worker() {
            owner.borrow_mut().worker_owner_objects.remove(&id);
            return Ok(JsValue::from(false));
        }
        let result = dispatch_lifecycle(&mut worker.runtime, "activate");
        let live = result.is_ok() && !worker.runtime.is_terminated_worker();
        let realm = worker
            .runtime
            .host_state
            .borrow()
            .worker_owner_realm
            .clone();
        if !live {
            worker.terminated = true;
            worker.runtime.host_state.borrow_mut().worker_terminated = true;
        }
        drop(worker);
        if let Err(error) = result {
            owner
                .borrow_mut()
                .event_loop
                .enqueue_worker_error(id, None, realm, error.to_string());
        }
        if live {
            owner.borrow_mut().workers.insert(id, entry);
        } else {
            owner.borrow_mut().worker_owner_objects.remove(&id);
        }
        Ok(JsValue::from(live))
    })
}

fn dispatch_lifecycle(runtime: &mut JsRuntime, phase: &str) -> JsResult<()> {
    call_lifecycle_callback(runtime, "dispatch", &[js_string!(phase).into()])?;
    runtime.run_jobs()?;
    if !runtime.is_terminated_worker() {
        call_lifecycle_callback(runtime, "finish", &[js_string!(phase).into()])?;
        runtime.run_jobs()?;
    }
    Ok(())
}

fn call_lifecycle_callback(runtime: &mut JsRuntime, name: &str, args: &[JsValue]) -> JsResult<()> {
    let document_id = runtime
        .context
        .realm()
        .host_defined()
        .get::<ModuleDocumentId>()
        .map(|identity| identity.0)
        .ok_or_else(|| {
            JsNativeError::error().with_message("ServiceWorker realm has no identity")
        })?;
    let holder = runtime
        .host_state
        .borrow()
        .service_worker_lifecycle_callbacks
        .get(&document_id)
        .cloned()
        .ok_or_else(|| {
            JsNativeError::error().with_message("ServiceWorker callbacks are unavailable")
        })?;
    runtime.with_active_host(|context| {
        let helper = holder.get(js_string!(name), context)?;
        let callable = helper.as_callable().ok_or_else(|| {
            JsNativeError::typ().with_message("ServiceWorker lifecycle callback is unavailable")
        })?;
        callable
            .call(&JsValue::undefined(), args, context)
            .map(|_| ())
    })
}

/// Transfers lifecycle completion to the owner without sharing realm values.
pub(super) fn lifecycle_complete_native(
    _: &JsValue,
    args: &[JsValue],
    _: &mut Context,
) -> JsResult<JsValue> {
    let installing = args.first().is_some_and(JsValue::to_boolean);
    let failed = args.get(1).is_some_and(JsValue::to_boolean);
    with_host_state(|state| {
        let state = state.borrow();
        if !state.worker_terminated {
            if let (Some(owner), Some(id)) = (&state.worker_owner, state.worker_id) {
                owner
                    .borrow_mut()
                    .event_loop
                    .enqueue_service_worker_lifecycle(id, installing, failed);
            }
        }
        Ok(JsValue::undefined())
    })
}

impl JsRuntime {
    pub(super) fn run_service_worker_lifecycle(
        &mut self,
        id: u64,
        installing: bool,
        failed: bool,
    ) -> JsResult<()> {
        let (owner, realm) = {
            let state = self.host_state.borrow();
            let Some(owner) = state.worker_owner_objects.get(&id).cloned() else {
                return Ok(());
            };
            let realm = state.workers.get(&id).and_then(|entry| {
                entry
                    .borrow()
                    .runtime
                    .host_state
                    .borrow()
                    .worker_owner_realm
                    .clone()
            });
            (owner, realm)
        };
        let previous = realm.map(|realm| self.context.enter_realm(realm));
        let result = call_lifecycle_callback(
            self,
            "complete",
            &[
                owner,
                js_string!(id.to_string()).into(),
                installing.into(),
                failed.into(),
            ],
        );
        if let Some(previous) = previous {
            self.context.enter_realm(previous);
        }
        result
    }
}

/// Queues an operation on the owner registration, without borrowing its runtime.
pub(super) fn registration_request_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let request = string_argument(args.first(), "", context)?;
    let operation = string_argument(args.get(1), "", context)?;
    with_host_state(|state| {
        let state = state.borrow();
        let (Some(owner), Some(id)) = (&state.worker_owner, state.worker_id) else {
            return Ok(false.into());
        };
        owner
            .borrow_mut()
            .event_loop
            .enqueue_service_worker_registration_request(id, request, operation);
        Ok(true.into())
    })
}

/// Settles a worker operation through its own event loop and realm.
pub(super) fn registration_response_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = worker_id_argument(args, context)?;
    let request = string_argument(args.get(1), "", context)?;
    let result = string_argument(args.get(2), "", context)?;
    let error = string_argument(args.get(3), "", context)?;
    with_host_state(|owner| {
        let entry = owner.borrow_mut().workers.remove(&id);
        if let Some(entry) = entry {
            // This callback runs from the queued owner job, after the calling
            // worker has yielded. Remove the map entry while executing author
            // callbacks, just like ordinary worker messages, so termination
            // never reborrows this WorkerRuntime.
            let mut worker = entry.borrow_mut();
            let result = worker
                .runtime
                .run_service_worker_registration_response(request, result, error)
                .and_then(|()| worker.runtime.run_jobs());
            let terminated = worker.runtime.is_terminated_worker();
            drop(worker);
            if !terminated {
                owner.borrow_mut().workers.insert(id, entry);
            } else {
                owner.borrow_mut().worker_owner_objects.remove(&id);
            }
            result?;
        }
        Ok(JsValue::undefined())
    })
}

impl JsRuntime {
    pub(super) fn run_service_worker_registration_request(
        &mut self,
        id: u64,
        request: String,
        operation: String,
    ) -> JsResult<()> {
        let (owner, realm) = {
            let state = self.host_state.borrow();
            let Some(owner) = state.worker_owner_objects.get(&id).cloned() else {
                return Ok(());
            };
            let realm = state.workers.get(&id).and_then(|entry| {
                entry
                    .borrow()
                    .runtime
                    .host_state
                    .borrow()
                    .worker_owner_realm
                    .clone()
            });
            (owner, realm)
        };
        let previous = realm.map(|realm| self.context.enter_realm(realm));
        let result = call_lifecycle_callback(
            self,
            "registrationRequest",
            &[
                owner,
                js_string!(id.to_string()).into(),
                js_string!(request).into(),
                js_string!(operation).into(),
            ],
        );
        if let Some(previous) = previous {
            self.context.enter_realm(previous);
        }
        result
    }

    pub(super) fn run_service_worker_registration_response(
        &mut self,
        request: String,
        result: String,
        error: String,
    ) -> JsResult<()> {
        call_lifecycle_callback(
            self,
            "registrationResponse",
            &[
                js_string!(request).into(),
                js_string!(result).into(),
                js_string!(error).into(),
            ],
        )
    }
}
