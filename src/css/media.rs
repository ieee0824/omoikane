//! CSS `@media` query parsing and evaluation.

use super::{MediaCondition, MediaQuery};
mod comments;
mod environment;
mod logical;
mod numeric_range;
mod palette;
mod resolution_math;
mod resolution_range;
pub use environment::MediaEnvironment;
pub use palette::ForcedColorPalette;

/// Media type used while evaluating conditional CSS rules.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MediaType {
    /// Interactive screen rendering.
    #[default]
    Screen,
    /// Printed or paged-media rendering.
    Print,
}

/// Evaluates a `@media` query against the given viewport dimensions.
///
/// Returns `true` when the query matches (i.e. its rules should apply).
///
/// `color_scheme_dark` indicates whether the system is in dark mode.
pub fn evaluate_media_query(
    query: &MediaQuery,
    viewport_width: f32,
    viewport_height: f32,
    color_scheme_dark: bool,
) -> bool {
    evaluate_media_query_for_type(
        query,
        viewport_width,
        viewport_height,
        color_scheme_dark,
        MediaType::Screen,
    )
}

/// Evaluates a media query for an explicit output media type.
pub fn evaluate_media_query_for_type(
    query: &MediaQuery,
    viewport_width: f32,
    viewport_height: f32,
    color_scheme_dark: bool,
    media_type: MediaType,
) -> bool {
    let mut environment = MediaEnvironment::default();
    environment.color_scheme_dark = color_scheme_dark;
    environment.media_type = media_type;
    evaluate_media_query_with_environment(query, viewport_width, viewport_height, &environment)
}

/// Evaluates a query against an explicit, immutable device-settings snapshot.
pub fn evaluate_media_query_with_environment(
    query: &MediaQuery,
    viewport_width: f32,
    viewport_height: f32,
    environment: &MediaEnvironment,
) -> bool {
    let type_matches = match query.media_type.as_deref() {
        None | Some("all") => true,
        Some("screen") => environment.media_type == MediaType::Screen,
        Some("print") => environment.media_type == MediaType::Print,
        Some(_) => false,
    };
    if !type_matches {
        return query.negated;
    }
    let result = logical::and(query.conditions.iter().map(|condition| {
        evaluate_condition(condition, viewport_width, viewport_height, environment)
    }));
    (if query.negated {
        result.map(|value| !value)
    } else {
        result
    }) == Some(true)
}

fn evaluate_condition(
    condition: &MediaCondition,
    viewport_width: f32,
    viewport_height: f32,
    environment: &MediaEnvironment,
) -> Option<bool> {
    let evaluate = |condition: &MediaCondition| {
        evaluate_condition(condition, viewport_width, viewport_height, environment)
    };
    Some(match condition {
        MediaCondition::Not(inner) => return evaluate(inner).map(|value| !value),
        MediaCondition::All(items) => return logical::and(items.iter().map(evaluate)),
        MediaCondition::Any(items) => return logical::or(items.iter().map(evaluate)),
        MediaCondition::MaxWidth(px) => viewport_width <= *px,
        MediaCondition::MinWidth(px) => viewport_width >= *px,
        MediaCondition::MaxWidthExclusive(px) => viewport_width < *px,
        MediaCondition::MinWidthExclusive(px) => viewport_width > *px,
        MediaCondition::MaxHeight(px) => viewport_height <= *px,
        MediaCondition::MinHeight(px) => viewport_height >= *px,
        MediaCondition::MaxHeightExclusive(px) => viewport_height < *px,
        MediaCondition::MinHeightExclusive(px) => viewport_height > *px,
        MediaCondition::OrientationPortrait => viewport_height >= viewport_width,
        MediaCondition::OrientationLandscape => viewport_width > viewport_height,
        MediaCondition::PrefersColorSchemeDark => environment.preferred_color_scheme_dark(),
        MediaCondition::PrefersColorSchemeLight => !environment.preferred_color_scheme_dark(),
        // Display capabilities come from the same immutable snapshot as
        // preferences; defaults describe the direct-color software backend.
        MediaCondition::Color { minimum, maximum } => numeric_feature_matches(
            environment.color_bits_per_component.into(),
            *minimum,
            *maximum,
            environment.color_bits_per_component != 0,
        ),
        MediaCondition::Monochrome { minimum, maximum } => numeric_feature_matches(
            environment.monochrome_bits_per_pixel.into(),
            *minimum,
            *maximum,
            environment.monochrome_bits_per_pixel != 0,
        ),
        MediaCondition::NumericFeature {
            name,
            value,
            operator,
        } => {
            return numeric_range::evaluate(
                name,
                *value,
                operator,
                viewport_width,
                viewport_height,
                environment,
            );
        }
        MediaCondition::Resolution { value, operator } => match operator.as_str() {
            "=" => environment.resolution_dppx == *value,
            ">=" => environment.resolution_dppx >= *value,
            "<=" => environment.resolution_dppx <= *value,
            ">" => environment.resolution_dppx > *value,
            "<" => environment.resolution_dppx < *value,
            _ => return None,
        },
        MediaCondition::EnvironmentFeature { name, value } => {
            return environment.matches(name, value);
        }
        MediaCondition::Unknown => return None,
    })
}

