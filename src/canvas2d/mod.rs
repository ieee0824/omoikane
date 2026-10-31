//! Software rasterizer for `CanvasRenderingContext2D` drawing operations
//! (HTML §4.12.5.1).
//!
//! Script owns the context state and path construction. Each drawing call
//! hands this module a [`DrawCommand`] with device-space paths, the paint,
//! compositing, shadow and clip state; [`draw`] updates an unpremultiplied
//! RGBA bitmap in place. Everything here is a pure function of its inputs.

mod composite;
mod paint;
mod raster;
mod shadow;
mod stroke;

use serde::Deserialize;

pub(crate) use composite::Operator;
pub(crate) use paint::{PaintSpec, Texture};
use raster::Mask;
pub(crate) use stroke::StrokeStyle;

/// A polyline; filling closes it implicitly.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct SubPath {
    pub(crate) points: Vec<[f64; 2]>,
    #[serde(default)]
    pub(crate) closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

/// A 2D affine matrix `[a, b, c, d, e, f]` mapping `(x, y)` to
/// `(a·x + c·y + e, b·x + d·y + f)`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub(crate) struct Matrix(pub(crate) [f64; 6]);

impl Matrix {
    pub(crate) fn identity() -> Self {
        Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
    }

    pub(crate) fn apply(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        let [a, b, c, d, e, f] = self.0;
        [a * x + c * y + e, b * x + d * y + f]
    }

    /// `self × other`: `other` applies first.
    pub(crate) fn multiply(&self, other: &Self) -> Self {
        let [a, b, c, d, e, f] = self.0;
        let [g, h, i, j, k, l] = other.0;
        Self([
            a * g + c * h,
            b * g + d * h,
            a * i + c * j,
            b * i + d * j,
            a * k + c * l + e,
            b * k + d * l + f,
        ])
    }

    pub(crate) fn inverse(&self) -> Option<Self> {
        let [a, b, c, d, e, f] = self.0;
        let determinant = a * d - b * c;
        if determinant == 0.0 || !determinant.is_finite() {
            return None;
        }
        Some(Self([
            d / determinant,
            -b / determinant,
            -c / determinant,
            a / determinant,
            (c * f - d * e) / determinant,
            (b * e - a * f) / determinant,
        ]))
    }

    fn map_paths(&self, paths: &[SubPath]) -> Vec<SubPath> {
        paths
            .iter()
            .map(|path| SubPath {
                points: path.points.iter().map(|point| self.apply(*point)).collect(),
                closed: path.closed,
            })
            .collect()
    }
}

/// A premultiplied RGBA color with components in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rgba(pub(crate) [f64; 4]);

impl Rgba {
    pub(crate) const TRANSPARENT: Self = Self([0.0; 4]);

    pub(crate) fn from_unpremultiplied(pixel: [u8; 4]) -> Self {
        let alpha = f64::from(pixel[3]) / 255.0;
        Self([
            f64::from(pixel[0]) / 255.0 * alpha,
            f64::from(pixel[1]) / 255.0 * alpha,
            f64::from(pixel[2]) / 255.0 * alpha,
            alpha,
        ])
    }

    pub(crate) fn to_unpremultiplied(self) -> [u8; 4] {
        let alpha = self.0[3].clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return [0; 4];
        }
        let channel = |value: f64| ((value / alpha).clamp(0.0, 1.0) * 255.0).round() as u8;
        [
            channel(self.0[0]),
            channel(self.0[1]),
            channel(self.0[2]),
            (alpha * 255.0).round() as u8,
        ]
    }

    pub(crate) fn scale(self, factor: f64) -> Self {
        Self(self.0.map(|value| value * factor))
    }

    pub(crate) fn lerp(self, other: Self, t: f64) -> Self {
        Self(std::array::from_fn(|i| {
            self.0[i] + (other.0[i] - self.0[i]) * t
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DrawKind {
    Fill,
    Stroke,
    /// `clearRect()`: clears the covered pixels inside the clip.
    Clear,
}

/// The shadow styles, with offsets in device pixels.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Shadow {
    pub(crate) color: [u8; 4],
    #[serde(default)]
    pub(crate) blur: f64,
    #[serde(default)]
    pub(crate) offset_x: f64,
    #[serde(default)]
    pub(crate) offset_y: f64,
}

/// One clip region in device space; the clip is their intersection.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ClipRegion {
    pub(crate) paths: Vec<SubPath>,
    #[serde(default)]
    pub(crate) rule: FillRule,
}

/// One drawing operation.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DrawCommand {
    pub(crate) kind: DrawKind,
    /// Device-space paths: the area to fill, the path to stroke, or the
    /// rectangle to clear or draw an image into.
    #[serde(default)]
    pub(crate) paths: Vec<SubPath>,
    #[serde(default)]
    pub(crate) rule: FillRule,
    /// The current transformation matrix. Strokes are traced and paints are
    /// positioned in the user space it defines.
    #[serde(default = "Matrix::identity")]
    pub(crate) transform: Matrix,
    #[serde(default)]
    pub(crate) stroke: Option<StrokeStyle>,
    #[serde(default)]
    pub(crate) paint: Option<PaintSpec>,
    #[serde(default = "one")]
    pub(crate) alpha: f64,
    #[serde(default)]
    pub(crate) composite: Option<String>,
    #[serde(default)]
    pub(crate) shadow: Option<Shadow>,
    #[serde(default)]
    pub(crate) clip: Vec<ClipRegion>,
    #[serde(default = "yes")]
    pub(crate) smoothing: bool,
}

fn one() -> f64 {
    1.0
}

fn yes() -> bool {
    true
}

/// An unpremultiplied RGBA bitmap borrowed for one drawing operation.
pub(crate) struct Surface<'a> {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) pixels: &'a mut [u8],
}

