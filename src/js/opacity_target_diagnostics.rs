//! Opt-in classification of opacity animation targets against child paint paths.

use super::*;
use crate::css::ComputedValue;
use crate::css::style::PropertyId;
use std::collections::{BTreeMap, HashMap, HashSet};

const MAX_HISTOGRAM_ENTRIES: usize = 16;

#[derive(Clone, Copy, Default)]
struct BoxFacts {
    visible: bool,
    hidden: bool,
    content_visibility_skipped: bool,
    intersects_viewport: bool,
}

pub(super) fn record(
    state: &mut HostState,
    layout: &LayoutBox,
    viewport: Rect,
    document_id: usize,
    targets: &HashSet<usize>,
    paint_attribution_enabled: bool,
) {
    if targets.is_empty()
        || (!paint_attribution_enabled
            && std::env::var_os("OMOIKANE_TRACE_STYLE_CLOCK_ALL").is_none())
    {
        return;
    }

    let mut boxes = HashMap::new();
    let mut line_fragments = HashSet::new();
    collect_layout_paths(layout, viewport, &mut boxes, &mut line_fragments);
    let target_nodes = targets
        .iter()
        .map(|identity| (*identity, state.get_node(*identity)))
        .collect::<Vec<_>>();
    let Some(resolver) = state
        .document_styles
        .get_mut(&document_id)
        .and_then(|entry| entry.resolver.as_mut())
    else {
        return;
    };

    let mut matching_layout_boxes = 0usize;
    let mut matching_line_fragments = 0usize;
    let mut visible_boxes = 0usize;
    let mut hidden_boxes = 0usize;
    let mut content_visibility_skipped = 0usize;
    let mut viewport_intersections = 0usize;
    let mut matching_box_opacity_below_one = 0usize;
    let mut missing_nodes = 0usize;
    let mut histogram = BTreeMap::<String, usize>::new();
    let mut histogram_overflow = 0usize;

    for (identity, node) in target_nodes {
        let facts = boxes.get(&identity);
        if let Some(facts) = facts {
            matching_layout_boxes += 1;
            visible_boxes += usize::from(facts.visible);
            hidden_boxes += usize::from(facts.hidden);
            content_visibility_skipped += usize::from(facts.content_visibility_skipped);
            viewport_intersections += usize::from(facts.intersects_viewport);
        }
        matching_line_fragments += usize::from(line_fragments.contains(&identity));

        let Some(node) = node else {
            missing_nodes += 1;
            continue;
        };
        let style = resolver.paint_style(&node);
        let opacity_below_one = matches!(
            style.get_by_id(PropertyId::Opacity),
            Some(ComputedValue::Number(value)) if value.is_finite() && *value < 1.0
        );
        if facts.is_some() && opacity_below_one {
            matching_box_opacity_below_one += 1;
        }
        let tag = node.tag_name().unwrap_or_else(|| "#non-element".to_owned());
        let display = match style.get_by_id(PropertyId::Display) {
            Some(ComputedValue::Keyword(value)) => value.as_str(),
            _ => "#other",
        };
        let key = format!("{}|{}", bounded_label(&tag), bounded_label(display));
        if histogram.contains_key(&key) || histogram.len() < MAX_HISTOGRAM_ENTRIES {
            *histogram.entry(key).or_default() += 1;
        } else {
            histogram_overflow += 1;
        }
    }

    eprintln!(
        "{}",
        crate::diagnostics::json_record(
            "OMOIKANE_OPACITY_TARGET_PATH",
            &serde_json::json!({
                "document_id": document_id,
                "opacity_animation_targets": targets.len(),
                "document_has_opacity_animation_pseudos": resolver.has_opacity_only_animation_pseudos(),
                "targets_matching_layout_box": matching_layout_boxes,
                "targets_matching_line_fragment": matching_line_fragments,
                "matching_box_visible": visible_boxes,
                "matching_box_hidden": hidden_boxes,
                "matching_box_content_visibility_skipped": content_visibility_skipped,
                "matching_box_intersects_viewport": viewport_intersections,
                "matching_box_opacity_below_one": matching_box_opacity_below_one,
                "missing_dom_nodes": missing_nodes,
                "tag_display_histogram": histogram,
                "tag_display_histogram_overflow": histogram_overflow,
            }),
        )
    );
}

fn collect_layout_paths(
    layout: &LayoutBox,
    viewport: Rect,
    boxes: &mut HashMap<usize, BoxFacts>,
    line_fragments: &mut HashSet<usize>,
) {
    let facts = boxes.entry(layout.node.identity()).or_default();
    facts.visible |= layout.visibility == crate::layout::Visibility::Visible;
    facts.hidden |= layout.visibility == crate::layout::Visibility::Hidden;
    facts.content_visibility_skipped |= layout.content_visibility_contents_skipped;
    facts.intersects_viewport |= rects_intersect(layout.dimensions.border_box(), viewport);
    for line in &layout.lines {
        for fragment in &line.fragments {
            line_fragments.insert(fragment.node.identity());
        }
        if let Some(overflow) = &line.text_overflow {
            for fragment in &overflow.fragments {
                line_fragments.insert(fragment.node.identity());
            }
        }
    }
    for child in &layout.children {
        collect_layout_paths(child, viewport, boxes, line_fragments);
    }
}

fn rects_intersect(left: Rect, right: Rect) -> bool {
    left.width > 0.0
        && left.height > 0.0
        && right.width > 0.0
        && right.height > 0.0
        && left.x < right.x + right.width
        && right.x < left.x + left.width
        && left.y < right.y + right.height
        && right.y < left.y + left.height
}

fn bounded_label(value: &str) -> &str {
    value.get(..value.len().min(32)).unwrap_or(value)
}
