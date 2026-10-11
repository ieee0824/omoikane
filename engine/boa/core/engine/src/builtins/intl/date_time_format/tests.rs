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
fn resolved_options_places_fractional_seconds_before_zone_names() {
    assert_script(
        r#"(() => {
        for (const formatMatcher of ['basic', 'best fit']) {
            const options = new Intl.DateTimeFormat('en-US', {
                formatMatcher, timeZone:'UTC', hour:'numeric', minute:'numeric',
                second:'numeric', fractionalSecondDigits:3, timeZoneName:'short'
            }).resolvedOptions();
            const keys = Object.keys(options);
            if (options.fractionalSecondDigits !== 3 || options.timeZoneName !== 'short' ||
                keys.indexOf('fractionalSecondDigits') >= keys.indexOf('timeZoneName'))
                throw new Error(formatMatcher + ': ' + keys.join(','));
        }
        return true;
        })()"#,
    );
}

#[test]
fn legacy_constructor_shares_the_intl_symbol_and_unwraps_only_supported_methods() {
    assert_script(
        r#"(() => {
        const receiver = Object.create(Intl.DateTimeFormat.prototype);
        const returned = Intl.DateTimeFormat.call(receiver, 'en', {timeZone:'UTC', year:'numeric'});
        if (returned !== receiver) throw new Error('receiver was replaced');
        const symbols = Object.getOwnPropertySymbols(receiver);
        if (symbols.length !== 1 || symbols[0].description !== 'IntlLegacyConstructedSymbol')
            throw new Error('missing legacy symbol');
        const descriptor = Object.getOwnPropertyDescriptor(receiver, symbols[0]);
        if (descriptor.writable || descriptor.enumerable || descriptor.configurable ||
            !(descriptor.value instanceof Intl.DateTimeFormat)) throw new Error('legacy descriptor');
        const number = Object.create(Intl.NumberFormat.prototype);
        Intl.NumberFormat.call(number);
        if (Object.getOwnPropertySymbols(number)[0] !== symbols[0]) throw new Error('symbol differs');
        const proxy = new Proxy(receiver, {get(target, key) {
            if (key !== symbols[0]) throw new Error('unexpected property read');
            return target[key];
        }});
        const options = Intl.DateTimeFormat.prototype.resolvedOptions.call(proxy);
        const getFormat = Object.getOwnPropertyDescriptor(Intl.DateTimeFormat.prototype,'format').get;
        const format = getFormat.call(proxy);
        if (options.timeZone !== 'UTC' || format(Date.UTC(2020,0,1)) !== '2020' ||
            getFormat.call(proxy) !== format) throw new Error('unwrap failed');
        for (const name of ['formatToParts','formatRange','formatRangeToParts']) {
            try { Intl.DateTimeFormat.prototype[name].call(receiver, 0, 1); }
            catch (error) { if (error instanceof TypeError) continue; throw error; }
            throw new Error(name + ' accepted a legacy receiver');
        }
        try { Intl.DateTimeFormat.call(receiver); }
        catch (error) { return error instanceof TypeError; }
        throw new Error('nonconfigurable symbol was replaced');
    })()"#,
    );
}

#[cfg(feature = "temporal")]
#[test]
fn temporal_formatting_preserves_wall_time_kind_calendar_and_coercion_order() {
    assert_script(
        r#"(() => {
        const plain = new Temporal.PlainDateTime(2024,9,19,12,23,37,847);
        const f = new Intl.DateTimeFormat('en', {timeZone:'America/Vancouver',
            calendar:'iso8601', year:'numeric', month:'numeric', day:'numeric',
            hour:'numeric', minute:'numeric', second:'numeric', fractionalSecondDigits:3,
            hourCycle:'h23'});
        const utc = new Intl.DateTimeFormat('en', {...f.resolvedOptions(), timeZone:'UTC'});
        if (f.format(plain) !== utc.format(Date.UTC(2024,8,19,12,23,37,847)) ||
            f.formatToParts(plain).map(p => p.value).join('') !== f.format(plain))
            throw new Error('plain wall time was shifted');
        const year = new Intl.DateTimeFormat('en', {year:'numeric',calendar:'iso8601',timeZone:'UTC'});
        if (year.format(new Temporal.PlainYearMonth(2024,9)) !== '2024')
            throw new Error('year-month format');
        const reject = callback => {
            try { callback(); } catch (error) { if (error instanceof TypeError) return; throw error; }
            throw new Error('expected TypeError');
        };
        reject(() => year.format(new Temporal.PlainTime(12)));
        reject(() => f.formatRange(plain, new Temporal.PlainDate(2024,9,19)));
        let calls = 0;
        reject(() => f.formatRange(plain, {valueOf() { ++calls; return 0; }}));
        if (calls !== 1) throw new Error('end conversion was skipped');
        const gregorian = new Intl.DateTimeFormat('en', {calendar:'gregory',timeZone:'UTC'});
        try { gregorian.format(new Temporal.PlainYearMonth(2024,9)); }
        catch (error) { if (error instanceof RangeError) return true; throw error; }
        throw new Error('year-month calendar mismatch was accepted');
    })()"#,
    );
}

