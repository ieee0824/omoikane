//! Class changes retain styles outside the proven selector dependency scope.
use super::*;

impl HostState {
    /// Invalidates class-dependent style values while preserving unrelated cache entries.
    pub(super) fn invalidate_class_for_node(
        &mut self,
        node: &NodeHandle,
        previous: Option<&str>,
        previous_non_scalar: bool,
    ) {
        self.form_validation.invalidate();
        let Some(document) = document_root_for_node(node) else {
            return;
        };
        if !document.is_html_document() {
            self.invalidate_style_cache_for_node_keeping_validation(node);
            return;
        }
        let current = node.get_attribute("class");
        let non_scalar = previous_non_scalar || node.attribute_has_non_scalar_value("class");
        let mut outcome = "deferred_class_resolver";
        if let Some(entry) = self.document_styles.get_mut(&document.identity()) {
            entry.needs_full_sample = true;
            entry.child_layout = None;
            if !entry.dirty
                && let Some(resolver) = entry.resolver.as_mut()
            {
                outcome = resolver.invalidate_class_subtree(
                    node,
                    previous,
                    current.as_deref(),
                    non_scalar,
                );
            }
        }
        frame_diagnostics::record_style_cache_invalidation(
            document.identity(),
            node.identity(),
            outcome,
        );
        if outcome.starts_with("fallback_") {
            task_diagnostics::record_ordinary_node_mutation_full();
        }
        self.invalidate_document_style_outputs(&document, false);
        if matches!(node.tag_name().as_deref(), Some("iframe" | "frame"))
            && let Some(child) = self.iframe_documents.get(&node.identity())
        {
            let child_document = child.document.clone();
            self.mark_document_style_dirty(&child_document);
        }
    }
}
