//! Fractional-second separators from the selected locale and numbering system.
use super::parts::{DateTimePart, PartKind};
use boa_intl_data::{NumberSymbol, OmoikaneGlobalNumberSymbolsV1, OmoikaneNumberSymbolsV1};
use icu_datetime::DateTimeFormatterPreferences;
use icu_provider::prelude::*;

#[derive(Debug, Clone)]
pub(super) struct FractionalSeparator(String);

impl FractionalSeparator {
    pub(super) fn load<P: BufferProvider + ?Sized>(
        provider: &P,
        preferences: DateTimeFormatterPreferences,
        numbering_system: &str,
    ) -> Result<Self, String> {
        let locale = OmoikaneNumberSymbolsV1::INFO.make_locale(preferences.locale_preferences);
        let attributes = DataMarkerAttributes::from_str_or_panic(numbering_system);
        let local = DataProvider::<OmoikaneNumberSymbolsV1>::load(
            &provider.as_deserializing(),
            DataRequest {
                id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
                ..Default::default()
            },
        )
        .map(|response| response.payload);
        let separator = match local {
            Ok(payload) => payload.get().get(NumberSymbol::Decimal).map(str::to_owned),
            Err(error) if error.kind == DataErrorKind::IdentifierNotFound => {
                let root = DataLocale::default();
                let payload = DataProvider::<OmoikaneGlobalNumberSymbolsV1>::load(
                    &provider.as_deserializing(),
                    DataRequest {
                        id: DataIdentifierBorrowed::for_marker_attributes_and_locale(
                            attributes, &root,
                        ),
                        ..Default::default()
                    },
                )
                .map_err(|error| format!("root fractional-second symbols: {error:?}"))?
                .payload;
                payload.get().get(NumberSymbol::Decimal).map(str::to_owned)
            }
            Err(error) => return Err(format!("fractional-second symbols: {error:?}")),
        };
        separator
            .map(Self)
            .ok_or_else(|| "fractional-second decimal separator is absent".into())
    }

    /// ICU's nested decimal part identifies the separator between second and
    /// fractional second. No rendered digits or other literals are inspected.
    pub(super) fn replace(&self, parts: &mut [DateTimePart]) -> Result<(), String> {
        let separator = &self.0;
        for index in 2..parts.len() {
            if parts[index].kind == PartKind::FractionalSecond
                && parts[index - 1].kind == PartKind::Literal
                && parts[index - 2].kind == PartKind::Second
            {
                parts[index - 1].value.clear();
                parts[index - 1].value.push_str(separator);
            }
        }
        Ok(())
    }
}
