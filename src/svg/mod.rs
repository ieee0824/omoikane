//! Basic SVG rendering: rasterizes inline SVG elements to an RGBA image.

use std::collections::{BTreeMap, HashSet};

use crate::dom::{Node, NodeHandle, NodeType};
use crate::layout::Rect;
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

    let resources = SvgResources::collect(svg_node, if vb_w > 0.0 { vb_w } else { width }, if vb_h > 0.0 { vb_h } else { height });

    let initial_paint = SvgPaint {
        fill: Some(SvgPaintValue::Solid(Color::rgb(0, 0, 0))),
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
    let mut visited = HashSet::new();
    render_svg_children(
        svg_node,
        &mut canvas,
        sx,
        sy,
        tx,
        ty,
        root_paint,
        &resources,
        &mut visited,
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

/// Returns the topmost SVG geometry node under a point in the displayed SVG
/// viewport.  Inline SVGs are rasterized as one replaced layout box, so the
/// regular CSS hit tester cannot see their child shapes.  This deterministic
/// geometry pass mirrors the small shape subset used by the rasterizer and
/// keeps `pointer-events` decisions at the DOM target level.
pub(crate) fn hit_test_svg(
    svg_node: &NodeHandle,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    computed_pointer_events: &mut dyn FnMut(&NodeHandle) -> Option<String>,
) -> Option<NodeHandle> {
    if svg_node.tag_name().as_deref() != Some("svg")
        || !x.is_finite()
        || !y.is_finite()
        || width <= 0.0
        || height <= 0.0
    {
        return None;
    }
    let attrs = svg_node.attributes().unwrap_or_default();
    let (vb_x, vb_y, vb_w, vb_h) = parse_viewbox(attrs.get("viewBox").or(attrs.get("viewbox")));
    let (point, scale_x, scale_y) = if vb_w > 0.0 && vb_h > 0.0 {
        (
            (vb_x + x * vb_w / width, vb_y + y * vb_h / height),
            width / vb_w,
            height / vb_h,
        )
    } else {
        ((x, y), 1.0, 1.0)
    };
    let root_paint = SvgPaint {
        fill: Some(SvgPaintValue::Solid(Color::rgb(0, 0, 0))),
        stroke: None,
        stroke_width: 1.0,
        line_cap: LineCap::Butt,
        line_join: LineJoin::Miter,
        opacity: 1.0,
        fill_opacity: 1.0,
        stroke_opacity: 1.0,
        current_color: Color::rgb(0, 0, 0),
    };
    let initial = SvgHitStyle {
        paint: resolve_paint(&root_paint, &attrs),
        pointer_events: computed_pointer_events(svg_node)
            .or_else(|| property_value(&attrs, "pointer-events"))
            .unwrap_or_else(|| "visiblepainted".to_string())
            .to_ascii_lowercase(),
        visible: !matches!(
            property_value(&attrs, "visibility").as_deref(),
            Some(value) if value.eq_ignore_ascii_case("hidden") || value.eq_ignore_ascii_case("collapse")
        ),
        displayed: !matches!(
            property_value(&attrs, "display").as_deref(),
            Some(value) if value.eq_ignore_ascii_case("none")
        ),
    };
    let mut visited_uses = HashSet::new();
    hit_test_svg_children(
        svg_node,
        svg_node,
        point,
        scale_x,
        scale_y,
        &initial,
        computed_pointer_events,
        &mut visited_uses,
    )
}

#[derive(Clone)]
struct SvgHitStyle {
    paint: SvgPaint,
    pointer_events: String,
    visible: bool,
    displayed: bool,
}

fn hit_test_svg_children(
    root: &NodeHandle,
    parent: &NodeHandle,
    point: (f32, f32),
    scale_x: f32,
    scale_y: f32,
    inherited: &SvgHitStyle,
    computed_pointer_events: &mut dyn FnMut(&NodeHandle) -> Option<String>,
    visited_uses: &mut HashSet<usize>,
) -> Option<NodeHandle> {
    let children = parent.child_nodes();
    for child in children.iter().rev() {
        if child.node_type() != NodeType::Element {
            continue;
        }
        let tag = child.tag_name().unwrap_or_default().to_ascii_lowercase();
        if tag == "defs" {
            continue;
        }
        let attrs = child.attributes().unwrap_or_default();
        // The rasterizer currently ignores SVG `transform` attributes; keep
        // hit testing in the same untransformed coordinate system until paint
        // gains matching transform support.
        let local_point = point;
        let child_scale_x = 1.0;
        let child_scale_y = 1.0;
        let paint = resolve_paint(&inherited.paint, &attrs);
        let pointer_events = computed_pointer_events(child)
            .or_else(|| property_value(&attrs, "pointer-events"))
            .unwrap_or_else(|| inherited.pointer_events.clone())
            .to_ascii_lowercase();
        let visible = inherited.visible
            && !matches!(
                property_value(&attrs, "visibility").as_deref(),
                Some(value) if value.eq_ignore_ascii_case("hidden") || value.eq_ignore_ascii_case("collapse")
            );
        let displayed = inherited.displayed
            && !matches!(
                property_value(&attrs, "display").as_deref(),
                Some(value) if value.eq_ignore_ascii_case("none")
            );
        if !displayed {
            continue;
        }
        let style = SvgHitStyle {
            paint,
            pointer_events,
            visible,
            displayed,
        };
        let next_scale_x = scale_x * child_scale_x;
        let next_scale_y = scale_y * child_scale_y;
        if tag == "use" {
            let inserted = visited_uses.insert(child.identity());
            if inserted {
                let hit = hit_test_svg_use(
                    root,
                    child,
                    local_point,
                    next_scale_x,
                    next_scale_y,
                    &style,
                    computed_pointer_events,
                    visited_uses,
                );
                visited_uses.remove(&child.identity());
                if hit {
                    return Some(child.clone());
                }
            }
        }
        if let Some(target) = hit_test_svg_children(
            root,
            child,
            local_point,
            next_scale_x,
            next_scale_y,
            &style,
            computed_pointer_events,
            visited_uses,
        ) {
            return Some(target);
        }
        if pointer_events.eq_ignore_ascii_case("none") {
            continue;
        }
        let geometry = svg_hit_geometry(
            &tag,
            &attrs,
            local_point,
            style.paint.stroke_width,
            next_scale_x,
            next_scale_y,
        );
        if geometry.is_empty() {
            continue;
        }
        if pointer_events_accepts(
            &style.pointer_events,
            &style.paint,
            style.visible,
            geometry,
        ) {
            return Some(child.clone());
        }
    }
    None
}

fn hit_test_svg_use(
    root: &NodeHandle,
    use_node: &NodeHandle,
    point: (f32, f32),
    scale_x: f32,
    scale_y: f32,
    inherited: &SvgHitStyle,
    computed_pointer_events: &mut dyn FnMut(&NodeHandle) -> Option<String>,
    visited_uses: &mut HashSet<usize>,
) -> bool {
    let attrs = use_node.attributes().unwrap_or_default();
    let Some(id) = attribute_value(&attrs, "href")
        .or_else(|| attribute_value(&attrs, "xlink:href"))
        .and_then(|href| parse_fragment_reference(&href))
    else {
        return false;
    };
    let Some(target) = find_svg_resource(root, &id) else {
        return false;
    };
    let target_attrs = target.attributes().unwrap_or_default();
    let target_tag = target
        .tag_name()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let x = parse_svg_coord(attribute_ref(&attrs, "x")).unwrap_or(0.0);
    let y = parse_svg_coord(attribute_ref(&attrs, "y")).unwrap_or(0.0);
    let local_point = (point.0 - x, point.1 - y);
    // The `<use>` instance controls the hit-test mode for the referenced
    // geometry.  A referenced node normally computes to the initial `auto`,
    // which must not erase an explicit mode such as `fill`/`none` on the
    // instance itself.  If the instance is `auto`, honor a non-default mode
    // explicitly authored on the referenced resource.
    let pointer_events = if !inherited.pointer_events.eq_ignore_ascii_case("auto") {
        inherited.pointer_events.clone()
    } else {
        computed_pointer_events(&target)
            .or_else(|| property_value(&target_attrs, "pointer-events"))
            .map(|value| value.to_ascii_lowercase())
            .filter(|value| value != "auto")
            .unwrap_or_else(|| inherited.pointer_events.clone())
    };
    let visible = inherited.visible
        && !matches!(
            property_value(&target_attrs, "visibility").as_deref(),
            Some(value)
                if value.eq_ignore_ascii_case("hidden") || value.eq_ignore_ascii_case("collapse")
        );
    let displayed = inherited.displayed
        && !matches!(
            property_value(&target_attrs, "display").as_deref(),
            Some(value) if value.eq_ignore_ascii_case("none")
        );
    if !displayed {
        return false;
    }
    let style = SvgHitStyle {
        paint: resolve_paint(&inherited.paint, &target_attrs),
        pointer_events,
        visible,
        displayed,
    };
    if hit_test_svg_children(
        root,
        &target,
        local_point,
        scale_x,
        scale_y,
        &style,
        computed_pointer_events,
        visited_uses,
    )
    .is_some()
    {
        return true;
    }
    if style.pointer_events.eq_ignore_ascii_case("none") {
        return false;
    }
    let geometry = svg_hit_geometry(
        &target_tag,
        &target_attrs,
        local_point,
        style.paint.stroke_width,
        scale_x,
        scale_y,
    );
    !geometry.is_empty()
        && pointer_events_accepts(
            &style.pointer_events,
            &style.paint,
            style.visible,
            geometry,
        )
}

fn find_svg_resource(root: &NodeHandle, id: &str) -> Option<NodeHandle> {
    if root
        .attributes()
        .and_then(|attrs| attribute_value(&attrs, "id"))
        .is_some_and(|value| value == id)
    {
        return Some(root.clone());
    }
    for child in root.child_nodes() {
        if let Some(found) = find_svg_resource(&child, id) {
            return Some(found);
        }
    }
    None
}

#[derive(Clone, Copy, Default)]
struct SvgHitGeometry {
    fill: bool,
    stroke: bool,
    bounding_box: bool,
}

impl SvgHitGeometry {
    fn is_empty(self) -> bool {
        !self.fill && !self.stroke && !self.bounding_box
    }
}

fn pointer_events_accepts(
    value: &str,
    paint: &SvgPaint,
    visible: bool,
    geometry: SvgHitGeometry,
) -> bool {
    let fill_painted = paint.fill.is_some() && paint.opacity * paint.fill_opacity > 0.0;
    let stroke_painted = paint.stroke.is_some()
        && paint.stroke_width.is_finite()
        && paint.stroke_width > 0.0
        && paint.opacity * paint.stroke_opacity > 0.0;
    match value {
        "none" => false,
        "fill" => geometry.fill,
        "stroke" => geometry.stroke,
        "painted" => (fill_painted && geometry.fill) || (stroke_painted && geometry.stroke),
        "visiblefill" => visible && geometry.fill,
        "visiblestroke" => visible && geometry.stroke,
        "visible" => visible && (geometry.fill || geometry.stroke),
        "bounding-box" => geometry.bounding_box,
        "all" => geometry.fill || geometry.stroke || geometry.bounding_box,
        "auto" | "visiblepainted" | _ => {
            visible && ((fill_painted && geometry.fill) || (stroke_painted && geometry.stroke))
        }
    }
}

fn svg_hit_geometry(
    tag: &str,
    attrs: &BTreeMap<String, String>,
    point: (f32, f32),
    stroke_width: f32,
    scale_x: f32,
    scale_y: f32,
) -> SvgHitGeometry {
    if !scale_x.is_finite() || !scale_y.is_finite() || scale_x <= 0.0 || scale_y <= 0.0 {
        return SvgHitGeometry::default();
    }
    // Geometry is authored in the SVG user coordinate system while the
    // rasterizer paints it after applying the viewBox-to-viewport scale.  Do
    // the same conversion here so non-uniform viewBox scaling and the painted
    // stroke width agree with hit testing.
    let point = (point.0 * scale_x, point.1 * scale_y);
    let stroke_width = if stroke_width.is_finite() && stroke_width > 0.0 {
        stroke_width * scale_x.min(scale_y)
    } else {
        0.0
    };
    match tag {
        "rect" => {
            let x = parse_svg_coord(attribute_ref(attrs, "x")).unwrap_or(0.0) * scale_x;
            let y = parse_svg_coord(attribute_ref(attrs, "y")).unwrap_or(0.0) * scale_y;
            let width = parse_svg_size(attribute_ref(attrs, "width"))
                .unwrap_or(0.0)
                .max(0.0)
                * scale_x;
            let height = parse_svg_size(attribute_ref(attrs, "height"))
                .unwrap_or(0.0)
                .max(0.0)
                * scale_y;
            if width <= 0.0 || height <= 0.0 {
                return SvgHitGeometry::default();
            }
            let fill = point_in_rect(point, Rect { x, y, width, height });
            let stroke = stroke_width > 0.0
                && point_near_rect(point, Rect { x, y, width, height }, stroke_width / 2.0);
            SvgHitGeometry {
                fill,
                stroke,
                bounding_box: point_in_rect(point, Rect { x, y, width, height }),
            }
        }
        "circle" => {
            let cx = parse_svg_coord(attribute_ref(attrs, "cx")).unwrap_or(0.0) * scale_x;
            let cy = parse_svg_coord(attribute_ref(attrs, "cy")).unwrap_or(0.0) * scale_y;
            let radius = parse_svg_size(attribute_ref(attrs, "r"))
                .unwrap_or(0.0)
                .abs()
                * scale_x.min(scale_y);
            let distance = ((point.0 - cx).powi(2) + (point.1 - cy).powi(2)).sqrt();
            SvgHitGeometry {
                fill: radius > 0.0 && distance <= radius,
                stroke: radius > 0.0
                    && stroke_width > 0.0
                    && (distance - radius).abs() <= stroke_width / 2.0,
                bounding_box: radius > 0.0
                    && point_in_rect(
                        point,
                        Rect {
                            x: cx - radius,
                            y: cy - radius,
                            width: radius * 2.0,
                            height: radius * 2.0,
                        },
                    ),
            }
        }
        "ellipse" => {
            let cx = parse_svg_coord(attribute_ref(attrs, "cx")).unwrap_or(0.0) * scale_x;
            let cy = parse_svg_coord(attribute_ref(attrs, "cy")).unwrap_or(0.0) * scale_y;
            let rx = parse_svg_size(attribute_ref(attrs, "rx"))
                .unwrap_or(0.0)
                .abs()
                * scale_x;
            let ry = parse_svg_size(attribute_ref(attrs, "ry"))
                .unwrap_or(0.0)
                .abs()
                * scale_y;
            if rx <= 0.0 || ry <= 0.0 {
                return SvgHitGeometry::default();
            }
            let normalized = ((point.0 - cx) / rx).powi(2) + ((point.1 - cy) / ry).powi(2);
            let outer_rx = rx + stroke_width / 2.0;
            let outer_ry = ry + stroke_width / 2.0;
            let inner_rx = (rx - stroke_width / 2.0).max(0.0);
            let inner_ry = (ry - stroke_width / 2.0).max(0.0);
            let outer = stroke_width > 0.0
                && ((point.0 - cx) / outer_rx).powi(2)
                    + ((point.1 - cy) / outer_ry).powi(2)
                    <= 1.0;
            let inner = inner_rx > 0.0
                && inner_ry > 0.0
                && ((point.0 - cx) / inner_rx).powi(2)
                    + ((point.1 - cy) / inner_ry).powi(2)
                    < 1.0;
            SvgHitGeometry {
                fill: normalized <= 1.0,
                stroke: outer && !inner,
                bounding_box: point_in_rect(
                    point,
                    Rect {
                        x: cx - rx,
                        y: cy - ry,
                        width: rx * 2.0,
                        height: ry * 2.0,
                    },
                ),
            }
        }
        "line" => {
            let start = (
                parse_svg_coord(attribute_ref(attrs, "x1")).unwrap_or(0.0) * scale_x,
                parse_svg_coord(attribute_ref(attrs, "y1")).unwrap_or(0.0) * scale_y,
            );
            let end = (
                parse_svg_coord(attribute_ref(attrs, "x2")).unwrap_or(0.0) * scale_x,
                parse_svg_coord(attribute_ref(attrs, "y2")).unwrap_or(0.0) * scale_y,
            );
            let distance = point_segment_distance(point, start, end);
            let hit = stroke_width > 0.0 && distance <= stroke_width / 2.0;
            let bbox = rect_for_points(&[start, end]);
            let bounding_box = point_in_rect(point, bbox);
            SvgHitGeometry { fill: false, stroke: hit, bounding_box }
        }
        "polyline" | "polygon" => {
            let points = parse_svg_points(attribute_ref(attrs, "points"))
                .into_iter()
                .map(|(x, y)| (x * scale_x, y * scale_y))
                .collect::<Vec<_>>();
            polygon_hit_geometry(&points, tag == "polygon", point, stroke_width)
        }
        "path" => {
            let subpaths = parse_path_subpaths(attribute_ref(attrs, "d"));
            let mut result = SvgHitGeometry::default();
            for (mut points, closed) in subpaths {
                for point in &mut points {
                    point.0 *= scale_x;
                    point.1 *= scale_y;
                }
                let geometry = polygon_hit_geometry(&points, closed, point, stroke_width);
                result.fill |= geometry.fill;
                result.stroke |= geometry.stroke;
                result.bounding_box |= geometry.bounding_box;
            }
            result
        }
        _ => SvgHitGeometry::default(),
    }
}

fn polygon_hit_geometry(
    points: &[(f32, f32)],
    closed: bool,
    point: (f32, f32),
    stroke_width: f32,
) -> SvgHitGeometry {
    if points.len() < 2 {
        return SvgHitGeometry::default();
    }
    let mut stroke = false;
    if stroke_width > 0.0 {
        for window in points.windows(2) {
            if point_segment_distance(point, window[0], window[1]) <= stroke_width / 2.0 {
                stroke = true;
                break;
            }
        }
        if closed && !stroke {
            stroke = point_segment_distance(point, *points.last().unwrap(), points[0])
                <= stroke_width / 2.0;
        }
    }
    let fill = closed && point_in_polygon(point, points);
    let bounding_box = point_in_rect(point, rect_for_points(points));
    SvgHitGeometry { fill, stroke, bounding_box }
}

fn parse_path_subpaths(value: Option<&String>) -> Vec<(Vec<(f32, f32)>, bool)> {
    let mut points = Vec::new();
    let mut subpaths = Vec::new();
    let mut closed = false;
    let mut current = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    for command in parse_path_data(value.map(String::as_str).unwrap_or_default()) {
        match command {
            PathCommand::MoveTo(x, y) => {
                if !points.is_empty() {
                    if points.len() >= 2 {
                        subpaths.push((std::mem::take(&mut points), closed));
                    } else {
                        // A close command leaves only the subpath's start
                        // point. It is not a segment to carry into the next
                        // moveto.
                        points.clear();
                    }
                }
                closed = false;
                current = (x, y);
                start = current;
                points.push(current);
            }
            PathCommand::LineTo(x, y) => {
                current = (x, y);
                points.push(current);
            }
            PathCommand::HorizontalLineTo(x) => {
                current = (x, current.1);
                points.push(current);
            }
            PathCommand::VerticalLineTo(y) => {
                current.1 = y;
                points.push(current);
            }
            PathCommand::CurveTo(cp1x, cp1y, cp2x, cp2y, x, y) => {
                let start_point = current;
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    let inverse = 1.0 - t;
                    points.push((
                        inverse.powi(3) * start_point.0
                            + 3.0 * inverse.powi(2) * t * cp1x
                            + 3.0 * inverse * t.powi(2) * cp2x
                            + t.powi(3) * x,
                        inverse.powi(3) * start_point.1
                            + 3.0 * inverse.powi(2) * t * cp1y
                            + 3.0 * inverse * t.powi(2) * cp2y
                            + t.powi(3) * y,
                    ));
                }
                current = (x, y);
            }
            PathCommand::QuadraticCurveTo(cpx, cpy, x, y) => {
                let start_point = current;
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    let inverse = 1.0 - t;
                    points.push((
                        inverse.powi(2) * start_point.0
                            + 2.0 * inverse * t * cpx
                            + t.powi(2) * x,
                        inverse.powi(2) * start_point.1
                            + 2.0 * inverse * t * cpy
                            + t.powi(2) * y,
                    ));
                }
                current = (x, y);
            }
            PathCommand::ArcTo(rx, ry, rotation, large_arc, sweep, x, y) => {
                flatten_arc(
                    &mut points,
                    current.0,
                    current.1,
                    rx,
                    ry,
                    rotation,
                    large_arc,
                    sweep,
                    x,
                    y,
                    1.0,
                    1.0,
                    0.0,
                    0.0,
                );
                current = (x, y);
            }
            PathCommand::Close => {
                if points.len() >= 2 {
                    subpaths.push((std::mem::take(&mut points), true));
                }
                current = start;
                points.push(current);
                closed = false;
            }
        }
    }
    if points.len() >= 2 {
        subpaths.push((points, closed));
    }
    subpaths
}

