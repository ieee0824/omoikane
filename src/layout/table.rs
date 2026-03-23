//! Table layout: `display: table`, rows, cells, and column width calculation.

use crate::css::ComputedStyle;
use crate::css::StyleResolver;
use crate::dom::{Node, NodeHandle, NodeType};

use super::{
    BoxDimensions, EdgeSizes, LayoutBox, Overflow, Rect,
    Visibility, VerticalAlign,
    explicit_length,
    intrinsic_width, layout_node, normalized_min_max_lengths, overflow, resolved_length,
    translate_layout_box_to_outer, translate_layout_contents, vertical_align, visibility, z_index,
};

pub(super) fn layout_table_container(
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
    let spacing = table_border_spacing(&style);
    let collapse_spacing = spacing * 2.0;
    let mut entries = collect_table_entries(node, resolver);
    let column_count = table_column_count(&entries);
    let total_spacing = spacing * (column_count as f32 + 1.0);
    let column_widths = compute_table_column_widths(
        &entries,
        resolver,
        column_count,
        (width - total_spacing).max(0.0),
    );
    let inner_width = (width - collapse_spacing).max(0.0);

    // === Pass 1: Layout all rows and collect rowspan info ===
    let mut row_boxes = Vec::new();
    let mut row_heights = Vec::new();
    let mut all_rowspan_cells: Vec<(usize, RowspanCellInfo)> = Vec::new(); // (start_row, info)
    let mut row_groups: Vec<Option<NodeHandle>> = Vec::new();
    let mut occupied_columns = vec![0usize; column_count];

    let mut pass1_cursor_y = y + spacing;
    for (row_index, entry) in entries.drain(..).enumerate() {
        for occupied in &mut occupied_columns {
            if *occupied > 0 {
                *occupied -= 1;
            }
        }

        let row_y = pass1_cursor_y;
        let (row_box, row_height, rowspan_cells) = layout_table_row_entry(
            &entry,
            resolver,
            x + spacing,
            row_y,
            column_count,
            &mut occupied_columns,
            &column_widths,
            spacing,
            viewport,
        )?;
        row_boxes.push(row_box);
        row_heights.push(row_height);
        row_groups.push(entry.row_group);
        pass1_cursor_y += row_height + spacing;

        for cell_info in rowspan_cells {
            all_rowspan_cells.push((row_index, cell_info));
        }
    }

    // === Pass 2: Distribute rowspan cell heights ===
    for (start_row, cell_info) in &all_rowspan_cells {
        let end_row = (*start_row + cell_info.rowspan).min(row_heights.len());
        let spanned_height: f32 = row_heights[*start_row..end_row].iter().sum();
        let spanned_spacing = (end_row - *start_row).saturating_sub(1) as f32 * spacing;
        let total_spanned = spanned_height + spanned_spacing;
        if cell_info.cell_height > total_spanned {
            let deficit = cell_info.cell_height - total_spanned;
            let per_row = deficit / (end_row - *start_row) as f32;
            for row_h in &mut row_heights[*start_row..end_row] {
                *row_h += per_row;
            }
        }
    }

    // === Pass 3: Adjust row positions and cell heights after redistribution ===
    let mut cursor_y = y + spacing;
    let mut children = Vec::new();
    let mut pending_group: Option<(NodeHandle, Vec<LayoutBox>, f32, f32)> = None;

    for (row_index, mut row_box) in row_boxes.into_iter().enumerate() {
        let final_height = row_heights[row_index];
        let dy = cursor_y - row_box.dimensions.content.y;
        if dy.abs() > 0.01 {
            let row_x = row_box.dimensions.content.x;
            translate_layout_box_to_outer(&mut row_box, row_x, cursor_y);
            // Re-translate children to new row y
            for child in &mut row_box.children {
                let cx = child.dimensions.content.x;
                translate_layout_box_to_outer(child, cx, cursor_y);
            }
        }
        // Stretch row and cells to final height
        let height_increase = final_height - row_box.dimensions.content.height;
        if height_increase > 0.01 {
            row_box.dimensions.content.height = final_height;
        }
        for child in &mut row_box.children {
            let rs = html_table_span_attribute(&child.node, "rowspan").unwrap_or(1);
            let cell_style = resolver.computed_style(&child.node);
            let valign = vertical_align(&cell_style);
            if rs <= 1 {
                // Non-rowspan cells stretch to match the row height
                if height_increase > 0.01 {
                    let content_used_height = used_content_height(child);
                    child.dimensions.content.height += height_increase;
                    // Re-apply vertical-align offset for middle/bottom
                    let extra = (child.dimensions.content.height - content_used_height).max(0.0);
                    let offset = match valign {
                        VerticalAlign::Bottom => extra,
                        VerticalAlign::Middle => extra / 2.0,
                        _ => 0.0,
                    };
                    if offset > 0.01 {
                        // Reset contents to top, then apply new offset
                        reset_content_to_top(child);
                        translate_layout_contents(child, 0.0, offset);
                    }
                }
            } else {
                // Rowspan cells stretch to span all their rows
                let end = (row_index + rs).min(row_heights.len());
                let spanned: f32 = row_heights[row_index..end].iter().sum();
                let spanned_spacing = (end - row_index).saturating_sub(1) as f32 * spacing;
                let old_height = child.dimensions.content.height;
                child.dimensions.content.height = spanned + spanned_spacing;
                let new_height = child.dimensions.content.height;
                if (new_height - old_height).abs() > 0.01 {
                    let content_used_height = used_content_height(child);
                    let extra = (new_height - content_used_height).max(0.0);
                    let offset = match valign {
                        VerticalAlign::Bottom => extra,
                        VerticalAlign::Middle => extra / 2.0,
                        _ => 0.0,
                    };
                    if offset > 0.01 {
                        reset_content_to_top(child);
                        translate_layout_contents(child, 0.0, offset);
                    }
                }
            }
        }
        cursor_y += final_height + spacing;

        if let Some(group_node) = row_groups[row_index].clone() {
            match &mut pending_group {
                Some((current_group, rows, _, _group_start_y)) if *current_group == group_node => {
                    rows.push(row_box);
                }
                Some((current_group, rows, _, group_start_y)) => {
                    let group_box = build_row_group_box(
                        current_group.clone(),
                        std::mem::take(rows),
                        x + spacing,
                        *group_start_y,
                        inner_width,
                    );
                    children.push(group_box);
                    *current_group = group_node.clone();
                    *group_start_y = cursor_y - final_height - spacing;
                    rows.push(row_box);
                }
                None => {
                    pending_group = Some((group_node, vec![row_box], inner_width, cursor_y - final_height - spacing));
                }
            }
        } else {
            if let Some((group_node, rows, _, group_start_y)) = pending_group.take() {
                let group_box =
                    build_row_group_box(group_node, rows, x + spacing, group_start_y, inner_width);
                children.push(group_box);
            }
            children.push(row_box);
        }
    }

    if let Some((group_node, rows, _, group_start_y)) = pending_group.take() {
        let group_box =
            build_row_group_box(group_node, rows, x + spacing, group_start_y, inner_width);
        children.push(group_box);
    }

    let auto_height = (cursor_y - y).max(spacing);
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
    Some(LayoutBox {
        node: node.clone(),
        dimensions: BoxDimensions {
            content: Rect {
                x,
                y,
                width,
                height: content_height,
            },
            padding,
            border,
            margin,
        },
        visibility: visibility(&style),
        overflow: overflow(&style),
        z_index: z_index(&style),
        lines: Vec::new(),
        children,
        marker: None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TableDisplay {
    Table,
    RowGroup,
    Row,
    Cell,
}

#[derive(Debug, Clone)]
pub(super) struct TableRowEntry {
    row_node: NodeHandle,
    row_group: Option<NodeHandle>,
    pub(super) cells: Vec<NodeHandle>,
}

pub(super) fn collect_table_entries(node: &NodeHandle, resolver: &mut StyleResolver) -> Vec<TableRowEntry> {
    let mut entries = Vec::new();
    let mut anonymous_cells = Vec::new();

    for child in node.child_nodes() {
        match table_display_for_node(&child, &resolver.computed_style(&child)) {
            Some(TableDisplay::RowGroup) => {
                flush_anonymous_row(&mut entries, &mut anonymous_cells);
                for row in child.child_nodes() {
                    match table_display_for_node(&row, &resolver.computed_style(&row)) {
                        Some(TableDisplay::Row) => entries.push(TableRowEntry {
                            row_node: row.clone(),
                            row_group: Some(child.clone()),
                            cells: collect_row_cells(&row, resolver),
                        }),
                        Some(TableDisplay::Cell) => anonymous_cells.push(row),
                        _ => {}
                    }
                }
            }
            Some(TableDisplay::Row) => {
                flush_anonymous_row(&mut entries, &mut anonymous_cells);
                entries.push(TableRowEntry {
                    row_node: child.clone(),
                    row_group: None,
                    cells: collect_row_cells(&child, resolver),
                });
            }
            Some(TableDisplay::Cell) => anonymous_cells.push(child),
            Some(TableDisplay::Table) => {
                // CSS 2.1 §17.2.1: wrap non-row/cell children in anonymous cell
                anonymous_cells.push(child);
            }
            _ => {
                if child.node_type() == NodeType::Element {
                    // Treat block-level children as anonymous cells
                    anonymous_cells.push(child);
                }
            }
        }
    }

    flush_anonymous_row(&mut entries, &mut anonymous_cells);
    entries
}

fn flush_anonymous_row(entries: &mut Vec<TableRowEntry>, anonymous_cells: &mut Vec<NodeHandle>) {
    if anonymous_cells.is_empty() {
        return;
    }
    entries.push(TableRowEntry {
        row_node: NodeHandle::element("tr"),
        row_group: None,
        cells: std::mem::take(anonymous_cells),
    });
}

fn collect_row_cells(row: &NodeHandle, resolver: &mut StyleResolver) -> Vec<NodeHandle> {
    row.child_nodes()
        .into_iter()
        .filter(|child| {
            matches!(
                table_display_for_node(child, &resolver.computed_style(child)),
                Some(TableDisplay::Cell)
            )
        })
        .collect()
}

pub(super) fn spanned_cell_width(column_widths: &[f32], start: usize, span: usize, spacing: f32) -> f32 {
    let end = (start + span).min(column_widths.len());
    let content: f32 = column_widths[start..end].iter().sum();
    let gaps = span.saturating_sub(1) as f32 * spacing;
    content + gaps
}

pub(super) fn column_x_offset(column_widths: &[f32], column: usize, spacing: f32) -> f32 {
    let mut offset = 0.0;
    for i in 0..column.min(column_widths.len()) {
        offset += column_widths[i] + spacing;
    }
    offset
}

fn layout_table_row_entry(
    entry: &TableRowEntry,
    resolver: &mut StyleResolver,
    x: f32,
    y: f32,
    column_count: usize,
    occupied_columns: &mut [usize],
    column_widths: &[f32],
    spacing: f32,
    viewport: Rect,
) -> Option<(LayoutBox, f32, Vec<RowspanCellInfo>)> {
    let mut measured = Vec::new();
    let mut row_height = 0.0f32;
    let mut column_cursor = 0usize;

    for cell in &entry.cells {
        while column_cursor < column_count && occupied_columns[column_cursor] > 0 {
            column_cursor += 1;
        }
        if column_cursor >= column_count {
            break;
        }

        let max_span = column_count.saturating_sub(column_cursor).max(1);
        let span = html_table_span_attribute(cell, "colspan")
            .unwrap_or(1)
            .max(1)
            .min(max_span);
        let rowspan = html_table_span_attribute(cell, "rowspan")
            .unwrap_or(1)
            .max(1);
        let cell_width = spanned_cell_width(column_widths, column_cursor, span, spacing);
        let cell_containing = Rect {
            x: 0.0,
            y: 0.0,
            width: cell_width,
            height: 0.0,
        };
        let mut layout_cell = layout_node(cell, resolver, cell_containing, viewport, None)?;
        let cell_style = resolver.computed_style(cell);
        let cell_height =
            explicit_length(&cell_style, "height").unwrap_or(layout_cell.total_height());
        layout_cell.dimensions.content.width = cell_width;
        layout_cell.dimensions.content.height = cell_height;
        // Only non-rowspan cells contribute to the row's initial height.
        // Rowspan cells will be distributed in a second pass.
        if rowspan <= 1 {
            row_height = row_height.max(layout_cell.total_height());
        }
        measured.push((column_cursor, span, rowspan, layout_cell, cell_style));
        if rowspan > 1 {
            for column in column_cursor..column_cursor.saturating_add(span) {
                if let Some(occupied) = occupied_columns.get_mut(column) {
                    *occupied = (*occupied).max(rowspan);
                }
            }
        }
        column_cursor = column_cursor.saturating_add(span);
    }

    let mut children = Vec::new();
    for (column_start, _span, _rowspan, mut cell, cell_style) in measured {
        let outer_x = x + column_x_offset(column_widths, column_start, spacing);
        let original_total_height = cell.total_height();
        let extra_height = (row_height - original_total_height).max(0.0);
        if extra_height > 0.0 {
            cell.dimensions.content.height += extra_height;
            let content_offset = match vertical_align(&cell_style) {
                VerticalAlign::Bottom => extra_height,
                VerticalAlign::Middle => extra_height / 2.0,
                _ => 0.0,
            };
            if content_offset > 0.0 {
                translate_layout_contents(&mut cell, 0.0, content_offset);
            }
        }
        let outer_y = y;
        translate_layout_box_to_outer(&mut cell, outer_x, outer_y);
        children.push(cell);
    }

    let row_width: f32 = column_widths.iter().sum::<f32>()
        + column_count.saturating_sub(1) as f32 * spacing;
    let row_box = LayoutBox {
        node: entry.row_node.clone(),
        dimensions: BoxDimensions {
            content: Rect {
                x,
                y,
                width: row_width,
                height: row_height,
            },
            ..BoxDimensions::default()
        },
        visibility: Visibility::Visible,
        overflow: Overflow::Visible,
        z_index: 0,
        lines: Vec::new(),
        children,
        marker: None,
    };

    // Collect rowspan cell heights for second-pass distribution
    let mut rowspan_cells = Vec::new();
    for child in &row_box.children {
        let rs = html_table_span_attribute(&child.node, "rowspan").unwrap_or(1);
        if rs > 1 {
            rowspan_cells.push(RowspanCellInfo {
                rowspan: rs,
                cell_height: child.total_height(),
            });
        }
    }

    Some((row_box, row_height, rowspan_cells))
}

struct RowspanCellInfo {
    rowspan: usize,
    cell_height: f32,
}

/// Compute the actual content height used by children/lines inside a cell.
fn used_content_height(layout: &LayoutBox) -> f32 {
    let top = layout.dimensions.content.y;
    let mut bottom = top;
    for child in &layout.children {
        let child_bottom = child.dimensions.content.y
            + child.dimensions.content.height
            + child.dimensions.padding.bottom
            + child.dimensions.border.bottom
            + child.dimensions.margin.bottom;
        bottom = bottom.max(child_bottom);
    }
    for line in &layout.lines {
        bottom = bottom.max(line.rect.y + line.rect.height);
    }
    (bottom - top).max(0.0)
}

/// Reset cell contents to the top of the content box (undo previous vertical offsets).
fn reset_content_to_top(layout: &mut LayoutBox) {
    let cell_top = layout.dimensions.content.y;
    // Find the current topmost content position
    let mut current_top = f32::INFINITY;
    for child in &layout.children {
        current_top = current_top.min(
            child.dimensions.content.y
                - child.dimensions.margin.top
                - child.dimensions.border.top
                - child.dimensions.padding.top,
        );
    }
    for line in &layout.lines {
        current_top = current_top.min(line.rect.y);
    }
    if current_top.is_finite() && (current_top - cell_top).abs() > 0.01 {
        let dy = cell_top - current_top;
        translate_layout_contents(layout, 0.0, dy);
    }
}

pub(super) fn table_column_count(entries: &[TableRowEntry]) -> usize {
    let mut occupied_columns = Vec::<usize>::new();
    let mut max_columns = 0usize;

    for entry in entries {
        for occupied in &mut occupied_columns {
            if *occupied > 0 {
                *occupied -= 1;
            }
        }

        let mut column_cursor = 0usize;
        for cell in &entry.cells {
            while column_cursor < occupied_columns.len() && occupied_columns[column_cursor] > 0 {
                column_cursor += 1;
            }

            let span = html_table_span_attribute(cell, "colspan")
                .unwrap_or(1)
                .max(1);
            let rowspan = html_table_span_attribute(cell, "rowspan")
                .unwrap_or(1)
                .max(1);
            let end = column_cursor.saturating_add(span);
            if end > occupied_columns.len() {
                occupied_columns.resize(end, 0);
            }
            if rowspan > 1 {
                for occupied in &mut occupied_columns[column_cursor..end] {
                    *occupied = (*occupied).max(rowspan);
                }
            }
            column_cursor = end;
        }

        max_columns = max_columns.max(column_cursor.max(occupied_columns.len()));
    }

    max_columns.max(1)
}

pub(super) fn compute_table_column_widths(
    entries: &[TableRowEntry],
    resolver: &mut StyleResolver,
    column_count: usize,
    available_width: f32,
) -> Vec<f32> {
    let mut column_hints = vec![0.0f32; column_count];

    // Single pass: scan all rows with rowspan tracking to collect hints and explicit flags
    let mut explicit_flags = vec![false; column_count];
    let mut occupied_columns = vec![0usize; column_count];
    for entry in entries {
        for occupied in &mut occupied_columns {
            if *occupied > 0 {
                *occupied -= 1;
            }
        }
        let mut col = 0usize;
        for cell in &entry.cells {
            while col < column_count && occupied_columns[col] > 0 {
                col += 1;
            }
            if col >= column_count {
                break;
            }
            let span = html_table_span_attribute(cell, "colspan")
                .unwrap_or(1)
                .max(1);
            let rowspan = html_table_span_attribute(cell, "rowspan")
                .unwrap_or(1)
                .max(1);
            let end = (col + span).min(column_count);
            if span == 1 {
                let cell_style = resolver.computed_style(cell);
                if let Some(w) = explicit_length(&cell_style, "width") {
                    column_hints[col] = column_hints[col].max(w);
                    explicit_flags[col] = true;
                } else {
                    let w = intrinsic_width(cell, resolver);
                    column_hints[col] = column_hints[col].max(w);
                    // Treat cells containing images as having a fixed minimum
                    // width so they are not compressed by text-heavy siblings.
                    if cell_contains_image(cell) {
                        explicit_flags[col] = true;
                    }
                }
            }
            if rowspan > 1 {
                for occupied in &mut occupied_columns[col..end] {
                    *occupied = (*occupied).max(rowspan);
                }
            }
            col = end;
        }
    }

    let fixed_total: f32 = column_hints
        .iter()
        .zip(explicit_flags.iter())
        .filter(|&(_, &is_explicit)| is_explicit)
        .map(|(&w, _)| w)
        .sum();
    let auto_hints: Vec<(usize, f32)> = column_hints
        .iter()
        .enumerate()
        .filter(|(i, _)| !explicit_flags[*i])
        .map(|(i, &w)| (i, w))
        .collect();
    let auto_hint_total: f32 = auto_hints.iter().map(|(_, w)| w).sum();
    let auto_count = auto_hints.len();

    let remaining = (available_width - fixed_total).max(0.0);

    // Distribute remaining width among auto columns:
    // Each auto column gets at least its intrinsic hint, then leftover is split equally
    let mut widths = column_hints.clone();
    if auto_count > 0 {
        if auto_hint_total <= remaining {
            let leftover = remaining - auto_hint_total;
            let equal_extra = leftover / auto_count as f32;
            for &(i, hint) in &auto_hints {
                widths[i] = hint + equal_extra;
            }
        } else if auto_hint_total > 0.0 {
            for &(i, hint) in &auto_hints {
                widths[i] = remaining * (hint / auto_hint_total);
            }
        } else {
            let equal = remaining / auto_count as f32;
            for &(i, _) in &auto_hints {
                widths[i] = equal;
            }
        }
    }

    // If auto columns pushed total over available, scale only auto columns down
    let total: f32 = widths.iter().sum();
    if total > available_width && total > 0.0 {
        let auto_total: f32 = auto_hints.iter().map(|&(i, _)| widths[i]).sum();
        let target_auto = (available_width - fixed_total).max(0.0);
        if auto_total > 0.0 {
            let scale = target_auto / auto_total;
            for &(i, _) in &auto_hints {
                widths[i] *= scale;
            }
        }
    }

    widths
}

pub(super) fn html_table_span_attribute(node: &NodeHandle, name: &str) -> Option<usize> {
    node.attributes()
        .and_then(|attrs| attrs.get(name).cloned())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn build_row_group_box(
    node: NodeHandle,
    rows: Vec<LayoutBox>,
    x: f32,
    y: f32,
    width: f32,
) -> LayoutBox {
    let height = rows
        .last()
        .map(|row| row.dimensions.content.y + row.dimensions.content.height - y)
        .unwrap_or(0.0);

    LayoutBox {
        node,
        dimensions: BoxDimensions {
            content: Rect {
                x,
                y,
                width,
                height,
            },
            ..BoxDimensions::default()
        },
        visibility: Visibility::Visible,
        overflow: Overflow::Visible,
        z_index: 0,
        lines: Vec::new(),
        children: rows,
        marker: None,
    }
}

pub(super) fn is_table_container(style: &ComputedStyle) -> bool {
    matches!(table_display(style), Some(TableDisplay::Table))
}

pub(super) fn is_table_container_element(node: &NodeHandle, style: &ComputedStyle) -> bool {
    if is_table_container(style) {
        return true;
    }
    // HTML default: <table> is display: table
    if matches!(style.get("display"), None) {
        if let Some(tag) = node.tag_name() {
            return tag.eq_ignore_ascii_case("table");
        }
    }
    false
}

pub(super) fn table_display(style: &ComputedStyle) -> Option<TableDisplay> {
    use crate::css::ComputedValue;
    match style.get("display") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("table") => {
            Some(TableDisplay::Table)
        }
        Some(ComputedValue::Keyword(keyword))
            if keyword.eq_ignore_ascii_case("table-row-group") =>
        {
            Some(TableDisplay::RowGroup)
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("table-row") => {
            Some(TableDisplay::Row)
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("table-cell") => {
            Some(TableDisplay::Cell)
        }
        _ => None,
    }
}

pub(super) fn table_display_for_node(node: &NodeHandle, style: &ComputedStyle) -> Option<TableDisplay> {
    if let Some(display) = table_display(style) {
        return Some(display);
    }
    // HTML default display values for table elements
    if matches!(style.get("display"), None) {
        if let Some(tag) = node.tag_name() {
            return match tag.to_ascii_lowercase().as_str() {
                "table" => Some(TableDisplay::Table),
                "thead" | "tbody" | "tfoot" => Some(TableDisplay::RowGroup),
                "tr" => Some(TableDisplay::Row),
                "td" | "th" => Some(TableDisplay::Cell),
                _ => None,
            };
        }
    }
    None
}

pub(super) fn table_border_spacing(style: &ComputedStyle) -> f32 {
    use crate::css::ComputedValue;
    if matches!(
        style.get("border-collapse"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("collapse")
    ) {
        return 0.0;
    }

    explicit_length(style, "border-spacing").unwrap_or(0.0)
}

/// Returns `true` if the node (or any direct child) contains an `<img>` element.
fn cell_contains_image(node: &NodeHandle) -> bool {
    if node.tag_name().as_deref() == Some("img") {
        return true;
    }
    for child in node.child_nodes() {
        if child.tag_name().as_deref() == Some("img") {
            return true;
        }
        if child.node_type() == NodeType::Element && cell_contains_image(&child) {
            return true;
        }
    }
    false
}
