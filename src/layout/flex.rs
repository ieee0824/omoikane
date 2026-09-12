//! Flex layout: `display: flex` container layout.

use crate::css::{AffineTransform, ComputedStyle, ComputedValue, StyleResolver};
use crate::dom::{Node, NodeHandle, NodeType};

use super::{
    AlignItems, BoxDimensions, EdgeSizes, FlexDirection, FlexWrap, JustifyContent, LayoutBox, Rect,
    edge_sizes, explicit_length, intrinsic_width, is_display_none, is_out_of_flow_positioned,
    layout_positioned_child, overflow, resolved_length, sort_children_by_z_index,
    translate_layout_box_to_outer, visibility, z_index,
};

mod anonymous;

#[derive(Debug, Clone)]
struct FlexItemSpec {
    node: NodeHandle,
    text_nodes: Vec<NodeHandle>,
    base_main_size: f32,
    min_main_size: f32,
    main_start_auto: bool,
    main_end_auto: bool,
    explicit_cross_size: Option<f32>,
    flex_grow: f32,
    flex_shrink: f32,
    align_self: Option<AlignItems>,
}

struct LaidOutFlexItem<'a> {
    spec: &'a FlexItemSpec,
    layout: LayoutBox,
    containing: Rect,
    used_height: Option<super::UsedHeight>,
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
    containing_height: f32,
    used_height: Option<super::UsedHeight>,
) -> Option<LayoutBox> {
    let direction = flex_direction(&style);
    let wrap = flex_wrap(&style);
    let justify = justify_content(&style);
    let align = align_items(&style);
    let main_gap = flex_main_axis_gap(&style, direction);
    let line_gap = flex_cross_axis_gap(&style, direction);

    let specified_height = resolved_length(&style, "height", containing_height)
        .map(|height| super::border_box_adjust_height(&style, height, &padding, &border))
        .map(|height| {
            super::clamp_content_height(&style, height, containing_height, padding, border)
        });
    let definite_height = used_height
        .and_then(super::UsedHeight::percentage_basis)
        .or(specified_height);

    let mut items = Vec::new();
    let mut positioned_children = Vec::new();
    let main_basis = match direction {
        FlexDirection::Row => width,
        FlexDirection::Column => definite_height.unwrap_or(0.0),
    };
    let mut pending_text = Vec::new();
    for child in node.layout_child_nodes() {
        if child.node_type() == NodeType::Text {
            pending_text.push(child);
            continue;
        }
        if child.node_type() != NodeType::Element {
            continue;
        }
        let child_style = resolver.computed_style(&child);
        if is_display_none(&child_style) {
            continue;
        }
        anonymous::append(
            &mut items,
            &mut pending_text,
            resolver,
            direction,
            align,
            width,
        );
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
        let (main_start_auto, main_end_auto) = main_axis_auto_margins(&child_style, direction);
        items.push(FlexItemSpec {
            node: child,
            text_nodes: Vec::new(),
            base_main_size,
            min_main_size,
            main_start_auto,
            main_end_auto,
            explicit_cross_size: explicit_cross_size(&child_style, direction),
            flex_grow: flex_grow(&child_style),
            flex_shrink: flex_shrink(&child_style),
            align_self: align_self(&child_style),
        });
    }

    anonymous::append(
        &mut items,
        &mut pending_text,
        resolver,
        direction,
        align,
        width,
    );

    let available_main_size = match direction {
        FlexDirection::Row => width,
        FlexDirection::Column => {
            let natural = items.iter().map(|item| item.base_main_size).sum::<f32>()
                + main_gap * items.len().saturating_sub(1) as f32;
            let height = used_height
                .map(|height| height.value)
                .or(specified_height)
                .unwrap_or(natural);
            super::clamp_content_height(&style, height, containing_height, padding, border)
        }
    };
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
            let child_style = resolver.computed_style(&item.node);
            let column_width = item.explicit_cross_size.unwrap_or_else(|| {
                if direction == FlexDirection::Column
                    && item.align_self.unwrap_or(align) != AlignItems::Stretch
                    && resolved_length(&child_style, "width", width).is_none()
                {
                    (intrinsic_width(&item.node, resolver)
                        + edge_sizes(&child_style, "margin").horizontal())
                    .min(width)
                } else {
                    width
                }
            });
            let child_containing = match direction {
                FlexDirection::Row => Rect {
                    x: 0.0,
                    y: 0.0,
                    width: *main_size,
                    height: definite_height.or(item.explicit_cross_size).unwrap_or(0.0),
                },
                FlexDirection::Column => Rect {
                    x: 0.0,
                    y: 0.0,
                    width: column_width,
                    height: *main_size,
                },
            };

            // A definite single-line row already knows its stretch height.
            // Apply it on the first pass so nested stretched flex containers
            // do not recursively double their layout work.
            let stretch_height = if direction == FlexDirection::Row
                && wrap == FlexWrap::NoWrap
                && stretches_height(&child_style, item.align_self.unwrap_or(align))
            {
                definite_height.map(|height| super::UsedHeight {
                    value: (height
                        - edge_sizes(&child_style, "margin").vertical()
                        - edge_sizes(&child_style, "padding").vertical()
                        - edge_sizes(&child_style, "border").vertical())
                    .max(0.0),
                    definite: true,
                })
            } else {
                None
            };
            let layout_child =
                layout_item(item, resolver, child_containing, viewport, stretch_height);
            if let Some(layout_child) = layout_child {
                let cross_size = match direction {
                    FlexDirection::Row => layout_child.total_height(),
                    FlexDirection::Column => layout_child.total_width(),
                };
                line_cross_size = line_cross_size.max(cross_size);
                laid_out.push(LaidOutFlexItem {
                    spec: item,
                    layout: layout_child,
                    containing: child_containing,
                    used_height: stretch_height,
                });
            }
        }

        if direction == FlexDirection::Column {
            let percentage_basis = definite_height.unwrap_or(0.0);
            let heights = grown_column_heights(
                &laid_out,
                resolver,
                available_main_for_items,
                percentage_basis,
            );
            for (laid_out_item, height) in laid_out.iter_mut().zip(heights) {
                let LaidOutFlexItem {
                    spec: item,
                    layout: child,
                    containing,
                    ..
                } = laid_out_item;
                let child_style = resolver.computed_style(&item.node);
                let definite = definite_height.is_some()
                    || flex_basis(&child_style, FlexDirection::Column).is_some();
                // Even an unchanged auto height needs a second layout when
                // flex layout has made its percentage basis definite.
                if height != child.dimensions.content.height
                    || (definite && explicit_length(&child_style, "height").is_none())
                {
                    if let Some(reflowed) = layout_item(
                        item,
                        resolver,
                        Rect {
                            height: percentage_basis,
                            ..*containing
                        },
                        viewport,
                        Some(super::UsedHeight {
                            value: height,
                            definite,
                        }),
                    ) {
                        *child = reflowed;
                    }
                }
            }
            line_cross_size = laid_out
                .iter()
                .map(|item| item.layout.total_width())
                .fold(0.0, f32::max);
        }

        // A single flex line uses the container's cross size. Using only the
        // tallest item here makes align-items:center/flex-end ineffective in
        // a definite-height row (and in a definite-width column).
        if wrap == FlexWrap::NoWrap {
            line_cross_size = match direction {
                FlexDirection::Row => super::clamp_content_height(
                    &style,
                    used_height
                        .map(|height| height.value)
                        .or(specified_height)
                        .unwrap_or(line_cross_size),
                    containing_height,
                    padding,
                    border,
                ),
                // A non-wrapping column has one flex line whose cross size is
                // the container's content width.  Using the widest child's
                // intrinsic width here makes align-items:center/flex-end align
                // inside that child-sized strip instead of across the column.
                FlexDirection::Column => width,
            };
        }

        if direction == FlexDirection::Row {
            for laid_out_item in &mut laid_out {
                let LaidOutFlexItem {
                    spec: item,
                    layout: child,
                    containing,
                    used_height,
                } = laid_out_item;
                let child_style = resolver.computed_style(&item.node);
                if !stretches_height(&child_style, item.align_self.unwrap_or(align)) {
                    continue;
                }
                let height = (line_cross_size
                    - child.dimensions.margin.vertical()
                    - child.dimensions.padding.vertical()
                    - child.dimensions.border.vertical())
                .max(0.0);
                let height = super::clamp_content_height(
                    &child_style,
                    height,
                    containing.height,
                    child.dimensions.padding,
                    child.dimensions.border,
                );
                if used_height.is_some() && child.dimensions.content.height == height {
                    continue;
                }
                if let Some(reflowed) = layout_item(
                    item,
                    resolver,
                    *containing,
                    viewport,
                    Some(super::UsedHeight {
                        value: height,
                        definite: true,
                    }),
                ) {
                    *child = reflowed;
                }
            }
        }

        let (total_main_size, auto_margin_count) =
            laid_out
                .iter()
                .fold((0.0f32, 0usize), |(total_size, auto_margins), item| {
                    let item_size = match direction {
                        FlexDirection::Row => item.layout.total_width(),
                        FlexDirection::Column => item.layout.total_height(),
                    };
                    (
                        total_size + item_size,
                        auto_margins
                            + usize::from(item.spec.main_start_auto)
                            + usize::from(item.spec.main_end_auto),
                    )
                });
        let used_main_size = total_main_size + fixed_main_gap;
        let positive_free_space = (available_main_size - used_main_size).max(0.0);
        let auto_margin = if auto_margin_count > 0 && positive_free_space > 0.0 {
            positive_free_space / auto_margin_count as f32
        } else {
            0.0
        };
        let (line_start, justify_gap) = if auto_margin > 0.0 {
            (0.0, 0.0)
        } else {
            justify_offsets(justify, available_main_size, used_main_size, laid_out.len())
        };

        let mut main_cursor = match direction {
            FlexDirection::Row => x + line_start,
            FlexDirection::Column => y + line_start,
        };

        let laid_out_count = laid_out.len();
        for (index, laid_out_item) in laid_out.into_iter().enumerate() {
            let LaidOutFlexItem {
                spec: item,
                layout: mut child,
                ..
            } = laid_out_item;
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

            if item.main_start_auto {
                main_cursor += auto_margin;
            }

            let (outer_x, outer_y) = match direction {
                FlexDirection::Row => (main_cursor, cross_cursor + cross_offset),
                FlexDirection::Column => (x + cross_offset, main_cursor),
            };
            translate_layout_box_to_outer(&mut child, outer_x, outer_y);
            children.push(child);

            main_cursor += child_main_size;
            if item.main_end_auto {
                main_cursor += auto_margin;
            }
            if index + 1 < laid_out_count {
                main_cursor += main_gap + justify_gap;
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

    let auto_height = if super::has_block_size_containment(&style) {
        0.0
    } else {
        match direction {
            FlexDirection::Row => cross_cursor - y,
            FlexDirection::Column => column_main_end - y,
        }
    };
    let content_height = used_height.map(|height| height.value).unwrap_or_else(|| {
        super::resolve_content_height(
            &style,
            containing_height,
            padding,
            border,
            y,
            y + auto_height,
        )
    });
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
        transform: AffineTransform::identity(),
        needs_scroll_translation: false,
        paint_scroll: None,
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

fn main_axis_auto_margins(style: &ComputedStyle, direction: FlexDirection) -> (bool, bool) {
    match direction {
        FlexDirection::Row => (
            super::is_auto(style.get("margin-left")),
            super::is_auto(style.get("margin-right")),
        ),
        FlexDirection::Column => (
            super::is_auto(style.get("margin-top")),
            super::is_auto(style.get("margin-bottom")),
        ),
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

/// Stretch applies only to an auto cross size without auto cross margins.
fn stretches_height(style: &ComputedStyle, align: AlignItems) -> bool {
    let auto_height = style.get("height").is_none()
        || matches!(style.get("height"), Some(ComputedValue::Keyword(value)) if value == "auto");
    let auto_margin = ["margin-top", "margin-bottom"].iter().any(
        |name| matches!(style.get(name), Some(ComputedValue::Keyword(value)) if value == "auto"),
    );
    align == AlignItems::Stretch && auto_height && !auto_margin
}

/// Distributes remaining height, freezing items at their max-height before
/// redistributing the unused share to the other growing items.
fn grown_column_heights(
    items: &[LaidOutFlexItem<'_>],
    resolver: &mut StyleResolver,
    available: f32,
    percentage_basis: f32,
) -> Vec<f32> {
    let mut heights: Vec<_> = items
        .iter()
        .map(|item| item.layout.dimensions.content.height)
        .collect();
    let mut free = (available
        - items
            .iter()
            .map(|item| item.layout.total_height())
            .sum::<f32>())
    .max(0.0);
    let mut growing: Vec<_> = items.iter().map(|item| item.spec.flex_grow > 0.0).collect();
    while free > 0.0 {
        let total_grow: f32 = items
            .iter()
            .zip(&growing)
            .filter(|(_, active)| **active)
            .map(|(item, _)| item.spec.flex_grow)
            .sum();
        if total_grow == 0.0 {
            break;
        }
        let mut distributed = 0.0;
        let mut clamped = false;
        for (index, laid_out_item) in items.iter().enumerate() {
            let LaidOutFlexItem {
                spec: item,
                layout: child,
                ..
            } = laid_out_item;
            if !growing[index] {
                continue;
            }
            let requested = heights[index] + free * item.flex_grow / total_grow;
            let style = resolver.computed_style(&item.node);
            let height = super::clamp_content_height(
                &style,
                requested,
                percentage_basis,
                child.dimensions.padding,
                child.dimensions.border,
            );
            distributed += (height - heights[index]).max(0.0);
            heights[index] = height;
            if height < requested {
                growing[index] = false;
                clamped = true;
            }
        }
        if !clamped || distributed <= 0.0 {
            break;
        }
        free = (free - distributed).max(0.0);
    }
    heights
}

// Parent flex/table/shrink-to-fit sizing must count the same anonymous items
// as layout. Otherwise nested text is squeezed into the adjacent icon's width.
pub(super) fn intrinsic_content_width(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
) -> f32 {
    let direction = flex_direction(style);
    let mut content_width = 0.0f32;
    let mut count = 0usize;
    let mut add = |width: f32| {
        content_width = match direction {
            FlexDirection::Row => content_width + width,
            FlexDirection::Column => content_width.max(width),
        };
        count += 1;
    };
    let mut pending = Vec::new();
    for child in node.layout_child_nodes() {
        if child.node_type() == NodeType::Text {
            pending.push(child);
            continue;
        }
        if child.node_type() != NodeType::Element {
            continue;
        }
        let child_style = resolver.computed_style(&child);
        if is_display_none(&child_style) {
            continue;
        }
        if let Some(text) = anonymous::take_text(&mut pending) {
            add(anonymous::max_width(&text, resolver));
        }
        if !is_out_of_flow_positioned(&child_style) {
            add(intrinsic_width(&child, resolver));
        }
    }
    if let Some(text) = anonymous::take_text(&mut pending) {
        add(anonymous::max_width(&text, resolver));
    }
    if direction == FlexDirection::Row {
        content_width += flex_main_axis_gap(style, direction) * count.saturating_sub(1) as f32;
    }
    content_width
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

fn layout_item(
    item: &FlexItemSpec,
    resolver: &mut StyleResolver,
    containing: Rect,
    viewport: Rect,
    used_height: Option<super::UsedHeight>,
) -> Option<LayoutBox> {
    if !item.text_nodes.is_empty() {
        return Some(anonymous::layout(item, resolver, containing, used_height));
    }
    super::layout_element(
        &item.node,
        resolver,
        containing,
        viewport,
        None,
        None,
        used_height,
    )
}
