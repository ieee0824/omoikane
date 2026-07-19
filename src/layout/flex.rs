//! Flex layout: `display: flex` container layout.

use crate::css::{ComputedStyle, ComputedValue, StyleResolver};
use crate::dom::{Node, NodeHandle, NodeType};

use super::{
    AlignItems, BoxDimensions, EdgeSizes, FlexDirection, FlexWrap, JustifyContent, LayoutBox,
    Rect,
    edge_sizes, explicit_length, intrinsic_width, is_display_none, is_out_of_flow_positioned,
    layout_node, layout_positioned_child, normalized_min_max_lengths, overflow, resolved_length,
    sort_children_by_z_index, translate_layout_box_to_outer, visibility, z_index,
};

#[derive(Debug, Clone)]
struct FlexItemSpec {
    node: NodeHandle,
    base_main_size: f32,
    min_main_size: f32,
    explicit_cross_size: Option<f32>,
    flex_grow: f32,
    flex_shrink: f32,
    align_self: Option<AlignItems>,
}

#[derive(Debug, Clone)]
struct FlexLine<'a> {
    items: Vec<&'a FlexItemSpec>,
}

pub(super) fn layout_flex_container(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: ComputedStyle,
    margin: EdgeSizes,
    padding: EdgeSizes,
    border: EdgeSizes,
    x: f32,
    y: f32,
    width: f32,
    viewport: Rect,
) -> Option<LayoutBox> {
    let direction = flex_direction(&style);
    let wrap = flex_wrap(&style);
    let justify = justify_content(&style);
    let align = align_items(&style);
    let main_gap = flex_main_axis_gap(&style, direction);
    let line_gap = flex_cross_axis_gap(&style, direction);

    let mut items = Vec::new();
    let mut positioned_children = Vec::new();
    let main_basis = match direction {
        FlexDirection::Row => width,
        FlexDirection::Column => 0.0, // Height percentage basis is unknown at this point
    };
    for child in node.child_nodes() {
        if child.node_type() != NodeType::Element {
            continue;
        }
        let child_style = resolver.computed_style(&child);
        if is_display_none(&child_style) {
            continue;
        }
        if is_out_of_flow_positioned(&child_style) {
            positioned_children.push((child, child_style));
            continue;
        }
        let base_main_size = flex_basis(&child_style, direction)
            .or_else(|| resolved_main_size(&child_style, direction, main_basis))
            .unwrap_or_else(|| auto_flex_base_main_size(&child, resolver, direction));
        let min_main_size = match direction {
            FlexDirection::Row => explicit_length(&child_style, "min-width")
                .unwrap_or_else(|| super::minimum_content_width(&child, resolver)),
            FlexDirection::Column => explicit_length(&child_style, "min-height").unwrap_or(0.0),
        };
        items.push(FlexItemSpec {
            node: child,
            base_main_size,
            min_main_size,
            explicit_cross_size: explicit_cross_size(&child_style, direction),
            flex_grow: flex_grow(&child_style),
            flex_shrink: flex_shrink(&child_style),
            align_self: align_self(&child_style),
        });
    }

    let available_main_size = flex_available_main_size(&style, direction, width, &items, main_gap);
    let lines = build_flex_lines(&items, available_main_size, wrap, main_gap);
    let mut children = Vec::new();
    let mut cross_cursor = y;
    let mut column_main_end = y;
    let line_count = lines.len();

    for (line_index, line) in lines.into_iter().enumerate() {
        let line_item_count = line.items.len();
        let fixed_main_gap = if line_item_count > 1 {
            main_gap * (line_item_count.saturating_sub(1)) as f32
        } else {
            0.0
        };
        let available_main_for_items = (available_main_size - fixed_main_gap).max(0.0);
        let resolved_main_sizes = resolve_flex_main_sizes(&line.items, available_main_for_items);
        let mut laid_out = Vec::new();
        let mut line_cross_size = 0.0f32;

        for (item, main_size) in line.items.iter().zip(resolved_main_sizes.iter()) {
            let child_containing = match direction {
                FlexDirection::Row => Rect {
                    x: 0.0,
                    y: 0.0,
                    width: *main_size,
                    height: item.explicit_cross_size.unwrap_or(0.0),
                },
                FlexDirection::Column => Rect {
                    x: 0.0,
                    y: 0.0,
                    width: item.explicit_cross_size.unwrap_or(width),
                    height: *main_size,
                },
            };

            if let Some(layout_child) =
                layout_node(&item.node, resolver, child_containing, viewport, None)
            {
                let cross_size = match direction {
                    FlexDirection::Row => layout_child.total_height(),
                    FlexDirection::Column => layout_child.total_width(),
                };
                line_cross_size = line_cross_size.max(cross_size);
                laid_out.push((item, layout_child));
            }
        }

        if direction == FlexDirection::Column {
            let natural_main_size: f32 = laid_out
                .iter()
                .map(|(_, child)| child.total_height())
                .sum::<f32>()
                + fixed_main_gap;
            let free_space = (available_main_size - natural_main_size).max(0.0);
            let total_grow: f32 = laid_out.iter().map(|(item, _)| item.flex_grow).sum();
            if free_space > 0.0 && total_grow > 0.0 {
                for (item, child) in &mut laid_out {
                    if item.flex_grow > 0.0 {
                        child.dimensions.content.height +=
                            free_space * item.flex_grow / total_grow;
                    }
                }
            }
        }

        // A single flex line uses the container's cross size. Using only the
        // tallest item here makes align-items:center/flex-end ineffective in
        // a definite-height row (and in a definite-width column).
        if wrap == FlexWrap::NoWrap {
            line_cross_size = match direction {
                FlexDirection::Row => {
                    let mut size = explicit_length(&style, "height")
                        .map(|height| {
                            super::border_box_adjust_height(&style, height, &padding, &border)
                        })
                        .unwrap_or(line_cross_size);
                    if let Some(min_height) = explicit_length(&style, "min-height") {
                        size = size.max(super::border_box_adjust_height(
                            &style,
                            min_height,
                            &padding,
                            &border,
                        ));
                    }
                    if let Some(max_height) = explicit_length(&style, "max-height") {
                        size = size.min(super::border_box_adjust_height(
                            &style,
                            max_height,
                            &padding,
                            &border,
                        ));
                    }
                    size
                }
                // A non-wrapping column has one flex line whose cross size is
                // the container's content width.  Using the widest child's
                // intrinsic width here makes align-items:center/flex-end align
                // inside that child-sized strip instead of across the column.
                FlexDirection::Column => width,
            };
        }

        let total_main_size: f32 = laid_out
            .iter()
            .map(|(_, child)| match direction {
                FlexDirection::Row => child.total_width(),
                FlexDirection::Column => child.total_height(),
            })
            .sum();
        let used_main_size = total_main_size + fixed_main_gap;
        let (line_start, justify_gap) =
            justify_offsets(justify, available_main_size, used_main_size, laid_out.len());

        let mut main_cursor = match direction {
            FlexDirection::Row => x + line_start,
            FlexDirection::Column => y + line_start,
        };

        let laid_out_count = laid_out.len();
        for (index, (item, mut child)) in laid_out.into_iter().enumerate() {
            let child_main_size = match direction {
                FlexDirection::Row => child.total_width(),
                FlexDirection::Column => child.total_height(),
            };
            let child_cross_size = match direction {
                FlexDirection::Row => child.total_height(),
                FlexDirection::Column => child.total_width(),
            };
            let align_value = item.align_self.unwrap_or(align);
            let cross_offset = align_offset(align_value, line_cross_size, child_cross_size);

            let (outer_x, outer_y) = match direction {
                FlexDirection::Row => (main_cursor, cross_cursor + cross_offset),
                FlexDirection::Column => (x + cross_offset, main_cursor),
            };
            translate_layout_box_to_outer(&mut child, outer_x, outer_y);
            children.push(child);

            if index + 1 < laid_out_count {
                main_cursor += child_main_size + main_gap + justify_gap;
            } else {
                main_cursor += child_main_size;
            }
        }

        if direction == FlexDirection::Column {
            column_main_end = column_main_end.max(main_cursor);
        }
        cross_cursor += line_cross_size;
        if line_index + 1 < line_count {
            cross_cursor += line_gap;
        }
    }

    let auto_height = match direction {
        FlexDirection::Row => cross_cursor - y,
        FlexDirection::Column => column_main_end - y,
    };
    let mut content_height = resolved_length(&style, "height", 0.0)
        .map(|h| super::border_box_adjust_height(&style, h, &padding, &border))
        .unwrap_or(auto_height);
    let (min_height, max_height) =
        normalized_min_max_lengths(&style, "min-height", "max-height", 0.0);
    if let Some(min_height) = min_height {
        let min_h = super::border_box_adjust_height(&style, min_height, &padding, &border);
        content_height = content_height.max(min_h);
    }
    if let Some(max_height) = max_height {
        let max_h = super::border_box_adjust_height(&style, max_height, &padding, &border);
        content_height = content_height.min(max_h);
    }
    let dimensions = BoxDimensions {
        content: Rect {
            x,
            y,
            width,
            height: content_height,
        },
        padding,
        border,
        margin,
    };
    for (child, style) in positioned_children {
        if let Some(positioned) = layout_positioned_child(
            &child,
            resolver,
            &style,
            dimensions,
            dimensions.content,
            viewport,
        ) {
            children.push(positioned);
        }
    }
    sort_children_by_z_index(&mut children);

    Some(LayoutBox {
        node: node.clone(),
        dimensions,
        visibility: visibility(&style),
        overflow: overflow(&style),
        z_index: z_index(&style),
        lines: Vec::new(),
        children,
        marker: None,
    })
}


