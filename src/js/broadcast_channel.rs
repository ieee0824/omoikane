//! `BroadcastChannel`: same-origin channel registration and message delivery
//! between runtimes on the owning thread.

use super::*;

thread_local! {
    /// Same-thread registry for page-owned BroadcastChannel endpoints.
    ///
    /// JavaScript runtimes are intentionally !Send and all host callbacks run
    /// on their owning thread, so a thread-local registry lets independent
    /// runtimes exchange context-independent clone wires without ever moving a
    /// Boa `JsValue` across realms (or introducing a global lock into the hot
    /// posted-message path).
    static BROADCAST_CHANNEL_REGISTRY: RefCell<Vec<BroadcastChannelRegistration>> =
        const { RefCell::new(Vec::new()) };
}

#[derive(Debug, Clone)]
pub(super) struct BroadcastChannelMetadata {
    name: String,
    origin: Option<StorageOrigin>,
    /// Serialized origin used for `MessageEvent.origin`.  This is captured at
    /// construction, matching the environment settings object that created
    /// the channel even if the host later updates its base URL.
    origin_text: String,
}

#[derive(Debug)]
struct BroadcastChannelRegistration {
    host_state: Weak<RefCell<HostState>>,
    channel_id: u64,
    name: String,
    origin: StorageOrigin,
}

/// Returns the tuple origin captured by a newly-created BroadcastChannel.
///
/// Opaque/non-HTTP locations have no `StorageOrigin` and therefore cannot
/// participate in a same-origin broadcast group.  The channel itself still
/// constructs successfully; its messages simply have no eligible recipients.
fn broadcast_channel_origin(state: &HostState) -> Option<StorageOrigin> {
    state
        .base_url
        .as_ref()
        .and_then(|url| StorageOrigin::from_url(&url.to_string()))
}

fn prune_broadcast_channel_registry(registry: &mut Vec<BroadcastChannelRegistration>) {
    registry.retain(|entry| entry.host_state.strong_count() > 0);
}

fn unregister_broadcast_channel(state: &Rc<RefCell<HostState>>, channel_id: u64) {
    BROADCAST_CHANNEL_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.retain(|entry| {
            if entry.channel_id != channel_id {
                return true;
            }
            entry
                .host_state
                .upgrade()
                .is_none_or(|registered| !Rc::ptr_eq(&registered, state))
        });
        prune_broadcast_channel_registry(&mut registry);
    });
}

fn register_broadcast_channel(
    state: &Rc<RefCell<HostState>>,
    channel_id: u64,
    metadata: &BroadcastChannelMetadata,
) {
    let Some(origin) = metadata.origin.clone() else {
        return;
    };
    BROADCAST_CHANNEL_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        prune_broadcast_channel_registry(&mut registry);
        registry.push(BroadcastChannelRegistration {
            host_state: Rc::downgrade(state),
            channel_id,
            name: metadata.name.clone(),
            origin,
        });
    });
}

/// Enqueues a context-independent clone wire for every eligible channel in
/// the sender's same-origin/name group.  The sender endpoint itself is
/// deliberately excluded, while another endpoint in the same runtime still
/// receives the message just like an endpoint in another runtime.
fn post_broadcast_channel(sender_state: &Rc<RefCell<HostState>>, channel_id: u64, data: String) {
    let Some(metadata) = sender_state
        .borrow()
        .broadcast_channel_metadata
        .get(&channel_id)
        .cloned()
    else {
        return;
    };
    let Some(origin) = metadata.origin.clone() else {
        return;
    };
    let sender_origin = metadata.origin_text;
    let destinations = BROADCAST_CHANNEL_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        prune_broadcast_channel_registry(&mut registry);
        registry
            .iter()
            .filter(|entry| entry.name == metadata.name && entry.origin == origin)
            .filter_map(|entry| {
                let target = entry.host_state.upgrade()?;
                if entry.channel_id == channel_id && Rc::ptr_eq(&target, sender_state) {
                    return None;
                }
                Some((target, entry.channel_id))
            })
            .collect::<Vec<_>>()
    });

    for (target, target_id) in destinations {
        target
            .borrow_mut()
            .event_loop
            .enqueue_broadcast_channel_message(target_id, data.clone(), sender_origin.clone());
    }
}

