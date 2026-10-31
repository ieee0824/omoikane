//! Stroke outlines (HTML §4.12.5.1.5 "trace a path").
//!
//! The outline is a set of polygons that all wind the same way, so filling
//! them with the nonzero rule paints their union.

use serde::Deserialize;

use super::SubPath;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LineJoin {
    Round,
    Bevel,
    #[default]
    Miter,
}

/// The line styles that shape a stroke, in user space.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StrokeStyle {
    pub(crate) width: f64,
    #[serde(default)]
    pub(crate) cap: LineCap,
    #[serde(default)]
    pub(crate) join: LineJoin,
    #[serde(default = "default_miter_limit")]
    pub(crate) miter_limit: f64,
    #[serde(default)]
    pub(crate) dash: Vec<f64>,
    #[serde(default)]
    pub(crate) dash_offset: f64,
}

fn default_miter_limit() -> f64 {
    10.0
}

type Point = [f64; 2];

/// The polygons covering the stroke of `paths` with `style`.
pub(crate) fn outline(paths: &[SubPath], style: &StrokeStyle) -> Vec<SubPath> {
    let mut polygons = Vec::new();
    if !(style.width > 0.0) || !style.width.is_finite() {
        return polygons;
    }
    for path in paths {
        let points = dedup(&path.points);
        if points.len() < 2 {
            continue;
        }
        if style.dash.iter().any(|length| *length > 0.0) {
            for dash in dashes(&points, path.closed, style) {
                trace(&dash, false, style, &mut polygons);
            }
        } else {
            trace(&points, path.closed, style, &mut polygons);
        }
    }
    for polygon in &mut polygons {
        if signed_area(&polygon.points) < 0.0 {
            polygon.points.reverse();
        }
    }
    polygons
}

/// Removes zero-length segments.
fn dedup(points: &[Point]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for point in points {
        if !point.iter().all(|value| value.is_finite()) {
            continue;
        }
        if out.last() != Some(point) {
            out.push(*point);
        }
    }
    out
}

/// Splits a polyline into its "on" dash intervals.
fn dashes(points: &[Point], closed: bool, style: &StrokeStyle) -> Vec<Vec<Point>> {
    let mut vertices = points.to_vec();
    if closed && vertices.first() != vertices.last() {
        vertices.push(vertices[0]);
    }
    let pattern = &style.dash;
    let period: f64 = pattern.iter().sum();
    let mut offset = style.dash_offset % period;
    if offset < 0.0 {
        offset += period;
    }
    // Find the dash entry and the distance into it at the path start.
    let mut index = 0;
    while offset >= pattern[index] {
        offset -= pattern[index];
        index = (index + 1) % pattern.len();
    }
    let mut remaining = pattern[index] - offset;
    let mut on = index % 2 == 0;
    let mut out = Vec::new();
    let mut current: Vec<Point> = if on { vec![vertices[0]] } else { Vec::new() };
    for segment in vertices.windows(2) {
        let (a, b) = (segment[0], segment[1]);
        let length = distance(a, b);
        let mut travelled = 0.0;
        while length - travelled > remaining {
            travelled += remaining;
            let point = lerp(a, b, travelled / length);
            if on {
                current.push(point);
                out.push(std::mem::take(&mut current));
            } else {
                current = vec![point];
            }
            on = !on;
            index = (index + 1) % pattern.len();
            remaining = pattern[index];
        }
        remaining -= length - travelled;
        if on {
            current.push(b);
        }
    }
    if on && current.len() > 1 {
        out.push(current);
    }
    out.into_iter()
        .map(|dash| dedup(&dash))
        .filter(|dash| dash.len() > 1)
        .collect()
}