pub(super) fn is_flex_container(style: &ComputedStyle) -> bool {
    matches!(
        style.get("display"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("flex")
    )
}

pub(super) fn flex_direction(style: &ComputedStyle) -> FlexDirection {
    match style.get("flex-direction") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("column") => {
            FlexDirection::Column
        }
        _ => FlexDirection::Row,
    }
}

fn flex_wrap(style: &ComputedStyle) -> FlexWrap {
    match style.get("flex-wrap") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("wrap") => {
            FlexWrap::Wrap
        }
        _ => FlexWrap::NoWrap,
    }
}

fn flex_gap(style: &ComputedStyle) -> Option<f32> {
    explicit_length(style, "gap")
}

fn flex_main_axis_gap(style: &ComputedStyle, direction: FlexDirection) -> f32 {
    match direction {
        FlexDirection::Row => explicit_length(style, "column-gap")
            .or_else(|| flex_gap(style))
            .unwrap_or(0.0),
        FlexDirection::Column => explicit_length(style, "row-gap")
            .or_else(|| flex_gap(style))
            .unwrap_or(0.0),
    }
}

fn flex_cross_axis_gap(style: &ComputedStyle, direction: FlexDirection) -> f32 {
    match direction {
        FlexDirection::Row => explicit_length(style, "row-gap")
            .or_else(|| flex_gap(style))
            .unwrap_or(0.0),
        FlexDirection::Column => explicit_length(style, "column-gap")
            .or_else(|| flex_gap(style))
            .unwrap_or(0.0),
    }
}

