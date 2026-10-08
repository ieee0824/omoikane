//! Resolution comparisons retain CSS token boundaries and range direction.
use super::{MediaCondition, parse_resolution};

pub(super) fn parse(input: &str) -> Option<MediaCondition> {
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
        operands.push(input[start..index].trim());
        let operator_start = index;
        index += 1;
        if bytes.get(index) == Some(&b'=') && bytes[operator_start] != b'=' {
            index += 1;
        }
        operators.push(&input[operator_start..index]);
        start = index;
    }
    operands.push(input[start..].trim());
    if operators.is_empty() {
        return None;
    }
    if !operands
        .iter()
        .any(|value| value.eq_ignore_ascii_case("resolution"))
    {
        return None;
    }
    let condition = match (operands.as_slice(), operators.as_slice()) {
        ([left, right], [operator]) => comparison(left, operator, right),
        ([left, feature, right], [first, second])
            if feature.eq_ignore_ascii_case("resolution") && same_direction(first, second) =>
        {
            Some(MediaCondition::All(vec![
                comparison(left, first, feature)?,
                comparison(feature, second, right)?,
            ]))
        }
        _ => None,
    };
    Some(condition.unwrap_or(MediaCondition::Unknown))
}

fn same_direction(first: &str, second: &str) -> bool {
    matches!(
        (first.as_bytes().first(), second.as_bytes().first()),
        (Some(b'<'), Some(b'<')) | (Some(b'>'), Some(b'>'))
    )
}

fn comparison(left: &str, operator: &str, right: &str) -> Option<MediaCondition> {
    let (number, operator) = if left.eq_ignore_ascii_case("resolution") {
        (right, operator)
    } else if right.eq_ignore_ascii_case("resolution") {
        (
            left,
            match operator {
                "<" => ">",
                "<=" => ">=",
                ">" => "<",
                ">=" => "<=",
                "=" => "=",
                _ => return None,
            },
        )
    } else {
        return None;
    };
    Some(MediaCondition::Resolution {
        value: parse_resolution(number)?,
        operator: operator.into(),
    })
}
