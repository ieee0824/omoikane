//! Anti-aliased coverage of filled paths.

use super::{FillRule, SubPath};

/// Vertical samples per pixel row.
const SUBSAMPLES: usize = 4;

/// Per-pixel coverage in `0.0..=1.0` over a `width × height` surface.
#[derive(Debug, Clone)]
pub(crate) struct Mask {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) coverage: Vec<f32>,
    /// The rows and columns holding nonzero coverage, as
    /// `(left, top, right, bottom)` with exclusive ends.
    pub(crate) bounds: (usize, usize, usize, usize),
}

impl Mask {
    /// A mask with full coverage everywhere.
    pub(crate) fn full(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            coverage: vec![1.0; width * height],
            bounds: (0, 0, width, height),
        }
    }

    pub(crate) fn at(&self, x: usize, y: usize) -> f32 {
        self.coverage[y * self.width + x]
    }

    /// Multiplies this mask by `other` (clip intersection).
    pub(crate) fn intersect(&mut self, other: &Mask) {
        for (value, factor) in self.coverage.iter_mut().zip(&other.coverage) {
            *value *= factor;
        }
        let (a, b) = (self.bounds, other.bounds);
        self.bounds = (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3));
        if self.bounds.0 >= self.bounds.2 || self.bounds.1 >= self.bounds.3 {
            self.bounds = (0, 0, 0, 0);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.bounds.0 >= self.bounds.2 || self.bounds.1 >= self.bounds.3
    }
}

struct Edge {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    /// +1 for a downward edge, -1 for an upward one.
    winding: i32,
}

/// Rasterizes `paths` (each implicitly closed) with `rule`.
pub(crate) fn fill_mask(paths: &[SubPath], rule: FillRule, width: usize, height: usize) -> Mask {
    let mut edges = edges(paths);
    let mut mask = Mask {
        width,
        height,
        coverage: vec![0.0; width * height],
        bounds: (0, 0, 0, 0),
    };
    if edges.is_empty() || width == 0 || height == 0 {
        return mask;
    }
    edges.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    let top = edges[0].y0.floor().max(0.0) as usize;
    let bottom = edges
        .iter()
        .map(|edge| edge.y1)
        .fold(f64::MIN, f64::max)
        .ceil()
        .clamp(0.0, height as f64) as usize;
    let (mut left, mut right, mut first_row, mut last_row) = (width, 0, height, 0);
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    let mut start = 0;
    for row in top..bottom {
        let line = &mut mask.coverage[row * width..(row + 1) * width];
        for sample in 0..SUBSAMPLES {
            let y = row as f64 + (sample as f64 + 0.5) / SUBSAMPLES as f64;
            // Edges that ended above `y` move to the front of the scan window.
            while start < edges.len() && edges[start].y1 <= y {
                start += 1;
            }
            crossings.clear();
            for edge in &edges[start..] {
                if edge.y0 > y {
                    break;
                }
                if edge.y1 > y {
                    let t = (y - edge.y0) / (edge.y1 - edge.y0);
                    crossings.push((edge.x0 + t * (edge.x1 - edge.x0), edge.winding));
                }
            }
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            for pair in 0..crossings.len() {
                winding += crossings[pair].1;
                let inside = match rule {
                    FillRule::NonZero => winding != 0,
                    FillRule::EvenOdd => winding % 2 != 0,
                };
                if inside && pair + 1 < crossings.len() {
                    let span = (crossings[pair].0, crossings[pair + 1].0);
                    if let Some((a, b)) = add_span(line, span, 1.0 / SUBSAMPLES as f32) {
                        left = left.min(a);
                        right = right.max(b);
                        first_row = first_row.min(row);
                        last_row = last_row.max(row + 1);
                    }
                }
            }
        }
    }
    if left < right {
        mask.bounds = (left, first_row, right, last_row);
    }
    mask
}

