//! Exact scalar gradient geometry and reuse of constant linear rows.
use super::*;

#[derive(Clone, Copy)]
struct Geometry {
    center_x: f32,
    center_y: f32,
    radius_x: f32,
    radius_y: f32,
    basis: f32,
    radial_scale: f32,
    zero_height: bool,
}

impl Geometry {
    fn resolve(gradient: &Gradient, area: Rect) -> Self {
        match gradient.kind {
            GradientKind::Linear { angle_deg } => {
                let angle = angle_deg.to_radians();
                let dx = angle.sin();
                let dy = -angle.cos();
                let length = dx.abs() * area.width + dy.abs() * area.height;
                Self {
                    center_x: area.x + area.width / 2.0,
                    center_y: area.y + area.height / 2.0,
                    radius_x: dx,
                    radius_y: dy,
                    basis: length,
                    radial_scale: length,
                    zero_height: false,
                }
            }
            GradientKind::Radial {
                circle,
                extent,
                center,
            } => radial_geometry(circle, extent, center, area),
            GradientKind::Conic { center, .. } => Self {
                center_x: resolve_component(center.x, area.x, area.width),
                center_y: resolve_component(center.y, area.y, area.height),
                radius_x: 0.0,
                radius_y: 0.0,
                basis: 1.0,
                radial_scale: 1.0,
                zero_height: false,
            },
        }
    }

    fn value(&self, kind: &GradientKind, px: i32, py: i32) -> f32 {
        let x = px as f32 + 0.5 - self.center_x;
        let y = py as f32 + 0.5 - self.center_y;
        match *kind {
            GradientKind::Linear { .. } => x * self.radius_x + y * self.radius_y + self.basis / 2.0,
            GradientKind::Radial { .. } => {
                ((x / self.radius_x).powi(2) + (y / self.radius_y).powi(2)).sqrt()
                    * self.radial_scale
            }
            GradientKind::Conic { from_deg, .. } => {
                ((x.atan2(-y).to_degrees() - from_deg).rem_euclid(360.0)) / 360.0
            }
        }
    }

    /// Linear projection is monotone across a row, including f32 rounding.
    /// Equal finite endpoint bits therefore prove every sample has that exact
    /// parameter. Equal colors alone would not prove this across hard stops.
    fn constant_row_value(self, kind: &GradientKind, x0: i32, x1: i32, py: i32) -> Option<f32> {
        if !matches!(kind, GradientKind::Linear { .. }) || x0 >= x1 {
            return None;
        }
        let first = self.value(kind, x0, py);
        let last = self.value(kind, x1 - 1, py);
        (first.is_finite() && last.is_finite() && first.to_bits() == last.to_bits())
            .then_some(first)
    }
}

pub(super) fn paint<const REUSE_ROWS: bool>(
    canvas: &mut Canvas,
    gradient: &Gradient,
    area: Rect,
    draw_area: Rect,
) {
    let geometry = Geometry::resolve(gradient, area);
    let stops = resolved_stops(&gradient.stops, geometry.basis);
    if REUSE_ROWS && matches!(gradient.kind, GradientKind::Linear { .. }) {
        paint_linear_rows(canvas, gradient, geometry, &stops, draw_area);
    } else {
        paint_scalar(canvas, gradient, geometry, &stops, draw_area);
    }
}

fn pixel_bounds(canvas: &Canvas, area: Rect) -> (i32, i32, i32, i32) {
    (
        area.x.floor().max(0.0) as i32,
        area.y.floor().max(0.0) as i32,
        (area.x + area.width).ceil().min(canvas.width as f32) as i32,
        (area.y + area.height).ceil().min(canvas.height as f32) as i32,
    )
}

fn paint_linear_rows(
    canvas: &mut Canvas,
    gradient: &Gradient,
    geometry: Geometry,
    stops: &[(Color, f32, Option<f32>)],
    draw_area: Rect,
) {
    let (x0, y0, x1, y1) = pixel_bounds(canvas, draw_area);
    for py in y0..y1 {
        let row_color = geometry
            .constant_row_value(&gradient.kind, x0, x1, py)
            .map(|value| {
                #[cfg(test)]
                REUSED_ROWS.with(|count| count.set(count.get() + 1));
                sample_stops(stops, value, gradient.repeating)
            });
        for px in x0..x1 {
            let color = if let Some(color) = row_color {
                color
            } else {
                let x = px as f32 + 0.5 - geometry.center_x;
                let y = py as f32 + 0.5 - geometry.center_y;
                let value = x * geometry.radius_x + y * geometry.radius_y + geometry.basis / 2.0;
                sample_stops(stops, value, gradient.repeating)
            };
            let index = ((py as u32 * canvas.width + px as u32) * 4) as usize;
            if index + 3 < canvas.pixels.len() {
                blend_pixel(&mut canvas.pixels[index..index + 4], color);
            }
        }
    }
}

