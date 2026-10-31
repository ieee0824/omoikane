//! Keepalive and deferred Fetch requests owned independently of a Document.

use super::*;
use crate::resource_worker::{
    ResourceKind, ResourceOutcome, ResourcePriority, ResourceScope, ResourceTask,
};

const KEEPALIVE_QUOTA: usize = 64 * 1024;

struct WorkerResponse {
    network: NetworkResponse,
    cache: PreflightCache,
}

struct Pending {
    document: usize,
    body_len: usize,
    deadline: Option<Instant>,
    request: Option<NetworkRequest>,
    scope: Option<ResourceScope>,
    task: Option<ResourceTask<WorkerResponse>>,
    report_activation: bool,
}

#[derive(Default)]
pub(in crate::js) struct BackgroundLoads {
    next_id: u64,
    reserved_bytes: usize,
    pending: HashMap<u64, Pending>,
    activated: HashSet<u64>,
}

fn submit(
    state: &mut HostState,
    request: NetworkRequest,
) -> JsResult<(ResourceScope, ResourceTask<WorkerResponse>)> {
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
                network: request.execute(&mut client, &mut cache),
                cache,
            },
        )
        .map_err(|error| JsNativeError::error().with_message(error.to_string()))?;
    Ok((scope, task))
}

fn activate(loads: &mut BackgroundLoads, state: &mut HostState, id: u64) -> JsResult<()> {
    let request = loads
        .pending
        .get_mut(&id)
        .and_then(|pending| pending.request.take());
    let Some(request) = request else {
        return Ok(());
    };
    let (scope, task) = submit(state, request)?;
    let pending = loads
        .pending
        .get_mut(&id)
        .expect("pending request remains owned");
    pending.scope = Some(scope);
    pending.task = Some(task);
    if pending.report_activation {
        loads.activated.insert(id);
    }
    Ok(())
}

fn activate_or_release(loads: &mut BackgroundLoads, state: &mut HostState, id: u64) {
    if activate(loads, state, id).is_ok() {
        return;
    }
    if let Some(pending) = loads.pending.remove(&id) {
        loads.reserved_bytes = loads.reserved_bytes.saturating_sub(pending.body_len);
    }
}

fn next_id(loads: &mut BackgroundLoads) -> JsResult<u64> {
    let id = loads
        .next_id
        .checked_add(1)
        .filter(|id| *id <= 9_007_199_254_740_991)
        .ok_or_else(|| {
            JsError::from(
                JsNativeError::error().with_message("background fetch identity exhausted"),
            )
        })?;
    loads.next_id = id;
    Ok(id)
}

fn start(
    state: &mut HostState,
    prepared: Prepared,
    body_len: usize,
    deadline: Option<Instant>,
    report_activation: bool,
) -> JsResult<u64> {
    let mut loads = std::mem::take(&mut state.background_loads);
    if loads.reserved_bytes.saturating_add(body_len) > KEEPALIVE_QUOTA {
        state.background_loads = loads;
        return Err(JsNativeError::typ()
            .with_message("keepalive request quota exceeded")
            .into());
    }
    let id = match next_id(&mut loads) {
        Ok(id) => id,
        Err(error) => {
            state.background_loads = loads;
            return Err(error);
        }
    };
    loads.reserved_bytes += body_len;
    loads.pending.insert(
        id,
        Pending {
            document: prepared.context.document_id,
            body_len,
            deadline,
            request: Some(prepared.network),
            scope: None,
            task: None,
            report_activation,
        },
    );
    if deadline.is_some_and(|deadline| deadline <= Instant::now()) {
        if let Err(error) = activate(&mut loads, state, id) {
            let pending = loads.pending.remove(&id).expect("inserted request");
            loads.reserved_bytes = loads.reserved_bytes.saturating_sub(pending.body_len);
            state.background_loads = loads;
            return Err(error);
        }
    }
    state.background_loads = loads;
    Ok(id)
}

