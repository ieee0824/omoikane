//! CSS Shapes geometry used by float line wrapping.

use crate::css::{ComputedStyle, ComputedValue};
use crate::paint::{ClipPathShape, basic_shape_from_value};

use super::{BoxDimensions, Rect, resolved_length};

#[derive(Debug, Clone)]
pub(super) struct ShapeOutside {
    shape: ShapeGeometry,
    margin: f32,
    clip: Rect,
}

#[derive(Debug, Clone)]
enum ShapeGeometry {
    Rect(Rect),
    Basic(ClipPathShape),
}

impl ShapeOutside {
    pub(super) fn from_style(
        style: &ComputedStyle,
        dimensions: BoxDimensions,
        containing_inline_size: f32,
    ) -> Option<Self> {
        let value = match style.get("shape-outside") {
            Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => value.trim(),
            _ => return None,
        };
        if value.eq_ignore_ascii_case("none") {
            return None;
        }

        let parts = split_top_level_whitespace(value);
        let reference_name = parts
            .iter()
            .find(|part| is_reference_box(part))
            .copied()
            .unwrap_or("margin-box");
        let reference = reference_box(dimensions, reference_name);
        let basic = parts.iter().find(|part| part.contains('(')).copied();
        let shape = if let Some(basic) = basic {
            ShapeGeometry::Basic(basic_shape_from_value(basic, reference)?)
        } else {
            ShapeGeometry::Rect(reference)
        };
        let clip = reference_box(dimensions, "margin-box");
        let margin = resolved_length(style, "shape-margin", containing_inline_size)
            .unwrap_or(0.0)
            .max(0.0);
        Some(Self {
            shape,
            margin,
            clip,
        })
    }

    /// Returns the horizontal extent of the exclusion area intersecting a
    /// line's block-axis band. The result is clipped to the float margin box,
    /// as required by CSS Shapes.
    pub(super) fn horizontal_bounds(&self, y: f32, height: f32) -> Option<(f32, f32)> {
        let band_start = y.max(self.clip.y);
        let band_end = (y + height.max(0.01)).min(self.clip.y + self.clip.height);
        if band_start >= band_end {
            return None;
        }
        let (mut min_x, mut max_x) = match &self.shape {
            ShapeGeometry::Rect(rect) => rect_band_bounds(*rect, band_start, band_end, self.margin),
            ShapeGeometry::Basic(shape) => {
                basic_shape_band_bounds(shape, band_start, band_end, self.margin)
            }
        }?;
        min_x = min_x.max(self.clip.x);
        max_x = max_x.min(self.clip.x + self.clip.width);
        (min_x <= max_x).then_some((min_x, max_x))
    }

    /// Returns the vertical extent of the exclusion area intersecting a
    /// vertical line's physical x-axis band. This is the transposed form of
    /// [`Self::horizontal_bounds`] used by vertical writing modes.
    pub(super) fn vertical_bounds(&self, x: f32, width: f32) -> Option<(f32, f32)> {
        let band_start = x.max(self.clip.x);
        let band_end = (x + width.max(0.01)).min(self.clip.x + self.clip.width);
        if band_start >= band_end {
            return None;
        }
        let (mut min_y, mut max_y) = match &self.shape {
            ShapeGeometry::Rect(rect) => {
                rect_column_bounds(*rect, band_start, band_end, self.margin)
            }
            ShapeGeometry::Basic(shape) => {
                basic_shape_column_bounds(shape, band_start, band_end, self.margin)
            }
        }?;
        min_y = min_y.max(self.clip.y);
        max_y = max_y.min(self.clip.y + self.clip.height);
        (min_y <= max_y).then_some((min_y, max_y))
    }
}

fn is_reference_box(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "margin-box" | "border-box" | "padding-box" | "content-box"
    )
}

