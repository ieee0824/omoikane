//! Duration input conversion and exact, locale-independent arithmetic.

use fixed_decimal::Decimal;

use super::options::DurationUnit;
use crate::{Context, JsNativeError, JsResult, JsValue, js_string};

const SECOND_NANOSECONDS: i128 = 1_000_000_000;
const TIME_LIMIT_NANOSECONDS: u128 = (1u128 << 53) * SECOND_NANOSECONDS as u128;
const CALENDAR_LIMIT: u128 = 1u128 << 32;

/// An owned, validated duration record in largest-to-smallest unit order.
///
/// All values are integers. Negative zero is normalized to zero. Values retain
/// the exact integer represented by an input Number, including unsafe integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DurationRecord {
    values: [i128; 10],
}

impl DurationRecord {
    /// Converts a duration-like value without consulting locale or environment.
    ///
    /// Objects are read once in alphabetical field order. Getters and ToNumber
    /// coercions can execute JS. This includes branded Temporal values: ECMA-402
    /// requires ordinary property access rather than reading their internal
    /// duration directly. String inputs are rejected with a `RangeError`.
    pub(super) fn from_input(input: &JsValue, context: &mut Context) -> JsResult<Self> {
        let Some(object) = input.as_object() else {
            return Err(if input.is_string() {
                JsNativeError::range().with_message("invalid duration string")
            } else {
                JsNativeError::typ().with_message("duration input must be an object")
            }
            .into());
        };
        let _input_root = object.clone().root();

        use DurationUnit::{
            Days, Hours, Microseconds, Milliseconds, Minutes, Months, Nanoseconds, Seconds, Weeks,
            Years,
        };
        let mut values = [0f64; 10];
        let mut any_defined = false;
        for unit in [
            Days,
            Hours,
            Microseconds,
            Milliseconds,
            Minutes,
            Months,
            Nanoseconds,
            Seconds,
            Weeks,
            Years,
        ] {
            let value = object.get(js_string!(unit.plural()), context)?;
            if value.is_undefined() {
                continue;
            }
            any_defined = true;
            let _value_root = value.as_object().map(|object| object.root());
            let number = value.to_number(context)?;
            if !number.is_finite() || number.fract() != 0.0 {
                return Err(JsNativeError::range()
                    .with_message(format!("{} must be a finite integer", unit.plural()))
                    .into());
            }
            values[unit.index()] = number;
        }
        if !any_defined {
            return Err(JsNativeError::typ()
                .with_message("duration input must define a duration field")
                .into());
        }
        Self::from_numbers(values)
    }

    /// Creates an owned record, rejecting inconsistent signs and duration bounds.
    ///
    /// Exact checked arithmetic avoids rounding valid near-limit durations up to
    /// the limit or losing subsecond contributions to large integer seconds.
    pub(super) fn new(values: [i128; 10]) -> JsResult<Self> {
        let result = Self { values };
        result.validate()?;
        Ok(result)
    }

    /// Returns one exact integer duration field.
    pub(super) const fn value(&self, unit: DurationUnit) -> i128 {
        self.values[unit.index()]
    }

    /// The common sign of the nonzero fields, or zero for an all-zero duration.
    pub(super) fn sign(&self) -> i8 {
        self.values
            .iter()
            .find(|value| **value != 0)
            .map_or(0, |value| value.signum() as i8)
    }

    /// Creates a decimal containing one integer unit, without rounding.
    pub(super) fn decimal_value(&self, unit: DurationUnit) -> Decimal {
        Decimal::from(self.value(unit))
    }

