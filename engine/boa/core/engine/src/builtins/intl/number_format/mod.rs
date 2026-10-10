use std::borrow::Cow;

use boa_gc::{Finalize, Trace, custom_trace};
use fixed_decimal::{Decimal, SignDisplay};
use icu_decimal::{
    options::GroupingStrategy, preferences::NumberingSystem, provider::DecimalDigitsV1,
};

mod backend;
mod compact;
mod compact_pattern;
mod currency;
mod data;
mod input;
mod notation;
mod options;
mod output;
mod parts;
mod pattern;
mod range;
mod range_affixes;
mod range_pattern;
mod units;
pub(crate) use backend::{NativeNumberFormatter, NumericFormatOptions, SharedNumberFormatCore};
use icu_locale::{
    Locale,
    extensions::unicode::{Value, key},
};
use icu_provider::DataMarkerAttributes;
pub(crate) use input::MathematicalValue;
use num_bigint::BigInt;
use num_traits::Num;
pub(crate) use options::*;
pub(crate) use parts::NumberPart;

use super::{
    Service,
    locale::{canonicalize_locale_list, filter_locales, resolve_locale, validate_extension},
    options::{IntlOptions, coerce_options_to_object},
};
use crate::{
    Context, JsArgs, JsData, JsNativeError, JsObject, JsResult, JsString, JsSymbol, JsValue,
    NativeFunction,
    builtins::{
        BuiltInConstructor, BuiltInObject, IntrinsicObject, builder::BuiltInBuilder,
        options::get_option, string::is_trimmable_whitespace,
    },
    context::intrinsics::{Intrinsics, StandardConstructor, StandardConstructors},
    js_string,
    object::{
        FunctionObjectBuilder, JsFunctionEdge, ObjectInitializer,
        internal_methods::get_prototype_from_constructor,
    },
    property::{Attribute, PropertyDescriptor},
    realm::Realm,
    string::StaticJsStrings,
};

#[cfg(test)]
mod tests;

/// The historical string booleans use the notation-dependent grouping default.
fn grouping_option(
    options: &JsObject,
    fallback: GroupingStrategy,
    context: &mut Context,
) -> JsResult<GroupingStrategy> {
    let value = options.get(js_string!("useGrouping"), context)?;
    if value.is_undefined() {
        return Ok(fallback);
    }
    if value.as_boolean() == Some(true) {
        return Ok(GroupingStrategy::Always);
    }
    if !value.to_boolean() {
        return Ok(GroupingStrategy::Never);
    }
    match value.to_string(context)?.to_std_string_escaped().as_str() {
        "min2" => Ok(GroupingStrategy::Min2),
        "auto" => Ok(GroupingStrategy::Auto),
        "always" => Ok(GroupingStrategy::Always),
        "true" | "false" => Ok(fallback),
        _ => Err(JsNativeError::range()
            .with_message("expected one of `min2`, `auto`, `always`, `true`, or `false`")
            .into()),
    }
}

#[derive(Debug, Finalize, JsData)]
pub(crate) struct NumberFormat {
    locale: Locale,
    formatter: NativeNumberFormatter,
    range_formatter: range::NativeNumberRangeFormatter,
    numbering_system: Option<Value>,
    unit_options: UnitFormatOptions,
    digit_options: DigitFormatOptions,
    notation: Notation,
    use_grouping: GroupingStrategy,
    sign_display: SignDisplay,
    bound_format: Option<JsFunctionEdge>,
}

// SAFETY: `bound_format` is the only traceable field.
unsafe impl Trace for NumberFormat {
    custom_trace!(this, mark, mark(&this.bound_format));
}

impl NumberFormat {
    /// [`FormatNumeric ( numberFormat, x )`][full] and [`FormatNumericToParts ( numberFormat, x )`][parts].
    ///
    /// [full]: https://tc39.es/ecma402/#sec-formatnumber
    /// [parts]: https://tc39.es/ecma402/#sec-formatnumbertoparts
    pub(crate) fn format(&self, value: Decimal) -> String {
        self.format_text(MathematicalValue::Finite(value))
    }

    fn format_text(&self, value: MathematicalValue) -> String {
        self.formatter.format_text(
            value,
            &NumericFormatOptions {
                digits: self.digit_options.clone(),
                sign_display: self.sign_display,
            },
        )
    }

    fn format_parts(&self, value: MathematicalValue) -> Vec<NumberPart> {
        self.formatter.format(
            value,
            &NumericFormatOptions {
                digits: self.digit_options.clone(),
                sign_display: self.sign_display,
            },
        )
    }
}

