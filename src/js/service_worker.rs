//! Classic ServiceWorker startup using the isolated Worker runtime.
use super::*;

enum ServiceWorkerStartResult {
    Started(u64),
    LoadFailure,
    SecurityFailure,
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
    let source = security_reporting::capture_csp_source(context);
    with_host_state(|owner| {
        start_service_worker(Rc::clone(owner), &url, &scope, document_id, source.as_ref()).map(
            |result| match result {
                ServiceWorkerStartResult::Started(id) => js_string!(id.to_string()).into(),
                ServiceWorkerStartResult::LoadFailure => JsValue::null(),
                ServiceWorkerStartResult::SecurityFailure => js_string!("security").into(),
            },
        )
    })
}

fn start_service_worker(
    owner: Rc<RefCell<HostState>>,
    url: &str,
    scope: &str,
    document_id: usize,
    source: Option<&security_reporting::CspSourceLocation>,
) -> JsResult<ServiceWorkerStartResult> {
    let PreparedDedicatedWorker {
        id,
        url: effective_url,
        csp_requested_url,
        source,
        content_type,
        service_worker_allowed,
        mut runtime,
    } = prepare_dedicated_worker(&owner, url, document_id, source, true)?;
    let Some(source) = source else {
        return Ok(ServiceWorkerStartResult::LoadFailure);
    };
    // The ServiceWorker loader rejects redirects before fetching their target.
    // Keep this identity check as a guard against future loader changes.
    if effective_url != csp_requested_url {
        return Ok(ServiceWorkerStartResult::LoadFailure);
    }
    if !content_type
        .as_deref()
        .and_then(|value| value.split(';').next())
        .is_some_and(is_javascript_mime_type)
    {
        return Ok(ServiceWorkerStartResult::SecurityFailure);
    }
    if let Err(error) =
        check_service_worker_scope(&effective_url, scope, service_worker_allowed.as_deref())
    {
        return Ok(error);
    }

    let Ok(scope_url) = scope.parse::<crate::http::Url>() else {
        return Ok(ServiceWorkerStartResult::LoadFailure);
    };
    let top_level_site = owner
        .borrow()
        .storage_access_top_level_site(document_id)
        .unwrap_or_else(|| crate::http::SchemefulSite::from_url(&scope_url));

    {
        let mut state = runtime.host_state.borrow_mut();
        state.worker_service_worker_scope = Some(scope.to_owned());
        state.worker_service_worker_top_level_site = Some(top_level_site.clone());
    }
    let encoded_scope = serde_json::to_string(scope).expect("scope URLs are serializable");
    runtime.eval(&format!(
        "__omoikane_install_service_worker_global({encoded_scope})"
    ))?;
    let evaluated =
        runtime.with_active_host(
            |context| match script_errors::evaluate_worker_initial_script(
                context,
                &source,
                &effective_url,
                &csp_requested_url,
            )? {
                script_errors::WorkerScriptOutcome::Completed => Ok(true),
                script_errors::WorkerScriptOutcome::ParseFailure => Ok(false),
                script_errors::WorkerScriptOutcome::Exception(error) => {
                    script_errors::report_worker_startup_exception(context, &error)?;
                    Ok(false)
                }
            },
        )?;
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
    owner
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .register_service_worker_scope(scope, &top_level_site);
    owner.borrow_mut().workers.insert(id, entry);
    Ok(ServiceWorkerStartResult::Started(id))
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
