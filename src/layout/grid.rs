//! Basic explicit CSS Grid track sizing and row-major auto-placement.

use crate::css::{ComputedStyle, ComputedValue, StyleResolver};
use crate::dom::{NodeHandle, NodeType};

use super::{
    BoxDimensions, EdgeSizes, LayoutBox, Rect, intrinsic_width, is_display_none,
    is_out_of_flow_positioned, layout_node, layout_positioned_child,
    normalized_min_max_lengths, overflow, resolved_length, sort_children_by_z_index,
    translate_layout_box_to_outer, visibility, z_index,
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Track {
    Px(f32),
    Percent(f32),
    Fr(f32),
    Auto,
}

#[derive(Clone, Copy, Debug)]
struct Placement {
    column: usize,
    row: usize,
    column_span: usize,
    row_span: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct AxisRequest {
    start: Option<usize>,
    span: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Alignment {
    Start,
    End,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
    Stretch,
}

pub(super) fn is_grid_container(style: &ComputedStyle) -> bool {
    matches!(style.get("display"), Some(ComputedValue::Keyword(value))
        if value.eq_ignore_ascii_case("grid") || value.eq_ignore_ascii_case("inline-grid"))
}

pub(super) fn layout_grid_container(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: ComputedStyle,
    margin: EdgeSizes,
    padding: EdgeSizes,
    border: EdgeSizes,
    x: f32,
    y: f32,
    width: f32,
    containing_block_height: f32,
    viewport: Rect,
) -> Option<LayoutBox> {
    let mut items = Vec::new();
    let mut positioned = Vec::new();
    for child in crate::dom::Node::child_nodes(node) {
        if crate::dom::Node::node_type(&child) != NodeType::Element { continue; }
        let child_style = resolver.computed_style(&child);
        if is_display_none(&child_style) { continue; }
        if is_out_of_flow_positioned(&child_style) {
            positioned.push((child, child_style));
        } else {
            items.push(child);
        }
    }

    let mut columns = track_list(&style, "grid-template-columns")
        .filter(|tracks| !tracks.is_empty())
        .unwrap_or_else(|| vec![Track::Fr(1.0)]);
    let explicit_column_count = columns.len();
    let mut explicit_rows = track_list(&style, "grid-template-rows").unwrap_or_default();
    let explicit_row_count = explicit_rows.len();
    let column_gap = gap(&style, "column-gap");
    let row_gap = gap(&style, "row-gap");
    let requests: Vec<_> = items.iter().map(|child| {
        let child_style = resolver.computed_style(child);
        (
            axis_request(&child_style, "grid-column", explicit_column_count),
            axis_request(&child_style, "grid-row", explicit_row_count),
        )
    }).collect();
    let placements = place_items(&requests, &mut columns, &mut explicit_rows);
    let column_intrinsics = auto_column_intrinsics(&columns, &items, &placements, resolver);
    let column_widths = resolve_tracks(&columns, width, column_gap, &column_intrinsics);
    let row_count = placements.iter().map(|p| p.row + p.row_span).max().unwrap_or(explicit_rows.len()).max(explicit_rows.len());
    let specified_height = resolved_length(&style, "height", containing_block_height)
        .map(|height| super::border_box_adjust_height(&style, height, &padding, &border));
    let row_basis = specified_height.unwrap_or(0.0);
    let fixed_row_heights: Vec<_> = explicit_rows.iter()
        .map(|track| fixed_track(*track, row_basis).unwrap_or(0.0))
        .collect();

    let mut laid_out = Vec::new();
    let mut content_row_heights = vec![0.0f32; row_count];
    for (index, child) in items.iter().enumerate() {
        let placement = placements[index];
        let height = track_area(&fixed_row_heights, placement.row, placement.row_span, row_gap);
        let cell_width = track_area(&column_widths, placement.column, placement.column_span, column_gap);
        let containing = Rect { x: 0.0, y: 0.0, width: cell_width, height };
        if let Some(layout) = layout_node(child, resolver, containing, viewport, None) {
            let occupied = content_row_heights[placement.row..placement.row + placement.row_span].iter().sum::<f32>()
                + row_gap * placement.row_span.saturating_sub(1) as f32;
            let deficit = (layout.total_height() - occupied).max(0.0);
            content_row_heights[placement.row + placement.row_span - 1] += deficit;
            laid_out.push((index, layout));
        }
    }

    let mut row_tracks = explicit_rows;
    row_tracks.resize(row_count, Track::Auto);
    let row_heights = resolve_tracks(&row_tracks, row_basis, row_gap, &content_row_heights);
    let auto_height = row_heights.iter().sum::<f32>()
        + row_gap * row_heights.len().saturating_sub(1) as f32;
    let mut content_height = specified_height.unwrap_or(auto_height);
    let (min_height, max_height) = normalized_min_max_lengths(&style, "min-height", "max-height", 0.0);
    if let Some(value) = min_height { content_height = content_height.max(super::border_box_adjust_height(&style, value, &padding, &border)); }
    if let Some(value) = max_height { content_height = content_height.min(super::border_box_adjust_height(&style, value, &padding, &border)); }

    let (column_widths, column_start, aligned_column_gap) = align_tracks(
        column_widths,
        column_gap,
        width,
        alignment(&style, "justify-content", Alignment::Start),
    );
    let (row_heights, row_start, aligned_row_gap) = align_tracks(
        row_heights,
        row_gap,
        content_height,
        alignment(&style, "align-content", Alignment::Start),
    );
    let column_offsets = offsets(&column_widths, aligned_column_gap, x + column_start);
    let row_offsets = offsets(&row_heights, aligned_row_gap, y + row_start);
    let mut children = Vec::new();
    for (index, mut child) in laid_out {
        let placement = placements[index];
        let child_style = resolver.computed_style(&items[index]);
        let cell_width = track_area(&column_widths, placement.column, placement.column_span, aligned_column_gap);
        let cell_height = track_area(&row_heights, placement.row, placement.row_span, aligned_row_gap);
        let justify = self_alignment(&child_style, "justify-self")
            .unwrap_or_else(|| alignment(&style, "justify-items", Alignment::Stretch));
        let align = self_alignment(&child_style, "align-self")
            .unwrap_or_else(|| alignment(&style, "align-items", Alignment::Stretch));
        let dx = item_offset(justify, cell_width, child.total_width());
        let dy = item_offset(align, cell_height, child.total_height());
        translate_layout_box_to_outer(
            &mut child,
            column_offsets[placement.column] + dx,
            row_offsets[placement.row] + dy,
        );
        children.push(child);
    }
    let dimensions = BoxDimensions { content: Rect { x, y, width, height: content_height }, padding, border, margin };
    for (child, child_style) in positioned {
        if let Some(child) = layout_positioned_child(&child, resolver, &child_style, dimensions, dimensions.content, viewport) {
            children.push(child);
        }
    }
    sort_children_by_z_index(&mut children);
    Some(LayoutBox { node: node.clone(), dimensions, visibility: visibility(&style), overflow: overflow(&style), z_index: z_index(&style), lines: Vec::new(), children, marker: None })
}

fn alignment(style: &ComputedStyle, property: &str, default: Alignment) -> Alignment {
    match style.get(property) {
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("start") || value.eq_ignore_ascii_case("flex-start") => Alignment::Start,
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("end") || value.eq_ignore_ascii_case("flex-end") => Alignment::End,
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("center") => Alignment::Center,
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("space-between") => Alignment::SpaceBetween,
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("space-around") => Alignment::SpaceAround,
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("space-evenly") => Alignment::SpaceEvenly,
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("stretch") => Alignment::Stretch,
        _ => default,
    }
}

fn self_alignment(style: &ComputedStyle, property: &str) -> Option<Alignment> {
    match style.get(property) {
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("auto") => None,
        Some(_) => Some(alignment(style, property, Alignment::Stretch)),
        None => None,
    }
}

fn item_offset(alignment: Alignment, available: f32, occupied: f32) -> f32 {
    let free = (available - occupied).max(0.0);
    match alignment {
        Alignment::End => free,
        Alignment::Center => free / 2.0,
        _ => 0.0,
    }
}

fn align_tracks(
    mut sizes: Vec<f32>,
    gap: f32,
    available: f32,
    alignment: Alignment,
) -> (Vec<f32>, f32, f32) {
    let count = sizes.len();
    let used = sizes.iter().sum::<f32>() + gap * count.saturating_sub(1) as f32;
    let free = (available - used).max(0.0);
    if free == 0.0 || count == 0 { return (sizes, 0.0, gap); }
    match alignment {
        Alignment::End => (sizes, free, gap),
        Alignment::Center => (sizes, free / 2.0, gap),
        Alignment::SpaceBetween if count > 1 => (sizes, 0.0, gap + free / (count - 1) as f32),
        Alignment::SpaceAround => {
            let share = free / count as f32;
            (sizes, share / 2.0, gap + share)
        }
        Alignment::SpaceEvenly => {
            let share = free / (count + 1) as f32;
            (sizes, share, gap + share)
        }
        Alignment::Stretch => {
            let share = free / count as f32;
            for size in &mut sizes { *size += share; }
            (sizes, 0.0, gap)
        }
        _ => (sizes, 0.0, gap),
    }
}

fn gap(style: &ComputedStyle, property: &str) -> f32 {
    match style.get(property) { Some(ComputedValue::Px(value)) => value.max(0.0), _ => 0.0 }
}

fn auto_column_intrinsics(tracks: &[Track], items: &[NodeHandle], placements: &[Placement], resolver: &mut StyleResolver) -> Vec<f32> {
    let mut values = vec![0.0f32; tracks.len()];
    for (index, child) in items.iter().enumerate() {
        let column = placements[index].column;
        if placements[index].column_span == 1 && tracks[column] == Track::Auto {
            values[column] = values[column].max(intrinsic_width(child, resolver));
        }
    }
    values
}

fn place_items(requests: &[(AxisRequest, AxisRequest)], columns: &mut Vec<Track>, rows: &mut Vec<Track>) -> Vec<Placement> {
    let mut result = vec![Placement { column: 0, row: 0, column_span: 1, row_span: 1 }; requests.len()];
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    for explicit_phase in [true, false] {
        for (index, &(column, row)) in requests.iter().enumerate() {
            if (column.start.is_some() || row.start.is_some()) != explicit_phase { continue; }
            let column_span = column.span.max(1);
            let row_span = row.span.max(1);
            let mut candidate_row = row.start.unwrap_or(0);
            let mut candidate_column = column.start.unwrap_or(0);
            loop {
                if column.start.is_none()
                    && row.start.is_none()
                    && candidate_column + column_span > columns.len()
                {
                    if column_span > columns.len() {
                        columns.resize(column_span, Track::Auto);
                    } else {
                        candidate_column = 0;
                        candidate_row += 1;
                    }
                }
                let needed_columns = candidate_column + column_span;
                if needed_columns > columns.len() { columns.resize(needed_columns, Track::Auto); }
                ensure_occupancy(&mut occupied, candidate_row + row_span, columns.len());
                if area_is_free(&occupied, candidate_column, candidate_row, column_span, row_span) { break; }
                if column.start.is_some() {
                    candidate_row += 1;
                } else if row.start.is_some() {
                    candidate_column += 1;
                } else {
                    candidate_column += 1;
                    if candidate_column + column_span > columns.len() {
                        candidate_column = 0;
                        candidate_row += 1;
                    }
                }
            }
            ensure_occupancy(&mut occupied, candidate_row + row_span, columns.len());
            for cells in &mut occupied[candidate_row..candidate_row + row_span] {
                cells[candidate_column..candidate_column + column_span].fill(true);
            }
            rows.resize(rows.len().max(candidate_row + row_span), Track::Auto);
            result[index] = Placement { column: candidate_column, row: candidate_row, column_span, row_span };
        }
    }
    result
}

fn ensure_occupancy(occupied: &mut Vec<Vec<bool>>, rows: usize, columns: usize) {
    for row in occupied.iter_mut() { row.resize(columns, false); }
    if occupied.len() < rows {
        occupied.resize_with(rows, || vec![false; columns]);
    }
}

fn area_is_free(occupied: &[Vec<bool>], column: usize, row: usize, column_span: usize, row_span: usize) -> bool {
    occupied[row..row + row_span].iter().all(|cells| cells[column..column + column_span].iter().all(|cell| !cell))
}

fn axis_request(style: &ComputedStyle, axis: &str, explicit_tracks: usize) -> AxisRequest {
    let start = grid_line(style.get(&format!("{axis}-start")));
    let end = grid_line(style.get(&format!("{axis}-end")));
    match (start, end) {
        (GridLine::Line(start), GridLine::Line(end)) => {
            let start = resolve_line(start, explicit_tracks);
            let end = resolve_line(end, explicit_tracks);
            AxisRequest { start: Some(start.min(end)), span: start.abs_diff(end).max(1) }
        }
        (GridLine::Line(start), GridLine::Span(span)) => AxisRequest { start: Some(resolve_line(start, explicit_tracks)), span },
        (GridLine::Line(start), _) => AxisRequest { start: Some(resolve_line(start, explicit_tracks)), span: 1 },
        (GridLine::Span(span), _) | (_, GridLine::Span(span)) => AxisRequest { start: None, span },
        (_, GridLine::Line(end)) => AxisRequest { start: Some(resolve_line(end, explicit_tracks).saturating_sub(1)), span: 1 },
        _ => AxisRequest { start: None, span: 1 },
    }
}

#[derive(Clone, Copy, Debug)]
enum GridLine { Auto, Line(isize), Span(usize) }

fn grid_line(value: Option<&ComputedValue>) -> GridLine {
    match value {
        Some(ComputedValue::Number(number)) => GridLine::Line(*number as isize),
        Some(ComputedValue::Keyword(value)) => {
            let parts: Vec<_> = value.split_whitespace().collect();
            if parts.len() == 2 && parts[0].eq_ignore_ascii_case("span") {
                return parts[1].parse::<usize>().ok().filter(|span| *span > 0).map(GridLine::Span).unwrap_or(GridLine::Auto);
            }
            value.parse::<isize>().ok().filter(|line| *line != 0).map(GridLine::Line).unwrap_or(GridLine::Auto)
        }
        _ => GridLine::Auto,
    }
}

fn resolve_line(line: isize, explicit_tracks: usize) -> usize {
    if line > 0 { (line as usize).saturating_sub(1) } else { (explicit_tracks as isize + line + 1).max(0) as usize }
}

fn track_area(sizes: &[f32], start: usize, span: usize, gap: f32) -> f32 {
    sizes[start..start + span].iter().sum::<f32>() + gap * span.saturating_sub(1) as f32
}

fn resolve_tracks(tracks: &[Track], basis: f32, gap: f32, auto_sizes: &[f32]) -> Vec<f32> {
    let gaps = gap * tracks.len().saturating_sub(1) as f32;
    let mut sizes = vec![0.0; tracks.len()];
    let mut fixed = gaps;
    let mut fr_total = 0.0;
    for (index, track) in tracks.iter().enumerate() {
        match *track {
            Track::Px(value) => sizes[index] = value.max(0.0),
            Track::Percent(value) => sizes[index] = (basis * value / 100.0).max(0.0),
            Track::Auto => sizes[index] = auto_sizes.get(index).copied().unwrap_or(0.0),
            Track::Fr(value) => fr_total += value.max(0.0),
        }
        fixed += sizes[index];
    }
    let remaining = (basis - fixed).max(0.0);
    if fr_total > 0.0 {
        for (index, track) in tracks.iter().enumerate() {
            if let Track::Fr(value) = track { sizes[index] = remaining * value.max(0.0) / fr_total; }
        }
    }
    sizes
}

fn fixed_track(track: Track, basis: f32) -> Option<f32> {
    match track { Track::Px(v) => Some(v), Track::Percent(v) => Some(basis * v / 100.0), _ => None }
}

fn offsets(sizes: &[f32], gap: f32, start: f32) -> Vec<f32> {
    let mut cursor = start;
    sizes.iter().map(|size| { let current = cursor; cursor += size + gap; current }).collect()
}

fn track_list(style: &ComputedStyle, property: &str) -> Option<Vec<Track>> {
    let value = match style.get(property)? {
        ComputedValue::Keyword(value) => value,
        ComputedValue::Px(value) => return Some(vec![Track::Px(*value)]),
        ComputedValue::Percentage(value) => return Some(vec![Track::Percent(*value)]),
        _ => return None,
    };
    let mut result = Vec::new();
    for token in split_tracks(value) {
        if token.to_ascii_lowercase().starts_with("repeat(") && token.ends_with(')') {
            let inner = &token[7..token.len() - 1];
            let (count, track) = inner.split_once(',')?;
            let count: usize = count.trim().parse().ok()?;
            let track = parse_track(track.trim())?;
            result.extend(std::iter::repeat_n(track, count));
        } else {
            result.push(parse_track(token.trim())?);
        }
    }
    Some(result)
}

fn split_tracks(value: &str) -> Vec<String> {
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut result = Vec::new();
    for (index, ch) in value.char_indices() {
        match ch { '(' => depth += 1, ')' => depth = depth.saturating_sub(1), _ => {} }
        if ch.is_whitespace() && depth == 0 {
            if start < index { result.push(value[start..index].to_string()); }
            start = index + ch.len_utf8();
        }
    }
    if start < value.len() { result.push(value[start..].to_string()); }
    result
}

fn parse_track(value: &str) -> Option<Track> {
    if value.eq_ignore_ascii_case("auto") { return Some(Track::Auto); }
    let lower = value.to_ascii_lowercase();
    if let Some(value) = lower.strip_suffix("px") { return value.trim().parse().ok().map(Track::Px); }
    if let Some(value) = lower.strip_suffix('%') { return value.trim().parse().ok().map(Track::Percent); }
    if let Some(value) = lower.strip_suffix("fr") { return value.trim().parse().ok().map(Track::Fr); }
    None
}
