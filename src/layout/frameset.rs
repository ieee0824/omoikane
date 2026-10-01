//! Legacy frameset tracks shared by live rendering and screenshots.

use super::*;

pub(super) fn layout_frameset_document(
    document: &NodeHandle,
    frameset: &NodeHandle,
    resolver: &mut StyleResolver,
    viewport: Rect,
) -> LayoutBox {
    let mut root = frame_box(document, viewport);
    root.children
        .push(layout_frameset(frameset, resolver, viewport));
    root
}

fn layout_frameset(node: &NodeHandle, resolver: &mut StyleResolver, rect: Rect) -> LayoutBox {
    let mut layout = frame_box(node, rect);
    let children: Vec<_> = node
        .child_nodes()
        .into_iter()
        .filter(|child| matches!(child.tag_name().as_deref(), Some("frame" | "frameset")))
        .filter(|child| !resolver.computed_style(child).is_display_none())
        .collect();
    let rows = node.get_attribute("rows");
    let cols = node.get_attribute("cols");
    let row_count = rows.as_deref().map(track_count).unwrap_or(1);
    let col_count = cols.as_deref().map(track_count).unwrap_or(1);
    let heights =
        parse_frameset_track_sizes(rows.as_deref(), row_count, rect.height.max(0.0) as u32);
    let widths = parse_frameset_track_sizes(cols.as_deref(), col_count, rect.width.max(0.0) as u32);
    let mut y = rect.y;
    let mut children = children.into_iter();
    for height in heights {
        let mut x = rect.x;
        for width in &widths {
            if let Some(child) = children.next() {
                let child_rect = Rect {
                    x,
                    y,
                    width: *width as f32,
                    height: height as f32,
                };
                layout.children.push(if child.has_tag_name("frameset") {
                    layout_frameset(&child, resolver, child_rect)
                } else {
                    frame_box(&child, child_rect)
                });
            }
            x += *width as f32;
        }
        y += height as f32;
    }
    layout
}

fn track_count(spec: &str) -> usize {
    spec.split(',')
        .filter(|token| !token.trim().is_empty())
        .count()
        .max(1)
}

fn frame_box(node: &NodeHandle, rect: Rect) -> LayoutBox {
    LayoutBox {
        node: node.clone(),
        pseudo: None,
        dimensions: BoxDimensions {
            content: rect,
            ..BoxDimensions::default()
        },
        visibility: Visibility::Visible,
        overflow: Overflow::Hidden,
        position_scheme: PositionScheme::Static,
        fixed_containing_block: false,
        z_index: 0,
        transform: AffineTransform::identity(),
        needs_scroll_translation: false,
        has_out_of_flow_descendants: None,
        content_visibility_contents_skipped: false,
        paint_scroll: None,
        block_fragments: Vec::new(),
        multicol: None,
        lines: Vec::new(),
        children: Vec::new(),
        marker: None,
    }
}

/// Resolves fixed pixel values, percentages and weighted stars into bounded
/// viewport tracks. Legacy numeric lists summing to 100 retain the screenshot
/// renderer's percentage interpretation.
pub(crate) fn parse_frameset_track_sizes(
    spec: Option<&str>,
    frame_count: usize,
    total_size: u32,
) -> Vec<u32> {
    if frame_count == 0 {
        return Vec::new();
    }

    let mut tokens: Vec<String> = spec
        .unwrap_or("")
        .split(',')
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        tokens.resize(frame_count, "*".to_string());
    }
    if tokens.len() < frame_count {
        tokens.resize(frame_count, "*".to_string());
    }
    if tokens.len() > frame_count {
        tokens.truncate(frame_count);
    }

    let all_plain_numeric = tokens
        .iter()
        .all(|token| !token.contains('*') && !token.ends_with('%') && token.parse::<f32>().is_ok());
    let numeric_sum = tokens
        .iter()
        .filter_map(|token| token.parse::<f32>().ok())
        .sum::<f32>();
    let treat_plain_as_percent = all_plain_numeric && (numeric_sum - 100.0).abs() <= 0.5;

    // Allocate absolute dimensions first, then percentages, then relative
    // dimensions, as specified by the HTML frameset rendering algorithm.
    let tracks: Vec<_> = tokens
        .iter()
        .map(|token| {
            if let Some(value) = token.strip_suffix('%') {
                (1, value.parse::<f64>().unwrap_or(0.0).max(0.0))
            } else if let Some(value) = token.strip_suffix('*') {
                (2, value.trim().parse::<f64>().unwrap_or(1.0).max(1.0))
            } else if let Ok(value) = token.parse::<f64>() {
                (usize::from(treat_plain_as_percent), value.max(0.0))
            } else {
                (2, 1.0)
            }
        })
        .collect();
    let mut sizes = vec![0.0; frame_count];
    let mut remaining = f64::from(total_size);
    for unit in 0..3 {
        let total: f64 = tracks
            .iter()
            .filter(|(kind, _)| *kind == unit)
            .map(|(_, value)| value)
            .sum();
        if total <= 0.0 {
            continue;
        }
        let requested = match unit {
            0 => total,
            1 => total * f64::from(total_size) / 100.0,
            _ => remaining,
        };
        let allocated = requested.min(remaining);
        for (index, (kind, value)) in tracks.iter().enumerate() {
            if *kind == unit {
                sizes[index] = value * allocated / total;
            }
        }
        remaining = (remaining - allocated).max(0.0);
    }
    let mut widths: Vec<_> = sizes.iter().map(|size| size.floor() as u32).collect();
    let remainder = total_size.saturating_sub(widths.iter().sum());
    distribute_track_remainder(&tracks, &mut widths, remainder);
    widths
}

/// Assigns integer rounding and unused space in HTML's unit priority order.
fn distribute_track_remainder(tracks: &[(usize, f64)], widths: &mut [u32], remainder: u32) {
    if let Some(index) = tracks.iter().rposition(|(unit, _)| *unit == 2) {
        widths[index] += remainder;
        return;
    }
    for unit in [1, 0] {
        let indices: Vec<_> = tracks
            .iter()
            .enumerate()
            .filter_map(|(index, (kind, _))| (*kind == unit).then_some(index))
            .collect();
        if indices.is_empty() {
            continue;
        }
        let base = remainder / indices.len() as u32;
        let extra = remainder % indices.len() as u32;
        for (position, index) in indices.into_iter().enumerate() {
            widths[index] += base + u32::from((position as u32) < extra);
        }
        return;
    }
    if let Some(last) = widths.last_mut() {
        *last += remainder;
    }
}

#[cfg(test)]
mod tests {
    use super::parse_frameset_track_sizes;

    #[test]
    fn tracks_allocate_absolute_then_percentage_then_relative() {
        assert_eq!(
            parse_frameset_track_sizes(Some("100,25%,*"), 3, 600),
            [100, 150, 350]
        );
        assert_eq!(
            parse_frameset_track_sizes(Some("600,50%,*"), 3, 800),
            [600, 200, 0]
        );
        assert_eq!(
            parse_frameset_track_sizes(Some("200,600,*"), 3, 400),
            [100, 300, 0]
        );
        assert_eq!(
            parse_frameset_track_sizes(Some("*,2*,*"), 3, 800),
            [200, 400, 200]
        );
        assert_eq!(
            parse_frameset_track_sizes(Some("100,100"), 2, 1000),
            [500, 500]
        );
        assert_eq!(parse_frameset_track_sizes(Some("0*,*"), 2, 100), [50, 50]);
    }
}
