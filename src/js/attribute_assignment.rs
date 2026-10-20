//! Opt-in diagnostics for exact ordinary style/class attribute assignments.
use super::*;

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
struct CachedDisplayNoneState {
    target: Option<bool>,
    ancestor: Option<bool>,
    ancestry: &'static str,
}

fn cached_layout_state(
    layout: Option<&LayoutBox>,
    node: &NodeHandle,
) -> (bool, Option<bool>, &'static str) {
    let Some(root) = layout else {
        return (false, None, "unknown");
    };
    let Some(layout) = find_layout_box(root, node) else {
        return (true, Some(false), "unknown");
    };
    let position = match layout.position_scheme {
        crate::layout::PositionScheme::Static => "static",
        crate::layout::PositionScheme::Relative => "relative",
        crate::layout::PositionScheme::Sticky => "sticky",
        crate::layout::PositionScheme::Absolute => "absolute",
        crate::layout::PositionScheme::Fixed => "fixed",
    };
    (true, Some(true), position)
}

fn cached_display_none_state(
    resolver: Option<&StyleResolver>,
    node: &NodeHandle,
) -> CachedDisplayNoneState {
    let Some(resolver) = resolver else {
        return CachedDisplayNoneState {
            target: None,
            ancestor: None,
            ancestry: "resolver_missing",
        };
    };
    let target = resolver.cached_display_none(node);
    if node.containing_shadow_root().is_some() {
        return CachedDisplayNoneState {
            target,
            ancestor: None,
            ancestry: "shadow_or_slot_unknown",
        };
    }

    let mut current = node.clone();
    let mut ancestor = Some(false);
    let mut ancestry = "detached_or_fragment_unknown";
    while let Some(parent) = current.parent_node() {
        match parent.node_type() {
            NodeType::Document => {
                return CachedDisplayNoneState {
                    target,
                    ancestor,
                    ancestry: "light_dom_complete",
                };
            }
            NodeType::Element => {
                match resolver.cached_display_none(&parent) {
                    Some(true) => ancestor = Some(true),
                    None if ancestor != Some(true) => ancestor = None,
                    Some(false) | None => {}
                }
                if parent.shadow_root().is_some() {
                    ancestry = "shadow_or_slot_unknown";
                    break;
                }
                current = parent;
            }
            _ => break,
        }
    }
    CachedDisplayNoneState {
        target,
        ancestor: ancestor.filter(|hidden| *hidden),
        ancestry,
    }
}

/// Reads only existing geometry, style, and timeline caches; never resolves new data.
pub(super) fn cached_state(state: &HostState, node: &NodeHandle) -> serde_json::Value {
    let document_id = document_root_for_node(node).map(|document| document.identity());
    let entry = document_id.and_then(|id| state.document_styles.get(&id));
    let resolver = entry.and_then(|entry| entry.resolver.as_ref());
    let layout = if document_id == Some(state.document.identity()) {
        state.layout_root.as_ref()
    } else {
        entry.and_then(|entry| entry.child_layout.as_ref().map(|(_, layout)| layout))
    };
    let (layout_cache_present, node_box_present, position) = cached_layout_state(layout, node);
    serde_json::json!({
        "position": position,
        "layout_cache_present": layout_cache_present,
        "node_box_present": node_box_present,
        "cached_display_none": cached_display_none_state(resolver, node),
        "running_animations": resolver.map(StyleResolver::has_running_animations),
        "running_transitions": resolver.map(StyleResolver::has_running_transitions),
    })
}

fn previous_value(node: &NodeHandle, name: &str, value: &JsString) -> (Option<usize>, bool) {
    let old = node.get_attribute_utf16(name);
    (
        old.as_ref().map(Vec::len),
        old.as_ref()
            .is_some_and(|units| units.iter().copied().eq(value.iter())),
    )
}

#[derive(Debug, serde::Serialize)]
struct StyleDifference {
    changed_properties: Vec<String>,
    order_changed: bool,
}

fn style_difference(old: &str, new: &str) -> StyleDifference {
    let declarations = |source: &str| {
        let declarations = crate::css::parse_style_attribute(source);
        let order: Vec<_> = declarations.iter().map(|d| d.name.clone()).collect();
        let mut values: BTreeMap<String, (crate::css::Value, bool)> = BTreeMap::new();
        for declaration in declarations {
            if values
                .get(&declaration.name)
                .is_some_and(|(_, important)| *important && !declaration.important)
            {
                continue;
            }
            values.insert(declaration.name, (declaration.value, declaration.important));
        }
        (order, values)
    };
    let (old_order, old_values) = declarations(old);
    let (new_order, new_values) = declarations(new);
    let names: std::collections::BTreeSet<_> = old_values.keys().chain(new_values.keys()).collect();
    StyleDifference {
        changed_properties: names
            .into_iter()
            .filter(|name| old_values.get(*name) != new_values.get(*name))
            .map(Clone::clone)
            .collect(),
        order_changed: old_order != new_order,
    }
}

