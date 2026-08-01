//! Basic SVG rendering: rasterizes inline SVG elements to an RGBA image.

use std::collections::BTreeMap;

use crate::dom::{Node, NodeHandle, NodeType};
use crate::paint::Canvas;
use crate::paint::Image;
use crate::paint::color::{Color, parse_color};

/// Renders an inline `<svg>` element to an RGBA image.
///
/// Returns `None` if the element is not an `<svg>` or has no renderable size.
pub fn render_svg_to_image(svg_node: &NodeHandle) -> Option<Image> {
    render_svg_to_image_with_current_color(svg_node, Color::rgb(0, 0, 0))
}

/// Renders an inline `<svg>` using `current_color` for `currentColor` paints.
pub(crate) fn render_svg_to_image_with_current_color(
    svg_node: &NodeHandle,
    current_color: Color,
) -> Option<Image> {
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

    let initial_paint = SvgPaint {
        fill: Some(Color::rgb(0, 0, 0)),
        stroke: None,
        stroke_width: 1.0,
        line_cap: LineCap::Butt,
        line_join: LineJoin::Miter,
        opacity: 1.0,
        fill_opacity: 1.0,
        stroke_opacity: 1.0,
        current_color,
    };
    let root_paint = resolve_paint(&initial_paint, &attrs);
    render_svg_children(
        svg_node,
        &mut canvas,
        sx,
        sy,
        tx,
        ty,
        root_paint,
    );

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

#[derive(Clone, Copy)]
struct SvgPaint {
    fill: Option<Color>,
    stroke: Option<Color>,
    stroke_width: f32,
    line_cap: LineCap,
    line_join: LineJoin,
    opacity: f32,
    fill_opacity: f32,
    stroke_opacity: f32,
    current_color: Color,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineCap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineJoin {
    Miter,
    Round,
    Bevel,
}

fn resolve_paint(parent: &SvgPaint, attrs: &BTreeMap<String, String>) -> SvgPaint {
    let current_color = property_value(attrs, "color")
        .and_then(|value| parse_color(&value))
        .unwrap_or(parent.current_color);
    let fill = property_value(attrs, "fill")
        .map(|value| parse_paint_value(&value, parent.fill, current_color))
        .unwrap_or(parent.fill);
    let stroke = property_value(attrs, "stroke")
        .map(|value| parse_paint_value(&value, parent.stroke, current_color))
        .unwrap_or(parent.stroke);
    let stroke_width = property_value(attrs, "stroke-width")
        .and_then(|value| parse_svg_nonnegative(Some(&value)))
        .unwrap_or(parent.stroke_width);
    let opacity = parent.opacity
        * parse_opacity(property_value(attrs, "opacity").as_deref()).unwrap_or(1.0);
    let fill_opacity = parent.fill_opacity
        * parse_opacity(property_value(attrs, "fill-opacity").as_deref()).unwrap_or(1.0);
    let stroke_opacity = parent.stroke_opacity
        * parse_opacity(property_value(attrs, "stroke-opacity").as_deref()).unwrap_or(1.0);
    let line_cap = property_value(attrs, "stroke-linecap")
        .map(|value| parse_line_cap(&value))
        .unwrap_or(parent.line_cap);
    let line_join = property_value(attrs, "stroke-linejoin")
        .map(|value| parse_line_join(&value))
        .unwrap_or(parent.line_join);
    SvgPaint {
        fill,
        stroke,
        stroke_width,
        line_cap,
        line_join,
        opacity,
        fill_opacity,
        stroke_opacity,
        current_color,
    }
}

fn parse_paint_value(value: &str, inherited: Option<Color>, current_color: Color) -> Option<Color> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") {
        None
    } else if value.eq_ignore_ascii_case("currentcolor") {
        Some(current_color)
    } else {
        parse_color(value).or(inherited)
    }
}

fn property_value(attrs: &BTreeMap<String, String>, name: &str) -> Option<String> {
    if let Some(style) = attrs
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("style"))
        .map(|(_, value)| value)
    {
        for declaration in style.split(';').rev() {
            let Some((key, value)) = declaration.split_once(':') else { continue };
            if key.trim().eq_ignore_ascii_case(name) {
                return Some(value.trim().to_string());
            }
        }
    }
    attrs
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_string())
}

fn parse_opacity(value: Option<&str>) -> Option<f32> {
    value?.trim().parse::<f32>().ok().map(|value| value.clamp(0.0, 1.0))
}

fn parse_line_cap(value: &str) -> LineCap {
    match value.trim().to_ascii_lowercase().as_str() {
        "round" => LineCap::Round,
        "square" => LineCap::Square,
        _ => LineCap::Butt,
    }
}

fn parse_line_join(value: &str) -> LineJoin {
    match value.trim().to_ascii_lowercase().as_str() {
        "round" => LineJoin::Round,
        "bevel" => LineJoin::Bevel,
        _ => LineJoin::Miter,
    }
}

