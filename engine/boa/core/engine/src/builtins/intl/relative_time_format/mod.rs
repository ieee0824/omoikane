//! Native RelativeTimeFormat owns immutable CLDR payloads and ICU formatters.
use boa_gc::{Finalize, Trace};
use boa_intl_data::OmoikaneRelativeTimePatternsV1;
use fixed_decimal::SignDisplay;
use icu_decimal::options::GroupingStrategy;
use icu_locale::{Locale, extensions::unicode::Value};
use icu_plurals::PluralRules as NativePluralRules;
use icu_provider::DataPayload;

use super::{
    Service,
    locale::{canonicalize_locale_list, filter_locales, resolve_locale},
    native_names::{load, option},
    number_format::{
        DigitFormatOptions, MathematicalValue, NativeNumberFormatter, NotationKind, NumberFormat,
        NumberFormatLocaleOptions, NumberPart, NumericFormatOptions, UnitFormatOptions,
    },
    options::{IntlOptions, coerce_options_to_object},
};
use crate::{
    Context, JsArgs, JsData, JsNativeError, JsObject, JsResult, JsString, JsSymbol, JsValue,
    builtins::{
        Array, BuiltInBuilder, BuiltInConstructor, BuiltInObject, IntrinsicObject,
        options::get_option,
    },
    context::intrinsics::{Intrinsics, StandardConstructor, StandardConstructors},
    js_string,
    object::{ObjectInitializer, internal_methods::get_prototype_from_constructor},
    property::Attribute,
    realm::Realm,
};

#[derive(Debug, Trace, Finalize, JsData)]
// SAFETY: every field is an owned Rust value, never a JavaScript GC edge.
#[boa_gc(unsafe_empty_trace)]
pub(crate) struct RelativeTimeFormat {
    locale: Locale,
    style: String,
    numeric: String,
    patterns: DataPayload<OmoikaneRelativeTimePatternsV1>,
    formatter: NativeNumberFormatter,
    plurals: NativePluralRules,
    number_options: NumericFormatOptions,
}

impl Service for RelativeTimeFormat {
    type LangMarker = OmoikaneRelativeTimePatternsV1;
    type LocaleOptions = NumberFormatLocaleOptions;

    fn resolve(
        locale: &mut Locale,
        options: &mut Self::LocaleOptions,
        provider: &crate::context::icu::IntlProvider,
    ) {
        <NumberFormat as Service>::resolve(locale, options, provider);
    }
}

impl IntrinsicObject for RelativeTimeFormat {
    fn init(realm: &Realm) {
        BuiltInBuilder::from_standard_constructor::<Self>(realm)
            .static_method(
                Self::supported_locales_of,
                js_string!("supportedLocalesOf"),
                1,
            )
            .property(
                JsSymbol::to_string_tag(),
                js_string!("Intl.RelativeTimeFormat"),
                Attribute::CONFIGURABLE,
            )
            .method(Self::resolved_options, js_string!("resolvedOptions"), 0)
            .method(Self::format, js_string!("format"), 2)
            .method(Self::format_to_parts, js_string!("formatToParts"), 2)
            .build();
    }

    fn get(intrinsics: &Intrinsics) -> JsObject {
        intrinsics
            .constructors()
            .relative_time_format()
            .constructor()
    }
}

impl BuiltInObject for RelativeTimeFormat {
    const NAME: JsString = js_string!("RelativeTimeFormat");
}

impl BuiltInConstructor for RelativeTimeFormat {
    const CONSTRUCTOR_ARGUMENTS: usize = 0;
    const PROTOTYPE_STORAGE_SLOTS: usize = 4;
    const CONSTRUCTOR_STORAGE_SLOTS: usize = 1;
    const STANDARD_CONSTRUCTOR: fn(&StandardConstructors) -> &StandardConstructor =
        StandardConstructors::relative_time_format;

    fn constructor(
        new_target: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        if new_target.is_undefined() {
            return Err(JsNativeError::typ()
                .with_message("Intl.RelativeTimeFormat requires new")
                .into());
        }
        let proto = get_prototype_from_constructor(
            new_target,
            StandardConstructors::relative_time_format,
            context,
        )?;
        let _proto_root = proto.clone().root();
        let requested = canonicalize_locale_list(args.get_or_undefined(0), context)?;
        let options = coerce_options_to_object(args.get_or_undefined(1), context)?;
        let _options_root = options.clone().root();
        let matcher =
            get_option(&options, js_string!("localeMatcher"), context)?.unwrap_or_default();
        let numbering = numbering_option(&options, context)?;
        let mut resolution = IntlOptions {
            matcher,
            service_options: NumberFormatLocaleOptions {
                numbering_system: numbering,
            },
        };
        let locale = resolve_locale::<Self>(requested, &mut resolution, context.intl_provider())?;
        let style = option(
            &options,
            "style",
            &["long", "short", "narrow"],
            Some("long"),
            context,
        )?;
        let numeric = option(
            &options,
            "numeric",
            &["always", "auto"],
            Some("always"),
            context,
        )?;
        let data = Self::new(
            locale,
            style,
            numeric,
            resolution.service_options.numbering_system,
            context,
        )?;
        Ok(
            JsObject::from_proto_and_data_with_shared_shape(context.root_shape(), proto, data)
                .into(),
        )
    }
}

