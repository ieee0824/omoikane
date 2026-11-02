//! Glyph outlines as polygons and `BASE` table baselines, for consumers
//! that fill text as paths (Canvas 2D).

use super::Font;
use ab_glyph::{Font as AbGlyphFont, GlyphId, OutlineCurve, Point};
use rustybuzz::ttf_parser::{Face as TtfFace, Tag};

/// Horizontal baseline positions from the `BASE` table of the default
/// script, in font units above the alphabetic baseline (y up).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct HorizontalBaselines {
    pub hanging: Option<f32>,
    pub ideographic: Option<f32>,
}

impl Font {
    /// The outline of `glyph_id` as closed polygons in font units (y up).
    /// Curves are flattened so no point strays more than about `tolerance`
    /// font units from the curve. Glyphs without an outline yield none.
    pub(crate) fn glyph_contours(&self, glyph_id: u16, tolerance: f32) -> Vec<Vec<[f32; 2]>> {
        let Some(outline) = self.inner.outline(GlyphId(glyph_id)) else {
            return Vec::new();
        };
        let mut contours: Vec<Vec<[f32; 2]>> = Vec::new();
        let mut last: Option<Point> = None;
        for curve in &outline.curves {
            let start = curve_start(curve);
            if last != Some(start) {
                contours.push(vec![[start.x, start.y]]);
            }
            let Some(contour) = contours.last_mut() else {
                continue;
            };
            flatten(curve, tolerance.max(1e-3), contour);
            last = Some(curve_end(curve));
        }
        for contour in &mut contours {
            if contour.len() > 1 && contour.first() == contour.last() {
                contour.pop();
            }
        }
        contours.retain(|contour| contour.len() > 2);
        contours
    }

    /// Hanging and ideographic baselines declared by the `BASE` table for
    /// the default script; both are `None` without a usable table.
    pub(crate) fn horizontal_baselines(&self) -> HorizontalBaselines {
        TtfFace::parse(self.inner.font_data(), self.face_index)
            .ok()
            .and_then(|face| face.raw_face().table(Tag::from_bytes(b"BASE")))
            .and_then(parse_base_table)
            .unwrap_or_default()
    }
}

fn curve_start(curve: &OutlineCurve) -> Point {
    match *curve {
        OutlineCurve::Line(start, _)
        | OutlineCurve::Quad(start, ..)
        | OutlineCurve::Cubic(start, ..) => start,
    }
}

fn curve_end(curve: &OutlineCurve) -> Point {
    match *curve {
        OutlineCurve::Line(_, end)
        | OutlineCurve::Quad(_, _, end)
        | OutlineCurve::Cubic(_, _, _, end) => end,
    }
}