fn with_alpha(color: Color, opacity: f32) -> Color {
    Color::rgba(color.r, color.g, color.b, (color.a as f32 * opacity.clamp(0.0, 1.0)).round() as u8)
}

fn parse_svg_points(value: Option<&String>) -> Vec<(f32, f32)> {
    let Some(value) = value else { return Vec::new() };
    let values = value
        .replace(',', " ")
        .split_whitespace()
        .filter_map(|part| part.parse::<f32>().ok())
        .collect::<Vec<_>>();
    values
        .chunks_exact(2)
        .map(|pair| (pair[0], pair[1]))
        .collect()
}

fn render_svg_children(
    node: &NodeHandle,
    canvas: &mut Canvas,
    sx: f32,
    sy: f32,
    tx: f32,
    ty: f32,
    inherited: SvgPaint,
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
        let paint = resolve_paint(&inherited, &attrs);
        let fill = paint
            .fill
            .map(|color| with_alpha(color, paint.opacity * paint.fill_opacity));
        let stroke = paint
            .stroke
            .map(|color| with_alpha(color, paint.opacity * paint.stroke_opacity));
        let tag = tag.to_ascii_lowercase();

        match tag.as_str() {
            "g" => {
                render_svg_children(&child, canvas, sx, sy, tx, ty, paint);
            }
            "rect" => {
                let rx = parse_svg_coord(attrs.get("x")).unwrap_or(0.0) * sx + tx;
                let ry = parse_svg_coord(attrs.get("y")).unwrap_or(0.0) * sy + ty;
                let rw = parse_svg_size(attrs.get("width")).unwrap_or(0.0) * sx;
                let rh = parse_svg_size(attrs.get("height")).unwrap_or(0.0) * sy;
                if rw > 0.0 && rh > 0.0 {
                    if let Some(fill_color) = fill {
                        canvas.fill_rect(
                            crate::layout::Rect { x: rx, y: ry, width: rw, height: rh },
                            fill_color,
                        );
                    }
                    let points = vec![(rx, ry), (rx + rw, ry), (rx + rw, ry + rh), (rx, ry + rh)];
                    stroke_polyline(canvas, &points, true, stroke, paint.stroke_width * sx.min(sy), paint.line_cap, paint.line_join);
                }
            }
            "circle" => {
                let cx = parse_svg_coord(attrs.get("cx")).unwrap_or(0.0) * sx + tx;
                let cy = parse_svg_coord(attrs.get("cy")).unwrap_or(0.0) * sy + ty;
                let r = parse_svg_size(attrs.get("r")).unwrap_or(0.0) * sx.min(sy);
                if r > 0.0 {
                    if let Some(fill_color) = fill {
                        fill_circle(canvas, cx, cy, r, fill_color);
                    }
                    if let Some(stroke_color) = stroke {
                        stroke_ellipse(canvas, cx, cy, r, r, paint.stroke_width * sx.min(sy), stroke_color);
                    }
                }
            }
            "ellipse" => {
                let cx = parse_svg_coord(attrs.get("cx")).unwrap_or(0.0) * sx + tx;
                let cy = parse_svg_coord(attrs.get("cy")).unwrap_or(0.0) * sy + ty;
                let rx = parse_svg_size(attrs.get("rx")).unwrap_or(0.0) * sx;
                let ry = parse_svg_size(attrs.get("ry")).unwrap_or(0.0) * sy;
                if rx > 0.0 && ry > 0.0 {
                    if let Some(fill_color) = fill {
                        fill_ellipse(canvas, cx, cy, rx, ry, fill_color);
                    }
                    if let Some(stroke_color) = stroke {
                        stroke_ellipse(canvas, cx, cy, rx, ry, paint.stroke_width * sx.min(sy), stroke_color);
                    }
                }
            }
            "line" => {
                let x1 = parse_svg_coord(attrs.get("x1")).unwrap_or(0.0) * sx + tx;
                let y1 = parse_svg_coord(attrs.get("y1")).unwrap_or(0.0) * sy + ty;
                let x2 = parse_svg_coord(attrs.get("x2")).unwrap_or(0.0) * sx + tx;
                let y2 = parse_svg_coord(attrs.get("y2")).unwrap_or(0.0) * sy + ty;
                stroke_polyline(
                    canvas,
                    &[(x1, y1), (x2, y2)],
                    false,
                    stroke,
                    paint.stroke_width * sx.min(sy),
                    paint.line_cap,
                    paint.line_join,
                );
            }
            "polyline" | "polygon" => {
                let points = parse_svg_points(attrs.get("points"))
                    .into_iter()
                    .map(|(x, y)| (x * sx + tx, y * sy + ty))
                    .collect::<Vec<_>>();
                let closed = tag == "polygon";
                if closed && points.len() >= 3 {
                    if let Some(fill_color) = fill {
                        fill_compound_path(canvas, std::slice::from_ref(&points), fill_color, FillRule::NonZero);
                    }
                }
                stroke_polyline(canvas, &points, closed, stroke, paint.stroke_width * sx.min(sy), paint.line_cap, paint.line_join);
            }
            "path" => {
                if let Some(d) = attrs.get("d") {
                    let fill_rule = match property_value(&attrs, "fill-rule").as_deref() {
                        Some(value) if value.eq_ignore_ascii_case("evenodd") => FillRule::EvenOdd,
                        _ => FillRule::NonZero,
                    };
                    render_path(canvas, d, sx, sy, tx, ty, fill, fill_rule, stroke, paint.stroke_width * sx.min(sy), paint.line_cap, paint.line_join);
                }
            }
            _ => {
                // Recurse into unknown elements (may contain renderable children)
                render_svg_children(&child, canvas, sx, sy, tx, ty, paint);
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
                canvas.blend_pixel(px, py, color);
            }
        }
    }
}

