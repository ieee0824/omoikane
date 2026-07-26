//! CSS Filter Effects function-list parsing and normalization.

use crate::paint::Color;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FilterFunction {
    Blur(f32),
    Brightness(f32),
    Contrast(f32),
    DropShadow {
        offset_x: f32,
        offset_y: f32,
        blur: f32,
        color: Color,
    },
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
            "drop-shadow" => parse_drop_shadow(argument)?,
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
    let mut start = parse_filter_list(start)?;
    let mut end = parse_filter_list(end)?;
    if start.is_empty() && !end.is_empty() {
        start = end.iter().map(identity_filter).collect();
    } else if end.is_empty() && !start.is_empty() {
        end = start.iter().map(identity_filter).collect();
    }
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
            (
                FilterFunction::DropShadow { offset_x: ax, offset_y: ay, blur: ab, color: ac },
                FilterFunction::DropShadow { offset_x: bx, offset_y: by, blur: bb, color: bc },
            ) => Some(FilterFunction::DropShadow {
                offset_x: mix(*ax, *bx),
                offset_y: mix(*ay, *by),
                blur: mix(*ab, *bb),
                color: Color::rgba(
                    mix(ac.r as f32, bc.r as f32).round() as u8,
                    mix(ac.g as f32, bc.g as f32).round() as u8,
                    mix(ac.b as f32, bc.b as f32).round() as u8,
                    mix(ac.a as f32, bc.a as f32).round() as u8,
                ),
            }),
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

fn identity_filter(function: &FilterFunction) -> FilterFunction {
    match function {
        FilterFunction::Blur(_) => FilterFunction::Blur(0.0),
        FilterFunction::Brightness(_) => FilterFunction::Brightness(1.0),
        FilterFunction::Contrast(_) => FilterFunction::Contrast(1.0),
        FilterFunction::DropShadow { offset_x, offset_y, blur, .. } => FilterFunction::DropShadow {
            offset_x: *offset_x,
            offset_y: *offset_y,
            blur: *blur,
            color: Color::rgba(0, 0, 0, 0),
        },
        FilterFunction::Grayscale(_) => FilterFunction::Grayscale(0.0),
        FilterFunction::HueRotate(_) => FilterFunction::HueRotate(0.0),
        FilterFunction::Invert(_) => FilterFunction::Invert(0.0),
        FilterFunction::Opacity(_) => FilterFunction::Opacity(1.0),
        FilterFunction::Saturate(_) => FilterFunction::Saturate(1.0),
        FilterFunction::Sepia(_) => FilterFunction::Sepia(0.0),
    }
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
    } else {
        (lower.strip_suffix("turn")?, 360.0)
    };
    let degrees = number.trim().parse::<f32>().ok()? * scale;
    degrees.is_finite().then_some(degrees)
}

fn parse_drop_shadow(input: &str) -> Option<FilterFunction> {
    let mut lengths = Vec::new();
    let mut color = None;
    for component in split_whitespace_components(input)? {
        if color.is_none()
            && let Some(parsed) = crate::paint::color::parse_color(component)
        {
            color = Some(parsed);
            continue;
        }
        lengths.push(parse_signed_length(component)?);
    }
    if !(2..=3).contains(&lengths.len()) {
        return None;
    }
    let blur = lengths.get(2).copied().unwrap_or(0.0);
    if blur < 0.0 {
        return None;
    }
    Some(FilterFunction::DropShadow {
        offset_x: lengths[0],
        offset_y: lengths[1],
        blur,
        color: color.unwrap_or(Color::rgba(0, 0, 0, 255)),
    })
}

fn parse_signed_length(input: &str) -> Option<f32> {
    let lower = input.trim().to_ascii_lowercase();
    if lower == "0" {
        return Some(0.0);
    }
    let value = lower.strip_suffix("px")?.trim().parse::<f32>().ok()?;
    value.is_finite().then_some(value)
}

fn split_whitespace_components(input: &str) -> Option<Vec<&str>> {
    let mut result = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (index, ch) in input.char_indices() {
        match ch {
            '(' => { depth += 1; start.get_or_insert(index); }
            ')' => { depth = depth.checked_sub(1)?; start.get_or_insert(index); }
            _ if ch.is_whitespace() && depth == 0 => {
                if let Some(begin) = start.take() { result.push(input[begin..index].trim()); }
            }
            _ => { start.get_or_insert(index); }
        }
    }
    if depth != 0 { return None; }
    if let Some(begin) = start { result.push(input[begin..].trim()); }
    Some(result)
}

fn format_number(value: f32) -> String {
    if value == 0.0 { "0".into() } else { value.to_string() }
}

fn format_function(function: &FilterFunction) -> String {
    match function {
        FilterFunction::Blur(value) => format!("blur({}px)", format_number(*value)),
        FilterFunction::Brightness(value) => format!("brightness({})", format_number(*value)),
        FilterFunction::Contrast(value) => format!("contrast({})", format_number(*value)),
        FilterFunction::DropShadow { offset_x, offset_y, blur, color } => format!(
            "drop-shadow({}px {}px {}px rgba({}, {}, {}, {}))",
            format_number(*offset_x), format_number(*offset_y), format_number(*blur),
            color.r, color.g, color.b, format_number(color.a as f32 / 255.0)
        ),
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
        assert_eq!(
            normalize_filter_list("drop-shadow(2px 3px 4px #ff0000)"),
            Some("drop-shadow(2px 3px 4px rgba(255, 0, 0, 1))".into())
        );
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
        assert_eq!(
            interpolate_filter_lists("none", "blur(10px) brightness(2)", 0.5),
            Some("blur(5px) brightness(1.5)".into())
        );
    }
}
