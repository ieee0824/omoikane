//! CSS multi-column formatting context.
//!
//! The ordinary block formatter first lays content out at the used column
//! width. This module then fragments its top-level flow items into column
//! boxes. Keeping the ordinary formatter as the source of line breaking,
//! margins, floats, and nested formatting contexts avoids a second, divergent
//! block layout implementation.

use std::collections::{HashMap, HashSet};

use super::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ColumnGeometry {
    pub(super) count: usize,
    pub(super) width: f32,
    pub(super) gap: f32,
    pub(super) rtl: bool,
}

pub(super) fn is_multicol_container(style: &ComputedStyle) -> bool {
    let is_auto = |property| match style.get(property) {
        None => true,
        Some(ComputedValue::Keyword(value)) => value.eq_ignore_ascii_case("auto"),
        _ => false,
    };
    !is_auto("column-count") || !is_auto("column-width")
}

pub(super) fn column_geometry(
    style: &ComputedStyle,
    available_inline_size: f32,
) -> Option<ColumnGeometry> {
    if !is_multicol_container(style) || available_inline_size <= 0.0 {
        return None;
    }
    let specified_count = match style.get("column-count") {
        Some(ComputedValue::Number(value)) if value.is_finite() && *value >= 1.0 => {
            Some((*value as usize).clamp(1, 1000))
        }
        _ => None,
    };
    let specified_width = match style.get("column-width") {
        Some(ComputedValue::Px(value)) if value.is_finite() && *value >= 0.0 => Some(*value),
        _ => None,
    };
    if specified_count.is_none() && specified_width.is_none() {
        return None;
    }
    let gap = match style.get("column-gap") {
        Some(ComputedValue::Px(value)) if value.is_finite() => (*value).max(0.0),
        _ => super::inline::font_size(style),
    };
    let fitting_count = specified_width.map(|column_width| {
        let denominator = column_width.max(1.0) + gap;
        (((available_inline_size + gap) / denominator).floor() as usize).clamp(1, 1000)
    });
    let count = match (specified_count, fitting_count) {
        (Some(count), Some(fitting)) => count.min(fitting),
        (Some(count), None) => count,
        (None, Some(fitting)) => fitting,
        (None, None) => 1,
    };
    let width =
        ((available_inline_size - gap * count.saturating_sub(1) as f32) / count as f32).max(0.0);
    Some(ColumnGeometry {
        count,
        width,
        gap,
        rtl: direction_is_rtl(style),
    })
}

#[derive(Debug, Clone, Copy)]
enum FlowItem {
    Line(usize),
    Child(usize),
    ChildLines {
        child: usize,
        start: usize,
        end: usize,
    },
    Spanner(usize),
}

