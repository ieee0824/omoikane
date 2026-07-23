//! CSS Transforms Level 1 parsing and two-dimensional matrix primitives.

use std::f32::consts::PI;

/// A two-dimensional affine transform using the CSS `matrix(a,b,c,d,e,f)`
/// convention.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AffineTransform {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Default for AffineTransform {
    fn default() -> Self {
        Self::identity()
    }
}

impl AffineTransform {
    pub const fn identity() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    pub fn is_identity(self) -> bool {
        approximately(self.a, 1.0)
            && approximately(self.b, 0.0)
            && approximately(self.c, 0.0)
            && approximately(self.d, 1.0)
            && approximately(self.e, 0.0)
            && approximately(self.f, 0.0)
    }

    /// Matrix multiplication. `self.multiply(other)` produces `self * other`,
    /// matching the order used by CSS transform lists.
    pub fn multiply(self, other: Self) -> Self {
        Self {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    pub fn translate(x: f32, y: f32) -> Self {
        Self {
            e: x,
            f: y,
            ..Self::identity()
        }
    }

    pub fn scale(x: f32, y: f32) -> Self {
        Self {
            a: x,
            d: y,
            ..Self::identity()
        }
    }

    pub fn rotate(radians: f32) -> Self {
        let (sin, cos) = radians.sin_cos();
        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        }
    }

    pub fn skew(x_radians: f32, y_radians: f32) -> Self {
        Self {
            a: 1.0,
            b: y_radians.tan(),
            c: x_radians.tan(),
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    pub fn transform_point(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    pub fn inverse(self) -> Option<Self> {
        let determinant = self.a * self.d - self.b * self.c;
        if !determinant.is_finite() || determinant.abs() < 1e-8 {
            return None;
        }
        let inverse = 1.0 / determinant;
        Some(Self {
            a: self.d * inverse,
            b: -self.b * inverse,
            c: -self.c * inverse,
            d: self.a * inverse,
            e: (self.c * self.f - self.d * self.e) * inverse,
            f: (self.b * self.e - self.a * self.f) * inverse,
        })
    }

    /// Applies a transform around an absolute CSS pixel origin.
    pub fn around(self, origin_x: f32, origin_y: f32) -> Self {
        Self::translate(origin_x, origin_y)
            .multiply(self)
            .multiply(Self::translate(-origin_x, -origin_y))
    }
}

fn approximately(left: f32, right: f32) -> bool {
    (left - right).abs() <= 1e-5
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TransformReferenceBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub font_size: f32,
    pub root_font_size: f32,
}

/// Parses a CSS 2D transform list and returns the absolute-coordinate matrix,
/// including `transform-origin`.
pub(crate) fn parse_transform_with_origin(
    transform: &str,
    origin: &str,
    reference: TransformReferenceBox,
) -> Option<AffineTransform> {
    let matrix = parse_transform_list(transform, reference)?;
    if matrix.is_identity() {
        return Some(matrix);
    }
    let (origin_x, origin_y) = parse_transform_origin(origin, reference)?;
    Some(matrix.around(origin_x, origin_y))
}

pub(crate) fn parse_transform_list(
    value: &str,
    reference: TransformReferenceBox,
) -> Option<AffineTransform> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") || is_css_wide_keyword(value) {
        return Some(AffineTransform::identity());
    }
    if value.is_empty() {
        return None;
    }

    let mut result = AffineTransform::identity();
    let mut cursor = 0;
    let bytes = value.as_bytes();
    while cursor < value.len() {
        while cursor < value.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == value.len() {
            break;
        }
        let name_start = cursor;
        while cursor < value.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'-')
        {
            cursor += 1;
        }
        if cursor == name_start || bytes.get(cursor) != Some(&b'(') {
            return None;
        }
        let name = &value[name_start..cursor];
        cursor += 1;
        let args_start = cursor;
        let mut depth = 1usize;
        while cursor < value.len() && depth > 0 {
            match bytes[cursor] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            cursor += 1;
        }
        if depth != 0 {
            return None;
        }
        let args = &value[args_start..cursor - 1];
        result = result.multiply(parse_transform_function(name, args, reference)?);
    }
    Some(result)
}

fn parse_transform_function(
    name: &str,
    args: &str,
    reference: TransformReferenceBox,
) -> Option<AffineTransform> {
    let args = split_args(args)?;
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "matrix" if args.len() == 6 => Some(AffineTransform {
            a: parse_number(args[0])?,
            b: parse_number(args[1])?,
            c: parse_number(args[2])?,
            d: parse_number(args[3])?,
            e: parse_number(args[4])?,
            f: parse_number(args[5])?,
        }),
        "translate" if (1..=2).contains(&args.len()) => Some(AffineTransform::translate(
            parse_length_percentage(args[0], reference.width, reference)?,
            if args.len() == 2 {
                parse_length_percentage(args[1], reference.height, reference)?
            } else {
                0.0
            },
        )),
        "translatex" if args.len() == 1 => Some(AffineTransform::translate(
            parse_length_percentage(args[0], reference.width, reference)?,
            0.0,
        )),
        "translatey" if args.len() == 1 => Some(AffineTransform::translate(
            0.0,
            parse_length_percentage(args[0], reference.height, reference)?,
        )),
        "translate3d" if args.len() == 3 && parse_zero_length(args[2]) => {
            Some(AffineTransform::translate(
                parse_length_percentage(args[0], reference.width, reference)?,
                parse_length_percentage(args[1], reference.height, reference)?,
            ))
        }
        "scale" if (1..=2).contains(&args.len()) => {
            let x = parse_scale(args[0])?;
            let y = if args.len() == 2 {
                parse_scale(args[1])?
            } else {
                x
            };
            Some(AffineTransform::scale(x, y))
        }
        "scalex" if args.len() == 1 => Some(AffineTransform::scale(parse_scale(args[0])?, 1.0)),
        "scaley" if args.len() == 1 => Some(AffineTransform::scale(1.0, parse_scale(args[0])?)),
        "rotate" if args.len() == 1 => Some(AffineTransform::rotate(parse_angle(args[0])?)),
        "skew" if (1..=2).contains(&args.len()) => Some(AffineTransform::skew(
            parse_angle(args[0])?,
            if args.len() == 2 {
                parse_angle(args[1])?
            } else {
                0.0
            },
        )),
        "skewx" if args.len() == 1 => Some(AffineTransform::skew(parse_angle(args[0])?, 0.0)),
        "skewy" if args.len() == 1 => Some(AffineTransform::skew(0.0, parse_angle(args[0])?)),
        _ => None,
    }
}

