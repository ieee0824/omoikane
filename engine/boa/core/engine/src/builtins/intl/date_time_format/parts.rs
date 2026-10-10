//! Native date/time parts from ICU's typed, potentially nested field spans.

use super::DateTimeFormat;
use crate::{
    Context, JsArgs, JsNativeError, JsResult, JsValue,
    builtins::{OrdinaryObject, array::Array},
    js_string,
};
use std::fmt;
use writeable::{Part, PartsWrite, TryWriteable};

/// An owned field or literal emitted by the selected calendar's pattern.
#[derive(Debug, PartialEq)]
pub(super) struct DateTimePart {
    pub(super) kind: PartKind,
    pub(super) value: String,
}

/// Replaces only the typed hour field when ECMA-402 h24 maps zero to 24.
/// Calendar/date fields, numeric punctuation and all literals remain intact.
pub(super) fn replace_hour(parts: &mut [DateTimePart], hour: &str) {
    for part in parts {
        if part.kind == PartKind::Hour && part.value != hour {
            part.value.clear();
            part.value.push_str(hour);
        }
    }
}

/// Formats the calendar's owned related ISO year with the selected numeric data.
/// Only ICU's typed related-year spans are replaced; output is never reparsed.
pub(super) fn localize_related_year(
    parts: &mut [DateTimePart],
    source_year: i32,
    formatter: &icu_decimal::DecimalFormatter,
) {
    use writeable::Writeable;
    if !parts.iter().any(|part| part.kind == PartKind::RelatedYear) {
        return;
    }
    let year = fixed_decimal::Decimal::from(source_year);
    let value = formatter.format(&year).write_to_string().into_owned();
    for part in parts {
        if part.kind == PartKind::RelatedYear {
            part.value.clone_from(&value);
        }
    }
}

/// Joins the exact parts used by formatToParts into the bound-format output.
pub(super) fn join(parts: Vec<DateTimePart>) -> String {
    let mut parts = parts.into_iter();
    let Some(first) = parts.next() else {
        return String::new();
    };
    let additional = parts.as_slice().iter().map(|part| part.value.len()).sum();
    let mut output = first.value;
    output.reserve(additional);
    for part in parts {
        output.push_str(&part.value);
    }
    output
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum PartKind {
    #[default]
    Literal,
    Era,
    Year,
    RelatedYear,
    YearName,
    Month,
    Day,
    Weekday,
    Hour,
    Minute,
    Second,
    FractionalSecond,
    DayPeriod,
    TimeZoneName,
}

impl PartKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Era => "era",
            Self::Year => "year",
            Self::RelatedYear => "relatedYear",
            Self::YearName => "yearName",
            Self::Month => "month",
            Self::Day => "day",
            Self::Weekday => "weekday",
            Self::Hour => "hour",
            Self::Minute => "minute",
            Self::Second => "second",
            Self::FractionalSecond => "fractionalSecond",
            Self::DayPeriod => "dayPeriod",
            Self::TimeZoneName => "timeZoneName",
        }
    }

    fn from_icu(part: Part) -> Option<Self> {
        use icu_datetime::parts;
        match part {
            parts::ERA => Some(Self::Era),
            parts::YEAR => Some(Self::Year),
            parts::RELATED_YEAR => Some(Self::RelatedYear),
            parts::YEAR_NAME => Some(Self::YearName),
            parts::MONTH => Some(Self::Month),
            parts::DAY => Some(Self::Day),
            parts::WEEKDAY => Some(Self::Weekday),
            parts::HOUR => Some(Self::Hour),
            parts::MINUTE => Some(Self::Minute),
            parts::SECOND => Some(Self::Second),
            parts::DAY_PERIOD => Some(Self::DayPeriod),
            parts::TIME_ZONE_NAME => Some(Self::TimeZoneName),
            _ => None,
        }
    }
}

