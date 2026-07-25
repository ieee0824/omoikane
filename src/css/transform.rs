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
    let functions = parse_transform_functions(value)?;
    if functions.is_empty() {
        return Some(AffineTransform::identity());
    }

    let mut result = AffineTransform::identity();
    for (name, args) in functions {
        result = result.multiply(parse_transform_function(name, args, reference)?);
    }
    Some(result)
}

fn parse_transform_functions(value: &str) -> Option<Vec<(&str, &str)>> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") || is_css_wide_keyword(value) {
        return Some(Vec::new());
    }
    if value.is_empty() {
        return None;
    }

    let mut functions = Vec::new();
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
        functions.push((name, args));
    }
    Some(functions)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LengthUnit {
    Zero,
    Px,
    Percent,
    Em,
    Rem,
}

#[derive(Debug, Clone, Copy)]
struct InterpolableLength {
    value: f32,
    unit: LengthUnit,
}

#[derive(Debug, Clone)]
enum TransformOperation {
    Matrix([f32; 6]),
    Translate(InterpolableLength, InterpolableLength),
    Scale(f32, f32),
    Rotate(f32),
    Skew(f32, f32),
}

/// Interpolates compatible CSS 2D transform lists without resolving relative
/// lengths. Percentages and font-relative lengths therefore remain available
/// for resolution against the element's real reference box during layout.
pub(crate) fn interpolate_transform_lists(
    start: &str,
    end: &str,
    progress: f32,
) -> Option<String> {
    let start_text = start;
    let end_text = end;
    let mut start = parse_interpolable_transform_list(start_text)?;
    let mut end = parse_interpolable_transform_list(end_text)?;
    if start.is_empty() && end.is_empty() {
        return Some("none".to_string());
    }
    if start.is_empty() {
        start = end.iter().map(TransformOperation::identity_like).collect();
    } else if end.is_empty() {
        end = start.iter().map(TransformOperation::identity_like).collect();
    }
    if start.len() < end.len() {
        start.extend(
            end[start.len()..]
                .iter()
                .map(TransformOperation::identity_like),
        );
    } else if end.len() < start.len() {
        end.extend(
            start[end.len()..]
                .iter()
                .map(TransformOperation::identity_like),
        );
    }

    let compatible = start
        .iter()
        .zip(&end)
        .map(|(start, end)| start.interpolate(end, progress))
        .collect::<Option<Vec<_>>>()
        .map(|items| items.join(" "));
    compatible.or_else(|| interpolate_decomposed_matrices(start_text, end_text, progress))
}

fn parse_interpolable_transform_list(value: &str) -> Option<Vec<TransformOperation>> {
    parse_transform_functions(value)?
        .into_iter()
        .map(|(name, args)| {
            let args = split_args(args)?;
            match name.to_ascii_lowercase().as_str() {
                "matrix" if args.len() == 6 => Some(TransformOperation::Matrix([
                    parse_number(args[0])?,
                    parse_number(args[1])?,
                    parse_number(args[2])?,
                    parse_number(args[3])?,
                    parse_number(args[4])?,
                    parse_number(args[5])?,
                ])),
                "translate" if (1..=2).contains(&args.len()) => {
                    Some(TransformOperation::Translate(
                        parse_interpolable_length(args[0])?,
                        if args.len() == 2 {
                            parse_interpolable_length(args[1])?
                        } else {
                            InterpolableLength::zero()
                        },
                    ))
                }
                "translatex" if args.len() == 1 => Some(TransformOperation::Translate(
                    parse_interpolable_length(args[0])?,
                    InterpolableLength::zero(),
                )),
                "translatey" if args.len() == 1 => Some(TransformOperation::Translate(
                    InterpolableLength::zero(),
                    parse_interpolable_length(args[0])?,
                )),
                "translate3d" if args.len() == 3 && parse_zero_length(args[2]) => {
                    Some(TransformOperation::Translate(
                        parse_interpolable_length(args[0])?,
                        parse_interpolable_length(args[1])?,
                    ))
                }
                "scale" if (1..=2).contains(&args.len()) => {
                    let x = parse_scale(args[0])?;
                    Some(TransformOperation::Scale(
                        x,
                        if args.len() == 2 {
                            parse_scale(args[1])?
                        } else {
                            x
                        },
                    ))
                }
                "scalex" if args.len() == 1 => {
                    Some(TransformOperation::Scale(parse_scale(args[0])?, 1.0))
                }
                "scaley" if args.len() == 1 => {
                    Some(TransformOperation::Scale(1.0, parse_scale(args[0])?))
                }
                "rotate" if args.len() == 1 => {
                    Some(TransformOperation::Rotate(parse_angle(args[0])?))
                }
                "skew" if (1..=2).contains(&args.len()) => Some(TransformOperation::Skew(
                    parse_angle(args[0])?,
                    if args.len() == 2 {
                        parse_angle(args[1])?
                    } else {
                        0.0
                    },
                )),
                "skewx" if args.len() == 1 => {
                    Some(TransformOperation::Skew(parse_angle(args[0])?, 0.0))
                }
                "skewy" if args.len() == 1 => {
                    Some(TransformOperation::Skew(0.0, parse_angle(args[0])?))
                }
                _ => None,
            }
        })
        .collect()
}

