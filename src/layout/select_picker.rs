//! The drop-down box of an open base appearance `select` (Issue #1573).
//!
//! A base appearance select renders its button in flow and the remaining
//! children inside `::picker(select)`. The picker is in the top layer while
//! the select is open, so it is laid out after the document, below the
//! select's border box. Anchor positioning fallbacks are not applied.

use super::*;
use crate::dom::select::select_button;

/// Appends the picker boxes of open base appearance selects in `document`
/// to `children`, which hold the document's laid-out boxes.
pub(super) fn append_open_pickers(
    document: &NodeHandle,
    resolver: &mut StyleResolver,
    viewport: LayoutViewport,
    children: &mut Vec<LayoutBox>,
) {
    let mut selects = Vec::new();
    collect_open_selects(document, &mut selects);
    for select in selects {
        let Some(anchor) = find_border_rect(children, &select) else {
            continue;
        };
        if let Some(picker) = layout_picker(&select, resolver, anchor, viewport) {
            children.push(picker);
        }
    }
}

/// Returns whether `select` renders as a base appearance drop-down box whose
/// picker also has base appearance.
pub(crate) fn renders_base_picker(select: &NodeHandle, resolver: &mut StyleResolver) -> bool {
    renders_base_drop_down(select, &resolver.computed_style(select))
        && resolver
            .computed_pseudo_style(select, PseudoElement::Picker)
            .is_some_and(|style| has_base_appearance(&style))
}

/// Returns whether `select`, with computed `style`, renders as a base
/// appearance drop-down box.
pub(super) fn renders_base_drop_down(select: &NodeHandle, style: &ComputedStyle) -> bool {
    has_base_appearance(style) && crate::dom::select::is_drop_down(select)
}

fn has_base_appearance(style: &ComputedStyle) -> bool {
    matches!(style.get("appearance"), Some(ComputedValue::Keyword(value)) if value == "base-select")
}

fn collect_open_selects(node: &NodeHandle, result: &mut Vec<NodeHandle>) {
    if node.is_html_element() && node.has_tag_name("select") && node.is_picker_open() {
        result.push(node.clone());
    }
    for child in node.child_nodes() {
        collect_open_selects(&child, result);
    }
    if let Some(shadow_root) = node.shadow_root() {
        collect_open_selects(&shadow_root, result);
    }
}

/// The border box of `node`'s principal box or atomic inline fragment.
fn find_border_rect(boxes: &[LayoutBox], node: &NodeHandle) -> Option<Rect> {
    for layout in boxes {
        if layout.pseudo.is_none() && layout.node.identity() == node.identity() {
            return Some(layout.dimensions.border_box());
        }
        let fragment = layout
            .lines
            .iter()
            .flat_map(|line| &line.fragments)
            .find(|fragment| fragment.node.identity() == node.identity());
        if let Some(fragment) = fragment {
            return Some(fragment.rect);
        }
        if let Some(rect) = find_border_rect(&layout.children, node) {
            return Some(rect);
        }
    }
    None
}

/// The select's children rendered in its picker: every child except the
/// leading `button`, which renders in the select itself.
pub(crate) fn picker_children(select: &NodeHandle) -> Vec<NodeHandle> {
    let button = select_button(select);
    select
        .layout_child_nodes()
        .into_iter()
        .filter(|child| child.node_type() == NodeType::Element)
        .filter(|child| {
            button
                .as_ref()
                .is_none_or(|button| button.identity() != child.identity())
        })
        .collect()
}

fn layout_picker(
    select: &NodeHandle,
    resolver: &mut StyleResolver,
    anchor: Rect,
    viewport: LayoutViewport,
) -> Option<LayoutBox> {
    if !renders_base_picker(select, resolver) {
        return None;
    }
    let style = resolver.computed_pseudo_style(select, PseudoElement::Picker)?;
    let padding = edge_sizes(&style, "padding");
    let border = edge_sizes(&style, "border");
    let items: Vec<NodeHandle> = picker_children(select)
        .into_iter()
        .filter(|child| !resolver.computed_style(child).is_display_none())
        .collect();
    let widest = items
        .iter()
        .map(|item| intrinsic_width(item, resolver))
        .fold(0.0f32, f32::max);
    let width = widest.max(anchor.width - padding.horizontal() - border.horizontal());
    let x = anchor.x + border.left + padding.left;
    let y = anchor.y + anchor.height + border.top + padding.top;
    let mut cursor_y = y;
    let mut children = Vec::new();
    for item in items {
        let containing = Rect {
            x,
            y: cursor_y,
            width,
            height: viewport.rect.height,
        };
        if let Some(layout) = layout_node(&item, resolver, containing, viewport, None) {
            cursor_y += layout.total_height();
            children.push(layout);
        }
    }
    Some(LayoutBox {
        node: select.clone(),
        pseudo: Some(PseudoElement::Picker),
        dimensions: BoxDimensions {
            content: Rect {
                x,
                y,
                width,
                height: cursor_y - y,
            },
            padding,
            border,
            margin: EdgeSizes::default(),
        },
        visibility: visibility(&style),
        overflow: Overflow::Visible,
        position_scheme: PositionScheme::Absolute,
        fixed_containing_block: false,
        z_index: i32::MAX,
        transform: AffineTransform::identity(),
        needs_scroll_translation: false,
        has_out_of_flow_descendants: None,
        content_visibility_contents_skipped: false,
        paint_scroll: None,
        block_fragments: Vec::new(),
        multicol: None,
        lines: Vec::new(),
        children,
        marker: None,
    })
}
