//! Collect named-window candidates without crossing the JS boundary per node.

use super::*;
/// Returns flat, ordered `[node ID, is frame, id, eligible name, ...]` slots. The bootstrap
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
    Ok(snapshot_values(root, context, || {}))
}

fn snapshot_values(
    root: Option<NodeHandle>,
    context: &mut Context,
    mut after_candidate: impl FnMut(),
) -> JsValue {
    let mut pending = root
        .map(|node| node.child_nodes().into_iter().rev().collect::<Vec<_>>())
        .unwrap_or_default();
    // Only immediate values and reference-counted JsStrings enter this buffer.
    // It contains no GC objects, so allocation/collection needs no root provider.
    let mut values: Vec<JsValue> = Vec::new();
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
            values.extend([
                JsValue::from(node.identity() as f64),
                JsValue::from(is_frame),
                id.map_or_else(JsValue::null, JsValue::from),
                name.map_or_else(JsValue::null, JsValue::from),
            ]);
            after_candidate();
        }
        pending.extend(node.child_nodes().into_iter().rev());
    }
    JsArray::from_iter(values, context).into()
}

fn nonempty_attribute(node: &NodeHandle, name: &str) -> Option<JsString> {
    let value = node.get_attribute_utf16(name)?;
    (!value.is_empty()).then(|| JsString::from(value.as_slice()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_gc::{Rooted, force_collect, force_minor_collect};

    #[test]
    fn named_snapshot_preserves_flat_candidates_through_collection() {
        let document = NodeHandle::document();
        let outer = NodeHandle::element("div");
        outer.set_attribute("id", "outer");
        let form = NodeHandle::element("form");
        form.set_attribute("name", "duplicate");
        let frame = NodeHandle::element("iframe");
        frame.set_attribute("id", "frame-id");
        frame.set_attribute("name", "duplicate");
        let empty_frame = NodeHandle::element("frame");
        let image = NodeHandle::element("img");
        let non_scalar = [0xd800, 0x006e];
        image.set_attribute_utf16("name", &non_scalar);
        let ignored = NodeHandle::element("span");
        ignored.set_attribute("name", "ineligible");
        document.append_child(outer.clone());
        outer.append_child(form.clone());
        form.append_child(frame.clone());
        outer.append_child(empty_frame.clone());
        outer.append_child(image.clone());
        outer.append_child(ignored);
        let mut context = Context::default();
        let mut collections = 0;
        let snapshot = Rooted::new(snapshot_values(Some(document), &mut context, || {
            force_minor_collect();
            force_collect();
            collections += 1;
        }));
        assert_eq!(collections, 5);
        force_minor_collect();
        force_collect();
        let array = JsArray::from_object(snapshot.as_object().unwrap().clone()).unwrap();
        assert_eq!(array.length(&mut context).unwrap(), 20);
        let expected = [
            (outer, false, Some(JsString::from("outer")), None),
            (form, false, None, Some(JsString::from("duplicate"))),
            (
                frame,
                true,
                Some(JsString::from("frame-id")),
                Some(JsString::from("duplicate")),
            ),
            (empty_frame, true, None, None),
            (
                image,
                false,
                None,
                Some(JsString::from(non_scalar.as_slice())),
            ),
        ];
        for (index, (node, is_frame, id, name)) in expected.into_iter().enumerate() {
            let offset = index as u32 * 4;
            assert_eq!(
                array.get(offset, &mut context).unwrap().as_number(),
                Some(node.identity() as f64)
            );
            assert_eq!(
                array.get(offset + 1, &mut context).unwrap().as_boolean(),
                Some(is_frame)
            );
            for (column, expected) in [(2, id), (3, name)] {
                let actual = array.get(offset + column, &mut context).unwrap();
                if let Some(expected) = expected {
                    assert_eq!(actual.as_string().unwrap().to_vec(), expected.to_vec());
                } else {
                    assert!(actual.is_null());
                }
            }
        }
    }
}