fn fill_ellipse(canvas: &mut Canvas, cx: f32, cy: f32, rx: f32, ry: f32, color: Color) {
    let x0 = ((cx - rx).floor() as i32).max(0) as u32;
    let y0 = ((cy - ry).floor() as i32).max(0) as u32;
    let x1 = ((cx + rx).ceil() as u32).min(canvas.width());
    let y1 = ((cy + ry).ceil() as u32).min(canvas.height());
    for py in y0..y1 {
        for px in x0..x1 {
            let dx = (px as f32 + 0.5 - cx) / rx;
            let dy = (py as f32 + 0.5 - cy) / ry;
            if dx * dx + dy * dy <= 1.0 {
                canvas.blend_pixel(px, py, color);
            }
        }
    }
}

fn stroke_ellipse(canvas: &mut Canvas, cx: f32, cy: f32, rx: f32, ry: f32, width: f32, color: Color) {
    if width <= 0.0 || color.a == 0 {
        return;
    }
    let outer_rx = rx + width / 2.0;
    let outer_ry = ry + width / 2.0;
    let inner_rx = (rx - width / 2.0).max(0.0);
    let inner_ry = (ry - width / 2.0).max(0.0);
    let x0 = ((cx - outer_rx).floor() as i32).max(0) as u32;
    let y0 = ((cy - outer_ry).floor() as i32).max(0) as u32;
    let x1 = ((cx + outer_rx).ceil() as u32).min(canvas.width());
    let y1 = ((cy + outer_ry).ceil() as u32).min(canvas.height());
    for py in y0..y1 {
        for px in x0..x1 {
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            let outer = (dx / outer_rx).powi(2) + (dy / outer_ry).powi(2) <= 1.0;
            let inner = inner_rx > 0.0 && inner_ry > 0.0
                && (dx / inner_rx).powi(2) + (dy / inner_ry).powi(2) < 1.0;
            if outer && !inner {
                canvas.blend_pixel(px, py, color);
            }
        }
    }
}

fn stroke_polyline(
    canvas: &mut Canvas,
    points: &[(f32, f32)],
    closed: bool,
    color: Option<Color>,
    width: f32,
    line_cap: LineCap,
    line_join: LineJoin,
) {
    let Some(color) = color else { return };
    if points.len() < 2 || width <= 0.0 || color.a == 0 {
        return;
    }
    let half = width / 2.0;
    let segment_count = if closed { points.len() } else { points.len() - 1 };
    for index in 0..segment_count {
        let start = points[index];
        let end = points[(index + 1) % points.len()];
        stroke_segment(canvas, start, end, half, color, if closed || index > 0 { LineCap::Butt } else { line_cap }, if closed || index + 1 < segment_count { LineCap::Butt } else { line_cap });
    }
    let join_count = if closed { points.len() } else { points.len().saturating_sub(2) };
    for index in 0..join_count {
        let (previous, point, next) = if closed {
            (
                points[(index + points.len() - 1) % points.len()],
                points[index],
                points[(index + 1) % points.len()],
            )
        } else {
            (points[index], points[index + 1], points[index + 2])
        };
        match line_join {
            LineJoin::Round => fill_circle(canvas, point.0, point.1, half, color),
            LineJoin::Bevel => stroke_bevel_join(canvas, previous, point, next, half, color),
            LineJoin::Miter => stroke_miter_join(canvas, previous, point, next, half, color),
        }
    }
}

fn stroke_segment(
    canvas: &mut Canvas,
    start: (f32, f32),
    end: (f32, f32),
    half: f32,
    color: Color,
    start_cap: LineCap,
    end_cap: LineCap,
) {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let length = (dx * dx + dy * dy).sqrt();
    if length == 0.0 {
        fill_circle(canvas, start.0, start.1, half, color);
        return;
    }
    let ux = dx / length;
    let uy = dy / length;
    let mut from = start;
    let mut to = end;
    if start_cap == LineCap::Square {
        from = (from.0 - ux * half, from.1 - uy * half);
    }
    if end_cap == LineCap::Square {
        to = (to.0 + ux * half, to.1 + uy * half);
    }
    let nx = -uy * half;
    let ny = ux * half;
    let polygon = vec![
        (from.0 + nx, from.1 + ny),
        (to.0 + nx, to.1 + ny),
        (to.0 - nx, to.1 - ny),
        (from.0 - nx, from.1 - ny),
    ];
    fill_compound_path(canvas, std::slice::from_ref(&polygon), color, FillRule::NonZero);
    if start_cap == LineCap::Round {
        fill_circle(canvas, start.0, start.1, half, color);
    }
    if end_cap == LineCap::Round {
        fill_circle(canvas, end.0, end.1, half, color);
    }
}

