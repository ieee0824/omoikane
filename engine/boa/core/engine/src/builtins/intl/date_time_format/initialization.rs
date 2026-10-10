//! Native DateTimeFormat initialization, separate from formatting and I/O.
use super::{
    DateTimeFormat,
    backend::DateTimeBackend,
    matcher::{FormatMatcher, MatchComponents},
    options::{Components, string_option},
};
use crate::builtins::{
    intl::{
        Service,
        locale::{canonicalize_locale_list, resolve_locale},
        options::{IntlOptions, coerce_options_to_object},
    },
    options::get_option,
};
use crate::{Context, JsNativeError, JsResult, JsString, JsValue, js_string};
use icu_locale::{
    Locale,
    extensions::unicode::{Value, key},
};

impl Service for DateTimeFormat {
    // Decimal symbols can be shared with the root locale even when date/time
    // patterns are locale-specific. Resolve against the locale's default-hour
    // time skeleton instead of discarding that date/time locale information.
    type LangMarker = icu_datetime::provider::DatetimePatternsTimeV1;
    const ATTRIBUTES: &'static icu_provider::DataMarkerAttributes =
        icu_provider::DataMarkerAttributes::from_str_or_panic("j");
    type LocaleOptions = ();
    fn resolve(locale: &mut Locale, _: &mut (), _: &crate::context::icu::IntlProvider) {
        let keywords = [key!("ca"), key!("nu"), key!("hc")]
            .map(|key| (key, locale.extensions.unicode.keywords.get(&key).cloned()));
        locale.extensions.unicode.clear();
        for (key, value) in keywords {
            if let Some(value) = value {
                locale.extensions.unicode.keywords.set(key, value);
            }
        }
    }
}

impl DateTimeFormat {
    /// Returns requested locales supported by the date/time pattern provider.
    pub(super) fn supported_locales_of(
        _: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        use crate::JsArgs;
        use crate::builtins::intl::locale::filter_locales;
        let requested = canonicalize_locale_list(args.get_or_undefined(0), context)?;
        filter_locales::<Self>(requested, args.get_or_undefined(1), context).map(JsValue::from)
    }

    pub(crate) fn new(
        locales: &JsValue,
        options: &JsValue,
        context: &mut Context,
    ) -> JsResult<Self> {
        Self::new_with_requirements(
            locales,
            options,
            super::DateTimeReqs::AnyAll,
            super::DateTimeReqs::Date,
            context,
        )
    }

    pub(crate) fn new_with_requirements(
        locales: &JsValue,
        options: &JsValue,
        required: super::DateTimeReqs,
        defaults: super::DateTimeReqs,
        context: &mut Context,
    ) -> JsResult<Self> {
        let requested = canonicalize_locale_list(locales, context)?;
        let options = coerce_options_to_object(options, context)?;
        let _options_root = options.clone().root();
        let matcher =
            get_option(&options, js_string!("localeMatcher"), context)?.unwrap_or_default();
        let calendar = unicode_option(&options, "calendar", context)?;
        let numbering = unicode_option(&options, "numberingSystem", context)?;
        let hour12 = get_option::<bool>(&options, js_string!("hour12"), context)?;
        let hour_cycle = string_option(
            &options,
            "hourCycle",
            &["h11", "h12", "h23", "h24"],
            context,
        )?;
        let mut intl_options = IntlOptions {
            matcher,
            service_options: (),
        };
        let locale = resolve_locale::<Self>(requested, &mut intl_options, context.intl_provider())?;
        let resolved = super::negotiation::resolve(
            locale,
            calendar,
            numbering,
            hour12,
            hour_cycle.as_ref().and_then(|cycle| {
                super::hour_cycle::DateTimeHourCycle::from_str(&cycle.to_std_string_escaped())
            }),
            context.intl_provider(),
        )
        .map_err(|error| JsNativeError::range().with_message(error))?;
        let time_zone = super::timezone::read_time_zone(&options, context)?;
        let components = Components::read(&options, required, defaults, context)?;
        let provider = context.intl_provider().erased_provider();
        let component_pattern = components.date_style.is_none() && components.time_style.is_none();
        let backend = if component_pattern
            && (components.format_matcher == FormatMatcher::Basic
                || components.day_period.is_some())
        {
            DateTimeBackend::new_basic(
                provider,
                resolved.preferences,
                MatchComponents::requested(&components),
                resolved.iso_calendar,
                resolved.hour_cycle,
            )
        } else {
            DateTimeBackend::new(
                provider,
                resolved.preferences,
                components.fields()?,
                components.zone_name.as_ref().and_then(|value| {
                    super::matcher::ZoneName::from_str(&value.to_std_string_escaped())
                }),
                resolved.iso_calendar,
                resolved.hour_cycle,
            )
        }
        .map_err(|error| JsNativeError::range().with_message(error))?;
        Ok(Self {
            locale: resolved.locale,
            time_zone,
            components,
            backend,
            bound_format: None,
        })
    }
}

fn unicode_option(
    options: &crate::JsObject,
    name: &str,
    context: &mut Context,
) -> JsResult<Option<Value>> {
    let option = get_option::<JsString>(options, js_string!(name), context)?;
    option
        .map(|value| {
            let value = value.to_std_string_escaped();
            if !value.split('-').all(|part| {
                (3..=8).contains(&part.len())
                    && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
            }) {
                return Err(JsNativeError::range()
                    .with_message(format!("invalid {name} option"))
                    .into());
            }
            Value::try_from_str(&value).map_err(|_| {
                JsNativeError::range()
                    .with_message(format!("invalid {name} option"))
                    .into()
            })
        })
        .transpose()
}
