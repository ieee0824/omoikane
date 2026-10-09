//! Receiver identity for legacy Window events, including live WindowProxy views.
use super::*;
use boa_gc::Rooted;

/// Weak keys prevent the runtime's registry from retaining detached proxies.
/// Captured native methods avoid reading author-modifiable WeakMap prototypes.
#[derive(Clone, Trace, Finalize)]
pub(super) struct ProxyRegistry {
    map: JsObject,
    get: JsObject,
    set: JsObject,
}

/// The Realm owns its constructor closure; retiring a Realm releases the edge.
#[derive(Debug, Clone, Trace, Finalize, boa_engine::JsData)]
struct PlatformEventFactory {
    factory: JsObject,
    interface_brand: JsObject,
}

pub(super) fn register(
    context: &mut Context,
    host: &Rc<RefCell<HostState>>,
    bindings: &mut BootstrapBindings,
) -> JsResult<()> {
    if host.borrow().window_proxy_registry.is_none() {
        let constructor = context.intrinsics().constructors().weak_map().constructor();
        let map = constructor.construct(&[], None, context)?;
        let _root = Rooted::new(map.clone());
        let prototype = context.intrinsics().constructors().weak_map().prototype();
        let get = prototype
            .get(js_string!("get"), context)?
            .as_callable()
            .expect("WeakMap.get");
        let set = prototype
            .get(js_string!("set"), context)?
            .as_callable()
            .expect("WeakMap.set");
        host.borrow_mut().window_proxy_registry = Some(ProxyRegistry { map, get, set });
    }
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_register_window_proxy"),
        3,
        NativeFunction::from_copy_closure(register_proxy_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_document_window"),
        1,
        NativeFunction::from_copy_closure(document_window_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_window_event_document"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            Ok(JsValue::from(
                receiver_document_id(args.first(), context)? as f64
            ))
        }),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_window_proxy_global"),
        1,
        NativeFunction::from_copy_closure(window_proxy_global_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_retained_window_global"),
        1,
        NativeFunction::from_copy_closure(retained_window_global_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_register_platform_event_factory"),
        2,
        NativeFunction::from_copy_closure(register_event_factory_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_platform_interface_brand"),
        2,
        NativeFunction::from_copy_closure(platform_interface_brand_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_iframe_platform_event"),
        3,
        NativeFunction::from_copy_closure(iframe_platform_event_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_iframe_nodes_in_subtree"),
        1,
        NativeFunction::from_copy_closure(iframe_nodes_in_subtree_native),
    )
}

/// Returns only a retained Realm's actual Window global. Listener placeholders
/// belong to the creator Realm and must not stand in for a retired child Window.
fn retained_window_global_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let Some(window) = args.first().and_then(JsValue::as_object) else {
        return Ok(JsValue::null());
    };
    let Some(realm) = window.associated_realm() else {
        return Ok(JsValue::null());
    };
    let previous = context.enter_realm(realm.clone());
    let global = context.global_object();
    context.enter_realm(previous);
    if !JsObject::equals(&window, &global) {
        return Ok(JsValue::null());
    }
    let Some(document) = realm
        .host_defined()
        .get::<ModuleDocumentId>()
        .map(|value| value.0)
    else {
        return Ok(JsValue::null());
    };
    // Keep the caller Realm restored before checking the retained Document.
    if same_origin_document(context, document)? {
        Ok(window.into())
    } else {
        Ok(JsValue::null())
    }
}

/// Resolves a child or popup Document's WindowProxy in its creator's Realm.
/// The caller can access its own Document without being allowed to inspect
/// the cross-origin frame element that embeds it. The private owner callback
/// creates or retrieves the proxy without exposing that element to the caller.
fn document_window_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document)?;
    window_for_document(document, context)
}

/// Trusted initialization lookup, also used after the public binding checks
/// access to the Document. It never exposes its owner frame to page code.
pub(super) fn window_for_document(document: usize, context: &mut Context) -> JsResult<JsValue> {
    let owner = with_host_state(|host| {
        let state = host.borrow();
        let frame = state
            .iframe_documents
            .iter()
            .find_map(|(frame, entry)| (entry.document.identity() == document).then_some(*frame));
        let target = if let Some(frame) = frame {
            let owner = state
                .get_node(frame)
                .as_ref()
                .and_then(owner_document_for_node)
                .ok_or_else(|| JsNativeError::typ().with_message("iframe has no owner Document"))?;
            Some((frame as f64, owner.identity(), "window"))
        } else {
            state.auxiliary_contexts.iter().find_map(|(id, entry)| {
                (entry.document.identity() == document).then_some((
                    *id as f64,
                    entry.opener_document_id,
                    "auxiliary-window",
                ))
            })
        };
        let Some((id, owner, kind)) = target else {
            return Ok(None);
        };
        let callback = state.iframe_navigation.owner(owner).ok_or_else(|| {
            JsNativeError::typ().with_message("browsing context owner is no longer active")
        })?;
        Ok(Some((id, callback, kind)))
    })?;
    let Some((id, callback, kind)) = owner else {
        return Ok(JsValue::null());
    };
    callback
        .as_callable()
        .expect("registered browsing context handler")
        .call(
            &JsValue::undefined(),
            &[JsValue::from(id), JsValue::null(), js_string!(kind).into()],
            context,
        )
}