/// Appends the points after the start of `curve` to `out`.
fn flatten(curve: &OutlineCurve, tolerance: f32, out: &mut Vec<[f32; 2]>) {
    let distance = |a: Point, b: Point| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
    let steps = |hull: f32| ((hull / tolerance).sqrt().ceil() as usize).clamp(1, 64);
    match *curve {
        OutlineCurve::Line(_, end) => out.push([end.x, end.y]),
        OutlineCurve::Quad(p0, p1, p2) => {
            let count = steps(distance(p0, p1) + distance(p1, p2));
            for step in 1..=count {
                let t = step as f32 / count as f32;
                let u = 1.0 - t;
                out.push([
                    u * u * p0.x + 2.0 * u * t * p1.x + t * t * p2.x,
                    u * u * p0.y + 2.0 * u * t * p1.y + t * t * p2.y,
                ]);
            }
        }
        OutlineCurve::Cubic(p0, p1, p2, p3) => {
            let count = steps(distance(p0, p1) + distance(p1, p2) + distance(p2, p3));
            for step in 1..=count {
                let t = step as f32 / count as f32;
                let u = 1.0 - t;
                let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                out.push([
                    a * p0.x + b * p1.x + c * p2.x + d * p3.x,
                    a * p0.y + b * p1.y + c * p2.y + d * p3.y,
                ]);
            }
        }
    }
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

/// Reads the default script's horizontal baselines, relative to the
/// alphabetic (`romn`) baseline when the table declares one.
fn parse_base_table(data: &[u8]) -> Option<HorizontalBaselines> {
    let axis = usize::from(read_u16(data, 4)?);
    if axis == 0 {
        return None;
    }
    let tag_list = axis + usize::from(read_u16(data, axis)?);
    let script_list = axis + usize::from(read_u16(data, axis + 2)?);
    let tag_count = usize::from(read_u16(data, tag_list)?);
    let tags = (0..tag_count)
        .map(|index| data.get(tag_list + 2 + index * 4..tag_list + 6 + index * 4))
        .collect::<Option<Vec<_>>>()?;
    let script_count = usize::from(read_u16(data, script_list)?);
    // Prefer the `DFLT` script, then the first one declared.
    let script = (0..script_count)
        .map(|index| script_list + 2 + index * 6)
        .find(|record| data.get(*record..record + 4) == Some(b"DFLT"))
        .or((script_count > 0).then_some(script_list + 2))?;
    let base_script = script_list + usize::from(read_u16(data, script + 4)?);
    let values_offset = usize::from(read_u16(data, base_script)?);
    if values_offset == 0 {
        return None;
    }
    let values = base_script + values_offset;
    let coord_count = usize::from(read_u16(data, values + 2)?);
    let coordinate = |tag: &[u8]| -> Option<f32> {
        let index = tags.iter().position(|candidate| *candidate == tag)?;
        if index >= coord_count {
            return None;
        }
        let coord = values + usize::from(read_u16(data, values + 4 + index * 2)?);
        Some(read_u16(data, coord + 2)? as i16 as f32)
    };
    let alphabetic = coordinate(b"romn").unwrap_or(0.0);
    Some(HorizontalBaselines {
        hanging: coordinate(b"hang").map(|value| value - alphabetic),
        ideographic: coordinate(b"ideo").map(|value| value - alphabetic),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_tables_report_baselines_relative_to_the_alphabetic_one() {
        // A horizontal axis with hang/ideo/math/romn and a DFLT script whose
        // coordinates are 512, 128, 384 and 0.
        let table = [
            0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x04, 0x00, 0x16, 0x00, 0x04,
            b'h', b'a', b'n', b'g', b'i', b'd', b'e', b'o', b'm', b'a', b't', b'h', b'r', b'o',
            b'm', b'n', 0x00, 0x01, b'D', b'F', b'L', b'T', 0x00, 0x08, 0x00, 0x06, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x03, 0x00, 0x04, 0x00, 0x0c, 0x00, 0x10, 0x00, 0x14, 0x00, 0x18,
            0x00, 0x01, 0x02, 0x00, 0x00, 0x01, 0x00, 0x80, 0x00, 0x01, 0x01, 0x80, 0x00, 0x01,
            0x00, 0x00,
        ];
        assert_eq!(
            parse_base_table(&table),
            Some(HorizontalBaselines {
                hanging: Some(512.0),
                ideographic: Some(128.0),
            })
        );
        assert_eq!(parse_base_table(&table[..10]), None);
    }

    #[test]
    fn quadratic_curves_are_flattened_into_closed_polygons() {
        let curves = [
            OutlineCurve::Line(Point { x: 0.0, y: 0.0 }, Point { x: 10.0, y: 0.0 }),
            OutlineCurve::Quad(
                Point { x: 10.0, y: 0.0 },
                Point { x: 10.0, y: 10.0 },
                Point { x: 0.0, y: 10.0 },
            ),
            OutlineCurve::Line(Point { x: 0.0, y: 10.0 }, Point { x: 0.0, y: 0.0 }),
        ];
        let mut points = vec![[0.0, 0.0]];
        for curve in &curves {
            flatten(curve, 0.5, &mut points);
        }
        assert!(points.len() > 4);
        assert_eq!(points.last(), Some(&[0.0, 0.0]));
        assert!(
            points
                .iter()
                .all(|[x, y]| (0.0..=10.0).contains(x) && (0.0..=10.0).contains(y))
        );
    }
}
