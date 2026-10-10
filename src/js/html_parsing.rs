//! HTML parsing for the unsafe markup insertion APIs.
use super::*;

pub(super) fn fragment_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    let source = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?;
    let source = dom_string::from_js(&source);
    let run_scripts = args.get(2).is_some_and(JsValue::to_boolean);
    let sanitizer = html_sanitizer::from_argument(args.get(3), context)?;
    let safe = args.get(4).is_some_and(JsValue::to_boolean);
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        let node = state
            .get_node(id)
            .ok_or_else(|| JsNativeError::reference().with_message("node not found"))?;
        let parsing_context = node.shadow_host().unwrap_or_else(|| node.clone());
        if safe
            && parsing_context.local_name().as_deref() == Some("script")
            && (parsing_context.is_html_element()
                || parsing_context.namespace_uri().as_deref() == Some("http://www.w3.org/2000/svg"))
        {
            return Ok(JsValue::null());
        }
        let parsed = match &source {
            DomString::Scalar(value) => {
                crate::html::TreeBuilder::parse_fragment_with_shadow_roots(value, &parsing_context)
            }
            DomString::Utf16(units) => {
                crate::html::TreeBuilder::parse_fragment_utf16_with_shadow_roots(
                    units,
                    &parsing_context,
                )
            }
        };
        if let Some(config) = &sanitizer {
            html_sanitizer::sanitize(&parsed.fragment(), config, safe);
            if let Some(root) = parsed.context_shadow_root() {
                html_sanitizer::sanitize(&root, config, safe);
            }
        }
        let owner = state
            .node_lifetime_owner(id)
            .map(|owner| owner.identity())
            .unwrap_or_else(|| context_document_id(context, &state));
        if let Some(parsed_root) = parsed.context_shadow_root() {
            if let Some(mode) = parsed_root.shadow_root_mode() {
                if let Some(root) = parsing_context.attach_shadow(mode) {
                    if let Some(settings) = parsed_root.shadow_root_settings() {
                        root.set_shadow_root_settings(settings);
                    }
                    for child in parsed_root.child_nodes() {
                        root.append_child(child);
                    }
                    state.register_tree_for_document(&root, Some(owner));
                    if run_scripts {
                        mark_inserted_scripts_in_tree(&mut state, &root);
                        state.schedule_connected_resource_loads(&root, true);
                    }
                    state.mark_style_dirty_for_node(&parsing_context);
                }
            }
        }
        let fragment = parsed.fragment();
        state.register_tree_for_document(&fragment, Some(owner));
        if run_scripts {
            mark_inserted_scripts_in_tree(&mut state, &fragment);
        }
        Ok(JsValue::from(fragment.identity() as f64))
    })
}

pub(super) fn document_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let source = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?;
    let source = dom_string::from_js(&source);
    let sanitizer = html_sanitizer::from_argument(args.get(1), context)?;
    let safe = args.get(2).is_some_and(JsValue::to_boolean);
    let parsed = match &source {
        DomString::Scalar(value) => crate::html::TreeBuilder::parse_inert(value),
        DomString::Utf16(units) => crate::html::TreeBuilder::parse_inert_utf16(units),
    };
    if let Some(config) = &sanitizer {
        html_sanitizer::sanitize(&parsed.document(), config, safe);
    }
    let quirks = parsed.quirks_mode();
    let document = parsed.document();
    let id = document.identity();
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        let creator = context_document_id(context, &state);
        if let Some(origin) = state.document_security_origins.get(&creator).cloned() {
            state.document_security_origins.insert(id, origin);
        }
        state.register_tree_for_document(&document, Some(id));
        state.document_styles.insert(
            id,
            DocumentStyleEntry {
                viewport_size: None,
                resolver: None,
                resources: Default::default(),
                web_fonts: Default::default(),
                dirty: true,
                needs_full_sample: true,
            },
        );
        Ok(js_string!(serde_json::json!({"id": id, "quirks": quirks}).to_string()).into())
    })
}

/// Reads immutable decoder metadata from the document, never from meta nodes.
pub(super) fn encoding_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, id)?;
    with_host_state(|host| {
        let encoding = host
            .borrow()
            .get_node(id)
            .and_then(|node| node.document_character_encoding())
            .ok_or_else(|| JsNativeError::typ().with_message("receiver is not a Document"))?;
        Ok(js_string!(encoding).into())
    })
}
