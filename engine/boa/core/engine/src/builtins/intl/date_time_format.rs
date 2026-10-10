//! This module implements the global `Intl.DateTimeFormat` object.
//!
//! `Intl.DateTimeFormat` is a built-in object that has properties and methods for date and time i18n.
//!
//! More information:
//!  - [ECMAScript reference][spec]
//!
//! [spec]: https://tc39.es/ecma402/#datetimeformat-objects

use crate::{
    Context, JsArgs, JsData, JsResult, JsString, JsValue,
    builtins::{
        BuiltInBuilder, BuiltInConstructor, BuiltInObject, IntrinsicObject, OrdinaryObject,
        options::OptionType,
    },
    context::intrinsics::{Intrinsics, StandardConstructor, StandardConstructors},
    error::JsNativeError,
    js_string,
    object::{JsObject, internal_methods::get_prototype_from_constructor},
    realm::Realm,
    string::StaticJsStrings,
};

use boa_gc::{Finalize, Trace, custom_trace};
use icu_calendar::preferences::CalendarAlgorithm;
use icu_datetime::preferences::HourCycle;
use icu_locale::extensions::unicode::Value;

mod backend;
mod calendar_patterns;
mod calendar_period;
mod candidates;
mod format;
mod hour_cycle;
mod initialization;
mod match_fields;
mod matcher;
mod negotiation;
mod options;
mod parts;
mod pattern;
mod range;
mod range_data;
mod range_fields;
mod range_parts;
mod range_pattern;
mod range_selection;
mod resolution;
mod timezone;

#[cfg(test)]
mod tests;

/// JavaScript `Intl.DateTimeFormat` object.
#[derive(Debug, Clone, Finalize, JsData)]
pub(crate) struct DateTimeFormat {
    locale: icu_locale::Locale,
    time_zone: timezone::ResolvedTimeZone,
    components: options::Components,
    backend: backend::DateTimeBackend,
    bound_format: Option<crate::object::JsFunctionEdge>,
}

// SAFETY: bound_format is the only GC-managed field. ICU data and JsString
// component values own their data independently of the JavaScript GC.
unsafe impl Trace for DateTimeFormat {
    custom_trace!(this, mark, mark(&this.bound_format));
}

impl IntrinsicObject for DateTimeFormat {
    fn init(realm: &Realm) {
        let format = BuiltInBuilder::callable(realm, Self::get_format)
            .name(js_string!("get format"))
            .build();
        BuiltInBuilder::from_standard_constructor::<Self>(realm)
            .property(
                crate::JsSymbol::to_string_tag(),
                js_string!("Intl.DateTimeFormat"),
                crate::property::Attribute::CONFIGURABLE,
            )
            .accessor(
                js_string!("format"),
                Some(format),
                None,
                crate::property::Attribute::CONFIGURABLE,
            )
            .method(Self::resolved_options, js_string!("resolvedOptions"), 0)
            .method(Self::format_to_parts, js_string!("formatToParts"), 1)
            .method(Self::format_range, js_string!("formatRange"), 2)
            .method(
                Self::format_range_to_parts,
                js_string!("formatRangeToParts"),
                2,
            )
            .static_method(
                Self::supported_locales_of,
                js_string!("supportedLocalesOf"),
                1,
            )
            .build();
    }

    fn get(intrinsics: &Intrinsics) -> JsObject {
        Self::STANDARD_CONSTRUCTOR(intrinsics.constructors()).constructor()
    }
}

impl BuiltInObject for DateTimeFormat {
    const NAME: JsString = StaticJsStrings::DATE_TIME_FORMAT;
}

impl BuiltInConstructor for DateTimeFormat {
    const CONSTRUCTOR_ARGUMENTS: usize = 0;
    const PROTOTYPE_STORAGE_SLOTS: usize = 7;
    const CONSTRUCTOR_STORAGE_SLOTS: usize = 1;

    const STANDARD_CONSTRUCTOR: fn(&StandardConstructors) -> &StandardConstructor =
        StandardConstructors::date_time_format;
    /// The `Intl.DateTimeFormat` constructor is the `%DateTimeFormat%` intrinsic object and a standard built-in property of the `Intl` object.
    ///
    /// More information:
    ///  - [ECMAScript reference][spec]
    ///  - [MDN documentation][mdn]
    ///
    /// [spec]: https://tc39.es/ecma402/#datetimeformat-objects
    /// [mdn]: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Intl/DateTimeFormat
    fn constructor(
        new_target: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        // 1. If NewTarget is undefined, let newTarget be the active function object, else let newTarget be NewTarget.
        let new_target = &if new_target.is_undefined() {
            context
                .active_function_object()
                .unwrap_or_else(|| {
                    context
                        .intrinsics()
                        .constructors()
                        .date_time_format()
                        .constructor()
                })
                .into()
        } else {
            new_target.clone()
        };
        let prototype = get_prototype_from_constructor(
            new_target,
            StandardConstructors::date_time_format,
            context,
        )?;
        let _prototype_root = prototype.clone().root();
        let data = Self::new(args.get_or_undefined(0), args.get_or_undefined(1), context)?;
        let date_time_format =
            JsObject::from_proto_and_data_with_shared_shape(context.root_shape(), prototype, data);

        // 5. Return dateTimeFormat.
        Ok(date_time_format.into())
    }
}

/// Represents the `required` and `defaults` arguments in the abstract operation
/// `toDateTimeOptions`.
///
/// Since `required` and `defaults` differ only in the `any` and `all` variants,
/// we combine both in a single variant `AnyAll`.
#[allow(unused)]
#[derive(Debug, PartialEq)]
pub(crate) enum DateTimeReqs {
    Date,
    Time,
    AnyAll,
}

