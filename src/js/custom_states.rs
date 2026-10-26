//! Private custom state mutation bridge; strings retain their UTF-16 units.
use super::*;

/// Publishes set changes and invalidates the element's actual document.
pub(super) fn update_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let operation = args.get(1).and_then(JsValue::as_number);
    if !matches!(operation, Some(0.0 | 1.0 | 2.0)) {
        return Err(JsNativeError::typ()
            .with_message("Invalid custom state mutation")
            .into());
    }
    let value = if operation == Some(0.0) {
        Vec::new()
    } else {
        args.get(2)
            .and_then(JsValue::as_string)
            .ok_or_else(|| JsNativeError::typ().with_message("Custom state string required"))?
            .to_vec()
    };
    with_render_mutation(|state| {
        let node = state
            .borrow()
            .get_node(node_id)
            .ok_or_else(|| JsNativeError::typ().with_message("Element required"))?;
        let changed = if operation == Some(0.0) {
            node.clear_custom_states()
        } else {
            node.update_custom_state(value, operation == Some(1.0))
        };
        if changed {
            state.borrow_mut().invalidate_style_cache_for_node(&node);
        }
        Ok(JsValue::from(changed))
    })
}
