//! Basic explicit CSS Grid track sizing and row-major auto-placement.

use std::collections::HashMap;

use crate::css::{ComputedStyle, ComputedValue, StyleResolver};
use crate::dom::{NodeHandle, NodeType};

use super::{
    BoxDimensions, EdgeSizes, LayoutBox, Rect, edge_sizes, intrinsic_width, is_display_none,
    is_out_of_flow_positioned, layout_node, layout_positioned_child,
    normalized_min_max_lengths, overflow, resolved_length, sort_children_by_z_index,
    translate_layout_box_to_outer, visibility, z_index,
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum TrackSize {
    Px(f32),
    Percent(f32),
    Fr(f32),
    Calc(f32, f32),
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Track {
    min: TrackSize,
    max: TrackSize,
    auto_fit: bool,
}

impl Track {
    fn new(size: TrackSize) -> Self {
        let min = if matches!(size, TrackSize::Fr(_)) {
            TrackSize::Px(0.0)
        } else {
            size
        };
        Self { min, max: size, auto_fit: false }
    }

    fn auto() -> Self { Self::new(TrackSize::Auto) }
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

#[derive(Clone, Copy, Debug)]
struct NamedArea {
    row_start: usize,
    row_span: usize,
    column_start: usize,
    column_span: usize,
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

    let column_gap = gap(&style, "column-gap");
    let row_gap = gap(&style, "row-gap");
    let specified_height = resolved_length(&style, "height", containing_block_height)
        .map(|height| super::border_box_adjust_height(&style, height, &padding, &border));
    let row_basis = specified_height.unwrap_or(0.0);
    let (named_areas, area_row_count, area_column_count) = named_areas(&style);
    let mut columns = track_list(&style, "grid-template-columns", width, column_gap)
        .filter(|tracks| !tracks.is_empty())
        .unwrap_or_else(|| {
            if area_column_count > 0 {
                vec![Track::auto(); area_column_count]
            } else {
                vec![Track::new(TrackSize::Fr(1.0))]
            }
        });
    columns.resize(columns.len().max(area_column_count), Track::auto());
    let explicit_column_count = columns.len();
    let mut explicit_rows = track_list(&style, "grid-template-rows", row_basis, row_gap)
        .unwrap_or_default();
    explicit_rows.resize(explicit_rows.len().max(area_row_count), Track::auto());
    let explicit_row_count = explicit_rows.len();
    let requests: Vec<_> = items.iter().map(|child| {
        let child_style = resolver.computed_style(child);
        (
            axis_request(&child_style, "grid-column", explicit_column_count, &named_areas, true),
            axis_request(&child_style, "grid-row", explicit_row_count, &named_areas, false),
        )
    }).collect();
    let mut placements = place_items(&requests, &mut columns, &mut explicit_rows);
    collapse_empty_auto_fit_tracks(&mut columns, &mut placements, true);
    collapse_empty_auto_fit_tracks(&mut explicit_rows, &mut placements, false);
    let column_intrinsics = auto_column_intrinsics(&columns, &items, &placements, resolver);
    let column_widths = resolve_tracks(&columns, width, column_gap, &column_intrinsics);
    let row_count = placements.iter().map(|p| p.row + p.row_span).max().unwrap_or(explicit_rows.len()).max(explicit_rows.len());
    let fixed_row_heights: Vec<_> = explicit_rows.iter()
        .map(|track| fixed_track(*track, row_basis).unwrap_or(0.0))
        .collect();

    let mut laid_out = Vec::new();
    let mut content_row_heights = vec![0.0f32; row_count];
    for (index, child) in items.iter().enumerate() {
        let placement = placements[index];
        let child_style = resolver.computed_style(child);
        let height = track_area(&fixed_row_heights, placement.row, placement.row_span, row_gap);
        let cell_width = track_area(&column_widths, placement.column, placement.column_span, column_gap);
        let justify = self_alignment(&child_style, "justify-self")
            .unwrap_or_else(|| alignment(&style, "justify-items", Alignment::Stretch));
        let item_width = if justify != Alignment::Stretch
            && resolved_length(&child_style, "width", cell_width).is_none()
        {
            let margin = edge_sizes(&child_style, "margin");
            (intrinsic_width(child, resolver) + margin.horizontal()).min(cell_width)
        } else {
            cell_width
        };
        let containing = Rect { x: 0.0, y: 0.0, width: item_width, height };
        if let Some(layout) = layout_node(child, resolver, containing, viewport, None) {
            let occupied = content_row_heights[placement.row..placement.row + placement.row_span].iter().sum::<f32>()
                + row_gap * placement.row_span.saturating_sub(1) as f32;
            let deficit = (layout.total_height() - occupied).max(0.0);
            content_row_heights[placement.row + placement.row_span - 1] += deficit;
            laid_out.push((index, layout));
        }
    }

    let mut row_tracks = explicit_rows;
    row_tracks.resize(row_count, Track::auto());
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
        if placements[index].column_span == 1
            && (tracks[column].min == TrackSize::Auto || tracks[column].max == TrackSize::Auto)
        {
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
                        columns.resize(column_span, Track::auto());
                    } else {
                        candidate_column = 0;
                        candidate_row += 1;
                    }
                }
                let needed_columns = candidate_column + column_span;
                if needed_columns > columns.len() { columns.resize(needed_columns, Track::auto()); }
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
            rows.resize(rows.len().max(candidate_row + row_span), Track::auto());
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

fn axis_request(
    style: &ComputedStyle,
    axis: &str,
    explicit_tracks: usize,
    named_areas: &HashMap<String, NamedArea>,
    column_axis: bool,
) -> AxisRequest {
    let start = grid_line(
        style.get(&format!("{axis}-start")),
        named_areas,
        column_axis,
        true,
    );
    let end = grid_line(
        style.get(&format!("{axis}-end")),
        named_areas,
        column_axis,
        false,
    );
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

fn grid_line(
    value: Option<&ComputedValue>,
    named_areas: &HashMap<String, NamedArea>,
    column_axis: bool,
    start_side: bool,
) -> GridLine {
    match value {
        Some(ComputedValue::Number(number)) => GridLine::Line(*number as isize),
        Some(ComputedValue::Keyword(value)) => {
            let parts: Vec<_> = value.split_whitespace().collect();
            if parts.len() == 1
                && (parts[0].eq_ignore_ascii_case("auto")
                    || parts[0].eq_ignore_ascii_case("span"))
            {
                return GridLine::Auto;
            }
            if parts.len() == 2 && parts[0].eq_ignore_ascii_case("span") {
                return parts[1].parse::<usize>().ok().filter(|span| *span > 0).map(GridLine::Span).unwrap_or(GridLine::Auto);
            }
            if let Some(line) = value.parse::<isize>().ok().filter(|line| *line != 0) {
                return GridLine::Line(line);
            }
            named_area_line(value, named_areas, column_axis, start_side)
                .map(|line| GridLine::Line((line + 1) as isize))
                .unwrap_or(GridLine::Auto)
        }
        _ => GridLine::Auto,
    }
}

fn named_area_line(
    name: &str,
    named_areas: &HashMap<String, NamedArea>,
    column_axis: bool,
    start_side: bool,
) -> Option<usize> {
    let (area_name, boundary_is_start) = if named_areas.contains_key(name) {
        (name, start_side)
    } else if let Some(name) = name.strip_suffix("-start") {
        (name, true)
    } else if let Some(name) = name.strip_suffix("-end") {
        (name, false)
    } else {
        (name, start_side)
    };
    let area = named_areas.get(area_name)?;
    let (start, span) = if column_axis {
        (area.column_start, area.column_span)
    } else {
        (area.row_start, area.row_span)
    };
    Some(if boundary_is_start { start } else { start + span })
}

fn named_areas(style: &ComputedStyle) -> (HashMap<String, NamedArea>, usize, usize) {
    let Some(ComputedValue::Keyword(value)) = style.get("grid-template-areas") else {
        return (HashMap::new(), 0, 0);
    };
    let rows = parse_area_rows(value);
    let row_count = rows.len();
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut cells: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for (row, names) in rows.iter().enumerate() {
        for (column, name) in names.iter().enumerate() {
            if !name.is_empty() && !name.chars().all(|ch| ch == '.') {
                cells.entry(name.clone()).or_default().push((row, column));
            }
        }
    }

    let mut areas = HashMap::new();
    for (name, positions) in cells {
        let row_start = positions.iter().map(|(row, _)| *row).min().unwrap_or(0);
        let row_end = positions.iter().map(|(row, _)| *row).max().unwrap_or(row_start) + 1;
        let column_start = positions.iter().map(|(_, column)| *column).min().unwrap_or(0);
        let column_end = positions.iter().map(|(_, column)| *column).max().unwrap_or(column_start) + 1;
        let rectangular = positions.len() == (row_end - row_start) * (column_end - column_start)
            && (row_start..row_end).all(|row| {
                (column_start..column_end).all(|column| {
                    rows.get(row)
                        .and_then(|row| row.get(column))
                        .is_some_and(|cell| cell == &name)
                })
            });
        if rectangular {
            areas.insert(name, NamedArea {
                row_start,
                row_span: row_end - row_start,
                column_start,
                column_span: column_end - column_start,
            });
        }
    }
    (areas, row_count, column_count)
}

fn parse_area_rows(value: &str) -> Vec<Vec<String>> {
    if value.trim().eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for ch in value.chars() {
        if !quoted {
            if ch == '"' {
                quoted = true;
                current.clear();
            }
            continue;
        }
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            rows.push(current.split_whitespace().map(str::to_string).collect());
            quoted = false;
        } else {
            current.push(ch);
        }
    }
    rows
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
    let mut flexible = Vec::new();
    let mut non_flexible = gaps;
    for (index, track) in tracks.iter().enumerate() {
        let minimum = resolve_track_size(track.min, basis)
            .unwrap_or_else(|| auto_sizes.get(index).copied().unwrap_or(0.0))
            .max(0.0);
        match track.max {
            TrackSize::Fr(fraction) if fraction > 0.0 => {
                sizes[index] = minimum;
                flexible.push((index, fraction));
            }
            TrackSize::Auto => {
                sizes[index] = minimum.max(auto_sizes.get(index).copied().unwrap_or(0.0));
                non_flexible += sizes[index];
            }
            maximum => {
                sizes[index] = resolve_track_size(maximum, basis).unwrap_or(minimum).max(minimum);
                non_flexible += sizes[index];
            }
        }
    }

    let mut remaining_space = (basis - non_flexible).max(0.0);
    let mut remaining_fraction = flexible.iter().map(|(_, fraction)| *fraction).sum::<f32>();
    let mut frozen = vec![false; flexible.len()];
    loop {
        let mut changed = false;
        for (flex_index, &(track_index, fraction)) in flexible.iter().enumerate() {
            if frozen[flex_index] || remaining_fraction <= 0.0 { continue; }
            let share = remaining_space * fraction / remaining_fraction;
            if share < sizes[track_index] {
                remaining_space = (remaining_space - sizes[track_index]).max(0.0);
                remaining_fraction -= fraction;
                frozen[flex_index] = true;
                changed = true;
            }
        }
        if !changed { break; }
    }
    for (flex_index, &(track_index, fraction)) in flexible.iter().enumerate() {
        if !frozen[flex_index] && remaining_fraction > 0.0 {
            sizes[track_index] = sizes[track_index]
                .max(remaining_space * fraction / remaining_fraction);
        }
    }
    sizes
}

fn fixed_track(track: Track, basis: f32) -> Option<f32> {
    if matches!(track.max, TrackSize::Fr(_) | TrackSize::Auto) {
        return None;
    }
    let minimum = resolve_track_size(track.min, basis).unwrap_or(0.0);
    resolve_track_size(track.max, basis).map(|maximum| maximum.max(minimum))
}

fn resolve_track_size(size: TrackSize, basis: f32) -> Option<f32> {
    match size {
        TrackSize::Px(value) => Some(value),
        TrackSize::Percent(value) => Some(basis * value / 100.0),
        TrackSize::Calc(px, percentage) => Some(px + basis * percentage / 100.0),
        TrackSize::Fr(_) | TrackSize::Auto => None,
    }
}

fn offsets(sizes: &[f32], gap: f32, start: f32) -> Vec<f32> {
    let mut cursor = start;
    sizes.iter().map(|size| { let current = cursor; cursor += size + gap; current }).collect()
}

fn track_list(style: &ComputedStyle, property: &str, basis: f32, gap: f32) -> Option<Vec<Track>> {
    let value = match style.get(property)? {
        ComputedValue::Keyword(value) => value,
        ComputedValue::Px(value) => return Some(vec![Track::new(TrackSize::Px(*value))]),
        ComputedValue::Percentage(value) => {
            return Some(vec![Track::new(TrackSize::Percent(*value))]);
        }
        _ => return None,
    };
    if value.eq_ignore_ascii_case("none") { return Some(Vec::new()); }
    let mut result = Vec::new();
    for token in split_tracks(value) {
        let token = token.trim();
        if token.starts_with('[') && token.ends_with(']') { continue; }
        if token.to_ascii_lowercase().starts_with("repeat(") && token.ends_with(')') {
            let inner = &token[7..token.len() - 1];
            let Some((count, pattern)) = split_once_top_level(inner, ',') else {
                result.push(Track::auto());
                continue;
            };
            let pattern_tracks = parse_track_sequence(pattern);
            if pattern_tracks.is_empty() {
                result.push(Track::auto());
                continue;
            }
            let repetition = count.trim();
            let count = if repetition.eq_ignore_ascii_case("auto-fill")
                || repetition.eq_ignore_ascii_case("auto-fit")
            {
                auto_repeat_count(&pattern_tracks, basis, gap)
            } else {
                repetition.parse::<usize>().unwrap_or(1)
            };
            let auto_fit = repetition.eq_ignore_ascii_case("auto-fit");
            for _ in 0..count.max(1) {
                result.extend(pattern_tracks.iter().map(|track| Track { auto_fit, ..*track }));
            }
        } else {
            result.push(parse_track(token).unwrap_or_else(Track::auto));
        }
    }
    Some(result)
}

fn split_tracks(value: &str) -> Vec<String> {
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut start = 0usize;
    let mut result = Vec::new();
    for (index, ch) in value.char_indices() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ => {}
        }
        if ch.is_whitespace() && paren_depth == 0 && bracket_depth == 0 {
            if start < index { result.push(value[start..index].to_string()); }
            start = index + ch.len_utf8();
        }
    }
    if start < value.len() { result.push(value[start..].to_string()); }
    result
}

fn parse_track(value: &str) -> Option<Track> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("auto")
        || value.eq_ignore_ascii_case("min-content")
        || value.eq_ignore_ascii_case("max-content")
    {
        return Some(Track::auto());
    }
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("minmax(") && lower.ends_with(')') {
        let inner = &value[7..value.len() - 1];
        let (minimum, maximum) = split_once_top_level(inner, ',')?;
        let min = parse_track_size(minimum.trim(), false)?;
        let max = parse_track_size(maximum.trim(), true)?;
        return Some(Track { min, max, auto_fit: false });
    }
    parse_track_size(value, true).map(Track::new)
}