    /// Adds smaller subsecond fields to seconds, milliseconds or microseconds.
    ///
    /// Other units are returned unchanged. This operation preserves the exact
    /// mathematical value; the formatting backend later applies its digit policy.
    pub(super) fn fractional_value(&self, unit: DurationUnit) -> JsResult<Decimal> {
        let (weights, scale): (&[i128], i16) = match unit {
            DurationUnit::Seconds => (&[1_000_000_000, 1_000_000, 1_000, 1], 9),
            DurationUnit::Milliseconds => (&[1_000_000, 1_000, 1], 6),
            DurationUnit::Microseconds => (&[1_000, 1], 3),
            _ => return Ok(self.decimal_value(unit)),
        };
        let value = checked_weighted_sum(&self.values[unit.index()..], weights)?;
        let mut decimal = Decimal::from(value);
        decimal.multiply_pow10(-scale);
        decimal.trim_end();
        Ok(decimal)
    }

    /// Copies the Number-valued internal fields of a branded Temporal.Duration.
    #[cfg(feature = "temporal")]
    pub(super) fn from_temporal(duration: &temporal_rs::Duration) -> JsResult<Self> {
        Self::from_numbers([
            duration.years() as f64,
            duration.months() as f64,
            duration.weeks() as f64,
            duration.days() as f64,
            duration.hours() as f64,
            duration.minutes() as f64,
            duration.seconds() as f64,
            duration.milliseconds() as f64,
            duration.microseconds() as f64,
            duration.nanoseconds() as f64,
        ])
    }

    fn from_numbers(numbers: [f64; 10]) -> JsResult<Self> {
        let mut values = [0i128; 10];
        for (value, number) in values.iter_mut().zip(numbers) {
            // i128::MAX rounds to 2^127 as f64. Every valid duration is much
            // smaller; reject before a saturating cast can change the input.
            if !number.is_finite() || number.fract() != 0.0 || number.abs() >= i128::MAX as f64 {
                return Err(JsNativeError::range()
                    .with_message("duration field is outside the valid range")
                    .into());
            }
            *value = number as i128;
        }
        Self::new(values)
    }

    fn validate(&self) -> JsResult<()> {
        let sign = self.sign();
        if self
            .values
            .iter()
            .any(|value| *value != 0 && value.signum() as i8 != sign)
        {
            return Err(JsNativeError::range()
                .with_message("duration fields must have the same sign")
                .into());
        }
        if self.values[..3]
            .iter()
            .any(|value| value.unsigned_abs() >= CALENDAR_LIMIT)
        {
            return Err(JsNativeError::range()
                .with_message("duration years, months and weeks must be smaller than 2^32")
                .into());
        }
        let nanoseconds = checked_weighted_sum(
            &self.values[DurationUnit::Days.index()..],
            &[
                86_400_000_000_000,
                3_600_000_000_000,
                60_000_000_000,
                SECOND_NANOSECONDS,
                1_000_000,
                1_000,
                1,
            ],
        )?;
        if nanoseconds.unsigned_abs() >= TIME_LIMIT_NANOSECONDS {
            return Err(JsNativeError::range()
                .with_message("duration time must be smaller than 2^53 seconds")
                .into());
        }
        Ok(())
    }
}