/// Parses a `@media` prelude string (the part between `@media` and `{`) into a list
/// of [`MediaQuery`] values separated by commas.
///
/// Invalid individual queries become `not all`; an empty prelude returns `None`.
pub fn parse_media_query_list(prelude: &str) -> Option<Vec<MediaQuery>> {
    let without_comments = comments::normalize(prelude);
    let prelude = without_comments.trim();
    if prelude.is_empty() {
        return None;
    }
    Some(
        super::split_top_level_commas(prelude)
            .into_iter()
            .map(|part| {
                parse_single_media_query(part.trim()).unwrap_or_else(|| MediaQuery {
                    negated: true,
                    media_type: Some("all".into()),
                    conditions: Vec::new(),
                })
            })
            .collect(),
    )
}

fn parse_single_media_query(input: &str) -> Option<MediaQuery> {
    let input = input.trim();
    let after_not = strip_keyword_prefix(input, "not");
    if input.starts_with('(') || after_not.is_some_and(|rest| rest.trim_start().starts_with('(')) {
        let condition = logical::parse_condition(input, true)?;
        return Some(MediaQuery {
            negated: false,
            media_type: None,
            conditions: unpack_conjunction(condition),
        });
    }
    let negated = after_not.is_some();
    let rest = after_not
        .or_else(|| strip_keyword_prefix(input, "only"))
        .unwrap_or(input)
        .trim_start();
    let end = rest
        .find(|ch: char| !ch.is_alphanumeric() && ch != '-' && ch != '_')
        .unwrap_or(rest.len());
    let name = rest.get(..end)?;
    if name.is_empty()
        || ["not", "only", "and", "or"]
            .iter()
            .any(|word| name.eq_ignore_ascii_case(word))
    {
        return None;
    }
    let remaining = rest[end..].trim_start();
    let conditions = if remaining.is_empty() {
        Vec::new()
    } else {
        let condition = strip_keyword_prefix(remaining, "and")?;
        unpack_conjunction(logical::parse_condition(condition.trim_start(), false)?)
    };
    Some(MediaQuery {
        negated,
        media_type: Some(name.to_ascii_lowercase()),
        conditions,
    })
}

fn unpack_conjunction(condition: MediaCondition) -> Vec<MediaCondition> {
    match condition {
        MediaCondition::All(conditions) => conditions,
        condition => vec![condition],
    }
}