// Nonlinear gradients and scalar-reference tests do not classify rows.
// Resolve the constant degenerate color once before traversing pixels.
fn paint_scalar(
    canvas: &mut Canvas,
    gradient: &Gradient,
    geometry: Geometry,
    stops: &[(Color, f32, Option<f32>)],
    draw_area: Rect,
) {
    let degenerate_color = geometry.zero_height.then(|| {
        if gradient.repeating {
            average_gradient_color(stops)
        } else {
            stops.last().unwrap().0
        }
    });
    let (x0, y0, x1, y1) = pixel_bounds(canvas, draw_area);
    let Geometry {
        center_x,
        center_y,
        radius_x,
        radius_y,
        basis,
        radial_scale,
        ..
    } = geometry;
    for py in y0..y1 {
        for px in x0..x1 {
            let x = px as f32 + 0.5 - center_x;
            let y = py as f32 + 0.5 - center_y;
            let color = if let Some(color) = degenerate_color {
                color
            } else {
                let value = match gradient.kind {
                    GradientKind::Linear { .. } => x * radius_x + y * radius_y + basis / 2.0,
                    GradientKind::Radial { .. } => {
                        ((x / radius_x).powi(2) + (y / radius_y).powi(2)).sqrt() * radial_scale
                    }
                    GradientKind::Conic { from_deg, .. } => {
                        ((x.atan2(-y).to_degrees() - from_deg).rem_euclid(360.0)) / 360.0
                    }
                };
                sample_stops(stops, value, gradient.repeating)
            };
            let index = ((py as u32 * canvas.width + px as u32) * 4) as usize;
            if index + 3 < canvas.pixels.len() {
                blend_pixel(&mut canvas.pixels[index..index + 4], color);
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    static REUSED_ROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests;

fn radial_geometry(
    circle: bool,
    extent: RadialExtent,
    center: GradientPosition,
    area: Rect,
) -> Geometry {
    let cx = resolve_component(center.x, area.x, area.width);
    let cy = resolve_component(center.y, area.y, area.height);
    let left = (cx - area.x).abs();
    let right = (area.x + area.width - cx).abs();
    let top = (cy - area.y).abs();
    let bottom = (area.y + area.height - cy).abs();
    let (mut rx, mut ry) = match extent {
        RadialExtent::ClosestSide => (left.min(right), top.min(bottom)),
        RadialExtent::FarthestSide => (left.max(right), top.max(bottom)),
        RadialExtent::ClosestCorner | RadialExtent::FarthestCorner => {
            let choose = if matches!(extent, RadialExtent::ClosestCorner) {
                f32::min
            } else {
                f32::max
            };
            let x = choose(left, right);
            let y = choose(top, bottom);
            (x, y)
        }
        RadialExtent::Explicit(x, y) => (
            resolve_length(x, area.width),
            resolve_length(y.unwrap_or(x), area.height),
        ),
    };
    if circle {
        let radius = match extent {
            RadialExtent::ClosestSide => left.min(right).min(top.min(bottom)),
            RadialExtent::FarthestSide => left.max(right).max(top.max(bottom)),
            RadialExtent::ClosestCorner => {
                [(left, top), (left, bottom), (right, top), (right, bottom)]
                    .into_iter()
                    .map(|(x, y)| x.hypot(y))
                    .fold(f32::INFINITY, f32::min)
            }
            RadialExtent::FarthestCorner => {
                [(left, top), (left, bottom), (right, top), (right, bottom)]
                    .into_iter()
                    .map(|(x, y)| x.hypot(y))
                    .fold(0.0, f32::max)
            }
            RadialExtent::Explicit(x, _) => resolve_length(x, area.width.min(area.height)),
        };
        rx = radius;
        ry = radius;
    } else if matches!(
        extent,
        RadialExtent::ClosestCorner | RadialExtent::FarthestCorner
    ) {
        // Scale the side-based ellipse so the selected corner lies on it.
        let scale = 2.0_f32.sqrt();
        rx *= scale;
        ry *= scale;
    }
    const DEGENERATE_RADIUS: f32 = 1.0e-6;
    let mut stop_basis = rx;
    let mut radial_scale = rx;
    let mut zero_height = false;
    if circle && rx <= 0.0 {
        rx = DEGENERATE_RADIUS;
        ry = DEGENERATE_RADIUS;
        stop_basis = 0.0;
        radial_scale = DEGENERATE_RADIUS;
    } else if !circle && rx <= 0.0 {
        rx = DEGENERATE_RADIUS;
        ry = 1.0 / DEGENERATE_RADIUS;
        stop_basis = 0.0;
        radial_scale = DEGENERATE_RADIUS;
    } else if !circle && ry <= 0.0 {
        zero_height = true;
    }
    Geometry {
        center_x: cx,
        center_y: cy,
        radius_x: rx,
        radius_y: ry,
        basis: stop_basis,
        radial_scale,
        zero_height,
    }
}