/// Snapshots frame identities in light-tree order without allocating JS views
/// for unrelated descendants. This does not load or retire any browsing context.
fn iframe_nodes_in_subtree_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let root = with_host_state(|host| Ok(host.borrow().get_node(node_id)))?;
    let mut frames = Vec::new();
    let mut pending: Vec<_> = root.into_iter().collect();
    while let Some(node) = pending.pop() {
        match node.node_type() {
            NodeType::Element => {
                let tag = node.local_name().unwrap_or_default();
                if tag.eq_ignore_ascii_case("iframe") || tag.eq_ignore_ascii_case("frame") {
                    frames.push(JsValue::new(node.identity() as f64));
                }
                pending.extend(node.child_nodes().into_iter().rev());
            }
            NodeType::Document | NodeType::DocumentFragment => {
                pending.extend(node.child_nodes().into_iter().rev());
            }
            _ => {}
        }
    }
    Ok(boa_engine::object::builtins::JsArray::from_iter(frames, context).into())
}

fn register_event_factory_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let factory = args
        .first()
        .and_then(JsValue::as_callable)
        .ok_or_else(|| JsNativeError::typ().with_message("Expected an event factory"))?;
    let interface_brand = args
        .get(1)
        .and_then(JsValue::as_callable)
        .ok_or_else(|| JsNativeError::typ().with_message("Expected an interface brand resolver"))?;
    context
        .realm()
        .host_defined_mut()
        .insert(PlatformEventFactory {
            factory: factory.clone(),
            interface_brand: interface_brand.clone(),
        });
    Ok(JsValue::undefined())
}

/// Uses the creation Realm's private brand records, never page-visible
/// prototype chains. WindowProxy identity is shared across bootstrap Realms.
fn platform_interface_brand_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let value = args.first().cloned().unwrap_or_default();
    let kind = args.get(1).cloned().unwrap_or_default();
    let Some(object) = value.as_object() else {
        return Ok(false.into());
    };
    if kind
        .as_string()
        .is_some_and(|name| name == js_string!("EventTarget"))
    {
        let registry = with_host_state(|host| Ok(host.borrow().window_proxy_registry.clone()))?
            .expect("WindowProxy registry initialized before bootstrap");
        let target = registry
            .get
            .call(&registry.map.clone().into(), &[value.clone()], context)?;
        if !target.is_undefined() {
            return Ok(true.into());
        }
    }
    let Some(realm) = object.associated_realm() else {
        return Ok(false.into());
    };
    let resolver = realm
        .host_defined()
        .get::<PlatformEventFactory>()
        .map(|entry| entry.interface_brand.clone());
    match resolver {
        Some(resolver) => resolver.call(&JsValue::undefined(), &[value, kind], context),
        None => Ok(false.into()),
    }
}

fn iframe_platform_event_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let window = iframe_global_native(
        &JsValue::undefined(),
        &[args.first().cloned().unwrap_or_default()],
        context,
    )?;
    let realm = window
        .as_object()
        .and_then(|window| window.associated_realm())
        .ok_or_else(|| JsNativeError::typ().with_message("Window is unavailable"))?;
    let factory = realm
        .host_defined()
        .get::<PlatformEventFactory>()
        .ok_or_else(|| JsNativeError::typ().with_message("Window event factory is unavailable"))?
        .factory
        .clone();
    factory.call(
        &window,
        &[
            args.get(1).cloned().unwrap_or_default(),
            args.get(2).cloned().unwrap_or_default(),
        ],
        context,
    )
}

fn register_proxy_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let proxy = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("WindowProxy must be an object"))?;
    let target = if args.get(2).and_then(JsValue::as_boolean).unwrap_or(false) {
        js_string!(parse_node_id(args.get(1), context)?.to_string()).into()
    } else {
        JsValue::new(parse_node_id(args.get(1), context)? as f64)
    };
    let registry = with_host_state(|host| Ok(host.borrow().window_proxy_registry.clone()))?
        .expect("WindowProxy registry initialized before bootstrap");
    registry.set.call(
        &registry.map.clone().into(),
        &[proxy.into(), target],
        context,
    )?;
    Ok(JsValue::undefined())
}