#[derive(Debug, Clone)]
pub(super) struct NumberFormatLocaleOptions {
    pub(super) numbering_system: Option<Value>,
}

impl Service for NumberFormat {
    type LangMarker = boa_intl_data::OmoikaneNumberSymbolsV1;

    type LocaleOptions = NumberFormatLocaleOptions;

    fn resolve(
        locale: &mut Locale,
        options: &mut Self::LocaleOptions,
        provider: &crate::context::icu::IntlProvider,
    ) {
        let supported = |nu: &Value| {
            NumberingSystem::try_from(nu.clone()).is_ok_and(|nu| {
                let attributes = DataMarkerAttributes::from_str_or_panic(nu.as_str());
                let root = icu_locale::langid!("und");
                validate_extension::<DecimalDigitsV1>(root.clone(), attributes, provider)
                    && (validate_extension::<Self::LangMarker>(
                        locale.id.clone(),
                        attributes,
                        provider,
                    ) || validate_extension::<boa_intl_data::OmoikaneGlobalNumberSymbolsV1>(
                        root, attributes, provider,
                    ))
            })
        };
        let requested = locale
            .extensions
            .unicode
            .keywords
            .get(&key!("nu"))
            .cloned()
            .filter(supported);
        let numbering_system = options
            .numbering_system
            .take()
            .filter(supported)
            .or_else(|| requested.clone());
        locale.extensions.unicode.clear();
        if let Some(nu) = requested.filter(|nu| Some(nu) == numbering_system.as_ref()) {
            locale.extensions.unicode.keywords.set(key!("nu"), nu);
        }

        options.numbering_system = numbering_system;
    }
}

impl IntrinsicObject for NumberFormat {
    fn init(realm: &Realm) {
        let get_format = BuiltInBuilder::callable(realm, Self::get_format)
            .name(js_string!("get format"))
            .build();

        BuiltInBuilder::from_standard_constructor::<Self>(realm)
            .static_method(
                Self::supported_locales_of,
                js_string!("supportedLocalesOf"),
                1,
            )
            .property(
                JsSymbol::to_string_tag(),
                js_string!("Intl.NumberFormat"),
                Attribute::CONFIGURABLE,
            )
            .accessor(
                js_string!("format"),
                Some(get_format),
                None,
                Attribute::CONFIGURABLE,
            )
            .method(Self::resolved_options, js_string!("resolvedOptions"), 0)
            .method(Self::format_to_parts, js_string!("formatToParts"), 1)
            .method(range::format_range, js_string!("formatRange"), 2)
            .method(
                range::format_range_to_parts,
                js_string!("formatRangeToParts"),
                2,
            )
            .build();
    }

    fn get(intrinsics: &Intrinsics) -> JsObject {
        Self::STANDARD_CONSTRUCTOR(intrinsics.constructors()).constructor()
    }
}

impl BuiltInObject for NumberFormat {
    const NAME: JsString = StaticJsStrings::NUMBER_FORMAT;
}

impl BuiltInConstructor for NumberFormat {
    const CONSTRUCTOR_ARGUMENTS: usize = 0;
    const PROTOTYPE_STORAGE_SLOTS: usize = 7;
    const CONSTRUCTOR_STORAGE_SLOTS: usize = 1;

    const STANDARD_CONSTRUCTOR: fn(&StandardConstructors) -> &StandardConstructor =
        StandardConstructors::number_format;