/// The abstract operation `toDateTimeOptions` is called with arguments `options`, `required` and
/// `defaults`.
///
/// More information:
///  - [ECMAScript reference][spec]
///
/// [spec]: https://tc39.es/ecma402/#sec-todatetimeoptions
#[allow(unused)]
pub(crate) fn to_date_time_options(
    options: &JsValue,
    required: &DateTimeReqs,
    defaults: &DateTimeReqs,
    context: &mut Context,
) -> JsResult<JsObject> {
    // 1. If options is undefined, let options be null;
    // otherwise let options be ? ToObject(options).
    // 2. Let options be ! OrdinaryObjectCreate(options).
    let options = if options.is_undefined() {
        None
    } else {
        Some(options.to_object(context)?)
    };
    let options = JsObject::from_proto_and_data_with_shared_shape(
        context.root_shape(),
        options,
        OrdinaryObject,
    );

    // 3. Let needDefaults be true.
    let mut need_defaults = true;

    // 4. If required is "date" or "any", then
    if [DateTimeReqs::Date, DateTimeReqs::AnyAll].contains(required) {
        // a. For each property name prop of « "weekday", "year", "month", "day" », do
        for property in [
            js_string!("weekday"),
            js_string!("year"),
            js_string!("month"),
            js_string!("day"),
        ] {
            // i. Let value be ? Get(options, prop).
            let value = options.get(property, context)?;

            // ii. If value is not undefined, let needDefaults be false.
            if !value.is_undefined() {
                need_defaults = false;
            }
        }
    }

    // 5. If required is "time" or "any", then
    if [DateTimeReqs::Time, DateTimeReqs::AnyAll].contains(required) {
        // a. For each property name prop of « "dayPeriod", "hour", "minute", "second",
        // "fractionalSecondDigits" », do
        for property in [
            js_string!("dayPeriod"),
            js_string!("hour"),
            js_string!("minute"),
            js_string!("second"),
            js_string!("fractionalSecondDigits"),
        ] {
            // i. Let value be ? Get(options, prop).
            let value = options.get(property, context)?;

            // ii. If value is not undefined, let needDefaults be false.
            if !value.is_undefined() {
                need_defaults = false;
            }
        }
    }

    // 6. Let dateStyle be ? Get(options, "dateStyle").
    let date_style = options.get(js_string!("dateStyle"), context)?;

    // 7. Let timeStyle be ? Get(options, "timeStyle").
    let time_style = options.get(js_string!("timeStyle"), context)?;

    // 8. If dateStyle is not undefined or timeStyle is not undefined, let needDefaults be false.
    if !date_style.is_undefined() || !time_style.is_undefined() {
        need_defaults = false;
    }

    // 9. If required is "date" and timeStyle is not undefined, then
    if required == &DateTimeReqs::Date && !time_style.is_undefined() {
        // a. Throw a TypeError exception.
        return Err(JsNativeError::typ()
            .with_message("'date' is required, but timeStyle was defined")
            .into());
    }

    // 10. If required is "time" and dateStyle is not undefined, then
    if required == &DateTimeReqs::Time && !date_style.is_undefined() {
        // a. Throw a TypeError exception.
        return Err(JsNativeError::typ()
            .with_message("'time' is required, but dateStyle was defined")
            .into());
    }

    // 11. If needDefaults is true and defaults is either "date" or "all", then
    if need_defaults && [DateTimeReqs::Date, DateTimeReqs::AnyAll].contains(defaults) {
        // a. For each property name prop of « "year", "month", "day" », do
        for property in [js_string!("year"), js_string!("month"), js_string!("day")] {
            // i. Perform ? CreateDataPropertyOrThrow(options, prop, "numeric").
            options.create_data_property_or_throw(property, js_string!("numeric"), context)?;
        }
    }

    // 12. If needDefaults is true and defaults is either "time" or "all", then
    if need_defaults && [DateTimeReqs::Time, DateTimeReqs::AnyAll].contains(defaults) {
        // a. For each property name prop of « "hour", "minute", "second" », do
        for property in [
            js_string!("hour"),
            js_string!("minute"),
            js_string!("second"),
        ] {
            // i. Perform ? CreateDataPropertyOrThrow(options, prop, "numeric").
            options.create_data_property_or_throw(property, js_string!("numeric"), context)?;
        }
    }

    // 13. Return options.
    Ok(options)
}

impl OptionType for CalendarAlgorithm {
    fn from_value(value: JsValue, context: &mut Context) -> JsResult<Self> {
        let s = value.to_string(context)?.to_std_string_escaped();
        Value::try_from_str(&s)
            .ok()
            .and_then(|v| CalendarAlgorithm::try_from(&v).ok())
            .ok_or_else(|| {
                JsNativeError::range()
                    .with_message(format!("provided calendar `{s}` is invalid"))
                    .into()
            })
    }
}

// TODO: track https://github.com/unicode-org/icu4x/issues/6597 and
// https://github.com/tc39/ecma402/issues/1002 for resolution on
// `HourCycle::H24`.
impl OptionType for HourCycle {
    fn from_value(value: JsValue, context: &mut Context) -> JsResult<Self> {
        match value.to_string(context)?.to_std_string_escaped().as_str() {
            "h11" => Ok(HourCycle::H11),
            "h12" => Ok(HourCycle::H12),
            "h23" => Ok(HourCycle::H23),
            _ => Err(JsNativeError::range()
                .with_message("provided hour cycle was not `h11`, `h12` or `h23`")
                .into()),
        }
    }
}
