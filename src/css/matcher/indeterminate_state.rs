//! Checkbox flags, unchecked radio groups and progress attribute presence.
use super::*;
use crate::dom::form_controls::html_local_name;

pub(super) fn matches(node: &NodeHandle, cache: &mut SelectorMatchCache) -> bool {
    match html_local_name(node).as_deref() {
        Some("progress") => node.attribute_value_ns(None, "value").is_none(),
        Some("input") => {
            let kind = node.attribute_value_ns(None, "type").unwrap_or_default();
            if kind.eq_ignore_ascii_case("checkbox") {
                node.input_indeterminate()
            } else if kind.eq_ignore_ascii_case("radio") {
                super::form_state_index::for_node(node, cache)
                    .indeterminate_radios
                    .contains(&node.identity())
            } else {
                false
            }
        }
        _ => false,
    }
}
