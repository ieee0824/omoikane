//! Contracts for Temporal slot precision and observable validation order.

use crate::{TestAction, run_test_actions};
use boa_macros::js_str;

#[test]
fn year_month_arithmetic_reads_duration_before_options() {
    run_test_actions([TestAction::assert_eq(
        r#"
        (() => {
            const ym = new Temporal.PlainYearMonth(2024, 3);
            for (const method of ['add', 'subtract']) {
                const calls = [];
                const fields = ['days', 'hours', 'microseconds', 'milliseconds',
                    'minutes', 'months', 'nanoseconds', 'seconds', 'weeks', 'years'];
                const duration = new Proxy({ months: 1 }, {
                    get(target, key) { calls.push(key); return target[key]; }
                });
                try { ym[method](duration, null); } catch (error) {
                    if (!(error instanceof TypeError)) return false;
                }
                if (calls.join() !== fields.join()) return false;
                let overflowReads = 0;
                try { ym[method]({ hours: 24 }, {
                    get overflow() { overflowReads++; return 'reject'; }
                }); return false; } catch (error) {
                    if (!(error instanceof RangeError) || overflowReads !== 1) return false;
                }
            }
            return true;
        })()
    "#,
        true,
    )]);
}

#[test]
fn year_month_uses_month_first_day_in_both_directions() {
    run_test_actions([
        TestAction::assert_eq(
            "new Temporal.PlainYearMonth(2024, 3).add({months: -1}, {overflow: 'reject'}).toString()",
            js_str!("2024-02"),
        ),
        TestAction::assert_eq(
            "new Temporal.PlainYearMonth(2023, 3).subtract({months: 1}, {overflow: 'reject'}).toString()",
            js_str!("2023-02"),
        ),
        TestAction::assert_eq(
            "new Temporal.PlainYearMonth(275760, 9).subtract({months: 1}).toString()",
            js_str!("+275760-08"),
        ),
        TestAction::assert_eq(
            r#"(() => { try { new Temporal.PlainYearMonth(-271821, 4).add({months: 1}); return false; } catch (error) { return error instanceof RangeError; } })()"#,
            true,
        ),
    ]);
}

#[test]
fn year_month_first_day_range_validation_follows_overflow_coercion() {
    run_test_actions([TestAction::assert_eq(
        r#"
        (() => {
            for (const method of ['add', 'subtract']) {
                for (const referenceDay of [1, 30]) {
                    const ym = new Temporal.PlainYearMonth(-271821, 4, 'iso8601', referenceDay);
                    for (const duration of [new Temporal.Duration(),
                        {months: method === 'add' ? 1 : -1}]) {
                        const calls = [];
                        const options = {
                            get overflow() {
                                calls.push('get overflow');
                                return {
                                    get toString() {
                                        calls.push('get overflow.toString');
                                        return () => {
                                            calls.push('call overflow.toString');
                                            return 'constrain';
                                        };
                                    }
                                };
                            }
                        };
                        try { ym[method](duration, options); return false; }
                        catch (error) { if (!(error instanceof RangeError)) return false; }
                        if (calls.join() !== 'get overflow,get overflow.toString,call overflow.toString') return false;
                    }
                    const sentinel = {};
                    try {
                        ym[method](new Temporal.Duration(), {get overflow() {throw sentinel;}});
                        return false;
                    } catch (error) { if (error !== sentinel) return false; }
                }
            }
            return true;
        })()
    "#,
        true,
    )]);
}

#[test]
fn year_month_arithmetic_validates_added_date_at_partial_month_limits() {
    run_test_actions([TestAction::assert_eq(
        r#"
        (() => {
            const minimum = new Temporal.PlainYearMonth(-271821, 4);
            if (minimum.toString() !== '-271821-04') return false;
            const may = new Temporal.PlainYearMonth(-271821, 5);
            for (const method of ['add', 'subtract']) {
                try { may[method]({months: method === 'add' ? -1 : 1}); return false; }
                catch (error) { if (!(error instanceof RangeError)) return false; }
            }
            const maximum = new Temporal.PlainYearMonth(275760, 9);
            if (maximum.add(new Temporal.Duration()).toString() !== '+275760-09' ||
                new Temporal.PlainYearMonth(275760, 8).add({months: 1}).toString() !== '+275760-09') return false;
            try { maximum.add({months: 1}); return false; }
            catch (error) { return error instanceof RangeError; }
        })()
    "#,
        true,
    )]);
}

#[test]
fn partial_calendar_objects_require_at_least_one_field_before_options() {
    run_test_actions([TestAction::assert_eq(
        r#"
        (() => {
            for (const receiver of [new Temporal.PlainYearMonth(2024, 5),
                new Temporal.ZonedDateTime(0n, 'UTC')]) {
                for (const partial of [{}, {months: 2}]) {
                    let reads = 0;
                    try { receiver.with(partial, {get overflow() {reads++; return 'reject';}});
                        return false;
                    } catch (error) {
                        if (!(error instanceof TypeError) || reads !== 0) return false;
                    }
                }
            }
            return true;
        })()
    "#,
        true,
    )]);
}

