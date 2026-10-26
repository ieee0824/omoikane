//! Private bridge for the document-owned active View Transition state.

use super::*;

/// Reads or updates the active transition flag and invalidates document styles.
pub(super) fn document_state_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, id)?;
    let active = args.get(1).and_then(JsValue::as_boolean);
    with_render_mutation(|state| {
        let document = state
            .borrow()
            .get_node(id)
            .filter(|node| node.node_type() == NodeType::Document)
            .ok_or_else(|| JsNativeError::typ().with_message("Document required"))?;
        let Some(active) = active else {
            return Ok(JsValue::from(
                document.document_has_active_view_transition() == Some(true),
            ));
        };
        let changed = document.set_document_active_view_transition(active);
        if changed {
            state
                .borrow_mut()
                .invalidate_style_cache_for_node(&document);
        }
        Ok(JsValue::from(changed))
    })
}