fn reference_box(dimensions: BoxDimensions, name: &str) -> Rect {
    match name.to_ascii_lowercase().as_str() {
        "content-box" => dimensions.content,
        "padding-box" => Rect {
            x: dimensions.content.x - dimensions.padding.left,
            y: dimensions.content.y - dimensions.padding.top,
            width: dimensions.content.width + dimensions.padding.left + dimensions.padding.right,
            height: dimensions.content.height + dimensions.padding.top + dimensions.padding.bottom,
        },
        "border-box" => dimensions.border_box(),
        _ => {
            let border = dimensions.border_box();
            Rect {
                x: border.x - dimensions.margin.left,
                y: border.y - dimensions.margin.top,
                width: border.width + dimensions.margin.left + dimensions.margin.right,
                height: border.height + dimensions.margin.top + dimensions.margin.bottom,
            }
        }
    }
}

fn rect_band_bounds(rect: Rect, y0: f32, y1: f32, margin: f32) -> Option<(f32, f32)> {
    if y1 <= rect.y - margin || y0 >= rect.y + rect.height + margin {
        return None;
    }
    Some((rect.x - margin, rect.x + rect.width + margin))
}

fn rect_column_bounds(rect: Rect, x0: f32, x1: f32, margin: f32) -> Option<(f32, f32)> {
    if x1 <= rect.x - margin || x0 >= rect.x + rect.width + margin {
        return None;
    }
    Some((rect.y - margin, rect.y + rect.height + margin))
}

fn basic_shape_band_bounds(
    shape: &ClipPathShape,
    y0: f32,
    y1: f32,
    margin: f32,
) -> Option<(f32, f32)> {
    match shape {
        ClipPathShape::Circle { center, radius, .. } => {
            ellipse_band_bounds(*center, (*radius + margin, *radius + margin), y0, y1)
        }
        ClipPathShape::Ellipse { center, radii, .. } => {
            ellipse_band_bounds(*center, (radii.0 + margin, radii.1 + margin), y0, y1)
        }
        ClipPathShape::RoundedRect { rect, radii } => {
            rounded_rect_band_bounds(*rect, *radii, y0, y1, margin)
        }
        ClipPathShape::Polygon { points, .. } => polygon_band_bounds(points, y0, y1, margin),
    }
}

fn basic_shape_column_bounds(
    shape: &ClipPathShape,
    x0: f32,
    x1: f32,
    margin: f32,
) -> Option<(f32, f32)> {
    match shape {
        ClipPathShape::Circle { center, radius, .. } => {
            ellipse_column_bounds(*center, (*radius + margin, *radius + margin), x0, x1)
        }
        ClipPathShape::Ellipse { center, radii, .. } => {
            ellipse_column_bounds(*center, (radii.0 + margin, radii.1 + margin), x0, x1)
        }
        ClipPathShape::RoundedRect { rect, radii } => {
            rounded_rect_column_bounds(*rect, *radii, x0, x1, margin)
        }
        ClipPathShape::Polygon { points, .. } => polygon_column_bounds(points, x0, x1, margin),
    }
}

fn ellipse_band_bounds(
    center: (f32, f32),
    radii: (f32, f32),
    y0: f32,
    y1: f32,
) -> Option<(f32, f32)> {
    if radii.0 <= 0.0 || radii.1 <= 0.0 || y1 <= center.1 - radii.1 || y0 >= center.1 + radii.1 {
        return None;
    }
    let closest_y = center.1.clamp(y0, y1);
    let normalized_y = ((closest_y - center.1) / radii.1).clamp(-1.0, 1.0);
    let dx = radii.0 * (1.0 - normalized_y * normalized_y).max(0.0).sqrt();
    Some((center.0 - dx, center.0 + dx))
}