fn split_args(value: &str) -> Option<Vec<&str>> {
    if value.trim().is_empty() {
        return Some(Vec::new());
    }
    if value.contains(',') {
        let args = value.split(',').map(str::trim).collect::<Vec<_>>();
        if args
            .iter()
            .any(|arg| arg.is_empty() || arg.contains(char::is_whitespace))
        {
            return None;
        }
        Some(args)
    } else {
        Some(value.split_whitespace().collect())
    }
}

fn parse_number(value: &str) -> Option<f32> {
    let number = value.trim().parse::<f32>().ok()?;
    number.is_finite().then_some(number)
}

fn parse_scale(value: &str) -> Option<f32> {
    if let Some(percent) = value.trim().strip_suffix('%') {
        return Some(parse_number(percent)? / 100.0);
    }
    parse_number(value)
}

fn parse_length_percentage(
    value: &str,
    percentage_basis: f32,
    reference: TransformReferenceBox,
) -> Option<f32> {
    let normalized = value.trim().to_ascii_lowercase();
    let value = normalized.as_str();
    if let Some(percent) = value.strip_suffix('%') {
        return Some(parse_number(percent)? * percentage_basis / 100.0);
    }
    if let Some(px) = value.strip_suffix("px") {
        return parse_number(px);
    }
    if let Some(rem) = value.strip_suffix("rem") {
        return Some(parse_number(rem)? * reference.root_font_size);
    }
    if let Some(em) = value.strip_suffix("em") {
        return Some(parse_number(em)? * reference.font_size);
    }
    let zero = parse_number(value)?;
    (zero == 0.0).then_some(0.0)
}

fn parse_zero_length(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    let value = normalized.as_str();
    value == "0" || value == "0px"
}

fn parse_angle(value: &str) -> Option<f32> {
    let normalized = value.trim().to_ascii_lowercase();
    let value = normalized.as_str();
    if let Some(degrees) = value.strip_suffix("deg") {
        return Some(parse_number(degrees)? * PI / 180.0);
    }
    if let Some(radians) = value.strip_suffix("rad") {
        return parse_number(radians);
    }
    if let Some(gradians) = value.strip_suffix("grad") {
        return Some(parse_number(gradians)? * PI / 200.0);
    }
    if let Some(turns) = value.strip_suffix("turn") {
        return Some(parse_number(turns)? * 2.0 * PI);
    }
    let zero = parse_number(value)?;
    (zero == 0.0).then_some(0.0)
}

