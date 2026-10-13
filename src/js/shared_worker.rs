//! Classic `SharedWorker`: the same-thread runtime registry, port connections,
//! and message delivery between owner pages and the shared global.

use super::*;

thread_local! {
    /// Same-thread registry for classic `SharedWorker` runtimes.  Shared
    /// workers are deliberately kept on the owning Boa thread: only
    /// structured-clone wires cross the registry, never a `JsValue`.
    static SHARED_WORKER_REGISTRY: RefCell<Vec<Rc<RefCell<SharedWorkerRuntime>>>> =
        const { RefCell::new(Vec::new()) };
    static NEXT_SHARED_WORKER_ID: Cell<u64> = const { Cell::new(1) };
    static NEXT_SHARED_WORKER_CONNECTION_ID: Cell<u64> = const { Cell::new(1) };
}

/// SharedWorker endpoints and identity held by one global.
#[derive(Default)]
pub(super) struct State {
    /// Shared-worker globals identify themselves so the event-loop pump does
    /// not recursively execute the registry entry currently being serviced.
    id: Option<u64>,
    /// Owned recorder configuration inherited from the creating page.
    error_destination: Option<(Arc<ErrorReporter>, ExecutionSurface)>,
    /// Page-owned `SharedWorkerPort` endpoint references keyed by a
    /// process-local connection id.  The endpoint remains in its own Boa
    /// realm; native delivery only retains it until the port is closed.
    ports: HashMap<u64, JsValue>,
}

impl State {
    pub(super) fn is_global(&self) -> bool {
        self.id.is_some()
    }

    /// Records a failure diagnostic without exporting its author payload.
    pub(super) fn record_failure(&self, startup: bool) {
        if self.is_global() {
            report_safe_worker_or_module_failure(
                self.error_destination.clone(),
                ErrorCategory::Worker,
                if startup {
                    "SHARED_WORKER_STARTUP_FAILED"
                } else {
                    "SHARED_WORKER_RUNTIME_FAILED"
                },
                "execute",
            );
        }
    }

    /// Traces the page-owned port endpoints retained for delivery.
    pub(super) unsafe fn trace(&self, tracer: &mut Tracer) {
        for port in self.ports.values() {
            unsafe { port.trace(tracer) };
        }
    }
}

/// The key used by the same-thread `SharedWorker` registry.  A worker is
/// shared only when its resolved script URL, name, and origin all match.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SharedWorkerKey {
    /// Shared workers are only constructible for an eligible tuple origin;
    /// opaque/no-origin callers are rejected before a key is created.
    origin: StorageOrigin,
    url: String,
    name: String,
}

/// One page-to-shared-worker connection.  The page endpoint is retained only
/// in the page's realm; the shared runtime receives the numeric id and clone
/// wire through its event loop.
struct SharedWorkerConnection {
    owner_state: Weak<RefCell<HostState>>,
    owner_port: Option<JsValue>,
    owner_origin: String,
    pending_to_owner: VecDeque<String>,
    /// A startup failure is delivered once the page binds its `SharedWorker`
    /// object.  Keeping it on the connection lets every caller observe the
    /// same failed shared runtime without exposing a native error directly
    /// from the constructor.
    startup_error: Option<script_errors::WorkerErrorNotification>,
    closed: bool,
}

/// State for one classic shared worker.  Its `JsRuntime` is independent of
/// every connecting page and remains in the thread-local registry while at
/// least one connection is alive.
struct SharedWorkerRuntime {
    key: SharedWorkerKey,
    runtime: Rc<RefCell<JsRuntime>>,
    startup_error: Option<script_errors::WorkerErrorNotification>,
    connections: HashMap<u64, SharedWorkerConnection>,
}

fn next_shared_worker_id() -> u64 {
    NEXT_SHARED_WORKER_ID.with(|next| {
        let id = next.get();
        next.set(id.saturating_add(1));
        id
    })
}

fn next_shared_worker_connection_id() -> u64 {
    NEXT_SHARED_WORKER_CONNECTION_ID.with(|next| {
        let id = next.get();
        next.set(id.saturating_add(1));
        id
    })
}

