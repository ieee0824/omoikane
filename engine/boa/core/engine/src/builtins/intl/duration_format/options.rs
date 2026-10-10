//! Owned duration formatting options, read in ECMA-402 order.

use crate::{
    Context, JsNativeError, JsObject, JsResult, JsValue,
    builtins::{
        intl::options::default_number_option,
        options::{OptionType, get_option},
    },
    js_string,
};

/// A duration field in formatting order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(super) enum DurationUnit {
    Years,
    Months,
    Weeks,
    Days,
    Hours,
    Minutes,
    Seconds,
    Milliseconds,
    Microseconds,
    Nanoseconds,
}

impl DurationUnit {
    /// All duration fields, from largest to smallest.
    pub(super) const ALL: [Self; 10] = [
        Self::Years,
        Self::Months,
        Self::Weeks,
        Self::Days,
        Self::Hours,
        Self::Minutes,
        Self::Seconds,
        Self::Milliseconds,
        Self::Microseconds,
        Self::Nanoseconds,
    ];

    /// The property name used by duration records and formatter options.
    pub(super) const fn plural(self) -> &'static str {
        match self {
            Self::Years => "years",
            Self::Months => "months",
            Self::Weeks => "weeks",
            Self::Days => "days",
            Self::Hours => "hours",
            Self::Minutes => "minutes",
            Self::Seconds => "seconds",
            Self::Milliseconds => "milliseconds",
            Self::Microseconds => "microseconds",
            Self::Nanoseconds => "nanoseconds",
        }
    }

    /// The unit name used by NumberFormat and duration parts.
    pub(super) const fn singular(self) -> &'static str {
        match self {
            Self::Years => "year",
            Self::Months => "month",
            Self::Weeks => "week",
            Self::Days => "day",
            Self::Hours => "hour",
            Self::Minutes => "minute",
            Self::Seconds => "second",
            Self::Milliseconds => "millisecond",
            Self::Microseconds => "microsecond",
            Self::Nanoseconds => "nanosecond",
        }
    }

    /// The index of this field in an owned duration array.
    pub(super) const fn index(self) -> usize {
        self as usize
    }

    /// Whether this field can be absorbed into a fractional larger unit.
    pub(super) const fn is_subsecond(self) -> bool {
        matches!(
            self,
            Self::Milliseconds | Self::Microseconds | Self::Nanoseconds
        )
    }

    const fn permits_style(self, style: UnitStyle) -> bool {
        match self {
            Self::Years | Self::Months | Self::Weeks | Self::Days => style.is_text(),
            Self::Hours | Self::Minutes | Self::Seconds => !matches!(style, UnitStyle::Fractional),
            Self::Milliseconds | Self::Microseconds | Self::Nanoseconds => {
                !matches!(style, UnitStyle::TwoDigit | UnitStyle::Fractional)
            }
        }
    }

    const fn updates_previous_style(self) -> bool {
        matches!(
            self,
            Self::Hours | Self::Minutes | Self::Seconds | Self::Milliseconds | Self::Microseconds
        )
    }
}

/// The overall display style.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum DurationStyle {
    Long,
    #[default]
    Short,
    Narrow,
    Digital,
}

impl DurationStyle {
    /// The string exposed by resolvedOptions.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Long => "long",
            Self::Short => "short",
            Self::Narrow => "narrow",
            Self::Digital => "digital",
        }
    }

    /// The width used by the unit list formatter.
    pub(super) const fn list_style(self) -> &'static str {
        match self {
            Self::Long => "long",
            Self::Short | Self::Digital => "short",
            Self::Narrow => "narrow",
        }
    }

    const fn text_style(self) -> UnitStyle {
        match self {
            Self::Long => UnitStyle::Long,
            Self::Short | Self::Digital => UnitStyle::Short,
            Self::Narrow => UnitStyle::Narrow,
        }
    }
}

impl OptionType for DurationStyle {
    fn from_value(value: JsValue, context: &mut Context) -> JsResult<Self> {
        match option_string(&value, context)?.as_str() {
            "long" => Ok(Self::Long),
            "short" => Ok(Self::Short),
            "narrow" => Ok(Self::Narrow),
            "digital" => Ok(Self::Digital),
            _ => Err(JsNativeError::range()
                .with_message("invalid duration style option")
                .into()),
        }
    }
}

/// An effective unit style. Fractional is internal and is exposed as numeric.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum UnitStyle {
    Long,
    #[default]
    Short,
    Narrow,
    Numeric,
    TwoDigit,
    Fractional,
}