fn parse_transform_origin(value: &str, reference: TransformReferenceBox) -> Option<(f32, f32)> {
    let value = value.trim();
    let value = if value.is_empty() || is_css_wide_keyword(value) {
        "50% 50%"
    } else {
        value
    };
    let values = value.split_whitespace().collect::<Vec<_>>();
    if values.is_empty() || values.len() > 3 {
        return None;
    }
    if values.len() == 3 {
        if values[2].ends_with('%') {
            return None;
        }
        parse_length_percentage(values[2], 0.0, reference)?;
    }

    let (x, y) = match values.as_slice() {
        [single] if is_vertical_keyword(single) => ("center", *single),
        [single] => (*single, "center"),
        [first, second, ..] if is_vertical_keyword(first) && is_horizontal_keyword(second) => {
            (*second, *first)
        }
        [first, second, ..] => (*first, *second),
        _ => return None,
    };
    Some((
        reference.x + parse_origin_axis(x, reference.width, true, reference)?,
        reference.y + parse_origin_axis(y, reference.height, false, reference)?,
    ))
}

fn parse_origin_axis(
    value: &str,
    basis: f32,
    horizontal: bool,
    reference: TransformReferenceBox,
) -> Option<f32> {
    match value.to_ascii_lowercase().as_str() {
        "left" if horizontal => Some(0.0),
        "right" if horizontal => Some(basis),
        "top" if !horizontal => Some(0.0),
        "bottom" if !horizontal => Some(basis),
        "center" => Some(basis / 2.0),
        _ => parse_length_percentage(value, basis, reference),
    }
}

fn is_horizontal_keyword(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "left" | "center" | "right"
    )
}

fn is_vertical_keyword(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "top" | "center" | "bottom"
    )
}

fn is_css_wide_keyword(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "initial" | "inherit" | "unset" | "revert" | "revert-layer"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference() -> TransformReferenceBox {
        TransformReferenceBox {
            x: 10.0,
            y: 20.0,
            width: 200.0,
            height: 100.0,
            font_size: 16.0,
            root_font_size: 16.0,
        }
    }

    #[test]
    fn composes_transform_functions_in_css_order() {
        let matrix = parse_transform_list("translate(10px, 20px) scale(2)", reference()).unwrap();
        assert_eq!(matrix.transform_point(1.0, 1.0), (12.0, 22.0));
    }

    #[test]
    fn parses_percentage_translate_and_angle_units() {
        let translated = parse_transform_list("translate(50%, 25%)", reference()).unwrap();
        assert_eq!(translated.transform_point(0.0, 0.0), (100.0, 25.0));
        let rotated = parse_transform_list("rotate(.25turn)", reference()).unwrap();
        let (x, y) = rotated.transform_point(2.0, 0.0);
        assert!(x.abs() < 0.001);
        assert!((y - 2.0).abs() < 0.001);

        assert!(parse_transform_list("translateX(2EM) rotate(90DEG)", reference()).is_some());
    }

    #[test]
    fn resolves_transform_origin_against_border_box() {
        let matrix = parse_transform_with_origin("scale(2)", "right bottom", reference()).unwrap();
        assert_eq!(matrix.transform_point(210.0, 120.0), (210.0, 120.0));
        assert_eq!(matrix.transform_point(10.0, 20.0), (-190.0, -80.0));
    }

    #[test]
    fn rejects_unknown_or_partially_valid_lists() {
        assert!(parse_transform_list("translateX(10px) bogus(1)", reference()).is_none());
        assert!(parse_transform_list("scale(2, nope)", reference()).is_none());
    }

    #[test]
    fn inverse_round_trips_points() {
        let matrix =
            parse_transform_list("translate(20px) rotate(30deg) scale(2, 3)", reference()).unwrap();
        let transformed = matrix.transform_point(12.0, -5.0);
        let restored = matrix
            .inverse()
            .unwrap()
            .transform_point(transformed.0, transformed.1);
        assert!((restored.0 - 12.0).abs() < 0.001);
        assert!((restored.1 + 5.0).abs() < 0.001);
    }
}