fn point_in_rect(point: (f32, f32), rect: Rect) -> bool {
    point.0 >= rect.x
        && point.0 <= rect.x + rect.width
        && point.1 >= rect.y
        && point.1 <= rect.y + rect.height
}

fn point_near_rect(point: (f32, f32), rect: Rect, half_width: f32) -> bool {
    let expanded = Rect {
        x: rect.x - half_width,
        y: rect.y - half_width,
        width: rect.width + half_width * 2.0,
        height: rect.height + half_width * 2.0,
    };
    point_in_rect(point, expanded)
        && (point.0 <= rect.x + half_width
            || point.0 >= rect.x + rect.width - half_width
            || point.1 <= rect.y + half_width
            || point.1 >= rect.y + rect.height - half_width)
}

fn point_segment_distance(point: (f32, f32), start: (f32, f32), end: (f32, f32)) -> f32 {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let length_sq = dx * dx + dy * dy;
    if length_sq <= f32::EPSILON {
        return ((point.0 - start.0).powi(2) + (point.1 - start.1).powi(2)).sqrt();
    }
    let t = (((point.0 - start.0) * dx + (point.1 - start.1) * dy) / length_sq).clamp(0.0, 1.0);
    let projection = (start.0 + t * dx, start.1 + t * dy);
    ((point.0 - projection.0).powi(2) + (point.1 - projection.1).powi(2)).sqrt()
}