impl UnitStyle {
    /// The user-visible style string.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Long => "long",
            Self::Short => "short",
            Self::Narrow => "narrow",
            Self::Numeric | Self::Fractional => "numeric",
            Self::TwoDigit => "2-digit",
        }
    }

    /// Whether this field is a standalone text unit.
    pub(super) const fn is_text(self) -> bool {
        matches!(self, Self::Long | Self::Short | Self::Narrow)
    }

    /// Whether this field is part of a numeric hours/minutes/seconds run.
    pub(super) const fn is_numeric(self) -> bool {
        matches!(self, Self::Numeric | Self::TwoDigit)
    }

    const fn continues_numeric(self) -> bool {
        self.is_numeric() || matches!(self, Self::Fractional)
    }
}

impl OptionType for UnitStyle {
    fn from_value(value: JsValue, context: &mut Context) -> JsResult<Self> {
        match option_string(&value, context)?.as_str() {
            "long" => Ok(Self::Long),
            "short" => Ok(Self::Short),
            "narrow" => Ok(Self::Narrow),
            "numeric" => Ok(Self::Numeric),
            "2-digit" => Ok(Self::TwoDigit),
            _ => Err(JsNativeError::range()
                .with_message("invalid duration unit style option")
                .into()),
        }
    }
}

/// Whether a zero-valued duration field is displayed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum UnitDisplay {
    #[default]
    Auto,
    Always,
}

impl UnitDisplay {
    /// The string exposed by resolvedOptions.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
        }
    }
}

impl OptionType for UnitDisplay {
    fn from_value(value: JsValue, context: &mut Context) -> JsResult<Self> {
        match option_string(&value, context)?.as_str() {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            _ => Err(JsNativeError::range()
                .with_message("invalid duration unit display option")
                .into()),
        }
    }
}

/// The effective style and zero-display policy for one duration field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct UnitOptions {
    pub(super) style: UnitStyle,
    pub(super) display: UnitDisplay,
}

/// Options owned by a DurationFormat instance, independent of its locale data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DurationOptions {
    pub(super) style: DurationStyle,
    pub(super) units: [UnitOptions; 10],
    pub(super) fractional_digits: Option<u8>,
}

impl DurationOptions {
    /// Reads style, unit pairs and fractionalDigits once, in ECMA-402 order.
    ///
    /// The caller resolves localeMatcher/numberingSystem before invoking this
    /// operation. Property access and string/number coercion can execute JS.
    pub(super) fn from_options(
        options: &JsObject,
        two_digit_hours: bool,
        context: &mut Context,
    ) -> JsResult<Self> {
        let _options_root = options.clone().root();
        let style = get_option(options, js_string!("style"), context)?.unwrap_or_default();
        let mut units = [UnitOptions::default(); 10];
        let mut previous = None;
        for unit in DurationUnit::ALL {
            let resolved =
                read_unit_options(unit, options, style, previous, two_digit_hours, context)?;
            units[unit.index()] = resolved;
            if unit.updates_previous_style() {
                previous = Some(resolved.style);
            }
        }
        let fractional_digits = read_fractional_digits(options, context)?;
        Ok(Self {
            style,
            units,
            fractional_digits,
        })
    }

    /// Returns the options for one field.
    pub(super) const fn unit(&self, unit: DurationUnit) -> UnitOptions {
        self.units[unit.index()]
    }
}

fn option_string(value: &JsValue, context: &mut Context) -> JsResult<String> {
    let _value_root = value.as_object().map(|object| object.root());
    Ok(value.to_string(context)?.to_std_string_escaped())
}

fn read_fractional_digits(options: &JsObject, context: &mut Context) -> JsResult<Option<u8>> {
    let value = options.get(js_string!("fractionalDigits"), context)?;
    let _value_root = value.as_object().map(|object| object.root());
    default_number_option(&value, 0u8, 9u8, context)
}

fn read_unit_options(
    unit: DurationUnit,
    options: &JsObject,
    base_style: DurationStyle,
    previous: Option<UnitStyle>,
    two_digit_hours: bool,
    context: &mut Context,
) -> JsResult<UnitOptions> {
    let requested = get_option::<UnitStyle>(options, js_string!(unit.plural()), context)?;
    if requested.is_some_and(|style| !unit.permits_style(style)) {
        return Err(JsNativeError::range()
            .with_message(format!("invalid {} style option", unit.plural()))
            .into());
    }
    let (mut style, mut display_default) = effective_default(unit, requested, base_style, previous);
    if style == UnitStyle::Numeric && unit.is_subsecond() {
        style = UnitStyle::Fractional;
        display_default = UnitDisplay::Auto;
    }
    let display = get_option(
        options,
        js_string!(format!("{}Display", unit.plural())),
        context,
    )?
    .unwrap_or(display_default);
    validate_chain(unit, style, display, previous)?;
    if unit == DurationUnit::Hours && two_digit_hours {
        style = UnitStyle::TwoDigit;
    }
    if matches!(unit, DurationUnit::Minutes | DurationUnit::Seconds)
        && previous.is_some_and(UnitStyle::is_numeric)
    {
        style = UnitStyle::TwoDigit;
    }
    Ok(UnitOptions { style, display })
}

