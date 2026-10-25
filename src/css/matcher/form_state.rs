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

/// Matches the placeholder state using attributes and owned dirty-value metadata.
pub(super) fn placeholder_shown(node: &NodeHandle) -> bool {
    let is_html = match node.namespace_uri().as_deref() {
        Some(HTML_NAMESPACE) => true,
        None => node.is_html_element(),
        _ => false,
    };
    if !is_html || node.attribute_value_ns(None, "placeholder").is_none() {
        return false;
    }
    let textarea = match node.local_name().as_deref() {
        Some("textarea") => true,
        Some("input") => {
            let kind = node.attribute_value_ns(None, "type").unwrap_or_default();
            if [
                "hidden",
                "date",
                "month",
                "week",
                "time",
                "datetime-local",
                "range",
                "color",
                "checkbox",
                "radio",
                "file",
                "submit",
                "image",
                "reset",
                "button",
            ]
            .iter()
            .any(|excluded| kind.eq_ignore_ascii_case(excluded))
            {
                return false;
            }
            false
        }
        _ => return false,
    };
    node.control_dirty_value_empty().unwrap_or_else(|| {
        if textarea {
            // Match the textarea value getter's initial newline handling.
            let value = crate::dom::collect_descendant_text(node, crate::dom::TextTraversal::All);
            matches!(value.as_str(), "" | "\n" | "\r" | "\r\n")
        } else {
            let value = node.attribute_value_ns(None, "value").unwrap_or_default();
            let kind = node.attribute_value_ns(None, "type").unwrap_or_default();
            let multiple = node.attribute_value_ns(None, "multiple").is_some();
            crate::dom::input_value::normalize(&kind, multiple, value.encode_utf16())
                .produces_empty(value.is_empty())
        }
    })
}
