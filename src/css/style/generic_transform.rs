//! Structural transform grammar and computation with explicit style inputs.
use super::*;

#[derive(Clone, Copy)]
enum ArgumentKind {
    Number,
    Scale,
    Length,
    LengthPercentage,
    Angle,
}

impl ArgumentKind {
    fn accepts(self, value: &Value) -> bool {
        use typed_math::NumericType as T;
        let kind = typed_math::infer(
            value,
            matches!(self, Self::LengthPercentage).then_some(T::LENGTH),
        );
        let literal_zero = matches!(value, Value::Number(number) if *number == 0.0);
        match self {
            Self::Number => kind == Some(T::NUMBER),
            Self::Scale => kind == Some(T::NUMBER) || kind == Some(T::PERCENTAGE),
            Self::Length | Self::LengthPercentage => literal_zero || kind == Some(T::LENGTH),
            Self::Angle => literal_zero || kind == Some(T::ANGLE),
        }
    }
    fn compute(self, value: &Value, context: ResolutionContext) -> Option<Value> {
        if matches!(value, Value::Number(number) if *number == 0.0) {
            match self {
                Self::Length | Self::LengthPercentage => {
                    return Some(Value::Length(0.0, "px".into()));
                }
                Self::Angle => return Some(Value::Length(0.0, "deg".into())),
                _ => {}
            }
        }
        if matches!(self, Self::Length | Self::LengthPercentage)
            && let Some(math) = evaluate_length_percentage_math(value, context)
        {
            return Some(computed_value_to_value(&computed_length_percentage_math(
                math,
                "--transform",
                context,
            )));
        }
        let value = individual_transform::computed_numeric_expression(value, context)?;
        if let (Self::Scale, Value::Percentage(number)) = (self, &value) {
            return Some(Value::Number(*number / 100.0));
        }
        Some(value)
    }
}

fn argument_kinds(name: &str, count: usize) -> Option<Vec<ArgumentKind>> {
    use ArgumentKind::*;
    Some(match name.to_ascii_lowercase().as_str() {
        "matrix" if count == 6 => vec![Number; 6],
        "matrix3d" if count == 16 => vec![Number; 16],
        "translate" if (1..=2).contains(&count) => vec![LengthPercentage; count],
        "translatex" | "translatey" if count == 1 => vec![LengthPercentage],
        "translatez" | "perspective" if count == 1 => vec![Length],
        "translate3d" if count == 3 => vec![LengthPercentage, LengthPercentage, Length],
        "scale" if (1..=2).contains(&count) => vec![Scale; count],
        "scalex" | "scaley" | "scalez" if count == 1 => vec![Scale],
        "scale3d" if count == 3 => vec![Scale; 3],
        "rotate" | "rotatex" | "rotatey" | "rotatez" if count == 1 => vec![Angle],
        "rotate3d" if count == 4 => vec![Number, Number, Number, Angle],
        "skew" if (1..=2).contains(&count) => vec![Angle; count],
        "skewx" | "skewy" if count == 1 => vec![Angle],
        _ => return None,
    })
}

pub(super) fn valid(value: &Value) -> bool {
    match value {
        Value::Keyword(word) => word.eq_ignore_ascii_case("none") || is_css_wide_keyword(word),
        Value::List(values) => {
            !values.is_empty()
                && values
                    .iter()
                    .all(|value| matches!(value, Value::Function { .. }) && valid(value))
        }
        Value::Function { name, arguments } => {
            argument_kinds(name, arguments.len()).is_some_and(|kinds| {
                kinds
                    .into_iter()
                    .zip(arguments)
                    .all(|(kind, value)| kind.accepts(value))
            })
        }
        _ => false,
    }
}

pub(super) fn compute(value: &Value, context: ResolutionContext) -> Option<Value> {
    match value {
        Value::Keyword(_) => Some(value.clone()),
        Value::List(values) => values
            .iter()
            .map(|value| compute(value, context))
            .collect::<Option<Vec<_>>>()
            .map(Value::List),
        Value::Function { name, arguments } => {
            let kinds = argument_kinds(name, arguments.len())?;
            let arguments = kinds
                .into_iter()
                .zip(arguments)
                .map(|(kind, value)| kind.compute(value, context))
                .collect::<Option<Vec<_>>>()?;
            Some(Value::Function {
                name: name.clone(),
                arguments,
            })
        }
        _ => None,
    }
}
