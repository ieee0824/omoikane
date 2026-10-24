//! Bounded numeric ancestry for a rejected source group; no DOM/CSS text is retained.
use super::*;

const MAX_REPORTED_ANCESTORS: usize = 16;

pub(super) struct FilterAncestorTrace {
    enabled: bool,
    path: Vec<serde_json::Value>,
}

impl FilterAncestorTrace {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            path: Vec::new(),
        }
    }

    pub(super) fn checkpoint(&self) -> usize {
        self.path.len()
    }

    pub(super) fn truncate(&mut self, checkpoint: usize) {
        self.path.truncate(checkpoint);
    }

    pub(super) fn push(
        &mut self,
        layout: &LayoutBox,
        style: &ComputedStyle,
        child: ScanState,
        clip: InvariantOverflowClip,
    ) {
        if !self.enabled {
            return;
        }
        let opacity = element_opacity(style);
        let filter = has_non_none_value(style, PropertyId::Filter);
        let mask = has_non_none_layer(style, PropertyId::MaskImage);
        let clip_path = has_non_none_value(style, PropertyId::ClipPath);
        let containment = crate::layout::has_containment(style, "paint");
        let transformed =
            layout.transform != AffineTransform::identity() || has_transform_style(style);
        let scroll = layout.node.scroll_offset();
        if !opacity.is_some_and(|value| value < 1.0)
            && !filter
            && !mask
            && !clip_path
            && !containment
            && !transformed
            && scroll == (0.0, 0.0)
            && layout.block_fragments.is_empty()
            && !layout.overflow.clips_overflow()
        {
            return;
        }
        let border = border_box_rect(layout);
        let padding = padding_box_rect(layout);
        let radii = [
            "border-top-left-radius",
            "border-top-right-radius",
            "border-bottom-right-radius",
            "border-bottom-left-radius",
        ]
        .map(|name| {
            style
                .get(name)
                .and_then(ComputedValue::linear_length_percentage_components)
        });
        self.path.push(serde_json::json!({
            "node_id":layout.node.identity(),
            "border_box":[border.x,border.y,border.width,border.height],
            "padding_box":[padding.x,padding.y,padding.width,padding.height],
            "opacity":opacity,"filter":filter,"spatial_filter":has_viewport_edge_filter(style),
            "mask":mask,"clip_path":clip_path,"paint_containment":containment,
            "transformed":transformed,"element_scroll":[scroll.0,scroll.1],
            "fragments":layout.block_fragments.len(),"radii":radii,
            "overflow_clips":layout.overflow.clips_overflow(),
            "overflow_invariant":layout.overflow.clips_overflow()
                && !child.clip_geometry_unproven && clip.covers_viewport(layout,style),
            "child_effect":child.effect_ancestor,"child_clip":child.clip_ancestor,
            "child_geometry_unproven":child.clip_geometry_unproven,
        }));
    }

    fn payload(&self, node_id: usize, state: ScanState) -> serde_json::Value {
        let start = self.path.len().saturating_sub(MAX_REPORTED_ANCESTORS);
        serde_json::json!({
            "node_id":node_id,"effect_ancestor":state.effect_ancestor,
            "clip_ancestor":state.clip_ancestor,
            "clip_geometry_unproven":state.clip_geometry_unproven,
            "ancestor_count":self.path.len(),"truncated":start!=0,
            "ancestors":&self.path[start..],
        })
    }

    pub(super) fn report(&self, node_id: usize, state: ScanState) {
        if self.enabled {
            eprintln!(
                "{}",
                crate::diagnostics::json_record(
                    "OMOIKANE_RETAINED_FILTER_ANCESTORS",
                    &self.payload(node_id, state),
                )
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_group_reports_only_the_nearest_sixteen_contributing_ancestors() {
        let mut trace = FilterAncestorTrace::new(true);
        trace.path = (1..=20)
            .map(|node| serde_json::json!({"node_id":node}))
            .collect();
        let payload = trace.payload(99, ScanState::default());
        let ancestors = payload["ancestors"].as_array().unwrap();
        assert_eq!(ancestors.len(), 16);
        assert_eq!(ancestors.first().unwrap()["node_id"], 5);
        assert_eq!(ancestors.last().unwrap()["node_id"], 20);
        assert_eq!(payload["ancestor_count"], 20);
        assert_eq!(payload["truncated"], true);
        let checkpoint = trace.checkpoint();
        trace.path.push(serde_json::json!({"node_id":100}));
        trace.truncate(checkpoint);
        assert_eq!(trace.payload(99, ScanState::default()), payload);
    }
}