impl RelativeTimeFormat {
    fn new(
        locale: Locale,
        style: String,
        numeric: String,
        numbering: Option<Value>,
        context: &mut Context,
    ) -> JsResult<Self> {
        let patterns = load::<OmoikaneRelativeTimePatternsV1>(context, &locale)?;
        let formatter = NativeNumberFormatter::new(
            context.intl_provider(),
            &locale,
            numbering
                .as_ref()
                .and_then(Value::as_single_subtag)
                .map(|nu| nu.as_str()),
            &UnitFormatOptions::Decimal,
            GroupingStrategy::Auto,
        )?;
        let plurals = NativePluralRules::try_new_cardinal_with_buffer_provider(
            context.intl_provider().erased_provider(),
            (&locale).into(),
        )
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
        // Only the intrinsic defaults are read; the caller's options are not reread.
        let defaults = coerce_options_to_object(&JsValue::undefined(), context)?;
        let _defaults_root = defaults.clone().root();
        let digits =
            DigitFormatOptions::from_options(&defaults, 0, 3, NotationKind::Standard, context)?;
        Ok(Self {
            locale,
            style,
            numeric,
            patterns,
            formatter,
            plurals,
            number_options: NumericFormatOptions {
                digits,
                sign_display: SignDisplay::Auto,
            },
        })
    }