    /// [`Intl.NumberFormat ( [ locales [ , options ] ] )`][spec].
    ///
    /// [spec]: https://tc39.es/ecma402/#sec-intl.numberformat
    fn constructor(
        new_target: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let this = context
            .native_call_receiver()
            .unwrap_or_else(JsValue::undefined);
        let _this_root = this.as_object().map(|object| object.root());
        let locales = args.get_or_undefined(0);
        let options = args.get_or_undefined(1);

        // 1. If NewTarget is undefined, let newTarget be the active function object, else let newTarget be NewTarget.
        let new_target_inner = &if new_target.is_undefined() {
            context
                .active_function_object()
                .unwrap_or_else(|| {
                    context
                        .intrinsics()
                        .constructors()
                        .number_format()
                        .constructor()
                })
                .into()
        } else {
            new_target.clone()
        };

        // 2. Let numberFormat be ? OrdinaryCreateFromConstructor(newTarget, "%Intl.NumberFormat.prototype%", « [[InitializedNumberFormat]], [[Locale]], [[DataLocale]], [[NumberingSystem]], [[Style]], [[Unit]], [[UnitDisplay]], [[Currency]], [[CurrencyDisplay]], [[CurrencySign]], [[MinimumIntegerDigits]], [[MinimumFractionDigits]], [[MaximumFractionDigits]], [[MinimumSignificantDigits]], [[MaximumSignificantDigits]], [[RoundingType]], [[Notation]], [[CompactDisplay]], [[UseGrouping]], [[SignDisplay]], [[RoundingIncrement]], [[RoundingMode]], [[ComputedRoundingPriority]], [[TrailingZeroDisplay]], [[BoundFormat]] »).
        let prototype = get_prototype_from_constructor(
            new_target_inner,
            StandardConstructors::number_format,
            context,
        )?;
        let _prototype_root = prototype.clone().root();

        let number_format = Self::new(locales, options, context)?;

        let number_format = JsObject::from_proto_and_data_with_shared_shape(
            context.root_shape(),
            prototype,
            number_format,
        );
        let _number_format_root = number_format.clone().root();

        // 31. Return unused.

        // 4. If the implementation supports the normative optional constructor mode of 4.3 Note 1, then
        //     a. Let this be the this value.
        //     b. Return ? ChainNumberFormat(numberFormat, NewTarget, this).
        // ChainNumberFormat ( numberFormat, newTarget, this )
        // <https://tc39.es/ecma402/#sec-chainnumberformat>

        let Some(this_obj) = this.as_object() else {
            return Ok(number_format.into());
        };

        let constructor = context
            .intrinsics()
            .constructors()
            .number_format()
            .constructor();

        // 1. If newTarget is undefined and ? OrdinaryHasInstance(%Intl.NumberFormat%, this) is true, then
        if new_target.is_undefined()
            && JsValue::ordinary_has_instance(&constructor.into(), &this, context)?
        {
            let fallback_symbol = context
                .intrinsics()
                .objects()
                .intl()
                .borrow()
                .data()
                .fallback_symbol();

            // a. Perform ? DefinePropertyOrThrow(this, %Intl%.[[FallbackSymbol]], PropertyDescriptor{ [[Value]]: numberFormat, [[Writable]]: false, [[Enumerable]]: false, [[Configurable]]: false }).
            this_obj.define_property_or_throw(
                fallback_symbol,
                PropertyDescriptor::builder()
                    .value(number_format)
                    .writable(false)
                    .enumerable(false)
                    .configurable(false),
                context,
            )?;
            // b. Return this.
            Ok(this)
        } else {
            // 2. Return numberFormat.
            Ok(number_format.into())
        }
    }
}

impl NumberFormat {
    /// Formats a mathematical value using this formatter's internal options.
    pub(crate) fn format_value(&self, value: &JsValue, context: &mut Context) -> JsResult<JsValue> {
        let value = input::to_mathematical_value(value, context)?;
        Ok(js_string!(self.format_text(value)).into())
    }