fn point_in_polygon(point: (f32, f32), points: &[(f32, f32)]) -> bool {
    let mut inside = false;
    for index in 0..points.len() {
        let previous = if index == 0 { points.len() - 1 } else { index - 1 };
        let (x0, y0) = points[index];
        let (x1, y1) = points[previous];
        let crosses = (y0 > point.1) != (y1 > point.1)
            && point.0 < (x1 - x0) * (point.1 - y0) / (y1 - y0) + x0;
        if crosses {
            inside = !inside;
        }
    }
    inside
}

#[derive(Clone)]
struct SvgPaint {
    fill: Option<SvgPaintValue>,
    stroke: Option<SvgPaintValue>,
    stroke_width: f32,
    line_cap: LineCap,
    line_join: LineJoin,
    opacity: f32,
    fill_opacity: f32,
    stroke_opacity: f32,
    current_color: Color,
}

#[derive(Clone)]
enum SvgPaintValue {
    Solid(Color),
    Gradient(String),
}

#[derive(Clone, Copy)]
struct SvgTransform {
    sx: f32,
    sy: f32,
    tx: f32,
    ty: f32,
    viewport_width: f32,
    viewport_height: f32,
}

#[derive(Clone)]
struct SvgResources {
    by_id: BTreeMap<String, NodeHandle>,
    viewport_width: f32,
    viewport_height: f32,
}

