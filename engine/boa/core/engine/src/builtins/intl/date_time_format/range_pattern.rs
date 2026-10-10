//! Pure owned interval compilation; formatted strings are never reparsed.
use icu_datetime::{
    pattern::DateTimePattern,
    provider::{
        fields::{Day, Field, FieldSymbol, Second, Year},
        pattern::{PatternItem, runtime::Pattern},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FieldId {
    Era,
    Year,
    RelatedYear,
    YearName,
    Month,
    Weekday,
    Day,
    DayPeriod,
    Hour,
    Minute,
    Second,
    Fraction,
    Zone,
}

impl FieldId {
    pub(super) fn bit(self) -> u16 {
        1 << self as u8
    }

    pub(super) fn of(symbol: FieldSymbol) -> Option<Self> {
        Some(match symbol {
            FieldSymbol::Era => Self::Era,
            FieldSymbol::Year(Year::Calendar) => Self::Year,
            FieldSymbol::Year(Year::RelatedIso) => Self::RelatedYear,
            FieldSymbol::Year(Year::Cyclic) => Self::YearName,
            FieldSymbol::Month(_) => Self::Month,
            FieldSymbol::Weekday(_) => Self::Weekday,
            FieldSymbol::Day(Day::DayOfMonth) => Self::Day,
            FieldSymbol::DayPeriod(_) => Self::DayPeriod,
            FieldSymbol::Hour(_) => Self::Hour,
            FieldSymbol::Minute => Self::Minute,
            FieldSymbol::Second(Second::Second) => Self::Second,
            FieldSymbol::DecimalSecond(_) => Self::Fraction,
            FieldSymbol::TimeZone(_) => Self::Zone,
            _ => return None,
        })
    }

    pub(super) fn is_date(self) -> bool {
        matches!(
            self,
            Self::Era
                | Self::Year
                | Self::RelatedYear
                | Self::YearName
                | Self::Month
                | Self::Weekday
                | Self::Day
        )
    }
}

#[derive(Debug, Clone)]
pub(super) struct PatternFields {
    pub(super) fields: [Option<Field>; 13],
    pub(super) mask: u16,
    pub(super) fraction: u8,
}

impl PatternFields {
    pub(super) fn new(pattern: &Pattern<'_>) -> Result<Self, String> {
        let mut result = Self {
            fields: [None; 13],
            mask: 0,
            fraction: 0,
        };
        for item in pattern.items.iter() {
            if let PatternItem::Field(field) = item {
                let id = FieldId::of(field.symbol).ok_or("unsupported interval field")?;
                result.fields[id as usize] = Some(field);
                result.mask |= id.bit();
                if let FieldSymbol::DecimalSecond(digits) = field.symbol {
                    result.fraction = digits as u8;
                    result.mask |= FieldId::Second.bit();
                }
            }
        }
        Ok(result)
    }

    pub(super) fn has(&self, id: FieldId) -> bool {
        self.mask & id.bit() != 0
    }

    pub(super) fn date_mask(&self) -> u16 {
        self.mask & ((1 << (FieldId::Day as u8 + 1)) - 1)
    }

    pub(super) fn time_mask(&self) -> u16 {
        self.mask & !self.date_mask()
    }

    /// Source skeleton y covers era-calendar, related and cyclic year spelling.
    /// This is only a matching identity; writer fields remain distinct.
    pub(super) fn logical_mask(&self) -> u16 {
        let years = FieldId::Year.bit() | FieldId::RelatedYear.bit() | FieldId::YearName.bit();
        (self.mask & !years)
            | if self.mask & years != 0 {
                FieldId::Year.bit()
            } else {
                0
            }
    }
}

#[derive(Debug, Clone)]
pub(super) struct IntervalPattern {
    pub(super) first: Pattern<'static>,
    pub(super) second: Pattern<'static>,
    pub(super) repeated: u16,
    pub(super) first_is_end: bool,
}

impl IntervalPattern {
    pub(super) fn parse(raw: &str, fallback_reversed: bool) -> Result<Self, String> {
        let (raw, first_is_end) = if let Some(raw) = raw.strip_prefix("latestFirst:") {
            (raw, true)
        } else if let Some(raw) = raw.strip_prefix("earliestFirst:") {
            (raw, false)
        } else {
            (raw, fallback_reversed)
        };
        let pattern: Pattern<'static> = DateTimePattern::try_from_pattern_str(raw)
            .map_err(|error| format!("interval pattern: {error:?}"))?
            .into();
        let mut seen = 0;
        let mut split = None;
        for (index, item) in pattern.items.iter().enumerate() {
            if let PatternItem::Field(field) = item {
                let id = FieldId::of(field.symbol).ok_or("unsupported interval pattern field")?;
                if seen & id.bit() != 0 {
                    split = Some(index);
                    break;
                }
                seen |= id.bit();
            }
        }
        let split = split.ok_or("interval pattern has no repeated field")?;
        let first: Pattern<'static> = pattern.items.iter().take(split).collect();
        let second: Pattern<'static> = pattern.items.iter().skip(split).collect();
        let repeated = PatternFields::new(&first)?.mask & PatternFields::new(&second)?.mask;
        Ok(Self {
            first,
            second,
            repeated,
            first_is_end,
        })
    }

    /// Adopt the resolved widths and hour symbols, retaining raw source literals.
    /// Cyclic/related-year spelling cannot be rewritten to an era-calendar year.
    pub(super) fn adapt(mut self, selected: &PatternFields) -> Result<Self, String> {
        let adapt = |pattern: Pattern<'static>| -> Result<Pattern<'static>, String> {
            let mut items = Vec::new();
            for item in pattern.items.iter() {
                let item = if let PatternItem::Field(mut field) = item {
                    let id = FieldId::of(field.symbol).ok_or("unsupported interval field")?;
                    if let Some(actual) = selected.fields[id as usize] {
                        // Month/weekday context comes from the interval's locale
                        // pattern; clock cycles and displayed precision come
                        // from the resolved single-date selection.
                        field.length = actual.length;
                        if matches!(id, FieldId::Hour | FieldId::Fraction) {
                            field.symbol = actual.symbol;
                        }
                    }
                    PatternItem::Field(field)
                } else {
                    item
                };
                items.push(item);
            }
            let pattern: Pattern<'static> = items.into_iter().collect();
            Ok(super::pattern::ordinary_day_period_spaces(pattern.into()).into())
        };
        self.first = adapt(self.first)?;
        self.second = adapt(self.second)?;
        let fields = PatternFields::new(&self.first)?.mask | PatternFields::new(&self.second)?.mask;
        if fields & selected.mask != selected.mask {
            return Err("interval omits a selected native field".into());
        }
        Ok(self)
    }
}

#[derive(Debug, Clone)]
pub(super) enum Placeholder {
    Literal(String),
    Value(u8),
}

pub(super) fn placeholders(raw: &str) -> Result<Vec<Placeholder>, String> {
    let mut result = Vec::new();
    let mut text = String::new();
    let mut chars = raw.chars().peekable();
    let mut counts = [0; 2];
    while let Some(ch) = chars.next() {
        if ch != '{' {
            text.push(ch);
            continue;
        }
        let digit = chars.next().ok_or("unterminated interval placeholder")?;
        let index = match digit {
            '0' => 0,
            '1' => 1,
            _ => return Err("invalid interval placeholder".into()),
        };
        if chars.next() != Some('}') {
            return Err("invalid interval placeholder closing brace".into());
        }
        if !text.is_empty() {
            result.push(Placeholder::Literal(core::mem::take(&mut text)));
        }
        counts[index] += 1;
        result.push(Placeholder::Value(index as u8));
    }
    if !text.is_empty() {
        result.push(Placeholder::Literal(text));
    }
    if counts != [1, 1] {
        return Err("interval fallback must contain both endpoints once".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_split_preserves_quoted_fields_and_shared_month_year() {
        let pattern = IntervalPattern::parse("MMM d 'M'\u{2009}–\u{2009}d, y", false).unwrap();
        assert_eq!(pattern.repeated, FieldId::Day.bit());
        assert!(
            PatternFields::new(&pattern.first)
                .unwrap()
                .has(FieldId::Month)
        );
        assert!(
            PatternFields::new(&pattern.second)
                .unwrap()
                .has(FieldId::Year)
        );
    }

    #[test]
    fn explicit_order_overrides_reversed_locale_fallback() {
        assert!(
            IntervalPattern::parse("latestFirst:d–d", false)
                .unwrap()
                .first_is_end
        );
        assert!(
            !IntervalPattern::parse("earliestFirst:d–d", true)
                .unwrap()
                .first_is_end
        );
        assert!(IntervalPattern::parse("d–d", true).unwrap().first_is_end);
        assert!(placeholders("prefix {1} - {0} suffix").is_ok());
        assert!(placeholders("{0}/{0}").is_err());
        assert!(IntervalPattern::parse("d 'unfinished", false).is_err());
    }
}