    /// Formats fresh parts after value coercion and native borrowing have completed.
    fn format_to_parts(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let object = this
            .as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ().with_message("receiver is not an Intl.NumberFormat")
            })?;
        let _object_root = object.clone().root();
        let value = input::to_mathematical_value(args.get_or_undefined(0), context)?;
        let parts = object.borrow().data().format_parts(value);
        parts::parts_to_js(parts, context)
    }

    /// Creates a new instance of `NumberFormat`.
    pub(crate) fn new(
        locales: &JsValue,
        options: &JsValue,
        context: &mut Context,
    ) -> JsResult<Self> {
        let requested_locales = canonicalize_locale_list(locales, context)?;
        let options = coerce_options_to_object(options, context)?;
        let _options_root = options.clone().root();
        let matcher =
            get_option(&options, js_string!("localeMatcher"), context)?.unwrap_or_default();
        let numbering_system =
            get_option::<NumberingSystem>(&options, js_string!("numberingSystem"), context)?;
        let mut intl_options = IntlOptions {
            matcher,
            service_options: NumberFormatLocaleOptions {
                numbering_system: numbering_system.map(Value::from),
            },
        };
        let locale = resolve_locale::<Self>(
            requested_locales,
            &mut intl_options,
            context.intl_provider(),
        )?;
        let unit_options = UnitFormatOptions::from_options(&options, context)?;
        let notation = get_option(&options, js_string!("notation"), context)?.unwrap_or_default();
        let (minimum, maximum) = match &unit_options {
            UnitFormatOptions::Currency { currency, .. } if notation == NotationKind::Standard => {
                let digits = currency::fraction_digits(context.intl_provider(), *currency)?;
                (digits, digits)
            }
            UnitFormatOptions::Percent => (0, 0),
            _ => (0, 3),
        };
        let digit_options =
            DigitFormatOptions::from_options(&options, minimum, maximum, notation, context)?;
        let compact_display =
            get_option(&options, js_string!("compactDisplay"), context)?.unwrap_or_default();
        let notation = match notation {
            NotationKind::Standard => Notation::Standard,
            NotationKind::Scientific => Notation::Scientific,
            NotationKind::Engineering => Notation::Engineering,
            NotationKind::Compact => Notation::Compact {
                display: compact_display,
            },
        };
        let default_grouping = if matches!(notation, Notation::Compact { .. }) {
            GroupingStrategy::Min2
        } else {
            GroupingStrategy::Auto
        };
        let use_grouping = grouping_option(&options, default_grouping, context)?;
        let sign_display =
            get_option(&options, js_string!("signDisplay"), context)?.unwrap_or(SignDisplay::Auto);
        let numbering_system = intl_options.service_options.numbering_system;
        let formatter = NativeNumberFormatter::new_with_notation(
            context.intl_provider(),
            &locale,
            numbering_system
                .as_ref()
                .map(|value| value.to_string())
                .as_deref(),
            &unit_options,
            use_grouping,
            notation,
        )?;
        let numbering_system = Some(
            formatter
                .numbering_system()
                .parse::<Value>()
                .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?,
        );
        let range_formatter = range::NativeNumberRangeFormatter::new(
            context.intl_provider(),
            &locale,
            &formatter,
            &unit_options,
        )?;
        Ok(Self {
            locale,
            numbering_system,
            formatter,
            range_formatter,
            unit_options,
            digit_options,
            notation,
            use_grouping,
            sign_display,
            bound_format: None,
        })
    }

    /// [`Intl.NumberFormat.supportedLocalesOf ( locales [ , options ] )`][spec].
    ///
    /// Returns an array containing those of the provided locales that are supported in number format
    /// without having to fall back to the runtime's default locale.
    ///
    /// More information:
    ///  - [MDN documentation][mdn]
    ///
    /// [spec]: https://tc39.es/ecma402/#sec-intl.numberformat.supportedlocalesof
    /// [mdn]: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Intl/NumberFormat/supportedLocalesOf
    fn supported_locales_of(
        _: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let locales = args.get_or_undefined(0);
        let options = args.get_or_undefined(1);

        // 1. Let availableLocales be %Intl.NumberFormat%.[[AvailableLocales]].
        // 2. Let requestedLocales be ? CanonicalizeLocaleList(locales).
        let requested_locales = canonicalize_locale_list(locales, context)?;

        // 3. Return ? FilterLocales(availableLocales, requestedLocales, options).
        filter_locales::<Self>(requested_locales, options, context).map(JsValue::from)
    }

    /// [`get Intl.NumberFormat.prototype.format`][spec].
    ///
    /// [spec]: https://tc39.es/ecma402/#sec-intl.numberformat.prototype.format
    fn get_format(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        // 1. Let nf be the this value.
        // 2. If the implementation supports the normative optional constructor mode of 4.3 Note 1, then
        //     a. Set nf to ? UnwrapNumberFormat(nf).
        // 3. Perform ? RequireInternalSlot(nf, [[InitializedNumberFormat]]).
        let nf = unwrap_number_format(this, context)?;
        let _nf_root = nf.clone().root();
        if let Some(function) = nf.borrow().data().bound_format.clone() {
            return Ok(function.root().into());
        }
        let bound_format = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_copy_closure_with_captures(
                |_, args, nf, context| {
                    // Coercion may reenter this formatter; borrow only after it finishes.
                    let value = input::to_mathematical_value(args.get_or_undefined(0), context)?;
                    let text = nf.borrow().data().format_text(value);
                    Ok(js_string!(text).into())
                },
                nf.clone(),
            ),
        )
        .length(1)
        .build();
        nf.borrow_mut().data_mut().bound_format = Some(bound_format.clone().into_edge());
        Ok(bound_format.into())
    }

    /// [`Intl.NumberFormat.prototype.resolvedOptions ( )`][spec].
    ///
    /// Returns a new object with properties reflecting the locale and options computed during the
    /// construction of the current `Intl.NumberFormat` object.
    ///
    /// More information:
    ///  - [MDN documentation][mdn]
    ///
    /// [spec]: https://tc39.es/ecma402/#sec-intl.numberformat.prototype.resolvedoptions
    /// [mdn]: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Intl/NumberFormat/resolvedOptions
    fn resolved_options(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        // This function provides access to the locale and options computed during initialization of the object.

        // 1. Let nf be the this value.
        // 2. If the implementation supports the normative optional constructor mode of 4.3 Note 1, then
        //     a. Set nf to ? UnwrapNumberFormat(nf).
        // 3. Perform ? RequireInternalSlot(nf, [[InitializedNumberFormat]]).
        let nf = unwrap_number_format(this, context)?;
        let _nf_root = nf.clone().root();
        let nf = nf.borrow();
        let nf = nf.data();

        // 4. Let options be OrdinaryObjectCreate(%Object.prototype%).
        // 5. For each row of Table 12, except the header row, in table order, do
        //     a. Let p be the Property value of the current row.
        //     b. Let v be the value of nf's internal slot whose name is the Internal Slot value of the current row.
        //     c. If v is not undefined, then
        //         i. If there is a Conversion value in the current row, then
        //             1. Assert: The Conversion value of the current row is number.
        //             2. Set v to 𝔽(v).
        //         ii. Perform ! CreateDataPropertyOrThrow(options, p, v).
        let mut options = ObjectInitializer::new(context);
        options.property(
            js_string!("locale"),
            js_string!(nf.locale.to_string()),
            Attribute::all(),
        );
        if let Some(nu) = &nf.numbering_system {
            options.property(
                js_string!("numberingSystem"),
                js_string!(nu.to_string()),
                Attribute::all(),
            );
        }

        options.property(
            js_string!("style"),
            nf.unit_options.style().to_js_string(),
            Attribute::all(),
        );

        match &nf.unit_options {
            UnitFormatOptions::Currency {
                currency,
                display,
                sign,
            } => {
                options.property(
                    js_string!("currency"),
                    currency.to_js_string(),
                    Attribute::all(),
                );
                options.property(
                    js_string!("currencyDisplay"),
                    display.to_js_string(),
                    Attribute::all(),
                );
                options.property(
                    js_string!("currencySign"),
                    sign.to_js_string(),
                    Attribute::all(),
                );
            }
            UnitFormatOptions::Unit { unit, display } => {
                options.property(js_string!("unit"), unit.to_js_string(), Attribute::all());
                options.property(
                    js_string!("unitDisplay"),
                    display.to_js_string(),
                    Attribute::all(),
                );
            }
            UnitFormatOptions::Decimal | UnitFormatOptions::Percent => {}
        }

        options.property(
            js_string!("minimumIntegerDigits"),
            nf.digit_options.minimum_integer_digits,
            Attribute::all(),
        );

        if let Some(Extrema { minimum, maximum }) = nf.digit_options.rounding_type.fraction_digits()
        {
            options
                .property(
                    js_string!("minimumFractionDigits"),
                    minimum,
                    Attribute::all(),
                )
                .property(
                    js_string!("maximumFractionDigits"),
                    maximum,
                    Attribute::all(),
                );
        }

        if let Some(Extrema { minimum, maximum }) =
            nf.digit_options.rounding_type.significant_digits()
        {
            options
                .property(
                    js_string!("minimumSignificantDigits"),
                    minimum,
                    Attribute::all(),
                )
                .property(
                    js_string!("maximumSignificantDigits"),
                    maximum,
                    Attribute::all(),
                );
        }

        let use_grouping = match nf.use_grouping {
            GroupingStrategy::Auto => js_string!("auto").into(),
            GroupingStrategy::Never => JsValue::from(false),
            GroupingStrategy::Always => js_string!("always").into(),
            GroupingStrategy::Min2 => js_string!("min2").into(),
            _ => {
                return Err(JsNativeError::typ()
                    .with_message("unsupported useGrouping value")
                    .into());
            }
        };

        options
            .property(js_string!("useGrouping"), use_grouping, Attribute::all())
            .property(
                js_string!("notation"),
                nf.notation.kind().to_js_string(),
                Attribute::all(),
            );

        if let Notation::Compact { display } = nf.notation {
            options.property(
                js_string!("compactDisplay"),
                display.to_js_string(),
                Attribute::all(),
            );
        }

        let sign_display = match nf.sign_display {
            SignDisplay::Auto => js_string!("auto"),
            SignDisplay::Never => js_string!("never"),
            SignDisplay::Always => js_string!("always"),
            SignDisplay::ExceptZero => js_string!("exceptZero"),
            SignDisplay::Negative => js_string!("negative"),
            _ => {
                return Err(JsNativeError::typ()
                    .with_message("unsupported signDisplay value")
                    .into());
            }
        };

        options
            .property(js_string!("signDisplay"), sign_display, Attribute::all())
            .property(
                js_string!("roundingIncrement"),
                nf.digit_options.rounding_increment.to_u16(),
                Attribute::all(),
            )
            .property(
                js_string!("roundingMode"),
                js_string!(rounding_mode_name(nf.digit_options.rounding_mode)),
                Attribute::all(),
            )
            .property(
                js_string!("roundingPriority"),
                nf.digit_options.rounding_priority.to_js_string(),
                Attribute::all(),
            )
            .property(
                js_string!("trailingZeroDisplay"),
                nf.digit_options.trailing_zero_display.to_js_string(),
                Attribute::all(),
            );

        // 6. Return options.
        Ok(options.build().into())
    }
}