/// Adds the outline of one polyline to `out`.
fn trace(points: &[Point], closed: bool, style: &StrokeStyle, out: &mut Vec<SubPath>) {
    let half = style.width / 2.0;
    let mut vertices = points.to_vec();
    if closed && vertices.len() > 1 && vertices.first() == vertices.last() {
        vertices.pop();
    }
    let count = vertices.len();
    let segments = if closed && count > 1 {
        count
    } else {
        count - 1
    };
    for index in 0..segments {
        let (a, b) = (vertices[index], vertices[(index + 1) % count]);
        let normal = scale(unit_normal(a, b), half);
        polygon(
            out,
            vec![
                add(a, normal),
                add(b, normal),
                sub(b, normal),
                sub(a, normal),
            ],
        );
    }
    let joins: Vec<usize> = if closed && count > 1 {
        (0..count).collect()
    } else {
        (1..count - 1).collect()
    };
    for index in joins {
        let previous = vertices[(index + count - 1) % count];
        let next = vertices[(index + 1) % count];
        join(out, previous, vertices[index], next, style);
    }
    if !(closed && count > 1) {
        cap(out, vertices[1], vertices[0], style);
        cap(out, vertices[count - 2], vertices[count - 1], style);
    }
}

fn join(out: &mut Vec<SubPath>, previous: Point, vertex: Point, next: Point, style: &StrokeStyle) {
    let half = style.width / 2.0;
    if style.join == LineJoin::Round {
        circle(out, vertex, half);
        return;
    }
    let (n0, n1) = (unit_normal(previous, vertex), unit_normal(vertex, next));
    let turn = cross(sub(vertex, previous), sub(next, vertex));
    if turn == 0.0 && dot(n0, n1) > 0.0 {
        return;
    }
    // The outer side of the turn is opposite to its direction.
    let side = if turn > 0.0 { -1.0 } else { 1.0 };
    let (o0, o1) = (
        add(vertex, scale(n0, half * side)),
        add(vertex, scale(n1, half * side)),
    );
    if style.join == LineJoin::Miter {
        let cos = dot(n0, n1).clamp(-1.0, 1.0);
        // The miter length ratio is 1 / sin(θ/2) for the angle θ between the
        // segments, which is 1 / cos(φ/2) for the angle φ between normals.
        let half_cos = ((1.0 + cos) / 2.0).sqrt();
        if half_cos > 0.0 && 1.0 / half_cos <= style.miter_limit {
            let bisector = normalize(add(n0, n1));
            let tip = add(vertex, scale(bisector, side * half / half_cos));
            polygon(out, vec![vertex, o0, tip, o1]);
            return;
        }
    }
    polygon(out, vec![vertex, o0, o1]);
}

/// The cap at `end` of the segment from `from`.
fn cap(out: &mut Vec<SubPath>, from: Point, end: Point, style: &StrokeStyle) {
    let half = style.width / 2.0;
    match style.cap {
        LineCap::Butt => {}
        LineCap::Round => circle(out, end, half),
        LineCap::Square => {
            let direction = scale(normalize(sub(end, from)), half);
            let normal = [-direction[1], direction[0]];
            let far = add(end, direction);
            polygon(
                out,
                vec![
                    add(end, normal),
                    add(far, normal),
                    sub(far, normal),
                    sub(end, normal),
                ],
            );
        }
    }
}

fn circle(out: &mut Vec<SubPath>, center: Point, radius: f64) {
    let steps = ((radius * 4.0).ceil() as usize).clamp(8, 256);
    let points = (0..steps)
        .map(|step| {
            let angle = step as f64 / steps as f64 * std::f64::consts::TAU;
            [
                center[0] + radius * angle.cos(),
                center[1] + radius * angle.sin(),
            ]
        })
        .collect();
    polygon(out, points);
}

fn polygon(out: &mut Vec<SubPath>, points: Vec<Point>) {
    out.push(SubPath {
        points,
        closed: true,
    });
}

pub(crate) fn signed_area(points: &[Point]) -> f64 {
    let mut area = 0.0;
    for index in 0..points.len() {
        let (a, b) = (points[index], points[(index + 1) % points.len()]);
        area += a[0] * b[1] - b[0] * a[1];
    }
    area / 2.0
}