fn parsed_difference(node: &NodeHandle, name: &str, value: &JsString) -> Option<StyleDifference> {
    if !name.eq_ignore_ascii_case("style") {
        return None;
    }
    let old = String::from_utf16(&node.get_attribute_utf16(name).unwrap_or_default()).ok()?;
    let new = value.to_std_string().ok()?;
    Some(style_difference(&old, &new))
}

/// Emits equality and changed declaration names, never property values.
pub(super) fn trace(
    node: &NodeHandle,
    name: &str,
    value: &JsString,
    cached_state: impl FnOnce() -> serde_json::Value,
) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_ATTRIBUTE_SET").is_some()) {
        return;
    }
    if !name.eq_ignore_ascii_case("style") && !name.eq_ignore_ascii_case("class") {
        return;
    }
    let (old_length, same_value) = previous_value(node, name, value);
    eprintln!(
        "OMOIKANE_ATTRIBUTE_SET {}",
        serde_json::json!({
            "node": node.identity(), "name": name,
            "old_length": old_length, "new_length": value.len(),
            "same_value": same_value,
            "cached_style_state": if name.eq_ignore_ascii_case("style") {
                Some(cached_state())
            } else {
                None
            },
            "declarations": parsed_difference(node, name, value),
        })
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declaration_diagnostics_separate_order_value_and_priority_changes() {
        let difference = style_difference("width:1px;height:2px", "height:2px;width:1px");
        assert!(difference.changed_properties.is_empty());
        assert!(difference.order_changed);
        let difference = style_difference("width:1px;height:2px", "width:2px;height:2px");
        assert_eq!(difference.changed_properties, ["width"]);
        assert!(!difference.order_changed);
        let difference = style_difference("color:red!important", "color:red");
        assert_eq!(difference.changed_properties, ["color"]);
        let difference = style_difference("color:red!important;color:blue", "color:red!important");
        assert!(difference.changed_properties.is_empty());
    }

    #[test]
    fn cached_missing_box_diagnostics_distinguish_layout_and_display_none_without_resolution() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let hidden_parent = NodeHandle::element("section");
        let target = NodeHandle::element("div");
        let self_hidden = NodeHandle::element("aside");
        hidden_parent.set_attribute("style", "display:none");
        self_hidden.set_attribute("style", "display:none");
        document.append_child(body.clone());
        body.append_child(hidden_parent.clone());
        hidden_parent.append_child(target.clone());
        body.append_child(self_hidden.clone());

        let mut resolver = StyleResolver::new();
        let layout = crate::layout::layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
        )
        .expect("the visible body must have a layout box");
        resolver.computed_style_shared(&target);
        resolver.computed_style_shared(&self_hidden);

        assert_eq!(cached_layout_state(None, &target), (false, None, "unknown"));
        assert_eq!(
            cached_layout_state(Some(&layout), &target),
            (true, Some(false), "unknown")
        );
        assert_eq!(
            cached_layout_state(Some(&layout), &body),
            (true, Some(true), "static")
        );
        assert_eq!(
            cached_display_none_state(Some(&resolver), &target),
            CachedDisplayNoneState {
                target: Some(false),
                ancestor: Some(true),
                ancestry: "light_dom_complete",
            }
        );
        assert_eq!(
            cached_display_none_state(Some(&resolver), &self_hidden),
            CachedDisplayNoneState {
                target: Some(true),
                ancestor: Some(false),
                ancestry: "light_dom_complete",
            }
        );

        let uncached = NodeHandle::element("article");
        body.append_child(uncached.clone());
        assert_eq!(resolver.cached_display_none(&uncached), None);
        assert_eq!(
            cached_display_none_state(Some(&resolver), &uncached).target,
            None
        );
        assert_eq!(
            resolver.cached_display_none(&uncached),
            None,
            "diagnostics must not populate a missing style cache entry",
        );

        let host = NodeHandle::element("main");
        let possible_slotted = NodeHandle::element("span");
        body.append_child(host.clone());
        host.append_child(possible_slotted.clone());
        host.attach_shadow(ShadowRootMode::Open).unwrap();
        assert_eq!(resolver.cached_display_none(&possible_slotted), None);
        assert_eq!(
            cached_display_none_state(Some(&resolver), &possible_slotted).ancestry,
            "shadow_or_slot_unknown",
        );
        assert_eq!(resolver.cached_display_none(&possible_slotted), None);
    }

    #[test]
    fn equality_distinguishes_missing_empty_and_exact_utf16_without_mutating_the_node() {
        let node = NodeHandle::element("div");
        let empty = JsString::from("");
        assert_eq!(previous_value(&node, "style", &empty), (None, false));
        node.set_attribute("style", "");
        assert_eq!(previous_value(&node, "style", &empty), (Some(0), true));
        node.set_attribute_utf16("style", &[0xd800]);
        let same = JsString::from(&[0xd800u16][..]);
        let different = JsString::from(&[0xd801u16][..]);
        assert_eq!(previous_value(&node, "style", &same), (Some(1), true));
        assert_eq!(previous_value(&node, "style", &different), (Some(1), false));
        assert_eq!(node.get_attribute_utf16("style"), Some(vec![0xd800]));
    }
}
