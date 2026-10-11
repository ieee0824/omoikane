//! Typed scalar math for individual rotations and scales.
use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Dimension {
    Number,
    Percentage,
    Angle,
    Length,
    Compound(typed_math::NumericType),
}

#[derive(Clone, Copy)]
pub(super) struct Scalar {
    pub value: f32,
    pub dimension: Dimension,
}

impl Dimension {
    fn numeric_type(self) -> typed_math::NumericType {
        use typed_math::NumericType as T;
        match self {
            Self::Number => T::NUMBER,
            Self::Percentage => T::PERCENTAGE,
            Self::Angle => T::ANGLE,
            Self::Length => T::LENGTH,
            Self::Compound(kind) => kind,
        }
    }
    fn from_type(kind: typed_math::NumericType) -> Self {
        use typed_math::NumericType as T;
        match kind {
            T::NUMBER => Self::Number,
            T::PERCENTAGE => Self::Percentage,
            T::ANGLE => Self::Angle,
            T::LENGTH => Self::Length,
            _ => Self::Compound(kind),
        }
    }
}

impl Scalar {
    // Keep IEEE intermediate values; clamp only after the entire expression.
    fn result(value: f32, dimension: Dimension) -> Option<Self> {
        Some(Self { value, dimension })
    }
    fn combine(self, other: Self, operator: &str) -> Option<Self> {
        match operator {
            "+" | "-" if self.dimension == other.dimension => Self::result(
                if operator == "+" {
                    self.value + other.value
                } else {
                    self.value - other.value
                },
                self.dimension,
            ),
            "*" | "/" => {
                let divide = operator == "/";
                let kind = self
                    .dimension
                    .numeric_type()
                    .product(other.dimension.numeric_type(), divide)?;
                Self::result(
                    if divide {
                        self.value / other.value
                    } else {
                        self.value * other.value
                    },
                    Dimension::from_type(kind),
                )
            }
            _ => None,
        }
    }
}

/// Evaluates an expression and clamps its completed value to the supported range.
pub(super) fn evaluate(value: &Value, context: ResolutionContext) -> Option<Scalar> {
    let mut result = evaluate_unclamped(value, context)?;
    result.value = if result.value.is_nan() {
        0.0
    } else {
        result.value.clamp(-f32::MAX, f32::MAX)
    };
    Some(result)
}

pub(super) fn evaluate_unclamped(value: &Value, context: ResolutionContext) -> Option<Scalar> {
    use Dimension::*;
    match value {
        Value::Number(value) => Scalar::result(*value, Number),
        Value::Keyword(name) if name.eq_ignore_ascii_case("pi") => {
            Scalar::result(std::f32::consts::PI, Number)
        }
        Value::Keyword(name) if name.eq_ignore_ascii_case("e") => {
            Scalar::result(std::f32::consts::E, Number)
        }
        Value::Percentage(value) => Scalar::result(*value, Percentage),
        Value::Keyword(name) if name.eq_ignore_ascii_case("infinity") => {
            Scalar::result(f32::INFINITY, Number)
        }
        Value::Keyword(name) if name.eq_ignore_ascii_case("-infinity") => {
            Scalar::result(f32::NEG_INFINITY, Number)
        }
        Value::Keyword(name) if name.eq_ignore_ascii_case("nan") => {
            Scalar::result(f32::NAN, Number)
        }
        Value::Length(value, unit) => {
            use typed_math::NumericType as T;
            let fixed = match unit.to_ascii_lowercase().as_str() {
                "s" => Some((*value * 1000.0, T::TIME)),
                "ms" => Some((*value, T::TIME)),
                "dpi" => Some((*value / 96.0, T::RESOLUTION)),
                "dpcm" => Some((*value * 2.54 / 96.0, T::RESOLUTION)),
                "dppx" | "x" => Some((*value, T::RESOLUTION)),
                _ => None,
            };
            if let Some((value, kind)) = fixed {
                return Scalar::result(value, Compound(kind));
            }
            let text = format!("{value}{unit}");
            if let Some(angle) = super::super::super::angle::CssAngle::parse(
                &text,
                super::super::super::angle::UnitlessZero::None,
                false,
            ) {
                Scalar::result(angle.degrees(), Angle)
            } else {
                Scalar::result(resolve_length_to_px(*value, unit, context)?, Length)
            }
        }
        Value::List(values) => expression(values, context),
        Value::Function { name, arguments } => function(name, arguments, context),
        _ => None,
    }
}

