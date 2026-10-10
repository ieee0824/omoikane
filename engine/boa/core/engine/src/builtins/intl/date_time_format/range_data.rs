//! Owned CLDR interval payload and locale glue, loaded on the first range call.
use super::{
    calendar_patterns::load_calendar_patterns,
    calendar_period::load_glue,
    range_pattern::{FieldId, PatternFields, Placeholder},
};
use boa_intl_data::{
    CalendarDatePatterns, OmoikaneCalendarDatePatternsV1, OmoikaneCalendarIntervalPatternsV1,
};
use icu_datetime::{
    DateTimeFormatterPreferences,
    provider::{
        fields::{Day, Field, FieldLength, FieldSymbol, Month, Year},
        neo::marker_attrs::PatternLength,
        pattern::{GenericPatternItem, PatternItem, runtime::Pattern},
    },
};
use icu_provider::prelude::*;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(super) struct RangeData {
    pub(super) intervals: DataPayload<OmoikaneCalendarIntervalPatternsV1>,
    dates: DataPayload<OmoikaneCalendarDatePatternsV1>,
    pub(super) glue: Vec<Placeholder>,
    pub(super) fallback: Vec<Placeholder>,
    pub(super) fallback_reversed: bool,
    plans: Vec<PlanEntry>,
}

#[derive(Debug, Clone)]
struct PlanEntry {
    start: super::range_selection::SharedPattern,
    end: super::range_selection::SharedPattern,
    difference: super::range_fields::Difference,
    same_date: bool,
    plan: Arc<super::range_selection::RangePlan>,
}

impl RangeData {
    pub(super) fn load<P: BufferProvider + ?Sized>(
        provider: &P,
        prefs: DateTimeFormatterPreferences,
        calendar: &str,
        length: PatternLength,
    ) -> Result<Self, String> {
        let locale = OmoikaneCalendarIntervalPatternsV1::INFO.make_locale(prefs.locale_preferences);
        let attributes = DataMarkerAttributes::try_from_str(calendar)
            .map_err(|error| format!("interval calendar: {error:?}"))?;
        let intervals = DataProvider::<OmoikaneCalendarIntervalPatternsV1>::load(
            &provider.as_deserializing(),
            DataRequest {
                id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
                ..Default::default()
            },
        )
        .map_err(|error| format!("calendar interval data: {error:?}"))?
        .payload;
        let fallback = super::range_pattern::placeholders(
            intervals
                .get()
                .fallback()
                .ok_or("missing calendar interval fallback")?,
        )?;
        let fallback_reversed = matches!(
            fallback
                .iter()
                .find(|token| matches!(token, Placeholder::Value(_))),
            Some(Placeholder::Value(1))
        );
        let glue_data = load_glue(provider, prefs, length)?;
        let mut glue = Vec::new();
        let mut text = String::new();
        for item in glue_data.get().pattern.items.iter() {
            match item {
                GenericPatternItem::Literal(ch) => text.push(ch),
                GenericPatternItem::Placeholder(index) => {
                    if !text.is_empty() {
                        glue.push(Placeholder::Literal(core::mem::take(&mut text)));
                    }
                    glue.push(Placeholder::Value(index));
                }
            }
        }
        if !text.is_empty() {
            glue.push(Placeholder::Literal(text));
        }
        Ok(Self {
            intervals,
            dates: load_calendar_patterns(provider, prefs, calendar)?,
            glue,
            fallback,
            fallback_reversed,
            plans: Vec::new(),
        })
    }

    /// Cache only selected, small plans. The locale's zero-copy raw maps are
    /// never copied, and a repeated range does not parse all source skeletons.
    pub(super) fn plan(
        &mut self,
        start: super::range_selection::SharedPattern,
        end: super::range_selection::SharedPattern,
        first: &super::range_fields::RangeFields,
        last: &super::range_fields::RangeFields,
        difference: super::range_fields::Difference,
        source_skeleton: Option<&str>,
    ) -> Result<Arc<super::range_selection::RangePlan>, String> {
        let same_date = first.same_date(last);
        if let Some(entry) = self.plans.iter().find(|entry| {
            entry.start == start
                && entry.end == end
                && entry.difference == difference
                && entry.same_date == same_date
        }) {
            return Ok(entry.plan.clone());
        }
        let plan = Arc::new(self.select(
            start.clone(),
            end.clone(),
            first,
            last,
            difference,
            source_skeleton,
        )?);
        if self.plans.len() == 16 {
            self.plans.remove(0);
        }
        self.plans.push(PlanEntry {
            start,
            end,
            difference,
            same_date,
            plan: plan.clone(),
        });
        Ok(plan)
    }