fn effective_default(
    unit: DurationUnit,
    requested: Option<UnitStyle>,
    base_style: DurationStyle,
    previous: Option<UnitStyle>,
) -> (UnitStyle, UnitDisplay) {
    if let Some(style) = requested {
        return (style, UnitDisplay::Always);
    }
    if base_style == DurationStyle::Digital {
        return if matches!(
            unit,
            DurationUnit::Hours | DurationUnit::Minutes | DurationUnit::Seconds
        ) {
            (UnitStyle::Numeric, UnitDisplay::Always)
        } else if unit.is_subsecond() {
            (UnitStyle::Numeric, UnitDisplay::Auto)
        } else {
            (UnitStyle::Short, UnitDisplay::Auto)
        };
    }
    if previous.is_some_and(UnitStyle::continues_numeric) {
        let display = if matches!(unit, DurationUnit::Minutes | DurationUnit::Seconds) {
            UnitDisplay::Always
        } else {
            UnitDisplay::Auto
        };
        return (UnitStyle::Numeric, display);
    }
    (base_style.text_style(), UnitDisplay::Auto)
}

fn validate_chain(
    unit: DurationUnit,
    style: UnitStyle,
    display: UnitDisplay,
    previous: Option<UnitStyle>,
) -> JsResult<()> {
    let invalid_fraction = style == UnitStyle::Fractional && display == UnitDisplay::Always;
    let invalid_successor = match previous {
        Some(UnitStyle::Fractional) => style != UnitStyle::Fractional,
        Some(UnitStyle::Numeric | UnitStyle::TwoDigit) => !style.continues_numeric(),
        _ => false,
    };
    if invalid_fraction || invalid_successor {
        return Err(JsNativeError::range()
            .with_message(format!("incompatible {} duration options", unit.plural()))
            .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Source;

    fn read(script: &str, context: &mut Context) -> JsResult<DurationOptions> {
        let object = context
            .eval(Source::from_bytes(script))?
            .as_object()
            .unwrap();
        DurationOptions::from_options(&object, false, context)
    }

    fn eval_string(script: &str, context: &mut Context) -> String {
        context
            .eval(Source::from_bytes(script))
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped()
    }

    #[test]
    fn default_and_digital_zero_display_are_distinct() {
        let mut context = Context::default();
        let short = read("({})", &mut context).unwrap();
        assert_eq!(short.style, DurationStyle::Short);
        assert_eq!(short.fractional_digits, None);
        assert!(
            short.units.iter().all(|unit| {
                unit.style == UnitStyle::Short && unit.display == UnitDisplay::Auto
            })
        );

        let digital = read("({ style: 'digital' })", &mut context).unwrap();
        assert_eq!(digital.unit(DurationUnit::Days).display, UnitDisplay::Auto);
        assert_eq!(digital.unit(DurationUnit::Hours).style, UnitStyle::Numeric);
        assert_eq!(
            digital.unit(DurationUnit::Hours).display,
            UnitDisplay::Always
        );
        for unit in [DurationUnit::Minutes, DurationUnit::Seconds] {
            assert_eq!(digital.unit(unit).style, UnitStyle::TwoDigit);
            assert_eq!(digital.unit(unit).display, UnitDisplay::Always);
        }
        for unit in [
            DurationUnit::Milliseconds,
            DurationUnit::Microseconds,
            DurationUnit::Nanoseconds,
        ] {
            assert_eq!(digital.unit(unit).style, UnitStyle::Fractional);
            assert_eq!(digital.unit(unit).display, UnitDisplay::Auto);
        }
    }

    #[test]
    fn reads_and_coerces_options_once_in_order() {
        let mut context = Context::default();
        let options = read(
            r#"globalThis.events = [];
            new Proxy({
                style: { toString() { events.push('coerce style'); return 'long'; } },
                seconds: { toString() { events.push('coerce seconds'); return 'short'; } },
                fractionalDigits: { valueOf() { events.push('coerce fractionalDigits'); return 2; } }
            }, { get(target, name) { events.push('get ' + name); return target[name]; } })"#,
            &mut context,
        )
        .unwrap();
        assert_eq!(options.style, DurationStyle::Long);
        assert_eq!(options.fractional_digits, Some(2));
        assert_eq!(
            eval_string("events.join('|')", &mut context),
            "get style|coerce style|get years|get yearsDisplay|get months|get monthsDisplay|\
             get weeks|get weeksDisplay|get days|get daysDisplay|get hours|get hoursDisplay|\
             get minutes|get minutesDisplay|get seconds|coerce seconds|get secondsDisplay|\
             get milliseconds|get millisecondsDisplay|get microseconds|get microsecondsDisplay|\
             get nanoseconds|get nanosecondsDisplay|get fractionalDigits|coerce fractionalDigits"
        );
    }

    #[test]
    fn explicit_style_displays_zero_and_numeric_chain_pads_successors() {
        let mut context = Context::default();
        let text = read("({ hours: 'long' })", &mut context).unwrap();
        assert_eq!(text.unit(DurationUnit::Hours).display, UnitDisplay::Always);
        assert_eq!(text.unit(DurationUnit::Minutes).display, UnitDisplay::Auto);
        for style in ["numeric", "2-digit"] {
            let options = read(&format!("({{ hours: '{style}' }})"), &mut context).unwrap();
            assert_eq!(
                options.unit(DurationUnit::Minutes).style,
                UnitStyle::TwoDigit
            );
            assert_eq!(
                options.unit(DurationUnit::Seconds).style,
                UnitStyle::TwoDigit
            );
            assert_eq!(
                options.unit(DurationUnit::Nanoseconds).style,
                UnitStyle::Fractional
            );
        }
    }

    #[test]
    fn invalid_unit_style_stops_before_display_getter() {
        let mut context = Context::default();
        let error = read(
            "globalThis.events = []; new Proxy({ years: 'numeric' }, \
             { get(target, name) { events.push(name); return target[name]; } })",
            &mut context,
        )
        .unwrap_err();
        assert!(error.as_native().is_some_and(JsNativeError::is_range));
        assert_eq!(eval_string("events.join('|')", &mut context), "style|years");
    }

    #[test]
    fn numeric_and_fractional_styles_cannot_resume_text_or_display_fractional_zero() {
        let mut context = Context::default();
        for script in [
            "({ hours: 'numeric', minutes: 'long' })",
            "({ seconds: 'numeric', milliseconds: 'short' })",
            "({ milliseconds: 'numeric', microseconds: 'narrow' })",
            "({ milliseconds: 'numeric', millisecondsDisplay: 'always' })",
            "({ years: '2-digit' })",
            "({ nanoseconds: '2-digit' })",
            "({ seconds: 'fractional' })",
        ] {
            let error = read(script, &mut context).unwrap_err();
            assert!(
                error.as_native().is_some_and(JsNativeError::is_range),
                "{script}"
            );
        }
    }

    #[test]
    fn fractional_digits_accept_zero_and_floor_within_bounds() {
        let mut context = Context::default();
        for (script, expected) in [
            ("({ fractionalDigits: 0 })", Some(0)),
            ("({ fractionalDigits: 9 })", Some(9)),
            ("({ fractionalDigits: 2.9 })", Some(2)),
            ("({ fractionalDigits: '4' })", Some(4)),
            ("({ fractionalDigits: undefined })", None),
        ] {
            assert_eq!(
                read(script, &mut context).unwrap().fractional_digits,
                expected
            );
        }
        for script in [
            "({ fractionalDigits: -1 })",
            "({ fractionalDigits: 9.1 })",
            "({ fractionalDigits: NaN })",
            "({ fractionalDigits: Infinity })",
        ] {
            let error = read(script, &mut context).unwrap_err();
            assert!(error.as_native().is_some_and(JsNativeError::is_range));
        }
        let error = read("({ fractionalDigits: 1n })", &mut context).unwrap_err();
        assert!(error.as_native().is_some_and(JsNativeError::is_type));
    }

    #[test]
    fn option_coercion_type_errors_and_user_exceptions_propagate() {
        let mut context = Context::default();
        let error = read("({ style: Symbol('style') })", &mut context).unwrap_err();
        assert!(error.as_native().is_some_and(JsNativeError::is_type));
        let error = read(
            "globalThis.sentinel = {}; ({ get seconds() { throw sentinel; } })",
            &mut context,
        )
        .unwrap_err();
        let sentinel = context.eval(Source::from_bytes("sentinel")).unwrap();
        assert!(JsValue::same_value(
            &error.to_opaque(&mut context),
            &sentinel
        ));
    }

    #[test]
    fn getter_created_coercion_objects_survive_collection() {
        let mut context = Context::default();
        context
            .register_global_callable(
                js_string!("collect"),
                0,
                crate::NativeFunction::from_fn_ptr(|_, _, _| {
                    boa_gc::force_collect();
                    Ok(JsValue::undefined())
                }),
            )
            .unwrap();
        let options = read(
            r#"({
                get style() {
                    let style = 'long';
                    return { toString() { collect(); return style; } };
                },
                get fractionalDigits() {
                    let digits = 3;
                    return { valueOf() { collect(); return digits; } };
                }
            })"#,
            &mut context,
        )
        .unwrap();
        assert_eq!(options.style, DurationStyle::Long);
        assert_eq!(options.fractional_digits, Some(3));
    }
}