fn shared_worker_origin(state: &HostState, document_id: usize) -> Option<StorageOrigin> {
    match state.document_security_origins.get(&document_id) {
        Some(DocumentSecurityOrigin::Tuple(origin)) => Some(origin.clone()),
        _ => None,
    }
}

fn prune_shared_worker_registry(registry: &mut Vec<Rc<RefCell<SharedWorkerRuntime>>>) {
    for entry in registry.iter() {
        let mut shared = entry.borrow_mut();
        shared.connections.retain(|_, connection| {
            !connection.closed && connection.owner_state.strong_count() > 0
        });
    }
    registry.retain(|entry| {
        let shared = entry.borrow();
        !shared.connections.is_empty()
            && !shared
                .runtime
                .borrow()
                .host_state
                .borrow()
                .worker_terminated
    });
}

fn shared_worker_entry_for_connection(
    connection_id: u64,
) -> Option<Rc<RefCell<SharedWorkerRuntime>>> {
    SHARED_WORKER_REGISTRY.with(|registry| {
        registry
            .borrow()
            .iter()
            .find(|entry| entry.borrow().connections.contains_key(&connection_id))
            .cloned()
    })
}

fn shared_worker_entry_for_key(key: &SharedWorkerKey) -> Option<Rc<RefCell<SharedWorkerRuntime>>> {
    SHARED_WORKER_REGISTRY.with(|registry| {
        registry
            .borrow()
            .iter()
            .find(|entry| entry.borrow().key == *key)
            .cloned()
    })
}

pub(super) fn terminate_shared_worker_connections(state: &Rc<RefCell<HostState>>) {
    let connection_ids: Vec<_> = state.borrow().shared_worker.ports.keys().copied().collect();
    for connection_id in connection_ids {
        if let Some(entry) = shared_worker_entry_for_connection(connection_id) {
            if let Some(connection) = entry.borrow_mut().connections.get_mut(&connection_id) {
                connection.closed = true;
                connection.owner_port = None;
                connection.pending_to_owner.clear();
            }
        }
    }
    state.borrow_mut().shared_worker.ports.clear();
}

fn shared_worker_connect_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let requested_url = string_argument(args.first(), "", context)?;
    let name = string_argument(args.get(1), "", context)?;
    let document_id = caller_document_id(context).ok_or_else(|| {
        JsNativeError::error().with_message("SharedWorker owner document is unavailable")
    })?;
    with_host_state(|state| {
        if state.borrow().shared_worker.id.is_some() {
            return Err(JsNativeError::error()
                .with_message("SharedWorker construction from a SharedWorker is unsupported")
                .into());
        }
        create_shared_worker_for_owner_state(Rc::clone(state), &requested_url, &name, document_id)
            .map(|id| JsValue::from(js_string!(id.to_string())))
    })
}

fn shared_worker_bind_port_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let connection_id = worker_id_argument(args, context)?;
    let port = args.get(1).cloned().unwrap_or_default();
    if !port.is_object() {
        return Err(JsNativeError::typ()
            .with_message("SharedWorker port must be an object")
            .into());
    }
    let owner_object = args.get(2).cloned().unwrap_or_default();
    if !owner_object.is_object() {
        return Err(JsNativeError::typ()
            .with_message("SharedWorker owner must be an object")
            .into());
    }
    let owner_realm = context.realm().clone();
    with_host_state(|state| {
        let Some(entry) = shared_worker_entry_for_connection(connection_id) else {
            return Ok(JsValue::undefined());
        };
        let (owner_state, origin, pending, startup_error) = {
            let mut shared = entry.borrow_mut();
            let Some(connection) = shared.connections.get_mut(&connection_id) else {
                return Ok(JsValue::undefined());
            };
            let Some(owner_state) = connection.owner_state.upgrade() else {
                connection.closed = true;
                return Ok(JsValue::undefined());
            };
            if !Rc::ptr_eq(&owner_state, state) {
                return Err(JsNativeError::error()
                    .with_message("SharedWorker port belongs to another global")
                    .into());
            }
            connection.owner_port = Some(port.clone());
            let origin = connection.owner_origin.clone();
            let pending = std::mem::take(&mut connection.pending_to_owner);
            let startup_error = connection.startup_error.take();
            (owner_state, origin, pending, startup_error)
        };
        state
            .borrow_mut()
            .shared_worker
            .ports
            .insert(connection_id, port.clone());
        for data in pending {
            owner_state
                .borrow_mut()
                .event_loop
                .enqueue_shared_worker_owner_message(
                    connection_id,
                    port.clone(),
                    data,
                    origin.clone(),
                );
        }
        if let Some(message) = startup_error {
            // SharedWorker startup failures are reported asynchronously on the
            // page-facing SharedWorker object, matching Dedicated Worker error
            // delivery and ensuring construction never silently succeeds.
            state.borrow_mut().event_loop.enqueue_worker_error(
                connection_id,
                Some(owner_object),
                Some(owner_realm.clone()),
                message,
            );
        }
        Ok(JsValue::undefined())
    })
}

