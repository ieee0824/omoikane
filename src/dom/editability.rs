//! Editing and selector classification derived from the current ordinary tree.
use super::{Node, NodeData, NodeHandle, NodeType, form_controls::html_local_name, input_value};

/// The enumerated content attribute's state; invalid and absent values inherit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContentEditable {
    True,
    PlaintextOnly,
    False,
    Inherit,
}

/// Parses the content attribute without consulting JavaScript or document state.
pub(crate) fn content_editable(node: &NodeHandle) -> ContentEditable {
    if html_local_name(node).is_none() {
        return ContentEditable::Inherit;
    }
    match node.attribute_value_ns(None, "contenteditable") {
        Some(value) if value.is_empty() || value.eq_ignore_ascii_case("true") => {
            ContentEditable::True
        }
        Some(value) if value.eq_ignore_ascii_case("plaintext-only") => {
            ContentEditable::PlaintextOnly
        }
        Some(value) if value.eq_ignore_ascii_case("false") => ContentEditable::False,
        _ => ContentEditable::Inherit,
    }
}

impl NodeHandle {
    /// Reads the flag owned by this Document; other node types return `None`.
    pub(crate) fn document_design_mode(&self) -> Option<bool> {
        match &self.0.borrow().data {
            NodeData::Document(_, metadata) => Some(metadata.design_mode_enabled),
            _ => None,
        }
    }

    /// Updates only the owned flag, returning whether it changed. The caller
    /// performs style invalidation, selection reset, and focusing in that order.
    pub(crate) fn set_document_design_mode(&self, enabled: bool) -> bool {
        let mut inner = self.0.borrow_mut();
        let NodeData::Document(_, metadata) = &mut inner.data else {
            return false;
        };
        let changed = metadata.design_mode_enabled != enabled;
        metadata.design_mode_enabled = enabled;
        changed
    }
}

/// Tests HTML's editing-host definition, including a design-mode document root.
pub(crate) fn is_editing_host(node: &NodeHandle) -> bool {
    html_local_name(node).is_some()
        && (matches!(
            content_editable(node),
            ContentEditable::True | ContentEditable::PlaintextOnly
        ) || node
            .parent_node()
            .is_some_and(|parent| parent.document_design_mode() == Some(true)))
}

/// Tests whether a node is an editing host or editable. Ordinary parents keep
/// inheritance from crossing a shadow root, slot, or detached tree boundary.
pub(crate) fn is_content_editable(node: &NodeHandle) -> bool {
    let mut current = node.clone();
    loop {
        if is_editing_host(&current) {
            return true;
        }
        if content_editable(&current) == ContentEditable::False {
            return false;
        }
        let Some(parent) = current.parent_node() else {
            return false;
        };
        let allowed = match current.node_type() {
            NodeType::Document | NodeType::DocumentFragment => false,
            NodeType::Element => {
                html_local_name(&current).is_some() || foreign_editable_root(&current)
            }
            _ => html_local_name(&parent).is_some(),
        };
        if !allowed {
            return false;
        }
        current = parent;
    }
}

fn foreign_editable_root(node: &NodeHandle) -> bool {
    matches!(
        (
            node.namespace_uri().as_deref(),
            node.local_name().as_deref()
        ),
        (Some("http://www.w3.org/2000/svg"), Some("svg"))
            | (Some("http://www.w3.org/1998/Math/MathML"), Some("math"))
    )
}

/// Returns the read-write state for elements; input/textarea mutability takes
/// precedence over inherited content editing. Other nodes match neither state.
pub(crate) fn read_write(node: &NodeHandle) -> Option<bool> {
    if node.node_type() != NodeType::Element {
        return None;
    }
    let mutable_control = || {
        !super::is_actually_disabled(node) && node.attribute_value_ns(None, "readonly").is_none()
    };
    Some(match html_local_name(node).as_deref() {
        Some("textarea") => mutable_control(),
        Some("input") => {
            let raw = node.attribute_value_ns(None, "type").unwrap_or_default();
            matches!(
                input_value::canonical_type(&raw),
                "text"
                    | "search"
                    | "url"
                    | "tel"
                    | "email"
                    | "password"
                    | "date"
                    | "month"
                    | "week"
                    | "time"
                    | "datetime-local"
                    | "number"
            ) && mutable_control()
        }
        _ => is_content_editable(node),
    })
}
