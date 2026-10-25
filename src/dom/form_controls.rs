//! Pure HTML form-control classification shared by selectors and DOM bindings.

use super::{Node, NodeHandle};

/// Returns HTML local names, including legacy HTML nodes without a namespace.
pub(crate) fn html_local_name(node: &NodeHandle) -> Option<String> {
    let html = match node.namespace_uri().as_deref() {
        Some("http://www.w3.org/1999/xhtml") => true,
        None => node.is_html_element(),
        _ => false,
    };
    html.then(|| node.local_name()).flatten()
}

/// Computes the button type from explicit attributes and its current parent.
/// Auto buttons with command attributes or a select parent are ordinary buttons.
/// Returns None for a node that is not an HTML button.
pub(crate) fn button_type(node: &NodeHandle) -> Option<&'static str> {
    if html_local_name(node).as_deref() != Some("button") {
        return None;
    }
    let kind = node.attribute_value_ns(None, "type").unwrap_or_default();
    match kind.to_ascii_lowercase().as_str() {
        "submit" => Some("submit"),
        "reset" => Some("reset"),
        "button" => Some("button"),
        _ => {
            let command = node.attribute_value_ns(None, "command").is_some()
                || node.attribute_value_ns(None, "commandfor").is_some();
            let select_parent = node
                .parent_node()
                .is_some_and(|parent| html_local_name(&parent).as_deref() == Some("select"));
            Some(if command || select_parent {
                "button"
            } else {
                "submit"
            })
        }
    }
}

/// Whether this element is an HTML submit button, independent of disabledness.
pub(crate) fn is_submit_button(node: &NodeHandle) -> bool {
    match html_local_name(node).as_deref() {
        Some("input") => node.attribute_value_ns(None, "type").is_some_and(|kind| {
            kind.eq_ignore_ascii_case("submit") || kind.eq_ignore_ascii_case("image")
        }),
        Some("button") => button_type(node) == Some("submit"),
        _ => false,
    }
}