/// The active field is owned state; nested borrows last only for a write callback.
#[derive(Default)]
struct PartsCollector {
    parts: Vec<DateTimePart>,
    active: PartKind,
    datetime_field: Option<PartKind>,
    separate: bool,
}

impl fmt::Write for PartsCollector {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if value.is_empty() {
            return Ok(());
        }
        if !self.separate
            && let Some(last) = self.parts.last_mut()
            && last.kind == self.active
        {
            last.value.push_str(value);
        } else {
            self.parts.push(DateTimePart {
                kind: self.active,
                value: value.to_owned(),
            });
        }
        self.separate = false;
        Ok(())
    }
}

impl PartsWrite for PartsCollector {
    type SubPartsWrite = Self;

    fn with_part(
        &mut self,
        part: Part,
        mut write: impl FnMut(&mut Self::SubPartsWrite) -> fmt::Result,
    ) -> fmt::Result {
        let previous = (self.active, self.datetime_field);
        let field = PartKind::from_icu(part);
        let numeric = match (self.datetime_field, part) {
            (Some(PartKind::Second), icu_decimal::parts::FRACTION) => {
                Some(PartKind::FractionalSecond)
            }
            (Some(PartKind::Second), icu_decimal::parts::DECIMAL) => Some(PartKind::Literal),
            _ => None,
        };
        let recognized = field.or(numeric);
        if let Some(kind) = recognized {
            self.active = kind;
            self.separate = true;
        }
        if field.is_some() {
            self.datetime_field = field;
        }
        let result = write(self);
        (self.active, self.datetime_field) = previous;
        if recognized.is_some() {
            // Adjacent identical fields are distinct spans; nested integer
            // annotations inside one field do not introduce a boundary.
            self.separate = true;
        }
        result
    }
}

/// Rejects both sink failures and ICU's best-effort formatting errors.
pub(super) fn collect<W: TryWriteable>(formatted: &W) -> Result<Vec<DateTimePart>, String>
where
    W::Error: fmt::Debug,
{
    let mut collector = PartsCollector::default();
    formatted
        .try_write_to_parts(&mut collector)
        .map_err(|error| format!("pattern writing: {error:?}"))?
        .map_err(|error| format!("pattern formatting: {error:?}"))?;
    Ok(collector.parts)
}

impl DateTimeFormat {
    /// Implements `%Intl.DateTimeFormat.prototype.formatToParts%`.
    pub(super) fn format_to_parts(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let object = this
            .as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ().with_message("receiver is not an Intl.DateTimeFormat")
            })?;
        let _object_root = object.clone().root();
        let epoch = super::format::input_epoch(args.get_or_undefined(0), context)?;
        let parts = object.borrow_mut().data_mut().parts_epoch(epoch, context)?;
        parts_array(parts, context).map(Into::into)
    }
}