fn stroke_bevel_join(
    canvas: &mut Canvas,
    previous: (f32, f32),
    point: (f32, f32),
    next: (f32, f32),
    half: f32,
    color: Color,
) {
    let Some((d1x, d1y)) = unit_vector(previous, point) else { return };
    let Some((d2x, d2y)) = unit_vector(point, next) else { return };
    let cross = d1x * d2y - d1y * d2x;
    if cross.abs() < 1e-5 {
        return;
    }
    let side = if cross > 0.0 { -1.0 } else { 1.0 };
    let p1 = (point.0 - d1y * half * side, point.1 + d1x * half * side);
    let p2 = (point.0 - d2y * half * side, point.1 + d2x * half * side);
    fill_compound_path(canvas, &[vec![point, p1, p2]], color, FillRule::NonZero);
}

fn stroke_miter_join(
    canvas: &mut Canvas,
    previous: (f32, f32),
    point: (f32, f32),
    next: (f32, f32),
    half: f32,
    color: Color,
) {
    let Some((d1x, d1y)) = unit_vector(previous, point) else { return };
    let Some((d2x, d2y)) = unit_vector(point, next) else { return };
    let cross = d1x * d2y - d1y * d2x;
    if cross.abs() < 1e-5 {
        return;
    }
    let side = if cross > 0.0 { -1.0 } else { 1.0 };
    let p1 = (point.0 - d1y * half * side, point.1 + d1x * half * side);
    let p2 = (point.0 - d2y * half * side, point.1 + d2x * half * side);
    let delta = (p2.0 - p1.0, p2.1 - p1.1);
    let distance = (delta.0 * d2y - delta.1 * d2x) / cross;
    let miter = (p1.0 + d1x * distance, p1.1 + d1y * distance);
    let miter_length = ((miter.0 - point.0).powi(2) + (miter.1 - point.1).powi(2)).sqrt();
    if miter_length <= half * 4.0 {
        fill_compound_path(canvas, &[vec![p1, miter, p2]], color, FillRule::NonZero);
    } else {
        stroke_bevel_join(canvas, previous, point, next, half, color);
    }
}

fn unit_vector(start: (f32, f32), end: (f32, f32)) -> Option<(f32, f32)> {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let length = (dx * dx + dy * dy).sqrt();
    (length > 0.0).then_some((dx / length, dy / length))
}

/// Renders an SVG path `d` attribute using common line and curve commands.
fn render_path(
    canvas: &mut Canvas,
    d: &str,
    sx: f32,
    sy: f32,
    tx: f32,
    ty: f32,
    fill: Option<Color>,
    fill_rule: FillRule,
    stroke: Option<Color>,
    stroke_width: f32,
    line_cap: LineCap,
    line_join: LineJoin,
) {
    let commands = parse_path_data(d);
    if commands.is_empty() {
        return;
    }

    // Collect flattened geometry from path commands. Keeping the closed bit
    // lets fills implicitly close an open path while strokes preserve open
    // endpoints and their line caps.
    let mut points = Vec::new();
    let mut subpaths: Vec<(Vec<(f32, f32)>, bool)> = Vec::new();
    let mut closed = false;
    let mut cx = 0.0f32;
    let mut cy = 0.0f32;
    let mut subpath_start = (0.0f32, 0.0f32);

    for cmd in &commands {
        match cmd {
            PathCommand::MoveTo(x, y) => {
                // Flush previous subpath
                if points.len() >= 2 {
                    subpaths.push((std::mem::take(&mut points), closed));
                }
                closed = false;
                cx = *x;
                cy = *y;
                subpath_start = (cx, cy);
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
            PathCommand::CurveTo(cp1x, cp1y, cp2x, cp2y, x, y) => {
                let start_x = cx;
                let start_y = cy;
                // Flatten cubic Bezier curves into short line segments. Twelve
                // segments are enough for the small inline icons and logos this
                // rasterizer handles while keeping path painting inexpensive.
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    let inverse = 1.0 - t;
                    let curve_x = inverse.powi(3) * start_x
                        + 3.0 * inverse.powi(2) * t * cp1x
                        + 3.0 * inverse * t.powi(2) * cp2x
                        + t.powi(3) * x;
                    let curve_y = inverse.powi(3) * start_y
                        + 3.0 * inverse.powi(2) * t * cp1y
                        + 3.0 * inverse * t.powi(2) * cp2y
                        + t.powi(3) * y;
                    points.push((curve_x * sx + tx, curve_y * sy + ty));
                }
                cx = *x;
                cy = *y;
            }
            PathCommand::QuadraticCurveTo(cpx, cpy, x, y) => {
                let start_x = cx;
                let start_y = cy;
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    let inverse = 1.0 - t;
                    let curve_x =
                        inverse.powi(2) * start_x + 2.0 * inverse * t * cpx + t.powi(2) * x;
                    let curve_y =
                        inverse.powi(2) * start_y + 2.0 * inverse * t * cpy + t.powi(2) * y;
                    points.push((curve_x * sx + tx, curve_y * sy + ty));
                }
                cx = *x;
                cy = *y;
            }
            PathCommand::ArcTo(rx, ry, rotation, large_arc, sweep, x, y) => {
                flatten_arc(
                    &mut points,
                    cx,
                    cy,
                    *rx,
                    *ry,
                    *rotation,
                    *large_arc,
                    *sweep,
                    *x,
                    *y,
                    sx,
                    sy,
                    tx,
                    ty,
                );
                cx = *x;
                cy = *y;
            }
            PathCommand::Close => {
                if points.len() >= 2 {
                    subpaths.push((std::mem::take(&mut points), true));
                }
                cx = subpath_start.0;
                cy = subpath_start.1;
                points.push((cx * sx + tx, cy * sy + ty));
                closed = false;
            }
        }
    }

    // Flush remaining subpath
    if points.len() >= 2 {
        subpaths.push((points, closed));
    }
    if let Some(fill) = fill {
        let fill_paths = subpaths
            .iter()
            .filter(|(points, _)| points.len() >= 3)
            .map(|(points, _)| points.clone())
            .collect::<Vec<_>>();
        if !fill_paths.is_empty() {
            fill_compound_path(canvas, &fill_paths, fill, fill_rule);
        }
    }
    for (points, closed) in subpaths {
        stroke_polyline(canvas, &points, closed, stroke, stroke_width, line_cap, line_join);
    }
}

