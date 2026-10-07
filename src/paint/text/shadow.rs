//! Text shadows reuse the foreground rasterizer and decoration geometry.
//! An allocation-free probe obtains source ink before viewport clipping.

use super::*;
use crate::css::style::text_shadow::TextShadow;

pub(super) fn style_shadows(style: &ComputedStyle) -> Vec<TextShadow> {
    style
        .get("text-shadow")
        .and_then(|value| crate::css::style::text_shadow::parse_computed(&value.css_text()))
        .unwrap_or_default()
}

fn fragment_shadows(
    fragment: &crate::layout::InlineFragment,
    resolver: &mut crate::css::StyleResolver,
) -> Vec<TextShadow> {
    let style = match fragment.style.pseudo {
        Some(pseudo) => resolver.paint_pseudo_style(&fragment.node, pseudo),
        None => Some(resolver.paint_style(&fragment.node)),
    };
    style.as_deref().map(style_shadows).unwrap_or_default()
}

fn paint_fragment_mask(
    canvas: &mut Canvas,
    fragment: &crate::layout::InlineFragment,
    line: &LineBox,
    resolver: &mut crate::css::StyleResolver,
    context: super::super::PaintContext<'_>,
    offset: super::super::PaintOffset,
) {
    if let InlineFragmentContent::FormControl(style, value, _) = &fragment.content {
        paint_form_control_text(
            canvas,
            fragment,
            offset.rect(fragment.rect),
            style,
            value,
            &None,
            resolver,
            Color::rgb(255, 255, 255),
            context.text_fonts,
            context.web_fonts,
            None,
            Some(Color::rgb(255, 255, 255)),
        );
        return;
    }
    let InlineFragmentContent::Text(text) = &fragment.content else {
        return;
    };
    let white = Color::rgb(255, 255, 255);
    let rect = offset.rect(fragment.rect);
    let (_, vertical) = paint_text_fragment_glyphs(
        canvas,
        fragment,
        rect,
        text,
        resolver,
        white,
        context.text_fonts,
        context.web_fonts,
        None,
        Some(white),
    );
    paint_text_fragment_decorations(
        canvas,
        fragment,
        rect,
        line,
        resolver,
        white,
        vertical,
        context.text_fonts,
        context.web_fonts,
        None,
        offset,
        &mut HashMap::new(),
        &mut HashMap::new(),
        Some(white),
    );
}

fn source_ink(
    fragment: &crate::layout::InlineFragment,
    line: &LineBox,
    resolver: &mut crate::css::StyleResolver,
    context: super::super::PaintContext<'_>,
) -> Option<Rect> {
    let mut probe = Canvas::text_ink_probe();
    paint_fragment_mask(
        &mut probe,
        fragment,
        line,
        resolver,
        context,
        super::super::PaintOffset::default(),
    );
    probe.probed_ink_bounds()
}

fn blur_support(blur: f32) -> f32 {
    let sigma = blur * 0.5;
    if sigma > 0.5 && sigma <= 2.0 {
        (sigma * 3.0).ceil()
    } else {
        super::super::border::gaussian_box_blur_radii(blur)
            .iter()
            .copied()
            .fold(0u32, u32::saturating_add) as f32
    }
}

fn blur_erases_ink(ink: Rect, blur: f32) -> bool {
    let sigma = blur * 0.5;
    if sigma > 0.5 && sigma <= 2.0 {
        return false;
    }
    let radii = super::super::border::gaussian_box_blur_radii(blur);
    let Some(radius) = radii.into_iter().find(|radius| *radius > 0) else {
        return false;
    };
    let divisor = (u64::from(radius) * 2 + 1).min(u64::from(u32::MAX)) as f64;
    // The first horizontal pass truncates alpha to u8. Even an entirely opaque
    // source row cannot survive if its total mass divided by this kernel is <1.
    // Two extra columns bound floor/ceil rasterization at fractional positions.
    255.0 * (f64::from(ink.width).ceil() + 2.0) / divisor < 1.0
}

fn shadow_bounds(ink: Rect, shadow: &TextShadow) -> Rect {
    let margin = blur_support(shadow.blur) + 1.0;
    Rect {
        x: ink.x + shadow.offset_x - margin,
        y: ink.y + shadow.offset_y - margin,
        width: ink.width + margin * 2.0,
        height: ink.height + margin * 2.0,
    }
}

fn ink_and_shadows(ink: Rect, shadows: &[TextShadow]) -> Rect {
    shadows.iter().fold(ink, |bounds, shadow| {
        super::super::union_rect(bounds, shadow_bounds(ink, shadow))
    })
}

