//! Basic SVG rendering: rasterizes inline SVG elements to an RGBA image.

use crate::dom::{Node, NodeHandle, NodeType};
use crate::paint::Canvas;
use crate::paint::Image;
use crate::paint::color::{Color, parse_color};

/// Renders an inline `<svg>` element to an RGBA image.
///
/// Returns `None` if the element is not an `<svg>` or has no renderable size.
pub fn render_svg_to_image(svg_node: &NodeHandle) -> Option<Image> {
    if svg_node.tag_name().as_deref() != Some("svg") {
        return None;
    }
    let attrs = svg_node.attributes().unwrap_or_default();

    // Determine SVG canvas size from width/height attributes or viewBox.
    let (vb_x, vb_y, vb_w, vb_h) = parse_viewbox(attrs.get("viewBox").or(attrs.get("viewbox")));
    let width = parse_svg_size(attrs.get("width")).unwrap_or(vb_w);
    let height = parse_svg_size(attrs.get("height")).unwrap_or(vb_h);

    if width <= 0.0 || height <= 0.0 || width > 4096.0 || height > 4096.0 {
        return None;
    }

    let w = width.round() as u32;
    let h = height.round() as u32;
    let mut canvas = Canvas::new(w, h);

    // Scale factor from viewBox to canvas
    let sx = if vb_w > 0.0 { width / vb_w } else { 1.0 };
    let sy = if vb_h > 0.0 { height / vb_h } else { 1.0 };
    let tx = -vb_x * sx;
    let ty = -vb_y * sy;

    render_svg_children(svg_node, &mut canvas, sx, sy, tx, ty, Color::rgb(0, 0, 0));

    Image::new(w, h, canvas.into_pixels()).ok()
}

/// Returns the SVG element's intended display size as `(width, height)`.
pub fn svg_display_size(svg_node: &NodeHandle) -> Option<(f32, f32)> {
    let attrs = svg_node.attributes().unwrap_or_default();
    let (_, _, vb_w, vb_h) = parse_viewbox(attrs.get("viewBox").or(attrs.get("viewbox")));
    let width = parse_svg_size(attrs.get("width")).unwrap_or(vb_w);
    let height = parse_svg_size(attrs.get("height")).unwrap_or(vb_h);
    if width > 0.0 && height > 0.0 {
        Some((width, height))
    } else {
        None
    }
}

fn render_svg_children(
    node: &NodeHandle,
    canvas: &mut Canvas,
    sx: f32,
    sy: f32,
    tx: f32,
    ty: f32,
    inherited_fill: Color,
) {
    for child in node.child_nodes() {
        if child.node_type() != NodeType::Element {
            continue;
        }
        let tag = match child.tag_name() {
            Some(t) => t,
            None => continue,
        };
        let attrs = child.attributes().unwrap_or_default();
        let fill_attr = attrs.get("fill").map(|s| s.as_str());
        // fill="none" means do not fill (transparent).
        let fill = match fill_attr {
            Some(v) if v.eq_ignore_ascii_case("none") => None,
            Some(v) => Some(parse_color(v).unwrap_or(inherited_fill)),
            None => Some(inherited_fill),
        };

        match tag.as_str() {
            "g" => {
                let g_fill = fill.unwrap_or(inherited_fill);
                render_svg_children(&child, canvas, sx, sy, tx, ty, g_fill);
            }
            "rect" => {
                let Some(fill_color) = fill else { continue };
                let rx = parse_svg_coord(attrs.get("x")).unwrap_or(0.0) * sx + tx;
                let ry = parse_svg_coord(attrs.get("y")).unwrap_or(0.0) * sy + ty;
                let rw = parse_svg_size(attrs.get("width")).unwrap_or(0.0) * sx;
                let rh = parse_svg_size(attrs.get("height")).unwrap_or(0.0) * sy;
                if rw > 0.0 && rh > 0.0 {
                    canvas.fill_rect(
                        crate::layout::Rect {
                            x: rx,
                            y: ry,
                            width: rw,
                            height: rh,
                        },
                        fill_color,
                    );
                }
            }
            "circle" => {
                let Some(fill_color) = fill else { continue };
                let cx = parse_svg_coord(attrs.get("cx")).unwrap_or(0.0) * sx + tx;
                let cy = parse_svg_coord(attrs.get("cy")).unwrap_or(0.0) * sy + ty;
                let r = parse_svg_size(attrs.get("r")).unwrap_or(0.0) * sx.min(sy);
                if r > 0.0 {
                    fill_circle(canvas, cx, cy, r, fill_color);
                }
            }
            "path" => {
                let Some(fill_color) = fill else { continue };
                if let Some(d) = attrs.get("d") {
                    let fill_rule = match attrs.get("fill-rule").map(String::as_str) {
                        Some(value) if value.eq_ignore_ascii_case("evenodd") => FillRule::EvenOdd,
                        _ => FillRule::NonZero,
                    };
                    render_path(canvas, d, sx, sy, tx, ty, fill_color, fill_rule);
                }
            }
            _ => {
                let recurse_fill = fill.unwrap_or(inherited_fill);
                // Recurse into unknown elements (may contain renderable children)
                render_svg_children(&child, canvas, sx, sy, tx, ty, recurse_fill);
            }
        }
    }
}

