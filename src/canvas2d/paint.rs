//! Fill and stroke styles: colors, gradients, patterns and images.

use serde::Deserialize;

use super::{Matrix, Rgba};

/// One gradient color stop: an offset and an unpremultiplied color.
pub(crate) type ColorStop = (f64, [u8; 4]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Repetition {
    #[default]
    Repeat,
    RepeatX,
    RepeatY,
    NoRepeat,
}

/// A paint source as script describes it. Image pixels travel separately.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum PaintSpec {
    Color {
        color: [u8; 4],
    },
    Linear {
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        stops: Vec<ColorStop>,
    },
    Radial {
        x0: f64,
        y0: f64,
        r0: f64,
        x1: f64,
        y1: f64,
        r1: f64,
        stops: Vec<ColorStop>,
    },
    Conic {
        angle: f64,
        x: f64,
        y: f64,
        stops: Vec<ColorStop>,
    },
    Pattern {
        width: usize,
        height: usize,
        #[serde(default)]
        repetition: Repetition,
        /// The pattern's own transform, applied before the drawing one.
        #[serde(default = "Matrix::identity")]
        transform: Matrix,
    },
    /// `drawImage()`: source pixels inside `source` map onto `destination`.
    Image {
        width: usize,
        height: usize,
        source: [f64; 4],
        destination: [f64; 4],
    },
}

/// An image whose pixels are unpremultiplied RGBA.
pub(crate) struct Texture<'a> {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) pixels: &'a [u8],
}

/// A paint ready to sample in device space.
pub(crate) struct Paint<'a> {
    kind: Kind<'a>,
    /// Device space to paint space.
    inverse: Matrix,
}

enum Kind<'a> {
    Color(Rgba),
    Linear {
        start: [f64; 2],
        delta: [f64; 2],
        stops: Vec<ColorStop>,
    },
    Radial {
        c0: [f64; 2],
        r0: f64,
        c1: [f64; 2],
        r1: f64,
        stops: Vec<ColorStop>,
    },
    Conic {
        angle: f64,
        center: [f64; 2],
        stops: Vec<ColorStop>,
    },
    Texture {
        texture: Texture<'a>,
        repetition: Repetition,
        /// The texel rectangle that clamps sampling, for `drawImage()`.
        bounds: Option<[f64; 4]>,
        smoothing: bool,
    },
    /// Paints nothing, such as a degenerate gradient.
    None,
}

impl<'a> Paint<'a> {
    /// Prepares `spec` drawn under `transform`. Returns `None` when the
    /// pattern or image needs pixels that were not supplied.
    pub(crate) fn new(
        spec: &PaintSpec,
        transform: Matrix,
        texture: Option<Texture<'a>>,
        smoothing: bool,
    ) -> Option<Self> {
        let sorted = |stops: &[ColorStop]| {
            let mut stops = stops.to_vec();
            stops.sort_by(|a, b| a.0.total_cmp(&b.0));
            stops
        };
        let mut paint_transform = transform;
        let kind = match spec {
            PaintSpec::Color { color } => Kind::Color(Rgba::from_unpremultiplied(*color)),
            PaintSpec::Linear { stops, .. }
            | PaintSpec::Radial { stops, .. }
            | PaintSpec::Conic { stops, .. }
                if stops.is_empty() =>
            {
                Kind::None
            }
            PaintSpec::Linear {
                x0,
                y0,
                x1,
                y1,
                stops,
            } => {
                if x0 == x1 && y0 == y1 {
                    Kind::None
                } else {
                    Kind::Linear {
                        start: [*x0, *y0],
                        delta: [x1 - x0, y1 - y0],
                        stops: sorted(stops),
                    }
                }
            }
            PaintSpec::Radial {
                x0,
                y0,
                r0,
                x1,
                y1,
                r1,
                stops,
            } => {
                if x0 == x1 && y0 == y1 && r0 == r1 {
                    Kind::None
                } else {
                    Kind::Radial {
                        c0: [*x0, *y0],
                        r0: *r0,
                        c1: [*x1, *y1],
                        r1: *r1,
                        stops: sorted(stops),
                    }
                }
            }
            PaintSpec::Conic { angle, x, y, stops } => Kind::Conic {
                angle: *angle,
                center: [*x, *y],
                stops: sorted(stops),
            },
            PaintSpec::Pattern {
                repetition,
                transform: pattern,
                ..
            } => {
                paint_transform = transform.multiply(pattern);
                Kind::Texture {
                    texture: texture?,
                    repetition: *repetition,
                    bounds: None,
                    smoothing,
                }
            }
            PaintSpec::Image {
                source,
                destination,
                ..
            } => {
                let [sx, sy, sw, sh] = *source;
                let [dx, dy, dw, dh] = *destination;
                let mapping = Matrix([
                    dw / sw,
                    0.0,
                    0.0,
                    dh / sh,
                    dx - sx * dw / sw,
                    dy - sy * dh / sh,
                ]);
                paint_transform = transform.multiply(&mapping);
                Kind::Texture {
                    texture: texture?,
                    repetition: Repetition::NoRepeat,
                    bounds: Some(*source),
                    smoothing,
                }
            }
        };
        let inverse = paint_transform.inverse().unwrap_or(Matrix::identity());
        Some(Self { kind, inverse })
    }

