//! CSS Filter Effects function-list parsing and normalization.

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FilterFunction {
    Blur(f32),
    Brightness(f32),
    Contrast(f32),
    Grayscale(f32),
    HueRotate(f32),
    Invert(f32),
    Opacity(f32),
    Saturate(f32),
    Sepia(f32),
}

pub(crate) fn parse_filter_list(input: &str) -> Option<Vec<FilterFunction>> {
    let input = input.trim();
    if input.eq_ignore_ascii_case("none") {
        return Some(Vec::new());
    }
    if input.is_empty() {
        return None;
    }

    let mut functions = Vec::new();
    let mut rest = input;
    while !rest.trim_start().is_empty() {
        rest = rest.trim_start();
        let open = rest.find('(')?;
        let name = rest[..open].trim().to_ascii_lowercase();
        if name.is_empty() || name.chars().any(|ch| !(ch.is_ascii_alphabetic() || ch == '-')) {
            return None;
        }
        let close = matching_paren(rest, open)?;
        let argument = rest[open + 1..close].trim();
        let function = match name.as_str() {
            "blur" => FilterFunction::Blur(parse_length(argument)?),
            "brightness" => FilterFunction::Brightness(parse_factor(argument, 1.0, false)?),
            "contrast" => FilterFunction::Contrast(parse_factor(argument, 1.0, false)?),
            "grayscale" => FilterFunction::Grayscale(parse_factor(argument, 1.0, true)?),
            "hue-rotate" => FilterFunction::HueRotate(parse_angle(argument)?),
            "invert" => FilterFunction::Invert(parse_factor(argument, 1.0, true)?),
            "opacity" => FilterFunction::Opacity(parse_factor(argument, 1.0, true)?),
            "saturate" => FilterFunction::Saturate(parse_factor(argument, 1.0, false)?),
            "sepia" => FilterFunction::Sepia(parse_factor(argument, 1.0, true)?),
            _ => return None,
        };
        functions.push(function);
        rest = &rest[close + 1..];
    }
    (!functions.is_empty()).then_some(functions)
}

pub(crate) fn normalize_filter_list(input: &str) -> Option<String> {
    let functions = parse_filter_list(input)?;
    if functions.is_empty() {
        return Some("none".into());
    }
    Some(functions.iter().map(format_function).collect::<Vec<_>>().join(" "))
}

pub(crate) fn interpolate_filter_lists(start: &str, end: &str, progress: f32) -> Option<String> {
    let start = parse_filter_list(start)?;
    let end = parse_filter_list(end)?;
    if start.len() != end.len() {
        return None;
    }
    let mix = |a: f32, b: f32| a + (b - a) * progress;
    let functions = start
        .iter()
        .zip(&end)
        .map(|(start, end)| match (start, end) {
            (FilterFunction::Blur(a), FilterFunction::Blur(b)) => Some(FilterFunction::Blur(mix(*a, *b))),
            (FilterFunction::Brightness(a), FilterFunction::Brightness(b)) => Some(FilterFunction::Brightness(mix(*a, *b))),
            (FilterFunction::Contrast(a), FilterFunction::Contrast(b)) => Some(FilterFunction::Contrast(mix(*a, *b))),
            (FilterFunction::Grayscale(a), FilterFunction::Grayscale(b)) => Some(FilterFunction::Grayscale(mix(*a, *b))),
            (FilterFunction::HueRotate(a), FilterFunction::HueRotate(b)) => Some(FilterFunction::HueRotate(mix(*a, *b))),
            (FilterFunction::Invert(a), FilterFunction::Invert(b)) => Some(FilterFunction::Invert(mix(*a, *b))),
            (FilterFunction::Opacity(a), FilterFunction::Opacity(b)) => Some(FilterFunction::Opacity(mix(*a, *b))),
            (FilterFunction::Saturate(a), FilterFunction::Saturate(b)) => Some(FilterFunction::Saturate(mix(*a, *b))),
            (FilterFunction::Sepia(a), FilterFunction::Sepia(b)) => Some(FilterFunction::Sepia(mix(*a, *b))),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some(functions.iter().map(format_function).collect::<Vec<_>>().join(" "))
}

fn matching_paren(input: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, ch) in input[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_length(input: &str) -> Option<f32> {
    let lower = input.trim().to_ascii_lowercase();
    let value: f32 = if lower == "0" {
        0.0
    } else {
        lower.strip_suffix("px")?.trim().parse().ok()?
    };
    (value >= 0.0 && value.is_finite()).then_some(value)
}

fn parse_factor(input: &str, default: f32, clamp_one: bool) -> Option<f32> {
    let input = input.trim();
    let value = if input.is_empty() {
        default
    } else if let Some(percent) = input.strip_suffix('%') {
        percent.trim().parse::<f32>().ok()? / 100.0
    } else {
        input.parse().ok()?
    };
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Some(if clamp_one { value.min(1.0) } else { value })
}

fn parse_angle(input: &str) -> Option<f32> {
    let lower = input.trim().to_ascii_lowercase();
    if lower == "0" {
        return Some(0.0);
    }
    let (number, scale) = if let Some(value) = lower.strip_suffix("deg") {
        (value, 1.0)
    } else if let Some(value) = lower.strip_suffix("grad") {
        (value, 0.9)
    } else if let Some(value) = lower.strip_suffix("rad") {
        (value, 180.0 / std::f32::consts::PI)
    } else if let Some(value) = lower.strip_suffix("turn") {
        (value, 360.0)
    } else {
        return None;
    };
    let degrees = number.trim().parse::<f32>().ok()? * scale;
    degrees.is_finite().then_some(degrees)
}

fn format_number(value: f32) -> String {
    if value == 0.0 { "0".into() } else { value.to_string() }
}

fn format_function(function: &FilterFunction) -> String {
    match function {
        FilterFunction::Blur(value) => format!("blur({}px)", format_number(*value)),
        FilterFunction::Brightness(value) => format!("brightness({})", format_number(*value)),
        FilterFunction::Contrast(value) => format!("contrast({})", format_number(*value)),
        FilterFunction::Grayscale(value) => format!("grayscale({})", format_number(*value)),
        FilterFunction::HueRotate(value) => format!("hue-rotate({}deg)", format_number(*value)),
        FilterFunction::Invert(value) => format!("invert({})", format_number(*value)),
        FilterFunction::Opacity(value) => format!("opacity({})", format_number(*value)),
        FilterFunction::Saturate(value) => format!("saturate({})", format_number(*value)),
        FilterFunction::Sepia(value) => format!("sepia({})", format_number(*value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_filter_lists() {
        assert_eq!(
            normalize_filter_list("brightness(150%) hue-rotate(.5turn) blur(2px)"),
            Some("brightness(1.5) hue-rotate(180deg) blur(2px)".into())
        );
        assert_eq!(normalize_filter_list("grayscale(200%)"), Some("grayscale(1)".into()));
        assert_eq!(normalize_filter_list("none"), Some("none".into()));
    }

    #[test]
    fn rejects_invalid_filter_lists() {
        for value in ["", "blur(-1px)", "brightness(-1)", "unknown(1)", "none blur(1px)", "blur(1em)"] {
            assert_eq!(normalize_filter_list(value), None, "{value}");
        }
    }

    #[test]
    fn interpolates_compatible_filter_lists() {
        assert_eq!(
            interpolate_filter_lists("brightness(1) blur(0)", "brightness(2) blur(10px)", 0.5),
            Some("brightness(1.5) blur(5px)".into())
        );
        assert_eq!(interpolate_filter_lists("blur(1px)", "sepia(1)", 0.5), None);
    }
}