impl SvgResources {
    fn collect(root: &NodeHandle, viewport_width: f32, viewport_height: f32) -> Self {
        let mut resources = Self {
            by_id: BTreeMap::new(),
            viewport_width,
            viewport_height,
        };
        resources.collect_node(root);
        resources
    }

    fn collect_node(&mut self, node: &NodeHandle) {
        if node.node_type() == NodeType::Element {
            if let Some(attrs) = node.attributes() {
                if let Some(id) = attribute_value(&attrs, "id") {
                    if !id.trim().is_empty() {
                        self.by_id.insert(id.trim().to_string(), node.clone());
                    }
                }
            }
        }
        for child in node.child_nodes() {
            self.collect_node(&child);
        }
    }

    fn node(&self, id: &str) -> Option<NodeHandle> {
        self.by_id.get(id).cloned()
    }

    fn gradient(&self, id: &str) -> Option<SvgGradient> {
        let mut visiting = HashSet::new();
        self.gradient_from_node(self.node(id)?, &mut visiting)
    }

    fn gradient_from_node(
        &self,
        node: NodeHandle,
        visiting: &mut HashSet<usize>,
    ) -> Option<SvgGradient> {
        let identity = node.identity();
        if !visiting.insert(identity) {
            return None;
        }
        let attrs = node.attributes().unwrap_or_default();
        let tag = node.tag_name()?.to_ascii_lowercase();
        let base = attribute_value(&attrs, "href")
            .or_else(|| attribute_value(&attrs, "xlink:href"))
            .and_then(|href| parse_fragment_reference(&href))
            .and_then(|id| self.node(&id))
            .and_then(|node| self.gradient_from_node(node, visiting));
        let result = parse_svg_gradient(&node, &attrs, &tag, base);
        visiting.remove(&identity);
        result
    }
}

#[derive(Clone, Copy)]
enum GradientUnits {
    ObjectBoundingBox,
    UserSpaceOnUse,
}

#[derive(Clone, Copy)]
enum SpreadMethod {
    Pad,
    Repeat,
    Reflect,
}

#[derive(Clone, Copy)]
enum GradientCoord {
    Percent(f32),
    Number(f32),
}

#[derive(Clone)]
enum SvgGradientKind {
    Linear {
        x1: GradientCoord,
        y1: GradientCoord,
        x2: GradientCoord,
        y2: GradientCoord,
    },
    Radial {
        cx: GradientCoord,
        cy: GradientCoord,
        r: GradientCoord,
        fx: GradientCoord,
        fy: GradientCoord,
    },
}

#[derive(Clone)]
struct SvgGradientStop {
    offset: f32,
    color: Color,
}

#[derive(Clone)]
struct SvgGradient {
    units: GradientUnits,
    spread: SpreadMethod,
    kind: SvgGradientKind,
    stops: Vec<SvgGradientStop>,
}

enum PaintSource {
    Solid(Color),
    Gradient { gradient: SvgGradient, opacity: f32 },
}

fn attribute_value(attrs: &BTreeMap<String, String>, name: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_string())
}