fn parse_track_size(value: &str, allow_fr: bool) -> Option<TrackSize> {
    let lower = value.trim().to_ascii_lowercase();
    if lower == "auto" || lower == "min-content" || lower == "max-content" {
        return Some(TrackSize::Auto);
    }
    if lower == "0" { return Some(TrackSize::Px(0.0)); }
    if lower.starts_with("calc(") && lower.ends_with(')') {
        return parse_calc_track(&lower);
    }
    if let Some(value) = lower.strip_suffix("px") {
        return value.trim().parse().ok().map(TrackSize::Px);
    }
    if let Some(value) = lower.strip_suffix('%') {
        return value.trim().parse().ok().map(TrackSize::Percent);
    }
    if allow_fr
        && let Some(value) = lower.strip_suffix("fr") {
            return value.trim().parse().ok().map(TrackSize::Fr);
        }
    None
}

fn parse_calc_track(value: &str) -> Option<TrackSize> {
    let inner = value.strip_prefix("calc(")?.strip_suffix(')')?.trim();
    let parts: Vec<_> = inner.split_whitespace().collect();
    if parts.len() != 3 { return None; }
    let (left_px, left_percentage) = calc_component(parts[0])?;
    let sign = match parts[1] { "+" => 1.0, "-" => -1.0, _ => return None };
    let (right_px, right_percentage) = calc_component(parts[2])?;
    Some(TrackSize::Calc(
        left_px + sign * right_px,
        left_percentage + sign * right_percentage,
    ))
}