pub(super) fn native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    if operation == "status" || operation == "cancel" {
        let id = args.get(1).and_then(JsValue::as_number).unwrap_or(0.0);
        let valid = id.is_finite() && id >= 1.0 && id.fract() == 0.0;
        if operation == "status" {
            let activated = valid
                && with_host_state(|host| {
                    Ok(host
                        .borrow()
                        .background_loads
                        .activated
                        .contains(&(id as u64)))
                })?;
            return Ok(activated.into());
        }
        let cancelled = valid
            && with_host_state(|host| {
                let mut state = host.borrow_mut();
                let loads = &mut state.background_loads;
                let can_cancel = loads
                    .pending
                    .get(&(id as u64))
                    .is_some_and(|pending| pending.request.is_some());
                if !can_cancel {
                    return Ok(false);
                }
                let pending = loads.pending.remove(&(id as u64)).expect("checked pending");
                loads.reserved_bytes = loads.reserved_bytes.saturating_sub(pending.body_len);
                Ok(true)
            })?;
        return Ok(cancelled.into());
    }
    let request_args = args.get(1..).unwrap_or_default();
    let url = string_argument(request_args.first(), "", context)?;
    let options = parse_options(url, request_args, context)?;
    let body_len = options.body_len();
    let delay = if operation == "fetchLater" {
        let value = args.get(10).cloned().unwrap_or_default();
        if value.is_undefined() {
            None
        } else {
            let milliseconds = value.to_number(context)?;
            if !milliseconds.is_finite() || milliseconds < 0.0 {
                return Err(JsNativeError::range()
                    .with_message("activateAfter must be a finite non-negative number")
                    .into());
            }
            Some(Duration::from_millis(
                milliseconds.ceil().min(u64::MAX as f64) as u64,
            ))
        }
    } else if operation == "beacon" {
        Some(Duration::ZERO)
    } else {
        return Err(JsNativeError::typ()
            .with_message("invalid background fetch operation")
            .into());
    };
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        let prepared = prepare(&mut state, options, context)?;
        if operation == "fetchLater"
            && state
                .background_loads
                .reserved_bytes
                .saturating_add(body_len)
                > KEEPALIVE_QUOTA
        {
            return Ok(false.into());
        }
        let deadline = delay.and_then(|delay| Instant::now().checked_add(delay));
        match start(
            &mut state,
            prepared,
            body_len,
            deadline,
            operation == "fetchLater",
        ) {
            Ok(_id) if operation == "beacon" => Ok(true.into()),
            Ok(id) => Ok((id as f64).into()),
            Err(_) if operation == "beacon" => Ok(false.into()),
            Err(error) => Err(error),
        }
    })
}

pub(super) fn activate_document(state: &mut HostState, document: usize) {
    let mut loads = std::mem::take(&mut state.background_loads);
    let ids = loads
        .pending
        .iter()
        .filter_map(|(id, pending)| {
            (pending.document == document && pending.request.is_some()).then_some(*id)
        })
        .collect::<Vec<_>>();
    for id in ids {
        activate_or_release(&mut loads, state, id);
    }
    state.background_loads = loads;
}

pub(super) fn poll(runtime: &mut JsRuntime) {
    let mut state = runtime.host_state.borrow_mut();
    let mut loads = std::mem::take(&mut state.background_loads);
    let now = Instant::now();
    let due = loads
        .pending
        .iter()
        .filter_map(|(id, pending)| {
            (pending.request.is_some() && pending.deadline.is_some_and(|deadline| deadline <= now))
                .then_some(*id)
        })
        .collect::<Vec<_>>();
    for id in due {
        activate_or_release(&mut loads, &mut state, id);
    }
    let finished = loads
        .pending
        .iter()
        .filter_map(|(id, pending)| {
            let task = pending.task.as_ref()?;
            match task.try_recv() {
                Ok(outcome) => Some((*id, outcome)),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(_) => Some((*id, ResourceOutcome::Panicked)),
            }
        })
        .collect::<Vec<_>>();
    for (id, outcome) in finished {
        if let ResourceOutcome::Complete(response) = outcome {
            state
                .cors_preflight_cache
                .merge(response.cache, Instant::now());
            let _ = response.network;
        }
        if let Some(pending) = loads.pending.remove(&id) {
            loads.reserved_bytes = loads.reserved_bytes.saturating_sub(pending.body_len);
        }
    }
    state.background_loads = loads;
}
