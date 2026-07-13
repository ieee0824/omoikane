//! Basic explicit CSS Grid track sizing and row-major auto-placement.

use crate::css::{ComputedStyle, ComputedValue, StyleResolver};
use crate::dom::{Node, NodeHandle, NodeType};

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
    viewport: Rect,
) -> Option<LayoutBox> {
    let mut items = Vec::new();
    let mut positioned = Vec::new();
    for child in node.child_nodes() {
        if child.node_type() != NodeType::Element { continue; }
        let child_style = resolver.computed_style(&child);
        if is_display_none(&child_style) { continue; }
        if is_out_of_flow_positioned(&child_style) {
            positioned.push((child, child_style));
        } else {
            items.push(child);
        }
    }

    let columns = track_list(&style, "grid-template-columns")
        .filter(|tracks| !tracks.is_empty())
        .unwrap_or_else(|| vec![Track::Fr(1.0)]);
    let explicit_rows = track_list(&style, "grid-template-rows").unwrap_or_default();
    let column_gap = gap(&style, "column-gap");
    let row_gap = gap(&style, "row-gap");
    let column_intrinsics = auto_column_intrinsics(&columns, &items, resolver);
    let column_widths = resolve_tracks(&columns, width, column_gap, &column_intrinsics);
    let row_count = items.len().div_ceil(columns.len()).max(explicit_rows.len());

    let mut laid_out = Vec::new();
    let mut content_row_heights = vec![0.0f32; row_count];
    for (index, child) in items.iter().enumerate() {
        let column = index % columns.len();
        let row = index / columns.len();
        let height = explicit_rows.get(row).and_then(|track| fixed_track(*track, 0.0)).unwrap_or(0.0);
        let containing = Rect { x: 0.0, y: 0.0, width: column_widths[column], height };
        if let Some(layout) = layout_node(child, resolver, containing, viewport, None) {
            content_row_heights[row] = content_row_heights[row].max(layout.total_height());
            laid_out.push((index, layout));
        }
    }

    let specified_height = resolved_length(&style, "height", 0.0)
        .map(|height| super::border_box_adjust_height(&style, height, &padding, &border));
    let row_basis = specified_height.unwrap_or(0.0);
    let mut row_tracks = explicit_rows;
    row_tracks.resize(row_count, Track::Auto);
    let row_heights = resolve_tracks(&row_tracks, row_basis, row_gap, &content_row_heights);
    let column_offsets = offsets(&column_widths, column_gap, x);
    let row_offsets = offsets(&row_heights, row_gap, y);
    let mut children = Vec::new();
    for (index, mut child) in laid_out {
        translate_layout_box_to_outer(
            &mut child,
            column_offsets[index % columns.len()],
            row_offsets[index / columns.len()],
        );
        children.push(child);
    }

    let auto_height = row_heights.iter().sum::<f32>()
        + row_gap * row_heights.len().saturating_sub(1) as f32;
    let mut content_height = specified_height.unwrap_or(auto_height);
    let (min_height, max_height) = normalized_min_max_lengths(&style, "min-height", "max-height", 0.0);
    if let Some(value) = min_height { content_height = content_height.max(super::border_box_adjust_height(&style, value, &padding, &border)); }
    if let Some(value) = max_height { content_height = content_height.min(super::border_box_adjust_height(&style, value, &padding, &border)); }
    let dimensions = BoxDimensions { content: Rect { x, y, width, height: content_height }, padding, border, margin };
    for (child, child_style) in positioned {
        if let Some(child) = layout_positioned_child(&child, resolver, &child_style, dimensions, dimensions.content, viewport) {
            children.push(child);
        }
    }
    sort_children_by_z_index(&mut children);
    Some(LayoutBox { node: node.clone(), dimensions, visibility: visibility(&style), overflow: overflow(&style), z_index: z_index(&style), lines: Vec::new(), children, marker: None })
}

fn gap(style: &ComputedStyle, property: &str) -> f32 {
    match style.get(property) { Some(ComputedValue::Px(value)) => value.max(0.0), _ => 0.0 }
}

fn auto_column_intrinsics(tracks: &[Track], items: &[NodeHandle], resolver: &mut StyleResolver) -> Vec<f32> {
    let mut values = vec![0.0f32; tracks.len()];
    for (index, child) in items.iter().enumerate() {
        let column = index % tracks.len();
        if tracks[column] == Track::Auto {
            values[column] = values[column].max(intrinsic_width(child, resolver));
        }
    }
    values
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
