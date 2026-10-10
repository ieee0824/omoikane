//! Contract tests cover real CLDR results, coercion order and owned GC boundaries.
use crate::{Context, JsValue, NativeFunction, Source, js_string};

fn assert_script(script: &str) {
    let mut context = Context::default();
    let result = context.eval(Source::from_bytes(script)).unwrap();
    assert_eq!(result, JsValue::new(true));
}

#[test]
fn relative_time_preserves_numeric_auto_plural_and_signed_zero() {
    assert_script(
        r#"(() => {
        const r = new Intl.RelativeTimeFormat('en-US');
        if (r.format(1,'day') !== 'in 1 day' || r.format(2,'days') !== 'in 2 days') return false;
        if (r.format(-0,'day') !== '0 days ago' || r.format(0,'day') !== 'in 0 days') return false;
        const auto = new Intl.RelativeTimeFormat('en',{numeric:'auto'});
        if (auto.format(-1,'day') !== 'yesterday' || auto.format(-0,'day') !== 'today') return false;
        if (auto.format(1,'day') !== 'tomorrow' || auto.format(2,'day') !== 'in 2 days') return false;
        if (new Intl.RelativeTimeFormat('ja').format(-1,'day') !== '1 日前') return false;
        if (new Intl.RelativeTimeFormat('de').format(2,'day') !== 'in 2 Tagen') return false;
        for (const style of ['long','short','narrow']) {
            const format = new Intl.RelativeTimeFormat('en',{style});
            for (const unit of ['year','quarter','month','week','day','hour','minute','second']) {
                if (format.format(2,unit) === '2 ' + unit) return false;
            }
        }
        for (const numberingSystem of ['abc','12345678','1234abcd-abc123','arab-true']) {
            if (new Intl.RelativeTimeFormat('en',{numberingSystem}).resolvedOptions().numberingSystem !== 'latn') return false;
        }
        const resolved = r.resolvedOptions();
        return resolved.locale === 'en-US' && resolved.numberingSystem === 'latn' &&
            resolved.style === 'long' && resolved.numeric === 'always';
    })()"#,
    );
}

#[test]
fn relative_time_parts_keep_numeric_unit_grouping_and_fresh_objects() {
    assert_script(
        r#"(() => {
        for (const locale of ['en-US','ja-JP','de-DE','ar']) {
            const r = new Intl.RelativeTimeFormat(locale);
            const parts = r.formatToParts(-1234.5,'days');
            if (parts.map(p=>p.value).join('') !== r.format(-1234.5,'day')) return false;
            if (!parts.some(p=>p.type === 'integer' && p.unit === 'day')) return false;
            if (!parts.some(p=>p.type === 'fraction' && p.unit === 'day')) return false;
            if (parts.some(p=>p.type !== 'literal' && p.unit !== 'day')) return false;
            parts[0].value = 'modified';
            if (r.formatToParts(-1234.5,'day')[0].value === 'modified') return false;
        }
        const arabic = new Intl.RelativeTimeFormat('ar').formatToParts(1,'year');
        if (arabic.length !== 1 || arabic[0].type !== 'literal' || ('unit' in arabic[0])) return false;
        const auto = new Intl.RelativeTimeFormat('en',{numeric:'auto'}).formatToParts(-1,'day');
        return auto.length === 1 && auto[0].type === 'literal' && auto[0].value === 'yesterday' && !('unit' in auto[0]);
    })()"#,
    );
}

#[test]
fn relative_time_brand_and_conversion_order_are_observable() {
    assert_script(
        r#"(() => {
        const r = new Intl.RelativeTimeFormat('en');
        let calls = [];
        const number = {valueOf(){calls.push('number'); return NaN;}};
        const unit = {toString(){calls.push('unit'); return 'invalid';}};
        try {r.format(number,unit); return false;} catch(e) {if (!(e instanceof RangeError)) throw e;}
        if (calls.join(',') !== 'number,unit') return false;
        calls = [];
        try {r.format.call({},number,unit); return false;} catch(e) {if (!(e instanceof TypeError)) throw e;}
        if (calls.length) return false;
        const sentinel = {};
        try {r.format({valueOf(){throw sentinel;}},unit); return false;} catch(e) {if (e !== sentinel) throw e;}
        if (calls.length) return false;
        try {r.format(1n,'day'); return false;} catch(e) {if (!(e instanceof TypeError)) throw e;}
        try {r.format(1,'DAY'); return false;} catch(e) {if (!(e instanceof RangeError)) throw e;}
        return Intl.RelativeTimeFormat.supportedLocalesOf(['en-US','ja-JP','de-DE','bal']).length === 4;
    })()"#,
    );
}

#[test]
fn display_names_uses_all_six_types_and_canonical_fallback() {
    assert_script(
        r#"(() => {
        for (const [type,code,value] of [['region','us','United States'],['script','latn','Latin'],
            ['language','FR','French'],['currency','usd','US Dollar'],
            ['calendar','GREGORY','Gregorian Calendar'],['dateTimeField','month','month']]) {
            const d = new Intl.DisplayNames('en-US',{type});
            if (d.of(code) !== value) throw new Error(type + ': ' + d.of(code));
            const r = d.resolvedOptions();
            if (r.type !== type || r.locale !== 'en-US' || r.style !== 'long' || r.fallback !== 'code') return false;
            if (('languageDisplay' in r) !== (type === 'language')) return false;
        }
        if (new Intl.DisplayNames('en',{type:'currency'}).of('zzz') !== 'ZZZ') return false;
        if (new Intl.DisplayNames('en',{type:'currency',fallback:'none'}).of('zzz') !== undefined) return false;
        if (new Intl.DisplayNames('ja',{type:'region'}).of('US') !== 'アメリカ合衆国') return false;
        return Intl.DisplayNames.supportedLocalesOf(['en-US','ja-JP','de-DE','bal']).length === 4;
    })()"#,
    );
}