#[cfg(feature = "temporal")]
#[test]
fn temporal_year_month_keeps_reference_dates_outside_instant_limits() {
    assert_script(
        r#"(() => {
        for (const calendar of ['iso8601','gregory']) {
            const f = new Intl.DateTimeFormat('en', {calendar, timeZone:'UTC'});
            for (const [year,month] of [[-271821,4],[275760,9]]) {
                const value = new Temporal.PlainYearMonth(year,month,calendar);
                const text = f.format(value), parts = f.formatToParts(value);
                if (!text.includes(String(month)) || !parts.some(p => p.type === 'year') ||
                    parts.map(p => p.value).join('') !== text ||
                    f.formatRange(value,value) !== text ||
                    f.formatRangeToParts(value,value).map(p => p.value).join('') !== text)
                    throw new Error('YearMonth reference date was clipped');
            }
        }
        return true;
    })()"#,
    );
}

#[cfg(feature = "temporal")]
#[test]
fn temporal_intl_keeps_named_zone_identifiers_while_normalizing_case() {
    assert_script(
        r#"(() => {
        for (const name of ['Etc/GMT','Etc/UTC','GMT','UTC','Asia/Calcutta','Europe/Bratislava']) {
            const ordinary = new Intl.DateTimeFormat('en', {timeZone:name,year:'numeric',month:'numeric',day:'numeric'});
            for (const input of [name,name.toLowerCase(),name.toUpperCase()]) {
                const f = new Intl.DateTimeFormat('en', {timeZone:input,year:'numeric',month:'numeric',day:'numeric'});
                if (f.resolvedOptions().timeZone !== name || f.format(0) !== ordinary.format(0))
                    throw new Error('named zone identifier or offset changed: ' + name);
            }
        }
        return true;
    })()"#,
    );
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
fn regional_twelve_hour_cycle_keeps_japanese_zero_based_hours() {
    assert_script(
        r#"(() => {
        for (const locale of ['en','fr','it','ja','zh','ko','ar','hi']) {
            const formatter = new Intl.DateTimeFormat(locale, {
                timeZone:'UTC', hour:'numeric', hour12:true
            });
            const expected = locale === 'ja' ? 'h11' : 'h12';
            if (formatter.resolvedOptions().hourCycle !== expected) return false;
            if (locale === 'ja' && !formatter.formatToParts(0).some(
                part => part.type === 'hour' && part.value === '0')) return false;
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
fn flexible_day_periods_follow_source_boundaries_and_fractional_wall_time() {
    assert_script(
        r#"(() => {
            for (const dayPeriod of ['long', 'short', 'narrow']) {
                const formatter = new Intl.DateTimeFormat('en', {timeZone:'UTC', dayPeriod});
                const noon = dayPeriod === 'narrow' ? 'n' : 'noon';
                for (const [hour, minute, second, millis, expected] of [
                    [0,0,0,0,'in the morning'], [11,59,59,999,'in the morning'],
                    [12,0,0,0,noon], [12,30,1,1,noon], [13,0,0,0,'in the afternoon'],
                    [17,59,59,999,'in the afternoon'], [18,0,0,0,'in the evening'],
                    [20,59,59,999,'in the evening'], [21,0,0,0,'at night']
                ]) {
                    const instant = Date.UTC(2024,0,1,hour,minute,second,millis);
                    const parts = formatter.formatToParts(instant);
                    if (formatter.format(instant) !== expected ||
                        parts.map(part => part.value).join('') !== expected ||
                        parts.filter(part => part.type === 'dayPeriod').length !== 1)
                        throw new Error(JSON.stringify({dayPeriod,hour,minute,second,millis,parts}));
                }
            }
            for (const [options, minute, second, expected] of [
                [{minute:'numeric'}, 0, 1, 'noon'],
                [{minute:'numeric'}, 1, 0, 'in the afternoon'],
                [{second:'numeric'}, 30, 0, 'noon'],
                [{second:'numeric'}, 0, 1, 'in the afternoon']
            ]) {
                const formatter = new Intl.DateTimeFormat('en', {...options, dayPeriod:'long', timeZone:'UTC'});
                const period = formatter.formatToParts(Date.UTC(2024,0,1,12,minute,second))
                    .find(part => part.type === 'dayPeriod');
                if (period?.value !== expected) throw new Error(JSON.stringify({options,minute,second,period}));
            }
            return true;
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

#[test]
fn lunisolar_temporal_year_roundtrips_related_year_fields() {
    assert_script(
        r#"(() => {
        for (const calendar of ['chinese','dangi']) {
            const date = new Temporal.PlainDate(2050,1,1,calendar);
            if (date.year !== 2049) return false;
            const from = Temporal.PlainDate.from({calendar,year:date.year,month:1,day:30});
            if (from.year !== 2049 || from.toString() !== '2049-03-03[u-ca='+calendar+']')
                return false;
            const parts = new Intl.DateTimeFormat('en', {
                calendar,timeZone:'UTC',year:'numeric',month:'numeric',day:'numeric'
            }).formatToParts(from);
            if (!parts.some(part => part.type === 'relatedYear' && part.value === '2049'))
                return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn hijri_eras_use_source_names_before_and_after_the_epoch() {
    assert_script(
        r#"(() => {
        for (const calendar of ['islamic-civil','islamic-tbla','islamic-umalqura']) {
            for (const era of ['long','short','narrow']) {
                const formatter = new Intl.DateTimeFormat('en', {
                    calendar,timeZone:'UTC',era,year:'numeric'
                });
                const names = [];
                for (const isoYear of [600,2025]) {
                    const date = new Date(0); date.setUTCFullYear(isoYear,5,15);
                    const parts = formatter.formatToParts(date);
                    if (parts.map(p=>p.value).join('') !== formatter.format(date)) return false;
                    const name = parts.find(p=>p.type === 'era');
                    if (!name || !name.value.length) return false;
                    names.push(name.value);
                }
                const expected = era === 'long' ? ['Before Hijrah','Anno Hegirae'] : ['BH','AH'];
                if (JSON.stringify(names) !== JSON.stringify(expected)) return false;
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn lunar_month_numbers_follow_codes_and_preserve_leap_patterns() {
    assert_script(
        r#"(() => {
        for (const calendar of ['chinese','dangi']) {
            for (const [month,expected] of [['numeric','5bis'],['2-digit','05bis'],['narrow','5b']]) {
                const formatter = new Intl.DateTimeFormat('en', {calendar,timeZone:'UTC',month});
                const date = Date.UTC(2047,6,22);
                const parts = formatter.formatToParts(date);
                if (parts.find(p=>p.type==='month')?.value !== expected ||
                    formatter.format(date) !== parts.map(p=>p.value).join('')) return false;
                const range = formatter.formatRangeToParts(date,date);
                if (range.find(p=>p.type==='month')?.value !== expected ||
                    formatter.formatRange(date,date) !== range.map(p=>p.value).join('')) return false;
            }
        }
        const formatter = new Intl.DateTimeFormat('en', {
            calendar:'chinese',timeZone:'UTC',year:'numeric',month:'numeric',day:'numeric'
        });
        const parts = formatter.formatToParts(Date.UTC(2100,0,1));
        if (parts.find(p=>p.type==='month')?.value !== '11' ||
            parts.find(p=>p.type==='day')?.value !== '21' ||
            parts.find(p=>p.type==='relatedYear')?.value !== '2099') return false;
        return true;
    })()"#,
    );
}

#[test]
fn hebrew_adar_names_follow_regular_and_leap_month_codes() {
    assert_script(
        r#"(() => {
        const year = new Temporal.PlainDate(2049,3,4,'hebrew').year;
        for (const [offset,code,name] of [[0,'M05L','Adar I'],[0,'M06','Adar II'],[1,'M06','Adar']]) {
            const date = Temporal.PlainDate.from({calendar:'hebrew',year:year+offset,monthCode:code,day:1});
            if (date.monthCode !== code) return false;
            for (const month of ['numeric','2-digit','short','long']) {
                const formatter = new Intl.DateTimeFormat('en', {calendar:'hebrew',timeZone:'UTC',month});
                const expected = month === 'numeric' ? String(date.month) :
                    month === '2-digit' ? String(date.month).padStart(2,'0') : name;
                const parts = formatter.formatToParts(date);
                if (parts.find(p=>p.type==='month')?.value !== expected ||
                    formatter.format(date) !== parts.map(p=>p.value).join('')) return false;
                const range = formatter.formatRangeToParts(date,date);
                if (range.find(p=>p.type==='month')?.value !== expected ||
                    formatter.formatRange(date,date) !== range.map(p=>p.value).join('')) return false;
            }
            const fields = new Intl.DateTimeFormat('en', {calendar:'hebrew',timeZone:'UTC',
                year:'numeric',month:'numeric',day:'numeric'}).formatToParts(date);
            if (fields.find(p=>p.type==='month')?.value !== name) return false;
        }
        return true;
    })()"#,
    );
}