fn parse_fragment_reference(value: &str) -> Option<String> {
    value
        .trim()
        .strip_prefix('#')
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_url_reference(value: &str) -> Option<String> {
    let value = value.trim();
    let open = value.find('(')?;
    if !value[..open].trim().eq_ignore_ascii_case("url") || !value.ends_with(')') {
        return None;
    }
    let inner = value[open + 1..value.len() - 1]
        .trim()
        .trim_matches(|ch| ch == '\'' || ch == '"');
    parse_fragment_reference(inner)
}

fn parse_gradient_coord(value: &str) -> Option<GradientCoord> {
    let value = value.trim();
    if let Some(percent) = value.strip_suffix('%') {
        return percent
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .map(GradientCoord::Percent);
    }
    value
        .strip_suffix("px")
        .unwrap_or(value)
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .map(GradientCoord::Number)
}

fn parse_gradient_units(value: Option<&str>) -> GradientUnits {
    match value.map(str::trim) {
        Some(value) if value.eq_ignore_ascii_case("userspaceonuse") => {
            GradientUnits::UserSpaceOnUse
        }
        _ => GradientUnits::ObjectBoundingBox,
    }
}

fn parse_spread_method(value: Option<&str>) -> SpreadMethod {
    match value.map(str::trim) {
        Some(value) if value.eq_ignore_ascii_case("repeat") => SpreadMethod::Repeat,
        Some(value) if value.eq_ignore_ascii_case("reflect") => SpreadMethod::Reflect,
        _ => SpreadMethod::Pad,
    }
}

fn parse_gradient_stop(node: &NodeHandle) -> Option<SvgGradientStop> {
    let attrs = node.attributes().unwrap_or_default();
    let offset = attribute_value(&attrs, "offset")
        .and_then(|value| parse_gradient_coord(&value))
        .map(|coord| match coord {
            GradientCoord::Percent(value) => value / 100.0,
            GradientCoord::Number(value) => value,
        })
        .map(|value| value.clamp(0.0, 1.0));
    let color = property_value(&attrs, "stop-color")
        .and_then(|value| parse_color(&value))
        .unwrap_or(Color::rgb(0, 0, 0));
    let opacity = parse_opacity(property_value(&attrs, "stop-opacity").as_deref()).unwrap_or(1.0);
    Some(SvgGradientStop {
        offset: offset.unwrap_or(f32::NAN),
        color: with_alpha(color, opacity),
    })
}

fn fix_gradient_stop_offsets(stops: &mut [SvgGradientStop]) {
    if stops.is_empty() {
        return;
    }
    if stops[0].offset.is_nan() {
        stops[0].offset = 0.0;
    }
    if stops.last().is_some_and(|stop| stop.offset.is_nan()) {
        stops.last_mut().unwrap().offset = 1.0;
    }
    let mut index = 0;
    while index < stops.len() {
        if !stops[index].offset.is_nan() {
            index += 1;
            continue;
        }
        let start = index - 1;
        let mut end = index + 1;
        while end < stops.len() && stops[end].offset.is_nan() {
            end += 1;
        }
        let end_offset = if end < stops.len() { stops[end].offset } else { 1.0 };
        let start_offset = stops[start].offset;
        let count = (end - start) as f32;
        for current in index..end {
            stops[current].offset = start_offset
                + (end_offset - start_offset) * (current - start) as f32 / count;
        }
        index = end;
    }
    let mut previous = 0.0;
    for stop in stops {
        stop.offset = stop.offset.clamp(previous, 1.0);
        previous = stop.offset;
    }
}

fn parse_svg_gradient(
    node: &NodeHandle,
    attrs: &BTreeMap<String, String>,
    tag: &str,
    base: Option<SvgGradient>,
) -> Option<SvgGradient> {
    if !matches!(tag, "lineargradient" | "radialgradient") {
        return None;
    }
    let units = attribute_value(attrs, "gradientUnits")
        .map(|value| parse_gradient_units(Some(&value)))
        .or_else(|| base.as_ref().map(|gradient| gradient.units))
        .unwrap_or(GradientUnits::ObjectBoundingBox);
    let spread = attribute_value(attrs, "spreadMethod")
        .map(|value| parse_spread_method(Some(&value)))
        .or_else(|| base.as_ref().map(|gradient| gradient.spread))
        .unwrap_or(SpreadMethod::Pad);

    let mut stops = node
        .child_nodes()
        .into_iter()
        .filter(|child| child.tag_name().is_some_and(|tag| tag.eq_ignore_ascii_case("stop")))
        .filter_map(|child| parse_gradient_stop(&child))
        .collect::<Vec<_>>();
    if stops.is_empty() {
        stops = base.as_ref().map(|gradient| gradient.stops.clone()).unwrap_or_default();
    }
    if stops.is_empty() {
        return None;
    }
    fix_gradient_stop_offsets(&mut stops);

    let kind = if tag == "lineargradient" {
        let (base_x1, base_y1, base_x2, base_y2) = match base.as_ref().map(|gradient| &gradient.kind) {
            Some(SvgGradientKind::Linear { x1, y1, x2, y2 }) => (*x1, *y1, *x2, *y2),
            _ => (
                GradientCoord::Percent(0.0),
                GradientCoord::Percent(0.0),
                GradientCoord::Percent(100.0),
                GradientCoord::Percent(0.0),
            ),
        };
        SvgGradientKind::Linear {
            x1: attribute_value(attrs, "x1").and_then(|value| parse_gradient_coord(&value)).unwrap_or(base_x1),
            y1: attribute_value(attrs, "y1").and_then(|value| parse_gradient_coord(&value)).unwrap_or(base_y1),
            x2: attribute_value(attrs, "x2").and_then(|value| parse_gradient_coord(&value)).unwrap_or(base_x2),
            y2: attribute_value(attrs, "y2").and_then(|value| parse_gradient_coord(&value)).unwrap_or(base_y2),
        }
    } else {
        let (base_cx, base_cy, base_r) = match base.as_ref().map(|gradient| &gradient.kind) {
            Some(SvgGradientKind::Radial { cx, cy, r, .. }) => (*cx, *cy, *r),
            _ => (
                GradientCoord::Percent(50.0),
                GradientCoord::Percent(50.0),
                GradientCoord::Percent(50.0),
            ),
        };
        let cx = attribute_value(attrs, "cx").and_then(|value| parse_gradient_coord(&value)).unwrap_or(base_cx);
        let cy = attribute_value(attrs, "cy").and_then(|value| parse_gradient_coord(&value)).unwrap_or(base_cy);
        let fx = attribute_value(attrs, "fx")
            .and_then(|value| parse_gradient_coord(&value))
            .or_else(|| match base.as_ref().map(|gradient| &gradient.kind) {
                Some(SvgGradientKind::Radial { fx, .. }) => Some(*fx),
                _ => None,
            })
            .unwrap_or(cx);
        let fy = attribute_value(attrs, "fy")
            .and_then(|value| parse_gradient_coord(&value))
            .or_else(|| match base.as_ref().map(|gradient| &gradient.kind) {
                Some(SvgGradientKind::Radial { fy, .. }) => Some(*fy),
                _ => None,
            })
            .unwrap_or(cy);
        SvgGradientKind::Radial {
            cx,
            cy,
            r: attribute_value(attrs, "r").and_then(|value| parse_gradient_coord(&value)).unwrap_or(base_r),
            fx,
            fy,
        }
    };

    Some(SvgGradient { units, spread, kind, stops })
}

impl SvgGradient {
    fn sample(&self, x: f32, y: f32, bbox: Rect, transform: SvgTransform) -> Color {
        let (value, valid) = match self.kind {
            SvgGradientKind::Linear { x1, y1, x2, y2 } => {
                let start = (
                    resolve_gradient_coord(x1, self.units, bbox.x, bbox.width, transform.tx, transform.sx, transform.viewport_width),
                    resolve_gradient_coord(y1, self.units, bbox.y, bbox.height, transform.ty, transform.sy, transform.viewport_height),
                );
                let end = (
                    resolve_gradient_coord(x2, self.units, bbox.x, bbox.width, transform.tx, transform.sx, transform.viewport_width),
                    resolve_gradient_coord(y2, self.units, bbox.y, bbox.height, transform.ty, transform.sy, transform.viewport_height),
                );
                let dx = end.0 - start.0;
                let dy = end.1 - start.1;
                let denominator = dx * dx + dy * dy;
                if denominator <= f32::EPSILON {
                    (0.0, false)
                } else {
                    (((x - start.0) * dx + (y - start.1) * dy) / denominator, true)
                }
            }
            SvgGradientKind::Radial { cx, cy, r, fx, fy } => {
                let center = (
                    resolve_gradient_coord(cx, self.units, bbox.x, bbox.width, transform.tx, transform.sx, transform.viewport_width),
                    resolve_gradient_coord(cy, self.units, bbox.y, bbox.height, transform.ty, transform.sy, transform.viewport_height),
                );
                let focal = (
                    resolve_gradient_coord(fx, self.units, bbox.x, bbox.width, transform.tx, transform.sx, transform.viewport_width),
                    resolve_gradient_coord(fy, self.units, bbox.y, bbox.height, transform.ty, transform.sy, transform.viewport_height),
                );
                let radius = resolve_gradient_radius(r, self.units, bbox, transform);
                if radius <= f32::EPSILON {
                    (1.0, false)
                } else {
                    let dx = x - focal.0;
                    let dy = y - focal.1;
                    let distance = dx.hypot(dy);
                    if distance <= f32::EPSILON {
                        (0.0, true)
                    } else {
                        let ux = dx / distance;
                        let uy = dy / distance;
                        let focus_x = focal.0 - center.0;
                        let focus_y = focal.1 - center.1;
                        let b = 2.0 * (focus_x * ux + focus_y * uy);
                        let c = focus_x * focus_x + focus_y * focus_y - radius * radius;
                        let discriminant = (b * b - 4.0 * c).max(0.0);
                        let boundary = (-b + discriminant.sqrt()) / 2.0;
                        if boundary > f32::EPSILON {
                            (distance / boundary, true)
                        } else {
                            (distance / radius, true)
                        }
                    }
                }
            }
        };
        if !valid {
            return self.stops.last().map(|stop| stop.color).unwrap_or(Color::rgba(0, 0, 0, 0));
        }
        sample_gradient_stops(&self.stops, apply_spread(value, self.spread))
    }
}

fn resolve_gradient_coord(
    coord: GradientCoord,
    units: GradientUnits,
    origin: f32,
    size: f32,
    user_origin: f32,
    user_scale: f32,
    viewport_size: f32,
) -> f32 {
    match units {
        GradientUnits::ObjectBoundingBox => {
            origin
                + match coord {
                    GradientCoord::Percent(value) => value / 100.0 * size,
                    GradientCoord::Number(value) => value * size,
                }
        }
        GradientUnits::UserSpaceOnUse => {
            user_origin
                + match coord {
                    GradientCoord::Percent(value) => value / 100.0 * viewport_size * user_scale,
                    GradientCoord::Number(value) => value * user_scale,
                }
        }
    }
}

fn resolve_gradient_radius(
    coord: GradientCoord,
    units: GradientUnits,
    bbox: Rect,
    transform: SvgTransform,
) -> f32 {
    match units {
        GradientUnits::ObjectBoundingBox => match coord {
            GradientCoord::Percent(value) => value / 100.0 * bbox.width.min(bbox.height),
            GradientCoord::Number(value) => value * bbox.width.min(bbox.height),
        },
        GradientUnits::UserSpaceOnUse => match coord {
            GradientCoord::Percent(value) => {
                value / 100.0
                    * transform.viewport_width.min(transform.viewport_height)
                    * transform.sx.min(transform.sy)
            }
            GradientCoord::Number(value) => value * transform.sx.min(transform.sy),
        },
    }
}

fn apply_spread(value: f32, spread: SpreadMethod) -> f32 {
    match spread {
        SpreadMethod::Pad => value.clamp(0.0, 1.0),
        SpreadMethod::Repeat => value.rem_euclid(1.0),
        SpreadMethod::Reflect => {
            let value = value.rem_euclid(2.0);
            if value > 1.0 { 2.0 - value } else { value }
        }
    }
}

fn sample_gradient_stops(stops: &[SvgGradientStop], value: f32) -> Color {
    let Some(first) = stops.first() else { return Color::rgba(0, 0, 0, 0) };
    if value <= first.offset {
        return first.color;
    }
    for pair in stops.windows(2) {
        let left = &pair[0];
        let right = &pair[1];
        if value <= right.offset {
            let t = if right.offset > left.offset {
                ((value - left.offset) / (right.offset - left.offset)).clamp(0.0, 1.0)
            } else {
                1.0
            };
            return interpolate_color(left.color, right.color, t);
        }
    }
    stops.last().map(|stop| stop.color).unwrap_or(first.color)
}

fn interpolate_color(left: Color, right: Color, t: f32) -> Color {
    let alpha = left.a as f32 + (right.a as f32 - left.a as f32) * t;
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color::rgba(lerp(left.r, right.r), lerp(left.g, right.g), lerp(left.b, right.b), alpha.round().clamp(0.0, 255.0) as u8)
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
        .map(|value| parse_paint_value(&value, parent.fill.as_ref(), current_color))
        .unwrap_or_else(|| parent.fill.clone());
    let stroke = property_value(attrs, "stroke")
        .map(|value| parse_paint_value(&value, parent.stroke.as_ref(), current_color))
        .unwrap_or_else(|| parent.stroke.clone());
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

fn parse_paint_value(
    value: &str,
    inherited: Option<&SvgPaintValue>,
    current_color: Color,
) -> Option<SvgPaintValue> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") {
        None
    } else if value.eq_ignore_ascii_case("currentcolor") {
        Some(SvgPaintValue::Solid(current_color))
    } else if let Some(id) = parse_url_reference(value) {
        Some(SvgPaintValue::Gradient(id))
    } else {
        parse_color(value)
            .map(SvgPaintValue::Solid)
            .or_else(|| inherited.cloned())
    }
}

