//! Conservative rule-cache reuse for mutations that leave stylesheet inputs intact.
//!
//! Child-list changes additionally retain computed styles outside the changed
//! parents when no selector or element state can observe them there.
use super::{
    HostState, NodeHandle, NodeType, NonInlineFullStyleInvalidation, document_root_for_node,
    frame_diagnostics, task_diagnostics,
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

/// The two ends of one child-list change, captured before any node moves.
///
/// `parents` are the elements whose child lists change: the destination and,
/// for a move, each moved node's previous parent. `moved` are the inserted or
/// removed nodes, whose subtrees may hold styles from their old position.
pub(super) struct ChildListChange {
    parents: Vec<NodeHandle>,
    moved: Vec<NodeHandle>,
}

impl ChildListChange {
    /// Captures inserting `node` into `parent`, including its previous parent.
    /// A fragment is emptied by insertion, so its children are recorded instead.
    pub(super) fn insertion(parent: &NodeHandle, node: &NodeHandle) -> Self {
        let mut parents = vec![parent.clone()];
        if let Some(previous) = node.parent_node()
            && &previous != parent
        {
            parents.push(previous);
        }
        let moved = if node.node_type() == NodeType::DocumentFragment {
            node.child_nodes()
        } else {
            vec![node.clone()]
        };
        Self { parents, moved }
    }

    /// Captures removing `node` from `parent`.
    pub(super) fn removal(parent: &NodeHandle, node: &NodeHandle) -> Self {
        Self {
            parents: vec![parent.clone()],
            moved: vec![node.clone()],
        }
    }

    /// Captures replacing all of `parent`'s children with nodes parsed or
    /// created detached, so they have no previous connected parent.
    pub(super) fn children_replaced(
        parent: &NodeHandle,
        removed: &[NodeHandle],
        inserted: &[NodeHandle],
    ) -> Self {
        Self {
            parents: vec![parent.clone()],
            moved: removed.iter().chain(inserted).cloned().collect(),
        }
    }

    /// Captures a Text, Comment or ProcessingInstruction data change, which
    /// selectors observe through its parent like a child-list change.
    pub(super) fn character_data(node: &NodeHandle) -> Self {
        Self {
            parents: node.parent_node().into_iter().collect(),
            moved: Vec::new(),
        }
    }

    /// Whether element state outside the changed subtrees can observe this
    /// change. Form controls determine `:default`, `:indeterminate` and option
    /// selectedness across a form or group; `dir=auto` resolves from
    /// descendant text. Exhausted scan budgets are treated as observable.
    fn reaches_shared_state(&self) -> bool {
        let mut remaining = NODE_BUDGET;
        self.moved
            .iter()
            .any(|node| contains_form_state(node, &mut remaining))
            || self.parents.iter().any(auto_direction_ancestry)
    }
}

fn contains_form_state(root: &NodeHandle, remaining: &mut usize) -> bool {
    let mut node = root.clone();
    loop {
        if *remaining == 0 {
            return true;
        }
        *remaining -= 1;
        if node.node_type() == NodeType::Element
            && matches!(
                node.tag_name().as_deref(),
                Some(
                    "input"
                        | "button"
                        | "select"
                        | "option"
                        | "optgroup"
                        | "datalist"
                        | "form"
                        | "fieldset"
                )
            )
        {
            return true;
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

fn auto_direction_ancestry(parent: &NodeHandle) -> bool {
    let mut current = Some(parent.clone());
    for _ in 0..ANCESTOR_BUDGET {
        let Some(node) = current else { return false };
        if node.node_type() == NodeType::Element
            && (node.tag_name().as_deref() == Some("bdi")
                || node
                    .get_attribute("dir")
                    .is_some_and(|dir| dir.trim().eq_ignore_ascii_case("auto")))
        {
            return true;
        }
        current = node.parent_node();
    }
    true
}

impl HostState {
    /// Invalidates the document containing `node` after `node`'s children or
    /// character data changed in place. An `<iframe>`'s fallback children are
    /// also part of its layout box, so its child document is rebuilt too.
    pub(super) fn invalidate_style_for_child_list(
        &mut self,
        node: &NodeHandle,
        rebuild: bool,
        change: &ChildListChange,
    ) {
        if rebuild {
            self.mark_style_dirty_for_node(node);
            return;
        }
        match document_root_for_node(node) {
            Some(document) => self.invalidate_document_for_tree_mutation(&document, false, change),
            None => self.form_validation.invalidate(),
        }
        if matches!(node.tag_name().as_deref(), Some("iframe" | "frame"))
            && let Some(child) = self.iframe_documents.get(&node.identity())
        {
            let child_document = child.document.clone();
            self.mark_document_style_dirty(&child_document);
        }
    }

    /// Invalidates all derived values while retaining proven-unchanged rule inputs.
    ///
    /// Without a stylesheet rebuild, only computed styles that `change` can
    /// reach are discarded; geometry is still invalidated document-wide.
    pub(super) fn invalidate_document_for_tree_mutation(
        &mut self,
        document: &NodeHandle,
        rebuild: bool,
        change: &ChildListChange,
    ) {
        if rebuild {
            self.mark_document_style_dirty(document);
            return;
        }
        self.form_validation.invalidate();
        if !document.is_html_document() || change.reaches_shared_state() {
            self.invalidate_document_style_cache_keeping_validation(
                document,
                NonInlineFullStyleInvalidation::OrdinaryNodeMutation,
            );
            return;
        }
        let parents: Vec<_> = change
            .parents
            .iter()
            .filter(|parent| document_root_for_node(parent).as_ref() == Some(document))
            .cloned()
            .collect();
        let mut outcome = "deferred_tree_resolver";
        if let Some(entry) = self.document_styles.get_mut(&document.identity()) {
            entry.needs_full_sample = true;
            entry.child_layout = None;
            if !entry.dirty
                && let Some(resolver) = entry.resolver.as_mut()
            {
                outcome = resolver.invalidate_tree_mutation_subtrees(&parents, &change.moved);
            }
        }
        let node_id = parents.first().map_or(0, NodeHandle::identity);
        frame_diagnostics::record_style_cache_invalidation(document.identity(), node_id, outcome);
        if outcome == "local_tree_subtree" {
            task_diagnostics::record_local_tree_mutation_invalidation();
        } else if outcome.starts_with("fallback_") {
            task_diagnostics::record_ordinary_node_mutation_full();
        }
        self.invalidate_document_style_outputs(document, false);
    }
}
