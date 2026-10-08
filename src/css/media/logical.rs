//! Owned media-condition syntax and three-valued boolean operations.
use super::{MediaCondition, find_matching_paren, parse_media_feature, strip_keyword_prefix};

pub(super) fn and(values: impl IntoIterator<Item = Option<bool>>) -> Option<bool> {
    let mut unknown = false;
    for value in values {
        match value {
            Some(false) => return Some(false),
            None => unknown = true,
            Some(true) => {}
        }
    }
    if unknown { None } else { Some(true) }
}

pub(super) fn or(values: impl IntoIterator<Item = Option<bool>>) -> Option<bool> {
    let mut unknown = false;
    for value in values {
        match value {
            Some(true) => return Some(true),
            None => unknown = true,
            Some(false) => {}
        }
    }
    if unknown { None } else { Some(false) }
}

pub(super) fn parse_condition(input: &str, allow_or: bool) -> Option<MediaCondition> {
    if let Some(after) = strip_keyword_prefix(input, "not") {
        let (condition, remainder) = parse_parenthesized(after.trim_start())?;
        return remainder
            .trim()
            .is_empty()
            .then(|| MediaCondition::Not(Box::new(condition)));
    }
    let (first, mut remainder) = parse_parenthesized(input)?;
    let mut conditions = vec![first];
    let mut is_or = None;
    loop {
        remainder = remainder.trim_start();
        if remainder.is_empty() {
            break;
        }
        let (disjunction, after) = if let Some(after) = strip_keyword_prefix(remainder, "and") {
            (false, after)
        } else if allow_or {
            (true, strip_keyword_prefix(remainder, "or")?)
        } else {
            return None;
        };
        if is_or.is_some_and(|previous| previous != disjunction) {
            return None;
        }
        is_or = Some(disjunction);
        let (next, after) = parse_parenthesized(after.trim_start())?;
        conditions.push(next);
        remainder = after;
    }
    if conditions.len() == 1 {
        return conditions.pop();
    }
    Some(if is_or == Some(true) {
        MediaCondition::Any(conditions)
    } else {
        MediaCondition::All(conditions)
    })
}

fn parse_parenthesized(input: &str) -> Option<(MediaCondition, &str)> {
    if !input.starts_with('(') {
        return None;
    }
    let close = find_matching_paren(input)?;
    let inner = input[1..close].trim();
    if inner.is_empty() {
        return None;
    }
    let condition = if inner.starts_with('(') || strip_keyword_prefix(inner, "not").is_some() {
        parse_condition(inner, true)?
    } else {
        parse_media_feature(inner)
    };
    Some((condition, &input[close + 1..]))
}
