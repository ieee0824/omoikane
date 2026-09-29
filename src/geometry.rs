//! Geometry shared by SVG fill hit testing and CSS clipping.

/// Whether a polygon is being used as an SVG fill or as a CSS clipping region.
#[derive(Clone, Copy)]
pub(crate) enum PolygonUse {
    /// SVG 2 includes points on a path, including a path with no area.
    SvgFill,
    /// A zero-area CSS clipping region does not receive pointer hits.
    CssClip,
}

/// Tests polygon containment with an explicit rule for a zero-area boundary.
///
/// Both uses include points on the boundary of a nondegenerate polygon. The
/// calculations use `f64` for intermediate products so a fixed, scale-dependent
/// tolerance is not needed for `f32` geometry.
pub(crate) fn point_in_polygon(
    point: (f32, f32),
    points: &[(f32, f32)],
    use_case: PolygonUse,
) -> bool {
    if points.len() < 2
        || !point.0.is_finite()
        || !point.1.is_finite()
        || points.iter().any(|(x, y)| !x.is_finite() || !y.is_finite())
    {
        return false;
    }

    let mut inside = false;
    let mut on_boundary = false;
    let mut previous = *points.last().unwrap();
    for &current in points {
        let (ax, ay) = (f64::from(previous.0), f64::from(previous.1));
        let (bx, by) = (f64::from(current.0), f64::from(current.1));
        let (px, py) = (f64::from(point.0), f64::from(point.1));
        let cross = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
        if cross == 0.0
            && px >= ax.min(bx)
            && px <= ax.max(bx)
            && py >= ay.min(by)
            && py <= ay.max(by)
        {
            on_boundary = true;
        }
        if (by > py) != (ay > py) && px < (ax - bx) * (py - by) / (ay - by) + bx {
            inside = !inside;
        }
        previous = current;
    }

    if matches!(use_case, PolygonUse::CssClip) && !has_area(points) {
        return false;
    }
    on_boundary || inside
}

fn has_area(points: &[(f32, f32)]) -> bool {
    if points.len() < 3 {
        return false;
    }
    let origin = points[0];
    let Some(other) = points.iter().copied().find(|point| *point != origin) else {
        return false;
    };
    points.iter().copied().any(|point| {
        let dx = f64::from(other.0) - f64::from(origin.0);
        let dy = f64::from(other.1) - f64::from(origin.1);
        let px = f64::from(point.0) - f64::from(origin.0);
        let py = f64::from(point.1) - f64::from(origin.1);
        dx * py - dy * px != 0.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_intersecting_polygon_is_not_mistaken_for_zero_area() {
        let bow_tie = [(0.0, 0.0), (10.0, 10.0), (0.0, 10.0), (10.0, 0.0)];
        assert!(point_in_polygon((5.0, 2.0), &bow_tie, PolygonUse::CssClip));
        assert!(!point_in_polygon(
            (5.0, -0.0001),
            &bow_tie,
            PolygonUse::CssClip
        ));
    }
}
