//! Pure ECMA-402 BasicFormatMatcher scoring of eleven owned component values.
use super::{options::Components, pattern::ComponentWidths};
use icu_datetime::provider::fields::FieldLength;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum FormatMatcher {
    Basic,
    #[default]
    BestFit,
}

/// The order is the BasicFormatMatcher value list, not display length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Width {
    TwoDigit = 0,
    Numeric = 1,
    Narrow = 2,
    Short = 3,
    Long = 4,
}

impl Width {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::TwoDigit => "2-digit",
            Self::Numeric => "numeric",
            Self::Narrow => "narrow",
            Self::Short => "short",
            Self::Long => "long",
        }
    }

    pub(super) const fn length(self) -> usize {
        match self {
            Self::Numeric => 1,
            Self::TwoDigit => 2,
            Self::Short => 3,
            Self::Long => 4,
            Self::Narrow => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ZoneName {
    Short,
    Long,
    ShortOffset,
    LongOffset,
    ShortGeneric,
    LongGeneric,
}

impl ZoneName {
    pub(super) fn from_str(value: &str) -> Option<Self> {
        match value {
            "short" => Some(Self::Short),
            "long" => Some(Self::Long),
            "shortOffset" => Some(Self::ShortOffset),
            "longOffset" => Some(Self::LongOffset),
            "shortGeneric" => Some(Self::ShortGeneric),
            "longGeneric" => Some(Self::LongGeneric),
            _ => None,
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Long => "long",
            Self::ShortOffset => "shortOffset",
            Self::LongOffset => "longOffset",
            Self::ShortGeneric => "shortGeneric",
            Self::LongGeneric => "longGeneric",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct MatchComponents {
    pub(super) weekday: Option<Width>,
    pub(super) era: Option<Width>,
    pub(super) year: Option<Width>,
    pub(super) month: Option<Width>,
    pub(super) day: Option<Width>,
    pub(super) day_period: Option<Width>,
    pub(super) hour: Option<Width>,
    pub(super) minute: Option<Width>,
    pub(super) second: Option<Width>,
    pub(super) fractional_digits: Option<u8>,
    pub(super) zone_name: Option<ZoneName>,
}

impl MatchComponents {
    pub(super) fn requested(components: &Components) -> Self {
        let widths = components.widths();
        Self::from_widths(
            widths,
            components.fractional_digits,
            components
                .zone_name
                .as_ref()
                .and_then(|value| ZoneName::from_str(&value.to_std_string_escaped())),
        )
    }

    fn from_widths(
        widths: ComponentWidths,
        fractional_digits: Option<u8>,
        zone_name: Option<ZoneName>,
    ) -> Self {
        let width = |value: Option<FieldLength>| {
            value.map(|value| match value {
                FieldLength::One | FieldLength::NumericOverride(_) => Width::Numeric,
                FieldLength::Two => Width::TwoDigit,
                FieldLength::Three | FieldLength::Six => Width::Short,
                FieldLength::Four => Width::Long,
                FieldLength::Five => Width::Narrow,
            })
        };
        Self {
            weekday: width(widths.weekday),
            era: width(widths.era),
            year: width(widths.year),
            month: width(widths.month),
            day: width(widths.day),
            day_period: width(widths.day_period),
            hour: width(widths.hour),
            minute: width(widths.minute),
            second: width(widths.second),
            fractional_digits,
            zone_name,
        }
    }

    pub(super) fn width_fields(self) -> [(&'static str, Option<Width>); 9] {
        [
            ("weekday", self.weekday),
            ("era", self.era),
            ("year", self.year),
            ("month", self.month),
            ("day", self.day),
            ("dayPeriod", self.day_period),
            ("hour", self.hour),
            ("minute", self.minute),
            ("second", self.second),
        ]
    }

    pub(super) fn has_date(self) -> bool {
        self.weekday.is_some()
            || self.era.is_some()
            || self.year.is_some()
            || self.month.is_some()
            || self.day.is_some()
    }

    pub(super) fn has_time(self) -> bool {
        self.day_period.is_some()
            || self.hour.is_some()
            || self.minute.is_some()
            || self.second.is_some()
            || self.fractional_digits.is_some()
            || self.zone_name.is_some()
    }

    pub(super) fn date_only(self) -> Self {
        Self {
            weekday: self.weekday,
            era: self.era,
            year: self.year,
            month: self.month,
            day: self.day,
            ..Self::default()
        }
    }

    pub(super) fn time_only(self) -> Self {
        Self {
            day_period: self.day_period,
            hour: self.hour,
            minute: self.minute,
            second: self.second,
            fractional_digits: self.fractional_digits,
            zone_name: self.zone_name,
            ..Self::default()
        }
    }

    /// Combines disjoint date/time record fields without constructing a pattern.
    pub(super) fn with_time(self, time: Self) -> Self {
        Self {
            day_period: time.day_period,
            hour: time.hour,
            minute: time.minute,
            second: time.second,
            fractional_digits: time.fractional_digits,
            zone_name: time.zone_name,
            ..self
        }
    }
}

/// A positive penalty, subtracted from the candidate score.
fn value_penalty(request: Option<i16>, format: Option<i16>) -> i16 {
    match (request, format) {
        (None, Some(_)) => 20,
        (Some(_), None) => 120,
        (Some(request), Some(format)) => match (format - request).clamp(-2, 2) {
            -2 => 8,
            -1 => 6,
            1 => 3,
            2 => 6,
            _ => 0,
        },
        (None, None) => 0,
    }
}

pub(super) fn zone_penalty(request: ZoneName, format: ZoneName) -> i16 {
    use ZoneName::*;
    match (request, format) {
        (request, format) if request == format => 0,
        (Short | ShortGeneric, ShortOffset) | (Long | LongGeneric, LongOffset) => 1,
        (Short | ShortGeneric, LongOffset) => 4,
        (Long | LongGeneric, ShortOffset) => 9,
        (Short, Long) | (ShortGeneric, LongGeneric) | (ShortOffset, LongOffset) => 3,
        (Long, Short) | (LongGeneric, ShortGeneric) | (LongOffset, ShortOffset) => 8,
        _ => 120,
    }
}

/// Side-effect-free scoring in the specification's eleven-property order.
pub(super) fn basic_score(request: MatchComponents, format: MatchComponents) -> i16 {
    let width_penalty: i16 = request
        .width_fields()
        .into_iter()
        .zip(format.width_fields())
        .map(|((_, request), (_, format))| {
            value_penalty(request.map(|v| v as i16), format.map(|v| v as i16))
        })
        .sum();
    let fraction_penalty = value_penalty(
        request.fractional_digits.map(i16::from),
        format.fractional_digits.map(i16::from),
    );
    let zone_penalty = match (request.zone_name, format.zone_name) {
        (Some(request), Some(format)) => zone_penalty(request, format),
        (request, format) => value_penalty(request.map(|_| 0), format.map(|_| 0)),
    };
    -width_penalty - fraction_penalty - zone_penalty
}

/// Selects the first maximum; an empty collection has no match.
#[cfg(test)]
pub(super) fn basic_match(
    request: MatchComponents,
    formats: impl IntoIterator<Item = MatchComponents>,
) -> Option<usize> {
    let mut best = None;
    for (index, format) in formats.into_iter().enumerate() {
        let score = basic_score(request, format);
        if best.is_none_or(|(_, best_score)| score > best_score) {
            best = Some((index, score));
        }
    }
    best.map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_and_fraction_penalties_are_asymmetric_and_clamped() {
        use Width::*;
        let year = |width| MatchComponents {
            year: Some(width),
            ..Default::default()
        };
        assert_eq!(basic_score(year(Numeric), year(TwoDigit)), -6);
        assert_eq!(basic_score(year(TwoDigit), year(Numeric)), -3);
        assert_eq!(basic_score(year(Long), year(TwoDigit)), -8);
        assert_eq!(basic_score(year(TwoDigit), year(Long)), -6);
        let fraction = |digits| MatchComponents {
            fractional_digits: Some(digits),
            ..Default::default()
        };
        assert_eq!(basic_score(fraction(1), fraction(3)), -6);
        assert_eq!(basic_score(fraction(3), fraction(1)), -8);
    }

    #[test]
    fn removal_addition_empty_and_ties_obey_spec() {
        let request = MatchComponents {
            year: Some(Width::Numeric),
            ..Default::default()
        };
        let extra = MatchComponents {
            year: Some(Width::Numeric),
            weekday: Some(Width::Long),
            month: Some(Width::Long),
            day: Some(Width::Numeric),
            hour: Some(Width::Numeric),
            second: Some(Width::Numeric),
            ..Default::default()
        };
        assert_eq!(basic_score(request, MatchComponents::default()), -120);
        assert_eq!(basic_score(request, extra), -100);
        assert_eq!(
            basic_match(request, [MatchComponents::default(), extra]),
            Some(1)
        );
        assert_eq!(basic_match(request, [request, request]), Some(0));
        assert_eq!(basic_match(request, []), None);
        assert_eq!(
            basic_score(MatchComponents::default(), MatchComponents::default()),
            0
        );
    }

    #[test]
    fn all_thirty_six_zone_penalties_match_the_specification() {
        use ZoneName::*;
        let values = [
            Short,
            Long,
            ShortOffset,
            LongOffset,
            ShortGeneric,
            LongGeneric,
        ];
        let expected = [
            [0, 3, 1, 4, 120, 120],
            [8, 0, 9, 1, 120, 120],
            [120, 120, 0, 3, 120, 120],
            [120, 120, 8, 0, 120, 120],
            [120, 120, 1, 4, 0, 3],
            [120, 120, 9, 1, 8, 0],
        ];
        for (i, request) in values.into_iter().enumerate() {
            for (j, available) in values.into_iter().enumerate() {
                assert_eq!(zone_penalty(request, available), expected[i][j]);
            }
        }
    }

    #[test]
    fn all_eleven_components_contribute_independently() {
        let complete = MatchComponents {
            weekday: Some(Width::Short),
            era: Some(Width::Short),
            year: Some(Width::Numeric),
            month: Some(Width::Numeric),
            day: Some(Width::Numeric),
            day_period: Some(Width::Short),
            hour: Some(Width::Numeric),
            minute: Some(Width::Numeric),
            second: Some(Width::Numeric),
            fractional_digits: Some(2),
            zone_name: Some(ZoneName::Short),
        };
        assert_eq!(basic_score(MatchComponents::default(), complete), -220);
        assert_eq!(basic_score(complete, MatchComponents::default()), -1320);
        assert_eq!(basic_score(complete, complete), 0);
        let date = complete.date_only();
        let time = complete.time_only();
        assert_eq!(date.with_time(time), complete);
    }
}