    /// Coercions run in spec order after brand checking and before borrowing native state.
    fn partition(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<Vec<RelativePart>> {
        let object = this
            .as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ().with_message("receiver is not Intl.RelativeTimeFormat")
            })?;
        let _object_root = object.clone().root();
        let number = args.get_or_undefined(0).to_number(context)?;
        let unit = args
            .get_or_undefined(1)
            .to_string(context)?
            .to_std_string_escaped();
        if !number.is_finite() {
            return Err(JsNativeError::range()
                .with_message("relative time must be finite")
                .into());
        }
        let unit = singular_unit(&unit)?;
        let result = object.borrow().data().partition_native(number, unit);
        result
    }

    fn partition_native(&self, number: f64, unit: &'static str) -> JsResult<Vec<RelativePart>> {
        let records = &self.patterns.get().records;
        let style = if records
            .get(format!("{unit}/{}/future/other", self.style).as_str())
            .is_some()
        {
            self.style.as_str()
        } else {
            "long"
        };
        if self.numeric == "auto" && number.fract() == 0.0 {
            // ToString(-0) is "0". CLDR labels have small integral keys.
            let key = format!(
                "{unit}/{style}/auto/{:.0}",
                if number == 0.0 { 0.0 } else { number }
            );
            if let Some(label) = records.get(key.as_str()) {
                return Ok(vec![RelativePart {
                    part: NumberPart {
                        kind: "literal",
                        value: label.to_owned(),
                    },
                    unit: None,
                }]);
            }
        }
        let absolute = number.abs();
        let rounded = self.number_options.digits.format_f64(absolute);
        let category = match self.plurals.category_for(&rounded) {
            icu_plurals::PluralCategory::Zero => "zero",
            icu_plurals::PluralCategory::One => "one",
            icu_plurals::PluralCategory::Two => "two",
            icu_plurals::PluralCategory::Few => "few",
            icu_plurals::PluralCategory::Many => "many",
            icu_plurals::PluralCategory::Other => "other",
        };
        let tense = if number.is_sign_negative() {
            "past"
        } else {
            "future"
        };
        let key = format!("{unit}/{style}/{tense}/{category}");
        let other = format!("{unit}/{style}/{tense}/other");
        let pattern = records
            .get(key.as_str())
            .or_else(|| records.get(other.as_str()))
            .ok_or_else(|| JsNativeError::typ().with_message("missing relative time pattern"))?;
        // Arabic singular/dual patterns intentionally contain no numeric token.
        let Some((before, after)) = pattern.split_once("{0}") else {
            return Ok(vec![RelativePart {
                part: NumberPart {
                    kind: "literal",
                    value: pattern.to_owned(),
                },
                unit: None,
            }]);
        };
        if after.contains("{0}") {
            return Err(JsNativeError::typ()
                .with_message("invalid relative time pattern")
                .into());
        }
        let numeric = self
            .formatter
            .format(MathematicalValue::Finite(rounded), &self.number_options);
        let mut parts = Vec::with_capacity(numeric.len() + 2);
        append_literal(&mut parts, before);
        parts.extend(numeric.into_iter().map(|part| RelativePart {
            part,
            unit: Some(unit),
        }));
        append_literal(&mut parts, after);
        Ok(parts)
    }

    /// Native parts own their strings before any fresh JS objects are published.
    fn format(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let parts = Self::partition(this, args, context)?;
        let value: String = parts.iter().map(|part| part.part.value.as_str()).collect();
        Ok(js_string!(value).into())
    }

    fn format_to_parts(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let parts = Self::partition(this, args, context)?;
        let result = Array::array_create(0, None, context)?;
        let _result_root = result.clone().root();
        for (index, part) in parts.into_iter().enumerate() {
            let mut object = ObjectInitializer::new(context);
            object
                .property(
                    js_string!("type"),
                    js_string!(part.part.kind),
                    Attribute::all(),
                )
                .property(
                    js_string!("value"),
                    js_string!(part.part.value),
                    Attribute::all(),
                );
            if let Some(unit) = part.unit {
                object.property(js_string!("unit"), js_string!(unit), Attribute::all());
            }
            let object = object.build();
            let _part_root = object.clone().root();
            result.create_data_property_or_throw(index, object, context)?;
        }
        Ok(result.into())
    }

    fn resolved_options(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let object = this
            .as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ().with_message("receiver is not Intl.RelativeTimeFormat")
            })?;
        let (locale, style, numeric, numbering) = {
            let borrow = object.borrow();
            let data = borrow.data();
            (
                data.locale.to_string(),
                data.style.clone(),
                data.numeric.clone(),
                data.formatter.numbering_system().to_owned(),
            )
        };
        Ok(ObjectInitializer::new(context)
            .property(js_string!("locale"), js_string!(locale), Attribute::all())
            .property(js_string!("style"), js_string!(style), Attribute::all())
            .property(js_string!("numeric"), js_string!(numeric), Attribute::all())
            .property(
                js_string!("numberingSystem"),
                js_string!(numbering),
                Attribute::all(),
            )
            .build()
            .into())
    }

    fn supported_locales_of(
        _: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let requested = canonicalize_locale_list(args.get_or_undefined(0), context)?;
        filter_locales::<Self>(requested, args.get_or_undefined(1), context).map(JsValue::from)
    }
}

struct RelativePart {
    part: NumberPart,
    unit: Option<&'static str>,
}

fn append_literal(parts: &mut Vec<RelativePart>, value: &str) {
    if !value.is_empty() {
        parts.push(RelativePart {
            part: NumberPart {
                kind: "literal",
                value: value.to_owned(),
            },
            unit: None,
        });
    }
}

fn singular_unit(unit: &str) -> JsResult<&'static str> {
    match unit {
        "second" | "seconds" => Ok("second"),
        "minute" | "minutes" => Ok("minute"),
        "hour" | "hours" => Ok("hour"),
        "day" | "days" => Ok("day"),
        "week" | "weeks" => Ok("week"),
        "month" | "months" => Ok("month"),
        "quarter" | "quarters" => Ok("quarter"),
        "year" | "years" => Ok("year"),
        _ => Err(JsNativeError::range()
            .with_message("invalid relative time unit")
            .into()),
    }
}

/// Well-formed unsupported Unicode types fall back rather than throwing.
fn numbering_option(options: &JsObject, context: &mut Context) -> JsResult<Option<Value>> {
    let value = get_option::<JsString>(options, js_string!("numberingSystem"), context)?;
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.to_std_string_escaped();
    if !value.split('-').all(|part| {
        (3..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }) {
        return Err(JsNativeError::range()
            .with_message("invalid numbering system")
            .into());
    }
    // Numeric numbering-system IDs are single subtags. Keep an unsupported
    // multi-subtag type unsupported; ICU's boolean `true` normalization must
    // not turn e.g. `arab-true` into the supported `arab` identifier.
    if value.contains('-') {
        return Ok(None);
    }
    Value::try_from_str(&value).map(Some).map_err(|_| {
        JsNativeError::range()
            .with_message("invalid numbering system")
            .into()
    })
}
