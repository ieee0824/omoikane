//! Dimension-checked numeric math, with unit resolution passed explicitly.
use super::super::{CssToken, Value, parse_style_attribute, tokenize};

pub(super) const UNITS: &[(&str, f32)] = &[
    ("dppx", 1.0),
    ("dpi", 1.0 / 96.0),
    ("dpcm", 2.54 / 96.0),
    ("x", 1.0),
];

pub(super) fn parse(input: &str) -> Option<f32> {
    let result = parse_typed(input, Dimension::Resolution, resolve_resolution)? as f32;
    result.is_finite().then_some(result)
}

pub(super) fn parse_number(input: &str) -> Option<f64> {
    parse_typed(input, Dimension::Number, |_, _| None)
}

pub(super) fn parse_length(input: &str) -> Option<f32> {
    let result = parse_typed(input, Dimension::Length, resolve_length)? as f32;
    result.is_finite().then_some(result)
}

type ResolveUnit = fn(f32, &str) -> Option<Scalar>;

fn resolve_resolution(value: f32, unit: &str) -> Option<Scalar> {
    let (_, factor) = UNITS
        .iter()
        .find(|(name, _)| unit.eq_ignore_ascii_case(name))?;
    Scalar::finite(f64::from(value) * f64::from(*factor), Dimension::Resolution)
}

fn resolve_length(value: f32, unit: &str) -> Option<Scalar> {
    // Reuse the existing initial-font/absolute-length policy; the arithmetic
    // evaluator receives this dependency explicitly rather than reading it.
    let pixels = super::parse_length_to_px(&format!("{value}{unit}"))?;
    Scalar::finite(f64::from(pixels), Dimension::Length)
}

fn parse_typed(input: &str, dimension: Dimension, resolve: ResolveUnit) -> Option<f64> {
    let tokens = tokenize(input).ok()?;
    if tokens.iter().any(|token| {
        matches!(
            token,
            CssToken::Semicolon | CssToken::Colon | CssToken::CurlyOpen | CssToken::CurlyClose
        )
    }) {
        return None;
    }
    let declarations = parse_style_attribute(&format!("resolution:{input}"));
    if declarations.len() != 1 {
        return None;
    }
    let value = &declarations[0].value;
    if !matches!(value, Value::Function { .. }) {
        return None;
    }
    let result = evaluate(value, resolve)?;
    (result.dimension == dimension).then_some(result.value)
}

#[derive(Clone, Copy, PartialEq)]
enum Dimension {
    Number,
    Resolution,
    Length,
}

#[derive(Clone, Copy)]
struct Scalar {
    pub value: f64,
    pub dimension: Dimension,
}

impl Scalar {
    fn finite(value: f64, dimension: Dimension) -> Option<Self> {
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

fn evaluate(value: &Value, resolve: ResolveUnit) -> Option<Scalar> {
    use Dimension::*;
    match value {
        Value::Number(value) => Scalar::finite(f64::from(*value), Number),
        Value::Length(value, unit) => resolve(*value, unit),
        Value::List(values) => expression(values, resolve),
        Value::Function { name, arguments } => function(name, arguments, resolve),
        _ => None,
    }
}

fn expression(values: &[Value], resolve: ResolveUnit) -> Option<Scalar> {
    let mut cursor = 0;
    let result = sum(values, &mut cursor, resolve)?;
    (cursor == values.len()).then_some(result)
}

fn sum(values: &[Value], cursor: &mut usize, resolve: ResolveUnit) -> Option<Scalar> {
    let mut result = product(values, cursor, resolve)?;
    while let Some(Value::Keyword(operator)) = values.get(*cursor) {
        if !matches!(operator.as_str(), "+" | "-") {
            break;
        }
        *cursor += 1;
        result = result.combine(product(values, cursor, resolve)?, operator)?;
    }
    Some(result)
}

fn product(values: &[Value], cursor: &mut usize, resolve: ResolveUnit) -> Option<Scalar> {
    let mut result = evaluate(values.get(*cursor)?, resolve)?;
    *cursor += 1;
    while let Some(Value::Keyword(operator)) = values.get(*cursor) {
        if !matches!(operator.as_str(), "*" | "/") {
            break;
        }
        *cursor += 1;
        let right = evaluate(values.get(*cursor)?, resolve)?;
        *cursor += 1;
        result = result.combine(right, operator)?;
    }
    Some(result)
}

fn function(name: &str, arguments: &[Value], resolve: ResolveUnit) -> Option<Scalar> {
    let name = name.to_ascii_lowercase();
    if name == "calc" {
        return evaluate(arguments.first().filter(|_| arguments.len() == 1)?, resolve);
    }
    let values = arguments
        .iter()
        .map(|value| evaluate(value, resolve))
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
                    .reduce(if name == "min" { f64::min } else { f64::max })?,
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