impl InterpolableLength {
    fn zero() -> Self {
        Self {
            value: 0.0,
            unit: LengthUnit::Zero,
        }
    }

    fn interpolate(self, other: Self, progress: f32) -> Option<Self> {
        let unit = match (self.unit, other.unit) {
            (LengthUnit::Zero, unit) | (unit, LengthUnit::Zero) => unit,
            (left, right) if left == right => left,
            _ => return None,
        };
        Some(Self {
            value: interpolate_number(self.value, other.value, progress),
            unit,
        })
    }

    fn render(self) -> String {
        let suffix = match self.unit {
            LengthUnit::Zero => "",
            LengthUnit::Px => "px",
            LengthUnit::Percent => "%",
            LengthUnit::Em => "em",
            LengthUnit::Rem => "rem",
        };
        format!("{}{suffix}", format_component(self.value))
    }
}

impl TransformOperation {
    fn identity_like(&self) -> Self {
        match self {
            Self::Matrix(_) => Self::Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
            Self::Translate(_, _) => {
                Self::Translate(InterpolableLength::zero(), InterpolableLength::zero())
            }
            Self::Scale(_, _) => Self::Scale(1.0, 1.0),
            Self::Rotate(_) => Self::Rotate(0.0),
            Self::Skew(_, _) => Self::Skew(0.0, 0.0),
        }
    }