fn broadcast_channel_id_argument(value: Option<&JsValue>, context: &mut Context) -> JsResult<u64> {
    let value = value.cloned().unwrap_or_default();
    if let Some(string) = value.as_string() {
        return string.to_std_string_escaped().parse::<u64>().map_err(|_| {
            JsNativeError::typ()
                .with_message("invalid BroadcastChannel id")
                .into()
        });
    }
    let number = value.to_number(context)?;
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 {
        return Err(JsNativeError::typ()
            .with_message("invalid BroadcastChannel id")
            .into());
    }
    Ok(number as u64)
}

/// Allocates a per-runtime BroadcastChannel id and records the endpoint
/// reference in that runtime's target table.  Modern callers pass a `WeakRef`
/// so this native table does not extend the channel's lifetime; realms without
/// WeakRef pass the endpoint itself as a strong compatibility fallback.
fn broadcast_channel_register_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let name = string_argument(args.first(), "", context)?;
    let endpoint = args.get(1).cloned().unwrap_or_default();
    if !endpoint.is_object() {
        return Err(JsNativeError::typ()
            .with_message("BroadcastChannel endpoint reference must be an object")
            .into());
    }
    with_host_state(|state| {
        let (channel_id, metadata) = {
            let mut host = state.borrow_mut();
            let channel_id = host.next_broadcast_channel_id;
            host.next_broadcast_channel_id = host.next_broadcast_channel_id.saturating_add(1);
            let metadata = BroadcastChannelMetadata {
                name,
                origin: broadcast_channel_origin(&host),
                origin_text: host_state_origin(&host),
            };
            host.broadcast_channels.insert(channel_id, endpoint);
            host.broadcast_channel_metadata
                .insert(channel_id, metadata.clone());
            (channel_id, metadata)
        };
        register_broadcast_channel(state, channel_id, &metadata);
        Ok(JsValue::from(js_string!(channel_id.to_string())))
    })
}

/// Posts a previously structured-cloned wire to every same-origin channel
/// with the same name.  The sender's own endpoint is excluded by the native
/// registry, while sibling endpoints in this runtime are treated exactly like
/// endpoints in another runtime.
pub(super) fn broadcast_channel_post_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let channel_id = broadcast_channel_id_argument(args.first(), context)?;
    let data = string_argument(args.get(1), "", context)?;
    with_host_state(|state| {
        post_broadcast_channel(state, channel_id, data);
        Ok(JsValue::undefined())
    })
}

fn broadcast_channel_close_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let channel_id = broadcast_channel_id_argument(args.first(), context)?;
    with_host_state(|state| {
        unregister_broadcast_channel(state, channel_id);
        let mut host = state.borrow_mut();
        host.broadcast_channels.remove(&channel_id);
        host.broadcast_channel_metadata.remove(&channel_id);
        Ok(JsValue::undefined())
    })
}

/// Registers the private BroadcastChannel hooks used by the DOM bootstrap.
pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    for (name, length, function) in [
        (
            js_string!("__omoikane_broadcast_channel_register"),
            2,
            NativeFunction::from_copy_closure(broadcast_channel_register_native),
        ),
        (
            js_string!("__omoikane_broadcast_channel_post"),
            2,
            NativeFunction::from_copy_closure(broadcast_channel_post_native),
        ),
        (
            js_string!("__omoikane_broadcast_channel_close"),
            1,
            NativeFunction::from_copy_closure(broadcast_channel_close_native),
        ),
    ] {
        register_private_builtin_callable(context, bindings, name, length, function)?;
    }
    Ok(())
}

