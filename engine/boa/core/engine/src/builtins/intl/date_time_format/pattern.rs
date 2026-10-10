//! Owned ICU pattern adaptation for ECMA-402 component widths.
use icu_datetime::{
    pattern::DateTimePattern,
    provider::{
        fields::{Day, FieldLength, FieldSymbol, Second, Year},
        pattern::{PatternItem, runtime::Pattern},
    },
};

/// Explicit component widths; absent entries preserve the selected locale pattern.
///
/// Pattern selection must run before this adjustment. In particular, changing
/// widths does not remove unwanted fields or select calendar/hour-cycle symbols.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ComponentWidths {
    pub(super) era: Option<FieldLength>,
    pub(super) year: Option<FieldLength>,
    pub(super) month: Option<FieldLength>,
    pub(super) day: Option<FieldLength>,
    pub(super) weekday: Option<FieldLength>,
    pub(super) day_period: Option<FieldLength>,
    pub(super) hour: Option<FieldLength>,
    pub(super) minute: Option<FieldLength>,
    pub(super) second: Option<FieldLength>,
}

impl ComponentWidths {
    /// Maps explicit date components to a CLDR skeleton, in canonical field
    /// order. Abbreviated era and weekday widths use the equivalent G/E form.
    /// A missing exact available format remains the semantic formatter's choice.
    pub(super) fn date_skeleton(self) -> Option<String> {
        let mut result = String::new();
        for (symbol, width) in [
            ('G', self.era),
            ('y', self.year),
            ('M', self.month),
            ('E', self.weekday),
            ('d', self.day),
        ] {
            let Some(width) = width else { continue };
            let count = match width {
                FieldLength::One | FieldLength::NumericOverride(_) => 1,
                FieldLength::Two => 2,
                FieldLength::Three if matches!(symbol, 'G' | 'E') => 1,
                FieldLength::Three => 3,
                FieldLength::Four => 4,
                FieldLength::Five => 5,
                FieldLength::Six => 6,
            };
            result.extend(std::iter::repeat_n(symbol, count));
        }
        (!result.is_empty()).then_some(result)
    }

    /// Adjusts component widths and browser day-period separators, preserving
    /// field order, calendar context, and all other literals.
    ///
    /// Year names and related ISO years are not interchangeable with a numeric
    /// calendar year. Likewise, ordinal days and milliseconds-in-day must not
    /// inherit the requested day-of-month or seconds-in-minute width.
    pub(super) fn apply(self, pattern: DateTimePattern) -> DateTimePattern {
        rewrite_pattern_items(pattern, |index, item, original| {
            let adjusted = if let PatternItem::Field(mut field) = item {
                let width = match field.symbol {
                    FieldSymbol::Era => self.era,
                    FieldSymbol::Year(Year::Calendar) => self.year,
                    // Some locales express a long/short month as a number
                    // followed by a literal unit (for example M'月'). Keep
                    // that representation selected by the semantic skeleton;
                    // replacing M with a textual name would repeat the unit.
                    FieldSymbol::Month(_)
                        if matches!(field.length, FieldLength::One | FieldLength::Two)
                            && self.month.is_some_and(|width| {
                                matches!(
                                    width,
                                    FieldLength::Three | FieldLength::Four | FieldLength::Five
                                )
                            })
                            && numeric_month_has_unit_suffix(original, index) =>
                    {
                        None
                    }
                    FieldSymbol::Month(_) => self.month,
                    FieldSymbol::Day(Day::DayOfMonth) => self.day,
                    FieldSymbol::Weekday(_) => self.weekday,
                    FieldSymbol::DayPeriod(_) => self.day_period,
                    // Locale time patterns may resolve a numeric request to
                    // two digits (notably minutes/seconds in a clock). Keep
                    // that contextual padding, while honoring explicit 2-digit.
                    FieldSymbol::Hour(_) => contextual_time_width(self.hour, field.length),
                    FieldSymbol::Minute => contextual_time_width(self.minute, field.length),
                    FieldSymbol::Second(Second::Second) => {
                        contextual_time_width(self.second, field.length)
                    }
                    _ => None,
                };
                if let Some(width) = width {
                    field.length = width;
                }
                PatternItem::Field(field)
            } else {
                item
            };
            ordinary_day_period_space(original, index, adjusted)
        })
    }
}

/// Returns whether a numeric month is immediately followed by a written unit.
///
/// CLDR patterns such as `M'月'` already spell a locale-specific month suffix;
/// replacing `M` with a textual month name would emit that suffix twice. Plain
/// numeric patterns and punctuation-separated dates still need the requested
/// textual width applied.
fn numeric_month_has_unit_suffix(pattern: &Pattern<'_>, index: usize) -> bool {
    matches!(
        pattern.items.get(index + 1),
        Some(PatternItem::Literal(suffix)) if suffix.is_alphabetic()
    )
}

