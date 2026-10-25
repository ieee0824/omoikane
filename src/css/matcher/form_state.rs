//! HTML form-state classification from the element's current attributes.

use crate::dom::{Node, NodeHandle};

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

/// Returns an eligible input's out-of-range state, without calling JavaScript.
pub(super) fn range_state(node: &NodeHandle) -> Option<bool> {
    use crate::dom::{form_controls::html_local_name, input_range, input_value};
    if html_local_name(node).as_deref() != Some("input")
        || crate::dom::is_actually_disabled(node)
        || node.attribute_value_ns(None, "readonly").is_some()
    {
        return None;
    }
    let mut ancestor = node.parent_node();
    while let Some(parent) = ancestor {
        if html_local_name(&parent).as_deref() == Some("datalist") {
            return None;
        }
        ancestor = parent.parent_node();
    }
    let kind = node.attribute_value_ns(None, "type").unwrap_or_default();
    let kind = input_value::canonical_type(&kind);
    if !matches!(
        kind,
        "number" | "range" | "date" | "month" | "week" | "time" | "datetime-local"
    ) {
        return None;
    }
    let attribute = |name| node.attribute_value_ns(None, name).unwrap_or_default();
    let min =
        input_range::attribute_number(kind, &attribute("min")).or((kind == "range").then_some(0.0));
    let max = input_range::attribute_number(kind, &attribute("max"))
        .or((kind == "range").then_some(100.0));
    if min.is_none() && max.is_none() {
        return None;
    }
    let value = if node.control_dirty_value_empty().is_some() {
        node.control_numeric_value(kind)
    } else if kind == "range" {
        Some(input_range::sanitize_range(
            &attribute("value"),
            &attribute("min"),
            &attribute("max"),
            &attribute("step"),
            &attribute("value"),
        ))
    } else {
        input_range::number(kind, &attribute("value"))
    };
    let (underflow, overflow) = input_range::violations(kind, value, min, max);
    Some(underflow || overflow)
}
