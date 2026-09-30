//! Web Locks: the per-thread client registry, lock requests from page script,
//! and delivery of grant/steal notifications to the owning runtime.

use super::*;

thread_local! {
    static WEB_LOCK_CLIENT_REGISTRY: RefCell<HashMap<u64, (Weak<RefCell<HostState>>, usize)>> =
        RefCell::new(HashMap::new());
    static PENDING_WEB_LOCK_NOTIFICATIONS: RefCell<Vec<WebLockNotification>> =
        const { RefCell::new(Vec::new()) };
}

/// Removes a client from the thread registry and queues the notifications its
/// removal produces for other clients waiting on the same locks.
pub(super) fn unregister_web_lock_client(manager: &StorageManager, client_id: u64) {
    let _ = WEB_LOCK_CLIENT_REGISTRY.try_with(|registry| {
        registry.borrow_mut().remove(&client_id);
    });
    queue_web_lock_notifications(manager.remove_web_lock_client(client_id));
}

pub(super) fn register_web_lock_client(
    host_state: &Rc<RefCell<HostState>>,
    document_id: usize,
    client_id: u64,
) {
    let _ = WEB_LOCK_CLIENT_REGISTRY.try_with(|registry| {
        registry
            .borrow_mut()
            .insert(client_id, (Rc::downgrade(host_state), document_id));
    });
}

fn queue_web_lock_notifications(notifications: Vec<WebLockNotification>) {
    let _ = PENDING_WEB_LOCK_NOTIFICATIONS
        .try_with(|pending| pending.borrow_mut().extend(notifications));
}

pub(super) fn flush_web_lock_notifications() {
    let Ok(notifications) = PENDING_WEB_LOCK_NOTIFICATIONS
        .try_with(|pending| std::mem::take(&mut *pending.borrow_mut()))
    else {
        return;
    };
    let mut retry = Vec::new();
    for notification in notifications {
        let registration = WEB_LOCK_CLIENT_REGISTRY
            .try_with(|registry| registry.borrow().get(&notification.client_id).cloned())
            .ok()
            .flatten();
        let Some((host, document_id)) = registration else {
            continue;
        };
        let Some(host) = host.upgrade() else {
            continue;
        };
        let Ok(mut state) = host.try_borrow_mut() else {
            retry.push(notification);
            continue;
        };
        state.event_loop.enqueue_web_lock(
            document_id,
            notification.request_id,
            notification.kind == WebLockNotificationKind::Stolen,
        );
    }
    if !retry.is_empty() {
        let _ =
            PENDING_WEB_LOCK_NOTIFICATIONS.try_with(|pending| pending.borrow_mut().extend(retry));
    }
}

fn web_lock_context(context: &Context) -> JsResult<(usize, u64, StorageManager, StorageOrigin)> {
    with_host_state(|host| {
        let document_id = {
            let state = host.borrow();
            context_document_id(context, &state)
        };
        let (client_id, created, manager, origin) = {
            let mut state = host.borrow_mut();
            if !document_is_secure_context(&state, document_id) {
                return Err(JsNativeError::error()
                    .with_message("Web Locks requires a secure context")
                    .into());
            }
            let origin = state
                .document_origins
                .get(&document_id)
                .cloned()
                .flatten()
                .ok_or_else(|| {
                    JsError::from(
                        JsNativeError::error().with_message("Web Locks requires a tuple origin"),
                    )
                })?;
            let manager = state.storage_manager.clone();
            let (client_id, created) = match state.web_lock_clients.get(&document_id).copied() {
                Some(client_id) => (client_id, false),
                None => {
                    let client_id = manager.create_web_lock_client();
                    state.web_lock_clients.insert(document_id, client_id);
                    (client_id, true)
                }
            };
            (client_id, created, manager, origin)
        };
        if created {
            register_web_lock_client(host, document_id, client_id);
        }
        Ok((document_id, client_id, manager, origin))
    })
}

