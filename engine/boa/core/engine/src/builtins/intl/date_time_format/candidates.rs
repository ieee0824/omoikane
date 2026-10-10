//! Constructor-local candidate matching; only the winning combination is built.
use super::{
    calendar_patterns::load_calendar_patterns,
    calendar_period::load_glue,
    hour_cycle::DateTimeHourCycle,
    match_fields::{FieldError, PatternProfile, components_from_pattern, parse_profile},
    matcher::{MatchComponents, Width, basic_score},
    pattern::ordinary_day_period_spaces,
};
use icu_datetime::{
    DateTimeFormatterPreferences,
    pattern::DateTimePattern,
    provider::{
        fields::{DecimalSecond, FieldSymbol, Hour, Second, Year},
        neo::marker_attrs::PatternLength,
        pattern::{PatternItem, runtime::Pattern},
    },
};
use icu_provider::prelude::*;
use std::sync::Arc;

/// A persistent selected record owns its pattern and source keys. Provider and
/// candidate borrows end at construction; formatting does not rescan the data.
#[derive(Debug, Clone)]
pub(super) struct SelectedFormat {
    pattern: Arc<DateTimePattern>,
    range_pattern: Arc<Pattern<'static>>,
    pub(super) related_year: bool,
    pub(super) fields: MatchComponents,
    // Range matching uses the CLDR source identity before rendered widths.
    interval_skeleton: String,
    pub(super) hour_cycle: DateTimeHourCycle,
}

#[derive(Debug)]
struct Candidate {
    key: String,
    profile: PatternProfile,
}

#[derive(Debug, Clone, Copy)]
struct Choice {
    first: usize,
    second: Option<usize>,
    fraction: Option<u8>,
    fields: MatchComponents,
    score: i16,
}

impl SelectedFormat {
    pub(super) fn load<P: BufferProvider + ?Sized>(
        provider: &P,
        prefs: DateTimeFormatterPreferences,
        calendar: &str,
        request: MatchComponents,
        hour_cycle: DateTimeHourCycle,
    ) -> Result<Self, String> {
        if let Some(selected) = Self::day_period_only(request, hour_cycle)? {
            return Ok(selected);
        }
        let data = load_calendar_patterns(provider, prefs, calendar)?;
        let candidates = read_candidates(data.get().available_formats.iter(), hour_cycle)?;
        let choice = select(request, &candidates)
            .ok_or_else(|| "calendar data contains no ECMA date/time format records".to_owned())?;
        let pattern = materialize(provider, prefs, &candidates, choice, hour_cycle)?;
        let mut fields = components_from_pattern(&pattern)
            .map_err(|error| format!("selected date/time metadata: {error:?}"))?;
        // The zone value is formatted independently from the surrounding CLDR
        // date/time pattern. Keep it in the selected Format Record so
        // resolvedOptions and BasicFormatMatcher observe the requested member.
        fields.zone_name = choice.fields.zone_name;
        if fields != choice.fields {
            return Err("selected pattern fields differ from matched metadata".into());
        }
        let first = &candidates[choice.first];
        let source_date_skeleton = first
            .profile
            .fields
            .has_date()
            .then_some(first.key.as_str());
        let source_time_skeleton = choice
            .second
            .map(|index| candidates[index].key.as_str())
            .or_else(|| {
                first
                    .profile
                    .fields
                    .has_time()
                    .then_some(first.key.as_str())
            });
        let interval_skeleton = match (source_date_skeleton, source_time_skeleton) {
            (Some(date), None) => date.to_owned(),
            (None, Some(time)) => time.to_owned(),
            _ => semantic_skeleton(fields, hour_cycle),
        };
        let related_year = pattern.items.iter().any(|item| {
            matches!(item, PatternItem::Field(field) if field.symbol == FieldSymbol::Year(Year::RelatedIso))
        });
        Ok(Self {
            interval_skeleton,
            range_pattern: Arc::new(pattern.clone()),
            pattern: Arc::new(pattern.into()),
            related_year,
            fields,
            hour_cycle,
        })
    }

