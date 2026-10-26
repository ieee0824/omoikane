//! Private editing-state queries and owned Document flag mutation.
use super::*;

/// Reads actual DOM editability instead of observable author-defined getters.
pub(super) fn query_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    let host_only = args.get(1).is_some_and(JsValue::to_boolean);
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsNativeError::typ().with_message("Node required"))?;
        Ok(JsValue::from(if host_only {
            crate::dom::editability::is_editing_host(&node)
        } else {
            crate::dom::editability::is_content_editable(&node)
        }))
    })
}

/// Gets the owned design-mode flag, or updates it and invalidates that document.
/// JavaScript performs the subsequent selection and focus effects privately.
pub(super) fn document_mode_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, id)?;
    let enabled = args.get(1).and_then(JsValue::as_boolean);
    if enabled.is_none() {
        return with_host_state(|state| {
            let enabled = state
                .borrow()
                .get_node(id)
                .and_then(|document| document.document_design_mode())
                .ok_or_else(|| JsNativeError::typ().with_message("Document required"))?;
            Ok(JsValue::from(enabled))
        });
    }
    with_render_mutation(|state| {
        let document = state
            .borrow()
            .get_node(id)
            .filter(|node| node.node_type() == NodeType::Document)
            .ok_or_else(|| JsNativeError::typ().with_message("Document required"))?;
        if let Some(enabled) = enabled {
            let changed = document.set_document_design_mode(enabled);
            if changed {
                state
                    .borrow_mut()
                    .invalidate_style_cache_for_node(&document);
            }
            Ok(JsValue::from(changed))
        } else {
            Ok(JsValue::from(document.document_design_mode() == Some(true)))
        }
    })
}
