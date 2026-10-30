//! Solid CSS outline geometry and painting.

use crate::css::style::PropertyId;
use crate::css::{ComputedStyle, ComputedValue};
use crate::layout::Rect;

use super::{
    BorderRegion, Canvas, Color, blend_pixel_with_coverage, border_radius_corners,
    resolve_color_value, rounded_rect_pixel_coverage,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutlineStyle {
    Solid,
    Dashed,
    Dotted,
    Double,
}

#[derive(Clone, Copy)]
struct OutlineGeometry {
    outer: Rect,
    inner: Rect,
    outer_radii: (f32, f32, f32, f32),
    inner_radii: (f32, f32, f32, f32),
    width: f32,
    style: OutlineStyle,
}

fn outline_metrics(style: &ComputedStyle) -> Option<(f32, OutlineStyle)> {
    let outline_style = match style.get_by_id(PropertyId::OutlineStyle) {
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("solid") => {
            OutlineStyle::Solid
        }
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("auto") => {
            OutlineStyle::Solid
        }
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("dashed") => {
            OutlineStyle::Dashed
        }
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("dotted") => {
            OutlineStyle::Dotted
        }
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("double") => {
            OutlineStyle::Double
        }
        _ => return None,
    };
    let width = match style.get_by_id(PropertyId::OutlineWidth) {
        Some(ComputedValue::Px(value)) | Some(ComputedValue::Number(value)) => *value,
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("thin") => 1.0,
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("thick") => 5.0,
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("medium") => 3.0,
        None => 3.0,
        _ => return None,
    };
    (width.is_finite() && width > 0.0).then_some((width, outline_style))
}

fn inflate(rect: Rect, horizontal: f32, vertical: f32) -> Rect {
    Rect {
        x: rect.x - horizontal,
        y: rect.y - vertical,
        width: rect.width + 2.0 * horizontal,
        height: rect.height + 2.0 * vertical,
    }
}

fn radii_at_offset(radii: (f32, f32, f32, f32), offset: f32) -> (f32, f32, f32, f32) {
    let adjusted = |radius: f32| {
        if radius > 0.0 {
            (radius + offset).max(0.0)
        } else {
            0.0
        }
    };
    (
        adjusted(radii.0),
        adjusted(radii.1),
        adjusted(radii.2),
        adjusted(radii.3),
    )
}

fn outline_geometry(style: &ComputedStyle, border_box: Rect) -> Option<OutlineGeometry> {
    let (width, outline_style) = outline_metrics(style)?;
    if !border_box.width.is_finite()
        || !border_box.height.is_finite()
        || border_box.width < 0.0
        || border_box.height < 0.0
    {
        return None;
    }
    let offset = match style.get_by_id(PropertyId::OutlineOffset) {
        Some(ComputedValue::Px(value)) | Some(ComputedValue::Number(value)) => *value,
        None => 0.0,
        _ => return None,
    };
    if !offset.is_finite() {
        return None;
    }
    // Each axis must retain at least twice the stroke width when a negative
    // offset collapses a narrow box. A single clamp would enlarge the other
    // axis of a tall or wide box too far.
    let horizontal_offset = offset.max(-border_box.width / 2.0);
    let vertical_offset = offset.max(-border_box.height / 2.0);
    let horizontal_outer = horizontal_offset + width;
    let vertical_outer = vertical_offset + width;
    let radii = border_radius_corners(style);
    // The rasterizer stores circular radii. At the per-axis minimum the
    // resulting shape is rectangular rather than an elliptical corner.
    let (outer_radii, inner_radii) = if horizontal_offset == vertical_offset {
        (
            radii_at_offset(radii, horizontal_outer),
            radii_at_offset(radii, horizontal_offset),
        )
    } else {
        ((0.0, 0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 0.0))
    };
    Some(OutlineGeometry {
        outer: inflate(border_box, horizontal_outer, vertical_outer),
        inner: inflate(border_box, horizontal_offset, vertical_offset),
        outer_radii,
        inner_radii,
        width,
        style: outline_style,
    })
}

/// Returns the painted extent of an outline for effect-surface allocation.
pub(super) fn outline_bounds(style: &ComputedStyle, border_box: Rect) -> Option<Rect> {
    outline_geometry(style, border_box).map(|outline| outline.outer)
}

/// Paints a box's outline over its content, clipped only by ancestors.
pub(super) fn paint_outline(
    canvas: &mut Canvas,
    style: &ComputedStyle,
    border_box: Rect,
    inherited_clip: Option<Rect>,
) {
    let Some(outline) = outline_geometry(style, border_box) else {
        return;
    };
    let color = resolve_color_value(style.get_by_id(PropertyId::OutlineColor), style)
        .or_else(|| resolve_color_value(style.get_by_id(PropertyId::Color), style))
        .unwrap_or(Color::rgb(0, 0, 0));
    if color.a == 0 {
        return;
    }
    match outline.style {
        OutlineStyle::Solid => paint_outline_annulus(
            canvas,
            outline.outer,
            outline.inner,
            outline.outer_radii,
            outline.inner_radii,
            outline.width,
            color,
            inherited_clip,
        ),
        OutlineStyle::Double => {
            let stripe = outline.width / 3.0;
            let middle_outer = inset_rect(outline.outer, stripe);
            let middle_inner = inset_rect(outline.outer, stripe * 2.0);
            let middle_outer_radii = radii_at_offset(outline.outer_radii, -stripe);
            let middle_inner_radii = radii_at_offset(outline.outer_radii, -stripe * 2.0);
            paint_outline_annulus(
                canvas,
                outline.outer,
                middle_outer,
                outline.outer_radii,
                middle_outer_radii,
                stripe,
                color,
                inherited_clip,
            );
            paint_outline_annulus(
                canvas,
                middle_inner,
                outline.inner,
                middle_inner_radii,
                outline.inner_radii,
                stripe,
                color,
                inherited_clip,
            );
        }
        OutlineStyle::Dashed | OutlineStyle::Dotted => {
            paint_patterned_outline(canvas, outline, color, inherited_clip)
        }
    }
}

fn inset_rect(rect: Rect, amount: f32) -> Rect {
    Rect {
        x: rect.x + amount,
        y: rect.y + amount,
        width: (rect.width - amount * 2.0).max(0.0),
        height: (rect.height - amount * 2.0).max(0.0),
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_outline_annulus(
    canvas: &mut Canvas,
    outer: Rect,
    inner: Rect,
    outer_radii: (f32, f32, f32, f32),
    inner_radii: (f32, f32, f32, f32),
    width: f32,
    color: Color,
    inherited_clip: Option<Rect>,
) {
    if outer_radii == (0.0, 0.0, 0.0, 0.0) {
        for strip in [
            Rect {
                height: width,
                ..outer
            },
            Rect {
                y: inner.y + inner.height,
                height: width,
                ..outer
            },
            Rect {
                y: inner.y,
                width,
                height: inner.height,
                ..outer
            },
            Rect {
                x: inner.x + inner.width,
                y: inner.y,
                width,
                height: inner.height,
            },
        ] {
            canvas.fill_rect_clipped(strip, color, inherited_clip);
        }
        return;
    }
    let (outer_tl, outer_tr, outer_br, outer_bl) = outer_radii;
    let (inner_tl, inner_tr, inner_br, inner_bl) = inner_radii;
    canvas.fill_rounded_rect_annulus(
        outer,
        outer_tl,
        outer_tr,
        outer_br,
        outer_bl,
        inner,
        inner_tl,
        inner_tr,
        inner_br,
        inner_bl,
        color,
        inherited_clip,
        BorderRegion::All,
        width,
    );
}

fn paint_patterned_outline(
    canvas: &mut Canvas,
    outline: OutlineGeometry,
    color: Color,
    inherited_clip: Option<Rect>,
) {
    let unit = match outline.style {
        OutlineStyle::Dashed => outline.width * 3.0,
        OutlineStyle::Dotted => outline.width,
        _ => return,
    }
    .max(1.0);
    let period = unit * 2.0;
    let x0 = outline.outer.x.floor().max(0.0) as i32;
    let y0 = outline.outer.y.floor().max(0.0) as i32;
    let x1 = (outline.outer.x + outline.outer.width)
        .ceil()
        .min(canvas.width() as f32) as i32;
    let y1 = (outline.outer.y + outline.outer.height)
        .ceil()
        .min(canvas.height() as f32) as i32;
    for y in y0..y1 {
        for x in x0..x1 {
            let center = (x as f32 + 0.5, y as f32 + 0.5);
            if inherited_clip.is_some_and(|clip| {
                center.0 < clip.x
                    || center.0 >= clip.x + clip.width
                    || center.1 < clip.y
                    || center.1 >= clip.y + clip.height
            }) {
                continue;
            }
            let outer = rounded_rect_pixel_coverage(x, y, outline.outer, outline.outer_radii);
            let inner = rounded_rect_pixel_coverage(x, y, outline.inner, outline.inner_radii);
            let coverage = (outer - inner).clamp(0.0, 1.0);
            if coverage == 0.0 {
                continue;
            }
            let phase = outline_perimeter_phase(outline.outer, center.0, center.1);
            if phase.rem_euclid(period) >= unit {
                continue;
            }
            let index = ((y as u32 * canvas.width() + x as u32) * 4) as usize;
            blend_pixel_with_coverage(&mut canvas.pixels[index..index + 4], color, coverage);
        }
    }
}

fn outline_perimeter_phase(rect: Rect, x: f32, y: f32) -> f32 {
    let top = (y - rect.y).abs();
    let right = (x - (rect.x + rect.width)).abs();
    let bottom = (y - (rect.y + rect.height)).abs();
    let left = (x - rect.x).abs();
    let nearest = top.min(right).min(bottom).min(left);
    if nearest == top {
        x - rect.x
    } else if nearest == right {
        rect.width + y - rect.y
    } else if nearest == bottom {
        rect.width * 2.0 + rect.height - (x - rect.x)
    } else {
        rect.width * 2.0 + rect.height * 2.0 - (y - rect.y)
    }
}
