//! The `filter` attribute (HTML §4.12.5.1.19): CSS filter functions applied
//! to the source image before shadows and compositing.

use super::shadow::{self, gaussian_blur};
use super::{Layer, Margin, Rgba, Shadow};
use crate::css::FilterFunction;

/// Parses a `filter` value as a CSS `<filter-value-list>` or `none`.
/// CSS-wide keywords and anything else invalid give `None`.
pub(crate) fn parse(value: &str) -> Option<Vec<FilterFunction>> {
    crate::css::parse_filter_list(value)
}

/// How far beyond the surface a shape can still reach it through `filters`.
pub(crate) fn margin(filters: &[FilterFunction]) -> Margin {
    let mut margin = Margin::default();
    for filter in filters {
        let (blur, dx, dy) = match filter {
            FilterFunction::Blur(radius) => (f64::from(*radius), 0.0, 0.0),
            FilterFunction::DropShadow {
                offset_x,
                offset_y,
                blur,
                ..
            } => (
                f64::from(*blur) / 2.0,
                f64::from(*offset_x),
                f64::from(*offset_y),
            ),
            _ => continue,
        };
        let reach = (blur.max(0.0) * 3.0).ceil().min(1.0e5) as usize + 1;
        let extent = |value: f64| value.round().clamp(0.0, 1.0e5) as usize + reach;
        margin.left += extent(dx);
        margin.top += extent(dy);
        margin.right += extent(-dx);
        margin.bottom += extent(-dy);
    }
    margin
}

/// Applies `filters` in order to the premultiplied `layer`.
pub(crate) fn apply(layer: Layer, filters: &[FilterFunction]) -> Layer {
    filters.iter().fold(layer, |layer, filter| match filter {
        FilterFunction::Blur(radius) => blur(layer, f64::from(*radius)),
        FilterFunction::DropShadow {
            offset_x,
            offset_y,
            blur,
            color,
        } => {
            let shadow = Shadow {
                color: [color.r, color.g, color.b, color.a],
                blur: f64::from(*blur),
                offset_x: f64::from(*offset_x),
                offset_y: f64::from(*offset_y),
            };
            match shadow::layer(&layer, &shadow) {
                Some(cast) => over(&layer, &cast),
                None => layer,
            }
        }
        FilterFunction::Opacity(amount) => map(layer, |[r, g, b, a]| {
            [r, g, b, a * f64::from(amount.clamp(0.0, 1.0))]
        }),
        other => {
            let matrix = color_matrix(other);
            map(layer, |[r, g, b, a]| {
                let row = |i: usize| {
                    matrix[i][0] * r + matrix[i][1] * g + matrix[i][2] * b + matrix[i][3]
                };
                [row(0), row(1), row(2), a]
            })
        }
    })
}

