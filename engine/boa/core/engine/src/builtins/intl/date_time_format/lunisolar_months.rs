//! Localize lunar month codes without confusing them with ordinal month indexes.
use super::parts::{DateTimePart, PartKind};
use boa_intl_data::OmoikaneDateTimeStylePatternsV1;
use icu_calendar::types::MonthInfo;
use icu_datetime::{DateTimeFormatterPreferences, provider::fields::components};
use icu_provider::prelude::*;
use writeable::Writeable;

/// Retains the source payload; all pattern and field borrows are call-local.
#[derive(Debug, Clone)]
pub(super) struct LunarMonthNames {
    data: DataPayload<OmoikaneDateTimeStylePatternsV1>,
}

impl LunarMonthNames {
    pub(super) fn load<P: BufferProvider + ?Sized>(
        provider: &P,
        preferences: DateTimeFormatterPreferences,
        calendar: &str,
    ) -> Result<Option<Self>, String> {
        if !matches!(calendar, "chinese" | "dangi") {
            return Ok(None);
        }
        let locale =
            OmoikaneDateTimeStylePatternsV1::INFO.make_locale(preferences.locale_preferences);
        let attributes = DataMarkerAttributes::from_str_or_panic(calendar);
        let data = DataProvider::<OmoikaneDateTimeStylePatternsV1>::load(
            &provider.as_deserializing(),
            DataRequest {
                id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
                ..Default::default()
            },
        )
        .map_err(|error| format!("lunar month patterns: {error:?}"))?
        .payload;
        Ok(Some(Self { data }))
    }

    pub(super) fn replace(
        &self,
        parts: &mut [DateTimePart],
        pattern: &icu_datetime::pattern::DateTimePattern,
        month: MonthInfo,
        decimal: &icu_decimal::DecimalFormatter,
    ) -> Result<(), String> {
        let width = components::Bag::from(pattern).month;
        let source = self.data.get();
        let family = match width {
            Some(components::Month::Numeric | components::Month::TwoDigit) => "monthNumeric",
            Some(components::Month::Narrow)
                if (1..=12).all(|number| {
                    source.get("monthNarrow", &number.to_string())
                        == Some(number.to_string().as_str())
                }) =>
            {
                "monthNarrowPattern"
            }
            _ => return Ok(()),
        };
        let mut number = fixed_decimal::Decimal::from(month.month_number());
        if width == Some(components::Month::TwoDigit) {
            number.pad_start(2);
        }
        let value = decimal.format(&number).write_to_string().into_owned();
        let value = if month.is_leap() {
            let template = source
                .get(family, "leap")
                .ok_or("missing lunar leap month pattern")?;
            if template.matches("{0}").count() != 1 {
                return Err("invalid lunar leap month pattern".to_owned());
            }
            template.replace("{0}", &value)
        } else {
            value
        };
        for part in parts {
            if part.kind == PartKind::Month {
                part.value.clone_from(&value);
            }
        }
        Ok(())
    }
}