/// Resolves a branded receiver without looking up page-visible properties.
/// Proxy bindings retain frame IDs so navigation selects the current Document.
pub(super) fn receiver_document_id(
    receiver: Option<&JsValue>,
    context: &mut Context,
) -> JsResult<usize> {
    let document = resolve_receiver_document_id(receiver, context)?;
    ensure_same_origin_document(context, document)?;
    Ok(document)
}

fn resolve_receiver_document_id(
    receiver: Option<&JsValue>,
    context: &mut Context,
) -> JsResult<usize> {
    let receiver = match receiver {
        None => context.global_object(),
        Some(value) if value.is_null_or_undefined() => context.global_object(),
        Some(value) => value
            .as_object()
            .ok_or_else(|| JsNativeError::typ().with_message("Illegal invocation"))?,
    };
    let registry = with_host_state(|host| Ok(host.borrow().window_proxy_registry.clone()))?
        .expect("WindowProxy registry initialized before bootstrap");
    let target = registry.get.call(
        &registry.map.clone().into(),
        &[receiver.clone().into()],
        context,
    )?;
    let document_id = if let Some(frame) = target.as_number() {
        let frame = frame as usize;
        // A genuine WindowProxy can refer to the caller's own child Window
        // even when its embedding element belongs to a cross-origin parent.
        // Resolve the browsing context here; the Document-origin check below
        // protects the Window's slots without requiring DOM access to its frame.
        with_host_state(|host| {
            let mut state = host.borrow_mut();
            let node = state
                .get_node(frame)
                .filter(|node| state.node_is_in_active_document(node))
                .ok_or_else(|| JsNativeError::typ().with_message("WindowProxy is closed"))?;
            let document = state
                .iframe_content_document(&node)
                .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
            Ok(document.identity())
        })?
    } else if let Some(auxiliary) = target.as_string() {
        let auxiliary = auxiliary
            .to_std_string_escaped()
            .parse::<u64>()
            .map_err(|_| JsNativeError::typ().with_message("Invalid auxiliary context id"))?;
        with_host_state(|host| {
            host.borrow()
                .auxiliary_contexts
                .get(&auxiliary)
                .map(|entry| entry.document.identity())
                .ok_or_else(|| {
                    JsNativeError::typ()
                        .with_message("WindowProxy is closed")
                        .into()
                })
        })?
    } else {
        let realm = receiver
            .associated_realm()
            .ok_or_else(|| JsNativeError::typ().with_message("Illegal invocation"))?;
        let document_id = realm
            .host_defined()
            .get::<ModuleDocumentId>()
            .ok_or_else(|| JsNativeError::typ().with_message("Illegal invocation"))?
            .0;
        let previous = context.enter_realm(realm);
        let global = context.global_object();
        context.enter_realm(previous);
        if receiver != global {
            return Err(JsNativeError::typ()
                .with_message("Illegal invocation")
                .into());
        }
        document_id
    };
    Ok(document_id)
}

/// Called directly by a WindowProxy trap so the immediate caller is the
/// script inspecting the proxy, rather than the Realm that created its facade.
fn window_proxy_global_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document = resolve_receiver_document_id(args.first(), context)?;
    if !same_origin_document(context, document)? {
        return Ok(JsValue::null());
    }
    with_host_state(|host| {
        let (realm, frame, auxiliary) = {
            let state = host.borrow();
            let frame = state
                .iframe_documents
                .iter()
                .find_map(|(id, entry)| (entry.document.identity() == document).then_some(*id));
            let auxiliary = state
                .auxiliary_contexts
                .iter()
                .find_map(|(id, entry)| (entry.document.identity() == document).then_some(*id));
            let realm = (state.document.identity() == document)
                .then(|| state.main_realm.clone())
                .flatten();
            (realm, frame, auxiliary)
        };
        let realm = if let Some(realm) = realm {
            realm
        } else if let Some(frame) = frame {
            ensure_iframe_realm(context, host, frame, document)?
        } else if let Some(auxiliary) = auxiliary {
            ensure_auxiliary_realm(context, host, auxiliary)?
        } else {
            return Ok(JsValue::null());
        };
        let previous = context.enter_realm(realm);
        let global = context.global_object();
        context.enter_realm(previous);
        Ok(global.into())
    })
}
