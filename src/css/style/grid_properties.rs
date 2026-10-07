//! Declaration grammar for item ordering and implicit grid tracks.

use super::*;

pub(super) fn validate(name: &str, value: &Value) -> Option<DeclarationValidation> {
    let name = name.to_ascii_lowercase();
    if !matches!(
        name.as_str(),
        "order" | "grid-auto-flow" | "grid-auto-rows" | "grid-auto-columns"
    ) {
        return None;
    }
    if matches!(value, Value::Keyword(word) if is_css_wide_keyword(&word.to_ascii_lowercase())) {
        return Some(DeclarationValidation::Unvalidated);
    }
    Some(match name.as_str() {
        "order" => match value {
            Value::Number(number) if number.is_finite() && number.fract() == 0.0 => {
                DeclarationValidation::Valid(ComputedValue::Number(*number))
            }
            Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
                calc_unitless_number(arguments)
                    .filter(|number| number.is_finite())
                    .map(|number| {
                        DeclarationValidation::Valid(ComputedValue::Number((number + 0.5).floor()))
                    })
                    .unwrap_or(DeclarationValidation::Invalid)
            }
            _ => DeclarationValidation::Invalid,
        },
        "grid-auto-flow" => flow_value(value)
            .map(|flow| DeclarationValidation::Valid(ComputedValue::Keyword(flow)))
            .unwrap_or(DeclarationValidation::Invalid),
        _ => {
            let values = match value {
                Value::List(values) => values.as_slice(),
                value => std::slice::from_ref(value),
            };
            if !values.is_empty() && values.iter().all(|value| valid_track(value, true)) {
                DeclarationValidation::Unvalidated
            } else {
                DeclarationValidation::Invalid
            }
        }
    })
}

fn flow_value(value: &Value) -> Option<String> {
    let text = render_value(value).to_ascii_lowercase();
    let mut direction = None;
    let mut dense = false;
    for word in text.split_whitespace() {
        match word {
            "row" | "column" if direction.is_none() => direction = Some(word),
            "dense" if !dense => dense = true,
            _ => return None,
        }
    }
    match (direction, dense) {
        (None, false) => None,
        (None, true) => Some("dense".into()),
        (Some(direction), false) => Some(direction.into()),
        (Some("row"), true) => Some("dense".into()),
        (Some(direction), true) => Some(format!("{direction} dense")),
    }
}

fn valid_track(value: &Value, allow_flexible: bool) -> bool {
    match value {
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("minmax") => {
            matches!(arguments.as_slice(), [minimum, maximum] if valid_track_breadth(minimum, false) && valid_track_breadth(maximum, true))
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("fit-content") => {
            matches!(arguments.as_slice(), [argument] if valid_track_length(argument))
        }
        _ => valid_track_breadth(value, allow_flexible),
    }
}

fn valid_track_breadth(value: &Value, allow_flexible: bool) -> bool {
    match value {
        Value::Keyword(word) => matches!(
            word.to_ascii_lowercase().as_str(),
            "auto" | "min-content" | "max-content"
        ),
        Value::Length(number, unit) if unit.eq_ignore_ascii_case("fr") => {
            allow_flexible && number.is_finite() && *number >= 0.0
        }
        _ => valid_track_length(value),
    }
}

fn valid_track_length(value: &Value) -> bool {
    let math =
        matches!(value, Value::Function { name, .. } if is_length_percentage_math_function(name));
    if math {
        evaluate_length_percentage_math(value, ResolutionContext::default()).is_some()
            && valid_logical_length(value, true, true)
    } else {
        valid_logical_length(value, false, true)
    }
}

pub(super) fn compute(value: &Value, context: ResolutionContext) -> String {
    match value {
        Value::List(values) => values
            .iter()
            .map(|value| compute(value, context))
            .collect::<Vec<_>>()
            .join(" "),
        Value::Function { name, .. } if is_length_percentage_math_function(name) => {
            computed_value_css_text(&compute_value(value, "width", context))
        }
        Value::Function { name, arguments }
            if name.eq_ignore_ascii_case("minmax") || name.eq_ignore_ascii_case("fit-content") =>
        {
            format!(
                "{}({})",
                name.to_ascii_lowercase(),
                arguments
                    .iter()
                    .map(|value| compute(value, context))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Value::Number(number) if *number == 0.0 => "0px".into(),
        _ => render_grid_track_value(value, context),
    }
}

/// Validates a CSSOM assignment while keeping relative units in specified values.
pub(crate) fn normalize_specified(property: &str, text: &str) -> Option<String> {
    if !supports_declaration(property, text) {
        return None;
    }
    let declarations = super::super::parse_style_attribute(&format!("{property}: {text}"));
    let value = &declarations.first()?.value;
    if property == "grid-auto-flow"
        && !matches!(value, Value::Keyword(word) if is_css_wide_keyword(&word.to_ascii_lowercase()))
    {
        flow_value(value)
    } else {
        Some(render_value(value))
    }
}