fn shared_worker_port_post_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let connection_id = worker_id_argument(args, context)?;
    let data = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let Some(entry) = shared_worker_entry_for_connection(connection_id) else {
            return Ok(JsValue::undefined());
        };
        if state.borrow().shared_worker.id.is_some() {
            // The call originated in the shared-worker realm.  Route it to
            // the page endpoint, retaining the wire until the page port has
            // been bound (the constructor binds immediately after connect).
            let (owner_state, owner_port, origin) = {
                let mut shared = entry.borrow_mut();
                let Some(connection) = shared.connections.get_mut(&connection_id) else {
                    return Ok(JsValue::undefined());
                };
                if connection.closed {
                    return Ok(JsValue::undefined());
                }
                let Some(owner_state) = connection.owner_state.upgrade() else {
                    connection.closed = true;
                    return Ok(JsValue::undefined());
                };
                let owner_port = connection.owner_port.clone();
                let origin = connection.owner_origin.clone();
                if owner_port.is_none() {
                    connection.pending_to_owner.push_back(data);
                    return Ok(JsValue::undefined());
                }
                (owner_state, owner_port, origin)
            };
            if let Some(owner_port) = owner_port {
                owner_state
                    .borrow_mut()
                    .event_loop
                    .enqueue_shared_worker_owner_message(connection_id, owner_port, data, origin);
            }
        } else {
            // The call originated in a page realm.  Ensure the connection is
            // owned by that exact global before enqueueing into the shared
            // worker runtime.
            let runtime = {
                let shared = entry.borrow();
                let Some(connection) = shared.connections.get(&connection_id) else {
                    return Ok(JsValue::undefined());
                };
                if connection.closed
                    || connection
                        .owner_state
                        .upgrade()
                        .is_none_or(|owner| !Rc::ptr_eq(&owner, state))
                {
                    return Ok(JsValue::undefined());
                }
                Rc::clone(&shared.runtime)
            };
            runtime
                .borrow_mut()
                .host_state
                .borrow_mut()
                .event_loop
                .enqueue_shared_worker_message(connection_id, data);
        }
        Ok(JsValue::undefined())
    })
}

fn shared_worker_port_close_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let connection_id = worker_id_argument(args, context)?;
    with_host_state(|state| {
        let Some(entry) = shared_worker_entry_for_connection(connection_id) else {
            state
                .borrow_mut()
                .shared_worker
                .ports
                .remove(&connection_id);
            return Ok(JsValue::undefined());
        };
        let mut shared = entry.borrow_mut();
        let Some(connection) = shared.connections.get_mut(&connection_id) else {
            state
                .borrow_mut()
                .shared_worker
                .ports
                .remove(&connection_id);
            return Ok(JsValue::undefined());
        };
        if state.borrow().shared_worker.id.is_none()
            && connection
                .owner_state
                .upgrade()
                .is_none_or(|owner| !Rc::ptr_eq(&owner, state))
        {
            return Ok(JsValue::undefined());
        }
        connection.closed = true;
        connection.owner_port = None;
        connection.pending_to_owner.clear();
        state
            .borrow_mut()
            .shared_worker
            .ports
            .remove(&connection_id);
        Ok(JsValue::undefined())
    })
}

