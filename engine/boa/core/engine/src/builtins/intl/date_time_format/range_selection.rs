//! Range selection from typed calendar fields and owned locale pattern data.
use super::{
    range_data::{RangeData, split_glue, width_distance},
    range_fields::{Difference, RangeFields},
    range_pattern::{FieldId, IntervalPattern, PatternFields, Placeholder},
};
use icu_datetime::{
    pattern::DateTimePattern,
    provider::{
        fields::{FieldSymbol, Hour},
        pattern::{PatternItem, runtime::Pattern},
    },
};
use std::sync::Arc;

pub(super) type SharedPattern = Arc<Pattern<'static>>;

/// A small selected plan; it never clones the locale map or an ICU formatter.
#[derive(Debug)]
pub(super) enum RangePlan {
    Single(SharedPattern),
    Interval(IntervalPattern),
    Fallback {
        start: SharedPattern,
        end: SharedPattern,
        tokens: Vec<Placeholder>,
    },
    Combined {
        date: SharedPattern,
        time: Box<Self>,
        glue: Vec<Placeholder>,
    },
}

impl RangeData {
    pub(super) fn select(
        &self,
        start: SharedPattern,
        end: SharedPattern,
        first: &RangeFields,
        last: &RangeFields,
        difference: Difference,
        source_skeleton: Option<&str>,
    ) -> Result<RangePlan, String> {
        let fields = PatternFields::new(&start)?;
        if start != end {
            return Ok(self.fallback(start, end));
        }
        if missing_context(&fields, difference) {
            let date = self.context_date(&fields, difference, fields.date_mask() == 0)?;
            let augmented = if fields.time_mask() == 0 {
                date
            } else {
                let time = if fields.date_mask() == 0 {
                    (*start).clone()
                } else {
                    split_glue(&start, &self.glue)?.1
                };
                combine(&date, &time, &self.glue)?
            };
            return self.interval_or_fallback(Arc::new(augmented), difference, source_skeleton);
        }
        if fields.date_mask() != 0 && fields.time_mask() != 0 && first.same_date(last) {
            let (date, time) = split_glue(&start, &self.glue)?;
            return Ok(RangePlan::Combined {
                date: Arc::new(date),
                time: Box::new(self.interval_or_fallback(
                    Arc::new(time),
                    difference,
                    source_skeleton,
                )?),
                glue: self.glue.clone(),
            });
        }
        self.interval_or_fallback(start, difference, source_skeleton)
    }

    fn fallback(&self, start: SharedPattern, end: SharedPattern) -> RangePlan {
        RangePlan::Fallback {
            start,
            end,
            tokens: self.fallback.clone(),
        }
    }

    fn interval_or_fallback(
        &self,
        selected: SharedPattern,
        difference: Difference,
        source_skeleton: Option<&str>,
    ) -> Result<RangePlan, String> {
        let fields = PatternFields::new(&selected)?;
        let twelve_hour = fields.fields[FieldId::Hour as usize]
            .is_some_and(|field| matches!(field.symbol, FieldSymbol::Hour(Hour::H11 | Hour::H12)));
        let key = difference.key(twelve_hour);
        let implicit_period = if twelve_hour {
            FieldId::DayPeriod.bit()
        } else {
            0
        };
        if let Some(skeleton) = source_skeleton
            && matching_skeleton_fields(skeleton, &fields, implicit_period).is_some()
            && let Some(raw) = self.intervals.get().get(skeleton, key)
        {
            return self.interval(selected, &fields, raw);
        }
        let mut best = None;
        for skeleton in self.intervals.get().skeletons() {
            // a is implicit in CLDR h skeletons, whereas actual patterns retain
            // its typed field and the selected width for output and sources.
            let Some(candidate) = matching_skeleton_fields(skeleton, &fields, implicit_period)
            else {
                continue;
            };
            let Some(raw) = self.intervals.get().get(skeleton, key) else {
                continue;
            };
            let score = width_distance(&fields, &candidate);
            if best.as_ref().is_none_or(|(previous, _)| score < *previous) {
                best = Some((score, raw));
            }
        }
        let Some((_, raw)) = best else {
            return Ok(self.fallback(selected.clone(), selected));
        };
        self.interval(selected, &fields, raw)
    }

    fn interval(
        &self,
        selected: SharedPattern,
        fields: &PatternFields,
        raw: &str,
    ) -> Result<RangePlan, String> {
        // Unsupported source fields remain in the provider. A selected pattern
        // that cannot cover all native fields uses the locale's full fallback.
        let interval = IntervalPattern::parse(raw, self.fallback_reversed)?;
        match interval.adapt(fields) {
            Ok(interval) => Ok(RangePlan::Interval(interval)),
            Err(_) => Ok(self.fallback(selected.clone(), selected)),
        }
    }
}

fn matching_skeleton_fields(
    skeleton: &str,
    fields: &PatternFields,
    implicit_period: u16,
) -> Option<PatternFields> {
    let Ok(pattern) = DateTimePattern::try_from_pattern_str(skeleton) else {
        return None;
    };
    let pattern: Pattern<'static> = pattern.into();
    let Ok(candidate) = PatternFields::new(&pattern) else {
        return None;
    };
    (candidate.logical_mask() & !implicit_period == fields.logical_mask() & !implicit_period)
        .then_some(candidate)
}

fn missing_context(fields: &PatternFields, difference: Difference) -> bool {
    match difference {
        Difference::Era => !fields.has(FieldId::Era),
        Difference::Year => fields.logical_mask() & FieldId::Year.bit() == 0,
        Difference::Month => !fields.has(FieldId::Month),
        Difference::Day => !fields.has(FieldId::Day) && !fields.has(FieldId::Weekday),
        _ => false,
    }
}

fn combine(
    date: &Pattern<'_>,
    time: &Pattern<'_>,
    glue: &[Placeholder],
) -> Result<Pattern<'static>, String> {
    let mut items = Vec::new();
    let mut counts = [0; 2];
    for token in glue {
        match token {
            Placeholder::Literal(text) => items.extend(text.chars().map(PatternItem::Literal)),
            Placeholder::Value(index @ 0..=1) => {
                counts[*index as usize] += 1;
                items.extend(if *index == 0 { time } else { date }.items.iter());
            }
            _ => return Err("invalid date/time glue placeholder".into()),
        }
    }
    if counts != [1, 1] {
        return Err("invalid date/time glue coverage".into());
    }
    Ok(items.into_iter().collect())
}
