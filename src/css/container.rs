//! CSS Container Queries size-condition parsing and evaluation.

/// A parsed `@container` prelude.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContainerQuery {
    /// Optional `<custom-ident>` selecting a named query container.
    pub name: Option<String>,
    condition: Condition,
}

#[derive(Debug, Clone, PartialEq)]
enum Condition {
    Feature(Feature),
    And(Vec<Condition>),
    Or(Vec<Condition>),
    Not(Box<Condition>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Inline,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Feature {
    axis: Axis,
    comparison: Comparison,
    value_px: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Comparison {
    Less,
    LessEqual,
    Equal,
    GreaterEqual,
    Greater,
}

impl ContainerQuery {
    /// Evaluates the query in the engine's current horizontal writing mode.
    pub fn matches(&self, width: f32, height: f32) -> bool {
        self.condition.matches(width, height)
    }

    /// Whether evaluating this query requires block-axis size containment.
    pub fn requires_block_size(&self) -> bool {
        self.condition.requires_block_size()
    }
}

impl Condition {
    fn matches(&self, width: f32, height: f32) -> bool {
        match self {
            Self::Feature(feature) => feature.matches(width, height),
            Self::And(conditions) => conditions
                .iter()
                .all(|condition| condition.matches(width, height)),
            Self::Or(conditions) => conditions
                .iter()
                .any(|condition| condition.matches(width, height)),
            Self::Not(condition) => !condition.matches(width, height),
        }
    }

    fn requires_block_size(&self) -> bool {
        match self {
            Self::Feature(feature) => feature.axis == Axis::Block,
            Self::And(conditions) | Self::Or(conditions) => {
                conditions.iter().any(Self::requires_block_size)
            }
            Self::Not(condition) => condition.requires_block_size(),
        }
    }
}

impl Feature {
    fn matches(&self, width: f32, height: f32) -> bool {
        let actual = match self.axis {
            Axis::Inline => width,
            Axis::Block => height,
        };
        match self.comparison {
            Comparison::Less => actual < self.value_px,
            Comparison::LessEqual => actual <= self.value_px,
            // Layout uses single-precision geometry, so decimal lengths and
            // accumulated box arithmetic can differ by a tiny subpixel amount.
            Comparison::Equal => (actual - self.value_px).abs() <= 0.01,
            Comparison::GreaterEqual => actual >= self.value_px,
            Comparison::Greater => actual > self.value_px,
        }
    }
}

/// Parses an optional container name followed by a size query condition.
pub(crate) fn parse_container_query(input: &str) -> Option<ContainerQuery> {
    let input = input.trim();
    let condition_start = input.find('(')?;
    let name_text = input[..condition_start].trim();
    let (name, condition_text) = if name_text.is_empty() {
        (None, &input[condition_start..])
    } else if name_text.eq_ignore_ascii_case("not") {
        (None, input)
    } else if is_custom_ident(name_text) {
        (Some(name_text.to_string()), &input[condition_start..])
    } else {
        return None;
    };
    let condition = parse_condition(condition_text.trim())?;
    Some(ContainerQuery { name, condition })
}

fn parse_condition(input: &str) -> Option<Condition> {
    let input = input.trim();
    if let Some(rest) = strip_keyword(input, "not") {
        return Some(Condition::Not(Box::new(parse_in_parens(rest.trim())?)));
    }

    let and_parts = split_top_level_keyword(input, "and")?;
    if and_parts.len() > 1 {
        if and_parts
            .iter()
            .any(|part| contains_top_level_keyword(part, "or"))
        {
            return None;
        }
        return and_parts
            .into_iter()
            .map(parse_in_parens)
            .collect::<Option<Vec<_>>>()
            .map(Condition::And);
    }
    let or_parts = split_top_level_keyword(input, "or")?;
    if or_parts.len() > 1 {
        return or_parts
            .into_iter()
            .map(parse_in_parens)
            .collect::<Option<Vec<_>>>()
            .map(Condition::Or);
    }
    parse_in_parens(input)
}

fn parse_in_parens(input: &str) -> Option<Condition> {
    let input = input.trim();
    if !input.starts_with('(') || matching_close(input)? != input.len() - 1 {
        return None;
    }
    let inner = input[1..input.len() - 1].trim();
    parse_chained_range(inner)
        .or_else(|| parse_feature(inner).map(Condition::Feature))
        .or_else(|| parse_condition(inner))
}

fn parse_chained_range(input: &str) -> Option<Condition> {
    let compact: String = input
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    for axis_name in ["inline-size", "block-size", "width", "height"] {
        let Some(axis_start) = compact.find(axis_name) else {
            continue;
        };
        let left = &compact[..axis_start];
        let right = &compact[axis_start + axis_name.len()..];
        if left.is_empty() || right.is_empty() {
            continue;
        }
        let (left_value, left_operator) = split_trailing_operator(left)?;
        let (right_operator, right_value) = split_leading_operator(right)?;
        let axis = parse_axis(axis_name)?;
        return Some(Condition::And(vec![
            Condition::Feature(Feature {
                axis,
                comparison: reverse_comparison(parse_comparison(left_operator)?),
                value_px: parse_length(left_value)?,
            }),
            Condition::Feature(Feature {
                axis,
                comparison: parse_comparison(right_operator)?,
                value_px: parse_length(right_value)?,
            }),
        ]));
    }
    None
}

fn split_trailing_operator(input: &str) -> Option<(&str, &str)> {
    for operator in [">=", "<=", ">", "<", "="] {
        if let Some(value) = input.strip_suffix(operator) {
            return Some((value, operator));
        }
    }
    None
}

fn split_leading_operator(input: &str) -> Option<(&str, &str)> {
    for operator in [">=", "<=", ">", "<", "="] {
        if let Some(value) = input.strip_prefix(operator) {
            return Some((operator, value));
        }
    }
    None
}

fn parse_feature(input: &str) -> Option<Feature> {
    if let Some((feature, value)) = input.split_once(':') {
        let feature = feature.trim().to_ascii_lowercase();
        let (comparison, axis) =
            if let Some(axis) = feature.strip_prefix("min-").and_then(parse_axis) {
                (Comparison::GreaterEqual, axis)
            } else if let Some(axis) = feature.strip_prefix("max-").and_then(parse_axis) {
                (Comparison::LessEqual, axis)
            } else {
                (Comparison::Equal, parse_axis(&feature)?)
            };
        return Some(Feature {
            axis,
            comparison,
            value_px: parse_length(value)?,
        });
    }

    let compact: String = input
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    for operator in [">=", "<=", ">", "<", "="] {
        let Some((left, right)) = compact.split_once(operator) else {
            continue;
        };
        if let Some(axis) = parse_axis(&left.to_ascii_lowercase()) {
            return Some(Feature {
                axis,
                comparison: parse_comparison(operator)?,
                value_px: parse_length(right)?,
            });
        }
        if let Some(axis) = parse_axis(&right.to_ascii_lowercase()) {
            return Some(Feature {
                axis,
                comparison: reverse_comparison(parse_comparison(operator)?),
                value_px: parse_length(left)?,
            });
        }
    }
    None
}

fn parse_axis(input: &str) -> Option<Axis> {
    match input {
        "width" | "inline-size" => Some(Axis::Inline),
        "height" | "block-size" => Some(Axis::Block),
        _ => None,
    }
}

fn parse_comparison(input: &str) -> Option<Comparison> {
    match input {
        "<" => Some(Comparison::Less),
        "<=" => Some(Comparison::LessEqual),
        "=" => Some(Comparison::Equal),
        ">=" => Some(Comparison::GreaterEqual),
        ">" => Some(Comparison::Greater),
        _ => None,
    }
}

fn reverse_comparison(comparison: Comparison) -> Comparison {
    match comparison {
        Comparison::Less => Comparison::Greater,
        Comparison::LessEqual => Comparison::GreaterEqual,
        Comparison::Equal => Comparison::Equal,
        Comparison::GreaterEqual => Comparison::LessEqual,
        Comparison::Greater => Comparison::Less,
    }
}

fn parse_length(input: &str) -> Option<f32> {
    let lower = input.trim().to_ascii_lowercase();
    if lower == "0" {
        return Some(0.0);
    }
    for (suffix, scale) in [("rem", 16.0), ("em", 16.0), ("px", 1.0)] {
        if let Some(number) = lower.strip_suffix(suffix) {
            return number
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|value| *value >= 0.0)
                .map(|value| value * scale);
        }
    }
    None
}

fn matching_close(input: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (index, character) in input.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_keyword<'a>(input: &'a str, keyword: &str) -> Option<Vec<&'a str>> {
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut parts = Vec::new();
    let bytes = input.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => depth = depth.checked_sub(1)?,
            _ if depth == 0 && keyword_at(input, index, keyword) => {
                let part = input[start..index].trim();
                if part.is_empty() {
                    return None;
                }
                parts.push(part);
                index += keyword.len();
                start = index;
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    if depth != 0 {
        return None;
    }
    let tail = input[start..].trim();
    if tail.is_empty() {
        return None;
    }
    parts.push(tail);
    Some(parts)
}

fn contains_top_level_keyword(input: &str, keyword: &str) -> bool {
    split_top_level_keyword(input, keyword).is_some_and(|parts| parts.len() > 1)
}

fn keyword_at(input: &str, index: usize, keyword: &str) -> bool {
    let end = index + keyword.len();
    end <= input.len()
        && input[index..end].eq_ignore_ascii_case(keyword)
        && input[..index]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
        && input[end..].chars().next().is_some_and(char::is_whitespace)
}

fn strip_keyword<'a>(input: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = input.get(keyword.len()..)?;
    (input[..keyword.len()].eq_ignore_ascii_case(keyword)
        && rest.chars().next().is_some_and(char::is_whitespace))
    .then_some(rest)
}

fn is_custom_ident(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    !matches!(lower.as_str(), "none" | "and" | "or" | "not" | "default")
        && input.chars().enumerate().all(|(index, character)| {
            character == '-'
                || character == '_'
                || character.is_ascii_alphanumeric() && (index > 0 || !character.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::parse_container_query;

    #[test]
    fn parses_named_and_unnamed_size_queries() {
        let query = parse_container_query("card (width >= 400px)").unwrap();
        assert_eq!(query.name.as_deref(), Some("card"));
        assert!(query.matches(400.0, 10.0));
        assert!(!query.matches(399.0, 10.0));

        let query = parse_container_query("(20rem < inline-size) and (max-height: 50px)").unwrap();
        assert!(query.matches(321.0, 50.0));
        assert!(!query.matches(320.0, 50.0));

        let query = parse_container_query("(300px < width <= 500px)").unwrap();
        assert!(query.matches(301.0, 0.0));
        assert!(query.matches(500.0, 0.0));
        assert!(!query.matches(300.0, 0.0));
        assert!(!query.matches(501.0, 0.0));

        let query = parse_container_query("(width = 100px)").unwrap();
        assert!(query.matches(100.005, 0.0));
        assert!(!query.matches(100.02, 0.0));
    }

    #[test]
    fn supports_boolean_conditions_and_rejects_invalid_preludes() {
        let query = parse_container_query("not ((width < 100px) or (block-size > 50px))").unwrap();
        assert!(query.matches(100.0, 50.0));
        assert!(!query.matches(99.0, 50.0));
        assert!(parse_container_query("none (width > 1px)").is_none());
        assert_eq!(
            parse_container_query("--card (width > 1px)")
                .unwrap()
                .name
                .as_deref(),
            Some("--card")
        );
        assert!(parse_container_query("card style(--theme: dark)").is_none());
        assert!(parse_container_query("card (orientation: landscape)").is_none());
    }
}