fn create_shared_worker_for_owner_state(
    owner_state: Rc<RefCell<HostState>>,
    requested_url: &str,
    name: &str,
    document_id: usize,
) -> JsResult<u64> {
    let (owner_url, base_url, origin, origin_text, creator_policy) = {
        let state = owner_state.borrow();
        let origin = shared_worker_origin(&state, document_id).ok_or_else(|| {
            JsNativeError::error().with_message("SharedWorker requires an eligible origin")
        })?;
        let origin_text = script_errors::document_fetch_origin(&state, document_id).serialize();
        (
            origin_text.clone(),
            state.base_url_for_document(document_id),
            origin,
            origin_text,
            state
                .document_csp
                .get(&document_id)
                .cloned()
                .unwrap_or_default(),
        )
    };
    let worker_url = resolve_worker_url(requested_url, &owner_url, base_url.as_ref())?;
    let key = SharedWorkerKey {
        origin,
        url: worker_url.clone(),
        name: name.to_string(),
    };
    let entry = if let Some(existing) = shared_worker_entry_for_key(&key) {
        existing
    } else {
        let loaded = {
            let mut state = owner_state.borrow_mut();
            worker_loading::load_worker_script(
                &worker_url,
                base_url.as_ref(),
                &mut state.http_client,
            )
        };
        let entry = Rc::new(RefCell::new(create_shared_worker_runtime(
            &owner_state,
            key,
            loaded,
            creator_policy,
        )?));
        SHARED_WORKER_REGISTRY.with(|registry| registry.borrow_mut().push(Rc::clone(&entry)));
        entry
    };

    let connection_id = next_shared_worker_connection_id();
    let startup_error = entry.borrow().startup_error.clone();
    entry.borrow_mut().connections.insert(
        connection_id,
        SharedWorkerConnection {
            owner_state: Rc::downgrade(&owner_state),
            owner_port: None,
            owner_origin: origin_text,
            pending_to_owner: VecDeque::new(),
            startup_error,
            closed: false,
        },
    );
    let runtime = Rc::clone(&entry.borrow().runtime);
    // The runtime is borrowed independently from the registry entry so
    // `postMessage` calls made synchronously by an onconnect handler can
    // safely look the connection up again.
    if entry.borrow().startup_error.is_none()
        && !runtime.borrow().host_state.borrow().worker_terminated
    {
        let _ = runtime.borrow_mut().eval(&format!(
            "__omoikane_dispatch_shared_worker_connect({connection_id:?})"
        ));
    }
    Ok(connection_id)
}

/// Prepares a new shared global from its own response while retaining the
/// requested URL in the registry key. The effective script path also supplies
/// the base for dynamic imports from the initial classic script.
fn create_shared_worker_runtime(
    owner_state: &Rc<RefCell<HostState>>,
    key: SharedWorkerKey,
    loaded: Option<worker_loading::LoadedWorkerScript>,
    creator_policy: CspPolicy,
) -> JsResult<SharedWorkerRuntime> {
    let (storage, session_id, user_agent, error_destination) = {
        let state = owner_state.borrow();
        (
            state.storage_manager.clone(),
            state.storage_session_id,
            state.navigator_user_agent.clone(),
            state.error_reporter.clone(),
        )
    };
    let worker_url = loaded
        .as_ref()
        .map_or(key.url.as_str(), |loaded| loaded.effective_url.as_str());
    let shared_id = next_shared_worker_id();
    let mut runtime = JsRuntime::with_document_url_and_storage(
        blank_html_document(),
        worker_url,
        storage,
        session_id,
    )?;
    runtime.set_user_agent(user_agent);
    if let Some(loaded) = loaded.as_ref() {
        runtime.install_worker_security_metadata(loaded, &creator_policy);
    }
    {
        let mut state = runtime.host_state.borrow_mut();
        state.shared_worker.id = Some(shared_id);
        state.shared_worker.error_destination = error_destination.clone();
    }
    runtime.eval(&format!(
        "__omoikane_install_shared_worker_global({worker_url:?}, {shared_id:?})"
    ))?;
    let startup_error = evaluate_shared_worker_initial_source(&mut runtime, loaded.as_ref());
    if startup_error.is_some() {
        report_safe_worker_or_module_failure(
            error_destination,
            ErrorCategory::Worker,
            if loaded.is_some() {
                "SHARED_WORKER_STARTUP_FAILED"
            } else {
                "SHARED_WORKER_FETCH_FAILED"
            },
            if loaded.is_some() { "execute" } else { "fetch" },
        );
        runtime.host_state.borrow_mut().worker_terminated = true;
    }
    Ok(SharedWorkerRuntime {
        key,
        runtime: Rc::new(RefCell::new(runtime)),
        startup_error,
        connections: HashMap::new(),
    })
}

