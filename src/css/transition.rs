//! CSS Transitions declaration parsing.
//!
//! Transition values use comma-separated lists whose commas must survive the
//! generic declaration parser. This module validates those lists and expands
//! the shorthand into its four longhands. Timeline sampling lives above the
//! CSS parser and consumes the normalized longhand values produced here.

use super::{Declaration, Value};

#[derive(Debug, Clone, PartialEq)]
struct TransitionDescriptor {
    property: String,
    duration: String,
    timing_function: String,
    delay: String,
}

pub(crate) fn expand_transition_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let Value::Keyword(input) = value else {
        return Vec::new();
    };
    let lower = input.trim().to_ascii_lowercase();
    if is_css_wide_keyword(&lower) {
        return transition_longhands()
            .into_iter()
            .map(|name| Declaration {
                name: name.to_string(),
                value: Value::Keyword(lower.clone()),
                important,
            })
            .collect();
    }
    let Some(descriptors) = parse_transition_shorthand(&input) else {
        return Vec::new();
    };

    let join = |select: fn(&TransitionDescriptor) -> &str| {
        descriptors
            .iter()
            .map(select)
            .collect::<Vec<_>>()
            .join(", ")
    };
    vec![
        Declaration {
            name: "transition-property".to_string(),
            value: Value::Keyword(join(|item| &item.property)),
            important,
        },
        Declaration {
            name: "transition-duration".to_string(),
            value: Value::Keyword(join(|item| &item.duration)),
            important,
        },
        Declaration {
            name: "transition-timing-function".to_string(),
            value: Value::Keyword(join(|item| &item.timing_function)),
            important,
        },
        Declaration {
            name: "transition-delay".to_string(),
            value: Value::Keyword(join(|item| &item.delay)),
            important,
        },
    ]
}

