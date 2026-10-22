//! Flex layout: `display: flex` container layout.

use crate::css::style::PropertyId;
use crate::css::{AffineTransform, ComputedDisplay, ComputedStyle, ComputedValue, StyleResolver};
use crate::dom::{Node, NodeHandle, NodeType};

use super::{
    AlignItems, BoxDimensions, EdgeSizes, FlexDirection, FlexWrap, JustifyContent, LayoutBox, Rect,
    edge_sizes, explicit_length, intrinsic_width, is_out_of_flow_positioned,
    layout_positioned_child, overflow, resolved_length, sort_children_by_z_index,
    translate_layout_box_to_outer, visibility, z_index,
};

mod anonymous;

#[cfg(test)]
thread_local! {
    static LAYOUT_VISITS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

#[derive(Debug, Clone)]
struct FlexItemSpec {
    node: NodeHandle,
    /// The flex container's content width, independent of this item's allocation.
    percentage_basis: f32,
    order: i32,
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
    viewport: super::LayoutViewport,
    containing_height: f32,
    used_height: Option<super::UsedHeight>,
) -> Option<LayoutBox> {
    #[cfg(test)]
    LAYOUT_VISITS.with(|visits| visits.set(visits.get().map(|count| count + 1)));
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

    let (items, positioned_children, available_main_size) = collect_flex_items(
        node,
        resolver,
        direction,
        align,
        width,
        &style,
        definite_height,
        specified_height,
        used_height,
        containing_height,
        padding,
        border,
        main_gap,
    );

    let lines = build_flex_lines(&items, available_main_size, wrap, main_gap);
    let mut children = Vec::new();
    let mut cross_cursor = y;
    let mut column_main_end = y;
    let line_count = lines.len();

    for (line_index, line) in lines.into_iter().enumerate() {
        let (mut laid_out, mut line_cross_size, fixed_main_gap) = resolve_line_main_sizes(
            &line,
            resolver,
            direction,
            align,
            wrap,
            width,
            definite_height,
            available_main_size,
            main_gap,
            viewport,
        );

        if direction == FlexDirection::Column {
            let available_main_for_items = (available_main_size - fixed_main_gap).max(0.0);
            line_cross_size = grow_column_line_items(
                &mut laid_out,
                resolver,
                available_main_for_items,
                definite_height,
                viewport,
            );
        }

        line_cross_size = resolve_line_cross_size(
            &mut laid_out,
            resolver,
            direction,
            wrap,
            align,
            &style,
            used_height,
            specified_height,
            containing_height,
            padding,
            border,
            width,
            line_cross_size,
            viewport,
        );

        let main_cursor = position_line_items(
            laid_out,
            &mut children,
            resolver,
            direction,
            justify,
            align,
            x,
            y,
            cross_cursor,
            line_cross_size,
            main_gap,
            fixed_main_gap,
            available_main_size,
        );

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
            node,
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
    let viewport = viewport.after_sizing(&style, dimensions, &mut children, resolver);
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
    children.sort_by_cached_key(|child| {
        if child.node.node_type() == NodeType::Element {
            item_order(&resolver.computed_style(&child.node))
        } else {
            0
        }
    });
    sort_children_by_z_index(&mut children);

    Some(LayoutBox {
        node: node.clone(),
        pseudo: None,
        dimensions,
        visibility: visibility(&style),
        overflow: overflow(&style),
        position_scheme: super::position_scheme(&style),
        fixed_containing_block: super::establishes_fixed_containing_block(&style),
        z_index: z_index(&style),
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

/// Walks a flex container's children, building `FlexItemSpec`s for in-flow
/// items and separating out absolutely positioned children (CSS Flexbox
/// Section 4, Flex Items), and computes the flex line's available main size.
fn collect_flex_items(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    direction: FlexDirection,
    align: AlignItems,
    width: f32,
    style: &ComputedStyle,
    definite_height: Option<f32>,
    specified_height: Option<f32>,
    used_height: Option<super::UsedHeight>,
    containing_height: f32,
    padding: EdgeSizes,
    border: EdgeSizes,
    main_gap: f32,
) -> (Vec<FlexItemSpec>, Vec<(NodeHandle, ComputedStyle)>, f32) {
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
        if child_style.is_display_none() {
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
            percentage_basis: width,
            order: item_order(&child_style),
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

    items.sort_by_key(|item| item.order);
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

    (items, positioned_children, available_main_size)
}

/// Resolves each flex item's used main size for one line and performs the
/// line's first layout pass (CSS Flexbox Section 9.3, Main Size
/// Determination).
fn resolve_line_main_sizes<'a>(
    line: &FlexLine<'a>,
    resolver: &mut StyleResolver,
    direction: FlexDirection,
    align: AlignItems,
    wrap: FlexWrap,
    width: f32,
    definite_height: Option<f32>,
    available_main_size: f32,
    main_gap: f32,
    viewport: super::LayoutViewport,
) -> (Vec<LaidOutFlexItem<'a>>, f32, f32) {
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
        let layout_child = layout_item(item, resolver, child_containing, viewport, stretch_height);
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

    (laid_out, line_cross_size, fixed_main_gap)
}

/// Grows column-direction flex items to fill the line (the flexible-length
/// resolution pass for `flex-direction: column`), and returns the line's
/// resulting cross size.
fn grow_column_line_items(
    laid_out: &mut [LaidOutFlexItem],
    resolver: &mut StyleResolver,
    available_main_for_items: f32,
    definite_height: Option<f32>,
    viewport: super::LayoutViewport,
) -> f32 {
    let percentage_basis = definite_height.unwrap_or(0.0);
    let heights = grown_column_heights(
        laid_out,
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
        let definite =
            definite_height.is_some() || flex_basis(&child_style, FlexDirection::Column).is_some();
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
    laid_out
        .iter()
        .map(|item| item.layout.total_width())
        .fold(0.0, f32::max)
}

/// Resolves a flex line's final cross size and, for row-direction
/// containers, stretches items whose cross size is `auto` to fill it
/// (CSS Flexbox Section 9.4, Cross Size Determination).
fn resolve_line_cross_size(
    laid_out: &mut [LaidOutFlexItem],
    resolver: &mut StyleResolver,
    direction: FlexDirection,
    wrap: FlexWrap,
    align: AlignItems,
    style: &ComputedStyle,
    used_height: Option<super::UsedHeight>,
    specified_height: Option<f32>,
    containing_height: f32,
    padding: EdgeSizes,
    border: EdgeSizes,
    width: f32,
    mut line_cross_size: f32,
    viewport: super::LayoutViewport,
) -> f32 {
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
        for laid_out_item in laid_out.iter_mut() {
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
            if child.dimensions.content.height == height
                && (used_height.is_some() || unchanged_stretch_is_invariant(child, resolver))
            {
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

    line_cross_size
}

/// Proves a narrow height-independent subtree: unwrapped rows with auto or
/// absolute heights ending in explicitly sized or plain text leaves. Making an unchanged
/// height definite cannot change these descendants. Other formatting contexts
/// and percentage heights still require the normal stretch pass.
fn unchanged_stretch_is_invariant(layout: &LayoutBox, resolver: &mut StyleResolver) -> bool {
    if layout.pseudo.is_some() {
        return false;
    }
    let style = resolver.computed_style(&layout.node);
    if style.get_by_id(PropertyId::Position).is_some_and(
        |value| !matches!(value, ComputedValue::Keyword(keyword) if keyword == "static"),
    ) {
        return false;
    }
    let absolute_height = matches!(
        style.get_by_id(PropertyId::Height),
        Some(ComputedValue::Px(_))
    );
    let auto_height = style
        .get_by_id(PropertyId::Height)
        .is_none_or(|value| matches!(value, ComputedValue::Keyword(keyword) if keyword == "auto"));
    if (!absolute_height && !auto_height)
        || ![PropertyId::MinHeight, PropertyId::MaxHeight].iter().all(|id| {
            style.get_by_id(*id).is_none_or(|value| {
                matches!(value, ComputedValue::Px(_) | ComputedValue::Number(_))
                    || matches!(value, ComputedValue::Keyword(keyword) if keyword == "auto" || keyword == "none")
            })
        })
    {
        return false;
    }
    if layout.children.is_empty() {
        return absolute_height
            || (auto_height && unchanged_text_leaf_is_invariant(layout, &style));
    }
    matches!(
        style.display(),
        Some(ComputedDisplay::Flex | ComputedDisplay::InlineFlex)
    ) && flex_direction(&style) == FlexDirection::Row
        && flex_wrap(&style) == FlexWrap::NoWrap
        && layout
            .children
            .iter()
            .all(|child| unchanged_stretch_is_invariant(child, resolver))
}

/// Text wrapping depends on the unchanged inline width, not on whether the
/// same horizontal block height has become definite. Exclude atomic/replaced
/// content, generated fragments, vertical flow and aspect-ratio sizing.
fn unchanged_text_leaf_is_invariant(layout: &LayoutBox, style: &ComputedStyle) -> bool {
    !super::has_replaced_layout_semantics(&layout.node)
        && !super::has_preferred_aspect_ratio(style)
        && style.get_by_id(PropertyId::WritingMode).is_none_or(
            |value| matches!(value, ComputedValue::Keyword(keyword) if keyword == "horizontal-tb"),
        )
        && layout
            .node
            .layout_child_nodes()
            .iter()
            .all(|node| node.node_type() == NodeType::Text)
        && layout.lines.iter().all(|line| {
            line.fragments.iter().all(|fragment| {
                matches!(&fragment.content, super::InlineFragmentContent::Text(_))
                    && fragment.style.pseudo.is_none()
            })
        })
}

/// Positions a flex line's items along the main axis, applying
/// justify-content, auto margins and cross-axis alignment offsets, then
/// appends each item's laid-out box to `children` (CSS Flexbox Section 9.5,
/// Main-Axis Alignment).  Returns the main-axis position immediately past
/// the last item.
fn position_line_items(
    laid_out: Vec<LaidOutFlexItem>,
    children: &mut Vec<LayoutBox>,
    resolver: &mut StyleResolver,
    direction: FlexDirection,
    justify: JustifyContent,
    align: AlignItems,
    x: f32,
    y: f32,
    cross_cursor: f32,
    line_cross_size: f32,
    main_gap: f32,
    fixed_main_gap: f32,
    available_main_size: f32,
) -> f32 {
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
        translate_layout_box_to_outer(&mut child, outer_x, outer_y, resolver);
        children.push(child);

        main_cursor += child_main_size;
        if item.main_end_auto {
            main_cursor += auto_margin;
        }
        if index + 1 < laid_out_count {
            main_cursor += main_gap + justify_gap;
        }
    }

    main_cursor
}

pub(super) fn is_flex_container(style: &ComputedStyle) -> bool {
    style.display() == Some(ComputedDisplay::Flex)
}

pub(super) fn flex_direction(style: &ComputedStyle) -> FlexDirection {
    match style.get_by_id(PropertyId::FlexDirection) {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("column") => {
            FlexDirection::Column
        }
        _ => FlexDirection::Row,
    }
}

fn flex_wrap(style: &ComputedStyle) -> FlexWrap {
    match style.get_by_id(PropertyId::FlexWrap) {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("wrap") => {
            FlexWrap::Wrap
        }
        _ => FlexWrap::NoWrap,
    }
}

fn main_axis_auto_margins(style: &ComputedStyle, direction: FlexDirection) -> (bool, bool) {
    match direction {
        FlexDirection::Row => (
            super::is_auto(style.get_by_id(PropertyId::MarginLeft)),
            super::is_auto(style.get_by_id(PropertyId::MarginRight)),
        ),
        FlexDirection::Column => (
            super::is_auto(style.get_by_id(PropertyId::MarginTop)),
            super::is_auto(style.get_by_id(PropertyId::MarginBottom)),
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
    match style.get_by_id(PropertyId::JustifyContent) {
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
    match style.get_by_id(PropertyId::AlignItems) {
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
    match style.get_by_id(PropertyId::AlignSelf) {
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
    let auto_height = style.get_by_id(PropertyId::Height).is_none()
        || matches!(style.get_by_id(PropertyId::Height), Some(ComputedValue::Keyword(value)) if value == "auto");
    let auto_margin = [PropertyId::MarginTop, PropertyId::MarginBottom].iter().any(
        |id| matches!(style.get_by_id(*id), Some(ComputedValue::Keyword(value)) if value == "auto"),
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
        if child_style.is_display_none() {
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

fn item_order(style: &ComputedStyle) -> i32 {
    match style.get_by_id(PropertyId::Order) {
        Some(ComputedValue::Number(value)) if value.is_finite() && value.fract() == 0.0 => {
            *value as i32
        }
        _ => 0,
    }
}

fn flex_grow(style: &ComputedStyle) -> f32 {
    match style.get_by_id(PropertyId::FlexGrow) {
        Some(ComputedValue::Number(value)) => *value,
        Some(ComputedValue::Px(value)) => *value,
        _ => 0.0,
    }
}

fn flex_shrink(style: &ComputedStyle) -> f32 {
    match style.get_by_id(PropertyId::FlexShrink) {
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
    viewport: super::LayoutViewport,
    used_height: Option<super::UsedHeight>,
) -> Option<LayoutBox> {
    if !item.text_nodes.is_empty() {
        return Some(anonymous::layout(item, resolver, containing, used_height));
    }
    super::layout_element_with_percentage_basis(
        &item.node,
        resolver,
        containing,
        viewport,
        None,
        used_height,
        item.percentage_basis,
    )
}

#[cfg(test)]
mod layout_cost_tests {
    use super::*;
    use crate::css::{Origin, parse_stylesheet};

    #[test]
    fn nested_auto_height_rows_have_bounded_layout_work() {
        const DEPTH: usize = 8;
        let root = NodeHandle::element("div");
        let mut parent = root.clone();
        for _ in 1..DEPTH {
            let child = NodeHandle::element("div");
            parent.append_child(child.clone());
            parent = child;
        }
        let leaf = NodeHandle::element("span");
        parent.append_child(leaf);
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("div { display:flex; width:100px } span { width:20px; height:20px }")
                .unwrap(),
        );
        LAYOUT_VISITS.with(|visits| visits.set(Some(0)));
        let layout = super::super::layout_tree(
            &root,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 200.0,
            },
        )
        .unwrap();
        let visits = LAYOUT_VISITS.with(|visits| visits.replace(None).unwrap());
        eprintln!("{DEPTH} nested rows: {visits} flex layouts");
        assert_eq!(layout.dimensions.content.height, 20.0);
        assert!(
            visits <= DEPTH * 3,
            "{DEPTH} nested rows required {visits} layouts"
        );
    }

    #[test]
    fn nested_text_rows_have_bounded_layout_work() {
        const DEPTH: usize = 8;
        let root = NodeHandle::element("div");
        let mut parent = root.clone();
        for _ in 1..DEPTH {
            let child = NodeHandle::element("div");
            parent.append_child(child.clone());
            parent = child;
        }
        let label = NodeHandle::element("span");
        label.append_child(NodeHandle::text("label"));
        parent.append_child(label);
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { display:flex; width:100px } span { font-size:10px; line-height:20px }",
            )
            .unwrap(),
        );
        LAYOUT_VISITS.with(|visits| visits.set(Some(0)));
        let layout = super::super::layout_tree(
            &root,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 200.0,
            },
        )
        .unwrap();
        let visits = LAYOUT_VISITS.with(|visits| visits.replace(None).unwrap());
        eprintln!("{DEPTH} nested text rows: {visits} flex layouts");
        assert_eq!(layout.dimensions.content.height, 20.0);
        assert!(
            visits <= DEPTH * 3,
            "{DEPTH} nested text rows required {visits} layouts"
        );
    }

    #[test]
    fn wrapped_text_keeps_its_lines_when_a_row_height_becomes_definite() {
        for stretch_height in [None, Some(160.0)] {
            let root = NodeHandle::element("div");
            let row = NodeHandle::element("div");
            let label = NodeHandle::element("span");
            label.append_child(NodeHandle::text(
                "alpha beta gamma delta epsilon zeta eta theta",
            ));
            row.append_child(label);
            root.append_child(row);
            if let Some(height) = stretch_height {
                let spacer = NodeHandle::element("i");
                spacer.set_attribute("style", &format!("width:0px;height:{height}px"));
                root.append_child(spacer);
            }
            let mut resolver = StyleResolver::new();
            resolver.add_stylesheet(Origin::Author, parse_stylesheet(
                "div { display:flex; width:100px } span { font:10px monospace; line-height:20px }",
            ).unwrap());
            let layout = super::super::layout_tree(
                &root,
                &mut resolver,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height: 200.0,
                },
            )
            .unwrap();
            let label = &layout.children[0].children[0];
            assert!(label.lines.len() > 1, "text must wrap at 100px");
            assert_eq!(
                label.dimensions.content.height,
                layout.dimensions.content.height
            );
            if let Some(height) = stretch_height {
                assert_eq!(label.dimensions.content.height, height);
            } else {
                assert_eq!(
                    label.dimensions.content.height,
                    label.lines.len() as f32 * 20.0
                );
            }
        }
    }

    #[test]
    fn percentage_descendants_still_resolve_after_auto_row_stretch() {
        let root = NodeHandle::element("div");
        let nested = NodeHandle::element("div");
        let percentage = NodeHandle::element("span");
        nested.append_child(percentage);
        root.append_child(nested);
        root.append_child(NodeHandle::element("i"));
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(
            "div { display:flex; width:100px } span { width:20px; height:50% } i { width:20px; height:80px }",
        ).unwrap());
        let layout = super::super::layout_tree(
            &root,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 200.0,
            },
        )
        .unwrap();
        let nested = &layout.children[0];
        assert_eq!(nested.dimensions.content.height, 80.0);
        assert_eq!(nested.children[0].dimensions.content.height, 40.0);
        assert!(!unchanged_stretch_is_invariant(nested, &mut resolver));
    }
}