    /// Whether the paint covers nothing.
    pub(crate) fn is_none(&self) -> bool {
        matches!(self.kind, Kind::None)
    }

    /// The premultiplied color at the center of device pixel `(x, y)`.
    pub(crate) fn sample(&self, x: i64, y: i64) -> Rgba {
        let point = self.inverse.apply([x as f64 + 0.5, y as f64 + 0.5]);
        match &self.kind {
            Kind::Color(color) => *color,
            Kind::None => Rgba::TRANSPARENT,
            Kind::Linear {
                start,
                delta,
                stops,
            } => {
                let t = ((point[0] - start[0]) * delta[0] + (point[1] - start[1]) * delta[1])
                    / (delta[0] * delta[0] + delta[1] * delta[1]);
                gradient_color(stops, t)
            }
            Kind::Radial {
                c0,
                r0,
                c1,
                r1,
                stops,
            } => radial_parameter(*c0, *r0, *c1, *r1, point)
                .map_or(Rgba::TRANSPARENT, |t| gradient_color(stops, t)),
            Kind::Conic {
                angle,
                center,
                stops,
            } => {
                let theta = (point[1] - center[1]).atan2(point[0] - center[0]) - angle;
                let t = (theta / std::f64::consts::TAU).rem_euclid(1.0);
                gradient_color(stops, t)
            }
            Kind::Texture {
                texture,
                repetition,
                bounds,
                smoothing,
            } => sample_texture(texture, *repetition, *bounds, *smoothing, point),
        }
    }
}

/// The largest ω with r(ω) ≥ 0 whose circle passes through `p` (HTML
/// "createRadialGradient").
fn radial_parameter(c0: [f64; 2], r0: f64, c1: [f64; 2], r1: f64, p: [f64; 2]) -> Option<f64> {
    let (cdx, cdy, dr) = (c1[0] - c0[0], c1[1] - c0[1], r1 - r0);
    let (px, py) = (p[0] - c0[0], p[1] - c0[1]);
    let a = cdx * cdx + cdy * cdy - dr * dr;
    let b = px * cdx + py * cdy + r0 * dr;
    let c = px * px + py * py - r0 * r0;
    let valid = |t: f64| r0 + t * dr >= 0.0;
    if a.abs() < 1e-12 {
        if b == 0.0 {
            return None;
        }
        let t = c / (2.0 * b);
        return valid(t).then_some(t);
    }
    let discriminant = b * b - a * c;
    if discriminant < 0.0 {
        return None;
    }
    let root = discriminant.sqrt();
    let (t1, t2) = ((b + root) / a, (b - root) / a);
    let (high, low) = if t1 > t2 { (t1, t2) } else { (t2, t1) };
    if valid(high) {
        Some(high)
    } else if valid(low) {
        Some(low)
    } else {
        None
    }
}

/// Interpolates unpremultiplied stop colors at `t`, clamped to the ends.
fn gradient_color(stops: &[ColorStop], t: f64) -> Rgba {
    let color = if t <= stops[0].0 {
        stops[0].1.map(f64::from)
    } else if t >= stops[stops.len() - 1].0 {
        stops[stops.len() - 1].1.map(f64::from)
    } else {
        // The last stop at or before `t` and the first one after it.
        let after = stops
            .iter()
            .position(|stop| stop.0 > t)
            .unwrap_or(stops.len() - 1);
        let (a, b) = (stops[after - 1], stops[after]);
        let span = b.0 - a.0;
        let f = if span > 0.0 { (t - a.0) / span } else { 1.0 };
        std::array::from_fn(|i| f64::from(a.1[i]) + (f64::from(b.1[i]) - f64::from(a.1[i])) * f)
    };
    let alpha = color[3] / 255.0;
    Rgba([
        color[0] / 255.0 * alpha,
        color[1] / 255.0 * alpha,
        color[2] / 255.0 * alpha,
        alpha,
    ])
}