#[derive(Debug, Clone, Copy)]
struct FlowAnchor {
    item: FlowItem,
    top: f32,
    height: f32,
    break_before: bool,
    break_after: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn layout_multicol_children(
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
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    used_height: Option<UsedHeight>,
) -> BlockChildrenResult {
    if is_vertical_writing(style) {
        return layout_vertical_multicol_children(
            node,
            resolver,
            style,
            padding,
            border,
            margin,
            x,
            y,
            width,
            containing_height,
            viewport,
            positioned_ancestor,
            used_height,
        );
    }

    let Some(geometry) = column_geometry(style, width) else {
        return margins::layout_children(
            node,
            resolver,
            style,
            padding,
            border,
            margin,
            x,
            y,
            width,
            containing_height,
            viewport,
            positioned_ancestor,
            used_height,
        );
    };

    let mut result = margins::layout_children(
        node,
        resolver,
        style,
        padding,
        border,
        margin,
        x,
        y,
        geometry.width,
        containing_height,
        viewport,
        positioned_ancestor,
        used_height,
    );

    // Apply the ordinary margin formatter's corrections before column
    // placement. Returning those corrections after changing columns would
    // shift fragments on the physical y axis a second time.
    let shifts = std::mem::take(&mut result.child_shifts);
    let mut shifts = shifts.into_iter().peekable();
    for (index, child) in result.children.iter_mut().enumerate() {
        let correction = if shifts
            .peek()
            .is_some_and(|(child_index, _)| *child_index == index)
        {
            shifts.next().expect("peeked child shift").1
        } else {
            0.0
        };
        margins::shift_flow(
            child,
            correction,
            resolver,
            establishes_positioned_containing_block(style),
            establishes_fixed_containing_block(style),
        );
    }

    // Spanners are measured at the full multicol inline size before balancing.
    // Their narrow provisional height must not be used to place the following
    // column row.
    for index in 0..result.children.len() {
        let span_node = result.children[index].node.clone();
        let child_style = resolver.computed_style(&span_node);
        let spans_all = matches!(
            child_style.get("column-span"),
            Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("all")
        );
        if !spans_all {
            continue;
        }
        let old_height = result.children[index].total_height();
        let outer_top = result.children[index].dimensions.content.y
            - result.children[index].dimensions.padding.top
            - result.children[index].dimensions.border.top
            - result.children[index].dimensions.margin.top;
        let old_bottom = outer_top + old_height;
        let containing = Rect {
            x,
            y: outer_top,
            width,
            height: containing_height,
        };
        if let Some(mut spanner) = layout_node(
            &span_node,
            resolver,
            containing,
            viewport,
            positioned_ancestor,
        ) {
            translate_layout_box_to_outer(&mut spanner, x, outer_top, resolver);
            let delta = spanner.total_height() - old_height;
            result.children[index] = spanner;
            if delta != 0.0 {
                for other_index in 0..result.children.len() {
                    if other_index == index {
                        continue;
                    }
                    let other = &mut result.children[other_index];
                    let other_top = other.dimensions.content.y
                        - other.dimensions.padding.top
                        - other.dimensions.border.top
                        - other.dimensions.margin.top;
                    if other_top >= old_bottom {
                        translate_layout_box(other, 0.0, delta, resolver);
                    }
                }
                for line in &mut result.lines {
                    if line.rect.y >= old_bottom {
                        translate_line(line, 0.0, delta);
                    }
                }
                for (_, _, static_position) in &mut result.positioned_children {
                    if static_position.y >= old_bottom {
                        static_position.y += delta;
                    }
                }
                result.cursor_y += delta;
                result.float_bottom += delta;
            }
        }
    }

    let mut atomic_line_by_node = HashMap::new();
    for (line_index, line) in result.lines.iter().enumerate() {
        for fragment in &line.fragments {
            if matches!(fragment.content, InlineFragmentContent::AtomicInline(_)) {
                atomic_line_by_node.insert(fragment.node.identity(), line_index);
            }
        }
    }

    let mut anchors = Vec::new();
    let mut fragmented_children = HashSet::new();
    for (index, line) in result.lines.iter().enumerate() {
        anchors.push(FlowAnchor {
            item: FlowItem::Line(index),
            top: line.rect.y,
            height: line.rect.height.max(0.0),
            break_before: false,
            break_after: false,
        });
    }
    for (index, child) in result.children.iter().enumerate() {
        if atomic_line_by_node.contains_key(&child.node.identity()) {
            continue;
        }
        let child_style = resolver.computed_style(&child.node);
        let outer_top = child.dimensions.content.y
            - child.dimensions.padding.top
            - child.dimensions.border.top
            - child.dimensions.margin.top;
        let spanner = matches!(
            child_style.get("column-span"),
            Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("all")
        );
        if !spanner && can_fragment_child_lines(child, &child_style) {
            let groups = child_line_groups(child, &child_style);
            let last_group = groups.len().saturating_sub(1);
            for (group_index, (start, end)) in groups.into_iter().enumerate() {
                let first = &child.lines[start];
                let last = &child.lines[end - 1];
                anchors.push(FlowAnchor {
                    item: FlowItem::ChildLines {
                        child: index,
                        start,
                        end,
                    },
                    top: first.rect.y,
                    height: (last.rect.y + last.rect.height - first.rect.y).max(0.0),
                    break_before: group_index == 0
                        && forces_column_break(child_style.get("break-before")),
                    break_after: group_index == last_group
                        && forces_column_break(child_style.get("break-after")),
                });
            }
            fragmented_children.insert(index);
            continue;
        }
        anchors.push(FlowAnchor {
            item: if spanner {
                FlowItem::Spanner(index)
            } else {
                FlowItem::Child(index)
            },
            top: outer_top,
            height: child.total_height().max(0.0),
            break_before: forces_column_break(child_style.get("break-before")),
            break_after: forces_column_break(child_style.get("break-after")),
        });
    }
    anchors.sort_by(|left, right| {
        left.top
            .total_cmp(&right.top)
            .then_with(|| flow_item_order(left.item).cmp(&flow_item_order(right.item)))
    });

    if anchors.is_empty() {
        result.cursor_y = y;
        result.float_bottom = y;
        return result;
    }

    let source_bottom = result.cursor_y.max(result.float_bottom);
    let source_height = (source_bottom - y).max(
        anchors
            .iter()
            .map(|anchor| anchor.top + anchor.height - y)
            .fold(0.0, f32::max),
    );
    let explicit_height = used_height.map(|height| height.value).or_else(|| {
        resolved_length(style, "height", containing_height)
            .map(|height| border_box_adjust_height(style, height, &padding, &border))
    });
    let fill_auto = matches!(
        style.get("column-fill"),
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("auto")
    );
    let balanced = balanced_height(&anchors, source_height, geometry.count);
    let column_height = match (explicit_height, fill_auto) {
        (Some(height), true) => height,
        (Some(height), false) => balanced.min(height).max(1.0),
        (None, _) => balanced.max(1.0),
    };

    let mut line_moves = vec![(0.0, 0.0); result.lines.len()];
    let mut child_moves = vec![None; result.children.len()];
    let mut child_line_moves = result
        .children
        .iter()
        .map(|child| vec![None; child.lines.len()])
        .collect::<Vec<_>>();
    let mut spanner_targets = Vec::new();
    let mut source_cursor = y;
    let mut row_y = y;
    let mut column = 0usize;
    let mut local_y = 0.0f32;
    let mut row_has_content = false;
    let mut maximum_bottom = y;
    let mut row_columns = Vec::new();
    let mut maximum_column = 0usize;

    for anchor in anchors {
        let gap = (anchor.top - source_cursor).max(0.0);
        advance_columns(&mut column, &mut local_y, gap, column_height);
        if anchor.break_before && local_y > 0.0 {
            column += 1;
            local_y = 0.0;
        }

        if matches!(anchor.item, FlowItem::Spanner(_)) {
            if row_has_content {
                row_columns.push((row_y, column));
                row_y += column_height;
            }
            column = 0;
            local_y = 0.0;
            row_has_content = false;
            let FlowItem::Spanner(index) = anchor.item else {
                unreachable!()
            };
            spanner_targets.push((index, row_y));
            row_y += anchor.height;
            maximum_bottom = maximum_bottom.max(row_y);
            source_cursor = (anchor.top + anchor.height).max(source_cursor);
            continue;
        }

        if anchor.height <= column_height
            && local_y > 0.0
            && local_y + anchor.height > column_height
        {
            column += 1;
            local_y = 0.0;
        }
        let target_x = column_x(x, width, geometry, column);
        let target_y = row_y + local_y;
        let dx = target_x - x;
        let dy = target_y - anchor.top;
        match anchor.item {
            FlowItem::Line(index) => line_moves[index] = (dx, dy),
            FlowItem::Child(index) => child_moves[index] = Some((dx, dy)),
            FlowItem::ChildLines { child, start, end } => {
                for movement in &mut child_line_moves[child][start..end] {
                    *movement = Some((dx, dy));
                }
            }
            FlowItem::Spanner(_) => unreachable!(),
        }
        local_y += anchor.height;
        row_has_content = true;
        maximum_column = maximum_column.max(column);
        maximum_bottom = maximum_bottom.max(target_y + anchor.height);
        source_cursor = (anchor.top + anchor.height).max(source_cursor);
        if anchor.break_after {
            column += 1;
            local_y = 0.0;
        }
    }

    for (line, &(dx, dy)) in result.lines.iter_mut().zip(&line_moves) {
        translate_line(line, dx, dy);
    }
    for (index, child) in result.children.iter_mut().enumerate() {
        if fragmented_children.contains(&index) {
            let mut atomic_moves = HashMap::new();
            for (line_index, line) in child.lines.iter_mut().enumerate() {
                let movement = child_line_moves[index][line_index].unwrap_or((0.0, 0.0));
                for fragment in &line.fragments {
                    if matches!(fragment.content, InlineFragmentContent::AtomicInline(_)) {
                        atomic_moves.insert(fragment.node.identity(), movement);
                    }
                }
                translate_line(line, movement.0, movement.1);
            }
            for nested in &mut child.children {
                if let Some(&(dx, dy)) = atomic_moves.get(&nested.node.identity()) {
                    translate_layout_box(nested, dx, dy, resolver);
                }
            }
            let decorations = child.dimensions.padding.vertical()
                + child.dimensions.border.vertical()
                + child.dimensions.margin.vertical();
            child.dimensions.content.height = child
                .dimensions
                .content
                .height
                .min((column_height - decorations).max(0.0));
        } else if let Some(&line_index) = atomic_line_by_node.get(&child.node.identity()) {
            let (dx, dy) = line_moves[line_index];
            translate_layout_box(child, dx, dy, resolver);
        } else if let Some((dx, dy)) = child_moves[index] {
            translate_layout_box(child, dx, dy, resolver);
        }
    }

    // A spanning child establishes a fresh column row. It was already laid out
    // at the full multicol width before balancing, so only its final row
    // position changes here.
    for (index, target_y) in spanner_targets {
        let spanner = &mut result.children[index];
        translate_layout_box_to_outer(spanner, x, target_y, resolver);
        maximum_bottom = maximum_bottom.max(target_y + spanner.total_height());
    }

    let balanced_bottom = if row_has_content {
        row_columns.push((row_y, column));
        row_y + column_height
    } else {
        row_y
    };
    result.cursor_y = explicit_height
        .map(|height| y + height)
        .unwrap_or_else(|| balanced_bottom.max(maximum_bottom));
    result.float_bottom = result.cursor_y;
    let used_column_count = geometry.count.max(maximum_column + 1);
    let overflow_width = geometry.width * used_column_count as f32
        + geometry.gap * used_column_count.saturating_sub(1) as f32;
    let mut rules = Vec::new();
    for (row_top, last_column) in row_columns {
        let columns = geometry.count.max(last_column + 1);
        for boundary in 1..columns {
            let offset = boundary as f32 * geometry.width + (boundary as f32 - 0.5) * geometry.gap;
            let center = if geometry.rtl {
                x + width - offset
            } else {
                x + offset
            };
            rules.push(Rect {
                x: center,
                y: row_top,
                width: 0.0,
                height: column_height,
            });
        }
    }
    result.multicol = Some(MultiColumnLayout {
        overflow: Rect {
            x,
            y,
            width: overflow_width.max(width),
            height: (result.cursor_y - y).max(0.0),
        },
        rules,
    });
    result
}

#[allow(clippy::too_many_arguments)]
fn layout_vertical_multicol_children(
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
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    used_height: Option<UsedHeight>,
) -> BlockChildrenResult {
    let inline_size = used_height
        .map(|height| height.value)
        .or_else(|| resolved_length(style, "height", containing_height))
        .or_else(|| (containing_height > 0.0).then_some(containing_height))
        .unwrap_or(1_000_000.0);
    let Some(geometry) = column_geometry(style, inline_size) else {
        return layout_vertical_block_children(
            node,
            resolver,
            style,
            padding,
            border,
            margin,
            x,
            y,
            width,
            containing_height,
            viewport,
            positioned_ancestor,
            used_height,
        );
    };
    let mut result = layout_vertical_block_children(
        node,
        resolver,
        style,
        padding,
        border,
        margin,
        x,
        y,
        width,
        geometry.width,
        viewport,
        positioned_ancestor,
        Some(UsedHeight {
            value: geometry.width,
            definite: true,
        }),
    );
    let vertical_rl = is_vertical_rl(style);
    let mut atomic_line_by_node = HashMap::new();
    for (line_index, line) in result.lines.iter().enumerate() {
        for fragment in &line.fragments {
            if matches!(fragment.content, InlineFragmentContent::AtomicInline(_)) {
                atomic_line_by_node.insert(fragment.node.identity(), line_index);
            }
        }
    }
    let progress = |left: f32, item_width: f32| {
        if vertical_rl {
            x + width - left - item_width
        } else {
            left - x
        }
    };
    let mut anchors = Vec::new();
    let mut line_progress = 0.0f32;
    for (index, line) in result.lines.iter().enumerate() {
        let physical_progress = progress(line.rect.x, line.rect.width);
        let top = physical_progress.max(line_progress);
        anchors.push(FlowAnchor {
            item: FlowItem::Line(index),
            top,
            height: line.rect.width.max(0.0),
            break_before: false,
            break_after: false,
        });
        line_progress = top + line.rect.width.max(0.0);
    }
    for (index, child) in result.children.iter().enumerate() {
        if atomic_line_by_node.contains_key(&child.node.identity()) {
            continue;
        }
        let child_style = resolver.computed_style(&child.node);
        let outer_left = child.dimensions.content.x
            - child.dimensions.padding.left
            - child.dimensions.border.left
            - child.dimensions.margin.left;
        anchors.push(FlowAnchor {
            item: FlowItem::Child(index),
            top: progress(outer_left, child.total_width()),
            height: child.total_width().max(0.0),
            break_before: forces_column_break(child_style.get("break-before")),
            break_after: forces_column_break(child_style.get("break-after")),
        });
    }
    anchors.sort_by(|left, right| left.top.total_cmp(&right.top));
    if anchors.is_empty() {
        result.cursor_y = y;
        result.float_bottom = y;
        return result;
    }
    let source_extent = anchors
        .iter()
        .map(|anchor| anchor.top + anchor.height)
        .fold(0.0, f32::max);
    let fill_auto = matches!(
        style.get("column-fill"),
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("auto")
    );
    let balanced = balanced_height(&anchors, source_extent, geometry.count).max(1.0);
    let column_block_size = if fill_auto {
        width.max(1.0)
    } else {
        balanced.min(width.max(1.0))
    };
    let mut line_moves = vec![(0.0, 0.0); result.lines.len()];
    let mut child_moves = vec![None; result.children.len()];
    let mut source_cursor = 0.0;
    let mut column = 0usize;
    let mut local = 0.0f32;
    let mut maximum_column = 0usize;
    for anchor in anchors {
        advance_columns(
            &mut column,
            &mut local,
            (anchor.top - source_cursor).max(0.0),
            column_block_size,
        );
        if anchor.break_before && local > 0.0 {
            column += 1;
            local = 0.0;
        }
        if anchor.height <= column_block_size
            && local > 0.0
            && local + anchor.height > column_block_size
        {
            column += 1;
            local = 0.0;
        }
        let target_x = if vertical_rl {
            x + width - local - anchor.height
        } else {
            x + local
        };
        let target_y = vertical_column_y(y, inline_size, geometry, column);
        let source_x = match anchor.item {
            FlowItem::Line(index) => result.lines[index].rect.x,
            FlowItem::Child(index) | FlowItem::Spanner(index) => {
                let child = &result.children[index];
                child.dimensions.content.x
                    - child.dimensions.padding.left
                    - child.dimensions.border.left
                    - child.dimensions.margin.left
            }
            FlowItem::ChildLines { .. } => unreachable!(),
        };
        let movement = (target_x - source_x, target_y - y);
        match anchor.item {
            FlowItem::Line(index) => line_moves[index] = movement,
            FlowItem::Child(index) | FlowItem::Spanner(index) => {
                child_moves[index] = Some(movement)
            }
            FlowItem::ChildLines { .. } => unreachable!(),
        }
        local += anchor.height;
        maximum_column = maximum_column.max(column);
        source_cursor = (anchor.top + anchor.height).max(source_cursor);
        if anchor.break_after {
            column += 1;
            local = 0.0;
        }
    }
    for (line, &(dx, dy)) in result.lines.iter_mut().zip(&line_moves) {
        translate_line(line, dx, dy);
    }
    for (index, child) in result.children.iter_mut().enumerate() {
        if let Some(&line_index) = atomic_line_by_node.get(&child.node.identity()) {
            let (dx, dy) = line_moves[line_index];
            translate_layout_box(child, dx, dy, resolver);
        } else if let Some((dx, dy)) = child_moves[index] {
            translate_layout_box(child, dx, dy, resolver);
        }
    }
    let used_columns = geometry.count.max(maximum_column + 1);
    let overflow_height =
        geometry.width * used_columns as f32 + geometry.gap * used_columns.saturating_sub(1) as f32;
    let mut rules = Vec::new();
    for boundary in 1..used_columns {
        let offset = boundary as f32 * geometry.width + (boundary as f32 - 0.5) * geometry.gap;
        let center = if geometry.rtl {
            y + inline_size - offset
        } else {
            y + offset
        };
        rules.push(Rect {
            x: if vertical_rl {
                x + width - column_block_size
            } else {
                x
            },
            y: center,
            width: column_block_size,
            height: 0.0,
        });
    }
    result.cursor_y = y + inline_size;
    result.float_bottom = result.cursor_y;
    result.multicol = Some(MultiColumnLayout {
        overflow: Rect {
            x,
            y,
            width,
            height: overflow_height.max(inline_size),
        },
        rules,
    });
    result
}

fn forces_column_break(value: Option<&ComputedValue>) -> bool {
    matches!(
        value,
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("column")
    )
}

fn avoids_column_break(value: Option<&ComputedValue>) -> bool {
    matches!(
        value,
        Some(ComputedValue::Keyword(keyword))
            if keyword.eq_ignore_ascii_case("avoid")
                || keyword.eq_ignore_ascii_case("avoid-column")
    )
}

fn can_fragment_child_lines(child: &LayoutBox, style: &ComputedStyle) -> bool {
    if child.lines.len() < 2 || avoids_column_break(style.get("break-inside")) {
        return false;
    }
    let atomic_nodes = child
        .lines
        .iter()
        .flat_map(|line| &line.fragments)
        .filter(|fragment| matches!(fragment.content, InlineFragmentContent::AtomicInline(_)))
        .map(|fragment| fragment.node.identity())
        .collect::<HashSet<_>>();
    child
        .children
        .iter()
        .all(|nested| atomic_nodes.contains(&nested.node.identity()))
}

fn child_line_groups(child: &LayoutBox, style: &ComputedStyle) -> Vec<(usize, usize)> {
    let line_count = child.lines.len();
    let count = |name| match style.get(name) {
        Some(ComputedValue::Number(value)) if value.is_finite() && *value >= 1.0 => {
            (*value as usize).min(line_count)
        }
        _ => 2.min(line_count),
    };
    let orphans = count("orphans");
    let widows = count("widows");
    if line_count < orphans + widows {
        return vec![(0, line_count)];
    }
    let mut groups = vec![(0, orphans)];
    for line in orphans..line_count - widows {
        groups.push((line, line + 1));
    }
    groups.push((line_count - widows, line_count));
    groups
}

fn flow_item_order(item: FlowItem) -> usize {
    match item {
        FlowItem::Line(_) => 0,
        FlowItem::Child(_) | FlowItem::ChildLines { .. } | FlowItem::Spanner(_) => 1,
    }
}

fn balanced_height(anchors: &[FlowAnchor], source_height: f32, count: usize) -> f32 {
    let tallest = anchors
        .iter()
        .filter(|anchor| !matches!(anchor.item, FlowItem::Spanner(_)))
        .map(|anchor| anchor.height)
        .fold(0.0, f32::max);
    if tallest == 0.0 {
        return 0.0;
    }
    let count = count.max(1);
    let mut low = tallest;
    let mut high = source_height.max(tallest);
    for _ in 0..24 {
        let candidate = (low + high) / 2.0;
        if maximum_segment_columns(anchors, candidate) <= count {
            high = candidate;
        } else {
            low = candidate;
        }
    }
    // Layout coordinates are kept on a stable 1/64 CSS-pixel grid. Rounding
    // upward preserves the result of the fit predicate.
    (high * 64.0).ceil() / 64.0
}

fn maximum_segment_columns(anchors: &[FlowAnchor], column_height: f32) -> usize {
    let mut maximum = 1usize;
    let mut columns = 1usize;
    let mut local = 0.0f32;
    let mut force_next = false;
    let mut source_cursor = anchors.first().map_or(0.0, |anchor| anchor.top);
    for anchor in anchors {
        if matches!(anchor.item, FlowItem::Spanner(_)) {
            maximum = maximum.max(columns);
            columns = 1;
            local = 0.0;
            force_next = false;
            source_cursor = anchor.top + anchor.height;
            continue;
        }
        let gap = (anchor.top - source_cursor).max(0.0);
        if gap > 0.0 {
            let advance = ((local + gap) / column_height).floor() as usize;
            columns += advance;
            local = (local + gap) - advance as f32 * column_height;
        }
        if force_next {
            columns += 1;
            local = 0.0;
            force_next = false;
        }
        if anchor.break_before && local > 0.0 {
            columns += 1;
            local = 0.0;
        }
        if anchor.height <= column_height && local > 0.0 && local + anchor.height > column_height {
            columns += 1;
            local = 0.0;
        }
        local += anchor.height;
        source_cursor = (anchor.top + anchor.height).max(source_cursor);
        if anchor.break_after {
            force_next = true;
        }
    }
    maximum.max(columns)
}

fn advance_columns(column: &mut usize, local_y: &mut f32, amount: f32, height: f32) {
    *local_y += amount;
    if *local_y >= height {
        let advance = (*local_y / height).floor() as usize;
        *column += advance;
        *local_y -= advance as f32 * height;
    }
}

fn column_x(origin_x: f32, container_width: f32, geometry: ColumnGeometry, column: usize) -> f32 {
    let advance = column as f32 * (geometry.width + geometry.gap);
    if geometry.rtl {
        origin_x + container_width - geometry.width - advance
    } else {
        origin_x + advance
    }
}

fn vertical_column_y(
    origin_y: f32,
    container_height: f32,
    geometry: ColumnGeometry,
    column: usize,
) -> f32 {
    let advance = column as f32 * (geometry.width + geometry.gap);
    if geometry.rtl {
        origin_y + container_height - geometry.width - advance
    } else {
        origin_y + advance
    }
}

fn translate_line(line: &mut LineBox, dx: f32, dy: f32) {
    line.rect.x += dx;
    line.rect.y += dy;
    line.baseline += dy;
    for fragment in &mut line.fragments {
        fragment.rect.x += dx;
        fragment.rect.y += dy;
    }
    if let Some(overflow) = &mut line.text_overflow {
        for fragment in &mut overflow.fragments {
            fragment.rect.x += dx;
            fragment.rect.y += dy;
        }
    }
}
