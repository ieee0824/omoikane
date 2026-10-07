//! Typed scalar math for individual rotations and scales.
use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Dimension {
    Number,
    Percentage,
    Angle,
    Length,
}

#[derive(Clone, Copy)]
pub(super) struct Scalar {
    pub value: f32,
    pub dimension: Dimension,
}

impl Scalar {
    fn finite(value: f32, dimension: Dimension) -> Option<Self> {
        value.is_finite().then_some(Self { value, dimension })
    }
    fn combine(self, other: Self, operator: &str) -> Option<Self> {
        match operator {
            "+" | "-" if self.dimension == other.dimension => Self::finite(
                if operator == "+" {
                    self.value + other.value
                } else {
                    self.value - other.value
                },
                self.dimension,
            ),
            "*" if self.dimension == Dimension::Number || other.dimension == Dimension::Number => {
                Self::finite(
                    self.value * other.value,
                    if self.dimension == Dimension::Number {
                        other.dimension
                    } else {
                        self.dimension
                    },
                )
            }
            "/" if other.dimension == Dimension::Number => {
                Self::finite(self.value / other.value, self.dimension)
            }
            "/" if self.dimension == other.dimension => {
                Self::finite(self.value / other.value, Dimension::Number)
            }
            _ => None,
        }
    }
}

pub(super) fn evaluate(value: &Value, context: ResolutionContext) -> Option<Scalar> {
    use Dimension::*;
    match value {
        Value::Number(value) => Scalar::finite(*value, Number),
        Value::Percentage(value) => Scalar::finite(*value, Percentage),
        Value::Length(value, unit) => {
            let text = format!("{value}{unit}");
            if let Some(angle) = super::super::super::angle::CssAngle::parse(
                &text,
                super::super::super::angle::UnitlessZero::None,
                false,
            ) {
                Scalar::finite(angle.degrees(), Angle)
            } else {
                Scalar::finite(resolve_length_to_px(*value, unit, context)?, Length)
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
    let mut result = evaluate(values.get(*cursor)?, context)?;
    *cursor += 1;
    while let Some(Value::Keyword(operator)) = values.get(*cursor) {
        if !matches!(operator.as_str(), "*" | "/") {
            break;
        }
        *cursor += 1;
        let right = evaluate(values.get(*cursor)?, context)?;
        *cursor += 1;
        result = result.combine(right, operator)?;
    }
    Some(result)
}

fn function(name: &str, arguments: &[Value], context: ResolutionContext) -> Option<Scalar> {
    let name = name.to_ascii_lowercase();
    if name == "calc" {
        return evaluate(arguments.first().filter(|_| arguments.len() == 1)?, context);
    }
    let values = arguments
        .iter()
        .map(|value| evaluate(value, context))
        .collect::<Option<Vec<_>>>()?;
    let first = *values.first()?;
    match name.as_str() {
        "sign" if values.len() == 1 => Scalar::finite(
            if first.value == 0.0 {
                first.value
            } else {
                first.value.signum()
            },
            Dimension::Number,
        ),
        "abs" if values.len() == 1 => Scalar::finite(first.value.abs(), first.dimension),
        "min" | "max"
            if values
                .iter()
                .all(|value| value.dimension == first.dimension) =>
        {
            Scalar::finite(
                values
                    .iter()
                    .map(|value| value.value)
                    .reduce(if name == "min" { f32::min } else { f32::max })?,
                first.dimension,
            )
        }
        "clamp"
            if values.len() == 3
                && values
                    .iter()
                    .all(|value| value.dimension == first.dimension) =>
        {
            Scalar::finite(
                values[1].value.min(values[2].value).max(first.value),
                first.dimension,
            )
        }
        _ => None,
    }
}