pub(in crate::paint) fn text_shadow_paint_bounds(
    layout: &LayoutBox,
    resolver: &mut crate::css::StyleResolver,
    context: super::super::PaintContext<'_>,
) -> Option<Rect> {
    let mut bounds = None;
    for line in &layout.lines {
        let fragments = line
            .text_overflow
            .as_ref()
            .map_or(line.fragments.as_slice(), |overflow| {
                overflow.fragments.as_slice()
            });
        for fragment in fragments {
            if fragment.style.visibility == crate::layout::Visibility::Hidden {
                continue;
            }
            let shadows = fragment_shadows(fragment, resolver);
            if let Some(ink) = source_ink(fragment, line, resolver, context) {
                let ink_bounds = ink_and_shadows(ink, &shadows);
                bounds = Some(
                    bounds.map_or(ink_bounds, |old| super::super::union_rect(old, ink_bounds)),
                );
            }
        }
    }
    if layout.marker.is_some() {
        let style = super::super::paint_box_style(layout, resolver);
        let mut probe = Canvas::text_ink_probe();
        paint_list_marker_glyphs(
            &mut probe,
            layout,
            Color::rgb(255, 255, 255),
            None,
            context.text_fonts,
            super::super::PaintOffset::default(),
        );
        if let Some(ink) = probe.probed_ink_bounds() {
            let ink = ink_and_shadows(ink, &style_shadows(&style));
            bounds = Some(bounds.map_or(ink, |old| super::super::union_rect(old, ink)));
        }
    }
    bounds
}

pub(super) fn paint_line_shadows(
    canvas: &mut Canvas,
    line: &LineBox,
    fragments: &[crate::layout::InlineFragment],
    resolver: &mut crate::css::StyleResolver,
    fallback: Color,
    clip: Option<Rect>,
    context: super::super::PaintContext<'_>,
    offset: super::super::PaintOffset,
) {
    for fragment in fragments {
        if !matches!(fragment.content, InlineFragmentContent::Text(_)) {
            continue;
        }
        if fragment.style.visibility == crate::layout::Visibility::Hidden {
            continue;
        }
        let shadows = fragment_shadows(fragment, resolver);
        if shadows.is_empty() {
            continue;
        }
        let Some(ink) = source_ink(fragment, line, resolver, context) else {
            continue;
        };
        let current_color = fragment_paint_color(fragment, resolver, fallback);
        paint_shadow_masks(
            canvas,
            ink,
            &shadows,
            current_color,
            clip,
            offset,
            |mask, mask_offset| {
                paint_fragment_mask(mask, fragment, line, resolver, context, mask_offset);
            },
        );
    }
}

pub(super) fn paint_shadow_masks(
    canvas: &mut Canvas,
    ink: Rect,
    shadows: &[TextShadow],
    current_color: Color,
    clip: Option<Rect>,
    offset: super::super::PaintOffset,
    mut paint_mask: impl FnMut(&mut Canvas, super::super::PaintOffset),
) {
    let canvas_bounds = Rect {
        x: 0.0,
        y: 0.0,
        width: canvas.width() as f32,
        height: canvas.height() as f32,
    };
    let visible = match clip {
        Some(clip) => super::super::intersect(canvas_bounds, clip),
        None => Some(canvas_bounds),
    };
    let Some(visible) = visible else {
        return;
    };
    for shadow in shadows.iter().rev() {
        let color = if shadow.color.eq_ignore_ascii_case("currentcolor") {
            current_color
        } else {
            let Some(color) = parse_color(&shadow.color) else {
                continue;
            };
            color
        };
        if color.a == 0 || blur_erases_ink(ink, shadow.blur) {
            continue;
        }
        let full_bounds = offset.rect(shadow_bounds(ink, shadow));
        let Some(destination) = super::super::intersect(full_bounds, visible) else {
            continue;
        };
        // Retain all source samples that can contribute through the blur.
        let support = blur_support(shadow.blur) + 1.0;
        let required = Rect {
            x: destination.x - support,
            y: destination.y - support,
            width: destination.width + 2.0 * support,
            height: destination.height + 2.0 * support,
        };
        let Some(bounds) = super::super::intersect(full_bounds, required) else {
            continue;
        };
        let x = bounds.x.floor();
        let y = bounds.y.floor();
        let width = (bounds.x + bounds.width).ceil() - x;
        let height = (bounds.y + bounds.height).ceil() - y;
        let mut mask = Canvas::new(width.max(0.0) as u32, height.max(0.0) as u32);
        paint_mask(
            &mut mask,
            offset.shifted(shadow.offset_x - x, shadow.offset_y - y),
        );
        let sigma = shadow.blur * 0.5;
        if sigma > 0.5 && sigma <= 2.0 {
            mask.gaussian_blur_alpha_zero_padded(sigma);
        } else {
            mask.box_blur_alpha_radii_zero_padded(&super::super::border::gaussian_box_blur_radii(
                shadow.blur,
            ));
        }
        canvas.composite_canvas_clipped(
            &mask,
            x as i32,
            y as i32,
            color.r,
            color.g,
            color.b,
            f32::from(color.a) / 255.0,
            clip,
        );
    }
}