fn calc_component(value: &str) -> Option<(f32, f32)> {
    if let Some(value) = value.strip_suffix("px") {
        return Some((value.parse().ok()?, 0.0));
    }
    if let Some(value) = value.strip_suffix('%') {
        return Some((0.0, value.parse().ok()?));
    }
    None
}

fn parse_track_sequence(value: &str) -> Vec<Track> {
    split_tracks(value)
        .into_iter()
        .filter(|token| !(token.starts_with('[') && token.ends_with(']')))
        .map(|token| parse_track(&token).unwrap_or_else(Track::auto))
        .collect()
}

fn split_once_top_level(value: &str, delimiter: char) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (index, ch) in value.char_indices() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if ch == delimiter && depth == 0 => {
                return Some((&value[..index], &value[index + ch.len_utf8()..]));
            }
            _ => {}
        }
    }
    None
}

fn auto_repeat_count(pattern: &[Track], basis: f32, gap: f32) -> usize {
    let minimum = pattern.iter().map(|track| {
        resolve_track_size(track.min, basis).unwrap_or(0.0).max(0.0)
    }).sum::<f32>();
    let stride = minimum + gap * pattern.len() as f32;
    if stride <= 0.0 { 1 } else { ((basis + gap) / stride).floor().max(1.0) as usize }
}

fn collapse_empty_auto_fit_tracks(
    tracks: &mut Vec<Track>,
    placements: &mut [Placement],
    columns: bool,
) {
    let collapsed: Vec<_> = tracks
        .iter()
        .enumerate()
        .filter_map(|(index, track)| {
            if !track.auto_fit { return None; }
            let occupied = placements.iter().any(|placement| {
                let (start, span) = if columns {
                    (placement.column, placement.column_span)
                } else {
                    (placement.row, placement.row_span)
                };
                index >= start && index < start + span
            });
            (!occupied).then_some(index)
        })
        .collect();

    for placement in placements {
        let start = if columns { &mut placement.column } else { &mut placement.row };
        *start -= collapsed.partition_point(|index| *index < *start);
    }
    for index in collapsed.into_iter().rev() {
        tracks.remove(index);
    }
}