fn checked_weighted_sum(values: &[i128], weights: &[i128]) -> JsResult<i128> {
    values
        .iter()
        .zip(weights)
        .try_fold(0i128, |sum, (value, weight)| {
            value
                .checked_mul(*weight)
                .and_then(|term| sum.checked_add(term))
        })
        .ok_or_else(|| {
            JsNativeError::range()
                .with_message("duration arithmetic is outside the valid range")
                .into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Source;

    fn read(script: &str, context: &mut Context) -> JsResult<DurationRecord> {
        let value = context.eval(Source::from_bytes(script))?;
        DurationRecord::from_input(&value, context)
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
    fn reads_fields_once_alphabetically_and_coerces_before_next_getter() {
        let mut context = Context::default();
        let duration = read(
            r#"globalThis.events = [];
            new Proxy({
                days: { valueOf() { events.push('coerce days'); return 1; } },
                months: { valueOf() { events.push('coerce months'); return 2; } }
            }, { get(target, name) { events.push('get ' + name); return target[name]; } })"#,
            &mut context,
        )
        .unwrap();
        assert_eq!(duration.value(DurationUnit::Days), 1);
        assert_eq!(duration.value(DurationUnit::Months), 2);
        assert_eq!(
            eval_string("events.join('|')", &mut context),
            "get days|coerce days|get hours|get microseconds|get milliseconds|get minutes|\
             get months|coerce months|get nanoseconds|get seconds|get weeks|get years"
        );
    }

    #[test]
    fn bounds_are_validated_after_remaining_getters() {
        let mut context = Context::default();
        let error = read(
            "globalThis.events = []; new Proxy({ days: 1e300 }, \
             { get(target, name) { events.push(name); return target[name]; } })",
            &mut context,
        )
        .unwrap_err();
        assert!(error.as_native().is_some_and(JsNativeError::is_range));
        assert_eq!(
            eval_string("events.join('|')", &mut context),
            "days|hours|microseconds|milliseconds|minutes|months|nanoseconds|seconds|weeks|years"
        );
    }

    #[test]
    fn fractional_number_throws_before_next_field_and_user_exception_is_preserved() {
        let mut context = Context::default();
        let error = read(
            "globalThis.events = []; new Proxy({ days: 0.5 }, \
             { get(target, name) { events.push(name); return target[name]; } })",
            &mut context,
        )
        .unwrap_err();
        assert!(error.as_native().is_some_and(JsNativeError::is_range));
        assert_eq!(eval_string("events.join('|')", &mut context), "days");

        let error = read(
            "globalThis.sentinel = {}; \
             ({ days: { valueOf() { throw sentinel; } }, get hours() { throw 'later'; } })",
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
    fn empty_and_nonobject_inputs_have_type_errors() {
        let mut context = Context::default();
        for script in [
            "undefined",
            "null",
            "1",
            "true",
            "({})",
            "({ years: undefined })",
        ] {
            let error = read(script, &mut context).unwrap_err();
            assert!(
                error.as_native().is_some_and(JsNativeError::is_type),
                "{script}"
            );
        }
        for script in ["({ seconds: 1n })", "({ seconds: Symbol('seconds') })"] {
            let error = read(script, &mut context).unwrap_err();
            assert!(error.as_native().is_some_and(JsNativeError::is_type));
        }
    }

    #[test]
    fn finite_integer_signs_and_negative_zero_follow_record_contract() {
        let mut context = Context::default();
        let zero = read("({ seconds: -0 })", &mut context).unwrap();
        assert_eq!(zero.sign(), 0);
        assert_eq!(zero.decimal_value(DurationUnit::Seconds).to_string(), "0");
        let coerced = read(
            "({ days: '2', seconds: true, nanoseconds: null })",
            &mut context,
        )
        .unwrap();
        assert_eq!(coerced.value(DurationUnit::Days), 2);
        assert_eq!(coerced.value(DurationUnit::Seconds), 1);
        for script in [
            "({ years: 1, seconds: -1 })",
            "({ seconds: NaN })",
            "({ microseconds: Infinity })",
            "({ nanoseconds: -Infinity })",
            "({ milliseconds: 1.5 })",
        ] {
            let error = read(script, &mut context).unwrap_err();
            assert!(
                error.as_native().is_some_and(JsNativeError::is_range),
                "{script}"
            );
        }
    }

    #[test]
    fn calendar_and_exact_time_limits_accept_the_last_valid_value() {
        for unit in [
            DurationUnit::Years,
            DurationUnit::Months,
            DurationUnit::Weeks,
        ] {
            let mut values = [0i128; 10];
            values[unit.index()] = (1i128 << 32) - 1;
            assert!(DurationRecord::new(values).is_ok());
            values[unit.index()] += 1;
            assert!(DurationRecord::new(values).is_err());
        }
        let mut values = [0i128; 10];
        values[DurationUnit::Seconds.index()] = (1i128 << 53) - 1;
        values[DurationUnit::Nanoseconds.index()] = 999_999_999;
        assert!(DurationRecord::new(values).is_ok());
        values[DurationUnit::Nanoseconds.index()] += 1;
        assert!(DurationRecord::new(values).is_err());
        for value in &mut values {
            *value = -*value;
        }
        assert!(DurationRecord::new(values).is_err());
        values[DurationUnit::Nanoseconds.index()] += 1;
        assert!(DurationRecord::new(values).is_ok());
    }

    #[test]
    fn arithmetic_overflow_is_a_range_error() {
        let mut values = [0i128; 10];
        values[DurationUnit::Days.index()] = i128::MAX;
        let error = DurationRecord::new(values).unwrap_err();
        assert!(error.as_native().is_some_and(JsNativeError::is_range));
        values[DurationUnit::Days.index()] = i128::MIN;
        let error = DurationRecord::new(values).unwrap_err();
        assert!(error.as_native().is_some_and(JsNativeError::is_range));
    }

    #[test]
    fn fractions_preserve_nanoseconds_beyond_double_precision() {
        let mut context = Context::default();
        for (script, expected) in [
            (
                "({ seconds: 10000000, nanoseconds: 1 })",
                "10000000.000000001",
            ),
            (
                "({ seconds: 1, milliseconds: 2, microseconds: 3, nanoseconds: Number.MAX_SAFE_INTEGER })",
                "9007200.256743991",
            ),
            (
                "({ milliseconds: 4503599627370497_000, microseconds: 4503599627370495_000000 })",
                "9007199254740991.975424",
            ),
            ("({ nanoseconds: -1 })", "-0.000000001"),
        ] {
            let record = read(script, &mut context).unwrap();
            assert_eq!(
                record
                    .fractional_value(DurationUnit::Seconds)
                    .unwrap()
                    .to_string(),
                expected
            );
        }
    }

    #[test]
    fn fractional_milliseconds_and_microseconds_keep_their_own_unit() {
        let mut context = Context::default();
        let duration = read(
            "({ milliseconds: 1, microseconds: 2, nanoseconds: 3 })",
            &mut context,
        )
        .unwrap();
        assert_eq!(
            duration
                .fractional_value(DurationUnit::Milliseconds)
                .unwrap()
                .to_string(),
            "1.002003"
        );
        assert_eq!(
            duration
                .fractional_value(DurationUnit::Microseconds)
                .unwrap()
                .to_string(),
            "2.003"
        );
    }

    #[test]
    fn getter_created_number_coercion_survives_collection() {
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
        let record = read(
            "({ get seconds() { let seconds = 7; \
             return { valueOf() { collect(); return seconds; } }; } })",
            &mut context,
        )
        .unwrap();
        assert_eq!(record.value(DurationUnit::Seconds), 7);
    }

    #[cfg(feature = "temporal")]
    #[test]
    fn temporal_inputs_use_ordinary_gets_and_strings_are_rejected() {
        let mut context = Context::default();
        let branded = read(
            "globalThis.events = []; \
             (() => { \
                 const duration = new Temporal.Duration(1, 2, 3, 4, 5, 6, 7, 8, 9, 10); \
                 return new Proxy(duration, { get(target, name) { \
                     events.push(name); return target[name]; \
                 } }); \
             })()",
            &mut context,
        )
        .unwrap();
        assert_eq!(branded.value(DurationUnit::Years), 1);
        assert_eq!(branded.value(DurationUnit::Nanoseconds), 10);
        assert_eq!(
            eval_string("events.join('|')", &mut context),
            "days|hours|microseconds|milliseconds|minutes|months|nanoseconds|seconds|weeks|years"
        );

        let error = read("'P1Y2M3W4DT5H6M7.00800901S'", &mut context).unwrap_err();
        assert!(error.as_native().is_some_and(JsNativeError::is_range));

        let overridden = read(
            "(() => { \
                 const duration = new Temporal.Duration(0, 0, 0, 0, 0, 0, 7); \
                 Object.defineProperty(duration, 'seconds', { get() { return 9; } }); \
                 return duration; \
             })()",
            &mut context,
        )
        .unwrap();
        assert_eq!(overridden.value(DurationUnit::Seconds), 9);
    }
}
