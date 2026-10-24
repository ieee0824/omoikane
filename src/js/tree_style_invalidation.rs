//! Conservative rule-cache reuse for mutations that leave stylesheet inputs intact.
use super::{
    HostState, NodeHandle, NodeType, NonInlineFullStyleInvalidation, document_root_for_node,
};

use crate::dom::Node;

const NODE_BUDGET: usize = 512;
const ANCESTOR_BUDGET: usize = 64;

/// Captures both insertion and removal contexts before a node is moved.
pub(super) fn requires_rebuild(parent: &NodeHandle, subtree: &NodeHandle) -> bool {
    if sensitive_ancestry(parent) {
        return true;
    }
    if let Some(previous) = subtree.parent_node()
        && &previous != parent
        && document_root_for_node(&previous).is_some()
        && sensitive_ancestry(&previous)
    {
        return true;
    }
    let mut remaining = NODE_BUDGET;
    sensitive_subtree(subtree, &mut remaining)
}

/// Element text replacement adds only text; inspect its existing children.
/// CharacterData also refreshes stylesheet-sensitive ancestors.
pub(super) fn text_replacement_requires_rebuild(node: &NodeHandle) -> bool {
    if matches!(
        node.node_type(),
        NodeType::Text | NodeType::Comment | NodeType::ProcessingInstruction
    ) {
        return node
            .parent_node()
            .is_some_and(|parent| sensitive_ancestry(&parent));
    }
    if !node.is_html_element() || sensitive_ancestry(node) {
        return true;
    }
    let mut remaining = NODE_BUDGET;
    let mut child = node.first_child();
    while let Some(current) = child {
        if sensitive_subtree(&current, &mut remaining) {
            return true;
        }
        child = current.next_sibling();
    }
    false
}

/// Old and new children share one scan limit; uncertainty rebuilds the cascade.
pub(super) fn replacement_requires_rebuild(
    parent: &NodeHandle,
    removed: &[NodeHandle],
    inserted: &[NodeHandle],
) -> bool {
    if !parent.is_html_element() || sensitive_ancestry(parent) {
        return true;
    }
    let mut remaining = NODE_BUDGET;
    removed
        .iter()
        .chain(inserted)
        .any(|node| sensitive_subtree(node, &mut remaining))
}

fn sensitive_ancestry(node: &NodeHandle) -> bool {
    let mut current = Some(node.clone());
    for _ in 0..ANCESTOR_BUDGET {
        let Some(node) = current else { return false };
        match node.node_type() {
            NodeType::Document => return !node.is_html_document(),
            NodeType::Element if node.is_html_element() => {
                if node.shadow_root().is_some()
                    || matches!(
                        node.tag_name().as_deref(),
                        Some(
                            "style"
                                | "link"
                                | "base"
                                | "meta"
                                | "script"
                                | "noscript"
                                | "iframe"
                                | "frame"
                                | "object"
                                | "embed"
                        )
                    )
                {
                    return true;
                }
            }
            _ => return true,
        }
        current = node.parent_node();
    }
    true
}

/// Walks only this subtree without allocating snapshots of arbitrarily wide nodes.
fn sensitive_subtree(root: &NodeHandle, remaining: &mut usize) -> bool {
    let mut node = root.clone();
    loop {
        if *remaining == 0 {
            return true;
        }
        *remaining -= 1;
        match node.node_type() {
            NodeType::Text | NodeType::Comment => {}
            NodeType::Element if node.is_html_element() => {
                // Insertion of inline style also needs the existing CSP membership refresh.
                if node.has_attribute("style")
                    || node.shadow_root().is_some()
                    || matches!(
                        node.tag_name().as_deref(),
                        Some(
                            "style"
                                | "link"
                                | "base"
                                | "meta"
                                | "script"
                                | "noscript"
                                | "template"
                                | "iframe"
                                | "frame"
                                | "object"
                                | "embed"
                                | "html"
                                | "head"
                                | "body"
                        )
                    )
                {
                    return true;
                }
            }
            _ => return true,
        }
        if let Some(child) = node.first_child() {
            node = child;
            continue;
        }
        loop {
            if &node == root {
                return false;
            }
            if let Some(sibling) = node.next_sibling() {
                node = sibling;
                break;
            }
            let Some(parent) = node.parent_node() else {
                return true;
            };
            node = parent;
        }
    }
}

impl HostState {
    /// Invalidates all derived values while retaining proven-unchanged rule inputs.
    pub(super) fn invalidate_document_for_tree_mutation(
        &mut self,
        document: &NodeHandle,
        rebuild: bool,
    ) {
        if rebuild {
            self.mark_document_style_dirty(document);
        } else {
            self.form_validation.invalidate();
            self.invalidate_document_style_cache_keeping_validation(
                document,
                NonInlineFullStyleInvalidation::OrdinaryNodeMutation,
            );
        }
    }
}