impl Surface<'_> {
    fn read(&self, x: usize, y: usize) -> Rgba {
        let index = (y * self.width + x) * 4;
        let pixel = &self.pixels[index..index + 4];
        Rgba::from_unpremultiplied([pixel[0], pixel[1], pixel[2], pixel[3]])
    }

    fn write(&mut self, x: usize, y: usize, color: Rgba) {
        let index = (y * self.width + x) * 4;
        self.pixels[index..index + 4].copy_from_slice(&color.to_unpremultiplied());
    }
}

/// A premultiplied image placed at `(left, top)` in device pixels.
pub(crate) struct Layer {
    pub(crate) left: i64,
    pub(crate) top: i64,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) data: Vec<Rgba>,
}

impl Layer {
    fn get(&self, x: usize, y: usize) -> Rgba {
        let (lx, ly) = (x as i64 - self.left, y as i64 - self.top);
        if lx < 0 || ly < 0 || lx >= self.width as i64 || ly >= self.height as i64 {
            return Rgba::TRANSPARENT;
        }
        self.data[ly as usize * self.width + lx as usize]
    }
}

/// Runs `command` on `surface`. `texture` holds the pattern or image pixels
/// the paint needs. Returns `false` when the command cannot be drawn, such
/// as a pattern without pixels.
pub(crate) fn draw(
    surface: &mut Surface<'_>,
    command: &DrawCommand,
    texture: Option<Texture<'_>>,
) -> bool {
    let (width, height) = (surface.width, surface.height);
    let clip = clip_mask(&command.clip, width, height);
    let (shape, rule) = match command.kind {
        DrawKind::Fill => (command.paths.clone(), command.rule),
        DrawKind::Stroke => {
            let Some(style) = &command.stroke else {
                return false;
            };
            let outline = stroke_outline(&command.paths, command.transform, style);
            (outline, FillRule::NonZero)
        }
        DrawKind::Clear => {
            let mask = raster::fill_mask(&command.paths, FillRule::NonZero, width, height);
            clear(surface, &mask, clip.as_ref());
            return true;
        }
    };
    let Some(spec) = &command.paint else {
        return false;
    };
    let operator = command
        .composite
        .as_deref()
        .and_then(Operator::parse)
        .unwrap_or_default();
    // `clear` removes what the shape covers whatever the paint is.
    let (spec, alpha) = match operator {
        Operator::Clear => (&CLEAR_PAINT, 1.0),
        _ => (spec, command.alpha),
    };
    let Some(paint) = paint::Paint::new(spec, command.transform, texture, command.smoothing) else {
        return false;
    };
    let shadow = command
        .shadow
        .as_ref()
        .filter(|shadow| shadow::is_visible(shadow));
    let margin = shadow.map_or(Margin::default(), shadow::margin);
    let source = source_layer(&shape, rule, width, height, margin, &paint);
    if let Some(shadow) = shadow.and_then(|shadow| shadow::layer(&source, shadow)) {
        composite(surface, &shadow, operator, alpha, clip.as_ref());
    }
    composite(surface, &source, operator, alpha, clip.as_ref());
    true
}

const CLEAR_PAINT: PaintSpec = PaintSpec::Color {
    color: [0, 0, 0, 255],
};

/// Extra device pixels around the surface that the source image covers, so
/// a shadow can be cast by parts of the shape outside the surface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Margin {
    pub(crate) left: usize,
    pub(crate) top: usize,
    pub(crate) right: usize,
    pub(crate) bottom: usize,
}

