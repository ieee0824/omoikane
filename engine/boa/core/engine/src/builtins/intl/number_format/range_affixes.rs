//! Affix sharing uses formatter style and typed part boundaries.

use std::ops::Range;

use super::{
    options::{CurrencyDisplay, UnitFormatOptions},
    parts::NumberPart,
};

#[derive(Clone, Copy, Debug)]
pub(super) enum Collapse {
    Numeric,
    Pattern,
    Unit,
}

impl Collapse {
    pub(super) fn for_style(style: &UnitFormatOptions) -> Self {
        match style {
            UnitFormatOptions::Decimal => Self::Numeric,
            UnitFormatOptions::Unit { .. }
            | UnitFormatOptions::Currency {
                display: CurrencyDisplay::Name,
                ..
            } => Self::Unit,
            UnitFormatOptions::Currency { .. } | UnitFormatOptions::Percent => Self::Pattern,
        }
    }
}

#[derive(Debug)]
pub(super) struct SharedAffixes {
    pub(super) prefix: Vec<NumberPart>,
    pub(super) suffix: Vec<NumberPart>,
}

pub(super) fn collapse(
    start: &mut Vec<NumberPart>,
    end: &mut Vec<NumberPart>,
    mode: Collapse,
) -> SharedAffixes {
    let empty = || SharedAffixes {
        prefix: Vec::new(),
        suffix: Vec::new(),
    };
    let (Some(left), Some(right)) = (numeric_bounds(start), numeric_bounds(end)) else {
        return empty();
    };
    let left_prefix = &start[..left.start];
    let right_prefix = &end[..right.start];
    let left_suffix = &start[left.end..];
    let right_suffix = &end[right.end..];
    let prefix_equal = left_prefix == right_prefix;
    let suffix_equal = left_suffix == right_suffix;
    let (prefix, suffix) = match mode {
        Collapse::Numeric => (
            prefix_equal && affix_length(left_prefix) > 1,
            suffix_equal && affix_length(left_suffix) > 1,
        ),
        Collapse::Pattern => {
            let length = affix_length(left_prefix) + affix_length(left_suffix);
            if !prefix_equal || !suffix_equal || length <= 1 {
                return empty();
            }
            (true, true)
        }
        Collapse::Unit => (
            prefix_equal && (unit_affix(left_prefix) || affix_length(left_prefix) > 1),
            suffix_equal && (unit_affix(left_suffix) || affix_length(left_suffix) > 1),
        ),
    };
    // Remove suffixes first so the owned indices remain valid.
    let suffix = if suffix {
        let shared = end.split_off(right.end);
        start.truncate(left.end);
        shared
    } else {
        Vec::new()
    };
    let prefix = if prefix {
        let shared = start.drain(..left.start).collect();
        end.drain(..right.start);
        shared
    } else {
        Vec::new()
    };
    SharedAffixes { prefix, suffix }
}

fn unit_affix(parts: &[NumberPart]) -> bool {
    parts
        .iter()
        .any(|part| matches!(part.kind, "unit" | "currency"))
}

fn affix_length(parts: &[NumberPart]) -> usize {
    parts.iter().map(|part| part.value.chars().count()).sum()
}

fn numeric_bounds(parts: &[NumberPart]) -> Option<Range<usize>> {
    let numeric = |part: &NumberPart| {
        matches!(
            part.kind,
            "integer"
                | "group"
                | "decimal"
                | "fraction"
                | "nan"
                | "infinity"
                | "compact"
                | "exponentSeparator"
                | "exponentMinusSign"
                | "exponentInteger"
        )
    };
    let first = parts.iter().position(numeric)?;
    let last = parts.iter().rposition(numeric)?;
    Some(first..last + 1)
}

pub(super) fn needs_spacing(start: &[NumberPart], end: &[NumberPart]) -> bool {
    let digit = |part: &NumberPart| matches!(part.kind, "integer" | "fraction" | "exponentInteger");
    let notation = start
        .iter()
        .chain(end)
        .any(|part| matches!(part.kind, "compact" | "exponentSeparator"));
    notation || !start.last().is_some_and(digit) || !end.first().is_some_and(digit)
}
