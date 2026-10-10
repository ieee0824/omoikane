//! Pure conversion of selected ICU pattern fields into ECMA-402 option values.
use super::matcher::MatchComponents;
use icu_datetime::{
    pattern::DateTimePattern,
    provider::{
        fields::{Day, FieldLength, FieldSymbol, Hour, Second, Year},
        pattern::{PatternItem, runtime::Pattern},
    },
};

#[derive(Debug, Default)]
pub(super) struct ResolvedPattern {
    pub(super) fields: Vec<(&'static str, &'static str)>,
    pub(super) hour_cycle: Option<&'static str>,
    pub(super) fractional_digits: Option<u8>,
}

impl ResolvedPattern {
    /// Basic formats expose the fields that actually won, without rewriting
    /// their widths to match the requested options.
    pub(super) fn from_components(fields: MatchComponents, hour_cycle: &'static str) -> Self {
        let mut result = Self {
            fields: fields
                .width_fields()
                .into_iter()
                .filter_map(|(name, width)| width.map(|width| (name, width.as_str())))
                .collect(),
            hour_cycle: fields.hour.map(|_| hour_cycle),
            fractional_digits: fields.fractional_digits,
        };
        if let Some(zone) = fields.zone_name {
            result.fields.push(("timeZoneName", zone.as_str()));
        }
        result
    }

    pub(super) fn from_pattern(pattern: DateTimePattern) -> Self {
        let pattern: Pattern<'static> = pattern.into();
        let mut result = Self::default();
        for item in pattern.items.iter() {
            let PatternItem::Field(field) = item else {
                continue;
            };
            let numeric = if field.length == FieldLength::Two {
                "2-digit"
            } else {
                "numeric"
            };
            let text = match field.length {
                FieldLength::Four => "long",
                FieldLength::Five => "narrow",
                _ => "short",
            };
            let pair = match field.symbol {
                FieldSymbol::Era => Some(("era", text)),
                FieldSymbol::Year(Year::Calendar | Year::RelatedIso) => Some(("year", numeric)),
                FieldSymbol::Year(Year::Cyclic) => Some(("year", "numeric")),
                FieldSymbol::Month(_) => Some((
                    "month",
                    if matches!(field.length, FieldLength::One | FieldLength::Two) {
                        numeric
                    } else {
                        text
                    },
                )),
                FieldSymbol::Day(Day::DayOfMonth) => Some(("day", numeric)),
                FieldSymbol::Weekday(_) => Some(("weekday", text)),
                FieldSymbol::DayPeriod(_) => Some(("dayPeriod", text)),
                FieldSymbol::Hour(hour) => {
                    result.hour_cycle = Some(match hour {
                        Hour::H11 => "h11",
                        Hour::H12 => "h12",
                        Hour::H23 => "h23",
                    });
                    Some(("hour", numeric))
                }
                FieldSymbol::Minute => Some(("minute", numeric)),
                FieldSymbol::Second(Second::Second) => Some(("second", numeric)),
                FieldSymbol::DecimalSecond(digits) => {
                    result.fractional_digits = Some(digits as u8);
                    Some(("second", numeric))
                }
                _ => None,
            };
            if let Some((name, value)) = pair {
                if let Some(existing) = result
                    .fields
                    .iter_mut()
                    .find(|(existing, _)| existing == &name)
                {
                    existing.1 = value;
                } else {
                    result.fields.push((name, value));
                }
            }
        }
        result
    }
}
