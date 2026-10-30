//! Structural types of numeric CSS expressions, independent of DOM values.
use super::Value;

/// Exponents of length, angle, time, resolution and percentage dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NumericType(pub(super) [i8; 5]);

impl NumericType {
    pub(super) const NUMBER: Self = Self([0; 5]);
    pub(super) const LENGTH: Self = Self([1, 0, 0, 0, 0]);
    pub(super) const ANGLE: Self = Self([0, 1, 0, 0, 0]);
    pub(super) const TIME: Self = Self([0, 0, 1, 0, 0]);
    pub(super) const RESOLUTION: Self = Self([0, 0, 0, 1, 0]);
    pub(super) const PERCENTAGE: Self = Self([0, 0, 0, 0, 1]);

    pub(super) fn product(self, other: Self, divide: bool) -> Option<Self> {
        let mut dimensions = [0; 5];
        for (index, dimension) in dimensions.iter_mut().enumerate() {
            *dimension = if divide {
                self.0[index].checked_sub(other.0[index])?
            } else {
                self.0[index].checked_add(other.0[index])?
            };
        }
        Some(Self(dimensions))
    }
}

/// Infers a type without substituting numerical values for runtime functions.
/// A percentage hint is supplied only by a grammar with that percentage basis.
pub(super) fn infer(value: &Value, percentage_hint: Option<NumericType>) -> Option<NumericType> {
    match value {
        Value::Number(_) | Value::TreeCounting { .. } => Some(NumericType::NUMBER),
        Value::Percentage(_) => Some(percentage_hint.unwrap_or(NumericType::PERCENTAGE)),
        Value::Length(_, unit) => unit_type(unit),
        Value::Keyword(name)
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "pi" | "e" | "infinity" | "-infinity" | "nan"
            ) =>
        {
            Some(NumericType::NUMBER)
        }
        Value::List(values) => expression(values, percentage_hint),
        Value::Function { name, arguments } => function(name, arguments, percentage_hint),
        _ => None,
    }
}

fn unit_type(unit: &str) -> Option<NumericType> {
    let unit = unit.to_ascii_lowercase();
    if super::is_css_length_unit(&unit) {
        return Some(NumericType::LENGTH);
    }
    match unit.as_str() {
        "deg" | "grad" | "rad" | "turn" => Some(NumericType::ANGLE),
        "s" | "ms" => Some(NumericType::TIME),
        "dppx" | "dpi" | "dpcm" | "x" => Some(NumericType::RESOLUTION),
        _ => None,
    }
}

fn function(name: &str, arguments: &[Value], hint: Option<NumericType>) -> Option<NumericType> {
    match name.to_ascii_lowercase().as_str() {
        "sibling-index" | "sibling-count" if arguments.is_empty() => Some(NumericType::NUMBER),
        "calc" if !arguments.is_empty() => argument_expression(arguments, hint),
        "min" | "max" | "hypot" if !arguments.is_empty() => consistent(arguments, hint),
        "clamp" if arguments.len() == 3 => consistent(arguments, hint),
        "abs" if arguments.len() == 1 => infer(&arguments[0], hint),
        "sign" if !arguments.is_empty() => {
            argument_expression(arguments, hint).map(|_| NumericType::NUMBER)
        }
        "sin" | "cos" | "tan" if arguments.len() == 1 => {
            let input = infer(&arguments[0], hint)?;
            matches!(input, NumericType::NUMBER | NumericType::ANGLE).then_some(NumericType::NUMBER)
        }
        "asin" | "acos" | "atan" if arguments.len() == 1 => {
            (infer(&arguments[0], hint)? == NumericType::NUMBER).then_some(NumericType::ANGLE)
        }
        "atan2" if arguments.len() == 2 => consistent(arguments, hint).map(|_| NumericType::ANGLE),
        "mod" | "rem" if arguments.len() == 2 => consistent(arguments, hint),
        "round" => round_type(arguments, hint),
        "pow" if arguments.len() == 2 => {
            all_numbers(arguments, hint).then_some(NumericType::NUMBER)
        }
        "sqrt" | "exp" if arguments.len() == 1 => {
            all_numbers(arguments, hint).then_some(NumericType::NUMBER)
        }
        "log" if matches!(arguments.len(), 1 | 2) => {
            all_numbers(arguments, hint).then_some(NumericType::NUMBER)
        }
        _ => None,
    }
}