fn web_locks_native(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    if operation == "available" {
        return with_host_state(|host| {
            let state = host.borrow();
            let document_id = context_document_id(context, &state);
            Ok(JsValue::from(
                document_is_secure_context(&state, document_id)
                    && state
                        .document_origins
                        .get(&document_id)
                        .is_some_and(Option::is_some),
            ))
        });
    }

    let (_, client_id, manager, origin) = web_lock_context(context)?;
    let request_id = |value: Option<&JsValue>, context: &mut Context| -> JsResult<u64> {
        string_argument(value, "", context)?
            .parse::<u64>()
            .map_err(|_| {
                JsNativeError::typ()
                    .with_message("invalid Web Lock request id")
                    .into()
            })
    };
    match operation.as_str() {
        "request" => {
            let name = string_argument(args.get(1), "", context)?;
            let mode = match string_argument(args.get(2), "exclusive", context)?.as_str() {
                "shared" => WebLockMode::Shared,
                "exclusive" => WebLockMode::Exclusive,
                _ => {
                    return Err(JsNativeError::typ()
                        .with_message("invalid Web Lock mode")
                        .into());
                }
            };
            let if_available = args.get(3).is_some_and(JsValue::to_boolean);
            let steal = args.get(4).is_some_and(JsValue::to_boolean);
            let (result, notifications) =
                manager.request_web_lock(&origin, client_id, name, mode, if_available, steal);
            queue_web_lock_notifications(notifications);
            flush_web_lock_notifications();
            let (status, id) = match result {
                WebLockRequestResult::Granted(id) => ("granted", Some(id)),
                WebLockRequestResult::Pending(id) => ("pending", Some(id)),
                WebLockRequestResult::Unavailable => ("unavailable", None),
            };
            Ok(js_string!(
                serde_json::json!({"status": status, "id": id.map(|id| id.to_string())})
                    .to_string()
            )
            .into())
        }
        "start" => {
            let status = match manager.start_web_lock(request_id(args.get(1), context)?) {
                WebLockStartResult::Held => "held",
                WebLockStartResult::Pending => "pending",
                WebLockStartResult::Stolen => "stolen",
                WebLockStartResult::Missing => "missing",
            };
            Ok(js_string!(status).into())
        }
        "release" => {
            let notifications = manager.release_web_lock(request_id(args.get(1), context)?);
            queue_web_lock_notifications(notifications);
            flush_web_lock_notifications();
            Ok(JsValue::undefined())
        }
        "cancel" => {
            let (cancelled, notifications) =
                manager.cancel_web_lock(request_id(args.get(1), context)?);
            queue_web_lock_notifications(notifications);
            flush_web_lock_notifications();
            Ok(JsValue::from(cancelled))
        }
        "finish-stolen" => {
            manager.finish_stolen_web_lock(request_id(args.get(1), context)?);
            Ok(JsValue::undefined())
        }
        "query" => {
            let (held, pending) = manager.query_web_locks(&origin);
            let snapshot = |lock: storage::WebLockSnapshot| {
                serde_json::json!({
                    "name": lock.name,
                    "mode": lock.mode.as_str(),
                    "clientId": format!("client-{}", lock.client_id),
                })
            };
            Ok(js_string!(
                serde_json::json!({
                    "held": held.into_iter().map(snapshot).collect::<Vec<_>>(),
                    "pending": pending.into_iter().map(snapshot).collect::<Vec<_>>(),
                })
                .to_string()
            )
            .into())
        }
        _ => Err(JsNativeError::typ()
            .with_message(format!("unknown Web Locks operation: {operation}"))
            .into()),
    }
}

/// Registers the private Web Locks hook used by the DOM bootstrap.
pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    for (name, length, function) in [(
        js_string!("__omoikane_web_locks"),
        5,
        NativeFunction::from_copy_closure(web_locks_native),
    )] {
        register_private_builtin_callable(context, bindings, name, length, function)?;
    }
    Ok(())
}
