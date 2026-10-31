//! Script bindings for the Canvas 2D rasterizer in [`crate::canvas2d`].
//!
//! Script keeps the context state and builds device-space paths; these
//! natives draw into a canvas bitmap, answer point queries and parse colors.

use super::canvas_blit::byte_view;
use super::*;
use crate::canvas2d::{
    self, DrawCommand, FillRule, Matrix, StrokeStyle, SubPath, Surface, Texture,
};

/// `__omoikane_canvas2d_draw(pixels, width, height, command, texture?)`
/// runs one [`DrawCommand`] JSON on an RGBA `Uint8Array`. Returns `false`
/// when the arguments do not describe a drawable operation.
pub(super) fn draw_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let dimension = |index: usize| {
        args.get(index)
            .and_then(JsValue::as_number)
            .filter(|value| value.is_finite() && *value >= 0.0 && value.fract() == 0.0)
            .map(|value| value as usize)
    };
    let (Some(width), Some(height)) = (dimension(1), dimension(2)) else {
        return Ok(false.into());
    };
    let Some(json) = args.get(3).and_then(JsValue::as_string) else {
        return Ok(false.into());
    };
    let Ok(command) = serde_json::from_str::<DrawCommand>(&json.to_std_string_escaped()) else {
        return Ok(false.into());
    };
    // Copy the texture before borrowing the target, which may share its buffer.
    let texture = match &command.paint {
        Some(canvas2d::PaintSpec::Pattern { width, height, .. })
        | Some(canvas2d::PaintSpec::Image { width, height, .. }) => {
            let Some(view) = byte_view(args.get(4), context)? else {
                return Ok(false.into());
            };
            let Some(bytes) = view.copy() else {
                return Ok(false.into());
            };
            if width.checked_mul(*height).and_then(|n| n.checked_mul(4)) != Some(bytes.len()) {
                return Ok(false.into());
            }
            Some((*width, *height, bytes))
        }
        _ => None,
    };
    let Some(target) = byte_view(args.first(), context)? else {
        return Ok(false.into());
    };
    let Some(length) = width.checked_mul(height).and_then(|n| n.checked_mul(4)) else {
        return Ok(false.into());
    };
    if length > target.length {
        return Ok(false.into());
    }
    let Some(mut data) = target.buffer.data_mut() else {
        return Ok(false.into());
    };
    let Some(pixels) = data
        .get_mut(target.offset..)
        .and_then(|tail| tail.get_mut(..length))
    else {
        return Ok(false.into());
    };
    let mut surface = Surface {
        width,
        height,
        pixels,
    };
    let texture = texture.as_ref().map(|(width, height, pixels)| Texture {
        width: *width,
        height: *height,
        pixels,
    });
    Ok(canvas2d::draw(&mut surface, &command, texture).into())
}

#[derive(serde::Deserialize)]
struct PointQuery {
    paths: Vec<SubPath>,
    #[serde(default)]
    rule: FillRule,
    #[serde(default = "Matrix::identity")]
    transform: Matrix,
    #[serde(default)]
    stroke: Option<StrokeStyle>,
    point: [f64; 2],
}

/// `__omoikane_canvas2d_contains(query)` answers `isPointInPath()`, or
/// `isPointInStroke()` when the query carries stroke styles.
pub(super) fn contains_native(_: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let Some(json) = args.first().and_then(JsValue::as_string) else {
        return Ok(false.into());
    };
    let Ok(query) = serde_json::from_str::<PointQuery>(&json.to_std_string_escaped()) else {
        return Ok(false.into());
    };
    let inside = match &query.stroke {
        Some(style) => canvas2d::stroke_contains(&query.paths, query.transform, style, query.point),
        None => canvas2d::path_contains(&query.paths, query.rule, query.point),
    };
    Ok(inside.into())
}

/// `__omoikane_canvas_parse_color(text)` parses a CSS color into
/// `[r, g, b, a]`, or returns `null`.
pub(super) fn parse_color_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let Some(text) = args.first().and_then(JsValue::as_string) else {
        return Ok(JsValue::null());
    };
    let Some(color) = crate::paint::color::parse_color(&text.to_std_string_escaped()) else {
        return Ok(JsValue::null());
    };
    let values = [color.r, color.g, color.b, color.a].map(|value| JsValue::from(i32::from(value)));
    Ok(boa_engine::object::builtins::JsArray::from_iter(values, context).into())
}

/// Converts SVG path data into absolute `M`, `L`, `Q`, `C` and `Z` commands
/// for `new Path2D(d)`. Elliptical arcs are flattened to lines.
fn svg_path_commands(d: &str) -> Vec<serde_json::Value> {
    use crate::svg::PathCommand;
    use serde_json::json;
    let (mut x, mut y, mut start) = (0.0_f32, 0.0_f32, (0.0_f32, 0.0_f32));
    let mut out = Vec::new();
    for command in crate::svg::parse_path_data(d) {
        match command {
            PathCommand::MoveTo(nx, ny) => {
                (x, y, start) = (nx, ny, (nx, ny));
                out.push(json!(["M", x, y]));
            }
            PathCommand::LineTo(nx, ny) => {
                (x, y) = (nx, ny);
                out.push(json!(["L", x, y]));
            }
            PathCommand::HorizontalLineTo(nx) => {
                x = nx;
                out.push(json!(["L", x, y]));
            }
            PathCommand::VerticalLineTo(ny) => {
                y = ny;
                out.push(json!(["L", x, y]));
            }
            PathCommand::CurveTo(x1, y1, x2, y2, nx, ny) => {
                (x, y) = (nx, ny);
                out.push(json!(["C", x1, y1, x2, y2, x, y]));
            }
            PathCommand::QuadraticCurveTo(x1, y1, nx, ny) => {
                (x, y) = (nx, ny);
                out.push(json!(["Q", x1, y1, x, y]));
            }
            PathCommand::ArcTo(rx, ry, rotation, large, sweep, nx, ny) => {
                let mut points = Vec::new();
                crate::svg::flatten_arc(
                    &mut points,
                    x,
                    y,
                    rx,
                    ry,
                    rotation,
                    large,
                    sweep,
                    nx,
                    ny,
                    1.0,
                    1.0,
                    0.0,
                    0.0,
                );
                out.extend(points.into_iter().map(|(px, py)| json!(["L", px, py])));
                (x, y) = (nx, ny);
            }
            PathCommand::Close => {
                (x, y) = start;
                out.push(json!(["Z"]));
            }
        }
    }
    out
}

/// `__omoikane_canvas_svg_path(d)` returns [`svg_path_commands`] as JSON.
pub(super) fn svg_path_native(_: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let d = args
        .first()
        .and_then(JsValue::as_string)
        .map(|text| text.to_std_string_escaped())
        .unwrap_or_default();
    let json = serde_json::Value::Array(svg_path_commands(&d)).to_string();
    Ok(js_string!(json).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_path_data_becomes_absolute_commands() {
        let commands =
            serde_json::Value::Array(svg_path_commands("M 1 2 h 3 v 4 z l 1 1")).to_string();
        assert_eq!(
            commands,
            r#"[["M",1.0,2.0],["L",4.0,2.0],["L",4.0,6.0],["Z"],["L",2.0,3.0]]"#
        );
    }
}