    /// ICU 2.0 cannot parse CLDR's flexible `B` field. A lone day-period
    /// request needs no locale-dependent field ordering, so use ICU's native
    /// noon/midnight `b` field while retaining the requested `B` metadata.
    fn day_period_only(
        request: MatchComponents,
        hour_cycle: DateTimeHourCycle,
    ) -> Result<Option<Self>, String> {
        let Some(width) = request.day_period else {
            return Ok(None);
        };
        let remainder = MatchComponents {
            day_period: None,
            zone_name: None,
            ..request
        };
        if remainder != MatchComponents::default() {
            return Ok(None);
        }

        let native = "b".repeat(width.length());
        let pattern: Pattern<'static> = DateTimePattern::try_from_pattern_str(&native)
            .map_err(|error| format!("day-period pattern: {error:?}"))?
            .into();
        Ok(Some(Self {
            pattern: Arc::new(pattern.clone().into()),
            range_pattern: Arc::new(pattern),
            related_year: false,
            fields: request,
            interval_skeleton: "B".repeat(width.length()),
            hour_cycle,
        }))
    }

    /// Borrows the immutable selected pattern; cloning formatter state only
    /// clones its owned shared handle, never its pattern-item buffer.
    pub(super) fn pattern(&self) -> &DateTimePattern {
        &self.pattern
    }

    /// Shares typed interval-selection fields without copying pattern buffers
    /// or reparsing a serialized pattern during each range operation.
    pub(super) fn range_pattern(&self) -> Arc<Pattern<'static>> {
        Arc::clone(&self.range_pattern)
    }

    /// Returns the CLDR identity that produced the selected single-date pattern.
    /// Separate date/time candidates have no combined source key, so their
    /// resolved semantic skeleton is the closest stable interval identity.
    pub(super) fn interval_skeleton(&self) -> &str {
        &self.interval_skeleton
    }
}

fn read_candidates<'a>(
    raw: impl Iterator<Item = (&'a str, &'a str)>,
    hour_cycle: DateTimeHourCycle,
) -> Result<Vec<Candidate>, String> {
    let twelve_hour = matches!(hour_cycle, DateTimeHourCycle::H11 | DateTimeHourCycle::H12);
    let mut candidates = Vec::new();
    for (key, raw) in raw {
        let profile = match parse_profile(raw) {
            Ok(profile) => profile,
            Err(FieldError::NonEcmaRecord) => continue,
            Err(FieldError::Invalid(error)) => {
                return Err(format!("CLDR available format {key}: {error}"));
            }
        };
        // pattern / pattern12 families have separate locale literals and widths.
        // Pick the effective family before scoring, preserving its actual fields.
        if profile
            .twelve_hour
            .is_some_and(|family| family != twelve_hour)
        {
            continue;
        }
        // Zone values are rendered by the backend's locale-aware offset writer.
        // Select the surrounding date/time record independently so an absent
        // zone skeleton cannot turn a valid ECMA-402 option into a RangeError.
        if profile.fields.zone_name.is_some() {
            continue;
        }
        candidates.push(Candidate {
            key: key.to_owned(),
            profile,
        });
    }
    Ok(candidates)
}

fn variants(fields: MatchComponents) -> impl Iterator<Item = (MatchComponents, Option<u8>)> {
    let base = std::iter::once((fields, fields.fractional_digits));
    let fractions = (1..=3)
        .filter(move |_| fields.second.is_some() && fields.fractional_digits.is_none())
        .map(move |digits| {
            (
                MatchComponents {
                    fractional_digits: Some(digits),
                    ..fields
                },
                Some(digits),
            )
        });
    base.chain(fractions)
}

fn consider(best: &mut Option<Choice>, candidate: Choice) {
    if best.is_none_or(|best| candidate.score > best.score) {
        *best = Some(candidate);
    }
}