fn paint_source(
    value: Option<&SvgPaintValue>,
    resources: &SvgResources,
    opacity: f32,
) -> Option<PaintSource> {
    match value? {
        SvgPaintValue::Solid(color) => Some(PaintSource::Solid(with_alpha(*color, opacity))),
        SvgPaintValue::Gradient(id) => resources
            .gradient(id)
            .map(|gradient| PaintSource::Gradient { gradient, opacity }),
    }
}

fn source_color(
    source: &PaintSource,
    x: f32,
    y: f32,
    bbox: Rect,
    transform: SvgTransform,
) -> Color {
    match source {
        PaintSource::Solid(color) => *color,
        PaintSource::Gradient { gradient, opacity } => {
            with_alpha(gradient.sample(x, y, bbox, transform), *opacity)
        }
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
    resources: &SvgResources,
    visited: &mut HashSet<usize>,
) {
    for child in node.child_nodes() {
        if child.node_type() != NodeType::Element {
            continue;
        }
        render_svg_element(
            &child,
            canvas,
            sx,
            sy,
            tx,
            ty,
            &inherited,
            resources,
            visited,
        );
    }
}

fn render_svg_element(
    child: &NodeHandle,
    canvas: &mut Canvas,
    sx: f32,
    sy: f32,
    tx: f32,
    ty: f32,
    inherited: &SvgPaint,
    resources: &SvgResources,
    visited: &mut HashSet<usize>,
) {
    let Some(tag) = child.tag_name() else { return };
    let attrs = child.attributes().unwrap_or_default();
    let paint = resolve_paint(inherited, &attrs);
    let fill = paint_source(
        paint.fill.as_ref(),
        resources,
        paint.opacity * paint.fill_opacity,
    );
    let stroke = paint_source(
        paint.stroke.as_ref(),
        resources,
        paint.opacity * paint.stroke_opacity,
    );
    let transform = SvgTransform {
        sx,
        sy,
        tx,
        ty,
        viewport_width: resources.viewport_width,
        viewport_height: resources.viewport_height,
    };
    let tag = tag.to_ascii_lowercase();

    match tag.as_str() {
        // Definitions are resources, not paintable descendants.
        "defs" => {}
        "g" => render_svg_children(child, canvas, sx, sy, tx, ty, paint, resources, visited),
        "use" => {
            let Some(id) = attribute_value(&attrs, "href")
                .or_else(|| attribute_value(&attrs, "xlink:href"))
                .and_then(|href| parse_fragment_reference(&href))
            else {
                return;
            };
            let Some(target) = resources.node(&id) else { return };
            if !visited.insert(target.identity()) {
                return;
            }
            let x = parse_svg_coord(attribute_ref(&attrs, "x")).unwrap_or(0.0);
            let y = parse_svg_coord(attribute_ref(&attrs, "y")).unwrap_or(0.0);
            render_svg_element(
                &target,
                canvas,
                sx,
                sy,
                tx + x * sx,
                ty + y * sy,
                &paint,
                resources,
                visited,
            );
            visited.remove(&target.identity());
        }
        "rect" => {
            let rx = parse_svg_coord(attribute_ref(&attrs, "x")).unwrap_or(0.0) * sx + tx;
            let ry = parse_svg_coord(attribute_ref(&attrs, "y")).unwrap_or(0.0) * sy + ty;
            let rw = parse_svg_size(attribute_ref(&attrs, "width")).unwrap_or(0.0) * sx;
            let rh = parse_svg_size(attribute_ref(&attrs, "height")).unwrap_or(0.0) * sy;
            if rw > 0.0 && rh > 0.0 {
                let bbox = Rect { x: rx, y: ry, width: rw, height: rh };
                if let Some(source) = fill.as_ref() {
                    fill_rect_source(canvas, bbox, source, transform);
                }
                let points = vec![(rx, ry), (rx + rw, ry), (rx + rw, ry + rh), (rx, ry + rh)];
                let stroke_color = stroke.as_ref().map(|source| {
                    source_color(source, rx + rw / 2.0, ry + rh / 2.0, bbox, transform)
                });
                stroke_polyline(canvas, &points, true, stroke_color, paint.stroke_width * sx.min(sy), paint.line_cap, paint.line_join);
            }
        }
        "circle" => {
            let cx = parse_svg_coord(attribute_ref(&attrs, "cx")).unwrap_or(0.0) * sx + tx;
            let cy = parse_svg_coord(attribute_ref(&attrs, "cy")).unwrap_or(0.0) * sy + ty;
            let r = parse_svg_size(attribute_ref(&attrs, "r")).unwrap_or(0.0) * sx.min(sy);
            if r > 0.0 {
                let bbox = Rect { x: cx - r, y: cy - r, width: r * 2.0, height: r * 2.0 };
                if let Some(source) = fill.as_ref() {
                    fill_circle_source(canvas, cx, cy, r, source, transform, bbox);
                }
                let stroke_color = stroke.as_ref().map(|source| source_color(source, cx, cy, bbox, transform));
                stroke_ellipse(canvas, cx, cy, r, r, paint.stroke_width * sx.min(sy), stroke_color);
            }
        }
        "ellipse" => {
            let cx = parse_svg_coord(attribute_ref(&attrs, "cx")).unwrap_or(0.0) * sx + tx;
            let cy = parse_svg_coord(attribute_ref(&attrs, "cy")).unwrap_or(0.0) * sy + ty;
            let rx = parse_svg_size(attribute_ref(&attrs, "rx")).unwrap_or(0.0) * sx;
            let ry = parse_svg_size(attribute_ref(&attrs, "ry")).unwrap_or(0.0) * sy;
            if rx > 0.0 && ry > 0.0 {
                let bbox = Rect { x: cx - rx, y: cy - ry, width: rx * 2.0, height: ry * 2.0 };
                if let Some(source) = fill.as_ref() {
                    fill_ellipse_source(canvas, cx, cy, rx, ry, source, transform, bbox);
                }
                let stroke_color = stroke.as_ref().map(|source| source_color(source, cx, cy, bbox, transform));
                stroke_ellipse(canvas, cx, cy, rx, ry, paint.stroke_width * sx.min(sy), stroke_color);
            }
        }
        "line" => {
            let x1 = parse_svg_coord(attribute_ref(&attrs, "x1")).unwrap_or(0.0) * sx + tx;
            let y1 = parse_svg_coord(attribute_ref(&attrs, "y1")).unwrap_or(0.0) * sy + ty;
            let x2 = parse_svg_coord(attribute_ref(&attrs, "x2")).unwrap_or(0.0) * sx + tx;
            let y2 = parse_svg_coord(attribute_ref(&attrs, "y2")).unwrap_or(0.0) * sy + ty;
            let bbox = rect_for_points(&[(x1, y1), (x2, y2)]);
            let stroke_color = stroke.as_ref().map(|source| source_color(source, (x1 + x2) / 2.0, (y1 + y2) / 2.0, bbox, transform));
            stroke_polyline(canvas, &[(x1, y1), (x2, y2)], false, stroke_color, paint.stroke_width * sx.min(sy), paint.line_cap, paint.line_join);
        }
        "polyline" | "polygon" => {
            let points = parse_svg_points(attribute_ref(&attrs, "points"))
                .into_iter()
                .map(|(x, y)| (x * sx + tx, y * sy + ty))
                .collect::<Vec<_>>();
            let closed = tag == "polygon";
            let bbox = rect_for_points(&points);
            if closed && points.len() >= 3 {
                if let Some(source) = fill.as_ref() {
                    fill_compound_source(canvas, std::slice::from_ref(&points), source, FillRule::NonZero, transform, bbox);
                }
            }
            let stroke_color = stroke.as_ref().map(|source| source_color(source, bbox.x + bbox.width / 2.0, bbox.y + bbox.height / 2.0, bbox, transform));
            stroke_polyline(canvas, &points, closed, stroke_color, paint.stroke_width * sx.min(sy), paint.line_cap, paint.line_join);
        }
        "path" => {
            if let Some(d) = attribute_value(&attrs, "d") {
                let fill_rule = match property_value(&attrs, "fill-rule").as_deref() {
                    Some(value) if value.eq_ignore_ascii_case("evenodd") => FillRule::EvenOdd,
                    _ => FillRule::NonZero,
                };
                render_path(canvas, &d, sx, sy, tx, ty, fill.as_ref(), fill_rule, stroke.as_ref(), paint.stroke_width * sx.min(sy), paint.line_cap, paint.line_join, transform);
            }
        }
        _ => render_svg_children(child, canvas, sx, sy, tx, ty, paint, resources, visited),
    }
}

fn attribute_ref<'a>(attrs: &'a BTreeMap<String, String>, name: &str) -> Option<&'a String> {
    attrs
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

