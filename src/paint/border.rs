//! Border painting, box-shadow, and triangle/quad rasterization.

mod shadow_mask_cache;

use std::rc::Rc;

pub(crate) use shadow_mask_cache::{
    FrameShadowMaskCache, RasterMeasurementToken, ShadowMaskCacheSummary,
};
#[cfg(test)]
use shadow_mask_cache::{MAX_FRAME_SHADOW_MASK_ENTRIES, MAX_FRAME_SHADOW_MASK_RGBA_BYTES};
use shadow_mask_cache::{
    MAX_FRAME_SHADOW_MASK_ENTRY_RGBA_BYTES, RectBits, ShadowBlurPlanKey, ShadowMaskGeneration,
    ShadowMaskKey, ShadowMaskShapeKey,
};

use crate::css::style::PropertyId;
use crate::css::{
    ComputedStyle, ComputedValue, NineSliceComputedValue, NineSliceLength, NineSliceRepeat,
};
use crate::layout::{LayoutBox, Rect};

use super::color::{Color, parse_color};
use super::{
    BorderRegion, Canvas, Image, border_box_rect, border_color, border_radius_corners,
    fill_triangle_clipped, fill_triangle_clipped_inclusive, has_border_radius, length_property,
    normalize_rect, padding_box_rect, resolve_color_value, rounded_rect_pixel_coverage,
};