fn select(request: MatchComponents, candidates: &[Candidate]) -> Option<Choice> {
    let mut best = None;
    let mut date = None;
    let mut time = None;
    for (index, candidate) in candidates.iter().enumerate() {
        for (base_fields, fraction) in variants(candidate.profile.fields) {
            let fields = MatchComponents {
                zone_name: request.zone_name,
                ..base_fields
            };
            let choice = Choice {
                first: index,
                second: None,
                fraction,
                fields,
                score: basic_score(request, fields),
            };
            consider(&mut best, choice);
            if base_fields.has_date() && !base_fields.has_time() {
                consider(
                    &mut date,
                    Choice {
                        score: basic_score(request.date_only(), fields),
                        ..choice
                    },
                );
            }
            if base_fields.has_time() && !base_fields.has_date() {
                consider(
                    &mut time,
                    Choice {
                        score: basic_score(request.time_only(), fields),
                        ..choice
                    },
                );
            }
        }
    }
    // All 11 penalties are additive. Date-only and time-only records are
    // disjoint, so independently maximizing their scores exactly selects the
    // first best pair of the ordered Cartesian list, without an O(n²) scan.
    if let (Some(date), Some(time)) = (date, time) {
        let fields = date.fields.with_time(time.fields);
        consider(
            &mut best,
            Choice {
                first: date.first,
                second: Some(time.first),
                fraction: time.fraction,
                fields,
                score: basic_score(request, fields),
            },
        );
    }
    best
}

fn materialize<P: BufferProvider + ?Sized>(
    provider: &P,
    prefs: DateTimeFormatterPreferences,
    candidates: &[Candidate],
    choice: Choice,
    hour_cycle: DateTimeHourCycle,
) -> Result<Pattern<'static>, String> {
    let first = owned_pattern(&candidates[choice.first])?;
    let combined = if let Some(second) = choice.second {
        let time = owned_pattern(&candidates[second])?;
        let length = match choice.fields.month {
            Some(Width::Long | Width::Narrow) => PatternLength::Long,
            Some(Width::Short) => PatternLength::Medium,
            _ => PatternLength::Short,
        };
        load_glue(provider, prefs, length)?
            .get()
            .pattern
            .clone()
            .combined(first, time)
            .map_err(|error| format!("date/time combination: {error:?}"))?
    } else {
        first
    };
    let adjusted = adapt_clock(combined, choice.fraction, hour_cycle);
    Ok(ordinary_day_period_spaces(adjusted.into()).into())
}

fn owned_pattern(candidate: &Candidate) -> Result<Pattern<'static>, String> {
    candidate
        .profile
        .pattern
        .clone()
        .ok_or_else(|| "selected CLDR pattern is not representable by ICU".into())
}

fn adapt_clock(
    pattern: Pattern<'static>,
    fraction: Option<u8>,
    cycle: DateTimeHourCycle,
) -> Pattern<'static> {
    pattern
        .items
        .iter()
        .map(|item| {
            let PatternItem::Field(mut field) = item else {
                return item;
            };
            match field.symbol {
                FieldSymbol::Hour(_) => {
                    field.symbol = FieldSymbol::Hour(match cycle {
                        DateTimeHourCycle::H11 => Hour::H11,
                        DateTimeHourCycle::H12 => Hour::H12,
                        DateTimeHourCycle::H23 | DateTimeHourCycle::H24 => Hour::H23,
                    })
                }
                FieldSymbol::Second(Second::Second) if fraction.is_some() => {
                    field.symbol =
                        FieldSymbol::DecimalSecond(match fraction.expect("checked digits") {
                            1 => DecimalSecond::Subsecond1,
                            2 => DecimalSecond::Subsecond2,
                            3 => DecimalSecond::Subsecond3,
                            _ => unreachable!("validated fraction width"),
                        });
                }
                _ => {}
            }
            PatternItem::Field(field)
        })
        .collect()
}