fn rect_for_points(points: &[(f32, f32)]) -> Rect {
    if points.is_empty() {
        return Rect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 };
    }
    let min_x = points.iter().map(|point| point.0).fold(f32::INFINITY, f32::min);
    let min_y = points.iter().map(|point| point.1).fold(f32::INFINITY, f32::min);
    let max_x = points.iter().map(|point| point.0).fold(f32::NEG_INFINITY, f32::max);
    let max_y = points.iter().map(|point| point.1).fold(f32::NEG_INFINITY, f32::max);
    Rect { x: min_x, y: min_y, width: (max_x - min_x).max(0.0), height: (max_y - min_y).max(0.0) }
}

fn rect_for_subpaths(subpaths: &[(Vec<(f32, f32)>, bool)]) -> Rect {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (points, _) in subpaths {
        for &(x, y) in points {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    if !min_x.is_finite() {
        return Rect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 };
    }
    Rect { x: min_x, y: min_y, width: (max_x - min_x).max(0.0), height: (max_y - min_y).max(0.0) }
}

fn fill_rect_source(canvas: &mut Canvas, rect: Rect, source: &PaintSource, transform: SvgTransform) {
    if let PaintSource::Solid(color) = source {
        canvas.fill_rect(rect, *color);
        return;
    }
    let x0 = rect.x.floor().max(0.0) as u32;
    let y0 = rect.y.floor().max(0.0) as u32;
    let x1 = (rect.x + rect.width).ceil().min(canvas.width() as f32).max(0.0) as u32;
    let y1 = (rect.y + rect.height).ceil().min(canvas.height() as f32).max(0.0) as u32;
    for y in y0..y1 {
        for x in x0..x1 {
            canvas.blend_pixel(x, y, source_color(source, x as f32 + 0.5, y as f32 + 0.5, rect, transform));
        }
    }
}

fn fill_circle_source(
    canvas: &mut Canvas,
    cx: f32,
    cy: f32,
    r: f32,
    source: &PaintSource,
    transform: SvgTransform,
    bbox: Rect,
) {
    let x0 = ((cx - r).floor() as i32).max(0) as u32;
    let y0 = ((cy - r).floor() as i32).max(0) as u32;
    let x1 = ((cx + r).ceil() as u32).min(canvas.width());
    let y1 = ((cy + r).ceil() as u32).min(canvas.height());
    let r2 = r * r;
    for py in y0..y1 {
        for px in x0..x1 {
            let x = px as f32 + 0.5;
            let y = py as f32 + 0.5;
            if (x - cx).powi(2) + (y - cy).powi(2) <= r2 {
                canvas.blend_pixel(px, py, source_color(source, x, y, bbox, transform));
            }
        }
    }
}

fn fill_ellipse_source(
    canvas: &mut Canvas,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    source: &PaintSource,
    transform: SvgTransform,
    bbox: Rect,
) {
    let x0 = ((cx - rx).floor() as i32).max(0) as u32;
    let y0 = ((cy - ry).floor() as i32).max(0) as u32;
    let x1 = ((cx + rx).ceil() as u32).min(canvas.width());
    let y1 = ((cy + ry).ceil() as u32).min(canvas.height());
    for py in y0..y1 {
        for px in x0..x1 {
            let x = px as f32 + 0.5;
            let y = py as f32 + 0.5;
            let dx = (x - cx) / rx;
            let dy = (y - cy) / ry;
            if dx * dx + dy * dy <= 1.0 {
                canvas.blend_pixel(px, py, source_color(source, x, y, bbox, transform));
            }
        }
    }
}

fn fill_compound_source(
    canvas: &mut Canvas,
    subpaths: &[Vec<(f32, f32)>],
    source: &PaintSource,
    fill_rule: FillRule,
    transform: SvgTransform,
    bbox: Rect,
) {
    if subpaths.is_empty() {
        return;
    }
    let min_y = subpaths.iter().flatten().map(|point| point.1).fold(f32::MAX, f32::min);
    let max_y = subpaths.iter().flatten().map(|point| point.1).fold(f32::MIN, f32::max);
    let y_start = (min_y.floor() as i32).max(0) as u32;
    let y_end = (max_y.ceil() as u32).min(canvas.height());
    for y in y_start..y_end {
        let scan_y = y as f32 + 0.5;
        let mut intersections = Vec::new();
        for points in subpaths {
            for index in 0..points.len() {
                let next = (index + 1) % points.len();
                let (x0, y0) = points[index];
                let (x1, y1) = points[next];
                if (y0 <= scan_y && y1 > scan_y) || (y1 <= scan_y && y0 > scan_y) {
                    let t = (scan_y - y0) / (y1 - y0);
                    let winding = if y1 > y0 { 1 } else { -1 };
                    intersections.push((x0 + t * (x1 - x0), winding));
                }
            }
        }
        intersections.sort_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(std::cmp::Ordering::Equal));
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
                    canvas.blend_pixel(x, y, source_color(source, x as f32 + 0.5, scan_y, bbox, transform));
                }
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

fn stroke_ellipse(canvas: &mut Canvas, cx: f32, cy: f32, rx: f32, ry: f32, width: f32, color: Option<Color>) {
    let Some(color) = color else { return };
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
    fill: Option<&PaintSource>,
    fill_rule: FillRule,
    stroke: Option<&PaintSource>,
    stroke_width: f32,
    line_cap: LineCap,
    line_join: LineJoin,
    transform: SvgTransform,
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
    let bbox = rect_for_subpaths(&subpaths);
    if let Some(fill) = fill {
        let fill_paths = subpaths
            .iter()
            .filter(|(points, _)| points.len() >= 3)
            .map(|(points, _)| points.clone())
            .collect::<Vec<_>>();
        if !fill_paths.is_empty() {
            fill_compound_source(canvas, &fill_paths, fill, fill_rule, transform, bbox);
        }
    }
    let stroke = stroke.map(|source| {
        source_color(
            source,
            bbox.x + bbox.width / 2.0,
            bbox.y + bbox.height / 2.0,
            bbox,
            transform,
        )
    });
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
    fn svg_hit_geometry_scales_viewbox_strokes_and_rejects_zero_width_paint() {
        let mut attrs = BTreeMap::new();
        attrs.insert("x1".to_string(), "5".to_string());
        attrs.insert("y1".to_string(), "0".to_string());
        attrs.insert("x2".to_string(), "5".to_string());
        attrs.insert("y2".to_string(), "10".to_string());

        // The viewBox is stretched 2x horizontally.  A two-unit user-space
        // stroke is therefore one display pixel wide on each side (the
        // rasterizer uses min(scale_x, scale_y) for stroke width).
        let outside = svg_hit_geometry("line", &attrs, (5.6, 5.0), 2.0, 2.0, 1.0);
        assert!(!outside.stroke, "horizontal viewBox scale must affect stroke distance");
        let inside = svg_hit_geometry("line", &attrs, (5.4, 5.0), 2.0, 2.0, 1.0);
        assert!(inside.stroke);

        let zero_width = svg_hit_geometry("line", &attrs, (5.0, 5.0), 0.0, 1.0, 1.0);
        assert!(!zero_width.stroke, "stroke-width:0 must not create hit geometry");

        let paint = SvgPaint {
            fill: None,
            stroke: Some(SvgPaintValue::Solid(Color::rgb(0, 0, 0))),
            stroke_width: 0.0,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            opacity: 1.0,
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            current_color: Color::rgb(0, 0, 0),
        };
        assert!(!pointer_events_accepts(
            "painted",
            &paint,
            true,
            SvgHitGeometry {
                fill: false,
                stroke: true,
                bounding_box: false,
            },
        ));
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
    fn svg_hit_path_does_not_bridge_subpaths_after_close() {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "d".to_string(),
            "M0 0 L0 10 Z M20 0 L20 10".to_string(),
        );
        let geometry = svg_hit_geometry("path", &attrs, (10.0, 0.0), 2.0, 1.0, 1.0);
        assert!(!geometry.stroke);
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

    #[test]
    fn defs_are_not_painted_and_use_expands_a_fragment_reference() {
        let html = r##"<svg width="12" height="6">
          <defs><rect id="tile" width="3" height="3" fill="red"/></defs>
          <use href="#tile" x="6" y="1"/>
        </svg>"##;
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
        assert_eq!(pixel(1, 1).a, 0, "a definition must not render in place");
        assert_eq!(pixel(7, 2), Color::rgb(255, 0, 0));
    }

    #[test]
    fn linear_gradient_stops_fill_a_rect_and_respect_spread() {
        let html = r##"<svg width="12" height="4">
          <defs><linearGradient id="g" x1="0%" x2="25%" spreadMethod="repeat">
            <stop offset="0%" stop-color="red"/><stop offset="100%" stop-color="blue"/>
          </linearGradient></defs>
          <rect width="12" height="4" fill="url(#g)"/>
        </svg>"##;
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
        let left = pixel(0, 2);
        let middle = pixel(2, 2);
        let repeated = pixel(3, 2);
        assert!(left.r > 200 && left.b < 60, "left sample: {left:?}");
        assert!(middle.r > 40 && middle.b > 40, "gradient should interpolate stops: {middle:?}");
        assert!(repeated.r > 200 && repeated.b < 60, "repeat spread should restart: {repeated:?}");
    }

    #[test]
    fn linear_gradient_fills_a_path() {
        let html = r##"<svg width="10" height="10">
          <defs><linearGradient id="path-gradient"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient></defs>
          <path d="M1 1H9V9H1Z" fill="url(#path-gradient)"/>
        </svg>"##;
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
        assert!(pixel(1, 5).r > pixel(8, 5).r);
        assert!(pixel(8, 5).b > pixel(1, 5).b);
    }

    #[test]
    fn radial_gradient_and_unresolved_or_cyclic_references_are_safe() {
        let html = r##"<svg width="12" height="6">
          <defs>
            <radialGradient id="rad"><stop offset="0" stop-color="white"/><stop offset="1" stop-color="black"/></radialGradient>
            <g id="loop"><use href="#loop"/></g>
          </defs>
          <circle cx="3" cy="3" r="3" fill="url(#rad)"/>
          <use href="#missing"/><use href="#loop"/>
        </svg>"##;
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
        assert!(pixel(3, 3).r > pixel(0, 3).r);
        assert_eq!(pixel(10, 3).a, 0);
    }
}