/// Adds `weight` × the horizontal coverage of `span` to `line`, returning
/// the touched columns.
fn add_span(line: &mut [f32], span: (f64, f64), weight: f32) -> Option<(usize, usize)> {
    let width = line.len() as f64;
    let (start, end) = (span.0.max(0.0), span.1.min(width));
    if start >= end {
        return None;
    }
    let first = start.floor() as usize;
    let last = (end.ceil() as usize).min(line.len());
    for (column, value) in line.iter_mut().enumerate().take(last).skip(first) {
        let covered = (end.min(column as f64 + 1.0) - start.max(column as f64)).max(0.0);
        *value = (*value + weight * covered as f32).min(1.0);
    }
    Some((first, last))
}

fn edges(paths: &[SubPath]) -> Vec<Edge> {
    let mut edges = Vec::new();
    for path in paths {
        let points = &path.points;
        if points.len() < 2 {
            continue;
        }
        for index in 0..points.len() {
            let a = points[index];
            let b = points[(index + 1) % points.len()];
            if a[1] == b[1] || !a.iter().chain(&b).all(|value| value.is_finite()) {
                continue;
            }
            let (top, bottom, winding) = if a[1] < b[1] { (a, b, 1) } else { (b, a, -1) };
            edges.push(Edge {
                x0: top[0],
                y0: top[1],
                x1: bottom[0],
                y1: bottom[1],
                winding,
            });
        }
    }
    edges
}

/// Whether `point` is inside `paths` under `rule`; points on an edge count
/// as inside.
pub(crate) fn contains(paths: &[SubPath], rule: FillRule, point: [f64; 2]) -> bool {
    let [x, y] = point;
    let mut winding = 0;
    for path in paths {
        let points = &path.points;
        for index in 0..points.len() {
            let a = points[index];
            let b = points[(index + 1) % points.len()];
            if on_segment(a, b, point) {
                return true;
            }
            if (a[1] <= y) != (b[1] <= y) {
                let crossing = a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
                if crossing > x {
                    winding += if b[1] > a[1] { 1 } else { -1 };
                }
            }
        }
    }
    match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    }
}

fn on_segment(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    let cross = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    if cross.abs() > 1e-9 * (1.0 + (b[0] - a[0]).abs() + (b[1] - a[1]).abs()) {
        return false;
    }
    p[0] >= a[0].min(b[0]) - 1e-9
        && p[0] <= a[0].max(b[0]) + 1e-9
        && p[1] >= a[1].min(b[1]) - 1e-9
        && p[1] <= a[1].max(b[1]) + 1e-9
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> SubPath {
        SubPath {
            points: vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            closed: true,
        }
    }

    #[test]
    fn aligned_rectangles_have_exact_coverage() {
        let mask = fill_mask(&[rect(1.0, 1.0, 2.0, 2.0)], FillRule::NonZero, 4, 4);
        assert_eq!(mask.bounds, (1, 1, 3, 3));
        assert_eq!(mask.at(1, 1), 1.0);
        assert_eq!(mask.at(2, 2), 1.0);
        assert_eq!(mask.at(0, 0), 0.0);
        assert_eq!(mask.at(3, 1), 0.0);
    }

    #[test]
    fn half_pixel_edges_are_antialiased() {
        let mask = fill_mask(&[rect(0.5, 0.0, 1.0, 1.0)], FillRule::NonZero, 3, 1);
        assert_eq!(mask.coverage, [0.5, 0.5, 0.0]);
    }

    #[test]
    fn fill_rules_differ_for_nested_paths() {
        let paths = [rect(0.0, 0.0, 4.0, 4.0), rect(1.0, 1.0, 2.0, 2.0)];
        assert_eq!(fill_mask(&paths, FillRule::NonZero, 4, 4).at(2, 2), 1.0);
        assert_eq!(fill_mask(&paths, FillRule::EvenOdd, 4, 4).at(2, 2), 0.0);
        assert!(contains(&paths, FillRule::NonZero, [2.0, 2.0]));
        assert!(!contains(&paths, FillRule::EvenOdd, [2.5, 2.5]));
        assert!(contains(&paths, FillRule::EvenOdd, [0.0, 2.0]));
    }
}