fn expression(values: &[Value], context: ResolutionContext) -> Option<Scalar> {
    let mut cursor = 0;
    let result = sum(values, &mut cursor, context)?;
    (cursor == values.len()).then_some(result)
}

fn sum(values: &[Value], cursor: &mut usize, context: ResolutionContext) -> Option<Scalar> {
    let mut result = product(values, cursor, context)?;
    while let Some(Value::Keyword(operator)) = values.get(*cursor) {
        if !matches!(operator.as_str(), "+" | "-") {
            break;
        }
        *cursor += 1;
        result = result.combine(product(values, cursor, context)?, operator)?;
    }
    Some(result)
}

fn product(values: &[Value], cursor: &mut usize, context: ResolutionContext) -> Option<Scalar> {
    let mut result = evaluate_unclamped(values.get(*cursor)?, context)?;
    *cursor += 1;
    while let Some(Value::Keyword(operator)) = values.get(*cursor) {
        if !matches!(operator.as_str(), "*" | "/") {
            break;
        }
        *cursor += 1;
        let right = evaluate_unclamped(values.get(*cursor)?, context)?;
        *cursor += 1;
        result = result.combine(right, operator)?;
    }
    Some(result)
}

fn function(name: &str, arguments: &[Value], context: ResolutionContext) -> Option<Scalar> {
    let name = name.to_ascii_lowercase();
    if name == "calc" {
        return evaluate_unclamped(arguments.first().filter(|_| arguments.len() == 1)?, context);
    }
    let values = arguments
        .iter()
        .map(|value| evaluate_unclamped(value, context))
        .collect::<Option<Vec<_>>>()?;
    let first = *values.first()?;
    match name.as_str() {
        "sin" | "cos" | "tan"
            if values.len() == 1
                && matches!(first.dimension, Dimension::Number | Dimension::Angle) =>
        {
            let radians = if first.dimension == Dimension::Angle {
                first.value.to_radians()
            } else {
                first.value
            };
            let result = match name.as_str() {
                "sin" => radians.sin(),
                "cos" => radians.cos(),
                _ => radians.tan(),
            };
            Scalar::result(result, Dimension::Number)
        }
        "asin" | "acos" | "atan" if values.len() == 1 && first.dimension == Dimension::Number => {
            let radians = match name.as_str() {
                "asin" => first.value.asin(),
                "acos" => first.value.acos(),
                _ => first.value.atan(),
            };
            Scalar::result(radians.to_degrees(), Dimension::Angle)
        }
        "atan2" if values.len() == 2 && first.dimension == values[1].dimension => Scalar::result(
            first.value.atan2(values[1].value).to_degrees(),
            Dimension::Angle,
        ),
        "sign" if values.len() == 1 => Scalar::result(
            if first.value == 0.0 {
                first.value
            } else {
                first.value.signum()
            },
            Dimension::Number,
        ),
        "abs" if values.len() == 1 => Scalar::result(first.value.abs(), first.dimension),
        _ => consistent_function(&name, &values),
    }
}

fn consistent_function(name: &str, values: &[Scalar]) -> Option<Scalar> {
    let first = *values.first()?;
    if !values
        .iter()
        .all(|value| value.dimension == first.dimension)
    {
        return None;
    }
    let value = match name {
        "min" | "max" => values
            .iter()
            .map(|value| value.value)
            .reduce(|left, right| {
                if left.is_nan() || right.is_nan() {
                    f32::NAN
                } else if name == "min" {
                    left.min(right)
                } else {
                    left.max(right)
                }
            })?,
        "hypot" => values
            .iter()
            .fold(0.0_f32, |result, value| result.hypot(value.value)),
        "clamp" if values.len() == 3 => {
            if values.iter().any(|value| value.value.is_nan()) {
                f32::NAN
            } else {
                values[1].value.min(values[2].value).max(first.value)
            }
        }
        "mod" | "rem" if values.len() == 2 => {
            let divisor = values[1].value;
            let remainder = first.value % divisor;
            if name == "rem" {
                remainder
            } else if divisor.is_infinite()
                && first.value.is_sign_negative() != divisor.is_sign_negative()
            {
                f32::NAN
            } else if remainder == 0.0 {
                0.0_f32.copysign(divisor)
            } else if remainder.is_sign_negative() != divisor.is_sign_negative() {
                remainder + divisor
            } else {
                remainder
            }
        }
        _ => return None,
    };
    Scalar::result(value, first.dimension)
}