pub(crate) fn paint_borders(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    style: &ComputedStyle,
    clip: Option<Rect>,
    offset: super::PaintOffset,
) {
    if paint_border_image(canvas, layout, style, clip, offset) {
        return;
    }
    if !has_any_solid_border(style) {
        return;
    }
    let mut border_box = offset.rect(border_box_rect(layout));
    let padding_box = offset.rect(padding_box_rect(layout));
    let mut border = layout.dimensions.border;
    let table_cell = matches!(style.get_by_id(PropertyId::Display), Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("table-cell"))
        || (style.get_by_id(PropertyId::Display).is_none()
            && matches!(layout.node.tag_name().as_deref(), Some("td" | "th")));
    if table_cell
        && matches!(style.get_by_id(PropertyId::BorderCollapse), Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("collapse"))
    {
        // Collapsed cell boxes end at the grid line. Paint both halves of the
        // shared border around that line, without enlarging the measured box.
        border_box.x -= border.left;
        border_box.y -= border.top;
        border_box.width += border.left + border.right;
        border_box.height += border.top + border.bottom;
        border.top *= 2.0;
        border.right *= 2.0;
        border.bottom *= 2.0;
        border.left *= 2.0;
    }

    if has_border_radius(style) {
        // 角丸ありのボーダー: ボーダー領域（border_box の角丸内 かつ padding_box の角丸外）を
        // 各サイドの色で塗る。
        let (tl, tr, br, bl) = border_radius_corners(style);
        // 内側コーナー半径（paddingbox側）
        let inner_tl = (tl - border.left.min(border.top)).max(0.0);
        let inner_tr = (tr - border.right.min(border.top)).max(0.0);
        let inner_br = (br - border.right.min(border.bottom)).max(0.0);
        let inner_bl = (bl - border.left.min(border.bottom)).max(0.0);

        let solid_all_sides = ["top", "right", "bottom", "left"]
            .into_iter()
            .all(|side| has_solid_border_side(style, side));
        let uniform_width =
            border.top == border.right && border.top == border.bottom && border.top == border.left;
        let colors = ["top", "right", "bottom", "left"]
            .map(|side| border_color_side(style, side).unwrap_or(Color::rgb(0, 0, 0)));
        let uniform_color = colors.iter().all(|color| *color == colors[0]);
        if solid_all_sides && uniform_width && uniform_color && border.top > 0.0 {
            for region in [
                BorderRegion::Top,
                BorderRegion::Right,
                BorderRegion::Bottom,
                BorderRegion::Left,
                BorderRegion::TopLeftCorner,
                BorderRegion::TopRightCorner,
                BorderRegion::BottomRightCorner,
                BorderRegion::BottomLeftCorner,
            ] {
                canvas.fill_rounded_rect_annulus(
                    border_box,
                    tl,
                    tr,
                    br,
                    bl,
                    padding_box,
                    inner_tl,
                    inner_tr,
                    inner_br,
                    inner_bl,
                    colors[0],
                    clip,
                    region,
                    border.top,
                );
            }
            return;
        }

        // 描画順: Left/Right を先（フル高さで描画）、その後 Top/Bottom でコーナーを上書きして色が勝つ
        // left border
        if border.left > 0.0 && has_solid_border_side(style, "left") {
            let color = border_color_side(style, "left").unwrap_or(Color::rgb(0, 0, 0));
            canvas.fill_rounded_rect_annulus(
                border_box,
                tl,
                tr,
                br,
                bl,
                padding_box,
                inner_tl,
                inner_tr,
                inner_br,
                inner_bl,
                color,
                clip,
                BorderRegion::Left,
                border.left,
            );
        }
        // right border
        if border.right > 0.0 && has_solid_border_side(style, "right") {
            let color = border_color_side(style, "right").unwrap_or(Color::rgb(0, 0, 0));
            canvas.fill_rounded_rect_annulus(
                border_box,
                tl,
                tr,
                br,
                bl,
                padding_box,
                inner_tl,
                inner_tr,
                inner_br,
                inner_bl,
                color,
                clip,
                BorderRegion::Right,
                border.right,
            );
        }
        // top border（コーナー優先）
        if border.top > 0.0 && has_solid_border_side(style, "top") {
            let color = border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0));
            canvas.fill_rounded_rect_annulus(
                border_box,
                tl,
                tr,
                br,
                bl,
                padding_box,
                inner_tl,
                inner_tr,
                inner_br,
                inner_bl,
                color,
                clip,
                BorderRegion::Top,
                border.top,
            );
        }
        // bottom border（コーナー優先）
        if border.bottom > 0.0 && has_solid_border_side(style, "bottom") {
            let color = border_color_side(style, "bottom").unwrap_or(Color::rgb(0, 0, 0));
            canvas.fill_rounded_rect_annulus(
                border_box,
                tl,
                tr,
                br,
                bl,
                padding_box,
                inner_tl,
                inner_tr,
                inner_br,
                inner_bl,
                color,
                clip,
                BorderRegion::Bottom,
                border.bottom,
            );
        }
        return;
    }

    if border.top > 0.0 && has_solid_border_side(style, "top") {
        canvas.fill_rect_clipped(
            Rect {
                x: border_box.x,
                y: border_box.y,
                width: border_box.width,
                height: border.top,
            },
            border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.bottom > 0.0 && has_solid_border_side(style, "bottom") {
        canvas.fill_rect_clipped(
            Rect {
                x: border_box.x,
                y: padding_box.y + padding_box.height,
                width: border_box.width,
                height: border.bottom,
            },
            border_color_side(style, "bottom").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.left > 0.0 && has_solid_border_side(style, "left") {
        canvas.fill_rect_clipped(
            Rect {
                x: border_box.x,
                y: border_box.y + border.top,
                width: border.left,
                height: border_box.height - border.top - border.bottom,
            },
            border_color_side(style, "left").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.right > 0.0 && has_solid_border_side(style, "right") {
        canvas.fill_rect_clipped(
            Rect {
                x: padding_box.x + padding_box.width,
                y: border_box.y + border.top,
                width: border.right,
                height: border_box.height - border.top - border.bottom,
            },
            border_color_side(style, "right").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
}

#[derive(Clone, Copy)]
struct Four<T> {
    top: T,
    right: T,
    bottom: T,
    left: T,
}

fn four<T: Clone>(values: &[T]) -> Option<Four<T>> {
    Some(match values {
        [all] => Four {
            top: all.clone(),
            right: all.clone(),
            bottom: all.clone(),
            left: all.clone(),
        },
        [vertical, horizontal] => Four {
            top: vertical.clone(),
            right: horizontal.clone(),
            bottom: vertical.clone(),
            left: horizontal.clone(),
        },
        [top, horizontal, bottom] => Four {
            top: top.clone(),
            right: horizontal.clone(),
            bottom: bottom.clone(),
            left: horizontal.clone(),
        },
        [top, right, bottom, left] => Four {
            top: top.clone(),
            right: right.clone(),
            bottom: bottom.clone(),
            left: left.clone(),
        },
        _ => return None,
    })
}

fn nine_slice_lengths(style: &ComputedStyle, id: PropertyId) -> Option<Vec<NineSliceLength>> {
    match style.get_by_id(id) {
        Some(ComputedValue::NineSlice(NineSliceComputedValue::Width(values)))
        | Some(ComputedValue::NineSlice(NineSliceComputedValue::Outset(values))) => {
            Some(values.clone())
        }
        Some(ComputedValue::Number(value)) => Some(vec![NineSliceLength::Number(*value)]),
        Some(ComputedValue::Percentage(value)) => Some(vec![NineSliceLength::Percentage(*value)]),
        Some(ComputedValue::Px(value)) => Some(vec![NineSliceLength::Px(*value)]),
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("auto") => {
            Some(vec![NineSliceLength::Auto])
        }
        _ => None,
    }
}

fn nine_slice_slice(style: &ComputedStyle, id: PropertyId) -> Option<(Vec<NineSliceLength>, bool)> {
    match style.get_by_id(id) {
        Some(ComputedValue::NineSlice(NineSliceComputedValue::Slice { values, fill })) => {
            Some((values.clone(), *fill))
        }
        Some(ComputedValue::Number(value)) => Some((vec![NineSliceLength::Number(*value)], false)),
        Some(ComputedValue::Percentage(value)) => {
            Some((vec![NineSliceLength::Percentage(*value)], false))
        }
        Some(ComputedValue::Keyword(value)) => value
            .strip_suffix('%')
            .and_then(|value| value.parse::<f32>().ok())
            .map(|value| (vec![NineSliceLength::Percentage(value)], false)),
        _ => None,
    }
}

fn nine_slice_repeat(style: &ComputedStyle, id: PropertyId) -> Option<Vec<NineSliceRepeat>> {
    match style.get_by_id(id) {
        Some(ComputedValue::NineSlice(NineSliceComputedValue::Repeat(values))) => {
            Some(values.clone())
        }
        Some(ComputedValue::Keyword(value)) => value
            .split_ascii_whitespace()
            .map(|value| match value.to_ascii_lowercase().as_str() {
                "stretch" => Some(NineSliceRepeat::Stretch),
                "repeat" => Some(NineSliceRepeat::Repeat),
                "round" => Some(NineSliceRepeat::Round),
                "space" => Some(NineSliceRepeat::Space),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn source_image(style: &ComputedStyle, source_id: PropertyId, area: Rect) -> Option<Image> {
    let text = match style.get_by_id(source_id) {
        Some(ComputedValue::Keyword(value))
        | Some(ComputedValue::String(value))
        | Some(ComputedValue::Color(value)) => value,
        _ => return None,
    };
    if text.eq_ignore_ascii_case("none") {
        return None;
    }
    if let Some(gradient) = super::color::parse_gradient(text) {
        let width = area.width.ceil().max(1.0) as u32;
        let height = area.height.ceil().max(1.0) as u32;
        let mut source = Canvas::new(width, height);
        super::color::paint_gradient(
            &mut source,
            &gradient,
            Rect {
                x: 0.0,
                y: 0.0,
                width: width as f32,
                height: height as f32,
            },
            None,
        );
        return Image::new(width, height, source.pixels).ok();
    }
    super::image::parse_background_image_value(text)
}

fn source_slice(value: &NineSliceLength, basis: f32) -> f32 {
    match value {
        NineSliceLength::Number(value) | NineSliceLength::Px(value) => *value,
        NineSliceLength::Percentage(value) => basis * value / 100.0,
        NineSliceLength::Math(value) => value.resolve(basis),
        NineSliceLength::Auto => 0.0,
    }
    .clamp(0.0, basis)
}

fn border_image_width(value: &NineSliceLength, border: f32, basis: f32, auto: f32) -> f32 {
    match value {
        NineSliceLength::Number(value) => value * border,
        NineSliceLength::Percentage(value) => basis * value / 100.0,
        NineSliceLength::Px(value) => *value,
        NineSliceLength::Math(value) => value.resolve(basis),
        NineSliceLength::Auto => auto,
    }
    .max(0.0)
}

fn border_image_outset(value: &NineSliceLength, border: f32) -> f32 {
    match value {
        NineSliceLength::Number(value) => value * border,
        NineSliceLength::Px(value) => *value,
        NineSliceLength::Math(value) => value.resolve(0.0),
        NineSliceLength::Percentage(_) | NineSliceLength::Auto => 0.0,
    }
    .max(0.0)
}

fn fit_opposing(start: &mut f32, end: &mut f32, available: f32) {
    let total = *start + *end;
    if total > available && total > 0.0 {
        let scale = available / total;
        *start *= scale;
        *end *= scale;
    }
}

fn draw_source_rect(
    canvas: &mut Canvas,
    image: &Image,
    source: Rect,
    destination: Rect,
    clip: Option<Rect>,
) {
    if source.width <= 0.0
        || source.height <= 0.0
        || destination.width <= 0.0
        || destination.height <= 0.0
    {
        return;
    }
    let scale_x = destination.width / source.width;
    let scale_y = destination.height / source.height;
    let mapped = Rect {
        x: destination.x - source.x * scale_x,
        y: destination.y - source.y * scale_y,
        width: image.width() as f32 * scale_x,
        height: image.height() as f32 * scale_y,
    };
    let tile_clip = match clip {
        Some(clip) => super::intersect(destination, clip),
        None => Some(destination),
    };
    if let Some(tile_clip) = tile_clip {
        canvas.draw_image_scaled_clipped(image, mapped, Some(tile_clip));
    }
}

fn axis_tiles(start: f32, length: f32, natural: f32, repeat: NineSliceRepeat) -> Vec<(f32, f32)> {
    if length <= 0.0 || natural <= 0.0 {
        return Vec::new();
    }
    match repeat {
        NineSliceRepeat::Stretch => vec![(start, length)],
        NineSliceRepeat::Repeat => {
            let count = (length / natural).ceil().max(1.0) as usize;
            let offset = (length - count as f32 * natural) / 2.0;
            (0..count)
                .map(|index| (start + offset + index as f32 * natural, natural))
                .collect()
        }
        NineSliceRepeat::Round => {
            let count = (length / natural).round().max(1.0) as usize;
            let size = length / count as f32;
            (0..count)
                .map(|index| (start + index as f32 * size, size))
                .collect()
        }
        NineSliceRepeat::Space => {
            let count = (length / natural).floor() as usize;
            if count <= 1 {
                return vec![(start + (length - natural) / 2.0, natural.min(length))];
            }
            let gap = (length - count as f32 * natural) / (count - 1) as f32;
            (0..count)
                .map(|index| (start + index as f32 * (natural + gap), natural))
                .collect()
        }
    }
}

#[cfg(test)]
mod nine_slice_tests {
    use super::*;

    #[test]
    fn repeat_modes_generate_specified_axis_geometry() {
        assert_eq!(
            axis_tiles(10.0, 20.0, 6.0, NineSliceRepeat::Stretch),
            vec![(10.0, 20.0)]
        );
        assert_eq!(
            axis_tiles(10.0, 20.0, 6.0, NineSliceRepeat::Repeat),
            vec![(8.0, 6.0), (14.0, 6.0), (20.0, 6.0), (26.0, 6.0)]
        );
        assert_eq!(
            axis_tiles(10.0, 20.0, 6.0, NineSliceRepeat::Round),
            vec![
                (10.0, 20.0 / 3.0),
                (10.0 + 20.0 / 3.0, 20.0 / 3.0),
                (10.0 + 40.0 / 3.0, 20.0 / 3.0),
            ]
        );
        assert_eq!(
            axis_tiles(10.0, 20.0, 6.0, NineSliceRepeat::Space),
            vec![(10.0, 6.0), (17.0, 6.0), (24.0, 6.0)]
        );
    }

    #[test]
    fn opposing_slices_and_widths_shrink_by_one_common_factor() {
        let (mut start, mut end) = (8.0, 12.0);
        fit_opposing(&mut start, &mut end, 10.0);
        assert_eq!((start, end), (4.0, 6.0));
    }

    #[test]
    fn repeat_geometry_scales_with_device_pixel_ratio() {
        for repeat in [
            NineSliceRepeat::Stretch,
            NineSliceRepeat::Repeat,
            NineSliceRepeat::Round,
            NineSliceRepeat::Space,
        ] {
            let css_pixels = axis_tiles(3.0, 24.0, 6.0, repeat);
            let device_pixels = axis_tiles(6.0, 48.0, 12.0, repeat);
            assert_eq!(css_pixels.len(), device_pixels.len(), "{repeat:?}");
            for ((css_start, css_size), (device_start, device_size)) in
                css_pixels.into_iter().zip(device_pixels)
            {
                assert_eq!(device_start, css_start * 2.0, "{repeat:?}");
                assert_eq!(device_size, css_size * 2.0, "{repeat:?}");
            }
        }
    }
}

fn draw_tiled_region(
    canvas: &mut Canvas,
    image: &Image,
    source: Rect,
    destination: Rect,
    repeat_x: NineSliceRepeat,
    repeat_y: NineSliceRepeat,
    clip: Option<Rect>,
) {
    let natural_width = source.width * destination.height / source.height.max(1.0);
    let natural_height = source.height * destination.width / source.width.max(1.0);
    let xs = axis_tiles(destination.x, destination.width, natural_width, repeat_x);
    let ys = axis_tiles(destination.y, destination.height, natural_height, repeat_y);
    for (x, width) in xs {
        for &(y, height) in &ys {
            draw_source_rect(
                canvas,
                image,
                source,
                Rect {
                    x,
                    y,
                    width,
                    height,
                },
                Some(destination).and_then(|area| match clip {
                    Some(clip) => super::intersect(area, clip),
                    None => Some(area),
                }),
            );
        }
    }
}

#[derive(Clone, Copy)]
struct NineSliceProperties {
    source: PropertyId,
    slice: PropertyId,
    width: PropertyId,
    outset: PropertyId,
    repeat: PropertyId,
}

const BORDER_IMAGE_PROPERTIES: NineSliceProperties = NineSliceProperties {
    source: PropertyId::BorderImageSource,
    slice: PropertyId::BorderImageSlice,
    width: PropertyId::BorderImageWidth,
    outset: PropertyId::BorderImageOutset,
    repeat: PropertyId::BorderImageRepeat,
};

const MASK_BORDER_PROPERTIES: NineSliceProperties = NineSliceProperties {
    source: PropertyId::MaskBorderSource,
    slice: PropertyId::MaskBorderSlice,
    width: PropertyId::MaskBorderWidth,
    outset: PropertyId::MaskBorderOutset,
    repeat: PropertyId::MaskBorderRepeat,
};

fn paint_border_image(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    style: &ComputedStyle,
    clip: Option<Rect>,
    offset: super::PaintOffset,
) -> bool {
    let border_box = offset.rect(border_box_rect(layout));
    let border = layout.dimensions.border;
    paint_nine_slice_image(
        canvas,
        style,
        BORDER_IMAGE_PROPERTIES,
        border_box,
        Four {
            top: border.top,
            right: border.right,
            bottom: border.bottom,
            left: border.left,
        },
        clip,
    )
}

fn paint_nine_slice_image(
    canvas: &mut Canvas,
    style: &ComputedStyle,
    properties: NineSliceProperties,
    border_box: Rect,
    border: Four<f32>,
    clip: Option<Rect>,
) -> bool {
    let Some(image) = source_image(style, properties.source, border_box) else {
        return false;
    };
    let Some((slice_values, fill)) = nine_slice_slice(style, properties.slice) else {
        return false;
    };
    let Some(slice) = four(&slice_values) else {
        return false;
    };
    let source_width = image.width() as f32;
    let source_height = image.height() as f32;
    let mut source = Four {
        top: source_slice(&slice.top, source_height),
        right: source_slice(&slice.right, source_width),
        bottom: source_slice(&slice.bottom, source_height),
        left: source_slice(&slice.left, source_width),
    };
    fit_opposing(&mut source.left, &mut source.right, source_width);
    fit_opposing(&mut source.top, &mut source.bottom, source_height);
    let Some(width_values) = nine_slice_lengths(style, properties.width) else {
        return false;
    };
    let Some(outset_values) = nine_slice_lengths(style, properties.outset) else {
        return false;
    };
    let Some(width_values) = four(&width_values) else {
        return false;
    };
    let Some(outset_values) = four(&outset_values) else {
        return false;
    };
    let outset = Four {
        top: border_image_outset(&outset_values.top, border.top),
        right: border_image_outset(&outset_values.right, border.right),
        bottom: border_image_outset(&outset_values.bottom, border.bottom),
        left: border_image_outset(&outset_values.left, border.left),
    };
    let area = Rect {
        x: border_box.x - outset.left,
        y: border_box.y - outset.top,
        width: border_box.width + outset.left + outset.right,
        height: border_box.height + outset.top + outset.bottom,
    };
    let mut widths = Four {
        top: border_image_width(&width_values.top, border.top, area.height, source.top),
        right: border_image_width(&width_values.right, border.right, area.width, source.right),
        bottom: border_image_width(
            &width_values.bottom,
            border.bottom,
            area.height,
            source.bottom,
        ),
        left: border_image_width(&width_values.left, border.left, area.width, source.left),
    };
    fit_opposing(&mut widths.left, &mut widths.right, area.width);
    fit_opposing(&mut widths.top, &mut widths.bottom, area.height);
    let Some(repeat) = nine_slice_repeat(style, properties.repeat) else {
        return false;
    };
    let repeat_x = repeat.first().copied().unwrap_or(NineSliceRepeat::Stretch);
    let repeat_y = repeat.get(1).copied().unwrap_or(repeat_x);
    let sx = [0.0, source.left, source_width - source.right, source_width];
    let sy = [
        0.0,
        source.top,
        source_height - source.bottom,
        source_height,
    ];
    let dx = [
        area.x,
        area.x + widths.left,
        area.x + area.width - widths.right,
        area.x + area.width,
    ];
    let dy = [
        area.y,
        area.y + widths.top,
        area.y + area.height - widths.bottom,
        area.y + area.height,
    ];
    for row in 0..3 {
        for column in 0..3 {
            if row == 1 && column == 1 && !fill {
                continue;
            }
            let source_rect = Rect {
                x: sx[column],
                y: sy[row],
                width: (sx[column + 1] - sx[column]).max(0.0),
                height: (sy[row + 1] - sy[row]).max(0.0),
            };
            let destination = Rect {
                x: dx[column],
                y: dy[row],
                width: (dx[column + 1] - dx[column]).max(0.0),
                height: (dy[row + 1] - dy[row]).max(0.0),
            };
            let tile_x = if column == 1 {
                repeat_x
            } else {
                NineSliceRepeat::Stretch
            };
            let tile_y = if row == 1 {
                repeat_y
            } else {
                NineSliceRepeat::Stretch
            };
            draw_tiled_region(
                canvas,
                &image,
                source_rect,
                destination,
                tile_x,
                tile_y,
                clip,
            );
        }
    }
    true
}

pub(super) fn apply_mask_border_alpha(
    canvas: &mut Canvas,
    style: &ComputedStyle,
    border_box: Rect,
) -> bool {
    if !matches!(
        style.get_by_id(PropertyId::MaskBorderSource),
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value))
            if !value.eq_ignore_ascii_case("none")
    ) {
        return false;
    }
    let border = EdgeSizesForPaint::from_style(style);
    let mut mask = Canvas::new(canvas.width(), canvas.height());
    if !paint_nine_slice_image(
        &mut mask,
        style,
        MASK_BORDER_PROPERTIES,
        border_box,
        Four {
            top: border.top,
            right: border.right,
            bottom: border.bottom,
            left: border.left,
        },
        None,
    ) {
        return false;
    }
    let luminance = matches!(
        style.get_by_id(PropertyId::MaskBorderMode),
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("luminance")
    );
    for (destination, source) in canvas
        .pixels
        .chunks_exact_mut(4)
        .zip(mask.pixels.chunks_exact(4))
    {
        let alpha = if luminance {
            let lightness = (u32::from(source[0]) * 2126
                + u32::from(source[1]) * 7152
                + u32::from(source[2]) * 722
                + 5_000)
                / 10_000;
            (lightness * u32::from(source[3]) / 255) as u8
        } else {
            source[3]
        };
        destination[3] = ((u16::from(destination[3]) * u16::from(alpha) + 127) / 255) as u8;
    }
    true
}

pub(crate) fn has_solid_border_side(style: &ComputedStyle, side: &str) -> bool {
    let value = match side {
        "top" => style.get_by_id(PropertyId::BorderTopStyle),
        "right" => style.get_by_id(PropertyId::BorderRightStyle),
        "bottom" => style.get_by_id(PropertyId::BorderBottomStyle),
        "left" => style.get_by_id(PropertyId::BorderLeftStyle),
        _ => style.get(&format!("border-{side}-style")),
    };
    if matches!(
        value,
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("solid")
    ) {
        return true;
    }

    matches!(
        style.get("border-style"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("solid")
    )
}

pub(crate) fn border_color_side(style: &ComputedStyle, side: &str) -> Option<Color> {
    let value = match side {
        "top" => style.get_by_id(PropertyId::BorderTopColor),
        "right" => style.get_by_id(PropertyId::BorderRightColor),
        "bottom" => style.get_by_id(PropertyId::BorderBottomColor),
        "left" => style.get_by_id(PropertyId::BorderLeftColor),
        _ => style.get(&format!("border-{side}-color")),
    };
    resolve_color_value(value, style).or_else(|| border_color(style))
}

#[derive(Clone, Copy)]
pub(crate) struct EdgeSizesForPaint {
    pub(crate) top: f32,
    pub(crate) right: f32,
    pub(crate) bottom: f32,
    pub(crate) left: f32,
}

impl EdgeSizesForPaint {
    pub(crate) fn from_style(style: &ComputedStyle) -> Self {
        Self {
            top: length_property(style, "border-top-width")
                .or_else(|| length_property(style, "border-width"))
                .unwrap_or(0.0),
            right: length_property(style, "border-right-width")
                .or_else(|| length_property(style, "border-width"))
                .unwrap_or(0.0),
            bottom: length_property(style, "border-bottom-width")
                .or_else(|| length_property(style, "border-width"))
                .unwrap_or(0.0),
            left: length_property(style, "border-left-width")
                .or_else(|| length_property(style, "border-width"))
                .unwrap_or(0.0),
        }
    }

    pub(crate) fn total_horizontal(self) -> f32 {
        self.left + self.right
    }

    pub(crate) fn total_vertical(self) -> f32 {
        self.top + self.bottom
    }
}

pub(crate) fn paint_rect_borders(
    canvas: &mut Canvas,
    rect: Rect,
    style: &ComputedStyle,
    border: EdgeSizesForPaint,
    clip: Option<Rect>,
) {
    if border.top > 0.0 && has_solid_border_side(style, "top") {
        canvas.fill_rect_clipped(
            Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: border.top,
            },
            border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.bottom > 0.0 && has_solid_border_side(style, "bottom") {
        canvas.fill_rect_clipped(
            Rect {
                x: rect.x,
                y: rect.y + rect.height - border.bottom,
                width: rect.width,
                height: border.bottom,
            },
            border_color_side(style, "bottom").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.left > 0.0 && has_solid_border_side(style, "left") {
        canvas.fill_rect_clipped(
            Rect {
                x: rect.x,
                y: rect.y + border.top,
                width: border.left,
                height: (rect.height - border.top - border.bottom).max(0.0),
            },
            border_color_side(style, "left").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.right > 0.0 && has_solid_border_side(style, "right") {
        canvas.fill_rect_clipped(
            Rect {
                x: rect.x + rect.width - border.right,
                y: rect.y + border.top,
                width: border.right,
                height: (rect.height - border.top - border.bottom).max(0.0),
            },
            border_color_side(style, "right").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
}

pub(crate) fn paint_zero_sized_border_box(
    canvas: &mut Canvas,
    rect: Rect,
    style: &ComputedStyle,
    border: EdgeSizesForPaint,
    clip: Option<Rect>,
) {
    let inner_left = rect.x + border.left;
    let inner_top = rect.y + border.top;
    let inner_right = rect.x + rect.width - border.right;
    let inner_bottom = rect.y + rect.height - border.bottom;

    let paint_top = |canvas: &mut Canvas| {
        if border.top > 0.0 && has_solid_border_side(style, "top") {
            fill_quad_clipped(
                canvas,
                (rect.x, rect.y),
                (rect.x + rect.width, rect.y),
                (inner_right, inner_top),
                (inner_left, inner_top),
                border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0)),
                clip,
            );
        }
    };
    let paint_bottom = |canvas: &mut Canvas| {
        if border.bottom > 0.0 && has_solid_border_side(style, "bottom") {
            fill_quad_clipped(
                canvas,
                (rect.x, rect.y + rect.height),
                (rect.x + rect.width, rect.y + rect.height),
                (inner_right, inner_bottom),
                (inner_left, inner_bottom),
                border_color_side(style, "bottom").unwrap_or(Color::rgb(0, 0, 0)),
                clip,
            );
        }
    };
    let paint_left = |canvas: &mut Canvas| {
        if border.left > 0.0 && has_solid_border_side(style, "left") {
            fill_quad_clipped(
                canvas,
                (rect.x, rect.y),
                (rect.x, rect.y + rect.height),
                (inner_left, inner_bottom),
                (inner_left, inner_top),
                border_color_side(style, "left").unwrap_or(Color::rgb(0, 0, 0)),
                clip,
            );
        }
    };
    let paint_right = |canvas: &mut Canvas| {
        if border.right > 0.0 && has_solid_border_side(style, "right") {
            fill_quad_clipped(
                canvas,
                (rect.x + rect.width, rect.y),
                (rect.x + rect.width, rect.y + rect.height),
                (inner_right, inner_bottom),
                (inner_right, inner_top),
                border_color_side(style, "right").unwrap_or(Color::rgb(0, 0, 0)),
                clip,
            );
        }
    };

    if border.top == 0.0 && border.bottom > 0.0 {
        paint_bottom(canvas);
        paint_left(canvas);
        paint_right(canvas);
    } else if border.bottom == 0.0 && border.top > 0.0 {
        paint_left(canvas);
        paint_right(canvas);
        if has_solid_border_side(style, "top") {
            let color = border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0));
            fill_triangle_clipped_inclusive(
                canvas,
                (rect.x, rect.y),
                (rect.x + rect.width, rect.y),
                (inner_right, inner_top),
                color,
                clip,
            );
            fill_triangle_clipped_inclusive(
                canvas,
                (rect.x, rect.y),
                (inner_right, inner_top),
                (inner_left, inner_top),
                color,
                clip,
            );
        }
    } else {
        paint_top(canvas);
        paint_bottom(canvas);
        paint_left(canvas);
        paint_right(canvas);
    }
}

pub(crate) fn fill_quad_clipped(
    canvas: &mut Canvas,
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    p4: (f32, f32),
    color: Color,
    clip: Option<Rect>,
) {
    fill_triangle_clipped(canvas, p1, p2, p3, color, clip);
    fill_triangle_clipped(canvas, p1, p3, p4, color, clip);
}

pub(crate) fn has_any_solid_border(style: &ComputedStyle) -> bool {
    ["top", "right", "bottom", "left"]
        .into_iter()
        .any(|side| has_solid_border_side(style, side))
}

/// `box-shadow` 宣言から解析した影のパラメータ。
#[derive(Debug, Clone)]
pub(crate) struct BoxShadow {
    pub(crate) offset_x: f32,
    pub(crate) offset_y: f32,
    pub(crate) blur_radius: f32,
    pub(crate) spread_radius: f32,
    pub(crate) color: Color,
    pub(crate) inset: bool,
}

/// `box-shadow` プロパティ値の文字列を解析して `BoxShadow` のリストを返す。
///
/// 書式: `[inset] <offset-x> <offset-y> [<blur>] [<spread>] [<color>]`
/// 複数の影はカンマ区切りで指定できる（rgba()/rgb() など関数の引数内のカンマは区切りとして扱わない）。
pub(crate) fn parse_box_shadow(value: &str) -> Vec<BoxShadow> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("none") || trimmed.is_empty() {
        return Vec::new();
    }

    // カンマ区切りで複数の影に分割（rgba() 内のカンマは無視）
    let shadow_strs = split_box_shadow_layers(trimmed);

    let mut shadows = Vec::new();
    for shadow_str in &shadow_strs {
        if let Some(shadow) = parse_single_box_shadow(shadow_str.trim()) {
            shadows.push(shadow);
        }
    }
    shadows
}

/// box-shadow 値をカンマで分割する（関数内のカンマは無視）。
pub(crate) fn split_box_shadow_layers(value: &str) -> Vec<String> {
    let mut layers = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for ch in value.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                layers.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        layers.push(current.trim().to_string());
    }
    layers
}

/// 単一の box-shadow トークン列を解析する。
fn parse_single_box_shadow(value: &str) -> Option<BoxShadow> {
    // トークン化: 数値（px付き）、色文字列、キーワードに分割
    let tokens = tokenize_shadow_value(value);

    let mut inset = false;
    let mut lengths: Vec<f32> = Vec::new();
    let mut color: Option<Color> = None;

    for token in &tokens {
        let lower = token.to_ascii_lowercase();
        if lower == "inset" {
            inset = true;
        } else if let Some(px) = parse_shadow_length(token) {
            lengths.push(px);
        } else if let Some(c) = parse_color(token) {
            color = Some(c);
        }
    }

    // offset-x, offset-y は必須
    if lengths.len() < 2 {
        return None;
    }

    let offset_x = lengths[0];
    let offset_y = lengths[1];
    let blur_radius = lengths.get(2).copied().unwrap_or(0.0).max(0.0);
    let spread_radius = lengths.get(3).copied().unwrap_or(0.0);
    let color = color.unwrap_or(Color::rgba(0, 0, 0, 255));

    Some(BoxShadow {
        offset_x,
        offset_y,
        blur_radius,
        spread_radius,
        color,
        inset,
    })
}

/// shadow 値文字列を空白で分割（関数呼び出しはひとまとめに）。
fn tokenize_shadow_value(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for ch in value.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
                if depth == 0 && !current.is_empty() {
                    tokens.push(current.trim().to_string());
                    current.clear();
                }
            }
            ' ' | '\t' if depth == 0 => {
                let tok = current.trim().to_string();
                if !tok.is_empty() {
                    tokens.push(tok);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let tok = current.trim().to_string();
    if !tok.is_empty() {
        tokens.push(tok);
    }
    tokens
}

/// CSS length 文字列（例: "10px", "-5px", "0"）を px 値として解析する。
fn parse_shadow_length(s: &str) -> Option<f32> {
    let s = s.trim();
    if let Some(stripped) = s.strip_suffix("px") {
        stripped.parse::<f32>().ok()
    } else if s == "0" {
        Some(0.0)
    } else {
        // 単純な数値（単位なし）も許容
        let v: Result<f32, _> = s.parse();
        v.ok()
    }
}

/// box-shadow をキャンバスに描画する（背景・ボーダーの前に描画）。
pub(crate) fn paint_box_shadow(
    canvas: &mut Canvas,
    style: &ComputedStyle,
    border_box: Rect,
    clip: Option<Rect>,
    shadow_masks: Option<&FrameShadowMaskCache>,
) {
    let shadow_value = match style.get_by_id(PropertyId::BoxShadow) {
        Some(ComputedValue::Keyword(v)) => v.clone(),
        _ => return,
    };

    let shadows = parse_box_shadow(&shadow_value);
    let radii = if has_border_radius(style) {
        let (tl, tr, br, bl) = border_radius_corners(style);
        Some((tl, tr, br, bl))
    } else {
        None
    };

    // CSS 仕様では後ろに書いた影ほど下に描画される（逆順で描画）
    for shadow in shadows.iter().rev() {
        if shadow.inset {
            // inset shadow は未実装（スキップ）
            continue;
        }
        paint_outer_box_shadow_cached(canvas, border_box, shadow, clip, radii, shadow_masks);
    }
}

fn shadow_buffer_intersects_canvas(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    canvas_width: u32,
    canvas_height: u32,
) -> bool {
    if width == 0 || height == 0 || canvas_width == 0 || canvas_height == 0 {
        return false;
    }
    if canvas_width > i32::MAX as u32 || canvas_height > i32::MAX as u32 {
        return true;
    }
    let right = i64::from(x) + i64::from(width);
    let bottom = i64::from(y) + i64::from(height);
    right > 0
        && bottom > 0
        && i64::from(x) < i64::from(canvas_width)
        && i64::from(y) < i64::from(canvas_height)
}

/// アウター box-shadow を描画する。
/// border_box の内側は描画しない（外側のみ）。
pub(crate) fn paint_outer_box_shadow(
    canvas: &mut Canvas,
    border_box: Rect,
    shadow: &BoxShadow,
    clip: Option<Rect>,
    radii: Option<(f32, f32, f32, f32)>,
) {
    paint_outer_box_shadow_cached(canvas, border_box, shadow, clip, radii, None);
}

#[derive(Clone, Copy)]
enum ShadowMaskShape {
    Rect,
    Rounded {
        source_radii: [f32; 4],
        knockout_radii: [f32; 4],
    },
}

#[derive(Clone, Copy)]
enum ShadowBlurPlan {
    RoundedZeroBlur,
    DirectGaussian { sigma: f32 },
    Box { ordered_radii: [u32; 3] },
}

struct ShadowMaskSpec {
    buf_x: i32,
    buf_y: i32,
    buf_w: u32,
    buf_h: u32,
    local_rect: Rect,
    border_local: Rect,
    spread: f32,
    blur_radius: f32,
    shape: ShadowMaskShape,
    blur: ShadowBlurPlan,
}

impl ShadowMaskSpec {
    fn cache_entry(&self) -> Option<(ShadowMaskKey, usize)> {
        if !self.spread.is_finite() || !self.blur_radius.is_finite() {
            return None;
        }
        let shape = match self.shape {
            ShadowMaskShape::Rect => ShadowMaskShapeKey::Rect,
            ShadowMaskShape::Rounded {
                source_radii,
                knockout_radii,
            } => {
                if !source_radii
                    .iter()
                    .chain(knockout_radii.iter())
                    .all(|radius| radius.is_finite())
                {
                    return None;
                }
                ShadowMaskShapeKey::Rounded {
                    source_radii: source_radii.map(f32::to_bits),
                    knockout_radii: knockout_radii.map(f32::to_bits),
                }
            }
        };
        let blur = match self.blur {
            ShadowBlurPlan::RoundedZeroBlur => ShadowBlurPlanKey::RoundedZeroBlur,
            ShadowBlurPlan::DirectGaussian { sigma } if sigma.is_finite() => {
                ShadowBlurPlanKey::DirectGaussian {
                    sigma_bits: sigma.to_bits(),
                }
            }
            ShadowBlurPlan::DirectGaussian { .. } => return None,
            ShadowBlurPlan::Box { ordered_radii } => ShadowBlurPlanKey::Box { ordered_radii },
        };
        let rgba_bytes = usize::try_from(self.buf_w)
            .ok()?
            .checked_mul(usize::try_from(self.buf_h).ok()?)?
            .checked_mul(4)?;
        Some((
            ShadowMaskKey {
                policy_version: 1,
                allocated_width: self.buf_w,
                allocated_height: self.buf_h,
                local_rect: RectBits::from_finite(self.local_rect)?,
                border_local: RectBits::from_finite(self.border_local)?,
                spread_bits: self.spread.to_bits(),
                blur_radius_bits: self.blur_radius.to_bits(),
                shape,
                blur,
            },
            rgba_bytes,
        ))
    }

    fn knockout_radii(&self) -> Option<(f32, f32, f32, f32)> {
        match self.shape {
            ShadowMaskShape::Rect => None,
            ShadowMaskShape::Rounded {
                knockout_radii: [tl, tr, br, bl],
                ..
            } => Some((tl, tr, br, bl)),
        }
    }
}

fn shadow_mask_spec(
    border_box: Rect,
    shadow_rect: Rect,
    shadow: &BoxShadow,
    radii: Option<(f32, f32, f32, f32)>,
) -> ShadowMaskSpec {
    let blur_radius = shadow.blur_radius;
    let spread = shadow.spread_radius;
    let sigma = blur_radius * 0.5;
    let direct_gaussian = blur_radius > 0.0 && sigma > 0.5 && sigma <= 2.0;
    let ordered_radii = gaussian_box_blur_radii(blur_radius);
    let (blur, margin) = if blur_radius <= 0.0 {
        (ShadowBlurPlan::RoundedZeroBlur, 0)
    } else if direct_gaussian {
        (
            ShadowBlurPlan::DirectGaussian { sigma },
            ((sigma * 3.0).ceil() as u32).saturating_add(1),
        )
    } else {
        (
            ShadowBlurPlan::Box { ordered_radii },
            ordered_radii
                .iter()
                .copied()
                .fold(0u32, u32::saturating_add)
                .saturating_add(1),
        )
    };
    let margin_f = margin as f32;
    let buf_x = (shadow_rect.x - margin_f).floor() as i32;
    let buf_y = (shadow_rect.y - margin_f).floor() as i32;
    let buf_w = (shadow_rect.width + margin_f * 2.0).ceil() as u32 + 2;
    let buf_h = (shadow_rect.height + margin_f * 2.0).ceil() as u32 + 2;
    let local_rect = Rect {
        x: shadow_rect.x - buf_x as f32,
        y: shadow_rect.y - buf_y as f32,
        width: shadow_rect.width,
        height: shadow_rect.height,
    };
    let border_local = Rect {
        x: border_box.x - buf_x as f32,
        y: border_box.y - buf_y as f32,
        width: border_box.width,
        height: border_box.height,
    };
    let shape = radii.map_or(ShadowMaskShape::Rect, |(tl, tr, br, bl)| {
        ShadowMaskShape::Rounded {
            source_radii: [tl + spread, tr + spread, br + spread, bl + spread],
            knockout_radii: [tl, tr, br, bl],
        }
    });
    ShadowMaskSpec {
        buf_x,
        buf_y,
        buf_w,
        buf_h,
        local_rect,
        border_local,
        spread,
        blur_radius,
        shape,
        blur,
    }
}

fn render_shadow_mask(spec: &ShadowMaskSpec) -> Canvas {
    let mut mask = Canvas::new(spec.buf_w, spec.buf_h);
    match spec.shape {
        ShadowMaskShape::Rect => {
            mask.fill_rect(spec.local_rect, Color::rgba(255, 255, 255, 255));
        }
        ShadowMaskShape::Rounded { source_radii, .. } => mask.fill_rounded_rect(
            spec.local_rect,
            Color::rgba(255, 255, 255, 255),
            source_radii[0],
            source_radii[1],
            source_radii[2],
            source_radii[3],
            None,
        ),
    }
    match spec.blur {
        ShadowBlurPlan::RoundedZeroBlur => {}
        ShadowBlurPlan::DirectGaussian { sigma } => mask.gaussian_blur_alpha_zero_padded(sigma),
        ShadowBlurPlan::Box { ordered_radii } => {
            mask.box_blur_alpha_radii_zero_padded(&ordered_radii);
        }
    }
    knock_out_shadow_interior(&mut mask, spec.border_local, spec.knockout_radii());
    mask
}

fn composite_shadow_mask(
    canvas: &mut Canvas,
    mask: &Canvas,
    alpha_spans: Option<&std::rc::Rc<crate::paint::AlphaRowSpans>>,
    spec: &ShadowMaskSpec,
    color: Color,
    clip: Option<Rect>,
) {
    if let Some(alpha_spans) = alpha_spans {
        canvas.composite_canvas_clipped_with_spans(
            mask,
            alpha_spans,
            spec.buf_x,
            spec.buf_y,
            color.r,
            color.g,
            color.b,
            color.a as f32 / 255.0,
            clip,
        );
    } else {
        canvas.composite_canvas_clipped(
            mask,
            spec.buf_x,
            spec.buf_y,
            color.r,
            color.g,
            color.b,
            color.a as f32 / 255.0,
            clip,
        );
    }
}

fn paint_shadow_mask(
    canvas: &mut Canvas,
    spec: ShadowMaskSpec,
    color: Color,
    clip: Option<Rect>,
    shadow_masks: Option<&FrameShadowMaskCache>,
) {
    let mut eligible_miss = None;
    if let Some(cache) = shadow_masks {
        match spec.cache_entry() {
            None => cache.record_generation(ShadowMaskGeneration::Unkeyable),
            Some((_, rgba_bytes)) if rgba_bytes > MAX_FRAME_SHADOW_MASK_ENTRY_RGBA_BYTES => {
                cache.record_generation(ShadowMaskGeneration::Oversize);
            }
            Some((key, rgba_bytes)) => {
                if let Some((mask, alpha_spans)) = cache.lookup(&key, rgba_bytes) {
                    composite_shadow_mask(canvas, &mask, alpha_spans.as_ref(), &spec, color, clip);
                    return;
                }
                cache.record_generation(ShadowMaskGeneration::Eligible);
                eligible_miss = Some((cache, key, rgba_bytes));
            }
        }
    }
    let mask = render_shadow_mask(&spec);
    if let Some((cache, key, expected_rgba_bytes)) = eligible_miss {
        debug_assert_eq!(mask.pixels.len(), expected_rgba_bytes);
        let rgba_capacity = mask.pixels.capacity();
        let mask = Rc::new(mask);
        composite_shadow_mask(canvas, &mask, None, &spec, color, clip);
        cache.retain_with_spans(key, rgba_capacity, Rc::clone(&mask), None);
    } else {
        composite_shadow_mask(canvas, &mask, None, &spec, color, clip);
    }
}

fn paint_outer_box_shadow_cached(
    canvas: &mut Canvas,
    border_box: Rect,
    shadow: &BoxShadow,
    clip: Option<Rect>,
    radii: Option<(f32, f32, f32, f32)>,
    shadow_masks: Option<&FrameShadowMaskCache>,
) {
    let spread = shadow.spread_radius;
    let blur = shadow.blur_radius;

    // shadow の矩形（spread 分だけ拡張）
    let shadow_rect = Rect {
        x: border_box.x + shadow.offset_x - spread,
        y: border_box.y + shadow.offset_y - spread,
        width: border_box.width + spread * 2.0,
        height: border_box.height + spread * 2.0,
    };

    // 負の spread により shadow が縮んで消滅した場合は描画をスキップする
    if shadow_rect.width <= 0.0 || shadow_rect.height <= 0.0 {
        return;
    }

    if blur <= 0.0 && radii.is_none() {
        paint_unblurred_rect_shadow(canvas, border_box, shadow_rect, shadow.color, clip);
        return;
    }
    let spec = shadow_mask_spec(border_box, shadow_rect, shadow, radii);
    if spec.buf_w == 0
        || spec.buf_h == 0
        || !shadow_buffer_intersects_canvas(
            spec.buf_x,
            spec.buf_y,
            spec.buf_w,
            spec.buf_h,
            canvas.width(),
            canvas.height(),
        )
    {
        return;
    }
    paint_shadow_mask(canvas, spec, shadow.color, clip, shadow_masks);
}

fn paint_unblurred_rect_shadow(
    canvas: &mut Canvas,
    border_box: Rect,
    shadow_rect: Rect,
    color: Color,
    clip: Option<Rect>,
) {
    let sx = shadow_rect.x;
    let sy = shadow_rect.y;
    let sw = shadow_rect.width;
    let sh = shadow_rect.height;
    let bx = border_box.x;
    let by = border_box.y;
    let bw = border_box.width;
    let bh = border_box.height;
    if sy < by {
        canvas.fill_rect_clipped(
            Rect {
                x: sx,
                y: sy,
                width: sw,
                height: by - sy,
            },
            color,
            clip,
        );
    }
    let shadow_bottom = sy + sh;
    let box_bottom = by + bh;
    if shadow_bottom > box_bottom {
        canvas.fill_rect_clipped(
            Rect {
                x: sx,
                y: box_bottom,
                width: sw,
                height: shadow_bottom - box_bottom,
            },
            color,
            clip,
        );
    }
    let band_top = by.max(sy);
    let band_bottom = box_bottom.min(shadow_bottom);
    if band_bottom <= band_top {
        return;
    }
    if sx < bx {
        canvas.fill_rect_clipped(
            Rect {
                x: sx,
                y: band_top,
                width: bx - sx,
                height: band_bottom - band_top,
            },
            color,
            clip,
        );
    }
    let box_right = bx + bw;
    let shadow_right = sx + sw;
    if shadow_right > box_right {
        canvas.fill_rect_clipped(
            Rect {
                x: box_right,
                y: band_top,
                width: shadow_right - box_right,
                height: band_bottom - band_top,
            },
            color,
            clip,
        );
    }
}

pub(super) fn gaussian_box_blur_radii(blur_radius: f32) -> [u32; 3] {
    let sigma = f64::from(blur_radius.max(0.0)) / 2.0;
    if sigma == 0.0 || !sigma.is_finite() {
        return [0; 3];
    }

    const PASSES: usize = 3;
    let ideal_width = (12.0 * sigma * sigma / PASSES as f64 + 1.0).sqrt();
    let mut lower_width = ideal_width.floor() as i64;
    if lower_width % 2 == 0 {
        lower_width -= 1;
    }
    lower_width = lower_width.max(1);
    let upper_width = lower_width.saturating_add(2);
    let lower_count = ((12.0 * sigma * sigma
        - PASSES as f64 * lower_width as f64 * lower_width as f64
        - 4.0 * PASSES as f64 * lower_width as f64
        - 3.0 * PASSES as f64)
        / (-4.0 * lower_width as f64 - 4.0))
        .round()
        .clamp(0.0, PASSES as f64) as usize;

    let lower_radius = ((lower_width - 1) / 2) as u32;
    let upper_radius = ((upper_width - 1) / 2) as u32;
    let mut radii = [upper_radius; PASSES];
    radii[..lower_count].fill(lower_radius);
    radii
}

fn knock_out_shadow_interior(
    shadow: &mut Canvas,
    border_box: Rect,
    radii: Option<(f32, f32, f32, f32)>,
) {
    let Some(area) = normalize_rect(border_box) else {
        return;
    };
    let x0 = area.x.floor().max(0.0) as i32;
    let y0 = area.y.floor().max(0.0) as i32;
    let x1 = (area.x + area.width).ceil().min(shadow.width as f32) as i32;
    let y1 = (area.y + area.height).ceil().min(shadow.height as f32) as i32;

    if let Some(radii) = radii {
        for py in y0..y1 {
            for px in x0..x1 {
                let coverage = rounded_rect_pixel_coverage(px, py, area, radii);
                if coverage <= 0.0 {
                    continue;
                }
                let index = (py as u32 * shadow.width + px as u32) as usize * 4 + 3;
                shadow.pixels[index] =
                    (shadow.pixels[index] as f32 * (1.0 - coverage)).round() as u8;
            }
        }
    } else {
        for py in y0..y1 {
            for px in x0..x1 {
                let index = (py as u32 * shadow.width + px as u32) as usize * 4 + 3;
                shadow.pixels[index] = 0;
            }
        }
    }
}

#[cfg(test)]
#[path = "border/shadow_cull_tests.rs"]
mod shadow_cull_tests;

#[cfg(test)]
#[path = "border/shadow_cache_baseline_tests.rs"]
mod shadow_cache_baseline_tests;

#[cfg(test)]
#[path = "border/shadow_mask_cache_tests.rs"]
mod shadow_mask_cache_tests;