fn all_numbers(arguments: &[Value], hint: Option<NumericType>) -> bool {
    arguments
        .iter()
        .all(|argument| infer(argument, hint) == Some(NumericType::NUMBER))
}

fn round_type(arguments: &[Value], hint: Option<NumericType>) -> Option<NumericType> {
    let (value, step) = match arguments {
        [value, step] => (value, step),
        [Value::Keyword(strategy), value, step]
            if matches!(
                strategy.to_ascii_lowercase().as_str(),
                "nearest" | "up" | "down" | "to-zero"
            ) =>
        {
            (value, step)
        }
        _ => return None,
    };
    let kind = infer(value, hint)?;
    (infer(step, hint) == Some(kind)).then_some(kind)
}

fn argument_expression(arguments: &[Value], hint: Option<NumericType>) -> Option<NumericType> {
    match arguments {
        [argument] => infer(argument, hint),
        arguments => expression(arguments, hint),
    }
}

fn consistent(arguments: &[Value], hint: Option<NumericType>) -> Option<NumericType> {
    let first = infer(arguments.first()?, hint)?;
    arguments
        .iter()
        .skip(1)
        .all(|argument| infer(argument, hint) == Some(first))
        .then_some(first)
}

fn expression(values: &[Value], hint: Option<NumericType>) -> Option<NumericType> {
    let mut cursor = 0;
    let result = parse(values, &mut cursor, 0, hint)?;
    (cursor == values.len()).then_some(result)
}

fn parse(
    values: &[Value],
    cursor: &mut usize,
    minimum: u8,
    hint: Option<NumericType>,
) -> Option<NumericType> {
    let mut left = infer(values.get(*cursor)?, hint)?;
    *cursor += 1;
    loop {
        let Some(Value::Keyword(operator)) = values.get(*cursor) else {
            break;
        };
        let precedence = match operator.as_str() {
            "+" | "-" => 1,
            "*" | "/" => 2,
            _ => break,
        };
        if precedence < minimum {
            break;
        }
        *cursor += 1;
        let right = parse(values, cursor, precedence + 1, hint)?;
        left = match operator.as_str() {
            "+" | "-" if left == right => left,
            "*" => left.product(right, false)?,
            "/" => left.product(right, true)?,
            _ => return None,
        };
    }
    Some(left)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(text: &str) -> Value {
        super::super::super::parse_style_attribute(&format!("--probe:{text}"))[0]
            .value
            .clone()
    }

    #[test]
    fn zero_runtime_products_remain_numbers() {
        assert_eq!(
            infer(&value("calc(0 * sibling-index())"), None),
            Some(NumericType::NUMBER)
        );
        assert_eq!(
            infer(&value("calc(sibling-count() * 1px)"), None),
            Some(NumericType::LENGTH)
        );
        assert_eq!(infer(&value("calc(1px + sibling-index())"), None), None);
    }

    #[test]
    fn types_follow_units_precedence_and_nested_numeric_comparisons() {
        assert_eq!(
            infer(
                &value("calc(min(sibling-index(), 3) * 10% + 5px)"),
                Some(NumericType::LENGTH)
            ),
            Some(NumericType::LENGTH)
        );
        assert_eq!(
            infer(&value("calc(sibling-index() * 2rad * pi)"), None),
            Some(NumericType::ANGLE)
        );
        assert_eq!(
            infer(&value("calc(250ms * sibling-count())"), None),
            Some(NumericType::TIME)
        );
        assert_eq!(
            infer(&value("calc(3dppx * sibling-index())"), None),
            Some(NumericType::RESOLUTION)
        );
        assert_eq!(infer(&value("sibling-index(1)"), None), None);
        let container_expression = value("calc(10 + (sign(2cqw - 10px) * 5))");
        assert_eq!(
            infer(&container_expression, None),
            Some(NumericType::NUMBER)
        );
    }

    #[test]
    fn stepped_and_exponential_functions_enforce_numeric_types() {
        assert_eq!(
            infer(&value("round(up, 10.5px, 1px)"), Some(NumericType::LENGTH)),
            Some(NumericType::LENGTH)
        );
        assert_eq!(infer(&value("round(10px, 1deg)"), None), None);
        for expression in ["pow(2, 3)", "sqrt(9)", "log(8, 2)", "log(e)", "exp(1)"] {
            assert_eq!(infer(&value(expression), None), Some(NumericType::NUMBER));
        }
        assert_eq!(infer(&value("pow(2px, 3)"), None), None);
        assert_eq!(infer(&value("log(8, 2px)"), None), None);
    }
}