#[test]
fn display_names_accepts_long_language_subtags_and_canonicalizes_the_tail() {
    assert_script(
        r#"(() => {
        const code = new Intl.DisplayNames('en', {type:'language'});
        const none = new Intl.DisplayNames('en', {type:'language', fallback:'none'});
        for (const language of ['abcde', 'abcdef', 'abcdefg', 'abcdefgh']) {
            if (code.of(language.toUpperCase()) !== language) return false;
            if (none.of(language) !== undefined) return false;
            const compound = language.toUpperCase() + '-latn-us-ABCDE-1234';
            if (typeof code.of(compound) !== 'string') return false;
            if (none.of(compound) !== undefined) return false;
            try {code.of(language + '-abcde-ABCDE'); return false;}
            catch (e) {if (!(e instanceof RangeError)) throw e;}
        }
        return true;
    })()"#,
    );
}

#[test]
fn display_names_preserves_dialect_standard_width_and_validation() {
    assert_script(
        r#"(() => {
        const dialect = new Intl.DisplayNames('en',{type:'language'});
        const standard = new Intl.DisplayNames('en',{type:'language',languageDisplay:'standard'});
        if (dialect.of('en-GB') !== 'British English' || standard.of('en-GB') !== 'English (United Kingdom)') return false;
        if (standard.of('zh-Hant-TW') !== 'Chinese (Traditional, Taiwan)') return false;
        if (new Intl.DisplayNames('en',{type:'region',style:'short'}).of('US') !== 'US') return false;
        for (const [type,code] of [['language','en-u-ca-gregory'],['language','en_US'],['language','root'],['language','abcd-GB'],['language','aa-aaaaa-aaaaa'],['region','USA'],
            ['script','Latin'],['currency','US'],['calendar','gregorian'],['dateTimeField','week']]) {
            try {new Intl.DisplayNames('en',{type}).of(code); return false;} catch(e) {if (!(e instanceof RangeError)) throw e;}
        }
        for (const options of [undefined,null,1,'x',{}]) {
            try {new Intl.DisplayNames('en',options); return false;} catch(e) {if (!(e instanceof TypeError)) throw e;}
        }
        try {Intl.DisplayNames('en',{type:'region'}); return false;} catch(e) {if (!(e instanceof TypeError)) throw e;}
        let reads = [];
        const options = new Proxy({type:'region'}, {get(target,key){ reads.push(key); return target[key]; }});
        new Intl.DisplayNames('en',options);
        return reads.join(',') === 'localeMatcher,style,type,fallback,languageDisplay';
    })()"#,
    );
}

#[test]
fn display_names_uses_contextual_script_names_and_currency_symbols_for_shorter_styles() {
    assert_script(
        r#"(() => {
        for (const [locale, longScript, script, currencies] of [
            ['en', 'Simplified Han', 'Simplified', ['$', '¥', '€']],
            ['ja', '漢字(簡体字)', '簡体字', ['$', '￥', '€']],
            ['de', 'Vereinfachtes Chinesisch', 'Vereinfacht', ['$', '¥', '€']]
        ]) {
            if (new Intl.DisplayNames(locale, {type:'script'}).of('Hans') !== longScript) return false;
            for (const style of ['short', 'narrow']) {
                const scripts = new Intl.DisplayNames(locale, {type:'script', style});
                if (scripts.of('Hans') !== script) return false;
                const names = new Intl.DisplayNames(locale, {type:'currency', style});
                if (['USD', 'JPY', 'EUR'].map(code => names.of(code)).join('|') !==
                    currencies.join('|')) return false;
            }
        }
        // en-US exercises provider locale fallback to the bundled en payload.
        const short = new Intl.DisplayNames('en-US', {type:'currency', style:'short', fallback:'none'});
        const narrow = new Intl.DisplayNames('en-US', {type:'currency', style:'narrow', fallback:'none'});
        return short.of('CAD') === 'CA$' && narrow.of('CAD') === '$' &&
            short.of('AED') === 'AED' && narrow.of('AED') === 'AED' &&
            short.of('ADP') === 'Andorran Peseta' && narrow.of('ADP') === 'Andorran Peseta' &&
            short.of('ZZZ') === undefined && narrow.of('ZZZ') === undefined;
    })()"#,
    );
}

#[test]
fn name_service_author_coercions_can_reenter_and_collect() {
    let mut context = Context::default();
    context
        .register_global_builtin_callable(
            js_string!("collect"),
            0,
            NativeFunction::from_fn_ptr(|_, _, _| {
                boa_gc::force_collect();
                Ok(JsValue::undefined())
            }),
        )
        .unwrap();
    let result = context.eval(Source::from_bytes(r#"(() => {
        const r = new Intl.RelativeTimeFormat('en');
        const d = new Intl.DisplayNames('en',{type:'region'});
        let calls = 0;
        const number = {valueOf(){ ++calls; collect(); if(r.format(2,'day') !== 'in 2 days') throw new Error('RTF reentry'); return 1; }};
        const unit = {toString(){ ++calls; collect(); return 'day'; }};
        if (r.formatToParts(number,unit).map(p=>p.value).join('') !== 'in 1 day' || calls !== 2) return false;
        const code = {toString(){ ++calls; collect(); if(d.of('GB') !== 'United Kingdom') throw new Error('DN reentry'); return 'US'; }};
        if (d.of(code) !== 'United States' || calls !== 3) return false;
        collect();
        return r.format(-0,'day') === '0 days ago' && d.of('US') === 'United States';
    })()"#)).unwrap();
    assert_eq!(result, JsValue::new(true));
}