/// Publishes fresh ordinary objects only after the native formatter borrow ends.
fn parts_array(
    parts: Vec<DateTimePart>,
    context: &mut Context,
) -> JsResult<crate::object::JsObject> {
    let result = Array::array_create(0, None, context)?;
    let _result_root = result.clone().root();
    for (index, part) in parts.into_iter().enumerate() {
        let object = context
            .intrinsics()
            .templates()
            .ordinary_object()
            .create(OrdinaryObject, vec![]);
        let _object_root = object.clone().root();
        object.create_data_property_or_throw(
            js_string!("type"),
            js_string!(part.kind.as_str()),
            context,
        )?;
        object.create_data_property_or_throw(
            js_string!("value"),
            js_string!(part.value),
            context,
        )?;
        result.create_data_property_or_throw(index, object, context)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{DateTimePart, PartKind, PartsCollector, collect, join, replace_hour};
    use crate::{Context, JsValue, NativeFunction, Source, js_string};
    use std::{fmt::Write, rc::Rc};
    use writeable::PartsWrite;

    fn assert_script(script: &str, context: &mut Context) {
        let actual = context
            .eval(Source::from_bytes(script))
            .unwrap_or_else(|error| {
                let message = error
                    .to_opaque(context)
                    .to_string(context)
                    .map(|message| message.to_std_string_escaped())
                    .unwrap_or_else(|_| error.to_string());
                panic!("DateTimeFormat parts contract threw {message}");
            });
        assert_eq!(actual, JsValue::new(true));
    }

    #[test]
    fn nested_decimal_annotations_split_only_fractional_seconds() {
        let mut sink = PartsCollector::default();
        sink.with_part(icu_datetime::parts::SECOND, |sink| {
            sink.with_part(icu_decimal::parts::INTEGER, |sink| sink.write_str("03"))?;
            sink.with_part(icu_decimal::parts::DECIMAL, |sink| sink.write_str("٫"))?;
            sink.with_part(icu_decimal::parts::FRACTION, |sink| sink.write_str("٢٣٤"))
        })
        .unwrap();
        let actual: Vec<_> = sink
            .parts
            .iter()
            .map(|part| (part.kind.as_str(), part.value.as_str()))
            .collect();
        assert_eq!(
            actual,
            [
                ("second", "03"),
                ("literal", "٫"),
                ("fractionalSecond", "٢٣٤")
            ]
        );

        sink.with_part(icu_datetime::parts::YEAR, |sink| {
            sink.with_part(icu_decimal::parts::FRACTION, |sink| sink.write_str("year"))
        })
        .unwrap();
        assert_eq!(sink.parts.last().unwrap().kind, PartKind::Year);
    }

    #[test]
    fn typed_boundaries_preserve_identical_adjacent_fields_and_literals() {
        let mut sink = PartsCollector::default();
        sink.write_str("\u{200f}").unwrap();
        sink.write_str(" ").unwrap();
        for text in ["12", "34"] {
            sink.with_part(icu_datetime::parts::YEAR, |sink| {
                for digit in text.chars() {
                    sink.with_part(icu_decimal::parts::INTEGER, |sink| sink.write_char(digit))?;
                }
                Ok(())
            })
            .unwrap();
        }
        sink.write_str("年").unwrap();
        assert_eq!(sink.parts.len(), 4);
        assert_eq!(sink.parts[0].value, "\u{200f} ");
        assert_eq!(sink.parts[1].value, "12");
        assert_eq!(sink.parts[2].value, "34");
        assert_eq!(sink.parts[3].value, "年");
        assert_eq!(sink.parts[1].kind, PartKind::Year);
        assert_eq!(sink.parts[2].kind, PartKind::Year);
        assert_eq!(sink.parts[3].kind, PartKind::Literal);
    }

    #[test]
    fn hour_replacement_and_join_reuse_leading_storage() {
        let mut hour = String::with_capacity(32);
        hour.push_str("00");
        let allocation = hour.as_ptr();
        let mut parts = vec![
            DateTimePart {
                kind: PartKind::Hour,
                value: hour,
            },
            DateTimePart {
                kind: PartKind::Literal,
                value: ":".to_owned(),
            },
            DateTimePart {
                kind: PartKind::Minute,
                value: "05".to_owned(),
            },
        ];

        replace_hour(&mut parts, "24");
        assert_eq!(parts[0].value.as_ptr(), allocation);
        let output = join(parts);
        assert_eq!(output, "24:05");
        assert_eq!(output.as_ptr(), allocation);
        assert_eq!(join(Vec::new()), "");
    }

    #[test]
    fn best_effort_output_is_not_published_on_formatter_failure() {
        let output: Result<&str, &str> = Err("missing calendar data");
        let error = collect(&output).unwrap_err();
        assert!(error.contains("missing calendar data"));
    }

    #[test]
    fn native_method_shape_brand_and_fresh_result_properties() {
        assert_script(
            r#"(() => {
                const method = Intl.DateTimeFormat.prototype.formatToParts;
                const descriptor = Object.getOwnPropertyDescriptor(
                    Intl.DateTimeFormat.prototype, 'formatToParts');
                if (method.name !== 'formatToParts' || method.length !== 1 ||
                    !descriptor.writable || descriptor.enumerable || !descriptor.configurable)
                    return false;
                const length = Object.getOwnPropertyDescriptor(method, 'length');
                if (length.writable || length.enumerable || !length.configurable) return false;
                let converted = false;
                for (const receiver of [undefined, null, {}, Intl.DateTimeFormat.prototype]) {
                    try { method.call(receiver, {valueOf() { converted = true; return 0; }}); }
                    catch (error) { if (error instanceof TypeError) continue; throw error; }
                    return false;
                }
                if (converted) return false;
                try { new method(); return false; } catch (error) {
                    if (!(error instanceof TypeError)) throw error;
                }
                const formatter = new Intl.DateTimeFormat('en', {timeZone: 'UTC'});
                const a = formatter.formatToParts(0), b = formatter.formatToParts(0);
                if (!Array.isArray(a) || a === b || a[0] === b[0] || !a.length) return false;
                for (const part of a) {
                    if (Object.getPrototypeOf(part) !== Object.prototype ||
                        Object.keys(part).join(',') !== 'type,value') return false;
                    for (const key of ['type', 'value']) {
                        const d = Object.getOwnPropertyDescriptor(part, key);
                        if (!d.writable || !d.enumerable || !d.configurable ||
                            typeof d.value !== 'string') return false;
                    }
                }
                a[0].value = 'mutated';
                return b[0].value !== 'mutated';
            })()"#,
            &mut Context::default(),
        );
    }

    #[test]
    fn field_values_and_locale_literals_match_native_format() {
        assert_script(
            r#"(() => {
                const time = Date.UTC(2020, 0, 2, 15, 4, 5);
                for (const locale of ['en-US', 'de', 'ja', 'ar', 'fa', 'bn', 'th', 'zh']) {
                    const formatter = new Intl.DateTimeFormat(locale, {
                        timeZone: 'UTC', year: 'numeric', month: '2-digit', day: '2-digit',
                        hour: '2-digit', minute: '2-digit', second: '2-digit'
                    });
                    const parts = formatter.formatToParts(time);
                    if (parts.map(part => part.value).join('') !== formatter.format(time))
                        throw new Error(locale + ': format and parts differ');
                    for (const type of ['year', 'month', 'day', 'hour', 'minute', 'second']) {
                        if (!parts.some(part => part.type === type && part.value.length))
                            throw new Error(locale + ': missing ' + type);
                    }
                }
                const formatter = new Intl.DateTimeFormat('en-US', {
                    timeZone: 'UTC', year: 'numeric', month: '2-digit', day: '2-digit'
                });
                const parts = formatter.formatToParts(time);
                return JSON.stringify(parts) === JSON.stringify([
                    {type: 'month', value: '01'}, {type: 'literal', value: '/'},
                    {type: 'day', value: '02'}, {type: 'literal', value: '/'},
                    {type: 'year', value: '2020'}
                ]);
            })()"#,
            &mut Context::default(),
        );
    }

    #[test]
    fn all_owned_calendar_variants_preserve_pattern_parts() {
        assert_script(
            r#"(() => {
                const calendars = [
                    'buddhist', 'chinese', 'coptic', 'dangi', 'ethiopic', 'gregory',
                    'hebrew', 'indian', 'islamic-civil', 'islamic-rgsa',
                    'islamic-umalqura', 'japanese', 'japanext', 'persian', 'roc'
                ];
                for (const locale of ['en', 'ja']) for (const calendar of calendars) {
                    const formatter = new Intl.DateTimeFormat(locale, {
                        calendar, timeZone: 'UTC', year: 'numeric', month: 'long', day: 'numeric'
                    });
                    for (const time of [0, Date.UTC(2023, 2, 22)]) {
                        const parts = formatter.formatToParts(time);
                        if (parts.map(part => part.value).join('') !== formatter.format(time))
                            throw new Error(locale + '/' + calendar + ': format and parts differ');
                        if (!parts.some(part => part.type === 'month') ||
                            !parts.some(part => part.type === 'day') ||
                            !parts.some(part => ['year', 'relatedYear', 'yearName'].includes(part.type)))
                            throw new Error(locale + '/' + calendar + ': missing date field');
                    }
                }
                return true;
            })()"#,
            &mut Context::default(),
        );
    }

    #[test]
    fn related_year_and_year_name_are_distinct_calendar_fields() {
        assert_script(
            r#"(() => {
                const time = Date.UTC(2019, 5, 1);
                const formatter = new Intl.DateTimeFormat('zh-u-ca-chinese', {
                    timeZone: 'UTC', year: 'numeric', month: 'long', day: 'numeric'
                });
                const parts = formatter.formatToParts(time);
                const correct = parts.some(part => part.type === 'relatedYear' && part.value === '2019') &&
                    parts.some(part => part.type === 'yearName' && part.value === '己亥') &&
                    !parts.some(part => part.type === 'year') &&
                    parts.map(part => part.value).join('') === formatter.format(time);
                if (!correct) throw new Error(JSON.stringify({resolved:formatter.resolvedOptions(), parts}));
                return true;
            })()"#,
            &mut Context::default(),
        );
    }

    #[test]
    fn era_weekday_and_day_period_keep_typed_spans_and_ordinary_space() {
        assert_script(
            r#"(() => {
                const time = Date.UTC(2020, 0, 2, 15, 4, 5);
                const formatter = new Intl.DateTimeFormat('en-US', {
                    timeZone: 'UTC', era: 'long', weekday: 'long', year: 'numeric',
                    month: 'long', day: 'numeric', hour: 'numeric', hour12: true
                });
                const parts = formatter.formatToParts(time);
                for (const [type, expected] of [
                    ['era', 'Anno Domini'], ['weekday', 'Thursday'], ['dayPeriod', 'PM']
                ]) {
                    if (!parts.some(part => part.type === type && part.value === expected))
                        throw new Error('missing ' + type);
                }
                const index = parts.findIndex(part => part.type === 'dayPeriod');
                return parts[index - 1].type === 'literal' &&
                    parts[index - 1].value === ' ' &&
                    parts.map(part => part.value).join('') === formatter.format(time);
            })()"#,
            &mut Context::default(),
        );
    }

    #[test]
    fn fractions_are_truncated_into_a_separate_part_with_localized_separator() {
        assert_script(
            r#"(() => {
                for (const locale of ['en', 'de', 'ar-u-nu-arab']) {
                    for (const digits of [1, 2, 3]) {
                        const formatter = new Intl.DateTimeFormat(locale, {
                            timeZone: 'UTC', hour: '2-digit', minute: '2-digit', second: '2-digit',
                            fractionalSecondDigits: digits, hourCycle: 'h23'
                        });
                        const parts = formatter.formatToParts(Date.UTC(2020, 0, 1, 1, 2, 3, 234));
                        const index = parts.findIndex(part => part.type === 'fractionalSecond');
                        const expected = (locale.startsWith('ar') ? '٢٣٤' : '234').slice(0, digits);
                        if (index < 2 || parts[index].value !== expected ||
                            parts[index - 1].type !== 'literal' ||
                            parts[index - 1].value !== (locale === 'de' ? ',' :
                                locale.startsWith('ar') ? '٫' : '.') ||
                            parts[index - 2].type !== 'second')
                            throw new Error(locale + '/' + digits + ': wrong fraction parts');
                        if (parts.map(part => part.value).join('') !==
                            formatter.format(Date.UTC(2020, 0, 1, 1, 2, 3, 234))) return false;
                    }
                }
                return true;
            })()"#,
            &mut Context::default(),
        );
    }

    #[test]
    fn coercion_is_once_reentrant_and_preserves_abrupt_completion() {
        let mut context = Context::default();
        context
            .register_global_callable(
                js_string!("collect"),
                0,
                NativeFunction::from_fn_ptr(|_, _, _| {
                    boa_gc::force_collect();
                    Ok(JsValue::undefined())
                }),
            )
            .unwrap();
        assert_script(
            r#"(() => {
                const formatter = new Intl.DateTimeFormat('en', {timeZone: 'UTC'});
                let calls = 0;
                const parts = formatter.formatToParts({valueOf() {
                    calls++;
                    if (formatter.formatToParts(0).map(part => part.value).join('') !==
                        formatter.format(0)) throw new Error('nested formatting');
                    collect();
                    return 1234;
                }});
                if (calls !== 1 || parts.map(part => part.value).join('') !==
                    formatter.format(1234)) return false;
                const error = {};
                try { formatter.formatToParts({valueOf() { throw error; }}); }
                catch (actual) { return actual === error; }
                return false;
            })()"#,
            &mut context,
        );
    }

    #[test]
    fn time_clip_rejects_invalid_values_and_keeps_inclusive_boundaries() {
        assert_script(
            r#"(() => {
                const formatter = new Intl.DateTimeFormat('en', {
                    timeZone: 'UTC', year: 'numeric', month: 'numeric', day: 'numeric',
                    hour: 'numeric', minute: 'numeric', second: 'numeric', fractionalSecondDigits: 3
                });
                for (const value of [NaN, Infinity, -Infinity, 8640000000000001, -8640000000000001]) {
                    try { formatter.formatToParts(value); return false; }
                    catch (error) { if (!(error instanceof RangeError)) throw error; }
                }
                for (const value of [8640000000000000, -8640000000000000]) {
                    if (!Array.isArray(formatter.formatToParts(value))) return false;
                }
                for (const [value, clipped] of [[1.9, 1], [-1.9, -1], [-0.9, 0]]) {
                    if (JSON.stringify(formatter.formatToParts(value)) !==
                        JSON.stringify(formatter.formatToParts(clipped))) return false;
                }
                for (const value of [1n, Symbol('date')]) {
                    try { formatter.formatToParts(value); return false; }
                    catch (error) { if (!(error instanceof TypeError)) throw error; }
                }
                return true;
            })()"#,
            &mut Context::default(),
        );
    }

    #[test]
    fn missing_input_uses_the_context_clock_and_explicit_null_is_epoch_zero() {
        let mut context = Context::builder()
            .clock(Rc::new(crate::context::time::FixedClock::from_millis(1234)))
            .build()
            .unwrap();
        assert_script(
            r#"(() => {
                const formatter = new Intl.DateTimeFormat('en', {
                    timeZone: 'UTC', hour: 'numeric', minute: 'numeric', second: 'numeric',
                    fractionalSecondDigits: 3
                });
                const serialize = value => JSON.stringify(value);
                return serialize(formatter.formatToParts()) === serialize(formatter.formatToParts(1234)) &&
                    serialize(formatter.formatToParts(undefined)) === serialize(formatter.formatToParts(1234)) &&
                    serialize(formatter.formatToParts(null)) === serialize(formatter.formatToParts(0));
            })()"#,
            &mut context,
        );
    }

    #[test]
    fn offset_zone_input_preserves_the_shared_epoch_conversion() {
        assert_script(
            r#"(() => {
                const formatter = new Intl.DateTimeFormat('en', {
                    timeZone: '+03:01', hour: 'numeric', minute: '2-digit', hourCycle: 'h23'
                });
                const parts = formatter.formatToParts(Date.UTC(1995, 11, 17, 3, 24, 56));
                return parts.some(part => part.type === 'hour' && ['6', '06'].includes(part.value)) &&
                    parts.some(part => part.type === 'minute' && part.value === '25') &&
                    parts.map(part => part.value).join('') === formatter.format(Date.UTC(1995, 11, 17, 3, 24, 56));
            })()"#,
            &mut Context::default(),
        );
    }
}
