//! Enumerate source candidates only after checking native provider capabilities.
use super::{date_time_format, locale::validate_extension};
use crate::{Context, JsArgs, JsNativeError, JsResult, JsValue, builtins::array::Array, js_string};
use boa_intl_data::{
    OmoikaneSupportedValuesV1, SANCTIONED_SIMPLE_UNIT_IDENTIFIERS, SupportedValues,
};
use icu_collator::{preferences::CollationType, provider::CollationMetadataV1};
use icu_locale::{LanguageIdentifier, extensions::unicode::Value, locale};
use icu_provider::prelude::*;

/// Implements `Intl.supportedValuesOf` with one observable key conversion.
/// Data queries use the current immutable provider; returned arrays are fresh.
/// See <https://tc39.es/ecma402/#sec-intl.supportedvaluesof>.
pub(super) fn supported_values_of(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let key = args
        .get_or_undefined(0)
        .to_string(context)?
        .to_std_string_escaped();
    let values = match key.as_str() {
        "timeZone" => time_zones(context)?,
        "unit" => SANCTIONED_SIMPLE_UNIT_IDENTIFIERS
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect(),
        "calendar" | "collation" | "currency" | "numberingSystem" => {
            let provider = context.intl_provider().erased_provider();
            let payload = DataProvider::<OmoikaneSupportedValuesV1>::load(
                &provider.as_deserializing(),
                DataRequest::default(),
            )
            .map_err(|error| {
                JsNativeError::range().with_message(format!("supported-value inventory: {error:?}"))
            })?
            .payload;
            select(payload.get(), &key, context)
        }
        _ => {
            return Err(JsNativeError::range()
                .with_message("invalid Intl.supportedValuesOf key")
                .into());
        }
    };
    let mut values: Vec<String> = values;
    values.sort_unstable();
    values.dedup();
    Ok(Array::create_array_from_list(
        values.into_iter().map(|value| js_string!(value).into()),
        context,
    )
    .into())
}

fn select(data: &SupportedValues<'_>, category: &str, context: &Context) -> Vec<String> {
    let provider = context.intl_provider();
    let preferences: icu_datetime::DateTimeFormatterPreferences = locale!("en").into();
    let locales: Vec<LanguageIdentifier> = data
        .identifiers("locales")
        .filter_map(|value| value.parse().ok())
        .collect();
    data.identifiers(category)
        .filter(|identifier| match category {
            "calendar" => identifier.parse::<Value>().ok().is_some_and(|value| {
                date_time_format::supports_calendar(&value, preferences, provider)
            }),
            "numberingSystem" => identifier
                .parse::<Value>()
                .ok()
                .is_some_and(|value| date_time_format::supports_numbering(&value, provider)),
            "collation" => identifier
                .parse::<Value>()
                .ok()
                .and_then(|value| CollationType::try_from(&value).ok())
                .is_some_and(|collation| {
                    let attributes = DataMarkerAttributes::from_str_or_panic(collation.as_str());
                    locales.iter().any(|locale| {
                        validate_extension::<CollationMetadataV1>(
                            locale.clone(),
                            attributes,
                            provider,
                        )
                    })
                }),
            "currency" => true,
            _ => false,
        })
        .map(str::to_owned)
        .collect()
}

fn time_zones(context: &Context) -> JsResult<Vec<String>> {
    let parser = icu_time::zone::iana::IanaParserExtended::try_new_with_buffer_provider(
        context.intl_provider().erased_provider(),
    )
    .map_err(|error| {
        JsNativeError::range().with_message(format!("available time-zone identifiers: {error:?}"))
    })?;
    Ok(parser
        .as_borrowed()
        .iter()
        .filter(|entry| entry.time_zone != icu_time::TimeZone::UNKNOWN)
        .map(|entry| {
            if ["Etc/UTC", "Etc/GMT", "GMT"].contains(&entry.canonical) {
                "UTC"
            } else {
                entry.canonical
            }
        })
        .map(str::to_owned)
        .collect())
}

#[cfg(all(test, feature = "intl_bundled"))]
mod tests {
    use crate::{Context, JsValue, Source};

    #[test]
    fn supported_inventories_are_fresh_sorted_and_observe_key_conversion_once() {
        let mut context = Context::default();
        let result = context.eval(Source::from_bytes(r#"(() => {
            function check(value, label) { if (!value) throw new Error(label); }
            check(Intl.supportedValuesOf.length === 1, 'function length');
            for (const key of ['calendar','collation','currency','numberingSystem','timeZone','unit']) {
                const values = Intl.supportedValuesOf(key);
                check(Array.isArray(values) && values.length > 0, key);
                for (let i = 0; i < values.length; ++i) {
                    check(typeof values[i] === 'string', key + ': string');
                    if (i) check(values[i-1] < values[i], key + ': sorted unique');
                }
                const again = Intl.supportedValuesOf(key);
                check(values !== again && JSON.stringify(values) === JSON.stringify(again), key + ': fresh');
                values.length = 0;
                check(again.length > 0, key + ': independent');
            }
            let reads = 0;
            check(Intl.supportedValuesOf({toString() { reads++; return 'unit'; }}).includes('meter'), 'coercion');
            check(reads === 1, 'conversion count');
            for (const key of [undefined, null, 'Calendar', 'unknown']) {
                try { Intl.supportedValuesOf(key); throw new Error('accepted invalid key'); }
                catch (error) { if (!(error instanceof RangeError)) throw error; }
            }
            try { Intl.supportedValuesOf(Symbol('unit')); throw new Error('converted symbol'); }
            catch (error) { if (!(error instanceof TypeError)) throw error; }
            check(Intl.supportedValuesOf('calendar').includes('gregory'), 'Gregorian support');
            check(Intl.supportedValuesOf('timeZone').includes('UTC'), 'UTC support');
            return true;
        })()"#)).unwrap();
        assert_eq!(result, JsValue::new(true));
    }
}