pub(crate) fn normalize_transition_longhand(name: &str, value: &str) -> Option<String> {
    let input = value.trim();
    let lower = input.to_ascii_lowercase();
    if is_css_wide_keyword(&lower) {
        return Some(lower);
    }
    let items = split_top_level(input, ',')?;
    if items.is_empty() {
        return None;
    }
    let normalized = match name {
        "transition-property" => {
            let mut properties = Vec::with_capacity(items.len());
            for item in items {
                let property = item.trim().to_ascii_lowercase();
                if !is_transition_property_name(&property) {
                    return None;
                }
                properties.push(property);
            }
            if properties.len() > 1 && properties.iter().any(|item| item == "none") {
                return None;
            }
            properties
        }
        "transition-duration" => items
            .into_iter()
            .map(|item| normalize_time(item, false))
            .collect::<Option<Vec<_>>>()?,
        "transition-delay" => items
            .into_iter()
            .map(|item| normalize_time(item, true))
            .collect::<Option<Vec<_>>>()?,
        "transition-timing-function" => items
            .into_iter()
            .map(normalize_timing_function)
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    Some(normalized.join(", "))
}

fn parse_transition_shorthand(input: &str) -> Option<Vec<TransitionDescriptor>> {
    let items = split_top_level(input, ',')?;
    if items.is_empty() {
        return None;
    }
    let mut descriptors = Vec::with_capacity(items.len());
    for item in items {
        let components = split_top_level_whitespace(item)?;
        if components.is_empty() {
            return None;
        }
        let component_count = components.len();
        let mut property = None;
        let mut duration = None;
        let mut timing_function = None;
        let mut delay = None;
        for component in components {
            if let Some(time) = normalize_time(component, delay.is_none()) {
                if duration.is_none() {
                    if time.starts_with('-') {
                        return None;
                    }
                    duration = Some(time);
                } else if delay.is_none() {
                    delay = normalize_time(component, true);
                } else {
                    return None;
                }
                continue;
            }
            if timing_function.is_none()
                && let Some(timing) = normalize_timing_function(component)
            {
                timing_function = Some(timing);
                continue;
            }
            let candidate = component.trim().to_ascii_lowercase();
            if property.is_none() && is_transition_property_name(&candidate) {
                property = Some(candidate);
                continue;
            }
            return None;
        }
        let property = property.unwrap_or_else(|| "all".to_string());
        // `none` is the alternative to the whole `<single-transition>#`
        // grammar, not a property name that can be combined with timings.
        if property == "none" && component_count != 1 {
            return None;
        }
        descriptors.push(TransitionDescriptor {
            property,
            duration: duration.unwrap_or_else(|| "0s".to_string()),
            timing_function: timing_function.unwrap_or_else(|| "ease".to_string()),
            delay: delay.unwrap_or_else(|| "0s".to_string()),
        });
    }
    if descriptors.len() > 1 && descriptors.iter().any(|item| item.property == "none") {
        return None;
    }
    Some(descriptors)
}

fn transition_longhands() -> [&'static str; 4] {
    [
        "transition-property",
        "transition-duration",
        "transition-timing-function",
        "transition-delay",
    ]
}

fn split_top_level(input: &str, separator: char) -> Option<Vec<&str>> {
    let mut values = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in input.char_indices() {
        match ch {
            '(' => depth = depth.checked_add(1)?,
            ')' => depth = depth.checked_sub(1)?,
            _ if ch == separator && depth == 0 => {
                let value = input[start..index].trim();
                if value.is_empty() {
                    return None;
                }
                values.push(value);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    let value = input[start..].trim();
    if value.is_empty() {
        return None;
    }
    values.push(value);
    Some(values)
}

fn split_top_level_whitespace(input: &str) -> Option<Vec<&str>> {
    let mut values = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (index, ch) in input.char_indices() {
        match ch {
            '(' => {
                depth = depth.checked_add(1)?;
                start.get_or_insert(index);
            }
            ')' => {
                depth = depth.checked_sub(1)?;
                start.get_or_insert(index);
            }
            _ if ch.is_whitespace() && depth == 0 => {
                if let Some(component_start) = start.take() {
                    values.push(input[component_start..index].trim());
                }
            }
            _ => {
                start.get_or_insert(index);
            }
        }
    }
    if depth != 0 {
        return None;
    }
    if let Some(component_start) = start {
        values.push(input[component_start..].trim());
    }
    Some(values)
}

fn normalize_time(input: &str, allow_negative: bool) -> Option<String> {
    let lower = input.trim().to_ascii_lowercase();
    let (number, unit) = if let Some(number) = lower.strip_suffix("ms") {
        (number, "ms")
    } else if let Some(number) = lower.strip_suffix('s') {
        (number, "s")
    } else if lower == "0" || lower == "+0" || lower == "-0" {
        return Some("0s".to_string());
    } else {
        return None;
    };
    let parsed = number.parse::<f32>().ok()?;
    if !parsed.is_finite() || (!allow_negative && parsed < 0.0) {
        return None;
    }
    Some(format_number(parsed) + unit)
}

fn normalize_timing_function(input: &str) -> Option<String> {
    let lower = input.trim().to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "linear" | "ease" | "ease-in" | "ease-out" | "ease-in-out" | "step-start" | "step-end"
    ) {
        return Some(lower);
    }
    if let Some(arguments) = function_arguments(&lower, "cubic-bezier") {
        let values = split_top_level(arguments, ',')?;
        if values.len() != 4 {
            return None;
        }
        let numbers = values
            .iter()
            .map(|value| value.parse::<f32>().ok())
            .collect::<Option<Vec<_>>>()?;
        if numbers.iter().any(|number| !number.is_finite())
            || !(0.0..=1.0).contains(&numbers[0])
            || !(0.0..=1.0).contains(&numbers[2])
        {
            return None;
        }
        return Some(format!(
            "cubic-bezier({}, {}, {}, {})",
            format_number(numbers[0]),
            format_number(numbers[1]),
            format_number(numbers[2]),
            format_number(numbers[3])
        ));
    }
    if let Some(arguments) = function_arguments(&lower, "steps") {
        let values = split_top_level(arguments, ',')?;
        if values.is_empty() || values.len() > 2 {
            return None;
        }
        let count = values[0].parse::<u32>().ok()?;
        if count == 0 {
            return None;
        }
        if values.len() == 1 {
            return Some(format!("steps({count})"));
        }
        let position = values[1].trim();
        if !matches!(
            position,
            "jump-start" | "jump-end" | "jump-none" | "jump-both" | "start" | "end"
        ) || (position == "jump-none" && count < 2)
        {
            return None;
        }
        return Some(format!("steps({count}, {position})"));
    }
    None
}

fn function_arguments<'a>(input: &'a str, name: &str) -> Option<&'a str> {
    input
        .strip_prefix(name)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

fn is_transition_property_name(input: &str) -> bool {
    if input.is_empty() || is_css_wide_keyword(input) {
        return false;
    }
    if input == "all" || input == "none" {
        return true;
    }
    let mut chars = input.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_' || first == '-')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn is_css_wide_keyword(input: &str) -> bool {
    matches!(
        input,
        "inherit" | "initial" | "unset" | "revert" | "revert-layer"
    )
}

fn format_number(value: f32) -> String {
    if value == 0.0 {
        "0".to_string()
    } else if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_shorthand_lists_and_defaults() {
        let declarations = expand_transition_shorthand(
            Value::Keyword(
                "opacity 200ms linear 50ms, transform 1s cubic-bezier(.1, .2, .3, 1)".into(),
            ),
            false,
        );
        assert_eq!(declarations.len(), 4);
        assert_eq!(
            declarations[0].value,
            Value::Keyword("opacity, transform".into())
        );
        assert_eq!(declarations[1].value, Value::Keyword("200ms, 1s".into()));
        assert_eq!(
            declarations[2].value,
            Value::Keyword("linear, cubic-bezier(0.1, 0.2, 0.3, 1)".into())
        );
        assert_eq!(declarations[3].value, Value::Keyword("50ms, 0s".into()));
    }

    #[test]
    fn rejects_negative_duration_and_invalid_timing_functions() {
        assert!(parse_transition_shorthand("opacity -1s").is_none());
        assert!(parse_transition_shorthand("none 1s").is_none());
        assert!(parse_transition_shorthand("opacity 1s cubic-bezier(2, 0, 0, 1)").is_none());
        assert!(normalize_transition_longhand("transition-duration", "1s, -1ms").is_none());
        assert!(normalize_transition_longhand("transition-timing-function", "steps(0)").is_none());
    }

    #[test]
    fn accepts_negative_delays_and_css_wide_keywords() {
        let descriptors = parse_transition_shorthand("opacity 2s ease -500ms").unwrap();
        assert_eq!(descriptors[0].duration, "2s");
        assert_eq!(descriptors[0].delay, "-500ms");
        assert_eq!(
            normalize_transition_longhand("transition-property", "initial"),
            Some("initial".into())
        );
    }
}
