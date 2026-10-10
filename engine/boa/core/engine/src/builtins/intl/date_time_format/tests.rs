//! Regression tests for observable locale resolution and its actual output.
use crate::{Context, JsValue, Source};

fn assert_script(script: &str) {
    let mut context = Context::default();
    let actual = context
        .eval(Source::from_bytes(script))
        .unwrap_or_else(|error| {
            let message = error
                .to_opaque(&mut context)
                .to_string(&mut context)
                .map(|message| message.to_std_string_escaped())
                .unwrap_or_else(|_| error.to_string());
            panic!("DateTimeFormat contract threw {message}");
        });
    assert_eq!(actual, JsValue::new(true));
}

#[test]
fn basic_fixed_pattern_reuses_data_without_retaining_input_year_or_midnight_hour() {
    assert_script(
        r#"(() => {
        for (const [locale,calendar] of [['en','gregory'],['zh','chinese'],['ko','dangi']]) {
            const f = new Intl.DateTimeFormat(locale, {formatMatcher:'basic', calendar,
                numberingSystem:'arab', timeZone:'UTC', year:'numeric', month:'long', day:'numeric',
                hour:'numeric', minute:'numeric', second:'numeric', hourCycle:'h24'});
            const inputs = [[Date.UTC(2019,0,1,0,4,5),'٢٠١٩','٢٠١٨','٢٤'],
                [Date.UTC(2019,5,1,13,4,5),'٢٠١٩','٢٠١٩','١٣'],
                [Date.UTC(2020,5,1,0,4,5),'٢٠٢٠','٢٠٢٠','٢٤']];
            const values = [];
            for (const [time,year,related,hour] of inputs) {
                const text = f.format(time), parts = f.formatToParts(time);
                if (text !== parts.map(x => x.value).join('') ||
                    !parts.some(x => x.type === 'hour' && x.value === hour) ||
                    !parts.some(x => x.type === (calendar === 'gregory' ? 'year' : 'relatedYear') &&
                        x.value === (calendar === 'gregory' ? year : related)))
                    throw new Error(JSON.stringify({locale,calendar,time,text,parts}));
                values.push(text);
            }
            if (values[0] === values[1] || values[1] === values[2] ||
                f.format(inputs[0][0]) !== values[0] ||
                f.formatToParts(inputs[0][0]).map(x => x.value).join('') !== values[0])
                throw new Error('fixed pattern retained a previous input value');
        }
        return true;
    })()"#,
    );
}

#[test]
fn basic_minute_second_selects_locale_ms_without_an_added_hour() {
    assert_script(
        r#"(() => {
        const instant = Date.UTC(2020, 0, 2, 15, 4, 5);
        for (const locale of ['en','de','ja','ar','fi']) {
            const f = new Intl.DateTimeFormat(locale, {formatMatcher:'basic', timeZone:'UTC',
                minute:'numeric', second:'numeric'});
            const r = f.resolvedOptions(), p = f.formatToParts(instant);
            if ('hour' in r || 'hourCycle' in r || 'hour12' in r || 'dayPeriod' in r ||
                !r.minute || !r.second || p.some(x => x.type === 'hour' || x.type === 'dayPeriod') ||
                !p.some(x => x.type === 'minute') || !p.some(x => x.type === 'second') ||
                p.map(x => x.value).join('') !== f.format(instant))
                throw new Error(JSON.stringify({locale,r,p}));
            if (locale === 'en' && (r.minute !== '2-digit' || r.second !== '2-digit' ||
                f.format(instant) !== '04:05')) throw new Error('selected ms width was overwritten');
            if (locale === 'fi' && f.format(instant) !== '4.05')
                throw new Error('locale ms separator or minute width was lost');
        }
        return true;
    })()"#,
    );
}

