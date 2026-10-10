//! Observable ECMA-402 boundaries exercised through native public methods.

use crate::{Context, JsValue, NativeFunction, Source, js_string};

fn assert_script(script: &str, context: &mut Context) {
    let actual = context
        .eval(Source::from_bytes(script))
        .unwrap_or_else(|error| {
            let message = error
                .to_opaque(context)
                .to_string(context)
                .map(|message| message.to_std_string_escaped())
                .unwrap_or_else(|_| error.to_string());
            panic!("DurationFormat contract threw {message}");
        });
    assert_eq!(actual, JsValue::new(true));
}

#[test]
fn constructor_and_methods_have_standard_shape_and_receiver_brand() {
    assert_script(
        r#"(() => {
        const C = Intl.DurationFormat;
        if (C.name !== 'DurationFormat' || C.length !== 0) return false;
        const property = Object.getOwnPropertyDescriptor(Intl, 'DurationFormat');
        if (!property.writable || property.enumerable || !property.configurable) return false;
        let converted = false;
        try { C({toString() { converted = true; return 'en'; }}); return false; }
        catch (error) { if (!(error instanceof TypeError)) throw error; }
        if (converted) return false;
        if (Object.prototype.toString.call(new C('en')) !== '[object Intl.DurationFormat]') return false;
        for (const [name, length] of [['format', 1], ['formatToParts', 1], ['resolvedOptions', 0]]) {
            const method = C.prototype[name], d = Object.getOwnPropertyDescriptor(C.prototype, name);
            if (method.name !== name || method.length !== length || !d.writable ||
                d.enumerable || !d.configurable || !('value' in d)) return false;
            for (const receiver of [undefined, null, {}, C.prototype]) {
                try { method.call(receiver, {get seconds() { converted = true; return 1; }}); return false; }
                catch (error) { if (!(error instanceof TypeError)) throw error; }
            }
            try { new method(); return false; } catch (error) {
                if (!(error instanceof TypeError)) throw error;
            }
        }
        return !converted && C.supportedLocalesOf.length === 1;
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn constructor_reads_prototype_then_locale_and_options_once_in_order() {
    assert_script(
        r#"(() => {
        const events = [];
        const target = new Proxy(function(){}, {get(object, name) {
            if (name === 'prototype') events.push('prototype');
            return object[name];
        }});
        const locales = {get length() { events.push('locales'); return 0; }};
        const options = new Proxy({}, {get(object, name) { events.push(name); return object[name]; }});
        Reflect.construct(Intl.DurationFormat, [locales, options], target);
        const expected = ['prototype', 'locales', 'localeMatcher', 'numberingSystem', 'style'];
        for (const unit of ['years', 'months', 'weeks', 'days', 'hours', 'minutes', 'seconds',
            'milliseconds', 'microseconds', 'nanoseconds']) expected.push(unit, unit + 'Display');
        expected.push('fractionalDigits');
        return events.join(',') === expected.join(',');
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn numbering_system_option_and_unicode_extension_follow_resolution_precedence() {
    assert_script(
        r#"(() => {
        const cases = [
            ['en-u-nu-arab', 'invalid', 'en-u-nu-arab', 'arab'],
            ['en-u-nu-invalid', 'invalid2', 'en', 'latn'],
            ['en-u-nu-latn', 'arab', 'en', 'arab'],
            ['en-u-nu-arab', 'arab', 'en-u-nu-arab', 'arab']
        ];
        for (const [locale, numberingSystem, expectedLocale, expectedSystem] of cases) {
            const resolved = new Intl.DurationFormat(locale, {numberingSystem}).resolvedOptions();
            if (resolved.locale !== expectedLocale || resolved.numberingSystem !== expectedSystem)
                throw new Error(locale + '/' + numberingSystem + ': resolution mismatch');
        }
        return true;
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn resolved_options_are_fresh_ordered_data_properties_and_keep_explicit_zero_digits() {
    assert_script(
        r#"(() => {
        const formatter = new Intl.DurationFormat('en', {style: 'digital', fractionalDigits: 0});
        const a = formatter.resolvedOptions(), b = formatter.resolvedOptions();
        const expected = ['locale', 'numberingSystem', 'style'];
        for (const unit of ['years', 'months', 'weeks', 'days', 'hours', 'minutes', 'seconds',
            'milliseconds', 'microseconds', 'nanoseconds']) expected.push(unit, unit + 'Display');
        expected.push('fractionalDigits');
        if (a === b || Object.keys(a).join(',') !== expected.join(',') || a.fractionalDigits !== 0)
            return false;
        for (const key of expected) {
            const d = Object.getOwnPropertyDescriptor(a, key);
            if (!d.writable || !d.enumerable || !d.configurable || !('value' in d)) return false;
        }
        a.style = 'mutated';
        return b.style === 'digital' && b.hours === 'numeric' && b.minutes === '2-digit' &&
            b.seconds === '2-digit' && b.milliseconds === 'numeric' && b.millisecondsDisplay === 'auto';
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn long_duration_uses_unit_list_data_and_all_ten_units() {
    assert_script(
        r#"(() => {
        const formatter = new Intl.DurationFormat('en', {style: 'long'});
        const duration = {years: 1, months: 2, weeks: 3, days: 4, hours: 5,
            minutes: 6, seconds: 7, milliseconds: 8, microseconds: 9, nanoseconds: 10};
        const parts = formatter.formatToParts(duration);
        const expected = '1 year, 2 months, 3 weeks, 4 days, 5 hours, 6 minutes, 7 seconds, ' +
            '8 milliseconds, 9 microseconds, 10 nanoseconds';
        if (formatter.format(duration) !== expected || parts.map(p => p.value).join('') !== expected)
            return false;
        return ['year', 'month', 'week', 'day', 'hour', 'minute', 'second', 'millisecond',
            'microsecond', 'nanosecond'].every(unit => parts.some(p => p.type === 'unit' && p.unit === unit));
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn digital_parts_preserve_unit_affinity_and_omit_unit_on_clock_separators() {
    assert_script(
        r#"(() => {
        const formatter = new Intl.DurationFormat('en', {style: 'digital', fractionalDigits: 3});
        const duration = {hours: 1, minutes: 2, seconds: 3, milliseconds: 4};
        const expected = [
            {type: 'integer', value: '1', unit: 'hour'},
            {type: 'literal', value: ':'},
            {type: 'integer', value: '02', unit: 'minute'},
            {type: 'literal', value: ':'},
            {type: 'integer', value: '03', unit: 'second'},
            {type: 'decimal', value: '.', unit: 'second'},
            {type: 'fraction', value: '004', unit: 'second'}
        ];
        const a = formatter.formatToParts(duration), b = formatter.formatToParts(duration);
        if (JSON.stringify(a) !== JSON.stringify(expected) || a === b || a[0] === b[0] ||
            formatter.format(duration) !== '1:02:03.004') return false;
        for (const part of a) for (const key of Object.keys(part)) {
            const d = Object.getOwnPropertyDescriptor(part, key);
            if (!d.writable || !d.enumerable || !d.configurable) return false;
        }
        return true;
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn clock_separators_and_fraction_symbols_come_from_locale_and_numbering_data() {
    assert_script(
        r#"(() => {
        const duration = {hours: 1, minutes: 2, seconds: 3, milliseconds: 4};
        for (const [locale, numberingSystem, expected] of [
            ['en', 'latn', '1:02:03.004'], ['de', 'latn', '1:02:03,004'],
            ['fi', 'latn', '1.02.03,004'], ['da', 'latn', '1.02.03,004'],
            ['en', 'arabext', '۱٫۰۲٫۰۳٫۰۰۴'], ['en', 'beng', '১:০২:০৩.০০৪']
        ]) {
            const formatter = new Intl.DurationFormat(locale, {style: 'digital', numberingSystem});
            const actual = formatter.format(duration);
            if (actual !== expected || formatter.formatToParts(duration).map(p => p.value).join('') !== actual)
                throw new Error(locale + '/' + numberingSystem + ': ' + actual + '; expected ' + expected);
        }
        return true;
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn auto_zero_clock_fields_skip_or_fill_only_required_gaps() {
    assert_script(
        r#"(() => {
        const formatter = new Intl.DurationFormat('en', {
            hours: 'numeric', hoursDisplay: 'auto', minutesDisplay: 'auto', secondsDisplay: 'auto',
            fractionalDigits: 0
        });
        for (const [duration, expected] of [
            [{hours: 0}, ''], [{hours: 1}, '1'], [{minutes: 1}, '01'], [{seconds: 1}, '01'],
            [{hours: 1, seconds: 1}, '1:00:01'],
            [{hours: 1, nanoseconds: 1}, '1:00:00'],
            [{minutes: 1, seconds: 1}, '01:01']
        ]) if (formatter.format(duration) !== expected) throw new Error(JSON.stringify(duration));
        const minutes = new Intl.DurationFormat('en', {minutes: 'numeric', secondsDisplay: 'auto'});
        return minutes.format({minutes: 2}) === '2';
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn only_first_displayed_unit_carries_sign_including_leading_negative_zero() {
    assert_script(
        r#"(() => {
        const duration = {hours: 0, minutes: -2, seconds: -3};
        const formatter = new Intl.DurationFormat('en', {style: 'digital'});
        if (formatter.format(duration) !== '-0:02:03') return false;
        const parts = formatter.formatToParts(duration);
        if (parts.filter(p => p.type === 'minusSign').length !== 1 || parts[0].unit !== 'hour') return false;
        const text = new Intl.DurationFormat('en', {style: 'long', yearsDisplay: 'always'});
        if (text.format({months: -2}) !== '-0 years, 2 months') return false;
        const zero = new Intl.DurationFormat('en');
        return zero.format({seconds: -0}) === '' && zero.formatToParts({seconds: -0}).length === 0;
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn fractional_seconds_truncate_and_preserve_exact_values_beyond_double_precision() {
    assert_script(
        r#"(() => {
        const formatter = new Intl.DurationFormat('en', {style: 'digital'});
        const cases = [
            [{seconds: 10000000, nanoseconds: 1}, '0:00:10000000.000000001'],
            [{seconds: 1, milliseconds: 2, microseconds: 3, nanoseconds: Number.MAX_SAFE_INTEGER},
                '0:00:9007200.256743991'],
            [{milliseconds: 4503599627370497_000, microseconds: 4503599627370495_000000},
                '0:00:9007199254740991.975424']
        ];
        for (const [duration, expected] of cases) if (formatter.format(duration) !== expected)
            throw new Error('precision ' + JSON.stringify(duration));
        for (const [digits, expected] of [[0, '-0:00:01'], [2, '-0:00:01.99'], [9, '-0:00:01.999999999']]) {
            const f = new Intl.DurationFormat('en', {style: 'digital', fractionalDigits: digits});
            if (f.format({seconds: -1, nanoseconds: -999999999}) !== expected) return false;
        }
        return true;
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn first_fractional_subsecond_style_absorbs_only_its_smaller_units() {
    assert_script(
        r#"(() => {
        const duration = {seconds: 3, milliseconds: 444, microseconds: 55, nanoseconds: 6};
        const milliseconds = new Intl.DurationFormat('en', {microseconds: 'numeric'});
        const microseconds = new Intl.DurationFormat('en', {nanoseconds: 'numeric'});
        return milliseconds.format(duration) === '3 sec, 444.055006 ms' &&
            microseconds.format(duration) === '3 sec, 444 ms, 55.006 μs';
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn unit_patterns_support_prefix_suffix_both_sides_and_number_omission() {
    assert_script(
        r#"(() => {
        const cases = [
            ['ar', -1, 'ثانية'], ['ar', -2, 'ثانيتان'], ['he', -2, 'שתי שניות'],
            ['sw', -1, 'sekunde -1'], ['ee-TG', -1, 'sekend -1 wo']
        ];
        for (const [locale, seconds, expected] of cases) {
            const formatter = new Intl.DurationFormat(locale, {style: 'long'});
            const parts = formatter.formatToParts({seconds});
            if (formatter.format({seconds}) !== expected || parts.map(p => p.value).join('') !== expected)
                throw new Error(locale + ': ' + formatter.format({seconds}));
            if (locale === 'ar' && parts.some(p => ['integer', 'minusSign'].includes(p.type))) return false;
            if (parts.some(p => p.unit !== 'second')) return false;
        }
        return true;
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn typed_list_spans_reinsert_duplicate_element_strings_by_position() {
    use super::{
        options::DurationUnit,
        parts::{DurationPart, format_list, list_parts},
    };
    use icu_list::{
        ListFormatter,
        options::{ListFormatterOptions, ListLength},
    };
    let context = Context::default();
    let formatter = ListFormatter::try_new_unit_with_buffer_provider(
        context.intl_provider().erased_provider(),
        icu_locale::locale!("en").into(),
        ListFormatterOptions::default().with_length(ListLength::Short),
    )
    .unwrap();
    let groups: Vec<Vec<DurationPart>> = [
        DurationUnit::Years,
        DurationUnit::Hours,
        DurationUnit::Seconds,
    ]
    .into_iter()
    .map(|unit| {
        vec![DurationPart {
            kind: "integer",
            value: "1".into(),
            unit: Some(unit),
        }]
    })
    .collect();
    assert_eq!(format_list(&formatter, &groups), "1, 1, 1");
    let parts = list_parts(&formatter, groups).unwrap();
    let actual: Vec<_> = parts.iter().filter_map(|part| part.unit).collect();
    assert_eq!(
        actual,
        [
            DurationUnit::Years,
            DurationUnit::Hours,
            DurationUnit::Seconds
        ]
    );
    assert_eq!(super::parts::join(&parts), "1, 1, 1");
}

#[test]
fn borrowed_list_groups_preserve_multi_element_order_and_parts() {
    use super::{
        options::DurationUnit,
        parts::{DurationPart, format_list, list_parts},
    };
    use icu_list::{
        ListFormatter,
        options::{ListFormatterOptions, ListLength},
    };
    let context = Context::default();
    let formatter = ListFormatter::try_new_unit_with_buffer_provider(
        context.intl_provider().erased_provider(),
        icu_locale::locale!("en").into(),
        ListFormatterOptions::default().with_length(ListLength::Short),
    )
    .unwrap();
    let groups: Vec<Vec<DurationPart>> = [
        (DurationUnit::Years, "1", "yr"),
        (DurationUnit::Months, "2", "mo"),
        (DurationUnit::Days, "3", "day"),
    ]
    .into_iter()
    .map(|(unit, value, label)| {
        vec![
            DurationPart {
                kind: "integer",
                value: value.into(),
                unit: Some(unit),
            },
            DurationPart::literal(" "),
            DurationPart {
                kind: "unit",
                value: label.into(),
                unit: Some(unit),
            },
        ]
    })
    .collect();

    assert_eq!(format_list(&formatter, &groups), "1 yr, 2 mo, 3 day");
    let parts = list_parts(&formatter, groups).unwrap();
    let actual: Vec<_> = parts
        .iter()
        .map(|part| (part.kind, part.value.as_str(), part.unit))
        .collect();
    assert_eq!(
        actual,
        [
            ("integer", "1", Some(DurationUnit::Years)),
            ("literal", " ", None),
            ("unit", "yr", Some(DurationUnit::Years)),
            ("literal", ", ", None),
            ("integer", "2", Some(DurationUnit::Months)),
            ("literal", " ", None),
            ("unit", "mo", Some(DurationUnit::Months)),
            ("literal", ", ", None),
            ("integer", "3", Some(DurationUnit::Days)),
            ("literal", " ", None),
            ("unit", "day", Some(DurationUnit::Days)),
        ]
    );
}

#[test]
fn borrowed_list_groups_preserve_empty_and_single_element_parts() {
    use super::{
        options::DurationUnit,
        parts::{DurationPart, format_list, list_parts},
    };
    use icu_list::{
        ListFormatter,
        options::{ListFormatterOptions, ListLength},
    };
    let context = Context::default();
    let formatter = ListFormatter::try_new_unit_with_buffer_provider(
        context.intl_provider().erased_provider(),
        icu_locale::locale!("en").into(),
        ListFormatterOptions::default().with_length(ListLength::Short),
    )
    .unwrap();

    let empty = Vec::<Vec<DurationPart>>::new();
    assert_eq!(format_list(&formatter, &empty), "");
    assert!(list_parts(&formatter, empty).unwrap().is_empty());

    let groups = vec![vec![
        DurationPart {
            kind: "integer",
            value: "12".into(),
            unit: Some(DurationUnit::Seconds),
        },
        DurationPart::literal(" "),
        DurationPart {
            kind: "unit",
            value: "sec".into(),
            unit: Some(DurationUnit::Seconds),
        },
    ]];
    assert_eq!(format_list(&formatter, &groups), "12 sec");
    let parts = list_parts(&formatter, groups).unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0].kind, "integer");
    assert_eq!(parts[1].kind, "literal");
    assert_eq!(parts[2].kind, "unit");
    assert_eq!(super::parts::join(&parts), "12 sec");
}

#[test]
fn duration_getters_can_reenter_and_collect_without_borrowing_formatter_state() {
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
        const formatter = new Intl.DurationFormat('en', {style: 'digital'});
        let reads = 0, conversions = 0;
        const actual = formatter.format({get seconds() {
            reads++;
            const value = 5;
            return {valueOf() {
                conversions++;
                if (formatter.format({seconds: 1}) !== '0:00:01') throw new Error('reentry');
                collect(); return value;
            }};
        }});
        const error = {};
        try { formatter.formatToParts({get seconds() {throw error;}}); }
        catch (actualError) {
            return reads === 1 && conversions === 1 && actual === '0:00:05' && actualError === error;
        }
        return false;
    })()"#,
        &mut context,
    );
}

#[test]
fn native_number_and_list_factories_ignore_author_global_constructors() {
    assert_script(
        r#"(() => {
        const formatter = new Intl.DurationFormat('en', {style: 'long'});
        Intl.NumberFormat = Intl.ListFormat = function() {throw new Error('tainted');};
        const fresh = new Intl.DurationFormat('en', {style: 'digital'});
        return formatter.format({hours: 1, minutes: 2}) === '1 hour, 2 minutes' &&
            fresh.format({hours: 1, minutes: 2}) === '1:02:00';
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn duration_number_formatters_share_cores_by_grouping_mode() {
    use super::options::DurationUnit;

    let mut context = Context::default();
    let locale = JsValue::from(js_string!("en"));
    let digital_options = context
        .eval(Source::from_bytes("({ style: 'digital' })"))
        .unwrap();
    let digital = super::DurationFormat::new(&locale, &digital_options, &mut context).unwrap();
    let ids = digital.backend.number_core_ids();

    let text = ids[DurationUnit::Years.index()].unwrap();
    assert_eq!(
        digital.backend.number_has_plural_rules(DurationUnit::Years),
        Some(true)
    );
    for unit in [
        DurationUnit::Months,
        DurationUnit::Weeks,
        DurationUnit::Days,
    ] {
        assert_eq!(ids[unit.index()], Some(text));
    }
    let numeric = ids[DurationUnit::Hours.index()].unwrap();
    assert_ne!(text, numeric);
    assert_eq!(
        digital.backend.number_has_plural_rules(DurationUnit::Hours),
        Some(false)
    );
    for unit in [DurationUnit::Minutes, DurationUnit::Seconds] {
        assert_eq!(ids[unit.index()], Some(numeric));
    }
    for unit in [
        DurationUnit::Milliseconds,
        DurationUnit::Microseconds,
        DurationUnit::Nanoseconds,
    ] {
        assert_eq!(ids[unit.index()], None);
    }

    let text_only =
        super::DurationFormat::new(&locale, &JsValue::undefined(), &mut context).unwrap();
    let ids = text_only.backend.number_core_ids();
    let first = ids[0].unwrap();
    assert!(ids.into_iter().all(|id| id == Some(first)));
}

#[test]
fn supported_locales_match_duration_data_and_preserve_requested_extensions() {
    assert_script(
        r#"(() => {
        const supported = Intl.DurationFormat.supportedLocalesOf(
            ['en-u-nu-arab', 'ja', 'de', 'fi', 'da', 'sw', 'ee-TG', 'bn'], {localeMatcher: 'lookup'});
        return supported.join(',') === 'en-u-nu-arab,ja,de,fi,da,sw,ee-TG,bn' &&
            Intl.DurationFormat.supportedLocalesOf([]).length === 0;
    })()"#,
        &mut Context::default(),
    );
}

#[cfg(feature = "temporal")]
#[test]
fn temporal_format_reads_properties_while_locale_path_uses_internal_duration() {
    assert_script(
        r#"(() => {
        const duration = new Temporal.Duration(0, 0, 0, 0, 1, 2, 3, 4, 5, 6);
        const formatter = new Intl.DurationFormat('en', {style: 'digital'});
        const expected = '1:02:03.004005006';
        let reads = 0;
        Object.defineProperty(duration, 'seconds', {get() {reads++; return 9;}});
        if (formatter.format(duration) !== '1:02:09.004005006' || reads !== 1) return false;
        try { formatter.format('PT1H2M3.004005006S'); return false; }
        catch (error) { if (!(error instanceof RangeError)) throw error; }
        const locale = Temporal.Duration.prototype.toLocaleString;
        Intl.DurationFormat = function() {throw new Error('tainted Intl');};
        return locale.call(duration, 'en', {style: 'digital'}) === expected &&
            locale.call(duration, 'de', {style: 'digital', fractionalDigits: 2}) === '1:02:03,00';
    })()"#,
        &mut Context::default(),
    );
}

#[test]
#[cfg(feature = "intl_bundled")]
fn pinned_numeric_digits_match_duration_metadata_and_numeric_parts() {
    let mut context = Context::default();
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../icu_provider/data/icu_decimal_digits.duration-contract.js"
    ));
    let actual = context.eval(Source::from_bytes(source)).unwrap();
    assert_eq!(actual, JsValue::from(true));
}

#[test]
fn scalar_text_matches_parts_for_digital_and_all_ten_duration_units() {
    assert_script(
        r#"(() => {
        const fields = ['years','months','weeks','days','hours','minutes','seconds',
            'milliseconds','microseconds','nanoseconds'];
        const records = [
            Object.fromEntries(fields.map((field,index) => [field,index + 1])),
            Object.fromEntries(fields.map((field,index) => [field,-index - 1])),
            {hours:1,minutes:0,seconds:3,milliseconds:4,microseconds:5,nanoseconds:6},
            {milliseconds:-1}, {seconds:0},
        ];
        for (const locale of ['en','ar-EG','fa','he','fr','de','bn','en-u-nu-mathbold']) {
            for (const style of ['long','short','narrow','digital']) {
                for (const fractionalDigits of [undefined,0,3,9]) {
                    const options = {style};
                    if (fractionalDigits !== undefined) options.fractionalDigits = fractionalDigits;
                    const df = new Intl.DurationFormat(locale, options);
                    for (const record of records) {
                        const text = df.format(record);
                        const parts = df.formatToParts(record);
                        if (text !== parts.map(part => part.value).join('')) throw new Error(
                            locale + '/' + style + '/' + fractionalDigits + '/' +
                            JSON.stringify(record) + ': ' + text + ' !== ' + JSON.stringify(parts));
                    }
                }
            }
            const options = {style:'long'};
            for (const field of fields) options[field + 'Display'] = 'always';
            const df = new Intl.DurationFormat(locale, options);
            // Arabic one/two patterns legitimately omit the numeric part.
            // The records above cover those patterns; use explicit numerals
            // here to verify that all ten independently formatted units exist.
            const record = Object.fromEntries(fields.map((field,index) => [field,index + 11]));
            const parts = df.formatToParts(record);
            if (new Set(parts.filter(part => part.type === 'integer').map(part => part.unit)).size !== 10)
                throw new Error('all ten duration units missing for ' + locale);
            if (df.format(record) !== parts.map(part => part.value).join('')) return false;
        }
        return true;
    })()"#,
        &mut Context::default(),
    );
}
