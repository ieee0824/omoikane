use crate::{Context, JsValue, NativeFunction, Source, js_string};

fn assert_script(script: &str, context: &mut Context) {
    let actual = context
        .eval(Source::from_bytes(script))
        .unwrap_or_else(|error| panic!("DateTimeFormat range contract threw {error}"));
    assert_eq!(actual, JsValue::new(true));
}

#[test]
fn range_shape_brand_undefined_and_conversion_order() {
    assert_script(
        r#"(() => {
        const proto = Intl.DateTimeFormat.prototype;
        for (const name of ['formatRange', 'formatRangeToParts']) {
            const method = proto[name], desc = Object.getOwnPropertyDescriptor(proto, name);
            if (method.name !== name || method.length !== 2 || !desc.writable ||
                desc.enumerable || !desc.configurable) return false;
            const length = Object.getOwnPropertyDescriptor(method, 'length');
            if (length.writable || length.enumerable || !length.configurable) return false;
            let calls = [];
            const start = {valueOf() { calls.push('start'); return NaN; }};
            const end = {valueOf() { calls.push('end'); return 0; }};
            for (const receiver of [undefined, null, {}, proto]) {
                try { method.call(receiver, start, end); return false; }
                catch (error) { if (!(error instanceof TypeError)) throw error; }
            }
            if (calls.length) return false;
            const formatter = new Intl.DateTimeFormat('en', {timeZone: 'UTC'});
            for (const args of [[start, undefined], [undefined, end]]) {
                try { method.apply(formatter, args); return false; }
                catch (error) { if (!(error instanceof TypeError)) throw error; }
            }
            if (calls.length) return false;
            try { method.call(formatter, start, end); return false; }
            catch (error) { if (!(error instanceof RangeError)) throw error; }
            if (calls.join() !== 'start,end') return false;
            calls = []; const sentinel = {};
            try { method.call(formatter, {valueOf() { throw sentinel; }}, end); return false; }
            catch (error) { if (error !== sentinel || calls.length) return false; }
            for (const value of [Symbol('date'), 1n]) {
                try { method.call(formatter, value, end); return false; }
                catch (error) { if (!(error instanceof TypeError)) throw error; }
            }
            for (const value of [Infinity, -Infinity, 8640000000000001, -8640000000000001]) {
                try { method.call(formatter, 0, value); return false; }
                catch (error) { if (!(error instanceof RangeError)) throw error; }
            }
        }
        return true;
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn shared_month_year_and_endpoint_parts_are_fresh_and_join_exactly() {
    assert_script(
        r#"(() => {
        const f = new Intl.DateTimeFormat('en', {
            timeZone: 'UTC', year: 'numeric', month: 'short', day: 'numeric'
        });
        const start = Date.UTC(2025, 0, 1), end = Date.UTC(2025, 0, 3);
        const expected = [
            {type:'month', value:'Jan', source:'shared'},
            {type:'literal', value:' ', source:'shared'},
            {type:'day', value:'1', source:'startRange'},
            {type:'literal', value:'\u2009–\u2009', source:'shared'},
            {type:'day', value:'3', source:'endRange'},
            {type:'literal', value:', ', source:'shared'},
            {type:'year', value:'2025', source:'shared'}
        ];
        const parts = f.formatRangeToParts(start, end);
        if (JSON.stringify(parts) !== JSON.stringify(expected) ||
            parts.map(p => p.value).join('') !== f.formatRange(start, end)) return false;
        const again = f.formatRangeToParts(start, end);
        if (parts === again || parts[0] === again[0]) return false;
        for (const part of parts) {
            if (Object.getPrototypeOf(part) !== Object.prototype ||
                Object.keys(part).join() !== 'type,value,source') return false;
            for (const key of Object.keys(part)) {
                const d = Object.getOwnPropertyDescriptor(part, key);
                if (!d.writable || !d.enumerable || !d.configurable) return false;
            }
        }
        parts[0].value = 'changed';
        return f.formatRangeToParts(start, end)[0].value === 'Jan';
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn interval_selection_keeps_the_requested_japanese_source_skeleton() {
    assert_script(
        r#"(() => {
        const start = Date.UTC(2024, 0, 2), end = Date.UTC(2024, 0, 4);
        const expected = [
            {type:'year', value:'2024', source:'shared'},
            {type:'literal', value:'年', source:'shared'},
            {type:'month', value:'1', source:'shared'},
            {type:'literal', value:'月', source:'shared'},
            {type:'day', value:'2', source:'startRange'},
            {type:'literal', value:'日～', source:'shared'},
            {type:'day', value:'4', source:'endRange'},
            {type:'literal', value:'日', source:'shared'}
        ];
        for (const formatMatcher of ['best fit', 'basic']) {
            const f = new Intl.DateTimeFormat('ja', {
                timeZone: 'UTC', year: 'numeric', month: 'short', day: 'numeric', formatMatcher
            });
            const parts = f.formatRangeToParts(start, end);
            if (f.format(start) !== '2024年1月2日' ||
                f.formatRange(start, end) !== '2024年1月2日～4日' ||
                f.formatRange(end, start) !== '2024年1月4日～2日' ||
                JSON.stringify(parts) !== JSON.stringify(expected)) return false;
        }
        return true;
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn descending_display_precision_and_time_zone_boundaries() {
    assert_script(
        r#"(() => {
        const date = new Intl.DateTimeFormat('en', {timeZone:'UTC', year:'numeric', month:'short', day:'numeric'});
        const a = Date.UTC(2025, 0, 1), b = Date.UTC(2025, 0, 3);
        const reversed = date.formatRangeToParts(b, a);
        if (typeof date.formatRange(b, a) !== 'string' ||
            reversed.find(p => p.source === 'startRange' && p.type === 'day').value !== '3' ||
            reversed.find(p => p.source === 'endRange' && p.type === 'day').value !== '1') return false;
        const collapsed = date.formatRangeToParts(a, a + 3600000);
        if (collapsed.some(p => p.source !== 'shared') ||
            collapsed.map(p => p.value).join('') !== date.format(a)) return false;
        const fractional = new Intl.DateTimeFormat('en', {timeZone:'UTC', second:'numeric', fractionalSecondDigits:1});
        if (fractional.formatRangeToParts(a+234, a+239).some(p => p.source !== 'shared')) return false;
        if (!fractional.formatRangeToParts(a+234, a+567).some(p => p.source === 'endRange')) return false;
        const zone = new Intl.DateTimeFormat('en', {timeZone:'+01:00', year:'numeric', month:'short', day:'numeric'});
        const before = Date.UTC(2025, 0, 1, 22, 30), after = Date.UTC(2025, 0, 1, 23, 30);
        const parts = zone.formatRangeToParts(before, after);
        return parts.find(p => p.type === 'day' && p.source === 'startRange').value === '1' &&
            parts.find(p => p.type === 'day' && p.source === 'endRange').value === '2' &&
            parts.map(p => p.value).join('') === zone.formatRange(before, after);
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn coercion_can_reenter_and_collect_with_both_arguments_rooted() {
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
        const f = new Intl.DateTimeFormat('en', {timeZone:'UTC'});
        let calls = [];
        const start = {valueOf() {
            calls.push('start');
            if (f.formatRange(0, 0) !== f.format(0)) throw new Error('reentry');
            collect(); return 0;
        }};
        const end = {valueOf() { calls.push('end'); collect(); return 86400000; }};
        const parts = f.formatRangeToParts(start, end);
        return calls.join() === 'start,end' &&
            parts.map(p => p.value).join('') === f.formatRange(0, 86400000);
    })()"#,
        &mut context,
    );
}

#[test]
fn hidden_larger_fields_supply_locale_calendar_context_and_equal_text_does_not_collapse() {
    assert_script(
        r#"(() => {
        const time = new Intl.DateTimeFormat('en', {timeZone:'UTC', hour:'numeric', minute:'numeric'});
        const start = Date.UTC(2025, 0, 1, 9), end = Date.UTC(2025, 0, 2, 9);
        const context = time.formatRangeToParts(start, end);
        if (!context.some(p => p.type === 'day') ||
            !context.some(p => p.source === 'startRange') ||
            !context.some(p => p.source === 'endRange')) return false;
        const narrow = new Intl.DateTimeFormat('en', {timeZone:'UTC', month:'narrow'});
        const march = Date.UTC(2025, 2, 1), may = Date.UTC(2025, 4, 1);
        if (narrow.format(march) !== narrow.format(may)) return false;
        const parts = narrow.formatRangeToParts(march, may);
        return parts.some(p => p.source === 'startRange') && parts.some(p => p.source === 'endRange');
    })()"#,
        &mut Context::default(),
    );
}

#[test]
fn all_seventeen_calendars_keep_native_fields_and_range_string_parts_agree() {
    assert_script(
        r#"(() => {
        for (const calendar of ['buddhist','chinese','coptic','dangi','ethioaa','ethiopic',
            'gregory','hebrew','indian','islamic','islamic-civil','islamic-rgsa',
            'islamic-tbla','islamic-umalqura','japanese','persian','roc']) {
            const f = new Intl.DateTimeFormat('zh', {calendar, timeZone:'UTC',
                year:'numeric', month:'long', day:'numeric'});
            const start = Date.UTC(1970, 0, 1), end = Date.UTC(2025, 0, 1);
            const parts = f.formatRangeToParts(start, end);
            if (parts.map(p => p.value).join('') !== f.formatRange(start, end) ||
                !parts.some(p => p.source === 'startRange') ||
                !parts.some(p => p.source === 'endRange')) return false;
            for (const [value, source] of [[start,'startRange'],[end,'endRange']]) {
                for (const part of f.formatToParts(value)) {
                    if (part.type === 'relatedYear' || part.type === 'yearName') {
                        if (!parts.some(p => p.type === part.type && p.value === part.value &&
                            (p.source === source || p.source === 'shared'))) return false;
                    }
                }
            }
        }
        return true;
    })()"#,
        &mut Context::default(),
    );
}