/// Returns the index of the closing `)` that matches the opening `(` at index 0.
fn find_matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_media_feature(inner: &str) -> MediaCondition {
    if let Some(condition) = parse_range_media_feature(inner) {
        return condition;
    }

    // inner is e.g. "max-width: 768px" or "orientation: portrait"
    let mut parts = inner.splitn(2, ':');
    let feature = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let value = parts.next();
    // A colon requires a value; only the colon-free form is boolean syntax.
    if value.is_some_and(|value| value.trim().is_empty()) {
        return MediaCondition::Unknown;
    }
    let value_str = value.unwrap_or("").trim();

    if let Some(condition) = parse_resolution_feature(&feature, value_str) {
        return condition;
    }
    if MediaEnvironment::recognizes(&feature) {
        return MediaCondition::EnvironmentFeature {
            name: feature,
            value: value_str.to_ascii_lowercase(),
        };
    }

    if let Some(condition) = numeric_range::parse_legacy(&feature, value_str) {
        return condition;
    }
    parse_legacy_media_feature(&feature, value_str)
}

fn parse_legacy_media_feature(feature: &str, value_str: &str) -> MediaCondition {
    match feature {
        // A size feature in boolean context is true for a non-zero size.
        "width" | "height" if !value_str.is_empty() => {
            if let Some(px) = parse_length_to_px(value_str) {
                return if feature == "width" {
                    MediaCondition::All(vec![
                        MediaCondition::MinWidth(px),
                        MediaCondition::MaxWidth(px),
                    ])
                } else {
                    MediaCondition::All(vec![
                        MediaCondition::MinHeight(px),
                        MediaCondition::MaxHeight(px),
                    ])
                };
            }
        }
        "prefers-color-scheme" if value_str.is_empty() => return MediaCondition::All(Vec::new()),
        "width" if value_str.is_empty() => return MediaCondition::MinWidthExclusive(0.0),
        "height" if value_str.is_empty() => return MediaCondition::MinHeightExclusive(0.0),
        "max-width" => {
            if let Some(px) = parse_length_to_px(value_str) {
                return MediaCondition::MaxWidth(px);
            }
        }
        "min-width" => {
            if let Some(px) = parse_length_to_px(value_str) {
                return MediaCondition::MinWidth(px);
            }
        }
        "max-height" => {
            if let Some(px) = parse_length_to_px(value_str) {
                return MediaCondition::MaxHeight(px);
            }
        }
        "min-height" => {
            if let Some(px) = parse_length_to_px(value_str) {
                return MediaCondition::MinHeight(px);
            }
        }
        "orientation" => match value_str.to_ascii_lowercase().as_str() {
            "portrait" => return MediaCondition::OrientationPortrait,
            "landscape" => return MediaCondition::OrientationLandscape,
            _ => {}
        },
        "prefers-color-scheme" => match value_str.to_ascii_lowercase().as_str() {
            "dark" => return MediaCondition::PrefersColorSchemeDark,
            "light" => return MediaCondition::PrefersColorSchemeLight,
            _ => {}
        },
        "color" if value_str.is_empty() => {
            return MediaCondition::Color {
                minimum: None,
                maximum: None,
            };
        }
        "min-color" => {
            if let Some(value) = parse_signed_integer(value_str) {
                return MediaCondition::Color {
                    minimum: Some(value),
                    maximum: None,
                };
            }
        }
        "max-color" => {
            if let Some(value) = parse_signed_integer(value_str) {
                return MediaCondition::Color {
                    minimum: None,
                    maximum: Some(value),
                };
            }
        }
        "monochrome" if value_str.is_empty() => {
            return MediaCondition::Monochrome {
                minimum: None,
                maximum: None,
            };
        }
        "min-monochrome" => {
            if let Some(value) = parse_signed_integer(value_str) {
                return MediaCondition::Monochrome {
                    minimum: Some(value),
                    maximum: None,
                };
            }
        }
        "max-monochrome" => {
            if let Some(value) = parse_signed_integer(value_str) {
                return MediaCondition::Monochrome {
                    minimum: None,
                    maximum: Some(value),
                };
            }
        }
        _ => {}
    }
    MediaCondition::Unknown
}