/// The intersection of `regions`, or `None` without a clip.
fn clip_mask(regions: &[ClipRegion], width: usize, height: usize) -> Option<Mask> {
    let mut regions = regions.iter();
    let first = regions.next()?;
    let mut mask = raster::fill_mask(&first.paths, first.rule, width, height);
    for region in regions {
        mask.intersect(&raster::fill_mask(
            &region.paths,
            region.rule,
            width,
            height,
        ));
    }
    Some(mask)
}

/// Traces device-space `paths` in the user space of `transform`.
fn stroke_outline(paths: &[SubPath], transform: Matrix, style: &StrokeStyle) -> Vec<SubPath> {
    let Some(inverse) = transform.inverse() else {
        return Vec::new();
    };
    let outline = stroke::outline(&inverse.map_paths(paths), style);
    transform.map_paths(&outline)
}

/// Image A of the drawing model: the paint inside `shape`, over the surface
/// grown by `margin`.
fn source_layer(
    shape: &[SubPath],
    rule: FillRule,
    width: usize,
    height: usize,
    margin: Margin,
    paint: &paint::Paint<'_>,
) -> Layer {
    let offset = Matrix([1.0, 0.0, 0.0, 1.0, margin.left as f64, margin.top as f64]);
    let coverage = raster::fill_mask(
        &offset.map_paths(shape),
        rule,
        width + margin.left + margin.right,
        height + margin.top + margin.bottom,
    );
    let (left, top, right, bottom) = coverage.bounds;
    let (layer_width, layer_height) = (right.saturating_sub(left), bottom.saturating_sub(top));
    let (origin_x, origin_y) = (margin.left as i64, margin.top as i64);
    let mut data = Vec::with_capacity(layer_width * layer_height);
    for y in top..top + layer_height {
        for x in left..left + layer_width {
            let cover = coverage.at(x, y);
            data.push(if cover > 0.0 && !paint.is_none() {
                paint
                    .sample(x as i64 - origin_x, y as i64 - origin_y)
                    .scale(f64::from(cover))
            } else {
                Rgba::TRANSPARENT
            });
        }
    }
    Layer {
        left: left as i64 - origin_x,
        top: top as i64 - origin_y,
        width: layer_width,
        height: layer_height,
        data,
    }
}

/// Composites `layer` × `alpha` onto `surface` with `operator` inside `clip`.
fn composite(
    surface: &mut Surface<'_>,
    layer: &Layer,
    operator: Operator,
    alpha: f64,
    clip: Option<&Mask>,
) {
    let (columns, rows) = if operator.is_unbounded() {
        (0..surface.width, 0..surface.height)
    } else {
        let clamp = |value: i64, limit: usize| value.clamp(0, limit as i64) as usize;
        (
            clamp(layer.left, surface.width)..clamp(layer.left + layer.width as i64, surface.width),
            clamp(layer.top, surface.height)
                ..clamp(layer.top + layer.height as i64, surface.height),
        )
    };
    for y in rows {
        for x in columns.clone() {
            let cover = clip.map_or(1.0, |clip| f64::from(clip.at(x, y)));
            if cover <= 0.0 {
                continue;
            }
            let destination = surface.read(x, y);
            let result = operator.apply(layer.get(x, y).scale(alpha), destination);
            surface.write(x, y, destination.lerp(result, cover));
        }
    }
}

fn clear(surface: &mut Surface<'_>, mask: &Mask, clip: Option<&Mask>) {
    let (left, top, right, bottom) = mask.bounds;
    for y in top..bottom {
        for x in left..right {
            let cover =
                f64::from(mask.at(x, y)) * clip.map_or(1.0, |clip| f64::from(clip.at(x, y)));
            if cover > 0.0 {
                let color = surface.read(x, y).scale(1.0 - cover);
                surface.write(x, y, color);
            }
        }
    }
}

/// `isPointInPath()` for device-space `paths` and a device-space point.
pub(crate) fn path_contains(paths: &[SubPath], rule: FillRule, point: [f64; 2]) -> bool {
    point.iter().all(|value| value.is_finite()) && raster::contains(paths, rule, point)
}

/// `isPointInStroke()` for device-space `paths` stroked under `transform`.
pub(crate) fn stroke_contains(
    paths: &[SubPath],
    transform: Matrix,
    style: &StrokeStyle,
    point: [f64; 2],
) -> bool {
    point.iter().all(|value| value.is_finite())
        && raster::contains(
            &stroke_outline(paths, transform, style),
            FillRule::NonZero,
            point,
        )
}

#[cfg(test)]
mod tests;
