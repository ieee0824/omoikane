//! Numeric media features evaluated from explicit display and viewport values.
use super::{MediaCondition, MediaEnvironment, parse_length_to_px};

const FEATURES: &[&str] = &[
    "width",
    "height",
    "device-width",
    "device-height",
    "color",
    "color-index",
    "monochrome",
    "aspect-ratio",
    "device-aspect-ratio",
];

fn value(feature: &str, input: &str) -> Option<f64> {
    if matches!(feature, "aspect-ratio" | "device-aspect-ratio") {
        return parse_ratio(input);
    }
    let value = if matches!(feature, "color" | "color-index" | "monochrome") {
        input.parse::<i64>().ok()? as f64
    } else {
        // A dimension token cannot contain whitespace between number and unit.
        if !input.contains('(') && input.chars().any(char::is_whitespace) {
            return None;
        }
        parse_length_to_px(input)? as f64
    };
    value.is_finite().then_some(value)
}

fn parse_ratio(input: &str) -> Option<f64> {
    if input.contains('(') {
        let value = super::resolution_math::parse_number(input)?;
        return (value >= 0.0).then_some(value);
    }
    fn component(input: &str) -> Option<f64> {
        let input = input.trim();
        let tokens = super::super::tokenize(input).ok()?;
        if !matches!(tokens.as_slice(), [super::super::CssToken::Number(_)]) {
            return None;
        }
        let number: f64 = input.parse().ok()?;
        (number.is_finite() && number >= 0.0).then_some(number)
    }
    let mut parts = input.split('/');
    let numerator = component(parts.next()?)?;
    let denominator = if let Some(value) = parts.next() {
        component(value)?
    } else {
        1.0
    };
    if parts.next().is_some() {
        return None;
    }
    Some(ratio(numerator, denominator))
}

fn ratio(width: f64, height: f64) -> f64 {
    // Fixed WPT aspect-ratio-002/004 define a zero denominator, including
    // 0/0, as infinite. Ordinary nondegenerate ratios use number division.
    if height == 0.0 {
        f64::INFINITY
    } else {
        width / height
    }
}

fn condition(name: &str, value: f64, operator: &str) -> MediaCondition {
    match (name, operator) {
        ("width", ">=") => return MediaCondition::MinWidth(value as f32),
        ("width", "<=") => return MediaCondition::MaxWidth(value as f32),
        ("width", ">") => return MediaCondition::MinWidthExclusive(value as f32),
        ("width", "<") => return MediaCondition::MaxWidthExclusive(value as f32),
        ("height", ">=") => return MediaCondition::MinHeight(value as f32),
        ("height", "<=") => return MediaCondition::MaxHeight(value as f32),
        ("height", ">") => return MediaCondition::MinHeightExclusive(value as f32),
        ("height", "<") => return MediaCondition::MaxHeightExclusive(value as f32),
        _ => {}
    }
    MediaCondition::NumericFeature {
        name: name.into(),
        value,
        operator: operator.into(),
    }
}

pub(super) fn parse_legacy(feature: &str, input: &str) -> Option<MediaCondition> {
    let (name, operator) = if let Some(name) = feature.strip_prefix("min-") {
        (name, ">=")
    } else if let Some(name) = feature.strip_prefix("max-") {
        (name, "<=")
    } else {
        (feature, "=")
    };
    let legacy_color_equality =
        matches!(name, "color" | "monochrome") && operator == "=" && !input.is_empty();
    if !matches!(
        name,
        "color-index" | "device-width" | "device-height" | "aspect-ratio" | "device-aspect-ratio"
    ) && !legacy_color_equality
    {
        return None;
    }
    if input.is_empty() {
        return Some(if operator == "=" {
            condition(name, 0.0, ">")
        } else {
            MediaCondition::Unknown
        });
    }
    Some(value(name, input).map_or(MediaCondition::Unknown, |value| {
        condition(name, value, operator)
    }))
}

pub(super) fn parse_range(input: &str) -> Option<MediaCondition> {
    let mut operands = Vec::new();
    let mut operators = Vec::new();
    let mut start = 0;
    let mut index = 0;
    let bytes = input.as_bytes();
    while index < bytes.len() {
        if !matches!(bytes[index], b'<' | b'>' | b'=') {
            index += 1;
            continue;
        }
        operands.push(input[start..index].trim().to_ascii_lowercase());
        let begin = index;
        index += 1;
        if bytes.get(index) == Some(&b'=') && bytes[begin] != b'=' {
            index += 1;
        }
        operators.push(&input[begin..index]);
        start = index;
    }
    operands.push(input[start..].trim().to_ascii_lowercase());
    if operators.is_empty()
        || !operands
            .iter()
            .any(|part| FEATURES.contains(&part.as_str()))
    {
        return None;
    }
    let parsed = match (operands.as_slice(), operators.as_slice()) {
        ([left, right], [operator]) => comparison(left, operator, right),
        ([left, name, right], [first, second])
            if FEATURES.contains(&name.as_str())
                && matches!(
                    (first.as_bytes()[0], second.as_bytes()[0]),
                    (b'<', b'<') | (b'>', b'>')
                ) =>
        {
            comparison(left, first, name)
                .zip(comparison(name, second, right))
                .map(|(left, right)| MediaCondition::All(vec![left, right]))
        }
        _ => None,
    };
    Some(parsed.unwrap_or(MediaCondition::Unknown))
}

fn comparison(left: &str, operator: &str, right: &str) -> Option<MediaCondition> {
    if FEATURES.contains(&left) {
        Some(condition(left, value(left, right)?, operator))
    } else if FEATURES.contains(&right) {
        let reverse = match operator {
            "<" => ">",
            "<=" => ">=",
            ">" => "<",
            ">=" => "<=",
            "=" => "=",
            _ => return None,
        };
        Some(condition(right, value(right, left)?, reverse))
    } else {
        None
    }
}

pub(super) fn evaluate(
    name: &str,
    value: f64,
    operator: &str,
    width: f32,
    height: f32,
    environment: &MediaEnvironment,
) -> Option<bool> {
    let actual = match name {
        "aspect-ratio" => ratio(width as f64, height as f64),
        "device-aspect-ratio" => ratio(
            environment.device_width.unwrap_or(width) as f64,
            environment.device_height.unwrap_or(height) as f64,
        ),
        "width" => width as f64,
        "height" => height as f64,
        "device-width" => environment.device_width.unwrap_or(width) as f64,
        "device-height" => environment.device_height.unwrap_or(height) as f64,
        "color" => environment.color_bits_per_component as f64,
        "color-index" => environment.color_index as f64,
        "monochrome" => environment.monochrome_bits_per_pixel as f64,
        _ => return None,
    };
    Some(match operator {
        "=" => actual == value,
        "<" => actual < value,
        "<=" => actual <= value,
        ">" => actual > value,
        ">=" => actual >= value,
        _ => return None,
    })
}