/// Abstract operation [`UnwrapNumberFormat ( nf )`][spec].
///
/// This also checks that the returned object is a `NumberFormat`, which skips the
/// call to `RequireInternalSlot`.
///
/// [spec]: https://tc39.es/ecma402/#sec-unwrapnumberformat
fn unwrap_number_format(nf: &JsValue, context: &mut Context) -> JsResult<JsObject<NumberFormat>> {
    // 1. If Type(nf) is not Object, throw a TypeError exception.
    let nf_o = nf.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("value was not an `Intl.NumberFormat` object")
    })?;

    if let Ok(nf) = nf_o.clone().downcast::<NumberFormat>() {
        // 3. Return nf.
        return Ok(nf);
    }

    // 2. If nf does not have an [[InitializedNumberFormat]] internal slot and ? OrdinaryHasInstance(%Intl.NumberFormat%, nf)
    //    is true, then
    let constructor = context
        .intrinsics()
        .constructors()
        .number_format()
        .constructor();
    if JsValue::ordinary_has_instance(&constructor.into(), nf, context)? {
        let fallback_symbol = context
            .intrinsics()
            .objects()
            .intl()
            .borrow()
            .data()
            .fallback_symbol();

        //    a. Return ? Get(nf, %Intl%.[[FallbackSymbol]]).
        if let Some(nf) = nf_o
            .get(fallback_symbol, context)?
            .as_object()
            .and_then(|o| o.downcast::<NumberFormat>().ok())
        {
            return Ok(nf);
        }
    }

    Err(JsNativeError::typ()
        .with_message("object was not an `Intl.NumberFormat` object")
        .into())
}