#[allow(clippy::too_many_arguments)]
fn flatten_arc(
    points: &mut Vec<(f32, f32)>,
    x1: f32,
    y1: f32,
    mut rx: f32,
    mut ry: f32,
    rotation: f32,
    large_arc: bool,
    sweep: bool,
    x2: f32,
    y2: f32,
    sx: f32,
    sy: f32,
    tx: f32,
    ty: f32,
) {
    rx = rx.abs();
    ry = ry.abs();
    if rx == 0.0 || ry == 0.0 || (x1 == x2 && y1 == y2) {
        points.push((x2 * sx + tx, y2 * sy + ty));
        return;
    }

    let phi = rotation.to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();
    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;
    let scale = (x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry)).sqrt();
    if scale > 1.0 {
        rx *= scale;
        ry *= scale;
    }

    let rx2 = rx * rx;
    let ry2 = ry * ry;
    let numerator = (rx2 * ry2 - rx2 * y1p * y1p - ry2 * x1p * x1p).max(0.0);
    let denominator = rx2 * y1p * y1p + ry2 * x1p * x1p;
    let sign = if large_arc == sweep { -1.0 } else { 1.0 };
    let factor = if denominator == 0.0 {
        0.0
    } else {
        sign * (numerator / denominator).sqrt()
    };
    let cxp = factor * rx * y1p / ry;
    let cyp = factor * -ry * x1p / rx;
    let cx = cos_phi * cxp - sin_phi * cyp + (x1 + x2) / 2.0;
    let cy = sin_phi * cxp + cos_phi * cyp + (y1 + y2) / 2.0;

    let angle =
        |ux: f32, uy: f32, vx: f32, vy: f32| ux.mul_add(vy, -uy * vx).atan2(ux * vx + uy * vy);
    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let start = angle(1.0, 0.0, ux, uy);
    let mut delta = angle(ux, uy, vx, vy);
    if !sweep && delta > 0.0 {
        delta -= std::f32::consts::TAU;
    }
    if sweep && delta < 0.0 {
        delta += std::f32::consts::TAU;
    }
    let segments = (delta.abs() / (std::f32::consts::PI / 12.0))
        .ceil()
        .max(1.0) as usize;
    for step in 1..=segments {
        let theta = start + delta * step as f32 / segments as f32;
        let (sin_theta, cos_theta) = theta.sin_cos();
        let x = cx + cos_phi * rx * cos_theta - sin_phi * ry * sin_theta;
        let y = cy + sin_phi * rx * cos_theta + cos_phi * ry * sin_theta;
        points.push((x * sx + tx, y * sy + ty));
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
    CurveTo(f32, f32, f32, f32, f32, f32),
    // Control point (cpx, cpy) + endpoint (x, y).
    QuadraticCurveTo(f32, f32, f32, f32),
    // Radii, x-axis rotation, large-arc flag, sweep flag, endpoint.
    ArcTo(f32, f32, f32, bool, bool, f32, f32),
    Close,
}