fn sample_texture(
    texture: &Texture<'_>,
    repetition: Repetition,
    bounds: Option<[f64; 4]>,
    smoothing: bool,
    point: [f64; 2],
) -> Rgba {
    let (width, height) = (texture.width as f64, texture.height as f64);
    if texture.width == 0 || texture.height == 0 {
        return Rgba::TRANSPARENT;
    }
    let repeat_x = matches!(repetition, Repetition::Repeat | Repetition::RepeatX);
    let repeat_y = matches!(repetition, Repetition::Repeat | Repetition::RepeatY);
    let [left, top, right, bottom] = bounds.map_or([0.0, 0.0, width, height], |[x, y, w, h]| {
        [
            x.max(0.0),
            y.max(0.0),
            (x + w).min(width),
            (y + h).min(height),
        ]
    });
    let inside_x = repeat_x || (point[0] >= left && point[0] < right);
    let inside_y = repeat_y || (point[1] >= top && point[1] < bottom);
    if !inside_x || !inside_y || left >= right || top >= bottom {
        return Rgba::TRANSPARENT;
    }
    let texel = |x: i64, y: i64| {
        let x = if repeat_x {
            x.rem_euclid(texture.width as i64)
        } else {
            x.clamp(left as i64, right.ceil() as i64 - 1)
        };
        let y = if repeat_y {
            y.rem_euclid(texture.height as i64)
        } else {
            y.clamp(top as i64, bottom.ceil() as i64 - 1)
        };
        let index = (y as usize * texture.width + x as usize) * 4;
        let pixel = &texture.pixels[index..index + 4];
        Rgba::from_unpremultiplied([pixel[0], pixel[1], pixel[2], pixel[3]])
    };
    if !smoothing {
        return texel(point[0].floor() as i64, point[1].floor() as i64);
    }
    let (u, v) = (point[0] - 0.5, point[1] - 0.5);
    let (x0, y0) = (u.floor(), v.floor());
    let (fx, fy) = (u - x0, v - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let top_row = texel(x0, y0).lerp(texel(x0 + 1, y0), fx);
    let bottom_row = texel(x0, y0 + 1).lerp(texel(x0 + 1, y0 + 1), fx);
    top_row.lerp(bottom_row, fy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(json: &str) -> PaintSpec {
        serde_json::from_str(json).unwrap()
    }

    fn color(paint: &Paint<'_>, x: usize, y: usize) -> [u8; 4] {
        paint.sample(x as i64, y as i64).to_unpremultiplied()
    }

    #[test]
    fn linear_gradients_interpolate_unpremultiplied_colors() {
        let gradient = spec(
            r#"{"type":"linear","x0":0,"y0":0,"x1":100,"y1":0,
                "stops":[[0,[255,0,0,255]],[1,[0,0,255,255]]]}"#,
        );
        let paint = Paint::new(&gradient, Matrix::identity(), None, true).unwrap();
        assert_eq!(color(&paint, 0, 0), [254, 0, 1, 255]);
        assert_eq!(color(&paint, 99, 0), [1, 0, 254, 255]);
        assert_eq!(color(&paint, 200, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn radial_gradients_pick_the_largest_parameter() {
        assert_eq!(
            radial_parameter([0.0, 0.0], 0.0, [0.0, 0.0], 10.0, [5.0, 0.0]),
            Some(0.5)
        );
        // Outside the cone of a gradient whose circles do not contain each
        // other, nothing is painted.
        assert_eq!(
            radial_parameter([0.0, 0.0], 1.0, [10.0, 0.0], 1.0, [5.0, 5.0]),
            None
        );
    }

    #[test]
    fn patterns_repeat_and_images_clamp() {
        let pixels = [
            255, 0, 0, 255, 0, 255, 0, 255, //
            0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let texture = || Texture {
            width: 2,
            height: 2,
            pixels: &pixels,
        };
        let pattern = spec(r#"{"type":"pattern","width":2,"height":2,"repetition":"repeat-x"}"#);
        let paint = Paint::new(&pattern, Matrix::identity(), Some(texture()), false).unwrap();
        assert_eq!(color(&paint, 3, 1), [255, 255, 255, 255]);
        assert_eq!(color(&paint, 3, 2), [0, 0, 0, 0]);
        let image = spec(
            r#"{"type":"image","width":2,"height":2,"source":[0,0,2,2],"destination":[0,0,4,4]}"#,
        );
        let paint = Paint::new(&image, Matrix::identity(), Some(texture()), false).unwrap();
        assert_eq!(color(&paint, 3, 0), [0, 255, 0, 255]);
        assert_eq!(color(&paint, 4, 0), [0, 0, 0, 0]);
    }
}