fn ellipse_column_bounds(
    center: (f32, f32),
    radii: (f32, f32),
    x0: f32,
    x1: f32,
) -> Option<(f32, f32)> {
    if radii.0 <= 0.0 || radii.1 <= 0.0 || x1 <= center.0 - radii.0 || x0 >= center.0 + radii.0 {
        return None;
    }
    let closest_x = center.0.clamp(x0, x1);
    let normalized_x = ((closest_x - center.0) / radii.0).clamp(-1.0, 1.0);
    let dy = radii.1 * (1.0 - normalized_x * normalized_x).max(0.0).sqrt();
    Some((center.1 - dy, center.1 + dy))
}

fn rounded_rect_band_bounds(
    rect: Rect,
    radii: (f32, f32, f32, f32),
    y0: f32,
    y1: f32,
    margin: f32,
) -> Option<(f32, f32)> {
    let expanded = Rect {
        x: rect.x - margin,
        y: rect.y - margin,
        width: rect.width + margin * 2.0,
        height: rect.height + margin * 2.0,
    };
    if y1 <= expanded.y || y0 >= expanded.y + expanded.height {
        return None;
    }
    let radii = (
        radii.0 + margin,
        radii.1 + margin,
        radii.2 + margin,
        radii.3 + margin,
    );
    let mut candidates = vec![y0, y1];
    candidates.extend([
        expanded.y + radii.0,
        expanded.y + radii.1,
        expanded.y + expanded.height - radii.2,
        expanded.y + expanded.height - radii.3,
    ]);
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    for y in candidates {
        if y < y0 || y > y1 || y < expanded.y || y > expanded.y + expanded.height {
            continue;
        }
        let (left, right) = rounded_rect_interval_at_y(expanded, radii, y);
        min_x = min_x.min(left);
        max_x = max_x.max(right);
    }
    min_x.is_finite().then_some((min_x, max_x))
}

fn rounded_rect_column_bounds(
    rect: Rect,
    radii: (f32, f32, f32, f32),
    x0: f32,
    x1: f32,
    margin: f32,
) -> Option<(f32, f32)> {
    let expanded = Rect {
        x: rect.x - margin,
        y: rect.y - margin,
        width: rect.width + margin * 2.0,
        height: rect.height + margin * 2.0,
    };
    if x1 <= expanded.x || x0 >= expanded.x + expanded.width {
        return None;
    }
    let radii = (
        radii.0 + margin,
        radii.1 + margin,
        radii.2 + margin,
        radii.3 + margin,
    );
    let mut candidates = vec![x0, x1];
    candidates.extend([
        expanded.x + radii.0,
        expanded.x + radii.3,
        expanded.x + expanded.width - radii.1,
        expanded.x + expanded.width - radii.2,
    ]);
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for x in candidates {
        if x < x0 || x > x1 || x < expanded.x || x > expanded.x + expanded.width {
            continue;
        }
        let (top, bottom) = rounded_rect_interval_at_x(expanded, radii, x);
        min_y = min_y.min(top);
        max_y = max_y.max(bottom);
    }
    min_y.is_finite().then_some((min_y, max_y))
}

fn rounded_rect_interval_at_y(rect: Rect, radii: (f32, f32, f32, f32), y: f32) -> (f32, f32) {
    let corner_offset = |radius: f32, center_y: f32| {
        if radius <= 0.0 {
            0.0
        } else {
            let dy = (y - center_y).abs().min(radius);
            radius - (radius * radius - dy * dy).max(0.0).sqrt()
        }
    };
    let left_offset = if y < rect.y + radii.0 {
        corner_offset(radii.0, rect.y + radii.0)
    } else if y > rect.y + rect.height - radii.3 {
        corner_offset(radii.3, rect.y + rect.height - radii.3)
    } else {
        0.0
    };
    let right_offset = if y < rect.y + radii.1 {
        corner_offset(radii.1, rect.y + radii.1)
    } else if y > rect.y + rect.height - radii.2 {
        corner_offset(radii.2, rect.y + rect.height - radii.2)
    } else {
        0.0
    };
    (rect.x + left_offset, rect.x + rect.width - right_offset)
}