fn evaluate_shared_worker_initial_source(
    runtime: &mut JsRuntime,
    loaded: Option<&worker_loading::LoadedWorkerScript>,
) -> Option<script_errors::WorkerErrorNotification> {
    let Some(loaded) = loaded else {
        return Some(script_errors::WorkerErrorNotification::LoadFailure);
    };
    let result = runtime.with_active_host(|context| {
        use script_errors::WorkerScriptOutcome;
        match script_errors::evaluate_worker_initial_script(
            context,
            &loaded.source,
            &loaded.effective_url,
            &loaded.initial_url,
        )? {
            WorkerScriptOutcome::Completed => Ok(Some(())),
            WorkerScriptOutcome::ParseFailure => Ok(None),
            WorkerScriptOutcome::Exception(error) => {
                script_errors::report_worker_startup_exception(context, &error).map(|()| Some(()))
            }
        }
    });
    match result {
        // Runtime exceptions are reported at the shared global and do not
        // stop subsequent connect tasks.
        Ok(Some(())) => None,
        Ok(None) => Some(script_errors::WorkerErrorNotification::LoadFailure),
        Err(_) => {
            runtime
                .host_state
                .borrow()
                .shared_worker
                .record_failure(true);
            runtime.host_state.borrow_mut().worker_terminated = true;
            None
        }
    }
}

/// Registers the private SharedWorker hooks used by the DOM bootstrap.
pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    for (name, length, function) in [
        (
            js_string!("__omoikane_shared_worker_connect"),
            2,
            NativeFunction::from_copy_closure(shared_worker_connect_native),
        ),
        (
            js_string!("__omoikane_shared_worker_bind_port"),
            3,
            NativeFunction::from_copy_closure(shared_worker_bind_port_native),
        ),
        (
            js_string!("__omoikane_shared_worker_port_post"),
            2,
            NativeFunction::from_copy_closure(shared_worker_port_post_native),
        ),
        (
            js_string!("__omoikane_shared_worker_port_close"),
            1,
            NativeFunction::from_copy_closure(shared_worker_port_close_native),
        ),
    ] {
        register_private_builtin_callable(context, bindings, name, length, function)?;
    }
    Ok(())
}

impl JsRuntime {
    /// Advances each connected shared runtime once for this owner's clock step.
    pub(super) fn advance_shared_worker_clocks(&self, elapsed_ms: u64) {
        if self.host_state.borrow().shared_worker.is_global() {
            return;
        }
        let runtimes = SHARED_WORKER_REGISTRY.with(|registry| {
            registry
                .borrow()
                .iter()
                .filter_map(|entry| {
                    let shared = entry.borrow();
                    shared
                        .connections
                        .values()
                        .any(|connection| {
                            !connection.closed
                                && connection
                                    .owner_state
                                    .upgrade()
                                    .is_some_and(|owner| Rc::ptr_eq(&owner, &self.host_state))
                        })
                        .then(|| Rc::clone(&shared.runtime))
                })
                .collect::<Vec<_>>()
        });
        for runtime in runtimes {
            runtime
                .borrow()
                .host_state
                .borrow_mut()
                .event_loop
                .advance(elapsed_ms);
        }
    }