#[test]
fn basic_fractional_candidates_use_native_decimal_digits_and_parts() {
    assert_script(
        r#"(() => {
        const instant = Date.UTC(2020, 0, 2, 15, 4, 5, 123);
        for (const [locale,nu,digits] of [['en','latn','0123456789'],
                ['de','latn','0123456789'], ['ar','arab','٠١٢٣٤٥٦٧٨٩']]) {
            for (const width of [1,2,3]) {
                const f = new Intl.DateTimeFormat(locale, {formatMatcher:'basic', timeZone:'UTC',
                    numberingSystem:nu, minute:'numeric', second:'numeric', fractionalSecondDigits:width});
                const r = f.resolvedOptions(), p = f.formatToParts(instant);
                const fraction = p.filter(x => x.type === 'fractionalSecond');
                const expected = '123'.slice(0,width).split('').map(x => digits[Number(x)]).join('');
                if (r.fractionalSecondDigits !== width || r.numberingSystem !== nu ||
                    fraction.length !== 1 || fraction[0].value !== expected ||
                    p.some(x => x.type === 'hour') || p.map(x => x.value).join('') !== f.format(instant))
                    throw new Error(JSON.stringify({locale,nu,width,r,p}));
                if (locale === 'de' && !p.some(x => x.type === 'literal' && x.value.includes(',')))
                    throw new Error('decimal separator did not come from the numbering provider');
                if (nu === 'arab' && !p.some(x => x.type === 'literal' && x.value.includes('٫')))
                    throw new Error('Arabic decimal separator was replaced');
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn basic_patterns_and_reported_widths_follow_all_four_hour_cycles() {
    assert_script(
        r#"(() => {
        for (const locale of ['en','ja']) {
            for (const [hc,midnight,noon] of [['h11',0,0],['h12',12,12],['h23',0,12],['h24',24,12]]) {
                const f = new Intl.DateTimeFormat(locale, {formatMatcher:'basic', timeZone:'UTC',
                    hour:'numeric', minute:'numeric', hourCycle:hc, numberingSystem:'latn'});
                const r = f.resolvedOptions();
                if (r.hourCycle !== hc || r.hour12 !== (hc === 'h11' || hc === 'h12') || 'dayPeriod' in r)
                    throw new Error(JSON.stringify({locale,hc,r}));
                for (const [time,expected] of [[0,midnight],[43200000,noon]]) {
                    const p = f.formatToParts(time), h = p.find(x => x.type === 'hour');
                    const expectedText = String(expected).padStart(r.hour === '2-digit' ? 2 : 1,'0');
                    if (!h || h.value !== expectedText ||
                        p.some(x => x.type === 'dayPeriod') !== r.hour12 ||
                        p.map(x => x.value).join('') !== f.format(time))
                        throw new Error(JSON.stringify({locale,hc,time,r,p}));
                }
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn basic_calendar_selection_retains_native_year_names_and_combined_fields() {
    assert_script(
        r#"(() => {
        const instant = Date.UTC(2019,5,1,15,4,5);
        for (const [locale,ca] of [['zh','chinese'],['ko','dangi'],['ja','japanese'],['th','buddhist']]) {
            const f = new Intl.DateTimeFormat(locale, {formatMatcher:'basic', timeZone:'UTC',
                calendar:ca, year:'numeric', month:'long', day:'numeric',
                hour:'numeric', minute:'numeric', second:'numeric', hourCycle:'h23'});
            const r = f.resolvedOptions(), p = f.formatToParts(instant);
            if (r.calendar !== ca || !['month','day','hour','minute','second'].every(
                type => p.some(x => x.type === type)) || p.map(x => x.value).join('') !== f.format(instant))
                throw new Error(JSON.stringify({locale,ca,r,p}));
            if ((ca === 'chinese' || ca === 'dangi') &&
                (!p.some(x => x.type === 'yearName') || !p.some(x => x.type === 'relatedYear')))
                throw new Error(JSON.stringify({locale,ca,r,p}));
            if (ca === 'japanese' && !p.some(x => x.type === 'era'))
                throw new Error('era required by the selected calendar was dropped');
        }
        return true;
    })()"#,
    );
}

#[test]
fn related_calendar_year_uses_selected_digits_for_basic_and_best_fit() {
    assert_script(
        r#"(() => {
        for (const [locale,calendar] of [['zh','chinese'],['ko','dangi']]) {
            for (const matcher of ['basic','best fit']) {
                const options = {formatMatcher:matcher, calendar, timeZone:'UTC',
                    year:'numeric', month:'long', day:'numeric'};
                const latin = new Intl.DateTimeFormat(locale, {...options, numberingSystem:'latn'});
                const arabic = new Intl.DateTimeFormat(locale, {...options, numberingSystem:'arab'});
                if (arabic.resolvedOptions().numberingSystem !== 'arab') return false;
                for (const [time,year,localized] of [[Date.UTC(2019,0,1),'2018','٢٠١٨'],
                        [Date.UTC(2019,5,1),'2019','٢٠١٩']]) {
                    const left = latin.formatToParts(time), right = arabic.formatToParts(time);
                    const ly = left.filter(x => x.type === 'relatedYear');
                    const ry = right.filter(x => x.type === 'relatedYear');
                    const names = p => p.filter(x => x.type === 'yearName');
                    if (ly.length !== 1 || ly[0].value !== year ||
                        ry.length !== 1 || ry[0].value !== localized ||
                        names(left).length !== 1 || JSON.stringify(names(left)) !== JSON.stringify(names(right)) ||
                        left.some(x => x.type === 'year') || right.some(x => x.type === 'year') ||
                        left.map(x => x.value).join('') !== latin.format(time) ||
                        right.map(x => x.value).join('') !== arabic.format(time))
                        throw new Error(JSON.stringify({locale,calendar,matcher,time,left,right}));
                }
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn localized_related_year_and_h24_keep_the_same_calendar_and_clock_parts() {
    assert_script(
        r#"(() => {
        for (const [locale,calendar] of [['zh','chinese'],['ko','dangi']]) {
            for (const formatMatcher of ['basic','best fit']) {
                const f = new Intl.DateTimeFormat(locale, {formatMatcher, calendar, numberingSystem:'arab',
                    timeZone:'UTC', year:'numeric', month:'long', day:'numeric',
                    hour:'numeric', minute:'numeric', second:'numeric', hourCycle:'h24'});
                const time = Date.UTC(2019,0,1,0,4,5), parts = f.formatToParts(time);
                if (!parts.some(x => x.type === 'relatedYear' && x.value === '٢٠١٨') ||
                    !parts.some(x => x.type === 'hour' && x.value === '٢٤') ||
                    !['month','day','minute','second','yearName'].every(type => parts.some(x => x.type === type)) ||
                    parts.map(x => x.value).join('') !== f.format(time))
                    throw new Error(JSON.stringify({locale,calendar,formatMatcher,parts}));
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn matcher_is_coerced_once_after_components_and_before_styles() {
    assert_script(
        r#"(() => {
        const events = [], matcher = {toString() {events.push('coerce'); return 'basic';}};
        const options = {timeZone:'UTC', minute:'numeric', second:'numeric',
            get formatMatcher() {events.push('matcher'); return matcher;},
            get dateStyle() {events.push('dateStyle'); return undefined;},
            get timeStyle() {events.push('timeStyle'); return undefined;}};
        const f = new Intl.DateTimeFormat('en',options);
        if (JSON.stringify(events) !== '["matcher","coerce","dateStyle","timeStyle"]' ||
            'formatMatcher' in f.resolvedOptions()) throw new Error(JSON.stringify(events));
        let calls = 0;
        new Intl.DateTimeFormat('en',{timeZone:'UTC',dateStyle:'short',
            get formatMatcher() {calls++; return 'basic';}}).format(0);
        if (calls !== 1) throw new Error('style request did not observe matcher exactly once');
        try {new Intl.DateTimeFormat('en',{formatMatcher:'wrong'}); return false;}
        catch(e) {if (!(e instanceof RangeError)) throw e;}
        return true;
    })()"#,
    );
}

#[test]
fn exact_wide_month_patterns_select_year_names_for_multiple_calendars_and_locales() {
    assert_script(
        r#"(() => {
        const time = Date.UTC(2019, 5, 1, 15, 4, 5);
        for (const [locale, calendar] of [['zh','chinese'], ['en','chinese'], ['ko','dangi']]) {
            for (const clock of [false, true]) {
              for (const weekday of [undefined, 'short']) {
                const formatter = new Intl.DateTimeFormat(locale, {
                    calendar, timeZone:'UTC', year:'numeric', month:'long', day:'numeric', weekday,
                    ...(clock ? {hour:'2-digit', minute:'2-digit', second:'2-digit', hourCycle:'h23'} : {})
                });
                const parts = formatter.formatToParts(time), resolved = formatter.resolvedOptions();
                if (resolved.calendar !== calendar || resolved.month !== 'long' ||
                    !parts.some(part => part.type === 'relatedYear' && part.value === '2019') ||
                    !parts.some(part => part.type === 'yearName' && part.value.length > 0) ||
                    parts.some(part => part.type === 'year') ||
                    parts.map(part => part.value).join('') !== formatter.format(time))
                    throw new Error(JSON.stringify({locale, calendar, clock, resolved, parts}));
                if (clock && !['hour','minute','second'].every(type => parts.some(part => part.type === type)))
                    throw new Error('calendar combination dropped clock fields');
                if (weekday && !parts.some(part => part.type === 'weekday'))
                    throw new Error('calendar pattern dropped weekday');
              }
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn calendar_pattern_width_changes_keep_source_fields_and_native_year_names() {
    assert_script(
        r#"(() => {
        const long = new Intl.DateTimeFormat('zh-u-ca-chinese', {
            timeZone:'UTC', year:'numeric', month:'long', day:'numeric'
        });
        const short = new Intl.DateTimeFormat('zh-u-ca-chinese', {
            timeZone:'UTC', year:'numeric', month:'short', day:'numeric'
        });
        for (const [year, name] of [[2018,'戊戌'], [2019,'己亥'], [2020,'庚子']]) {
            const time = Date.UTC(year, 5, 1);
            const parts = long.formatToParts(time), brief = short.formatToParts(time);
            if (!parts.some(part => part.type === 'yearName' && part.value === name) ||
                !parts.some(part => part.type === 'relatedYear' && part.value === String(year)) ||
                brief.some(part => part.type === 'yearName') ||
                parts.map(part => part.value).join('') !== long.format(time) ||
                brief.map(part => part.value).join('') !== short.format(time))
                throw new Error(JSON.stringify({year, parts, brief}));
        }
        return true;
    })()"#,
    );
}

#[test]
fn unsupported_calendar_and_numbering_options_fall_back_without_locale_additions() {
    assert_script(
        r#"(() => {
        for (const locale of ['en', 'de', 'ja', 'th', 'fa', 'ar', 'bn']) {
            const options = {timeZone: 'UTC', year: 'numeric', month: 'numeric', day: 'numeric'};
            const baseline = new Intl.DateTimeFormat(locale, options);
            const fallback = new Intl.DateTimeFormat(locale, {
                ...options, calendar: 'foobar', numberingSystem: 'foobar'
            });
            const a = baseline.resolvedOptions(), b = fallback.resolvedOptions();
            if (a.locale !== b.locale || a.calendar !== b.calendar ||
                a.numberingSystem !== b.numberingSystem || baseline.format(0) !== fallback.format(0))
                throw new Error(locale + ': unsupported option changed formatting');
        }
        return true;
    })()"#,
    );
}

#[test]
fn supported_extensions_survive_unsupported_options_and_equal_options() {
    assert_script(
        r#"(() => {
        const locale = 'en-u-ca-buddhist-hc-h11-nu-arab';
        const options = {timeZone: 'UTC', year: 'numeric', hour: 'numeric'};
        for (const extra of [{}, {calendar:'foobar', numberingSystem:'foobar'},
                            {calendar:'buddhist', numberingSystem:'arab', hourCycle:'h11'}]) {
            const formatter = new Intl.DateTimeFormat(locale, {...options, ...extra});
            const result = formatter.resolvedOptions();
            if (result.locale !== locale || result.calendar !== 'buddhist' ||
                result.numberingSystem !== 'arab' || result.hourCycle !== 'h11')
                throw new Error(JSON.stringify({extra, result}));
            const parts = formatter.formatToParts(0);
            if (!parts.some(part => part.type === 'year' && part.value === '٢٥١٣') ||
                !parts.some(part => part.type === 'hour' && part.value === '٠'))
                throw new Error(JSON.stringify({extra, result, parts}));
        }
        return true;
    })()"#,
    );
}

#[test]
fn options_override_formatting_without_inserting_their_values_into_public_locale() {
    assert_script(
        r#"(() => {
        const formatter = new Intl.DateTimeFormat('en-u-ca-buddhist-hc-h11-nu-arab', {
            calendar:'gregory', numberingSystem:'latn', hourCycle:'h24',
            timeZone:'UTC', year:'numeric', hour:'numeric'
        });
        const result = formatter.resolvedOptions();
        const parts = formatter.formatToParts(0);
        const correct = result.locale === 'en' && result.calendar === 'gregory' &&
            result.numberingSystem === 'latn' && result.hourCycle === 'h24' &&
            parts.some(part => part.type === 'year' && part.value === '1970') &&
            parts.some(part => part.type === 'hour' && part.value === '24') &&
            parts.map(part => part.value).join('') === formatter.format(0);
        if (!correct) throw new Error(JSON.stringify({result, parts}));
        return true;
    })()"#,
    );
}

#[test]
fn calendar_periods_combine_with_clocks_using_the_selected_calendar_and_digits() {
    assert_script(
        r#"(() => {
        for (const locale of ['en', 'de', 'ja', 'ar', 'th']) {
            for (const date of [{year:'numeric'}, {month:'long'}, {year:'numeric',month:'short'}]) {
                const formatter = new Intl.DateTimeFormat(locale, {
                    ...date, calendar:'gregory', numberingSystem:'latn',
                    timeZone:'UTC', hour:'2-digit', hourCycle:'h24'
                });
                const parts = formatter.formatToParts(0);
                if (!parts.some(part => part.type === 'hour' && part.value === '24') ||
                    !!date.year !== parts.some(part => part.type === 'year' && part.value === '1970') ||
                    !!date.month !== parts.some(part => part.type === 'month') ||
                    parts.some(part => part.type === 'day') ||
                    parts.map(part => part.value).join('') !== formatter.format(0))
                    throw new Error(JSON.stringify({locale,date,parts}));
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn unsupported_and_irrelevant_extensions_are_removed_from_resolved_locale() {
    assert_script(
        r#"(() => {
        const formatter = new Intl.DateTimeFormat('en-u-ca-foobar-co-phonebk-hc-foobar-nu-foobar', {
            timeZone:'UTC', hour:'numeric'
        });
        const baseline = new Intl.DateTimeFormat('en', {timeZone:'UTC', hour:'numeric'});
        return formatter.resolvedOptions().locale === 'en' &&
            formatter.format(0) === baseline.format(0);
    })()"#,
    );
}

#[test]
fn locale_default_calendars_use_likely_regions_and_iso_uses_civil_iso_fields() {
    assert_script(
        r#"(() => {
        const options = {timeZone:'UTC', year:'numeric', numberingSystem:'latn'};
        const thai = new Intl.DateTimeFormat('th', options);
        const persian = new Intl.DateTimeFormat('fa', options);
        if (thai.resolvedOptions().calendar !== 'buddhist' ||
            !thai.formatToParts(0).some(part => part.type === 'year' && part.value === '2513') ||
            persian.resolvedOptions().calendar !== 'persian' ||
            !persian.formatToParts(0).some(part => part.type === 'year' && part.value === '1348')) return false;
        for (const locale of ['th', 'fa', 'en', 'ja', 'de']) {
            const formatter = new Intl.DateTimeFormat(locale, {...options, calendar:'iso8601'});
            if (formatter.resolvedOptions().calendar !== 'iso8601' ||
                !formatter.formatToParts(0).some(part => part.type === 'year' && part.value === '1970'))
                throw new Error(locale + ': ISO calendar retained locale-default calculation');
        }
        return true;
    })()"#,
    );
}

#[test]
fn all_four_hour_cycles_format_midnight_and_noon_and_report_the_same_cycle() {
    assert_script(
        r#"(() => {
        const expected = {h11:['00','00',true], h12:['12','12',true],
                          h23:['00','12',false], h24:['24','12',false]};
        for (const cycle of Object.keys(expected)) {
            for (const requested of ['en', 'en-u-hc-' + cycle]) {
                const options = {timeZone:'UTC', hour:'2-digit', minute:'2-digit'};
                if (requested === 'en') options.hourCycle = cycle;
                const formatter = new Intl.DateTimeFormat(requested, options);
                const result = formatter.resolvedOptions();
                if (result.hourCycle !== cycle || result.hour12 !== expected[cycle][2]) return false;
                for (const [index, time] of [[0,0],[1,43200000]]) {
                    const parts = formatter.formatToParts(time);
                    if (!parts.some(part => part.type === 'hour' && part.value === expected[cycle][index]) ||
                        parts.some(part => part.type === 'dayPeriod') !== expected[cycle][2] ||
                        parts.map(part => part.value).join('') !== formatter.format(time))
                        throw new Error(requested + '/' + cycle + ': hour output disagrees');
                }
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn h24_midnight_uses_selected_digits_and_keeps_the_date_and_minutes() {
    assert_script(
        r#"(() => {
        const formatter = new Intl.DateTimeFormat('en', {
            timeZone:'UTC', numberingSystem:'arab', hourCycle:'h24', year:'numeric',
            month:'2-digit', day:'2-digit', hour:'2-digit', minute:'2-digit'
        });
        const time = Date.UTC(2020,0,2,0,7), parts = formatter.formatToParts(time);
        return formatter.resolvedOptions().locale === 'en' &&
            formatter.resolvedOptions().numberingSystem === 'arab' &&
            parts.some(part => part.type === 'hour' && part.value === '٢٤') &&
            parts.some(part => part.type === 'minute' && part.value === '٠٧') &&
            parts.some(part => part.type === 'day' && part.value === '٠٢') &&
            parts.map(part => part.value).join('') === formatter.format(time);
    })()"#,
    );
}

#[test]
fn hour12_overrides_hc_but_keeps_other_supported_extensions() {
    assert_script(
        r#"(() => {
        for (const hour12 of [true,false]) {
            const formatter = new Intl.DateTimeFormat('en-u-hc-h11-nu-arab', {
                hour12, hourCycle:'h24', timeZone:'UTC', hour:'numeric'
            });
            const result = formatter.resolvedOptions();
            if (result.locale !== 'en-u-nu-arab' || result.hour12 !== hour12 ||
                result.hourCycle !== (hour12 ? 'h12' : 'h23')) return false;
            if (!formatter.formatToParts(0).some(part => part.type === 'hour' &&
                part.value === (hour12 ? '١٢' : '٠٠'))) return false;
        }
        const noHour = new Intl.DateTimeFormat('en-u-hc-h24', {timeZone:'UTC', year:'numeric'}).resolvedOptions();
        return noHour.locale === 'en-u-hc-h24' && !('hourCycle' in noHour) && !('hour12' in noHour);
    })()"#,
    );
}

#[test]
fn time_style_h24_formats_real_midnight_without_exposing_component_options() {
    assert_script(
        r#"(() => {
        const formatter = new Intl.DateTimeFormat('de', {timeZone:'UTC', timeStyle:'short', hourCycle:'h24'});
        const result = formatter.resolvedOptions(), parts = formatter.formatToParts(0);
        return result.hourCycle === 'h24' && result.hour12 === false && result.timeStyle === 'short' &&
            !('hour' in result) && !('minute' in result) &&
            parts.some(part => part.type === 'hour' && part.value === '24') &&
            parts.map(part => part.value).join('') === formatter.format(0);
    })()"#,
    );
}

#[test]
fn option_getters_keep_spec_order_and_hour_cycle_is_read_even_with_hour12() {
    assert_script(
        r#"(() => {
        const observed = [], values = {hour12:false, hourCycle:'h24', timeZone:'UTC'};
        const options = new Proxy(values, {get(target,key) { observed.push(key); return target[key]; }});
        new Intl.DateTimeFormat('en', options);
        const expected = ['localeMatcher','calendar','numberingSystem','hour12','hourCycle','timeZone',
            'weekday','era','year','month','day','dayPeriod','hour','minute','second',
            'fractionalSecondDigits','timeZoneName','formatMatcher','dateStyle','timeStyle'];
        if (JSON.stringify(observed) !== JSON.stringify(expected)) throw new Error(JSON.stringify(observed));
        const calls = [];
        try { new Intl.DateTimeFormat('en', {get calendar() {calls.push('calendar'); return 'ab';},
            get numberingSystem() {calls.push('numberingSystem'); return 'latn';}}); return false; }
        catch (error) { if (!(error instanceof RangeError)) throw error; }
        return JSON.stringify(calls) === '["calendar"]';
    })()"#,
    );
}

#[test]
fn day_period_and_all_time_zone_name_styles_format_parts_and_ranges() {
    assert_script(
        r#"(() => {
        const instant = Date.UTC(2024, 0, 2, 13, 4, 5);
        for (const formatMatcher of ['basic', 'best fit']) {
            for (const timeZoneName of [
                'short', 'long', 'shortOffset', 'longOffset', 'shortGeneric', 'longGeneric'
            ]) {
                const formatter = new Intl.DateTimeFormat('en-US', {
                    formatMatcher, timeZone:'UTC', timeZoneName
                });
                const resolved = formatter.resolvedOptions();
                const parts = formatter.formatToParts(instant);
                if (resolved.timeZoneName !== timeZoneName ||
                    !resolved.year || !resolved.month || !resolved.day ||
                    !parts.some(part => part.type === 'timeZoneName') ||
                    parts.map(part => part.value).join('') !== formatter.format(instant))
                    throw new Error(JSON.stringify({formatMatcher,timeZoneName,resolved,parts}));

                const dateZone = new Intl.DateTimeFormat('en-US', {
                    formatMatcher, timeZone:'UTC', weekday:'long', timeZoneName
                });
                const dateZoneParts = dateZone.formatToParts(instant);
                if (!dateZone.resolvedOptions().weekday ||
                    !dateZoneParts.some(part => part.type === 'weekday') ||
                    !dateZoneParts.some(part => part.type === 'timeZoneName') ||
                    dateZoneParts.map(part => part.value).join('') !== dateZone.format(instant))
                    throw new Error(JSON.stringify({formatMatcher,timeZoneName,dateZoneParts}));

                const range = formatter.formatRangeToParts(instant, instant + 86400000);
                if (!range.some(part => part.type === 'timeZoneName' && part.source === 'shared') ||
                    range.map(part => part.value).join('') !==
                        formatter.formatRange(instant, instant + 86400000))
                    throw new Error(JSON.stringify({formatMatcher,timeZoneName,range}));
            }

            for (const options of [
                {dayPeriod:'short'},
                {dayPeriod:'long', hour:'numeric'},
                {dayPeriod:'narrow', hour:'2-digit'}
            ]) {
                const formatter = new Intl.DateTimeFormat('en-US', {
                    ...options, formatMatcher, timeZone:'UTC'
                });
                const resolved = formatter.resolvedOptions();
                const parts = formatter.formatToParts(instant);
                if (!resolved.dayPeriod ||
                    !parts.some(part => part.type === 'dayPeriod') ||
                    ('hour' in options) !== parts.some(part => part.type === 'hour') ||
                    parts.map(part => part.value).join('') !== formatter.format(instant))
                    throw new Error(JSON.stringify({formatMatcher,options,resolved,parts}));

                const range = formatter.formatRangeToParts(
                    Date.UTC(2024, 0, 2, 1, 4, 5), instant
                );
                if (!range.some(part => part.type === 'dayPeriod') ||
                    range.map(part => part.value).join('') !== formatter.formatRange(
                        Date.UTC(2024, 0, 2, 1, 4, 5), instant
                    ))
                    throw new Error(JSON.stringify({formatMatcher,options,range}));
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn time_zone_text_writes_are_repeatable_and_match_parts() {
    assert_script(
        r#"(() => {
        const instants = [
            Date.UTC(2024, 0, 2, 0, 0, 0),
            Date.UTC(2024, 6, 2, 13, 4, 5)
        ];
        for (const locale of ['en-US', 'de', 'ar']) {
            for (const timeZoneName of ['shortOffset', 'longOffset']) {
                const formatter = new Intl.DateTimeFormat(locale, {
                    timeZone:'America/New_York', timeZoneName,
                    year:'numeric', month:'2-digit', day:'2-digit',
                    hour:'2-digit', minute:'2-digit', second:'2-digit'
                });
                const expected = instants.map(value => formatter.format(value));
                const expectedZones = instants.map(value => formatter.formatToParts(value)
                    .find(part => part.type === 'timeZoneName').value);
                if (expectedZones[0] === expectedZones[1])
                    throw new Error(JSON.stringify({locale,timeZoneName,expectedZones}));
                for (let pass = 0; pass < 3; pass++) {
                    for (let index = 0; index < instants.length; index++) {
                        const value = formatter.format(instants[index]);
                        const parts = formatter.formatToParts(instants[index]);
                        const zones = parts.filter(part => part.type === 'timeZoneName');
                        if (value !== expected[index] ||
                            parts.map(part => part.value).join('') !== value ||
                            zones.length !== 1 || zones[0].value !== expectedZones[index])
                            throw new Error(JSON.stringify({locale,timeZoneName,pass,index,value,parts}));
                    }
                }
            }
        }
        return true;
    })()"#,
    );
}