/// The affine RGB transform of a color filter function, as rows of
/// `[r, g, b, offset]` (W3C Filter Effects §13).
fn color_matrix(filter: &FilterFunction) -> [[f64; 4]; 3] {
    let diagonal = |scale: f64, offset: f64| {
        [
            [scale, 0.0, 0.0, offset],
            [0.0, scale, 0.0, offset],
            [0.0, 0.0, scale, offset],
        ]
    };
    match filter {
        FilterFunction::Brightness(amount) => diagonal(f64::from(*amount), 0.0),
        FilterFunction::Contrast(amount) => {
            let amount = f64::from(*amount);
            diagonal(amount, 0.5 - 0.5 * amount)
        }
        FilterFunction::Invert(amount) => {
            let amount = f64::from(amount.clamp(0.0, 1.0));
            diagonal(1.0 - 2.0 * amount, amount)
        }
        FilterFunction::Grayscale(amount) => {
            let s = 1.0 - f64::from(amount.clamp(0.0, 1.0));
            with_offset([
                [
                    0.2126 + 0.7874 * s,
                    0.7152 - 0.7152 * s,
                    0.0722 - 0.0722 * s,
                ],
                [
                    0.2126 - 0.2126 * s,
                    0.7152 + 0.2848 * s,
                    0.0722 - 0.0722 * s,
                ],
                [
                    0.2126 - 0.2126 * s,
                    0.7152 - 0.7152 * s,
                    0.0722 + 0.9278 * s,
                ],
            ])
        }
        FilterFunction::Sepia(amount) => {
            let s = 1.0 - f64::from(amount.clamp(0.0, 1.0));
            with_offset([
                [0.393 + 0.607 * s, 0.769 - 0.769 * s, 0.189 - 0.189 * s],
                [0.349 - 0.349 * s, 0.686 + 0.314 * s, 0.168 - 0.168 * s],
                [0.272 - 0.272 * s, 0.534 - 0.534 * s, 0.131 + 0.869 * s],
            ])
        }
        FilterFunction::Saturate(amount) => {
            let s = f64::from(*amount);
            with_offset([
                [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
                [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
                [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s],
            ])
        }
        FilterFunction::HueRotate(degrees) => {
            let (sin, cos) = f64::from(*degrees).to_radians().sin_cos();
            with_offset([
                [
                    0.213 + cos * 0.787 - sin * 0.213,
                    0.715 - cos * 0.715 - sin * 0.715,
                    0.072 - cos * 0.072 + sin * 0.928,
                ],
                [
                    0.213 - cos * 0.213 + sin * 0.143,
                    0.715 + cos * 0.285 + sin * 0.140,
                    0.072 - cos * 0.072 - sin * 0.283,
                ],
                [
                    0.213 - cos * 0.213 - sin * 0.787,
                    0.715 - cos * 0.715 + sin * 0.715,
                    0.072 + cos * 0.928 + sin * 0.072,
                ],
            ])
        }
        _ => diagonal(1.0, 0.0),
    }
}

fn with_offset(rows: [[f64; 3]; 3]) -> [[f64; 4]; 3] {
    rows.map(|[r, g, b]| [r, g, b, 0.0])
}

/// Maps each pixel's unpremultiplied `[r, g, b, a]`, clamping the result.
fn map(mut layer: Layer, f: impl Fn([f64; 4]) -> [f64; 4]) -> Layer {
    for pixel in &mut layer.data {
        let alpha = pixel.0[3];
        if alpha <= 0.0 {
            continue;
        }
        let [r, g, b, a] = f([
            pixel.0[0] / alpha,
            pixel.0[1] / alpha,
            pixel.0[2] / alpha,
            alpha,
        ]);
        let a = a.clamp(0.0, 1.0);
        *pixel = Rgba([
            r.clamp(0.0, 1.0) * a,
            g.clamp(0.0, 1.0) * a,
            b.clamp(0.0, 1.0) * a,
            a,
        ]);
    }
    layer
}

/// A Gaussian blur of standard deviation `sigma` over every channel.
fn blur(layer: Layer, sigma: f64) -> Layer {
    if !(sigma > 0.0) || layer.width == 0 || layer.height == 0 {
        return layer;
    }
    let pad = (sigma * 3.0).ceil().min(1.0e4) as usize;
    let (width, height) = (layer.width + pad * 2, layer.height + pad * 2);
    let mut channels = vec![vec![0.0; width * height]; 4];
    for y in 0..layer.height {
        for x in 0..layer.width {
            let pixel = layer.data[y * layer.width + x].0;
            for (channel, value) in channels.iter_mut().zip(pixel) {
                channel[(y + pad) * width + x + pad] = value;
            }
        }
    }
    for channel in &mut channels {
        gaussian_blur(channel, width, height, sigma);
    }
    Layer {
        left: layer.left - pad as i64,
        top: layer.top - pad as i64,
        width,
        height,
        data: (0..width * height)
            .map(|i| {
                Rgba([
                    channels[0][i],
                    channels[1][i],
                    channels[2][i],
                    channels[3][i],
                ])
            })
            .collect(),
    }
}

/// `top` composited source-over onto `bottom`, over both layers' bounds.
fn over(top: &Layer, bottom: &Layer) -> Layer {
    let left = top.left.min(bottom.left);
    let up = top.top.min(bottom.top);
    let right = (top.left + top.width as i64).max(bottom.left + bottom.width as i64);
    let down = (top.top + top.height as i64).max(bottom.top + bottom.height as i64);
    let (width, height) = ((right - left) as usize, (down - up) as usize);
    let sample = |layer: &Layer, x: i64, y: i64| {
        let (lx, ly) = (x - layer.left, y - layer.top);
        if lx < 0 || ly < 0 || lx >= layer.width as i64 || ly >= layer.height as i64 {
            Rgba::TRANSPARENT
        } else {
            layer.data[ly as usize * layer.width + lx as usize]
        }
    };
    let mut data = Vec::with_capacity(width * height);
    for y in up..down {
        for x in left..right {
            let (source, destination) = (sample(top, x, y), sample(bottom, x, y));
            let keep = 1.0 - source.0[3];
            data.push(Rgba(std::array::from_fn(|i| {
                source.0[i] + destination.0[i] * keep
            })));
        }
    }
    Layer {
        left,
        top: up,
        width,
        height,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(color: [f64; 4]) -> Layer {
        Layer {
            left: 10,
            top: 10,
            width: 4,
            height: 4,
            data: vec![Rgba(color); 16],
        }
    }

    #[test]
    fn values_parse_like_the_css_filter_property() {
        assert_eq!(parse("blur(  5px)"), Some(vec![FilterFunction::Blur(5.0)]));
        assert_eq!(parse("none"), Some(Vec::new()));
        for invalid in [
            "blur(10)",
            "blur 10px",
            "inherit",
            "initial",
            "unset",
            "",
            "null",
        ] {
            assert_eq!(parse(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn color_functions_map_unpremultiplied_channels() {
        let red = [0.5, 0.0, 0.0, 0.5];
        let inverted = apply(square(red), &[FilterFunction::Invert(1.0)]);
        assert_eq!(inverted.data[0], Rgba([0.0, 0.5, 0.5, 0.5]));
        let gray = apply(
            square([1.0, 1.0, 1.0, 1.0]),
            &[FilterFunction::Grayscale(1.0)],
        );
        assert!(
            gray.data[0]
                .0
                .iter()
                .all(|value| (value - 1.0).abs() < 1e-9)
        );
        let faded = apply(square(red), &[FilterFunction::Opacity(0.5)]);
        assert_eq!(faded.data[0], Rgba([0.25, 0.0, 0.0, 0.25]));
        let dark = apply(
            square([1.0, 1.0, 1.0, 1.0]),
            &[FilterFunction::Brightness(0.5)],
        );
        assert_eq!(dark.data[0], Rgba([0.5, 0.5, 0.5, 1.0]));
    }

    #[test]
    fn blur_and_drop_shadow_spread_beyond_the_shape() {
        let blurred = apply(square([1.0; 4]), &[FilterFunction::Blur(2.0)]);
        assert!(blurred.width > 4 && blurred.left < 10);
        let total: f64 = blurred.data.iter().map(|pixel| pixel.0[3]).sum();
        assert!((total - 16.0).abs() < 0.5, "{total}");
        let shadowed = apply(
            square([0.0, 0.0, 1.0, 1.0]),
            &[FilterFunction::DropShadow {
                offset_x: 10.0,
                offset_y: 0.0,
                blur: 0.0,
                color: crate::paint::Color {
                    r: 255,
                    g: 0,
                    b: 0,
                    a: 255,
                },
            }],
        );
        let at = |x: i64, y: i64| {
            shadowed.data
                [((y - shadowed.top) as usize) * shadowed.width + (x - shadowed.left) as usize]
        };
        assert_eq!(at(11, 11), Rgba([0.0, 0.0, 1.0, 1.0]));
        assert_eq!(at(21, 11), Rgba([1.0, 0.0, 0.0, 1.0]));
        assert!(margin(&[FilterFunction::Blur(2.0)]).left >= 6);
    }
}