#[test]
fn zoned_string_parse_errors_precede_option_reads() {
    run_test_actions([TestAction::assert_eq(
        r#"
        (() => {
            const reads = [];
            const options = new Proxy({}, {get(_, key) {reads.push(key);}});
            try { Temporal.ZonedDateTime.from('2020-13-34T25:60:60+99:99[UTC]', options); }
            catch (error) { if (!(error instanceof RangeError)) return false; }
            if (reads.length) return false;
            try { Temporal.ZonedDateTime.from('1976-11-18Z', null); return false; }
            catch (error) { return error instanceof RangeError; }
        })()
    "#,
        true,
    )]);
}

#[test]
fn calendar_required_fields_precede_month_code_ranges() {
    run_test_actions([TestAction::assert_eq(
        r#"
        (() => {
            for (const constructor of [Temporal.PlainDateTime, Temporal.ZonedDateTime]) {
                for (const fields of [{monthCode: 'M99L', day: 1},
                    {year: 2021, day: 32}, {year: 2021, monthCode: 'M99L'}]) {
                    try { constructor.from({...fields, timeZone: 'UTC'}, {overflow: 'reject'});
                        return false;
                    } catch (error) { if (!(error instanceof TypeError)) return false; }
                }
                try { constructor.from({year: 2021, day: 1, monthCode: 'M99L', timeZone: 'UTC'});
                    return false;
                } catch (error) { if (!(error instanceof RangeError)) return false; }
            }
            return true;
        })()
    "#,
        true,
    )]);
}

#[test]
fn iso_month_day_year_only_controls_leap_day_overflow() {
    run_test_actions([
        TestAction::assert_eq(
            "Temporal.PlainMonthDay.from({year: -999999, month: 2, day: 29}).toString()",
            js_str!("02-28"),
        ),
        TestAction::assert_eq(
            "Temporal.PlainMonthDay.from({year: -1000000, month: 2, day: 29}).toString()",
            js_str!("02-29"),
        ),
        TestAction::assert_eq(
            "new Temporal.PlainMonthDay(2, 29).with({year: -999999}).toString()",
            js_str!("02-28"),
        ),
        TestAction::assert_eq(
            r#"(() => { try { Temporal.PlainMonthDay.from({year: -999999, month: 2, day: 29}, {overflow: 'reject'}); return false; } catch (error) { return error instanceof RangeError; } })()"#,
            true,
        ),
    ]);
}

#[test]
fn duration_result_slots_have_number_precision() {
    run_test_actions([
        TestAction::run(
            "const huge = Temporal.Duration.from({microseconds: Number.MAX_SAFE_INTEGER}); const added = huge.add({microseconds: Number.MAX_SAFE_INTEGER - 1});",
        ),
        TestAction::assert_eq("added.toString()", js_str!("PT18014398509.48198S")),
        TestAction::assert_eq(
            "Temporal.Duration.compare(added.add({microseconds: 1}), added)",
            0,
        ),
        TestAction::assert_eq(
            "new Temporal.Instant(0n).until(new Temporal.Instant(18446744073709551616n), {largestUnit: 'microseconds'}).toString()",
            js_str!("PT18446744073.709552616S"),
        ),
    ]);
}

#[test]
fn duration_serializes_fraction_beside_date_or_hour() {
    run_test_actions([
        TestAction::assert_eq(
            "new Temporal.Duration(1, 0, 0, 0, 0, 0, 0, 0, 0, 1).toString()",
            js_str!("P1YT0.000000001S"),
        ),
        TestAction::assert_eq(
            "new Temporal.Duration(0, 1, 0, 0, 0, 0, 0, 0, 1).toJSON()",
            js_str!("P1MT0.000001S"),
        ),
        TestAction::assert_eq(
            "new Temporal.Duration(0, 0, 0, 0, -1, 0, 0, 0, 0, -1).toString()",
            js_str!("-PT1H0.000000001S"),
        ),
        TestAction::assert_eq(
            "new Temporal.Duration(0, 0, 0, 0, 1, 0, 0, 0, 0, 1).toString({fractionalSecondDigits: 8})",
            js_str!("PT1H0.00000000S"),
        ),
    ]);
}

#[test]
fn duration_string_rejects_non_subsecond_smallest_units_without_serializing() {
    run_test_actions([TestAction::assert_eq(
        r#"["year", "month", "week", "day", "hour", "minute"].every(
            smallestUnit => {
                try {
                    new Temporal.Duration(0, 0, 0, 0, 0, 0, 1).toString({ smallestUnit });
                    return false;
                } catch (error) {
                    return error instanceof RangeError;
                }
            }
        )"#,
        true,
    )]);
}

