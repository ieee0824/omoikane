//! Resource workers own transport; callbacks and their Realms stay on the owner.
use super::*;
use crate::resource_worker::{
    ResourceKind, ResourceOutcome, ResourcePriority, ResourceScope, ResourceTask,
};

#[cfg(test)]
mod tests;

struct WorkerResponse {
    network: NetworkResponse,
    cache: PreflightCache,
}

#[derive(Clone)]
struct Callbacks {
    loaded: TimerPayload,
    failed: TimerPayload,
}

struct Pending {
    context: ResponseContext,
    realm: Realm,
    callbacks: Callbacks,
    scope: ResourceScope,
    task: ResourceTask<WorkerResponse>,
}

#[derive(Default)]
pub(in crate::js) struct FetchLoads {
    next_id: u64,
    pending: HashMap<u64, Pending>,
}

impl FetchLoads {
    pub(in crate::js) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(in crate::js) fn remove_document(&mut self, document: usize) {
        self.pending
            .retain(|_, pending| pending.context.document_id != document);
    }

    pub(in crate::js) unsafe fn trace_pending(&self, tracer: &mut Tracer) {
        for pending in self.pending.values() {
            unsafe { event_loop::trace_timer_payload(&pending.callbacks.loaded, tracer) };
            unsafe { event_loop::trace_timer_payload(&pending.callbacks.failed, tracer) };
        }
    }
}

fn bind_callback(callback: JsValue, realm: &Realm, document: usize, main: usize) -> TimerPayload {
    let payload = TimerPayload::Callback {
        callback,
        args: Vec::new(),
    };
    if document == main {
        payload
    } else {
        TimerPayload::Realm {
            payload: Box::new(payload),
            realm: realm.clone(),
            document_id: document,
        }
    }
}

pub(super) fn begin(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let enabled = with_host_state(|host| Ok(host.borrow().nonblocking_fetch_loading))?;
    if args.is_empty() || !enabled {
        return Ok(enabled.into());
    }
    let url = string_argument(args.first(), "undefined", context)?;
    let options = parse_options(url, args, context)?;
    let callbacks = args.get(9).and_then(JsValue::as_object).ok_or_else(|| {
        JsError::from(JsNativeError::typ().with_message("invalid fetch completion callbacks"))
    })?;
    let loaded = callbacks.get(0_u32, context)?;
    let failed = callbacks.get(1_u32, context)?;
    if !loaded.is_callable() || !failed.is_callable() {
        return Err(JsNativeError::typ()
            .with_message("invalid fetch completion callbacks")
            .into());
    }
    let realm = context.realm().clone();
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        let prepared = prepare(&mut state, options, context)?;
        let document = prepared.context.document_id;
        let id = state
            .fetch_loads
            .next_id
            .checked_add(1)
            .filter(|id| *id <= 9_007_199_254_740_991)
            .ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("fetch identity exhausted"))
            })?;
        let mut client = state.http_client.fork_for_worker();
        client.set_shared_cookie_store(Arc::clone(&state.cookie_store));
        let mut cache = state.cors_preflight_cache.clone();
        let scope = ResourceScope::default();
        let task = state
            .resource_queue
            .submit(
                &scope,
                ResourcePriority::Critical,
                ResourceKind::Other,
                move |_| WorkerResponse {
                    network: prepared.network.execute(&mut client, &mut cache),
                    cache,
                },
            )
            .map_err(|error| {
                JsError::from(JsNativeError::error().with_message(error.to_string()))
            })?;
        let main = state.document.identity();
        let callbacks = Callbacks {
            loaded: bind_callback(loaded, &realm, document, main),
            failed: bind_callback(failed, &realm, document, main),
        };
        state.fetch_loads.next_id = id;
        state.fetch_loads.pending.insert(
            id,
            Pending {
                context: prepared.context,
                realm,
                callbacks,
                scope,
                task,
            },
        );
        Ok((id as f64).into())
    })
}

pub(super) fn cancel(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.first().and_then(JsValue::as_number).unwrap_or(0.0);
    if !id.is_finite() || id < 1.0 || id.fract() != 0.0 {
        return Ok(JsValue::undefined());
    }
    let document = with_host_state(|host| {
        Ok(host
            .borrow()
            .fetch_loads
            .pending
            .get(&(id as u64))
            .map(|pending| pending.context.document_id))
    })?;
    if let Some(document) = document {
        ensure_same_origin_document(context, document)?;
        with_host_state(|host| {
            host.borrow_mut().fetch_loads.pending.remove(&(id as u64));
            Ok(())
        })?;
    }
    Ok(JsValue::undefined())
}

fn add_argument(payload: &mut TimerPayload, argument: JsValue) {
    match payload {
        TimerPayload::Callback { args, .. } => args.push(argument),
        TimerPayload::Realm { payload, .. } => add_argument(payload, argument),
        _ => unreachable!("fetch completion is a bound callback"),
    }
}

pub(super) fn poll(runtime: &mut JsRuntime) {
    let _active = activate_host_state(Rc::clone(&runtime.host_state));
    let finished: Vec<_> = runtime
        .host_state
        .borrow()
        .fetch_loads
        .pending
        .iter()
        .filter_map(|(id, pending)| match pending.task.try_recv() {
            Ok(outcome) => Some((*id, outcome)),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => Some((*id, ResourceOutcome::Panicked)),
        })
        .collect();
    for (id, outcome) in finished {
        deliver(runtime, id, outcome);
    }
}

fn deliver(runtime: &mut JsRuntime, id: u64, outcome: ResourceOutcome<WorkerResponse>) {
    let (metadata, realm) = {
        let state = runtime.host_state.borrow();
        let Some(pending) = state.fetch_loads.pending.get(&id) else {
            return;
        };
        (pending.context.clone(), pending.realm.clone())
    };
    let document = metadata.document_id;
    let result = {
        let mut state = runtime.host_state.borrow_mut();
        if !state.document_is_active(document) {
            state.fetch_loads.pending.remove(&id);
            return;
        }
        match outcome {
            ResourceOutcome::Complete(response) => {
                state
                    .cors_preflight_cache
                    .merge(response.cache, Instant::now());
                apply(&mut state, metadata, response.network)
            }
            _ => Err(JsNativeError::typ()
                .with_message("fetch worker failed or was cancelled")
                .into()),
        }
    };
    // Error construction may collect: release the HostState borrow and keep
    // both callbacks in its traced pending table until delivery is queued.
    let previous = runtime.context.enter_realm(realm);
    let (loaded, value) = match result {
        Ok(payload) => (true, js_string!(payload.as_str()).into()),
        Err(error) => (false, error.to_opaque(&mut runtime.context)),
    };
    runtime.context.enter_realm(previous);
    let mut state = runtime.host_state.borrow_mut();
    if let Some(pending) = state.fetch_loads.pending.get(&id) {
        let mut payload = if loaded {
            pending.callbacks.loaded.clone()
        } else {
            pending.callbacks.failed.clone()
        };
        add_argument(&mut payload, value);
        state
            .event_loop
            .enqueue_networking_for_document(payload, document);
    }
    state.fetch_loads.pending.remove(&id);
}
