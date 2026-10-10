//! Observable constructor option reads and locale negotiation.

use super::{
    DurationFormat,
    backend::DurationBackend,
    options::{DurationOptions, DurationUnit},
};
use crate::{
    Context, JsArgs, JsNativeError, JsResult, JsValue,
    builtins::{
        intl::{
            Service,
            locale::{
                canonicalize_locale_list, filter_locales, resolve_locale, validate_extension,
            },
            options::IntlOptions,
        },
        options::{OptionType, get_option, get_options_object},
    },
    context::icu::IntlProvider,
    js_string,
    object::ObjectInitializer,
    property::Attribute,
};
use boa_intl_data::{DurationDigitalField, OmoikaneDurationDigitalV1};
use icu_decimal::preferences::NumberingSystem;
use icu_locale::{
    Locale,
    extensions::unicode::{Value, key},
};
use icu_provider::prelude::*;

#[derive(Debug, Default)]
pub(in crate::builtins::intl) struct LocaleOptions {
    numbering_system: Option<Value>,
}

impl Service for DurationFormat {
    type LangMarker = OmoikaneDurationDigitalV1;
    type LocaleOptions = LocaleOptions;

    fn resolve(locale: &mut Locale, options: &mut LocaleOptions, provider: &IntlProvider) {
        let supported = |value: &Value| {
            validate_extension::<OmoikaneDurationDigitalV1>(
                locale.id.clone(),
                DataMarkerAttributes::from_str_or_panic(&value.to_string()),
                provider,
            )
        };
        let extension = locale
            .extensions
            .unicode
            .keywords
            .get(&key!("nu"))
            .cloned()
            .filter(&supported);
        let selected = options
            .numbering_system
            .take()
            .filter(supported)
            .or_else(|| extension.clone());
        locale.extensions.unicode.clear();
        if let Some(extension) = extension.filter(|extension| Some(extension) == selected.as_ref())
        {
            locale
                .extensions
                .unicode
                .keywords
                .set(key!("nu"), extension);
        }
        options.numbering_system = selected;
    }
}

impl DurationFormat {
    /// Reads observable options once, then loads owned native formatter data.
    pub(super) fn new(
        locales: &JsValue,
        options: &JsValue,
        context: &mut Context,
    ) -> JsResult<Self> {
        let _locales_root = locales.as_object().map(|object| object.root());
        let _options_input_root = options.as_object().map(|object| object.root());
        let requested = canonicalize_locale_list(locales, context)?;
        let options = get_options_object(options)?;
        let _options_root = options.clone().root();
        let matcher =
            get_option(&options, js_string!("localeMatcher"), context)?.unwrap_or_default();
        let value = options.get(js_string!("numberingSystem"), context)?;
        let _value_root = value.as_object().map(|object| object.root());
        let numbering_system = if value.is_undefined() {
            None
        } else {
            let system = NumberingSystem::from_value(value, context)?;
            Some(Value::try_from_str(system.as_str()).expect("validated numbering system"))
        };
        let mut negotiated = IntlOptions {
            matcher,
            service_options: LocaleOptions { numbering_system },
        };
        let locale = resolve_locale::<Self>(requested, &mut negotiated, context.intl_provider())?;
        let digital = load_digital(
            context.intl_provider(),
            &locale,
            negotiated.service_options.numbering_system.as_ref(),
        )?;
        let numbering_system = digital
            .get()
            .get(DurationDigitalField::NumberingSystem)
            .ok_or_else(|| JsNativeError::typ().with_message("duration numbering data is missing"))?
            .to_owned();
        // ECMA-402 leaves this locale-data Boolean implementation-defined.
        // A CLDR clock's `hh` does not require a duration's leading hour to be
        // padded, and forcing it also changes textual hours via option step 8.
        let options = DurationOptions::from_options(&options, false, context)?;
        let backend = DurationBackend::new(
            context.intl_provider(),
            &locale,
            &numbering_system,
            &options,
            digital,
        )?;
        Ok(Self {
            locale,
            numbering_system,
            options,
            backend,
        })
    }

    pub(super) fn supported_locales_of(
        _: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let requested = canonicalize_locale_list(args.get_or_undefined(0), context)?;
        filter_locales::<Self>(requested, args.get_or_undefined(1), context).map(Into::into)
    }

    pub(super) fn resolved_options(
        this: &JsValue,
        _: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let object = Self::receiver(this)?;
        let _object_root = object.clone().root();
        let data = object.borrow();
        let data = data.data();
        let mut result = ObjectInitializer::new(context);
        let attributes = Attribute::all();
        result.property(
            js_string!("locale"),
            js_string!(data.locale.to_string()),
            attributes,
        );
        result.property(
            js_string!("numberingSystem"),
            js_string!(data.numbering_system.as_str()),
            attributes,
        );
        result.property(
            js_string!("style"),
            js_string!(data.options.style.as_str()),
            attributes,
        );
        for unit in DurationUnit::ALL {
            let options = data.options.unit(unit);
            result.property(
                js_string!(unit.plural()),
                js_string!(options.style.as_str()),
                attributes,
            );
            result.property(
                js_string!(format!("{}Display", unit.plural())),
                js_string!(options.display.as_str()),
                attributes,
            );
        }
        if let Some(digits) = data.options.fractional_digits {
            result.property(js_string!("fractionalDigits"), digits, attributes);
        }
        Ok(result.build().into())
    }
}

fn load_digital(
    provider: &IntlProvider,
    locale: &Locale,
    nu: Option<&Value>,
) -> JsResult<DataPayload<OmoikaneDurationDigitalV1>> {
    let data_locale = DataLocale::from(locale.id.clone());
    let attribute = nu.map(ToString::to_string).unwrap_or_default();
    let attributes = DataMarkerAttributes::from_str_or_panic(&attribute);
    DataProvider::<OmoikaneDurationDigitalV1>::load(
        provider,
        DataRequest {
            id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &data_locale),
            ..Default::default()
        },
    )
    .map(|response| response.payload)
    .map_err(|error| {
        JsNativeError::typ()
            .with_message(format!("duration data: {error}"))
            .into()
    })
}