fn parse_resolution_feature(feature: &str, value_str: &str) -> Option<MediaCondition> {
    if matches!(feature, "resolution" | "min-resolution" | "max-resolution") {
        if feature == "resolution" && value_str.is_empty() {
            return Some(MediaCondition::EnvironmentFeature {
                name: feature.into(),
                value: String::new(),
            });
        }
        return Some(
            parse_resolution(value_str).map_or(MediaCondition::Unknown, |value| {
                MediaCondition::Resolution {
                    value,
                    operator: match feature {
                        "min-resolution" => ">=",
                        "max-resolution" => "<=",
                        _ => "=",
                    }
                    .into(),
                }
            }),
        );
    }
    None
}

/// Parses the Media Queries Level 4 range syntax used by utility CSS frameworks,
/// for example `(width >= 851px)` and `(48rem <= width)`.
fn parse_range_media_feature(inner: &str) -> Option<MediaCondition> {
    if let Some(condition) = resolution_range::parse(inner) {
        return Some(condition);
    }
    if let Some(condition) = numeric_range::parse_range(inner) {
        return Some(condition);
    }
    let compact: String = inner.chars().filter(|ch| !ch.is_whitespace()).collect();
    for operator in [">=", "<=", ">", "<"] {
        let Some((left, right)) = compact.split_once(operator) else {
            continue;
        };
        let left = left.to_ascii_lowercase();
        let right = right.to_ascii_lowercase();

        return match (left.as_str(), right.as_str(), operator) {
            ("width", value, ">=") => parse_length_to_px(value).map(MediaCondition::MinWidth),
            ("width", value, "<=") => parse_length_to_px(value).map(MediaCondition::MaxWidth),
            ("width", value, ">") => {
                parse_length_to_px(value).map(MediaCondition::MinWidthExclusive)
            }
            ("width", value, "<") => {
                parse_length_to_px(value).map(MediaCondition::MaxWidthExclusive)
            }
            ("height", value, ">=") => parse_length_to_px(value).map(MediaCondition::MinHeight),
            ("height", value, "<=") => parse_length_to_px(value).map(MediaCondition::MaxHeight),
            ("height", value, ">") => {
                parse_length_to_px(value).map(MediaCondition::MinHeightExclusive)
            }
            ("height", value, "<") => {
                parse_length_to_px(value).map(MediaCondition::MaxHeightExclusive)
            }
            (value, "width", "<=") => parse_length_to_px(value).map(MediaCondition::MinWidth),
            (value, "width", ">=") => parse_length_to_px(value).map(MediaCondition::MaxWidth),
            (value, "width", "<") => {
                parse_length_to_px(value).map(MediaCondition::MinWidthExclusive)
            }
            (value, "width", ">") => {
                parse_length_to_px(value).map(MediaCondition::MaxWidthExclusive)
            }
            (value, "height", "<=") => parse_length_to_px(value).map(MediaCondition::MinHeight),
            (value, "height", ">=") => parse_length_to_px(value).map(MediaCondition::MaxHeight),
            (value, "height", "<") => {
                parse_length_to_px(value).map(MediaCondition::MinHeightExclusive)
            }
            (value, "height", ">") => {
                parse_length_to_px(value).map(MediaCondition::MaxHeightExclusive)
            }
            _ => None,
        };
    }
    None
}

fn parse_resolution(value: &str) -> Option<f32> {
    if value.contains('(') {
        return resolution_math::parse(value);
    }
    let value = value.trim().to_ascii_lowercase();
    // MQ4 defines infinite as a keyword above every numeric resolution.
    if value == "infinite" {
        return Some(f32::INFINITY);
    }
    for &(unit, factor) in resolution_math::UNITS {
        if let Some(number) = value.strip_suffix(unit) {
            let number: f32 = number.parse().ok()?;
            return (number.is_finite() && (number * factor).is_finite())
                .then_some(number * factor);
        }
    }
    None
}