#[test]
fn duration_totals_round_the_exact_ratio_once() {
    run_test_actions([
        TestAction::assert_eq(
            "Temporal.Duration.from({hours: 816, nanoseconds: 2049187497660}).total('hours')",
            816.56921874935,
        ),
        TestAction::assert_eq(
            "Temporal.Duration.from({milliseconds: 9007199254740992, microseconds: 1999}).total('milliseconds')",
            9007199254740994.0,
        ),
        TestAction::assert_eq(
            "Temporal.Duration.from({weeks: 5, days: 5}).total({unit: 'months', relativeTo: '1972-01-31'})",
            1.3548387096774193,
        ),
        TestAction::assert_eq(
            "Temporal.Duration.from({months: 1, hours: 10}).total({unit: 'months', relativeTo: '2020-01-31'})",
            1.0134408602150538,
        ),
    ]);
}

#[test]
fn calendar_rounding_boundaries_remain_anchored_to_origin() {
    run_test_actions([
        TestAction::assert_eq(
            "Temporal.Duration.from({months: 1, hours: 10}).round({smallestUnit: 'months', roundingMode: 'expand', relativeTo: '2020-01-31'}).toString()",
            js_str!("P2M"),
        ),
        TestAction::assert_eq(
            "Temporal.Duration.from({years: 2345, hours: 12}).round({smallestUnit: 'years', roundingMode: 'expand', relativeTo: '2020-02-29'}).toString()",
            js_str!("P2346Y"),
        ),
        TestAction::assert_eq(
            "new Temporal.Duration().round({smallestUnit: 'hours', largestUnit: 'years', relativeTo: new Temporal.ZonedDateTime(0n, 'UTC')}).toString()",
            js_str!("PT0S"),
        ),
    ]);
}

#[test]
fn calendar_rounding_keeps_exact_endpoints_and_constrained_units() {
    run_test_actions([
        TestAction::assert_eq(
            r#"
            ['ceil', 'floor', 'expand', 'trunc', 'halfCeil', 'halfFloor',
             'halfExpand', 'halfTrunc', 'halfEven'].map(roundingMode =>
                Temporal.Duration.from({months: 1}).round({
                    smallestUnit: 'months', roundingMode,
                    relativeTo: '2020-01-31'
                }).toString()).join(',')
            "#,
            js_str!("P1M,P1M,P1M,P1M,P1M,P1M,P1M,P1M,P1M"),
        ),
        TestAction::assert_eq(
            "Temporal.Duration.from({months: 1, hours: 10}).round({smallestUnit: 'months', roundingMode: 'trunc', relativeTo: '2020-01-31'}).toString()",
            js_str!("P1M"),
        ),
        TestAction::assert_eq(
            "Temporal.Duration.from({years: 1, hours: 1}).round({smallestUnit: 'years', relativeTo: '2020-02-29'}).toString()",
            js_str!("P1Y"),
        ),
        TestAction::assert_eq(
            "Temporal.Duration.from({years: 1}).round({smallestUnit: 'months', relativeTo: '2020-02-29'}).toString()",
            js_str!("P1Y"),
        ),
        TestAction::assert_eq(
            "Temporal.Duration.from({months: 13}).round({largestUnit: 'years', smallestUnit: 'months', relativeTo: '2020-02-29'}).toString()",
            js_str!("P1Y1M"),
        ),
    ]);
}

#[test]
fn calendar_rounding_uses_signed_windows_and_increment_ties() {
    run_test_actions([
        TestAction::assert_eq(
            "Temporal.Duration.from({months: -1, hours: -10}).round({smallestUnit: 'months', roundingMode: 'expand', relativeTo: '2020-03-31'}).toString()",
            js_str!("-P2M"),
        ),
        TestAction::assert_eq(
            "Temporal.Duration.from({months: -1, hours: -10}).round({smallestUnit: 'months', roundingMode: 'trunc', relativeTo: '2020-03-31'}).toString()",
            js_str!("-P1M"),
        ),
        TestAction::assert_eq(
            "Temporal.Duration.from({months: -1, hours: -10}).total({unit: 'months', relativeTo: '2020-03-31'})",
            -1.014367816091954,
        ),
        TestAction::assert_eq(
            "Temporal.Duration.from({days: 30}).round({smallestUnit: 'months', roundingIncrement: 2, roundingMode: 'halfExpand', relativeTo: '2020-01-31'}).toString()",
            js_str!("P2M"),
        ),
        TestAction::assert_eq(
            "Temporal.Duration.from({days: 30}).round({smallestUnit: 'months', roundingIncrement: 2, roundingMode: 'halfEven', relativeTo: '2020-01-31'}).toString()",
            js_str!("PT0S"),
        ),
        TestAction::assert_eq(
            "Temporal.Duration.from({days: 30}).round({smallestUnit: 'months', roundingIncrement: 2, roundingMode: 'halfEven', relativeTo: '2020-03-31'}).toString()",
            js_str!("PT0S"),
        ),
    ]);
}
