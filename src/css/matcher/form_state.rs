//! HTML form-state classification from the element's current attributes.

use crate::dom::NodeHandle;

const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";

/// Returns required/optional for eligible controls, or neither for other nodes.
/// Disabledness and readonly do not change requiredness. HTML input types that
/// ignore `required` match neither pseudo-class, even if the attribute is set.
pub(super) fn requiredness(node: &NodeHandle) -> Option<bool> {
    let is_html = match node.namespace_uri().as_deref() {
        Some(HTML_NAMESPACE) => true,
        None => node.is_html_element(), // Legacy HTML nodes created by Rust.
        _ => false,
    };
    if !is_html {
        return None;
    }
    match node.local_name().as_deref() {
        Some("select" | "textarea") => {}
        Some("input") => {
            if node.attribute_value_ns(None, "type").is_some_and(|kind| {
                [
                    "hidden", "range", "color", "button", "image", "reset", "submit",
                ]
                .iter()
                .any(|excluded| kind.eq_ignore_ascii_case(excluded))
            }) {
                return None;
            }
        }
        _ => return None,
    }
    Some(node.attribute_value_ns(None, "required").is_some())
}