fn parse_signed_integer(value: &str) -> Option<i64> {
    value.trim().parse().ok()
}

fn numeric_feature_matches(
    actual: i64,
    minimum: Option<i64>,
    maximum: Option<i64>,
    boolean_value: bool,
) -> bool {
    match (minimum, maximum) {
        (None, None) => boolean_value,
        (Some(min), None) => actual >= min,
        (None, Some(max)) => actual <= max,
        (Some(min), Some(max)) => actual >= min && actual <= max,
    }
}

/// Strips a case-insensitive keyword prefix with word boundary check.
fn strip_keyword_prefix<'a>(input: &'a str, keyword: &str) -> Option<&'a str> {
    let len = keyword.len();
    if input
        .get(..len)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(keyword))
    {
        let after = &input[len..];
        let next = after.chars().next();
        if next.is_none() || next.is_some_and(char::is_whitespace) {
            Some(after)
        } else {
            None
        }
    } else {
        None
    }
}

/// Parses a media-query length using the initial font and line-height metrics.
/// Media queries are outside any element, so document font styles do not apply.
fn parse_length_to_px(s: &str) -> Option<f32> {
    if s.contains('(') {
        return resolution_math::parse_length(s);
    }
    let lower = s.trim().to_ascii_lowercase();
    // A dimension is one CSS token; whitespace cannot separate its unit.
    if lower.chars().any(char::is_whitespace) {
        return None;
    }
    for (unit, factor) in [
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("q", 96.0 / 101.6),
        ("pt", 96.0 / 72.0),
        ("pc", 16.0),
    ] {
        if let Some(number) = lower.strip_suffix(unit) {
            return number
                .trim()
                .parse::<f32>()
                .ok()
                .map(|number| number * factor);
        }
    }
    if let Some(num_str) = lower.strip_suffix("px") {
        return num_str.trim().parse::<f32>().ok();
    }
    if let Some(num_str) = lower.strip_suffix("rem") {
        return num_str.trim().parse::<f32>().ok().map(|n| n * 16.0);
    }
    if let Some(num_str) = lower.strip_suffix("em") {
        return num_str.trim().parse::<f32>().ok().map(|n| n * 16.0);
    }
    for unit in ["rlh", "lh", "cap", "ex", "ch", "ic"] {
        if let Some(number) = lower.strip_suffix(unit) {
            let number = number.trim().parse::<f32>().ok()?;
            let metrics = crate::font::load_default_text_fonts_shared()
                .first()
                .map(|font| font.css_relative_metrics(16.0, false, false))
                .unwrap_or_else(|| crate::font::CssRelativeFontMetrics::fallback(16.0, false));
            let basis = match unit {
                "ex" => metrics.ex,
                "ch" => metrics.ch,
                "cap" => metrics.cap,
                "ic" => metrics.ic,
                "lh" | "rlh" => 19.2,
                _ => unreachable!(),
            };
            return Some(number * basis);
        }
    }
    if lower == "0" {
        return Some(0.0);
    }
    None
}

#[cfg(test)]
mod font_relative_tests {
    use super::*;

    #[test]
    fn media_query_font_units_use_initial_not_document_metrics() {
        let metrics = crate::font::load_default_text_fonts_shared()
            .first()
            .map(|font| font.css_relative_metrics(16.0, false, false))
            .unwrap_or_else(|| crate::font::CssRelativeFontMetrics::fallback(16.0, false));
        for (unit, basis) in [
            ("ex", metrics.ex),
            ("ch", metrics.ch),
            ("cap", metrics.cap),
            ("ic", metrics.ic),
            ("lh", 19.2),
            ("rlh", 19.2),
        ] {
            let query = parse_media_query_list(&format!("(min-width: 2{unit})")).unwrap();
            assert!(evaluate_media_query(&query[0], basis * 2.0, 100.0, false));
            assert!(!evaluate_media_query(&query[0], basis, 100.0, false));
        }
    }
}