/// Adapts browser AM/PM spacing without changing selected fields or widths.
pub(super) fn ordinary_day_period_spaces(pattern: DateTimePattern) -> DateTimePattern {
    rewrite_pattern_items(pattern, |index, item, original| {
        ordinary_day_period_space(original, index, item)
    })
}

/// Applies a pattern rewrite with no allocation when every item is unchanged.
///
/// Once the first changed item is found, the original prefix is copied and the
/// remaining items continue through the same rewrite. This keeps width and
/// separator adaptation in one transformation and creates at most one new
/// ICU pattern buffer.
fn rewrite_pattern_items(
    pattern: DateTimePattern,
    mut rewrite: impl FnMut(usize, PatternItem, &Pattern<'_>) -> PatternItem,
) -> DateTimePattern {
    let original: Pattern<'static> = pattern.into();
    {
        let mut items = original.items.iter().enumerate();
        while let Some((index, item)) = items.next() {
            let adjusted = rewrite(index, item, &original);
            if adjusted != item {
                let rewritten: Pattern<'static> = original
                    .items
                    .iter()
                    .take(index)
                    .chain(std::iter::once(adjusted))
                    .chain(items.map(|(index, item)| rewrite(index, item, &original)))
                    .collect();
                return rewritten.into();
            }
        }
    }
    original.into()
}

fn ordinary_day_period_space(
    pattern: &Pattern<'_>,
    index: usize,
    item: PatternItem,
) -> PatternItem {
    if item == PatternItem::Literal('\u{202f}')
        && (is_day_period(pattern.items.get(index + 1))
            || is_day_period(
                index
                    .checked_sub(1)
                    .and_then(|index| pattern.items.get(index)),
            ))
    {
        PatternItem::Literal(' ')
    } else {
        item
    }
}

fn is_day_period(item: Option<PatternItem>) -> bool {
    matches!(item, Some(PatternItem::Field(field)) if matches!(field.symbol, FieldSymbol::DayPeriod(_)))
}

fn contextual_time_width(
    requested: Option<FieldLength>,
    selected: FieldLength,
) -> Option<FieldLength> {
    if requested == Some(FieldLength::One) && selected == FieldLength::Two {
        None
    } else {
        requested
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn month_length(pattern: DateTimePattern) -> FieldLength {
        let pattern: Pattern<'static> = pattern.into();
        pattern
            .items
            .iter()
            .find_map(|item| match item {
                PatternItem::Field(field) if matches!(field.symbol, FieldSymbol::Month(_)) => {
                    Some(field.length)
                }
                _ => None,
            })
            .expect("test pattern contains a month")
    }

    #[test]
    fn textual_month_width_is_applied_without_a_unit_suffix() {
        let widths = ComponentWidths {
            month: Some(FieldLength::Five),
            ..Default::default()
        };
        for raw in ["M", "M/d"] {
            let pattern = DateTimePattern::try_from_pattern_str(raw).unwrap();
            assert_eq!(month_length(widths.apply(pattern)), FieldLength::Five);
        }
    }

    #[test]
    fn numeric_month_with_written_unit_keeps_numeric_width() {
        let widths = ComponentWidths {
            month: Some(FieldLength::Four),
            ..Default::default()
        };
        let pattern = DateTimePattern::try_from_pattern_str("M'月'").unwrap();
        assert_eq!(month_length(widths.apply(pattern)), FieldLength::One);
    }

    #[test]
    fn unchanged_pattern_reuses_its_owned_item_buffer() {
        let original: Pattern<'static> = DateTimePattern::try_from_pattern_str("y-MM-dd")
            .unwrap()
            .into();
        let original_items = original.items.as_bytes().as_ptr();

        let widths = ComponentWidths {
            year: Some(FieldLength::One),
            month: Some(FieldLength::Two),
            day: Some(FieldLength::Two),
            ..Default::default()
        };
        let unchanged: Pattern<'static> = widths.apply(original.into()).into();

        assert_eq!(unchanged.items.as_bytes().as_ptr(), original_items);
    }

    #[test]
    fn widths_and_day_period_spacing_are_rewritten_together() {
        let widths = ComponentWidths {
            era: Some(FieldLength::Four),
            year: Some(FieldLength::Two),
            month: Some(FieldLength::Four),
            day: Some(FieldLength::Two),
            hour: Some(FieldLength::Two),
            minute: Some(FieldLength::One),
            second: Some(FieldLength::One),
            ..Default::default()
        };
        let pattern =
            DateTimePattern::try_from_pattern_str("G y r M d D h:mm:ss\u{202f}a").unwrap();

        assert_eq!(
            widths.apply(pattern).to_string(),
            "GGGG yy r MMMM dd D hh:mm:ss a"
        );
    }
}