    /// Pumps every live shared worker owned by this Boa thread.  A shared
    /// worker has its own runtime and therefore cannot be serviced by the
    /// page's event-loop queues directly; running it between page tasks keeps
    /// cross-realm messages deterministic while avoiding a background thread.
    pub(super) fn run_shared_worker_background_tasks(&mut self) {
        if self.host_state.borrow().shared_worker.id.is_some() {
            return;
        }
        let entries = SHARED_WORKER_REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            prune_shared_worker_registry(&mut registry);
            registry.iter().cloned().collect::<Vec<_>>()
        });
        for entry in entries {
            let same_runtime = {
                let shared = entry.borrow();
                Rc::ptr_eq(&shared.runtime.borrow().host_state, &self.host_state)
            };
            if same_runtime {
                continue;
            }
            let (runtime, has_connections) = {
                let shared = entry.borrow();
                (Rc::clone(&shared.runtime), !shared.connections.is_empty())
            };
            if !has_connections {
                continue;
            }
            // Shared-worker failures are isolated from the owner page just as
            // Dedicated Worker failures are.  The connection remains usable
            // for subsequent tasks unless the worker explicitly closes.
            let mut runtime = runtime.borrow_mut();
            let result = runtime.run_until_idle();
            let errors = runtime.take_task_errors_filtered(true);
            for _ in 0..usize::from(result.is_err()) + errors.len() {
                runtime
                    .host_state
                    .borrow()
                    .shared_worker
                    .record_failure(false);
            }
        }
        SHARED_WORKER_REGISTRY
            .with(|registry| prune_shared_worker_registry(&mut registry.borrow_mut()));
    }

    pub(super) fn run_shared_worker_message(
        &mut self,
        connection_id: u64,
        data: String,
    ) -> JsResult<()> {
        if self.host_state.borrow().shared_worker.id.is_none() {
            return Ok(());
        }
        let global = self.context.global_object();
        global.set(
            js_string!("__omoikane_shared_worker_message_connection"),
            JsValue::from(js_string!(connection_id.to_string())),
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_shared_worker_message_wire"),
            JsValue::from(js_string!(data)),
            true,
            &mut self.context,
        )?;
        let result = self.eval(
            "var __omoikane_shared_worker_message_port = __omoikane_get_shared_worker_port(__omoikane_shared_worker_message_connection); if (__omoikane_shared_worker_message_port && !__omoikane_shared_worker_message_port._closed) { try { __omoikane_shared_worker_message_port._queueMessage(__omoikane_decode_worker_message(__omoikane_shared_worker_message_wire)); } catch (error) { __omoikane_shared_worker_message_port.dispatchEvent(new MessageEvent('messageerror', { data: null, origin: location.origin, source: null, ports: [] })); } }",
        );
        let _ = global.set(
            js_string!("__omoikane_shared_worker_message_connection"),
            JsValue::undefined(),
            true,
            &mut self.context,
        );
        let _ = global.set(
            js_string!("__omoikane_shared_worker_message_wire"),
            JsValue::undefined(),
            true,
            &mut self.context,
        );
        self.record_error_from("shared worker message", result);
        Ok(())
    }

    pub(super) fn run_shared_worker_owner_message(
        &mut self,
        connection_id: u64,
        port: JsValue,
        data: String,
        origin: String,
    ) -> JsResult<()> {
        if !self
            .host_state
            .borrow()
            .shared_worker
            .ports
            .contains_key(&connection_id)
        {
            return Ok(());
        }
        let global = self.context.global_object();
        global.set(
            js_string!("__omoikane_shared_worker_owner_port"),
            port,
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_shared_worker_owner_wire"),
            JsValue::from(js_string!(data)),
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_shared_worker_owner_origin"),
            JsValue::from(js_string!(origin)),
            true,
            &mut self.context,
        )?;
        let result = self.eval(
            "var __omoikane_shared_worker_owner_target = __omoikane_shared_worker_owner_port; if (__omoikane_shared_worker_owner_target && !__omoikane_shared_worker_owner_target._closed) { try { __omoikane_shared_worker_owner_target._queueMessage(__omoikane_decode_worker_message(__omoikane_shared_worker_owner_wire)); } catch (error) { __omoikane_shared_worker_owner_target.dispatchEvent(new MessageEvent('messageerror', { data: null, origin: __omoikane_shared_worker_owner_origin, source: null, ports: [] })); } }",
        );
        for name in [
            "__omoikane_shared_worker_owner_port",
            "__omoikane_shared_worker_owner_wire",
            "__omoikane_shared_worker_owner_origin",
        ] {
            let _ = global.set(
                js_string!(name),
                JsValue::undefined(),
                true,
                &mut self.context,
            );
        }
        self.record_error_from("shared worker owner message", result);
        Ok(())
    }
}