fn rounded_rect_interval_at_x(rect: Rect, radii: (f32, f32, f32, f32), x: f32) -> (f32, f32) {
    let corner_offset = |radius: f32, center_x: f32| {
        if radius <= 0.0 {
            0.0
        } else {
            let dx = (x - center_x).abs().min(radius);
            radius - (radius * radius - dx * dx).max(0.0).sqrt()
        }
    };
    let top_offset = if x < rect.x + radii.0 {
        corner_offset(radii.0, rect.x + radii.0)
    } else if x > rect.x + rect.width - radii.1 {
        corner_offset(radii.1, rect.x + rect.width - radii.1)
    } else {
        0.0
    };
    let bottom_offset = if x < rect.x + radii.3 {
        corner_offset(radii.3, rect.x + radii.3)
    } else if x > rect.x + rect.width - radii.2 {
        corner_offset(radii.2, rect.x + rect.width - radii.2)
    } else {
        0.0
    };
    (rect.y + top_offset, rect.y + rect.height - bottom_offset)
}

fn polygon_band_bounds(points: &[(f32, f32)], y0: f32, y1: f32, margin: f32) -> Option<(f32, f32)> {
    if points.len() < 3 {
        return None;
    }
    let sample_start = y0 - margin;
    let sample_end = y1 + margin;
    let mut xs = Vec::new();
    for &(x, y) in points {
        if y >= sample_start && y <= sample_end {
            xs.push(x);
        }
    }
    for edge in points
        .windows(2)
        .chain(std::iter::once(&[points[points.len() - 1], points[0]][..]))
    {
        let (a, b) = (edge[0], edge[1]);
        for y in [sample_start, sample_end] {
            if (a.1 <= y && y <= b.1) || (b.1 <= y && y <= a.1) {
                if (b.1 - a.1).abs() < f32::EPSILON {
                    xs.extend([a.0, b.0]);
                } else {
                    let t = (y - a.1) / (b.1 - a.1);
                    xs.push(a.0 + t * (b.0 - a.0));
                }
            }
        }
    }
    let min_x = xs.iter().copied().fold(f32::INFINITY, f32::min);
    let max_x = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    min_x
        .is_finite()
        .then_some((min_x - margin, max_x + margin))
}

fn polygon_column_bounds(
    points: &[(f32, f32)],
    x0: f32,
    x1: f32,
    margin: f32,
) -> Option<(f32, f32)> {
    if points.len() < 3 {
        return None;
    }
    let sample_start = x0 - margin;
    let sample_end = x1 + margin;
    let mut ys = Vec::new();
    for &(x, y) in points {
        if x >= sample_start && x <= sample_end {
            ys.push(y);
        }
    }
    for edge in points
        .windows(2)
        .chain(std::iter::once(&[points[points.len() - 1], points[0]][..]))
    {
        let (a, b) = (edge[0], edge[1]);
        for x in [sample_start, sample_end] {
            if (a.0 <= x && x <= b.0) || (b.0 <= x && x <= a.0) {
                if (b.0 - a.0).abs() < f32::EPSILON {
                    ys.extend([a.1, b.1]);
                } else {
                    let t = (x - a.0) / (b.0 - a.0);
                    ys.push(a.1 + t * (b.1 - a.1));
                }
            }
        }
    }
    let min_y = ys.iter().copied().fold(f32::INFINITY, f32::min);
    let max_y = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    min_y
        .is_finite()
        .then_some((min_y - margin, max_y + margin))
}

fn split_top_level_whitespace(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (index, ch) in value.char_indices() {
        if ch.is_ascii_whitespace() && depth == 0 {
            if let Some(part_start) = start.take() {
                parts.push(&value[part_start..index]);
            }
            continue;
        }
        start.get_or_insert(index);
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    if let Some(part_start) = start {
        parts.push(&value[part_start..]);
    }
    parts
}