fn parse_path_data(d: &str) -> Vec<PathCommand> {
    let mut commands = Vec::new();
    let mut chars = d.chars().peekable();
    let mut current_command = ' ';
    let mut cx = 0.0f32;
    let mut cy = 0.0f32;
    let mut subpath_start_x = 0.0f32;
    let mut subpath_start_y = 0.0f32;
    let mut last_cubic_control = None;
    let mut last_quadratic_control = None;

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
                last_cubic_control = None;
                last_quadratic_control = None;
                if let Some(x) = parse_number(&mut chars) {
                    skip_whitespace_and_commas(&mut chars);
                    if let Some(y) = parse_number(&mut chars) {
                        cx = x;
                        cy = y;
                        subpath_start_x = x;
                        subpath_start_y = y;
                        commands.push(PathCommand::MoveTo(x, y));
                        current_command = 'L'; // subsequent coords are LineTo
                    }
                }
            }
            'm' => {
                last_cubic_control = None;
                last_quadratic_control = None;
                if let Some(dx) = parse_number(&mut chars) {
                    skip_whitespace_and_commas(&mut chars);
                    if let Some(dy) = parse_number(&mut chars) {
                        cx += dx;
                        cy += dy;
                        subpath_start_x = cx;
                        subpath_start_y = cy;
                        commands.push(PathCommand::MoveTo(cx, cy));
                        current_command = 'l';
                    }
                }
            }
            'L' => {
                last_cubic_control = None;
                last_quadratic_control = None;
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
                last_cubic_control = None;
                last_quadratic_control = None;
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
                last_cubic_control = None;
                last_quadratic_control = None;
                if let Some(x) = parse_number(&mut chars) {
                    cx = x;
                    commands.push(PathCommand::HorizontalLineTo(x));
                }
            }
            'h' => {
                last_cubic_control = None;
                last_quadratic_control = None;
                if let Some(dx) = parse_number(&mut chars) {
                    cx += dx;
                    commands.push(PathCommand::HorizontalLineTo(cx));
                }
            }
            'V' => {
                last_cubic_control = None;
                last_quadratic_control = None;
                if let Some(y) = parse_number(&mut chars) {
                    cy = y;
                    commands.push(PathCommand::VerticalLineTo(y));
                }
            }
            'v' => {
                last_cubic_control = None;
                last_quadratic_control = None;
                if let Some(dy) = parse_number(&mut chars) {
                    cy += dy;
                    commands.push(PathCommand::VerticalLineTo(cy));
                }
            }
            'C' => {
                last_quadratic_control = None;
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
                    last_cubic_control = Some((nums[2], nums[3]));
                    commands.push(PathCommand::CurveTo(
                        nums[0], nums[1], nums[2], nums[3], nums[4], nums[5],
                    ));
                }
            }
            'c' => {
                last_quadratic_control = None;
                let mut nums = Vec::new();
                for _ in 0..6 {
                    skip_whitespace_and_commas(&mut chars);
                    if let Some(n) = parse_number(&mut chars) {
                        nums.push(n);
                    }
                }
                if nums.len() == 6 {
                    let start_x = cx;
                    let start_y = cy;
                    cx += nums[4];
                    cy += nums[5];
                    last_cubic_control = Some((start_x + nums[2], start_y + nums[3]));
                    commands.push(PathCommand::CurveTo(
                        start_x + nums[0],
                        start_y + nums[1],
                        start_x + nums[2],
                        start_y + nums[3],
                        cx,
                        cy,
                    ));
                }
            }
            'S' | 's' => {
                last_quadratic_control = None;
                let mut nums = Vec::new();
                for _ in 0..4 {
                    skip_whitespace_and_commas(&mut chars);
                    if let Some(n) = parse_number(&mut chars) {
                        nums.push(n);
                    }
                }
                if nums.len() == 4 {
                    let cp1 = last_cubic_control
                        .map(|(x, y)| (2.0 * cx - x, 2.0 * cy - y))
                        .unwrap_or((cx, cy));
                    let (cp2x, cp2y, x, y) = if current_command == 's' {
                        (cx + nums[0], cy + nums[1], cx + nums[2], cy + nums[3])
                    } else {
                        (nums[0], nums[1], nums[2], nums[3])
                    };
                    commands.push(PathCommand::CurveTo(cp1.0, cp1.1, cp2x, cp2y, x, y));
                    cx = x;
                    cy = y;
                    last_cubic_control = Some((cp2x, cp2y));
                }
            }
            'Q' | 'q' => {
                last_cubic_control = None;
                let mut nums = Vec::new();
                for _ in 0..4 {
                    skip_whitespace_and_commas(&mut chars);
                    if let Some(n) = parse_number(&mut chars) {
                        nums.push(n);
                    }
                }
                if nums.len() == 4 {
                    let (cpx, cpy, x, y) = if current_command == 'q' {
                        (cx + nums[0], cy + nums[1], cx + nums[2], cy + nums[3])
                    } else {
                        (nums[0], nums[1], nums[2], nums[3])
                    };
                    commands.push(PathCommand::QuadraticCurveTo(cpx, cpy, x, y));
                    cx = x;
                    cy = y;
                    last_quadratic_control = Some((cpx, cpy));
                }
            }
            'T' | 't' => {
                last_cubic_control = None;
                let mut nums = Vec::new();
                for _ in 0..2 {
                    skip_whitespace_and_commas(&mut chars);
                    if let Some(n) = parse_number(&mut chars) {
                        nums.push(n);
                    }
                }
                if nums.len() == 2 {
                    let (cpx, cpy) = last_quadratic_control
                        .map(|(x, y)| (2.0 * cx - x, 2.0 * cy - y))
                        .unwrap_or((cx, cy));
                    let (x, y) = if current_command == 't' {
                        (cx + nums[0], cy + nums[1])
                    } else {
                        (nums[0], nums[1])
                    };
                    commands.push(PathCommand::QuadraticCurveTo(cpx, cpy, x, y));
                    cx = x;
                    cy = y;
                    last_quadratic_control = Some((cpx, cpy));
                }
            }
            'A' | 'a' => {
                last_cubic_control = None;
                last_quadratic_control = None;
                let mut nums = Vec::new();
                for _ in 0..7 {
                    skip_whitespace_and_commas(&mut chars);
                    if let Some(n) = parse_number(&mut chars) {
                        nums.push(n);
                    }
                }
                if nums.len() == 7 {
                    let (x, y) = if current_command == 'a' {
                        (cx + nums[5], cy + nums[6])
                    } else {
                        (nums[5], nums[6])
                    };
                    commands.push(PathCommand::ArcTo(
                        nums[0],
                        nums[1],
                        nums[2],
                        nums[3] != 0.0,
                        nums[4] != 0.0,
                        x,
                        y,
                    ));
                    cx = x;
                    cy = y;
                }
            }
            'Z' | 'z' => {
                last_cubic_control = None;
                last_quadratic_control = None;
                commands.push(PathCommand::Close);
                cx = subpath_start_x;
                cy = subpath_start_y;
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
                    canvas.blend_pixel(x, y, color);
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

fn parse_svg_nonnegative(value: Option<&String>) -> Option<f32> {
    let s = value?.trim();
    s.strip_suffix("px")
        .unwrap_or(s)
        .parse::<f32>()
        .ok()
        .filter(|v| *v >= 0.0)
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
    fn renders_root_current_color_with_supplied_css_color() {
        let html = r#"<svg width="10" height="10" fill="currentColor"><rect width="10" height="10"/></svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let svg = find_svg(&doc).unwrap();
        let image =
            render_svg_to_image_with_current_color(&svg, Color::rgb(255, 255, 255)).unwrap();
        let center = (5 * 10 + 5) * 4;
        assert_eq!(&image.pixels()[center..center + 4], &[255, 255, 255, 255]);
    }

    #[test]
    fn svg_without_fill_keeps_black_initial_fill() {
        let html = r#"<svg width="10" height="10"><rect width="10" height="10"/></svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let svg = find_svg(&doc).unwrap();
        let image =
            render_svg_to_image_with_current_color(&svg, Color::rgb(255, 255, 255)).unwrap();
        let center = (5 * 10 + 5) * 4;
        assert_eq!(&image.pixels()[center..center + 4], &[0, 0, 0, 255]);
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
    fn renders_svg_ellipse_line_polyline_and_polygon_with_strokes() {
        let html = r#"<svg width="40" height="24">
          <ellipse cx="8" cy="8" rx="6" ry="4" fill="red" stroke="black" stroke-width="2"/>
          <line x1="16" y1="2" x2="16" y2="14" stroke="blue" stroke-width="2" stroke-linecap="round"/>
          <polyline points="20,2 25,8 20,14" fill="none" stroke="green" stroke-width="2"/>
          <polygon points="28,2 38,2 34,14 28,14" fill="yellow" stroke="black" stroke-width="1"/>
        </svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let image = render_svg_to_image(&find_svg(&doc).unwrap()).unwrap();
        let pixel = |x: u32, y: u32| {
            let index = (y * image.width() + x) as usize * 4;
            Color::rgba(
                image.pixels()[index],
                image.pixels()[index + 1],
                image.pixels()[index + 2],
                image.pixels()[index + 3],
            )
        };
        assert_eq!(pixel(8, 8), Color::rgb(255, 0, 0));
        assert_eq!(pixel(16, 8).b, 255);
        assert!(pixel(25, 8).g > 0);
        assert_eq!(pixel(32, 8), Color::rgb(255, 255, 0));
        assert!(pixel(8, 3).a > 0, "ellipse stroke should be painted");
    }

    #[test]
    fn svg_group_inherits_paint_and_applies_opacity() {
        let html = r#"<svg width="20" height="10" color="white">
          <g fill="currentColor" opacity="0.5"><rect x="0" y="0" width="10" height="10"/></g>
          <g fill="none" stroke="blue" stroke-width="2" stroke-linejoin="round">
            <path d="M12 2L18 2L18 8"/>
          </g>
        </svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let image = render_svg_to_image(&find_svg(&doc).unwrap()).unwrap();
        let pixel = |x: u32, y: u32| {
            let index = (y * image.width() + x) as usize * 4;
            Color::rgba(
                image.pixels()[index],
                image.pixels()[index + 1],
                image.pixels()[index + 2],
                image.pixels()[index + 3],
            )
        };
        let filled = pixel(5, 5);
        assert_eq!((filled.r, filled.g, filled.b), (255, 255, 255));
        assert!(filled.a >= 127 && filled.a <= 128, "opacity should halve alpha: {filled:?}");
        assert!(pixel(15, 2).b > 0, "inherited stroke should be painted");
    }

    #[test]
    fn svg_style_uses_last_declaration_and_close_resets_path_stroke_point() {
        let html = r#"<svg width="20" height="10">
          <rect width="8" height="8" style="fill:red; fill:blue"/>
          <path fill="none" stroke="black" stroke-width="1" d="M2 2H6V6ZH10"/>
        </svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let svg = find_svg(&doc).unwrap();
        let image = render_svg_to_image(&svg).unwrap();
        let pixel = |x: u32, y: u32| {
            let index = (y * image.width() + x) as usize * 4;
            Color::rgba(
                image.pixels()[index],
                image.pixels()[index + 1],
                image.pixels()[index + 2],
                image.pixels()[index + 3],
            )
        };
        assert_eq!(pixel(4, 0), Color::rgb(0, 0, 255));
        assert!(pixel(9, 1).a > 0, "commands after close should start at the subpath origin");
        assert_eq!(pixel(9, 6).a, 0, "the closed path must not continue from its old endpoint");
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
    fn close_path_restores_current_point_for_relative_commands() {
        let cmds = parse_path_data("M10 10L20 10Zm5 0");
        assert!(matches!(cmds.last(), Some(PathCommand::MoveTo(15.0, 10.0))));
    }

    #[test]
    fn smooth_cubic_curve_reflects_the_previous_control_point() {
        let cmds = parse_path_data("M0 0C0 5 5 10 10 10S20 5 20 0");
        assert!(matches!(
            cmds.last(),
            Some(PathCommand::CurveTo(15.0, 10.0, 20.0, 5.0, 20.0, 0.0))
        ));
    }

    #[test]
    fn relative_smooth_cubic_curve_uses_current_point() {
        let cmds = parse_path_data("M10 10s5 5 10 0");
        assert!(matches!(
            cmds.last(),
            Some(PathCommand::CurveTo(10.0, 10.0, 15.0, 15.0, 20.0, 10.0))
        ));
    }

    #[test]
    fn smooth_quadratic_curve_reflects_the_previous_control_point() {
        let cmds = parse_path_data("M0 0Q5 10 10 0T20 0");
        assert!(matches!(
            cmds.last(),
            Some(PathCommand::QuadraticCurveTo(15.0, -10.0, 20.0, 0.0))
        ));
    }

    #[test]
    fn absolute_and_relative_arc_commands_update_the_endpoint() {
        let cmds = parse_path_data("M1 2A3 4 15 0 1 8 9a3 4 0 1 0 2-3");
        assert!(matches!(
            cmds.get(1),
            Some(PathCommand::ArcTo(3.0, 4.0, 15.0, false, true, 8.0, 9.0))
        ));
        assert!(matches!(
            cmds.get(2),
            Some(PathCommand::ArcTo(3.0, 4.0, 0.0, true, false, 10.0, 6.0))
        ));
    }

    #[test]
    fn elliptical_arc_paths_are_flattened_before_filling() {
        let html = r#"<svg width="20" height="20"><path fill="black" d="M2 10A8 8 0 0 1 18 10A8 8 0 0 1 2 10Z"/></svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let image = render_svg_to_image(&find_svg(&doc).unwrap()).unwrap();

        assert_eq!(image.pixels()[(10 * 20 + 10) * 4 + 3], 255);
        assert_eq!(image.pixels()[(0 * 20) * 4 + 3], 0);
    }

    #[test]
    fn quadratic_bezier_paths_are_flattened_before_filling() {
        let html = r#"<svg width="10" height="10"><path fill="black" d="M1 8Q5 0 9 8Z"/></svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let image = render_svg_to_image(&find_svg(&doc).unwrap()).unwrap();

        assert_eq!(image.pixels()[(6 * 10 + 5) * 4 + 3], 255);
        assert_eq!(image.pixels()[(0 * 10 + 5) * 4 + 3], 0);
    }

    #[test]
    fn cubic_bezier_paths_are_flattened_before_filling() {
        let html = r#"<svg width="10" height="10"><path fill="black" d="M1 5C1 1 9 1 9 5C9 9 1 9 1 5Z"/></svg>"#;
        let doc = TreeBuilder::parse(html).document();
        let image = render_svg_to_image(&find_svg(&doc).unwrap()).unwrap();

        assert_eq!(image.pixels()[(5 * 10 + 5) * 4 + 3], 255);
        assert_eq!(image.pixels()[(0 * 10) * 4 + 3], 0);
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