fn fill_circle(canvas: &mut Canvas, cx: f32, cy: f32, r: f32, color: Color) {
    let x0 = ((cx - r).floor() as i32).max(0) as u32;
    let y0 = ((cy - r).floor() as i32).max(0) as u32;
    let x1 = ((cx + r).ceil() as u32).min(canvas.width());
    let y1 = ((cy + r).ceil() as u32).min(canvas.height());
    let r2 = r * r;
    for py in y0..y1 {
        for px in x0..x1 {
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= r2 {
                canvas.set_pixel(px, py, color);
            }
        }
    }
}

/// Renders an SVG path `d` attribute using M, L, H, V, Z commands.
/// Curved commands (C, S, Q, T, A) are approximated as straight lines to endpoints.
fn render_path(
    canvas: &mut Canvas,
    d: &str,
    sx: f32,
    sy: f32,
    tx: f32,
    ty: f32,
    fill: Color,
    fill_rule: FillRule,
) {
    let commands = parse_path_data(d);
    if commands.is_empty() {
        return;
    }

    // Collect polygon points from path commands
    let mut points = Vec::new();
    let mut subpaths = Vec::new();
    let mut cx = 0.0f32;
    let mut cy = 0.0f32;

    for cmd in &commands {
        match cmd {
            PathCommand::MoveTo(x, y) => {
                // Flush previous subpath
                if points.len() >= 3 {
                    subpaths.push(std::mem::take(&mut points));
                }
                cx = *x;
                cy = *y;
                points.push((cx * sx + tx, cy * sy + ty));
            }
            PathCommand::LineTo(x, y) => {
                cx = *x;
                cy = *y;
                points.push((cx * sx + tx, cy * sy + ty));
            }
            PathCommand::HorizontalLineTo(x) => {
                cx = *x;
                points.push((cx * sx + tx, cy * sy + ty));
            }
            PathCommand::VerticalLineTo(y) => {
                cy = *y;
                points.push((cx * sx + tx, cy * sy + ty));
            }
            PathCommand::CurveTo(_, _, _, _, x, y) => {
                cx = *x;
                cy = *y;
                points.push((cx * sx + tx, cy * sy + ty));
            }
            PathCommand::Close => {
                if points.len() >= 3 {
                    subpaths.push(std::mem::take(&mut points));
                }
            }
        }
    }

    // Flush remaining subpath
    if points.len() >= 3 {
        subpaths.push(points);
    }
    if !subpaths.is_empty() {
        fill_compound_path(canvas, &subpaths, fill, fill_rule);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FillRule {
    NonZero,
    EvenOdd,
}

#[derive(Debug)]
enum PathCommand {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    HorizontalLineTo(f32),
    VerticalLineTo(f32),
    // Control points (cp1x, cp1y, cp2x, cp2y) + endpoint (x, y).
    // Control points are parsed but currently only the endpoint is used for rendering.
    #[allow(dead_code)]
    CurveTo(f32, f32, f32, f32, f32, f32),
    Close,
}

fn parse_path_data(d: &str) -> Vec<PathCommand> {
    let mut commands = Vec::new();
    let mut chars = d.chars().peekable();
    let mut current_command = ' ';
    let mut cx = 0.0f32;
    let mut cy = 0.0f32;

    while chars.peek().is_some() {
        let remaining_before = chars.clone().count();
        skip_whitespace_and_commas(&mut chars);
        if let Some(&ch) = chars.peek()
            && ch.is_ascii_alphabetic()
        {
            current_command = ch;
            chars.next();
        }
        skip_whitespace_and_commas(&mut chars);

        match current_command {
            'M' => {
                if let Some(x) = parse_number(&mut chars) {
                    skip_whitespace_and_commas(&mut chars);
                    if let Some(y) = parse_number(&mut chars) {
                        cx = x;
                        cy = y;
                        commands.push(PathCommand::MoveTo(x, y));
                        current_command = 'L'; // subsequent coords are LineTo
                    }
                }
            }
            'm' => {
                if let Some(dx) = parse_number(&mut chars) {
                    skip_whitespace_and_commas(&mut chars);
                    if let Some(dy) = parse_number(&mut chars) {
                        cx += dx;
                        cy += dy;
                        commands.push(PathCommand::MoveTo(cx, cy));
                        current_command = 'l';
                    }
                }
            }
            'L' => {
                if let Some(x) = parse_number(&mut chars) {
                    skip_whitespace_and_commas(&mut chars);
                    if let Some(y) = parse_number(&mut chars) {
                        cx = x;
                        cy = y;
                        commands.push(PathCommand::LineTo(x, y));
                    }
                }
            }
            'l' => {
                if let Some(dx) = parse_number(&mut chars) {
                    skip_whitespace_and_commas(&mut chars);
                    if let Some(dy) = parse_number(&mut chars) {
                        cx += dx;
                        cy += dy;
                        commands.push(PathCommand::LineTo(cx, cy));
                    }
                }
            }
            'H' => {
                if let Some(x) = parse_number(&mut chars) {
                    cx = x;
                    commands.push(PathCommand::HorizontalLineTo(x));
                }
            }
            'h' => {
                if let Some(dx) = parse_number(&mut chars) {
                    cx += dx;
                    commands.push(PathCommand::HorizontalLineTo(cx));
                }
            }
            'V' => {
                if let Some(y) = parse_number(&mut chars) {
                    cy = y;
                    commands.push(PathCommand::VerticalLineTo(y));
                }
            }
            'v' => {
                if let Some(dy) = parse_number(&mut chars) {
                    cy += dy;
                    commands.push(PathCommand::VerticalLineTo(cy));
                }
            }
            'C' => {
                let mut nums = Vec::new();
                for _ in 0..6 {
                    skip_whitespace_and_commas(&mut chars);
                    if let Some(n) = parse_number(&mut chars) {
                        nums.push(n);
                    }
                }
                if nums.len() == 6 {
                    cx = nums[4];
                    cy = nums[5];
                    commands.push(PathCommand::CurveTo(
                        nums[0], nums[1], nums[2], nums[3], nums[4], nums[5],
                    ));
                }
            }
            'c' => {
                let mut nums = Vec::new();
                for _ in 0..6 {
                    skip_whitespace_and_commas(&mut chars);
                    if let Some(n) = parse_number(&mut chars) {
                        nums.push(n);
                    }
                }
                if nums.len() == 6 {
                    cx += nums[4];
                    cy += nums[5];
                    commands.push(PathCommand::CurveTo(
                        cx - nums[4] + nums[0],
                        cy - nums[5] + nums[1],
                        cx - nums[4] + nums[2],
                        cy - nums[5] + nums[3],
                        cx,
                        cy,
                    ));
                }
            }
            'Z' | 'z' => {
                commands.push(PathCommand::Close);
            }
            _ => {
                // Skip unknown commands
                chars.next();
            }
        }

        // Prevent infinite loops on malformed input
        let remaining_after = chars.clone().count();
        if remaining_after >= remaining_before {
            chars.next(); // Force progress
        }
    }

    commands
}

fn skip_whitespace_and_commas(chars: &mut std::iter::Peekable<std::str::Chars>) {
    while let Some(&ch) = chars.peek() {
        if ch.is_ascii_whitespace() || ch == ',' {
            chars.next();
        } else {
            break;
        }
    }
}

fn parse_number(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<f32> {
    let mut s = String::new();
    if let Some(&ch) = chars.peek()
        && (ch == '-' || ch == '+')
    {
        s.push(ch);
        chars.next();
    }
    let mut has_dot = false;
    while let Some(&ch) = chars.peek() {
        if ch.is_ascii_digit() {
            s.push(ch);
            chars.next();
        } else if ch == '.' && !has_dot {
            has_dot = true;
            s.push(ch);
            chars.next();
        } else {
            break;
        }
    }
    if s.is_empty() || s == "-" || s == "+" {
        None
    } else {
        s.parse().ok()
    }
}

/// Scanline fill for all subpaths together, preserving holes according to the
/// SVG `fill-rule` property.
fn fill_compound_path(
    canvas: &mut Canvas,
    subpaths: &[Vec<(f32, f32)>],
    color: Color,
    fill_rule: FillRule,
) {
    if subpaths.is_empty() {
        return;
    }

    let min_y = subpaths
        .iter()
        .flatten()
        .map(|p| p.1)
        .fold(f32::MAX, f32::min);
    let max_y = subpaths
        .iter()
        .flatten()
        .map(|p| p.1)
        .fold(f32::MIN, f32::max);
    let y_start = (min_y.floor() as i32).max(0) as u32;
    let y_end = (max_y.ceil() as u32).min(canvas.height());

    for y in y_start..y_end {
        let scan_y = y as f32 + 0.5;
        let mut intersections = Vec::new();

        for points in subpaths {
            for i in 0..points.len() {
                let j = (i + 1) % points.len();
                let (x0, y0) = points[i];
                let (x1, y1) = points[j];

                if (y0 <= scan_y && y1 > scan_y) || (y1 <= scan_y && y0 > scan_y) {
                    let t = (scan_y - y0) / (y1 - y0);
                    let winding = if y1 > y0 { 1 } else { -1 };
                    intersections.push((x0 + t * (x1 - x0), winding));
                }
            }
        }

        intersections.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut winding = 0i32;
        for pair in intersections.windows(2) {
            winding += pair[0].1;
            let inside = match fill_rule {
                FillRule::NonZero => winding != 0,
                FillRule::EvenOdd => winding.unsigned_abs() % 2 == 1,
            };
            if inside {
                let x_start = (pair[0].0.floor() as i32).max(0) as u32;
                let x_end = (pair[1].0.ceil() as u32).min(canvas.width());
                for x in x_start..x_end {
                    canvas.set_pixel(x, y, color);
                }
            }
        }
    }
}

fn parse_viewbox(value: Option<&String>) -> (f32, f32, f32, f32) {
    let Some(vb) = value else {
        return (0.0, 0.0, 0.0, 0.0);
    };
    let parts: Vec<f32> = vb
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter_map(|s| s.parse().ok())
        .collect();
    match parts.as_slice() {
        [x, y, w, h] => (*x, *y, *w, *h),
        _ => (0.0, 0.0, 0.0, 0.0),
    }
}

/// Parses a non-negative SVG length (for width, height, r, etc.).
fn parse_svg_size(value: Option<&String>) -> Option<f32> {
    let s = value?.trim();
    s.strip_suffix("px")
        .unwrap_or(s)
        .parse::<f32>()
        .ok()
        .filter(|v| *v > 0.0)
}

/// Parses an SVG coordinate (may be negative, for x, y, cx, cy, etc.).
fn parse_svg_coord(value: Option<&String>) -> Option<f32> {
    let s = value?.trim();
    s.strip_suffix("px").unwrap_or(s).parse::<f32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::{Node, NodeHandle};
    use crate::html::TreeBuilder;

    fn find_svg(node: &NodeHandle) -> Option<NodeHandle> {
        if node.tag_name().as_deref() == Some("svg") {
            return Some(node.clone());
        }
        for child in node.child_nodes() {
            if let Some(found) = find_svg(&child) {
                return Some(found);
            }
        }
        None
    }

    #[test]
    fn renders_svg_rect() {
        let html = r#"<svg width="10" height="10"><rect x="0" y="0" width="10" height="10" fill="red"/></svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let svg = find_svg(&doc).unwrap();
        let image = render_svg_to_image(&svg).unwrap();
        assert_eq!(image.width(), 10);
        assert_eq!(image.height(), 10);
        // Center pixel should be red
        let idx = (5 * 10 + 5) * 4;
        assert_eq!(image.pixels()[idx], 255); // R
        assert_eq!(image.pixels()[idx + 1], 0); // G
        assert_eq!(image.pixels()[idx + 2], 0); // B
        assert_eq!(image.pixels()[idx + 3], 255); // A
    }

    #[test]
    fn renders_svg_circle() {
        let html =
            r#"<svg width="20" height="20"><circle cx="10" cy="10" r="8" fill="blue"/></svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let svg = find_svg(&doc).unwrap();
        let image = render_svg_to_image(&svg).unwrap();
        assert_eq!(image.width(), 20);
        assert_eq!(image.height(), 20);
        // Center pixel should be blue
        let idx = (10 * 20 + 10) * 4;
        assert_eq!(image.pixels()[idx], 0);
        assert_eq!(image.pixels()[idx + 1], 0);
        assert_eq!(image.pixels()[idx + 2], 255);
    }

    #[test]
    fn svg_display_size_from_attributes() {
        let html = r#"<svg width="24" height="24" viewBox="0 0 24 24"></svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let svg = find_svg(&doc).unwrap();
        assert_eq!(svg_display_size(&svg), Some((24.0, 24.0)));
    }

    #[test]
    fn svg_display_size_from_viewbox_only() {
        let html = r#"<svg viewBox="0 0 32 32"></svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let svg = find_svg(&doc).unwrap();
        assert_eq!(svg_display_size(&svg), Some((32.0, 32.0)));
    }

    #[test]
    fn parse_path_data_m_l_z() {
        let cmds = parse_path_data("M 0 0 L 10 0 L 10 10 Z");
        assert_eq!(cmds.len(), 4);
    }

    #[test]
    fn nonzero_fill_rule_preserves_oppositely_wound_hole() {
        let html =
            r#"<svg width="10" height="10"><path fill="black" d="M1 1H9V9H1Z M3 3V7H7V3Z"/></svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let image = render_svg_to_image(&find_svg(&doc).unwrap()).unwrap();

        assert_eq!(image.pixels()[(2 * 10 + 2) * 4 + 3], 255);
        assert_eq!(image.pixels()[(5 * 10 + 5) * 4 + 3], 0);
    }

    #[test]
    fn evenodd_fill_rule_preserves_same_winding_hole() {
        let html = r#"<svg width="10" height="10"><path fill="black" fill-rule="evenodd" d="M1 1H9V9H1Z M3 3H7V7H3Z"/></svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let image = render_svg_to_image(&find_svg(&doc).unwrap()).unwrap();

        assert_eq!(image.pixels()[(2 * 10 + 2) * 4 + 3], 255);
        assert_eq!(image.pixels()[(5 * 10 + 5) * 4 + 3], 0);
    }
}