    fn interpolate(&self, other: &Self, progress: f32) -> Option<String> {
        match (self, other) {
            (Self::Matrix(start), Self::Matrix(end)) => Some(format!(
                "matrix({})",
                start
                    .iter()
                    .zip(end)
                    .map(|(start, end)| {
                        format_component(interpolate_number(*start, *end, progress))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            (Self::Translate(start_x, start_y), Self::Translate(end_x, end_y)) => Some(format!(
                "translate({}, {})",
                start_x.interpolate(*end_x, progress)?.render(),
                start_y.interpolate(*end_y, progress)?.render()
            )),
            (Self::Scale(start_x, start_y), Self::Scale(end_x, end_y)) => Some(format!(
                "scale({}, {})",
                format_component(interpolate_number(*start_x, *end_x, progress)),
                format_component(interpolate_number(*start_y, *end_y, progress))
            )),
            (Self::Rotate(start), Self::Rotate(end)) => Some(format!(
                "rotate({}rad)",
                format_component(interpolate_number(*start, *end, progress))
            )),
            (Self::Skew(start_x, start_y), Self::Skew(end_x, end_y)) => Some(format!(
                "skew({}rad, {}rad)",
                format_component(interpolate_number(*start_x, *end_x, progress)),
                format_component(interpolate_number(*start_y, *end_y, progress))
            )),
            _ => None,
        }
    }

    fn matrix(&self) -> Option<AffineTransform> {
        match self {
            Self::Matrix(values) => Some(AffineTransform {
                a: values[0],
                b: values[1],
                c: values[2],
                d: values[3],
                e: values[4],
                f: values[5],
            }),
            Self::Translate(x, y) => Some(AffineTransform::translate(
                x.absolute_px()?,
                y.absolute_px()?,
            )),
            Self::Scale(x, y) => Some(AffineTransform::scale(*x, *y)),
            Self::Rotate(angle) => Some(AffineTransform::rotate(*angle)),
            Self::Skew(x, y) => Some(AffineTransform::skew(*x, *y)),
        }
    }
}

impl InterpolableLength {
    fn absolute_px(self) -> Option<f32> {
        matches!(self.unit, LengthUnit::Zero | LengthUnit::Px).then_some(self.value)
    }
}

#[derive(Debug, Clone, Copy)]
struct DecomposedTransform {
    translate_x: f32,
    translate_y: f32,
    rotation: f32,
    skew_x: f32,
    scale_x: f32,
    scale_y: f32,
}

fn interpolate_decomposed_matrices(start: &str, end: &str, progress: f32) -> Option<String> {
    let start = transform_operations_matrix(&parse_interpolable_transform_list(start)?)?;
    let end = transform_operations_matrix(&parse_interpolable_transform_list(end)?)?;
    let start = decompose_transform(start)?;
    let mut end = decompose_transform(end)?;
    if (start.rotation - end.rotation).abs() > PI {
        if start.rotation < end.rotation {
            end.rotation -= 2.0 * PI;
        } else {
            end.rotation += 2.0 * PI;
        }
    }
    Some(format!(
        "translate({}px, {}px) rotate({}rad) skewX({}rad) scale({}, {})",
        format_component(interpolate_number(
            start.translate_x,
            end.translate_x,
            progress
        )),
        format_component(interpolate_number(
            start.translate_y,
            end.translate_y,
            progress
        )),
        format_component(interpolate_number(start.rotation, end.rotation, progress)),
        format_component(interpolate_number(start.skew_x, end.skew_x, progress)),
        format_component(interpolate_number(start.scale_x, end.scale_x, progress)),
        format_component(interpolate_number(start.scale_y, end.scale_y, progress)),
    ))
}

fn transform_operations_matrix(operations: &[TransformOperation]) -> Option<AffineTransform> {
    operations
        .iter()
        .try_fold(AffineTransform::identity(), |matrix, operation| {
            Some(matrix.multiply(operation.matrix()?))
        })
}

fn decompose_transform(matrix: AffineTransform) -> Option<DecomposedTransform> {
    let mut row0 = [matrix.a, matrix.b];
    let mut row1 = [matrix.c, matrix.d];
    let mut scale_x = row0[0].hypot(row0[1]);
    if !scale_x.is_finite() || scale_x <= f32::EPSILON {
        return None;
    }
    row0[0] /= scale_x;
    row0[1] /= scale_x;

    let mut skew = row0[0] * row1[0] + row0[1] * row1[1];
    row1[0] -= row0[0] * skew;
    row1[1] -= row0[1] * skew;
    let mut scale_y = row1[0].hypot(row1[1]);
    if !scale_y.is_finite() || scale_y <= f32::EPSILON {
        return None;
    }
    row1[0] /= scale_y;
    row1[1] /= scale_y;
    skew /= scale_y;

    if row0[0] * row1[1] - row0[1] * row1[0] < 0.0 {
        if matrix.a < matrix.d {
            scale_x = -scale_x;
            row0[0] = -row0[0];
            row0[1] = -row0[1];
            skew = -skew;
        } else {
            scale_y = -scale_y;
        }
    }
    Some(DecomposedTransform {
        translate_x: matrix.e,
        translate_y: matrix.f,
        rotation: row0[1].atan2(row0[0]),
        skew_x: skew.atan(),
        scale_x,
        scale_y,
    })
}

fn parse_interpolable_length(value: &str) -> Option<InterpolableLength> {
    let normalized = value.trim().to_ascii_lowercase();
    let (number, unit) = if let Some(value) = normalized.strip_suffix("rem") {
        (parse_number(value)?, LengthUnit::Rem)
    } else if let Some(value) = normalized.strip_suffix("px") {
        (parse_number(value)?, LengthUnit::Px)
    } else if let Some(value) = normalized.strip_suffix("em") {
        (parse_number(value)?, LengthUnit::Em)
    } else if let Some(value) = normalized.strip_suffix('%') {
        (parse_number(value)?, LengthUnit::Percent)
    } else {
        let number = parse_number(&normalized)?;
        if number != 0.0 {
            return None;
        }
        (number, LengthUnit::Zero)
    };
    Some(InterpolableLength {
        value: number,
        unit,
    })
}

fn interpolate_number(start: f32, end: f32, progress: f32) -> f32 {
    start + (end - start) * progress
}

fn format_component(value: f32) -> String {
    if value.abs() < 1e-6 {
        return "0".to_string();
    }
    let rendered = format!("{value:.6}");
    rendered
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
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
    fn interpolates_compatible_transform_lists_without_losing_relative_units() {
        assert_eq!(
            interpolate_transform_lists(
                "translate(10%, 2rem) scale(1)",
                "translate(50%, 4rem) scale(3)",
                0.5,
            ),
            Some("translate(30%, 3rem) scale(2, 2)".to_string())
        );
    }

    #[test]
    fn interpolates_none_using_each_transform_functions_identity() {
        assert_eq!(
            interpolate_transform_lists("none", "translateX(40px) rotate(180deg)", 0.5),
            Some(format!(
                "translate(20px, 0) rotate({}rad)",
                format_component(PI / 2.0)
            ))
        );
    }

    #[test]
    fn interpolated_transform_matches_firefox_midpoint_matrix() {
        let value = interpolate_transform_lists(
            "none",
            "translate(50%, 2rem) rotate(180deg)",
            0.5,
        )
        .unwrap();
        let matrix = parse_transform_list(
            &value,
            TransformReferenceBox {
                x: 0.0,
                y: 0.0,
                width: 55.0,
                height: 10.0,
                font_size: 16.0,
                root_font_size: 16.0,
            },
        )
        .unwrap();
        assert!(matrix.a.abs() < 0.000_01);
        assert!((matrix.b - 1.0).abs() < 0.000_01);
        assert!((matrix.c + 1.0).abs() < 0.000_01);
        assert!(matrix.d.abs() < 0.000_01);
        assert!((matrix.e - 13.75).abs() < 0.000_01);
        assert!((matrix.f - 16.0).abs() < 0.000_01);
    }

    #[test]
    fn rejects_incompatible_transform_operations_and_length_units() {
        let decomposed = interpolate_transform_lists("scale(2)", "rotate(1rad)", 0.5).unwrap();
        assert!(parse_transform_list(&decomposed, reference()).is_some());
        assert!(
            interpolate_transform_lists("translateX(1em)", "translateX(1rem)", 0.5).is_none()
        );
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