impl JsRuntime {
    pub(super) fn run_broadcast_channel_message(
        &mut self,
        channel_id: u64,
        data: String,
        origin: String,
    ) -> JsResult<()> {
        let channel = self
            .host_state
            .borrow()
            .broadcast_channels
            .get(&channel_id)
            .cloned();
        let Some(channel) = channel else {
            // The endpoint was closed (or the runtime was torn down) after
            // the task was queued.  Closing drops queued messages silently.
            return Ok(());
        };
        if let Err(error) = self.install_broadcast_channel_values(channel, channel_id, data, origin)
        {
            self.record_task_error(format!("[broadcast channel setup] {error}"));
            let cleanup_result = self.clear_broadcast_channel_values();
            self.record_error_from("broadcast channel cleanup", cleanup_result);
            return Ok(());
        }
        let result = self.eval(
            "const __omoikane_broadcast_channel_ref_value = __omoikane_broadcast_channel_ref; const __omoikane_broadcast_channel_target = (__omoikane_broadcast_channel_ref_value && typeof __omoikane_broadcast_channel_ref_value.deref === 'function') ? __omoikane_broadcast_channel_ref_value.deref() : __omoikane_broadcast_channel_ref_value; if (__omoikane_broadcast_channel_target && !__omoikane_broadcast_channel_target._closed) { \
             let __omoikane_broadcast_channel_decoded; \
             let __omoikane_broadcast_channel_decoded_ok = false; \
             try { __omoikane_broadcast_channel_decoded = __omoikane_decode_worker_message(__omoikane_broadcast_channel_wire); __omoikane_broadcast_channel_decoded_ok = true; } \
             catch (error) { \
               __omoikane_broadcast_channel_target.dispatchEvent(new MessageEvent('messageerror', { \
                 data: null, origin: __omoikane_broadcast_channel_origin, source: null, ports: [] \
               })); \
             } \
             if (__omoikane_broadcast_channel_decoded_ok) { \
               __omoikane_broadcast_channel_target.dispatchEvent(new MessageEvent('message', { \
                 data: __omoikane_broadcast_channel_decoded, origin: __omoikane_broadcast_channel_origin, source: null, ports: [] \
               })); \
             } \
             } !__omoikane_broadcast_channel_target",
        );
        if result.as_ref().ok().and_then(JsValue::as_boolean) == Some(true) {
            unregister_broadcast_channel(&self.host_state, channel_id);
            let mut state = self.host_state.borrow_mut();
            state.broadcast_channels.remove(&channel_id);
            state.broadcast_channel_metadata.remove(&channel_id);
        }
        let cleanup_result = self.clear_broadcast_channel_values();
        self.record_error_from("broadcast channel cleanup", cleanup_result);
        self.record_error_from("broadcast channel", result);
        Ok(())
    }

    fn install_broadcast_channel_values(
        &mut self,
        channel: JsValue,
        channel_id: u64,
        data: String,
        origin: String,
    ) -> JsResult<()> {
        let global = self.context.global_object();
        global.set(
            js_string!("__omoikane_broadcast_channel_ref"),
            channel,
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_broadcast_channel_id"),
            JsValue::from(js_string!(channel_id.to_string())),
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_broadcast_channel_wire"),
            JsValue::from(js_string!(data)),
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_broadcast_channel_origin"),
            JsValue::from(js_string!(origin)),
            true,
            &mut self.context,
        )?;
        Ok(())
    }

    fn clear_broadcast_channel_values(&mut self) -> JsResult<()> {
        let global = self.context.global_object();
        let target_result = global.set(
            js_string!("__omoikane_broadcast_channel_ref"),
            JsValue::undefined(),
            true,
            &mut self.context,
        );
        let wire_result = global.set(
            js_string!("__omoikane_broadcast_channel_wire"),
            JsValue::undefined(),
            true,
            &mut self.context,
        );
        let id_result = global.set(
            js_string!("__omoikane_broadcast_channel_id"),
            JsValue::undefined(),
            true,
            &mut self.context,
        );
        let origin_result = global.set(
            js_string!("__omoikane_broadcast_channel_origin"),
            JsValue::undefined(),
            true,
            &mut self.context,
        );
        target_result?;
        id_result?;
        wire_result?;
        origin_result?;
        Ok(())
    }
}
