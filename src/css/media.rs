//! CSS `@media` query parsing and evaluation.

use super::{MediaCondition, MediaQuery};

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
    let type_matches = match query.media_type.as_deref() {
        None | Some("all") => true,
        Some("screen") => true,
        Some("print") => false,
        Some(_) => false,
    };

    if !type_matches {
        return query.negated;
    }

    let conditions_match = query.conditions.iter().all(|cond| match cond {
        MediaCondition::MaxWidth(px) => viewport_width <= *px,
        MediaCondition::MinWidth(px) => viewport_width >= *px,
        MediaCondition::MaxHeight(px) => viewport_height <= *px,
        MediaCondition::MinHeight(px) => viewport_height >= *px,
        MediaCondition::OrientationPortrait => viewport_height >= viewport_width,
        MediaCondition::OrientationLandscape => viewport_width > viewport_height,
        MediaCondition::PrefersColorSchemeDark => color_scheme_dark,
        MediaCondition::PrefersColorSchemeLight => !color_scheme_dark,
        // Omoikane models a color display with 8 bits per color component and
        // no monochrome framebuffer. These are the MQ3 values exposed by the
        // rendering backend rather than user preferences.
        MediaCondition::Color { minimum, maximum } => {
            numeric_feature_matches(8, *minimum, *maximum, true)
        }
        MediaCondition::Monochrome { minimum, maximum } => {
            numeric_feature_matches(0, *minimum, *maximum, false)
        }
        MediaCondition::Unknown => false,
    });

    if query.negated {
        !conditions_match
    } else {
        conditions_match
    }
}

/// Parses a `@media` prelude string (the part between `@media` and `{`) into a list
/// of [`MediaQuery`] values separated by commas.
///
/// Returns `None` on parse failure.
pub fn parse_media_query_list(prelude: &str) -> Option<Vec<MediaQuery>> {
    let prelude = prelude.trim();
    if prelude.is_empty() {
        return None;
    }
    let queries: Option<Vec<MediaQuery>> = prelude
        .split(',')
        .map(|part| parse_single_media_query(part.trim()))
        .collect();
    queries
}

fn parse_single_media_query(input: &str) -> Option<MediaQuery> {
    let input = input.trim();
    let (negated, rest) = if input.len() >= 3
        && input[..3].eq_ignore_ascii_case("not")
    {
        let after = &input[3..];
        let next = after.chars().next();
        if next.is_none() || next == Some(' ') || next == Some('\t') || next == Some('(') {
            (true, after.trim_start())
        } else {
            (false, input)
        }
    } else {
        (false, input)
    };

    // Collect tokens: media type idents and feature conditions in parentheses.
    let mut media_type: Option<String> = None;
    let mut conditions = Vec::new();
    let mut remaining = rest.trim();

    // Try to read leading media type (an ident before any `(` or `and`).
    if !remaining.starts_with('(') {
        let end = remaining
            .find(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .unwrap_or(remaining.len());
        let word = &remaining[..end];
        if !word.is_empty() && !word.eq_ignore_ascii_case("and") {
            // Strip the CSS `only` modifier (e.g. `only screen and ...`).
            // `only` is a syntactic hint for older user agents; we ignore it
            // and continue parsing the actual media type that follows.
            if word.eq_ignore_ascii_case("only") {
                remaining = remaining[end..].trim_start();
                // Read the actual media type after `only`.
                let end2 = remaining
                    .find(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
                    .unwrap_or(remaining.len());
                let type_word = &remaining[..end2];
                if !type_word.is_empty() && !type_word.eq_ignore_ascii_case("and") {
                    media_type = Some(type_word.to_ascii_lowercase());
                    remaining = remaining[end2..].trim_start();
                }
            } else {
                media_type = Some(word.to_ascii_lowercase());
                remaining = remaining[end..].trim_start();
            }
            // Consume optional `and` keyword.
            if let Some(after_and) = strip_keyword_prefix(remaining, "and") {
                let after_and = after_and.trim_start();
                if after_and.starts_with('(') || after_and.is_empty() {
                    remaining = after_and;
                }
            }
        }
    }

    // Parse zero or more feature conditions joined by `and`.
    loop {
        remaining = remaining.trim_start();
        if remaining.is_empty() {
            break;
        }
        if !remaining.starts_with('(') {
            // Skip unknown tokens (e.g. bare `and`).
            if let Some(after_and) = strip_keyword_prefix(remaining, "and") {
                remaining = after_and.trim_start();
                continue;
            }
            // A feature outside parentheses is invalid MQ3 syntax. Preserve a
            // false condition so a malformed query cannot accidentally match.
            conditions.push(MediaCondition::Unknown);
            break;
        }
        // Find matching closing paren.
        let close = find_matching_paren(remaining)?;
        let inner = remaining[1..close].trim();
        conditions.push(parse_media_feature(inner));
        remaining = &remaining[close + 1..];
        remaining = remaining.trim_start();
        // Consume optional `and` between features.
        if let Some(after_and) = strip_keyword_prefix(remaining, "and") {
            remaining = after_and.trim_start();
        }
    }

    Some(MediaQuery {
        negated,
        media_type,
        conditions,
    })
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
    // inner is e.g. "max-width: 768px" or "orientation: portrait"
    let mut parts = inner.splitn(2, ':');
    let feature = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let value_str = parts.next().unwrap_or("").trim();

    match feature.as_str() {
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
            if let Some(value) = parse_non_negative_integer(value_str) {
                return MediaCondition::Color {
                    minimum: Some(value),
                    maximum: None,
                };
            }
        }
        "max-color" => {
            if let Some(value) = parse_non_negative_integer(value_str) {
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
            if let Some(value) = parse_non_negative_integer(value_str) {
                return MediaCondition::Monochrome {
                    minimum: Some(value),
                    maximum: None,
                };
            }
        }
        "max-monochrome" => {
            if let Some(value) = parse_non_negative_integer(value_str) {
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

fn parse_non_negative_integer(value: &str) -> Option<u32> {
    value.trim().parse().ok()
}

fn numeric_feature_matches(
    actual: u32,
    minimum: Option<u32>,
    maximum: Option<u32>,
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
    if input.len() >= len && input[..len].eq_ignore_ascii_case(keyword) {
        let after = &input[len..];
        let next = after.chars().next();
        if next.is_none() || next == Some(' ') || next == Some('\t') || next == Some('(') {
            Some(after)
        } else {
            None
        }
    } else {
        None
    }
}

/// Parses a CSS length value like `768px` or `48em` and converts it to pixels.
/// Supports `px`, `em`, and `rem` (at 16px per em/rem); other units return `None`.
fn parse_length_to_px(s: &str) -> Option<f32> {
    let lower = s.trim().to_ascii_lowercase();
    if let Some(num_str) = lower.strip_suffix("px") {
        return num_str.trim().parse::<f32>().ok();
    }
    if let Some(num_str) = lower.strip_suffix("rem") {
        return num_str.trim().parse::<f32>().ok().map(|n| n * 16.0);
    }
    if let Some(num_str) = lower.strip_suffix("em") {
        return num_str.trim().parse::<f32>().ok().map(|n| n * 16.0);
    }
    if lower == "0" {
        return Some(0.0);
    }
    None
}