fn justify_content(style: &ComputedStyle) -> JustifyContent {
    match style.get("justify-content") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("center") => {
            JustifyContent::Center
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("flex-end") => {
            JustifyContent::FlexEnd
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("space-between") => {
            JustifyContent::SpaceBetween
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("space-around") => {
            JustifyContent::SpaceAround
        }
        _ => JustifyContent::FlexStart,
    }
}

fn align_items(style: &ComputedStyle) -> AlignItems {
    match style.get("align-items") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("flex-start") => {
            AlignItems::FlexStart
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("center") => {
            AlignItems::Center
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("flex-end") => {
            AlignItems::FlexEnd
        }
        _ => AlignItems::Stretch,
    }
}

fn align_self(style: &ComputedStyle) -> Option<AlignItems> {
    match style.get("align-self") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("flex-start") => {
            Some(AlignItems::FlexStart)
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("center") => {
            Some(AlignItems::Center)
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("flex-end") => {
            Some(AlignItems::FlexEnd)
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("stretch") => {
            Some(AlignItems::Stretch)
        }
        _ => None,
    }
}

fn explicit_main_size(style: &ComputedStyle, direction: FlexDirection) -> Option<f32> {
    match direction {
        FlexDirection::Row => explicit_length(style, "width"),
        FlexDirection::Column => explicit_length(style, "height"),
    }
}

/// Like `explicit_main_size` but also resolves percentages and `calc(% +/- px)` using the
/// container's main axis size as basis.
fn resolved_main_size(style: &ComputedStyle, direction: FlexDirection, basis: f32) -> Option<f32> {
    match direction {
        FlexDirection::Row => resolved_length(style, "width", basis),
        FlexDirection::Column => resolved_length(style, "height", basis),
    }
}

fn explicit_cross_size(style: &ComputedStyle, direction: FlexDirection) -> Option<f32> {
    match direction {
        FlexDirection::Row => explicit_length(style, "height"),
        FlexDirection::Column => explicit_length(style, "width"),
    }
}

fn flex_basis(style: &ComputedStyle, direction: FlexDirection) -> Option<f32> {
    explicit_length(style, "flex-basis").or_else(|| explicit_main_size(style, direction))
}

fn flex_available_main_size(
    style: &ComputedStyle,
    direction: FlexDirection,
    width: f32,
    items: &[FlexItemSpec],
    main_gap: f32,
) -> f32 {
    match direction {
        FlexDirection::Row => width,
        FlexDirection::Column => {
            let item_count = items.len();
            let gap_total = if item_count > 1 {
                main_gap * (item_count.saturating_sub(1)) as f32
            } else {
                0.0
            };
            let natural = items.iter().map(|item| item.base_main_size).sum::<f32>() + gap_total;
            let mut available = explicit_main_size(style, direction).unwrap_or(natural);
            if let Some(min_height) = explicit_length(style, "min-height") {
                available = available.max(min_height);
            }
            if let Some(max_height) = explicit_length(style, "max-height") {
                available = available.min(max_height);
            }
            available
        }
    }
}

fn auto_flex_base_main_size(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    direction: FlexDirection,
) -> f32 {
    match direction {
        // The resolved main size is passed to layout_node as the item's
        // containing width.  That path subtracts the item's margins before
        // computing its content width, so the flex base must be an outer size
        // as well.  intrinsic_width intentionally excludes margins for
        // shrink-to-fit callers.
        FlexDirection::Row => {
            let style = resolver.computed_style(node);
            intrinsic_width(node, resolver) + edge_sizes(&style, "margin").horizontal()
        }
        FlexDirection::Column => 0.0,
    }
}

fn flex_grow(style: &ComputedStyle) -> f32 {
    match style.get("flex-grow") {
        Some(ComputedValue::Number(value)) => *value,
        Some(ComputedValue::Px(value)) => *value,
        _ => 0.0,
    }
}

fn flex_shrink(style: &ComputedStyle) -> f32 {
    match style.get("flex-shrink") {
        Some(ComputedValue::Number(value)) => *value,
        Some(ComputedValue::Px(value)) => *value,
        _ => 1.0,
    }
}

fn build_flex_lines<'a>(
    items: &'a [FlexItemSpec],
    available_main_size: f32,
    wrap: FlexWrap,
    main_gap: f32,
) -> Vec<FlexLine<'a>> {
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut occupied = 0.0f32;

    for item in items {
        let item_size = item.base_main_size;
        let gap = if current.is_empty() { 0.0 } else { main_gap };
        let would_wrap = wrap == FlexWrap::Wrap
            && !current.is_empty()
            && occupied + gap + item_size > available_main_size;
        if would_wrap {
            lines.push(FlexLine { items: current });
            current = Vec::new();
            occupied = 0.0;
        }
        let leading_gap = if current.is_empty() { 0.0 } else { main_gap };
        occupied += leading_gap + item_size;
        current.push(item);
    }

    if !current.is_empty() {
        lines.push(FlexLine { items: current });
    }

    lines
}

fn resolve_flex_main_sizes(items: &[&FlexItemSpec], available_main_size: f32) -> Vec<f32> {
    let total_base: f32 = items.iter().map(|item| item.base_main_size).sum();
    let total_grow: f32 = items.iter().map(|item| item.flex_grow).sum();
    let total_shrink_factor: f32 = items
        .iter()
        .map(|item| item.flex_shrink * item.base_main_size)
        .sum();

    items
        .iter()
        .map(|item| {
            if total_base < available_main_size && total_grow > 0.0 {
                let extra = available_main_size - total_base;
                item.base_main_size + extra * (item.flex_grow / total_grow)
            } else if total_base > available_main_size && total_shrink_factor > 0.0 {
                let overflow = total_base - available_main_size;
                let shrink =
                    overflow * ((item.flex_shrink * item.base_main_size) / total_shrink_factor);
                (item.base_main_size - shrink).max(item.min_main_size)
            } else {
                item.base_main_size
            }
        })
        .collect()
}

fn justify_offsets(
    justify: JustifyContent,
    available_main_size: f32,
    used_main_size: f32,
    item_count: usize,
) -> (f32, f32) {
    let free_space = (available_main_size - used_main_size).max(0.0);
    match justify {
        JustifyContent::FlexStart => (0.0, 0.0),
        JustifyContent::Center => (free_space / 2.0, 0.0),
        JustifyContent::FlexEnd => (free_space, 0.0),
        JustifyContent::SpaceBetween if item_count > 1 => {
            (0.0, free_space / (item_count - 1) as f32)
        }
        JustifyContent::SpaceAround if item_count > 0 => {
            let gap = free_space / item_count as f32;
            (gap / 2.0, gap)
        }
        _ => (0.0, 0.0),
    }
}

fn align_offset(align: AlignItems, line_cross_size: f32, child_cross_size: f32) -> f32 {
    match align {
        AlignItems::FlexStart | AlignItems::Stretch => 0.0,
        AlignItems::Center => (line_cross_size - child_cross_size).max(0.0) / 2.0,
        AlignItems::FlexEnd => (line_cross_size - child_cross_size).max(0.0),
    }
}