/// Abstract operation [`StringToNumber ( str )`][spec], but specialized for the conversion
/// to a `FixedDecimal`.
///
/// [spec]: https://tc39.es/ecma262/#sec-stringtonumber
pub(crate) fn js_string_to_fixed_decimal(string: &JsString) -> Option<Decimal> {
    // 1. Let text be ! StringToCodePoints(str).
    // 2. Let literal be ParseText(text, StringNumericLiteral).
    let Ok(string) = string.to_std_string() else {
        // 3. If literal is a List of errors, return NaN.
        return None;
    };
    // 4. Return StringNumericValue of literal.
    let string = string.trim_matches(is_trimmable_whitespace);
    match string {
        "" => return Some(Decimal::from(0)),
        "-Infinity" | "Infinity" | "+Infinity" => return None,
        _ => {}
    }

    let mut s = string.bytes();
    let base = match (s.next(), s.next()) {
        (Some(b'0'), Some(b'b' | b'B')) => Some(2),
        (Some(b'0'), Some(b'o' | b'O')) => Some(8),
        (Some(b'0'), Some(b'x' | b'X')) => Some(16),
        // Make sure that no further variants of "infinity" are parsed.
        (Some(b'i' | b'I'), _) => {
            return None;
        }
        _ => None,
    };

    // Parse numbers that begin with `0b`, `0o` and `0x`.
    let s = if let Some(base) = base {
        let string = &string[2..];
        if string.is_empty() {
            return None;
        }
        let int = BigInt::from_str_radix(string, base).ok()?;
        let int_str = int.to_string();

        Cow::Owned(int_str)
    } else {
        Cow::Borrowed(string)
    };

    Decimal::try_from_str(&s).ok()
}
