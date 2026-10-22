//! Solid CSS outline geometry and painting.

use crate::css::{ComputedStyle, ComputedValue};
use crate::layout::Rect;

use super::{BorderRegion, Canvas, Color, border_radius_corners, resolve_color_value};

#[derive(Clone, Copy)]
struct SolidOutline {
    outer: Rect,
    inner: Rect,
    outer_radii: (f32, f32, f32, f32),
    inner_radii: (f32, f32, f32, f32),
    width: f32,
}

fn outline_width(style: &ComputedStyle) -> Option<f32> {
    if !matches!(
        style.get("outline-style"),
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("solid")
    ) {
        return None;
    }
    let width = match style.get("outline-width") {
        Some(ComputedValue::Px(value)) | Some(ComputedValue::Number(value)) => *value,
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("thin") => 1.0,
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("thick") => 5.0,
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("medium") => 3.0,
        None => 3.0,
        _ => return None,
    };
    (width.is_finite() && width > 0.0).then_some(width)
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

fn solid_outline(style: &ComputedStyle, border_box: Rect) -> Option<SolidOutline> {
    let width = outline_width(style)?;
    if !border_box.width.is_finite()
        || !border_box.height.is_finite()
        || border_box.width < 0.0
        || border_box.height < 0.0
    {
        return None;
    }
    let offset = match style.get("outline-offset") {
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
    Some(SolidOutline {
        outer: inflate(border_box, horizontal_outer, vertical_outer),
        inner: inflate(border_box, horizontal_offset, vertical_offset),
        outer_radii,
        inner_radii,
        width,
    })
}

/// Returns the painted extent of a solid outline for effect-surface allocation.
pub(super) fn solid_outline_bounds(style: &ComputedStyle, border_box: Rect) -> Option<Rect> {
    solid_outline(style, border_box).map(|outline| outline.outer)
}

/// Paints a box's solid outline over its content, clipped only by ancestors.
pub(super) fn paint_solid_outline(
    canvas: &mut Canvas,
    style: &ComputedStyle,
    border_box: Rect,
    inherited_clip: Option<Rect>,
) {
    let Some(outline) = solid_outline(style, border_box) else {
        return;
    };
    let color = resolve_color_value(style.get("outline-color"), style)
        .or_else(|| resolve_color_value(style.get("color"), style))
        .unwrap_or(Color::rgb(0, 0, 0));
    if color.a == 0 {
        return;
    }
    if outline.outer_radii == (0.0, 0.0, 0.0, 0.0) {
        let outer = outline.outer;
        let inner = outline.inner;
        for strip in [
            Rect {
                height: outline.width,
                ..outer
            },
            Rect {
                y: inner.y + inner.height,
                height: outline.width,
                ..outer
            },
            Rect {
                y: inner.y,
                width: outline.width,
                height: inner.height,
                ..outer
            },
            Rect {
                x: inner.x + inner.width,
                y: inner.y,
                width: outline.width,
                height: inner.height,
            },
        ] {
            canvas.fill_rect_clipped(strip, color, inherited_clip);
        }
        return;
    }
    let (outer_tl, outer_tr, outer_br, outer_bl) = outline.outer_radii;
    let (inner_tl, inner_tr, inner_br, inner_bl) = outline.inner_radii;
    canvas.fill_rounded_rect_annulus(
        outline.outer,
        outer_tl,
        outer_tr,
        outer_br,
        outer_bl,
        outline.inner,
        inner_tl,
        inner_tr,
        inner_br,
        inner_bl,
        color,
        inherited_clip,
        BorderRegion::All,
        outline.width,
    );
}