fn unit_normal(a: Point, b: Point) -> Point {
    let direction = normalize(sub(b, a));
    [-direction[1], direction[0]]
}

fn normalize(v: Point) -> Point {
    let length = v[0].hypot(v[1]);
    if length == 0.0 {
        [0.0, 0.0]
    } else {
        [v[0] / length, v[1] / length]
    }
}

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn scale(v: Point, factor: f64) -> Point {
    [v[0] * factor, v[1] * factor]
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn distance(a: Point, b: Point) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

fn lerp(a: Point, b: Point, t: f64) -> Point {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

#[cfg(test)]
mod tests {
    use super::super::{FillRule, raster};
    use super::*;

    fn style(width: f64) -> StrokeStyle {
        StrokeStyle {
            width,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 10.0,
            dash: Vec::new(),
            dash_offset: 0.0,
        }
    }

    fn line(points: &[Point], closed: bool) -> Vec<SubPath> {
        vec![SubPath {
            points: points.to_vec(),
            closed,
        }]
    }

    fn coverage(paths: &[SubPath], style: &StrokeStyle, x: usize, y: usize) -> f32 {
        raster::fill_mask(&outline(paths, style), FillRule::NonZero, 20, 20).at(x, y)
    }

    #[test]
    fn caps_extend_open_lines() {
        let path = line(&[[5.0, 10.0], [15.0, 10.0]], false);
        assert_eq!(coverage(&path, &style(4.0), 10, 9), 1.0);
        assert_eq!(coverage(&path, &style(4.0), 4, 10), 0.0);
        let square = StrokeStyle {
            cap: LineCap::Square,
            ..style(4.0)
        };
        assert_eq!(coverage(&path, &square, 4, 10), 1.0);
    }

    #[test]
    fn miter_joins_fill_corners_within_the_limit() {
        let path = line(&[[2.0, 10.0], [10.0, 10.0], [10.0, 2.0]], false);
        assert_eq!(coverage(&path, &style(4.0), 11, 11), 1.0);
        let bevel = StrokeStyle {
            join: LineJoin::Bevel,
            ..style(4.0)
        };
        assert_eq!(coverage(&path, &bevel, 11, 11), 0.0);
        let limited = StrokeStyle {
            miter_limit: 1.0,
            ..style(4.0)
        };
        assert_eq!(coverage(&path, &limited, 11, 11), 0.0);
    }

    #[test]
    fn dashes_split_lines() {
        let dashed = StrokeStyle {
            dash: vec![2.0, 2.0],
            ..style(2.0)
        };
        let path = line(&[[0.0, 10.0], [20.0, 10.0]], false);
        assert_eq!(coverage(&path, &dashed, 0, 9), 1.0);
        assert_eq!(coverage(&path, &dashed, 2, 9), 0.0);
        assert_eq!(coverage(&path, &dashed, 4, 9), 1.0);
        let shifted = StrokeStyle {
            dash_offset: 2.0,
            ..dashed
        };
        assert_eq!(coverage(&path, &shifted, 0, 9), 0.0);
    }

    #[test]
    fn closed_two_point_paths_join_at_both_ends() {
        let path = line(&[[5.0, 10.0], [15.0, 10.0], [5.0, 10.0]], true);
        let round = StrokeStyle {
            join: LineJoin::Round,
            ..style(6.0)
        };
        assert_eq!(coverage(&path, &round, 3, 10), 1.0);
        assert_eq!(coverage(&path, &style(6.0), 3, 10), 0.0);
    }

    #[test]
    fn closed_paths_join_at_the_start() {
        let path = line(&[[5.0, 5.0], [15.0, 5.0], [15.0, 15.0], [5.0, 15.0]], true);
        assert_eq!(coverage(&path, &style(2.0), 4, 4), 1.0);
        assert_eq!(coverage(&path, &style(2.0), 10, 10), 0.0);
    }
}
