//! Shadows (HTML §4.12.5.1.18).

use super::{Layer, Margin, Rgba, Shadow};

/// Whether `shadow` draws anything (HTML "shadows are drawn").
pub(crate) fn is_visible(shadow: &Shadow) -> bool {
    shadow.color[3] > 0
        && [shadow.blur, shadow.offset_x, shadow.offset_y]
            .iter()
            .all(|value| value.is_finite())
        && (shadow.blur > 0.0 || shadow.offset_x != 0.0 || shadow.offset_y != 0.0)
}

/// How far beyond the surface the shape can cast a visible shadow.
pub(crate) fn margin(shadow: &Shadow) -> Margin {
    let blur = (shadow.blur.max(0.0) * 1.5).ceil() as usize + 1;
    let (x, y) = (shadow.offset_x.round(), shadow.offset_y.round());
    let extent = |value: f64| (value.max(0.0).min(1.0e5)) as usize + blur;
    Margin {
        left: extent(x),
        top: extent(y),
        right: extent(-x),
        bottom: extent(-y),
    }
}

/// Image B of the drawing model: the shadow cast by `source`, or `None`
/// when the shadow styles draw nothing.
pub(crate) fn layer(source: &Layer, shadow: &Shadow) -> Option<Layer> {
    let color = Rgba::from_unpremultiplied(shadow.color);
    let offsets = [shadow.offset_x, shadow.offset_y];
    let blur = if shadow.blur.is_finite() {
        shadow.blur.max(0.0)
    } else {
        0.0
    };
    if color.0[3] <= 0.0
        || !offsets.iter().all(|value| value.is_finite())
        || (blur == 0.0 && offsets == [0.0, 0.0])
        || source.width == 0
        || source.height == 0
    {
        return None;
    }
    let sigma = blur / 2.0;
    let pad = (sigma * 3.0).ceil() as usize;
    let (width, height) = (source.width + pad * 2, source.height + pad * 2);
    let mut alpha = vec![0.0; width * height];
    for y in 0..source.height {
        for x in 0..source.width {
            alpha[(y + pad) * width + x + pad] = source.data[y * source.width + x].0[3];
        }
    }
    if sigma > 0.0 {
        gaussian_blur(&mut alpha, width, height, sigma);
    }
    Some(Layer {
        left: source.left - pad as i64 + shadow.offset_x.round() as i64,
        top: source.top - pad as i64 + shadow.offset_y.round() as i64,
        width,
        height,
        data: alpha.into_iter().map(|value| color.scale(value)).collect(),
    })
}

/// Approximates a Gaussian blur with three box blurs per axis.
fn gaussian_blur(values: &mut [f64], width: usize, height: usize, sigma: f64) {
    for radius in box_radii(sigma) {
        box_blur(values, width, height, radius, true);
        box_blur(values, width, height, radius, false);
    }
}

/// Box radii whose three passes match a Gaussian of `sigma` (W3C Filter
/// Effects "feGaussianBlur").
fn box_radii(sigma: f64) -> [usize; 3] {
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let lower = {
        let mut width = ideal.floor() as usize;
        if width % 2 == 0 {
            width = width.saturating_sub(1);
        }
        width.max(1)
    };
    let upper = lower + 2;
    let threshold =
        ((12.0 * sigma * sigma - 3.0 * (lower * lower) as f64 - 12.0 * lower as f64 - 9.0)
            / (-4.0 * lower as f64 - 4.0))
            .round();
    std::array::from_fn(|pass| {
        let size = if (pass as f64) < threshold {
            lower
        } else {
            upper
        };
        size / 2
    })
}

fn box_blur(values: &mut [f64], width: usize, height: usize, radius: usize, horizontal: bool) {
    if radius == 0 {
        return;
    }
    let (lines, length) = if horizontal {
        (height, width)
    } else {
        (width, height)
    };
    let index = |line: usize, position: usize| {
        if horizontal {
            line * width + position
        } else {
            position * width + line
        }
    };
    let span = (radius * 2 + 1) as f64;
    let mut line_values = vec![0.0; length];
    for line in 0..lines {
        for (position, value) in line_values.iter_mut().enumerate() {
            *value = values[index(line, position)];
        }
        let mut sum: f64 = line_values.iter().take(radius + 1).sum();
        for position in 0..length {
            values[index(line, position)] = sum / span;
            if position + radius + 1 < length {
                sum += line_values[position + radius + 1];
            }
            if position >= radius {
                sum -= line_values[position - radius];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Layer {
        Layer {
            left: 10,
            top: 10,
            width: 4,
            height: 4,
            data: vec![Rgba([0.0, 0.0, 0.0, 1.0]); 16],
        }
    }

    fn shadow(blur: f64, offset: f64) -> Shadow {
        Shadow {
            color: [255, 0, 0, 255],
            blur,
            offset_x: offset,
            offset_y: offset,
        }
    }

    #[test]
    fn offset_shadows_move_the_source_alpha() {
        let layer = layer(&square(), &shadow(0.0, 5.0)).unwrap();
        assert_eq!((layer.left, layer.top, layer.width), (15, 15, 4));
        assert_eq!(layer.get(15, 15).0, [1.0, 0.0, 0.0, 1.0]);
        assert!(super::layer(&square(), &shadow(0.0, 0.0)).is_none());
    }

    #[test]
    fn blurred_shadows_spread_and_keep_their_total() {
        let layer = layer(&square(), &shadow(4.0, 0.0)).unwrap();
        let total: f64 = layer.data.iter().map(|color| color.0[3]).sum();
        assert!((total - 16.0).abs() < 1e-6, "{total}");
        assert!(layer.get(9, 12).0[3] > 0.0);
        assert!(layer.get(12, 12).0[3] < 1.0);
    }
}
