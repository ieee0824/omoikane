//! Collect named-window candidates without crossing the JS boundary per node.

use super::*;

/// Returns ordered `[node ID, is frame, id, eligible name]` rows. The bootstrap
/// still resolves frame access and installs properties in its owning Realm.
/// Only ordinary element children are traversed, matching the named-property
/// walk: shadow roots, template contents and child Documents remain separate.
pub(super) fn snapshot_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let root = with_host_state(|state| Ok(state.borrow().get_node(node_id)))?;
    let mut pending = root
        .map(|node| node.child_nodes().into_iter().rev().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut rows: Vec<JsValue> = Vec::new();
    while let Some(node) = pending.pop() {
        if node.node_type() != NodeType::Element {
            continue;
        }
        let tag = node.local_name().unwrap_or_default();
        let is_frame = matches!(tag.as_str(), "iframe" | "frame");
        let id = nonempty_attribute(&node, "id");
        let name = if is_frame || matches!(tag.as_str(), "embed" | "form" | "img" | "object") {
            nonempty_attribute(&node, "name")
        } else {
            None
        };
        if is_frame || id.is_some() || name.is_some() {
            rows.push(
                JsArray::from_iter(
                    [
                        JsValue::from(node.identity() as f64),
                        JsValue::from(is_frame),
                        id.map_or_else(JsValue::null, JsValue::from),
                        name.map_or_else(JsValue::null, JsValue::from),
                    ],
                    context,
                )
                .into(),
            );
        }
        pending.extend(node.child_nodes().into_iter().rev());
    }
    Ok(JsArray::from_iter(rows, context).into())
}

fn nonempty_attribute(node: &NodeHandle, name: &str) -> Option<JsString> {
    let value = node.get_attribute_utf16(name)?;
    (!value.is_empty()).then(|| JsString::from(value.as_slice()))
}
