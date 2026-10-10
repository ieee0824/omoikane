//! Endpoint ownership from typed interval pattern spans, never string equality.
use super::{
    parts::{DateTimePart, PartKind},
    range_pattern::{FieldId, Placeholder},
    range_selection::RangePlan,
};
use icu_datetime::provider::pattern::runtime::Pattern;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RangeSource {
    Start,
    End,
    Shared,
}

impl RangeSource {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Start => "startRange",
            Self::End => "endRange",
            Self::Shared => "shared",
        }
    }
}

#[derive(Debug)]
pub(super) struct RangePart {
    pub(super) part: DateTimePart,
    pub(super) source: RangeSource,
}

pub(super) fn render<F>(plan: &RangePlan, format: &mut F) -> Result<Vec<RangePart>, String>
where
    F: FnMut(&Pattern<'static>, bool) -> Result<Vec<DateTimePart>, String>,
{
    match plan {
        RangePlan::Single(pattern) => Ok(tag(format(pattern, false)?, RangeSource::Shared)),
        RangePlan::Fallback { start, end, tokens } => {
            let first = tag(format(start, false)?, RangeSource::Start);
            let last = tag(format(end, true)?, RangeSource::End);
            combine(first, last, tokens)
        }
        RangePlan::Interval(interval) => {
            let first = format(&interval.first, interval.first_is_end)?;
            let second = format(&interval.second, !interval.first_is_end)?;
            let mut parts = endpoint_spans(
                first,
                interval.repeated,
                if interval.first_is_end {
                    RangeSource::End
                } else {
                    RangeSource::Start
                },
            );
            parts.extend(endpoint_spans(
                second,
                interval.repeated,
                if interval.first_is_end {
                    RangeSource::Start
                } else {
                    RangeSource::End
                },
            ));
            Ok(coalesce(parts))
        }
        RangePlan::Combined { date, time, glue } => {
            let date = tag(format(date, false)?, RangeSource::Shared);
            let time = render(time, format)?;
            // CLDR date/time glue has {0}=time and {1}=date.
            combine(time, date, glue)
        }
    }
}

fn endpoint_spans(parts: Vec<DateTimePart>, repeated: u16, source: RangeSource) -> Vec<RangePart> {
    let is_endpoint =
        |part: &DateTimePart| kind_id(part.kind).is_some_and(|id| repeated & id.bit() != 0);
    let first = parts.iter().position(is_endpoint);
    let last = parts.iter().rposition(is_endpoint);
    parts
        .into_iter()
        .enumerate()
        .map(|(index, part)| RangePart {
            part,
            source: if first
                .zip(last)
                .is_some_and(|(first, last)| (first..=last).contains(&index))
            {
                source
            } else {
                RangeSource::Shared
            },
        })
        .collect()
}

fn kind_id(kind: PartKind) -> Option<FieldId> {
    Some(match kind {
        PartKind::Era => FieldId::Era,
        PartKind::Year => FieldId::Year,
        PartKind::RelatedYear => FieldId::RelatedYear,
        PartKind::YearName => FieldId::YearName,
        PartKind::Month => FieldId::Month,
        PartKind::Day => FieldId::Day,
        PartKind::Weekday => FieldId::Weekday,
        PartKind::Hour => FieldId::Hour,
        PartKind::Minute => FieldId::Minute,
        PartKind::Second => FieldId::Second,
        PartKind::FractionalSecond => FieldId::Fraction,
        PartKind::DayPeriod => FieldId::DayPeriod,
        PartKind::TimeZoneName => FieldId::Zone,
        PartKind::Literal => return None,
    })
}

fn tag(parts: Vec<DateTimePart>, source: RangeSource) -> Vec<RangePart> {
    parts
        .into_iter()
        .map(|part| RangePart { part, source })
        .collect()
}

fn combine(
    first: Vec<RangePart>,
    second: Vec<RangePart>,
    tokens: &[Placeholder],
) -> Result<Vec<RangePart>, String> {
    let mut values = [Some(first), Some(second)];
    let mut parts = Vec::new();
    for token in tokens {
        match token {
            Placeholder::Literal(text) => parts.push(RangePart {
                part: DateTimePart {
                    kind: PartKind::Literal,
                    value: text.clone(),
                },
                source: RangeSource::Shared,
            }),
            Placeholder::Value(index @ 0..=1) => parts.extend(
                values[*index as usize]
                    .take()
                    .ok_or("duplicate range placeholder")?,
            ),
            _ => return Err("invalid range placeholder".into()),
        }
    }
    if values.iter().any(Option::is_some) {
        return Err("missing range placeholder".into());
    }
    Ok(coalesce(parts))
}

fn coalesce(parts: Vec<RangePart>) -> Vec<RangePart> {
    let mut result: Vec<RangePart> = Vec::new();
    for part in parts {
        if part.part.kind == PartKind::Literal {
            if let Some(previous) = result.last_mut() {
                if previous.part.kind == PartKind::Literal && previous.source == part.source {
                    previous.part.value.push_str(&part.part.value);
                    continue;
                }
            }
        }
        result.push(part);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::intl::date_time_format::range_pattern::{IntervalPattern, placeholders};
    use icu_datetime::pattern::DateTimePattern;
    use std::sync::Arc;

    fn part(kind: PartKind, value: &str) -> DateTimePart {
        DateTimePart {
            kind,
            value: value.to_owned(),
        }
    }

    #[test]
    fn endpoint_literals_follow_spans_and_identical_values_keep_both_sources() {
        let input = vec![
            part(PartKind::Month, "M"),
            part(PartKind::Literal, " "),
            part(PartKind::Day, "1"),
            part(PartKind::Literal, " / "),
            part(PartKind::Hour, "1"),
            part(PartKind::Literal, " – "),
        ];
        let result = endpoint_spans(
            input,
            FieldId::Day.bit() | FieldId::Hour.bit(),
            RangeSource::Start,
        );
        assert_eq!(
            result.iter().map(|part| part.source).collect::<Vec<_>>(),
            [
                RangeSource::Shared,
                RangeSource::Shared,
                RangeSource::Start,
                RangeSource::Start,
                RangeSource::Start,
                RangeSource::Shared
            ]
        );
        let mut input = tag(vec![part(PartKind::Day, "1")], RangeSource::Start);
        input.extend(tag(vec![part(PartKind::Day, "1")], RangeSource::End));
        assert_eq!(coalesce(input).len(), 2);
    }

    #[test]
    fn reversed_fallback_and_explicit_latest_first_preserve_argument_identity() {
        let pattern: Arc<Pattern<'static>> =
            Arc::new(DateTimePattern::try_from_pattern_str("d").unwrap().into());
        let fallback = RangePlan::Fallback {
            start: pattern.clone(),
            end: pattern,
            tokens: placeholders("prefix {1} - {0} suffix").unwrap(),
        };
        let result = render(&fallback, &mut |_, end| {
            Ok(vec![part(PartKind::Day, if end { "2" } else { "1" })])
        })
        .unwrap();
        assert_eq!(
            result
                .iter()
                .map(|part| (part.part.value.as_str(), part.source))
                .collect::<Vec<_>>(),
            [
                ("prefix ", RangeSource::Shared),
                ("2", RangeSource::End),
                (" - ", RangeSource::Shared),
                ("1", RangeSource::Start),
                (" suffix", RangeSource::Shared)
            ]
        );
        let interval =
            RangePlan::Interval(IntervalPattern::parse("latestFirst:d–d", false).unwrap());
        let result = render(&interval, &mut |pattern, end| {
            let mut result = Vec::new();
            for item in pattern.items.iter() {
                result.push(match item {
                    icu_datetime::provider::pattern::PatternItem::Field(_) => {
                        part(PartKind::Day, if end { "2" } else { "1" })
                    }
                    icu_datetime::provider::pattern::PatternItem::Literal(ch) => {
                        part(PartKind::Literal, &ch.to_string())
                    }
                });
            }
            Ok(result)
        })
        .unwrap();
        assert_eq!(
            result
                .iter()
                .map(|part| (part.part.value.as_str(), part.source))
                .collect::<Vec<_>>(),
            [
                ("2", RangeSource::End),
                ("–", RangeSource::Shared),
                ("1", RangeSource::Start)
            ]
        );
    }
}