    /// Chooses a real calendar pattern for additional context, then adjusts only
    /// explicitly resolved widths. No civil date order is hardcoded.
    pub(super) fn context_date(
        &self,
        selected: &PatternFields,
        missing: super::range_fields::Difference,
        time_only: bool,
    ) -> Result<Pattern<'static>, String> {
        use super::range_fields::Difference;
        let mut wanted = selected.clone();
        wanted.mask = selected.date_mask();
        for id in [
            FieldId::DayPeriod,
            FieldId::Hour,
            FieldId::Minute,
            FieldId::Second,
            FieldId::Fraction,
            FieldId::Zone,
        ] {
            wanted.fields[id as usize] = None;
        }
        let add = |wanted: &mut PatternFields, id: FieldId, field: Field| {
            if !wanted.has(id) {
                wanted.mask |= id.bit();
                wanted.fields[id as usize] = Some(field);
            }
        };
        if missing == Difference::Era {
            add(
                &mut wanted,
                FieldId::Era,
                Field {
                    symbol: FieldSymbol::Era,
                    length: FieldLength::Three,
                },
            );
        }
        if time_only || missing <= Difference::Year {
            if wanted.logical_mask() & FieldId::Year.bit() == 0 {
                add(
                    &mut wanted,
                    FieldId::Year,
                    Field {
                        symbol: FieldSymbol::Year(Year::Calendar),
                        length: FieldLength::One,
                    },
                );
            }
        }
        if time_only || missing <= Difference::Month {
            add(
                &mut wanted,
                FieldId::Month,
                Field {
                    symbol: FieldSymbol::Month(Month::Format),
                    length: FieldLength::One,
                },
            );
        }
        if time_only || missing <= Difference::Day {
            add(
                &mut wanted,
                FieldId::Day,
                Field {
                    symbol: FieldSymbol::Day(Day::DayOfMonth),
                    length: FieldLength::One,
                },
            );
        }
        choose_context(self.dates.get(), &wanted)
    }
}

fn choose_context(
    data: &CalendarDatePatterns<'_>,
    wanted: &PatternFields,
) -> Result<Pattern<'static>, String> {
    let mut best = None;
    for (_, raw) in data.available_formats.iter() {
        let Ok(profile) = super::match_fields::parse_profile(raw) else {
            continue;
        };
        let Some(pattern) = profile.pattern else {
            continue;
        };
        let Ok(fields) = PatternFields::new(&pattern) else {
            continue;
        };
        if fields.logical_mask() != wanted.logical_mask() {
            continue;
        }
        let score = width_distance(wanted, &fields);
        if best.as_ref().is_none_or(|(previous, _)| score < *previous) {
            best = Some((score, pattern));
        }
    }
    let (_, pattern) = best.ok_or("calendar has no pattern for required interval context")?;
    let adjusted = pattern
        .items
        .iter()
        .map(|item| match item {
            PatternItem::Field(mut field) => {
                if let Some(id) = FieldId::of(field.symbol) {
                    if let Some(actual) = wanted.fields[id as usize] {
                        field.length = actual.length;
                    }
                }
                PatternItem::Field(field)
            }
            item => item,
        })
        .collect();
    Ok(adjusted)
}

pub(super) fn width_distance(first: &PatternFields, second: &PatternFields) -> u32 {
    first
        .fields
        .iter()
        .zip(second.fields)
        .filter_map(|(first, second)| {
            (*first).zip(second).map(|(first, second)| {
                u32::from(width(first.length).abs_diff(width(second.length)))
            })
        })
        .sum()
}

fn width(length: FieldLength) -> u8 {
    match length {
        FieldLength::One | FieldLength::NumericOverride(_) => 1,
        FieldLength::Two => 2,
        FieldLength::Three => 3,
        FieldLength::Four => 4,
        FieldLength::Five => 5,
        FieldLength::Six => 6,
    }
}

/// Separates date/time patterns using the provider's actual glue literal tokens.
/// Only typed pattern items are inspected; localized rendered values are opaque.
pub(super) fn split_glue(
    pattern: &Pattern<'_>,
    glue: &[Placeholder],
) -> Result<(Pattern<'static>, Pattern<'static>), String> {
    let values: Vec<_> = glue
        .iter()
        .filter_map(|item| match item {
            Placeholder::Value(index) => Some(*index),
            _ => None,
        })
        .collect();
    if values.len() != 2 || values[0] == values[1] {
        return Err("invalid date/time glue".into());
    }
    let first = glue
        .iter()
        .position(|item| matches!(item, Placeholder::Value(_)))
        .unwrap();
    let last = glue
        .iter()
        .rposition(|item| matches!(item, Placeholder::Value(_)))
        .unwrap();
    let literals = |tokens: &[Placeholder]| -> Vec<PatternItem> {
        tokens
            .iter()
            .flat_map(|token| match token {
                Placeholder::Literal(text) => text.chars().map(PatternItem::Literal).collect(),
                _ => Vec::new(),
            })
            .collect()
    };
    let prefix = literals(&glue[..first]);
    let middle = literals(&glue[first + 1..last]);
    let suffix = literals(&glue[last + 1..]);
    let items: Vec<_> = pattern.items.iter().collect();
    if !items.starts_with(&prefix) || !items.ends_with(&suffix) {
        return Err("selected date/time glue differs".into());
    }
    let date_first = values[0] == 1;
    let mut boundary = None;
    for index in prefix.len()..items.len().saturating_sub(suffix.len()) {
        let PatternItem::Field(field) = items[index] else {
            continue;
        };
        let id = FieldId::of(field.symbol).ok_or("unsupported date/time field")?;
        if id.is_date() != date_first {
            boundary = Some(index);
            break;
        }
    }
    let right_field = boundary.ok_or("selected pattern has no second component")?;
    let left_field = (prefix.len()..right_field)
        .rfind(|&index| matches!(items[index], PatternItem::Field(_)))
        .ok_or("selected pattern has no first component")?;
    let gap = &items[left_field + 1..right_field];
    let split = if middle.is_empty() {
        gap.len()
    } else {
        gap.windows(middle.len())
            .rposition(|value| value == middle)
            .ok_or("locale glue separator not found")?
    } + left_field
        + 1;
    let left = items[prefix.len()..split].iter().copied().collect();
    let right = items[split + middle.len()..items.len() - suffix.len()]
        .iter()
        .copied()
        .collect();
    Ok(if date_first {
        (left, right)
    } else {
        (right, left)
    })
}
