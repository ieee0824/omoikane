//! Bounded, read-only evidence for DOM/layout collapse under the paint trace.

use std::sync::OnceLock;

use crate::css::StyleResolver;
use crate::dom::{Node, NodeHandle};
use crate::layout::LayoutBox;

const NODE_LIMIT: usize = 8192;
const ROOT_LIMIT: usize = 24;

impl super::HostState {
    /// Records structure only: no page text, attribute values or author callbacks.
    pub(super) fn trace_layout_structure(&self) {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        if !*ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_PAINT").is_some()) {
            return;
        }
        let resolver = self
            .document_styles
            .get(&self.document.identity())
            .and_then(|entry| entry.resolver.as_ref());
        let mut data = structure(&self.document, self.layout_root.as_ref(), resolver);
        data["generation"] = self.layout_generation.into();
        data["scroll"] = serde_json::json!([self.window_scroll.0, self.window_scroll.1]);
        data["content_visibility_skipped"] = self.content_visibility_skipped_nodes.len().into();
        eprintln!(
            "{}",
            crate::diagnostics::json_record("OMOIKANE_LAYOUT_STRUCTURE", &data)
        );
    }
}

fn structure(
    document: &NodeHandle,
    layout: Option<&LayoutBox>,
    resolver: Option<&StyleResolver>,
) -> serde_json::Value {
    let mut count = 0;
    let mut current = Some(document.clone());
    while let Some(node) = current.take() {
        if count == NODE_LIMIT {
            current = Some(node);
            break;
        }
        count += 1;
        current = node
            .first_child()
            .or_else(|| next_after_subtree(&node, document));
    }
    let mut pending: Vec<_> = layout.into_iter().collect();
    let mut layout_count = 0;
    while let Some(node) = pending.pop() {
        if layout_count == NODE_LIMIT {
            pending.push(node);
            break;
        }
        layout_count += 1;
        // The stack holds borrowed boxes only; cap its size independently of
        // arbitrarily wide author DOM/layout trees.
        let available = NODE_LIMIT.saturating_sub(pending.len());
        pending.extend(node.children.iter().take(available));
    }
    let body = document.document_body();
    let mut roots = Vec::new();
    if let Some(body) = &body {
        if let Some(html) = body.parent_node() {
            roots.push(root_record(&html, resolver));
        }
        roots.push(root_record(body, resolver));
        let mut child = body.first_child();
        let mut inspected = 0;
        while let Some(node) = child {
            if inspected == ROOT_LIMIT || roots.len() == ROOT_LIMIT {
                break;
            }
            inspected += 1;
            if node.tag_name().is_some() {
                roots.push(root_record(&node, resolver));
            }
            child = node.next_sibling();
        }
    }
    serde_json::json!({
        "document_id": document.identity(), "dom_nodes": count,
        "dom_truncated": current.is_some(), "layout_boxes": layout_count,
        "layout_truncated": !pending.is_empty(), "body_present": body.is_some(),
        "roots": roots,
        "layout_content_rect": layout.map(|layout| {
            let rect = layout.dimensions.content;
            [rect.x, rect.y, rect.width, rect.height]
        }),
    })
}

fn next_after_subtree(node: &NodeHandle, root: &NodeHandle) -> Option<NodeHandle> {
    let mut node = node.clone();
    while node != *root {
        if let Some(sibling) = node.next_sibling() {
            return Some(sibling);
        }
        node = node.parent_node()?;
    }
    None
}

fn root_record(node: &NodeHandle, resolver: Option<&StyleResolver>) -> serde_json::Value {
    serde_json::json!({
        "node_id": node.identity(), "tag": node.tag_name(),
        "has_children": node.first_child().is_some(),
        "cached_display_none": resolver.and_then(|resolver| resolver.cached_display_none(node)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_distinguishes_detached_content_from_retained_hidden_dom() {
        let document =
            crate::html::TreeBuilder::parse("<body><div><p>news</p></div></body>").document();
        let body = document.document_body().unwrap();
        let before = structure(&document, None, None);
        body.set_attribute("style", "display:none");
        let hidden = structure(&document, None, None);
        assert_eq!(before["dom_nodes"], hidden["dom_nodes"]);
        let child = body.first_child().unwrap();
        body.remove_child(&child).unwrap();
        let detached = structure(&document, None, None);
        assert!(detached["dom_nodes"].as_u64().unwrap() < hidden["dom_nodes"].as_u64().unwrap());
        assert_eq!(detached["body_present"], true);
        assert_eq!(detached["roots"][1]["has_children"], false);
    }

    #[test]
    fn hidden_body_keeps_dom_nodes_but_reduces_layout_and_reports_cached_style() {
        let document =
            crate::html::TreeBuilder::parse("<body><div><p>news</p></div></body>").document();
        let mut runtime = super::super::JsRuntime::with_document(document).unwrap();
        runtime.set_viewport(200.0, 100.0);
        let before = {
            let mut state = runtime.host_state.borrow_mut();
            state.ensure_layout();
            let resolver = state.document_styles[&state.document.identity()]
                .resolver
                .as_ref();
            structure(&state.document, state.layout_root.as_ref(), resolver)
        };
        runtime.eval("document.body.style.display='none'").unwrap();
        let hidden = {
            let mut state = runtime.host_state.borrow_mut();
            state.ensure_layout();
            let resolver = state.document_styles[&state.document.identity()]
                .resolver
                .as_ref();
            structure(&state.document, state.layout_root.as_ref(), resolver)
        };
        assert_eq!(before["dom_nodes"], hidden["dom_nodes"]);
        assert!(
            hidden["layout_boxes"].as_u64().unwrap() < before["layout_boxes"].as_u64().unwrap()
        );
        assert_eq!(hidden["roots"][1]["cached_display_none"], true);
    }

    #[test]
    fn structural_scan_is_bounded_and_does_not_read_attribute_values() {
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let body = document.document_body().unwrap();
        for _ in 0..NODE_LIMIT {
            body.append_child(NodeHandle::element("div"));
        }
        body.set_attribute("data-secret", "unrelated-secret-value");
        let data = structure(&document, None, None);
        assert_eq!(data["dom_nodes"], NODE_LIMIT);
        assert_eq!(data["dom_truncated"], true);
        assert_eq!(data["roots"].as_array().unwrap().len(), ROOT_LIMIT);
        assert!(!data.to_string().contains("unrelated-secret-value"));
    }
}