fn semantic_skeleton(fields: MatchComponents, cycle: DateTimeHourCycle) -> String {
    let mut result = String::new();
    let hour = match cycle {
        DateTimeHourCycle::H11 => 'K',
        DateTimeHourCycle::H12 => 'h',
        DateTimeHourCycle::H23 | DateTimeHourCycle::H24 => 'H',
    };
    for (symbol, width) in [
        ('G', fields.era),
        ('y', fields.year),
        ('M', fields.month),
        ('E', fields.weekday),
        ('d', fields.day),
        ('B', fields.day_period),
        (hour, fields.hour),
        ('m', fields.minute),
        ('s', fields.second),
    ] {
        if let Some(width) = width {
            result.extend(std::iter::repeat_n(symbol, width.length()));
        }
    }
    if let Some(digits) = fields.fractional_digits {
        result.extend(std::iter::repeat_n('S', usize::from(digits)));
    }
    // The zone is formatted independently by `ZoneWriter`, so interval lookup
    // must use only the fields represented by the ICU date/time pattern.
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates() -> Vec<Candidate> {
        read_candidates(
            [
                ("date1", "MMMM d, y"),
                ("date2", "MMMM d 'different' y"),
                ("clock", "HH:mm:ss"),
                ("minutes", "mm:ss"),
            ]
            .into_iter(),
            DateTimeHourCycle::H23,
        )
        .unwrap()
    }

    #[test]
    fn selected_widths_and_first_tie_are_kept_without_request_rewrite() {
        let request = MatchComponents {
            year: Some(Width::Numeric),
            month: Some(Width::Short),
            day: Some(Width::Numeric),
            ..Default::default()
        };
        let choice = select(request, &candidates()).unwrap();
        assert_eq!(choice.first, 0);
        assert_eq!(choice.second, None);
        assert_eq!(choice.fields.month, Some(Width::Long));
        assert_eq!(choice.score, -3);
    }

    #[test]
    fn linear_pair_selection_matches_exhaustive_metadata_reference() {
        let candidates = candidates();
        let request = MatchComponents {
            year: Some(Width::Numeric),
            month: Some(Width::Short),
            day: Some(Width::Numeric),
            minute: Some(Width::Numeric),
            second: Some(Width::Numeric),
            fractional_digits: Some(2),
            ..Default::default()
        };
        let selected = select(request, &candidates).unwrap();
        let mut exhaustive = None;
        for (index, candidate) in candidates.iter().enumerate() {
            for (fields, fraction) in variants(candidate.profile.fields) {
                consider(
                    &mut exhaustive,
                    Choice {
                        first: index,
                        second: None,
                        fraction,
                        fields,
                        score: basic_score(request, fields),
                    },
                );
            }
        }
        for (date, d) in candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.profile.fields.has_date() && !c.profile.fields.has_time())
        {
            for (time, t) in candidates
                .iter()
                .enumerate()
                .filter(|(_, c)| !c.profile.fields.has_date() && c.profile.fields.has_time())
            {
                for (clock, fraction) in variants(t.profile.fields) {
                    let fields = d.profile.fields.with_time(clock);
                    consider(
                        &mut exhaustive,
                        Choice {
                            first: date,
                            second: Some(time),
                            fraction,
                            fields,
                            score: basic_score(request, fields),
                        },
                    );
                }
            }
        }
        let reference = exhaustive.unwrap();
        assert_eq!(
            (
                selected.first,
                selected.second,
                selected.fraction,
                selected.fields,
                selected.score
            ),
            (
                reference.first,
                reference.second,
                reference.fraction,
                reference.fields,
                reference.score
            )
        );
        assert_eq!(selected.second, Some(3));
    }

    #[test]
    fn fraction_adaptation_and_hour_cycle_recompute_typed_metadata() {
        let profile = parse_profile("h:mm:ss a").unwrap();
        let pattern = adapt_clock(profile.pattern.unwrap(), Some(3), DateTimeHourCycle::H11);
        let fields = components_from_pattern(&pattern).unwrap();
        assert_eq!(fields.hour, Some(Width::Numeric));
        assert_eq!(fields.second, Some(Width::TwoDigit));
        assert_eq!(fields.fractional_digits, Some(3));
        assert_eq!(fields.day_period, None);
        assert_eq!(
            semantic_skeleton(fields, DateTimeHourCycle::H11),
            "KmmssSSS"
        );
    }
}
