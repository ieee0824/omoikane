//! Collapsed margins carry both signs across adjoining edges. The temporary
//! metadata belongs to one layout invocation, not to DOM nodes or CSS styles.

use super::*;
use std::cell::RefCell;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Strut {
    positive: f32,
    negative: f32,
}

impl Strut {
    fn new(value: f32) -> Self {
        Self {
            positive: value.max(0.0),
            negative: value.min(0.0),
        }
    }
    fn merge(self, other: Self) -> Self {
        Self {
            positive: self.positive.max(other.positive),
            negative: self.negative.min(other.negative),
        }
    }
    pub(super) fn value(self) -> f32 {
        self.positive + self.negative
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Info {
    pub(super) top: Strut,
    pub(super) bottom: Strut,
    through: bool,
}
impl Info {
    fn new(margin: EdgeSizes) -> Self {
        Self {
            top: Strut::new(margin.top),
            bottom: Strut::new(margin.bottom),
            through: false,
        }
    }
}

thread_local! {
    static EDGES: RefCell<Option<HashMap<usize, Info>>> = const { RefCell::new(None) };
}

pub(super) struct Scope(Option<HashMap<usize, Info>>);
impl Scope {
    pub(super) fn new() -> Self {
        Self(EDGES.with(|edges| edges.replace(Some(HashMap::new()))))
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        EDGES.with(|edges| edges.replace(self.0.take()));
    }
}
pub(super) fn forget(node: &NodeHandle) {
    EDGES.with(|edges| {
        if let Some(edges) = edges.borrow_mut().as_mut() {
            edges.remove(&node.identity());
        }
    });
}
pub(super) fn remember(node: &NodeHandle, info: Info) {
    EDGES.with(|edges| {
        if let Some(edges) = edges.borrow_mut().as_mut() {
            edges.insert(node.identity(), info);
        }
    });
}
fn take(layout: &LayoutBox) -> Info {
    EDGES
        .with(|edges| {
            edges
                .borrow_mut()
                .as_mut()
                .and_then(|edges| edges.remove(&layout.node.identity()))
        })
        .unwrap_or_else(|| Info::new(layout.dimensions.margin))
}

fn auto(style: &ComputedStyle, property: &str) -> bool {
    style.get(property).is_none()
        || matches!(style.get(property), Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("auto"))
}

fn formatting_root(node: &NodeHandle, style: &ComputedStyle, resolver: &mut StyleResolver) -> bool {
    let parent = node.parent_node();
    if parent
        .as_ref()
        .is_none_or(|parent| parent.node_type() == NodeType::Document)
    {
        return true;
    }
    if float_side(style) != FloatSide::None
        || is_out_of_flow_positioned(style)
        || overflow(style) == Overflow::Hidden
        || has_containment(style, "layout")
        || has_containment(style, "paint")
        || matches!(
            table::table_display_for_node(node, style),
            Some(table::TableDisplay::Cell)
        )
        || matches!(style.get("display"), Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("flow-root") || value.eq_ignore_ascii_case("inline-block"))
    {
        return true;
    }
    let parent = parent.and_then(|parent| {
        if parent.node_type() == NodeType::DocumentFragment {
            parent.shadow_host()
        } else {
            Some(parent)
        }
    });
    parent.is_some_and(|parent| {
        let parent_style = resolver.computed_style(&parent);
        is_flex_container(&parent_style) || is_grid_container(&parent_style)
    })
}

pub(super) fn shift_lines(lines: &mut [LineBox], dy: f32) {
    for line in lines {
        line.rect.y += dy;
        line.baseline += dy;
        for fragment in &mut line.fragments {
            fragment.rect.y += dy;
        }
    }
}

// An ancestor's flow movement must not move a descendant pinned to an external
// containing block. Auto top/bottom still follow their hypothetical position.
pub(super) fn shift_flow(
    layout: &mut LayoutBox,
    dy: f32,
    resolver: &mut StyleResolver,
    containing_block_moves: bool,
) {
    if dy == 0.0 {
        return;
    }
    let style = resolver.computed_style(&layout.node);
    let outside = position_scheme(&style) == PositionScheme::Fixed
        || (position_scheme(&style) == PositionScheme::Absolute && !containing_block_moves);
    if outside && (!auto(&style, "top") || !auto(&style, "bottom")) {
        return;
    }
    layout.dimensions.content.y += dy;
    shift_lines(&mut layout.lines, dy);
    if let Some(marker) = &mut layout.marker {
        marker.y += dy;
    }
    let containing_block_moves =
        containing_block_moves || establishes_positioned_containing_block(&style);
    for child in &mut layout.children {
        shift_flow(child, dy, resolver, containing_block_moves);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn layout_children(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    padding: EdgeSizes,
    border: EdgeSizes,
    margin: EdgeSizes,
    x: f32,
    y: f32,
    width: f32,
    containing_height: f32,
    viewport: Rect,
    positioned_ancestor: Option<BoxDimensions>,
    used_height: Option<UsedHeight>,
) -> BlockChildrenResult {
    let independent = formatting_root(node, style, resolver);
    let top_allowed = !independent && border.top == 0.0 && padding.top == 0.0;
    let bottom_allowed = !independent
        && border.bottom == 0.0
        && padding.bottom == 0.0
        && auto(style, "height")
        && used_height.is_none()
        && resolved_length(style, "min-height", containing_height).unwrap_or(0.0) == 0.0;
    let mut info = Info::new(margin);
    let mut top_open = top_allowed;
    let mut all_through = true;
    let mut had_clearance = false;
    let mut pending = Strut::default();
    let mut pending_active = false;
    let mut cursor_y = y;
    let mut children = Vec::new();
    let mut lines = Vec::new();
    let mut positioned_children = Vec::new();
    let mut child_shifts = Vec::new();
    let mut inline_nodes = Vec::new();
    let mut float_regions = Vec::new();
    let child_height_basis = used_height
        .and_then(UsedHeight::percentage_basis)
        .or_else(|| {
            resolved_length(style, "height", containing_height)
                .map(|height| border_box_adjust_height(style, height, &padding, &border))
        })
        .unwrap_or(0.0);

    for child in node.layout_child_nodes() {
        if child.node_type() == NodeType::Comment {
            continue;
        }
        let cs = (child.node_type() == NodeType::Element).then(|| resolver.computed_style(&child));
        if cs.as_ref().is_some_and(is_display_none) || is_non_rendered_html_element(&child) {
            continue;
        }
        if is_inline_child(&child, resolver) {
            inline_nodes.push(child);
            continue;
        }
        let previous_lines = lines.len();
        flush_pending_inline_nodes(
            &mut inline_nodes,
            resolver,
            style,
            &float_regions,
            &mut cursor_y,
            x,
            width,
            &mut lines,
        );
        if lines[previous_lines..]
            .iter()
            .any(|line| line.rect.height > 0.0)
        {
            top_open = false;
            all_through = false;
            pending = Strut::default();
            pending_active = false;
        }
        let Some(cs) = cs else {
            continue;
        };
        let specified_top = edge_sizes(&cs, "margin").top;
        let predicted_delta = if pending_active {
            pending.value() + specified_top - pending.merge(Strut::new(specified_top)).value()
        } else {
            0.0
        };
        // With parent/first-child collapse, the hypothetical border edge is
        // at the parent's content start. If it interferes with a float, insert
        // clearance and stop that collapse even when the clearance itself is
        // zero or negative after restoring the child's specified margin.
        let collapsing_top =
            top_open && float_side(&cs) == FloatSide::None && !is_out_of_flow_positioned(&cs);
        let before_clear = cursor_y;
        apply_clear(
            &mut cursor_y,
            &cs,
            if collapsing_top { 0.0 } else { specified_top },
            predicted_delta,
            &float_regions,
        );
        let cleared = cursor_y != before_clear;
        if cleared && collapsing_top {
            cursor_y -= specified_top;
        }
        if cleared {
            top_open = false;
            had_clearance = true;
        }
        let child_y = cursor_y - predicted_delta;
        // Clearance places the border edge below the float. The margin edge
        // may still overlap it and must not retain that float's width offset.
        let float_y = if clear_side(&cs) != ClearSide::None {
            child_y + specified_top
        } else {
            child_y
        };
        let offsets = active_float_offsets(&float_regions, float_y, x, width);
        let containing =
            child_containing_rect(&cs, child_y, &offsets, x, width, child_height_basis);
        if is_out_of_flow_positioned(&cs) {
            positioned_children.push((child, cs, containing));
            continue;
        }
        let side = float_side(&cs);
        if side != FloatSide::None {
            layout_float_child(
                &child,
                &cs,
                resolver,
                side,
                child_y,
                x,
                width,
                viewport,
                positioned_ancestor,
                &mut float_regions,
                &mut children,
            );
            continue;
        }
        let next_pos_ancestor = if establishes_positioned_containing_block(style) {
            Some(BoxDimensions {
                content: Rect {
                    x,
                    y,
                    width,
                    height: 0.0,
                },
                padding,
                border,
                margin,
            })
        } else {
            positioned_ancestor
        };
        if let Some(layout) = layout_node(&child, resolver, containing, viewport, next_pos_ancestor)
        {
            let child_info = take(&layout);
            let actual_delta = if top_open {
                child_info.top.value()
            } else if pending_active {
                pending.value() + child_info.top.value() - pending.merge(child_info.top).value()
            } else {
                0.0
            };
            child_shifts.push((children.len(), predicted_delta - actual_delta));
            if top_open {
                info.top = info.top.merge(child_info.top);
            }
            if child_info.through && !cleared {
                if top_open {
                    info.top = info.top.merge(child_info.bottom);
                } else {
                    let joined = pending.merge(child_info.top).merge(child_info.bottom);
                    cursor_y += joined.value() - pending.value();
                    pending = joined;
                    pending_active = true;
                }
            } else {
                cursor_y += layout.total_height() - actual_delta;
                pending = child_info.bottom;
                pending_active = true;
                top_open = false;
                all_through = false;
            }
            children.push(layout);
        }
    }
    let previous_lines = lines.len();
    flush_pending_inline_nodes(
        &mut inline_nodes,
        resolver,
        style,
        &float_regions,
        &mut cursor_y,
        x,
        width,
        &mut lines,
    );
    if lines[previous_lines..]
        .iter()
        .any(|line| line.rect.height > 0.0)
    {
        all_through = false;
        pending_active = false;
    }
    if bottom_allowed && pending_active {
        info.bottom = info.bottom.merge(pending);
        cursor_y -= pending.value();
    }
    info.through = !independent
        && all_through
        && !had_clearance
        && lines.iter().all(|line| line.rect.height == 0.0)
        && padding.vertical() == 0.0
        && border.vertical() == 0.0
        && used_height.is_none_or(|height| height.value == 0.0)
        && resolved_length(style, "height", containing_height).unwrap_or(0.0) == 0.0
        && resolved_length(style, "min-height", containing_height).unwrap_or(0.0) == 0.0;
    let float_bottom = if independent {
        float_regions
            .iter()
            .map(|region| region.outer.y + region.outer.height)
            .fold(y, f32::max)
    } else {
        y
    };
    BlockChildrenResult {
        children,
        lines,
        cursor_y,
        float_bottom,
        positioned_children,
        margin_info: Some(info),
        child_shifts,
    }
}
