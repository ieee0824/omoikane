//! Short-lived owned evidence for one narrow leaf background/data mutation.
use super::*;

#[derive(Clone, Copy)]
pub(super) enum Mutation {
    BackgroundImage,
    DataLoaded,
}

pub(super) struct Candidate {
    document: usize,
    node: usize,
    style_generation: u64,
    layout_generation: u64,
    before: Arc<crate::css::ComputedStyle>,
}

fn plain_leaf(node: &NodeHandle) -> bool {
    if !node.is_html_element() || !node.has_tag_name("div") || !node.child_nodes().is_empty() {
        return false;
    }
    if node
        .parent_node()
        .is_some_and(|parent| parent.node_type() == NodeType::Document)
    {
        return false;
    }
    let mut current = Some(node.clone());
    while let Some(node) = current {
        if node.node_type() == NodeType::Document {
            return true;
        }
        if !node.is_html_element()
            || node.shadow_root().is_some()
            || node.assigned_slot().is_some()
            || node.containing_shadow_root().is_some()
            || node.top_layer_order().is_some()
        {
            return false;
        }
        current = node.parent_node();
    }
    false
}

fn inspect_layout(root: &LayoutBox, node: usize, count: &mut usize) -> bool {
    if root.lines.iter().any(|line| {
        line.fragments
            .iter()
            .chain(
                line.text_overflow
                    .iter()
                    .flat_map(|paint| paint.fragments.iter()),
            )
            .any(|fragment| {
                fragment.node.identity() == node
                    || fragment
                        .text_source
                        .iter()
                        .any(|source| source.node.identity() == node)
            })
    }) {
        return false;
    }
    if root.node.identity() == node {
        *count += 1;
        if root.pseudo.is_some()
            || root.position_scheme != crate::layout::PositionScheme::Absolute
            || root.visibility != crate::layout::Visibility::Visible
            || !root.children.is_empty()
            || !root.lines.is_empty()
            || root.marker.is_some()
            || !root.block_fragments.is_empty()
            || root.multicol.is_some()
            || root.content_visibility_contents_skipped
        {
            return false;
        }
    }
    root.children
        .iter()
        .all(|child| inspect_layout(child, node, count))
}

impl HostState {
    // Called after the DOM attribute write but BEFORE resolver eviction. Only
    // reads the old immutable cache/layout: no old-DOM recomputation or rollback.
    pub(super) fn leaf_paint_candidate(
        &self,
        node: &NodeHandle,
    ) -> Result<Candidate, &'static str> {
        if !plain_leaf(node) {
            return Err("not_plain_leaf");
        }
        let document = document_root_for_node(node).ok_or("detached")?;
        if document.identity() != self.document.identity() || !document.is_html_document() {
            return Err("not_main_html");
        }
        let entry = self
            .document_styles
            .get(&document.identity())
            .ok_or("resolver_missing")?;
        if entry.dirty || entry.viewport_size != Some((self.viewport.width, self.viewport.height)) {
            return Err("dirty_or_viewport");
        }
        let root = self.layout_root.as_ref().ok_or("layout_missing")?;
        let mut count = 0;
        if !inspect_layout(root, node.identity(), &mut count) || count != 1 {
            return Err("layout_or_fragment");
        }
        let before = entry
            .resolver
            .as_ref()
            .ok_or("resolver_missing")?
            .cached_leaf_paint_style(node)?;
        Ok(Candidate {
            document: document.identity(),
            node: node.identity(),
            style_generation: self.style_generation,
            layout_generation: self.layout_generation,
            before,
        })
    }

    pub(super) fn finish_leaf_paint(
        &mut self,
        node: &NodeHandle,
        candidate: Result<Candidate, &'static str>,
        mutation: Mutation,
        cache_outcome: &'static str,
    ) -> bool {
        let result = self.validate_leaf_paint(node, candidate, mutation, cache_outcome);
        let (kind, reason) = (
            match mutation {
                Mutation::BackgroundImage => "background",
                Mutation::DataLoaded => "data-loaded",
            },
            result.err().unwrap_or("keep"),
        );
        frame_diagnostics::record_leaf_paint_reuse(node.identity(), kind, reason);
        result.is_ok()
    }

    fn validate_leaf_paint(
        &mut self,
        node: &NodeHandle,
        candidate: Result<Candidate, &'static str>,
        mutation: Mutation,
        cache_outcome: &'static str,
    ) -> Result<(), &'static str> {
        let candidate = candidate?;
        let allowed = match mutation {
            Mutation::BackgroundImage => {
                matches!(cache_outcome, "local" | "local_raw_style_subject")
            }
            Mutation::DataLoaded => cache_outcome == "local_data_attribute_none",
        };
        if !allowed {
            return Err("nonlocal_cache_policy");
        }
        if candidate.node != node.identity()
            || candidate.document != self.document.identity()
            || candidate.style_generation != self.style_generation
            || candidate.layout_generation != self.layout_generation
            || self.layout_root.is_none()
            || !plain_leaf(node)
            || document_root_for_node(node)
                .is_none_or(|document| document.identity() != candidate.document)
        {
            return Err("context_changed");
        }
        let entry = self
            .document_styles
            .get_mut(&candidate.document)
            .ok_or("resolver_missing")?;
        if entry.dirty || entry.viewport_size != Some((self.viewport.width, self.viewport.height)) {
            return Err("dirty_or_viewport");
        }
        entry
            .resolver
            .as_mut()
            .ok_or("resolver_missing")?
            .validate_leaf_paint_style(
                node,
                &candidate.before,
                matches!(mutation, Mutation::BackgroundImage),
            )
    }
}
