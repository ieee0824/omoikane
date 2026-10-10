use crate::builtins::intl::number_format::RoundingIncrement;
use crate::{Context, JsValue, NativeFunction, Source, js_string};
use fixed_decimal::RoundingIncrement::*;

#[test]
fn owned_parts_join_reuses_leading_storage_and_preserves_unicode_text() {
    use super::parts::{NumberPart, parts_into_string, replace_value};

    let mut separator = String::with_capacity(4);
    separator.push('.');
    let separator_allocation = separator.as_ptr();
    replace_value(&mut separator, "\u{202f}");
    assert_eq!(separator, "\u{202f}");
    assert_eq!(separator.as_ptr(), separator_allocation);

    let mut leading = String::with_capacity(64);
    leading.push('\u{2067}');
    let allocation = leading.as_ptr();
    let parts = vec![
        NumberPart {
            kind: "literal",
            value: leading,
        },
        NumberPart {
            kind: "currency",
            value: "د.إ".to_owned(),
        },
        NumberPart {
            kind: "literal",
            value: "\u{a0}".to_owned(),
        },
        NumberPart {
            kind: "integer",
            value: "١٬٢٣٤".to_owned(),
        },
        NumberPart {
            kind: "decimal",
            value: ".".to_owned(),
        },
        NumberPart {
            kind: "fraction",
            value: "٥٠".to_owned(),
        },
        NumberPart {
            kind: "literal",
            value: "\u{2069}".to_owned(),
        },
    ];
    let output = parts_into_string(parts);
    assert_eq!(output, "\u{2067}د.إ\u{a0}١٬٢٣٤.٥٠\u{2069}");
    assert_eq!(output.as_ptr(), allocation);
}

#[test]
fn owned_range_parts_join_reuses_leading_storage_and_preserves_unicode_text() {
    use super::range::{RangePart, parts_into_string};

    let mut leading = String::with_capacity(64);
    leading.push('\u{2067}');
    let allocation = leading.as_ptr();
    let parts = vec![
        RangePart::shared("literal", leading),
        RangePart::shared("currency", "د.إ".to_owned()),
        RangePart::shared("literal", "\u{a0}".to_owned()),
        RangePart::shared("integer", "١٬٢٣٤".to_owned()),
        RangePart::shared("literal", " – ".to_owned()),
        RangePart::shared("integer", "٥٬٦٧٨".to_owned()),
        RangePart::shared("literal", "\u{2069}".to_owned()),
    ];
    let output = parts_into_string(parts);
    assert_eq!(output, "\u{2067}د.إ\u{a0}١٬٢٣٤ – ٥٬٦٧٨\u{2069}");
    assert_eq!(output.as_ptr(), allocation);
    assert_eq!(parts_into_string(Vec::new()), "");
}

#[test]
fn number_text_and_parts_keep_ordinary_currency_and_bidi_separators_in_sync() {
    assert_script(
        r#"(() => {
        const cases = [
            new Intl.NumberFormat('fr', {useGrouping:true}),
            new Intl.NumberFormat('fr', {style:'currency',currency:'EUR'}),
            new Intl.NumberFormat('ar', {numberingSystem:'arab',style:'currency',currency:'USD',signDisplay:'always'})
        ];
        for (const nf of cases) {
            const parts = nf.formatToParts('-1234.5');
            if (nf.format('-1234.5') !== parts.map(part => part.value).join('')) return false;
            if (!parts.some(part => part.type === 'decimal') ||
                !parts.some(part => part.type === 'group')) return false;
        }
        const ordinary = cases[0].formatToParts('1234.5');
        const currency = cases[1].formatToParts('1234.5');
        if (ordinary.find(part => part.type === 'decimal').value !== ',' ||
            ordinary.find(part => part.type === 'group').value !== '\u202f' ||
            currency.find(part => part.type === 'decimal').value !== ',' ||
            currency.find(part => part.type === 'group').value !== '\u202f') return false;
        const bidi = cases[2].formatToParts('-1234.5');
        return bidi.some(part => part.type === 'literal' && /[\u061c\u200e\u200f]/.test(part.value)) &&
            bidi.filter(part => part.type !== 'literal').every(part => !/[\u061c\u200e\u200f]/.test(part.value));
    })()"#,
    );
}

#[test]
fn bigint_locale_string_preserves_large_owned_decimal_input() {
    assert_script(
        r#"(() => {
        const digits = '9'.repeat(16384);
        const value = BigInt(digits);
        return value.toLocaleString('en-US', {useGrouping:false}) === digits &&
            (-value).toLocaleString('en-US', {useGrouping:false}) === '-' + digits &&
            value.toLocaleString('en-US', {
                useGrouping:false, signDisplay:'always'
            }) === '+' + digits;
    })()"#,
    );
}

#[test]
fn owned_pattern_rendering_preserves_numeric_storage_and_typed_affixes() {
    use super::{compact_pattern::CompactPattern, pattern::NumberPattern};
    use super::{parts::NumberPart, parts::parts_to_string};
    use boa_intl_data::OmoikaneNumberSymbolsV1;
    use fixed_decimal::Sign;

    let context = Context::default();
    let symbols = super::data::load::<OmoikaneNumberSymbolsV1>(
        context.intl_provider(),
        &icu_locale::locale!("en"),
        "latn",
    )
    .unwrap();
    let number = || {
        [("integer", "12"), ("decimal", "."), ("fraction", "34")]
            .map(|(kind, value)| NumberPart {
                kind,
                value: value.to_owned(),
            })
            .into_iter()
            .collect::<Vec<_>>()
    };
    let storage = |parts: &[NumberPart]| {
        parts
            .iter()
            .filter(|part| matches!(part.kind, "integer" | "decimal" | "fraction"))
            .map(|part| part.value.as_ptr())
            .collect::<Vec<_>>()
    };
    let input = number();
    let parts_allocation = input.as_ptr();
    let original = storage(&input);
    let parts = NumberPattern::parse("#,##0")
        .unwrap()
        .render(input, Sign::None, symbols.get(), "");
    assert_eq!(parts.as_ptr(), parts_allocation);
    assert_eq!(storage(&parts), original);

    let input = number();
    let original = storage(&input);
    let parts = NumberPattern::parse("¤#,##0.00;(¤#,##0.00)")
        .unwrap()
        .render(input, Sign::Negative, symbols.get(), "USD");
    assert_eq!(parts_to_string(&parts), "(USD12.34)");
    assert_eq!(storage(&parts), original);
    assert_eq!(
        parts.iter().map(|part| part.kind).collect::<Vec<_>>(),
        [
            "literal", "currency", "integer", "decimal", "fraction", "literal"
        ]
    );

    let input = number();
    let original = storage(&input);
    let parts = CompactPattern::parse("0' units';(0' units')")
        .unwrap()
        .render(input, Sign::Negative, symbols.get(), "");
    assert_eq!(parts_to_string(&parts), "(12.34 units)");
    assert_eq!(storage(&parts), original);
}

#[test]
fn owned_placeholders_preserve_repeated_and_omitted_numeric_values() {
    use super::{parts::NumberPart, parts::parts_to_string, pattern::placeholders};
    let number = || {
        vec![NumberPart {
            kind: "integer",
            value: "١٢٣٤٥".to_owned(),
        }]
    };
    let input = number();
    let original = input[0].value.as_ptr();
    let parts = placeholders("値 {0} / {0} : {1} ({0})", input, "EUR", "currency");
    assert_eq!(parts_to_string(&parts), "値 ١٢٣٤٥ / ١٢٣٤٥ : EUR (١٢٣٤٥)");
    let integers: Vec<_> = parts.iter().filter(|part| part.kind == "integer").collect();
    assert_eq!(integers.len(), 3);
    assert_eq!(integers[2].value.as_ptr(), original);
    assert_ne!(integers[0].value.as_ptr(), original);
    assert_ne!(integers[1].value.as_ptr(), original);
    let input = number();
    let original = input[0].value.as_ptr();
    let parts = placeholders("{1} {0}", input, "EUR", "currency");
    assert_eq!(parts_to_string(&parts), "EUR ١٢٣٤٥");
    assert_eq!(parts.last().unwrap().value.as_ptr(), original);
    assert_eq!(
        parts_to_string(&placeholders("unit", number(), "", "unit")),
        "unit"
    );
    assert_eq!(
        parts_to_string(&placeholders("{{0}}", number(), "", "unit")),
        "{١٢٣٤٥}"
    );
}

#[test]
fn range_rendered_stream_equality_preserves_unicode_and_partition_boundaries() {
    use super::{parts::NumberPart, range::equal_rendered_values};
    let parts = |values: &[&str]| {
        values
            .iter()
            .map(|value| NumberPart {
                kind: "literal",
                value: (*value).to_owned(),
            })
            .collect::<Vec<_>>()
    };
    for (left, right) in [
        (vec![], vec![""]),
        (vec!["", "", ""], vec![]),
        (
            vec!["é", "", "\u{200f}", "𐐷", "三"],
            vec!["é\u{200f}", "𐐷三"],
        ),
        (vec!["12", ".", "34"], vec!["1", "2.3", "4"]),
    ] {
        assert!(equal_rendered_values(&parts(&left), &parts(&right)));
    }
    for (left, right) in [
        (vec![], vec!["a"]),
        (vec!["é"], vec!["e\u{301}"]),
        (vec!["a"], vec!["ab"]),
        (vec!["\u{200f}", "a"], vec!["a", "\u{200f}"]),
    ] {
        assert!(!equal_rendered_values(&parts(&left), &parts(&right)));
    }
    let start = vec![NumberPart {
        kind: "integer",
        value: "1".to_owned(),
    }];
    assert!(equal_rendered_values(&start, &parts(&["1"])));
}

#[test]
fn range_methods_have_ordinary_builtin_property_and_brand_contracts() {
    assert_script(
        r#"(() => {
        for (const name of ['formatRange','formatRangeToParts']) {
            const f = Intl.NumberFormat.prototype[name];
            const p = Object.getOwnPropertyDescriptor(Intl.NumberFormat.prototype,name);
            if (f.name !== name || f.length !== 2 || !p.writable || p.enumerable || !p.configurable ||
                Object.getPrototypeOf(f) !== Function.prototype || !Object.isExtensible(f) ||
                Object.hasOwn(f,'prototype')) return false;
            for (const key of ['name','length']) {
                const d = Object.getOwnPropertyDescriptor(f,key);
                if (d.writable || d.enumerable || !d.configurable) return false;
            }
            for (const receiver of [undefined,null,1,'x',{},Intl.NumberFormat.prototype]) {
                try { f.call(receiver,1,2); return false; } catch(e) { if (!(e instanceof TypeError)) throw e; }
            }
            try { Reflect.construct(f,[1,2]); return false; } catch(e) { if (!(e instanceof TypeError)) throw e; }
            const legacy = Object.create(Intl.NumberFormat.prototype);
            Intl.NumberFormat.call(legacy,'en');
            try { f.call(legacy,1,2); return false; } catch(e) { if (!(e instanceof TypeError)) throw e; }
        }
        return true;
    })()"#,
    );
}

#[test]
fn range_endpoint_coercion_orders_brand_undefined_start_end_and_nan_checks() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('en');
        for (const name of ['formatRange','formatRangeToParts']) {
            let log = [];
            const start = {valueOf(){ log.push('start'); return 1; }};
            const end = {valueOf(){ log.push('end'); return 2; }};
            nf[name](start,end);
            if (log.join(',') !== 'start,end') return false;
            log = [];
            for (const args of [[],[start],[undefined,end],[start,undefined]]) {
                try { nf[name](...args); return false; } catch(e) { if (!(e instanceof TypeError)) throw e; }
            }
            if (log.length) return false;
            try { nf[name].call({},start,end); return false; } catch(e) { if (!(e instanceof TypeError)) throw e; }
            if (log.length) return false;
            const sentinel = {};
            try { nf[name]({valueOf(){ throw sentinel; }},end); return false; }
            catch(e) { if (e !== sentinel) throw e; }
            if (log.length) return false;
            try { nf[name](NaN,Symbol()); return false; } catch(e) { if (!(e instanceof TypeError)) throw e; }
            for (const args of [[NaN,2],[1,NaN],['invalid',2],[1,'invalid']]) {
                try { nf[name](...args); return false; } catch(e) { if (!(e instanceof RangeError)) throw e; }
            }
            for (const args of [[Symbol(),2],[1,Symbol()]]) {
                try { nf[name](...args); return false; } catch(e) { if (!(e instanceof TypeError)) throw e; }
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn range_keeps_exact_bigints_strings_descending_infinities_and_signed_zero() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('en');
        const a = '987654321987654321', b = '987654321987654322';
        const expected = '987,654,321,987,654,321–987,654,321,987,654,322';
        if (nf.formatRange(a,b) !== expected || nf.formatRange(BigInt(a),BigInt(b)) !== expected) return false;
        if (nf.formatRange(23,12) !== '23–12' || nf.formatRange(23n,12n) !== '23–12') return false;
        if (nf.formatRange(Infinity,-Infinity) !== '∞ – -∞' ||
            nf.formatRange(-Infinity,Infinity) !== '-∞ – ∞' ||
            nf.formatRange(0,-0) !== '0 – -0' || nf.formatRange(-0,0) !== '-0–0') return false;
        for (const args of [[23,-Infinity],[Infinity,23],[-0,-1]]) {
            const parts = nf.formatRangeToParts(...args);
            if (parts.map(p=>p.value).join('') !== nf.formatRange(...args) ||
                !parts.some(p=>p.source==='startRange') || !parts.some(p=>p.source==='endRange')) return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn range_currency_examples_preserve_actual_test262_en_us_and_pt_pt_outputs() {
    assert_script(
        r#"(() => {
        const en = new Intl.NumberFormat('en-US',{style:'currency',currency:'USD',maximumFractionDigits:0});
        if (en.formatRange(3,5) !== '$3 – $5' || en.formatRange(2.9,3.1) !== '~$3') return false;
        const plus = new Intl.NumberFormat('en-US',{style:'currency',currency:'USD',signDisplay:'always'});
        if (plus.formatRange(2.9,3.1) !== '+$2.90–3.10') return false;
        const pt = new Intl.NumberFormat('pt-PT',{style:'currency',currency:'EUR',maximumFractionDigits:0});
        if (pt.formatRange(3,5) !== '3 - 5\u00a0€' || pt.formatRange(2.9,3.1) !== '~3\u00a0€') return false;
        const ptPlus = new Intl.NumberFormat('pt-PT',{style:'currency',currency:'EUR',signDisplay:'always'});
        if (ptPlus.formatRange(2.9,3.1) !== '+2,90 - 3,10\u00a0€') return false;
        const plain = new Intl.NumberFormat('pt-PT');
        return plain.formatRange('987654321987654321','987654321987654322') ===
            '987\u00a0654\u00a0321\u00a0987\u00a0654\u00a0321 - 987\u00a0654\u00a0321\u00a0987\u00a0654\u00a0322';
    })()"#,
    );
}

#[test]
fn range_parts_match_original_source_and_ordinary_property_descriptors() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('en-US',{style:'currency',currency:'USD',maximumFractionDigits:0});
        const parts = nf.formatRangeToParts(3,5);
        const expected = [['currency','$','startRange'],['integer','3','startRange'],
            ['literal',' – ','shared'],['currency','$','endRange'],['integer','5','endRange']];
        if (JSON.stringify(parts.map(p=>[p.type,p.value,p.source])) !== JSON.stringify(expected)) return false;
        const other = nf.formatRangeToParts(3,5);
        return parts !== other && Object.getPrototypeOf(parts) === Array.prototype && parts.every((p,i)=>
            p !== other[i] && Object.getPrototypeOf(p) === Object.prototype &&
            Object.keys(p).join(',') === 'type,value,source' && ['type','value','source'].every(key=>{
                const d = Object.getOwnPropertyDescriptor(p,key);
                return d.writable && d.enumerable && d.configurable;
            }));
    })()"#,
    );
}

#[test]
fn range_equal_and_rounded_equal_outputs_are_approximate_and_fully_shared() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('en-US',{style:'currency',currency:'USD',maximumFractionDigits:0});
        const expected = [['approximatelySign','~','shared'],['currency','$','shared'],['integer','3','shared']];
        for (const args of [[3,3],[2.999,3.001]]) {
            const p = nf.formatRangeToParts(...args);
            if (JSON.stringify(p.map(p=>[p.type,p.value,p.source])) !== JSON.stringify(expected) ||
                p.map(p=>p.value).join('') !== nf.formatRange(...args)) return false;
        }
        const plain = new Intl.NumberFormat('en');
        if (plain.formatRange(Infinity,Infinity) !== '~∞' || plain.formatRange(-0,-0) !== '~-0') return false;
        const never = new Intl.NumberFormat('en',{signDisplay:'never'});
        return never.formatRange(-0,0) === '~0' && never.formatRangeToParts(-1,1).every(p=>p.source==='shared');
    })()"#,
    );
}

#[test]
fn range_approximately_sign_uses_resolved_symbols_without_legacy_pattern_spaces() {
    assert_script(
        r#"(() => {
        for (const [locale,options,sign,digit] of [
            ['ja',{},'約','1'], ['fr',{},'≃','1'], ['yo',{},'dáàṣì','1'],
            ['yo-BJ',{},'dáàshì','1'], ['ar-EG',{},'~','١'],
            ['ja',{numberingSystem:'arab'},'約','١']]) {
            const nf = new Intl.NumberFormat(locale,options);
            const p = nf.formatRangeToParts(1,1);
            if (nf.formatRange(1,1) !== sign+digit || p.length !== 2 ||
                p[0].type !== 'approximatelySign' || p[0].value !== sign ||
                p[1].type !== 'integer' || p[1].value !== digit ||
                !p.every(p=>p.source==='shared')) throw new Error(locale+': '+JSON.stringify(p));
        }
        return true;
    })()"#,
    );
}

#[test]
fn range_unit_and_currency_name_collapse_reselects_ordered_plural_grammar() {
    assert_script(
        r#"(() => {
        const en = new Intl.NumberFormat('en',{style:'unit',unit:'meter',unitDisplay:'long'});
        if (en.formatRange(1,2) !== '1–2 meters' || en.formatRange(-1,-2) !== '-1 – -2 meters') return false;
        const ru = new Intl.NumberFormat('ru',{style:'unit',unit:'meter',unitDisplay:'long'});
        if (ru.formatRange(1,2) !== '1–2 метра') return false;
        const ar = new Intl.NumberFormat('ar-EG',{style:'unit',unit:'meter',unitDisplay:'long'});
        if (ar.formatRange(1,2) !== '١–٢ متر') throw new Error(ar.formatRange(1,2));
        const narrow = new Intl.NumberFormat('en',{style:'unit',unit:'meter',unitDisplay:'narrow'});
        if (narrow.formatRange(1,2) !== '1–2m') return false;
        const names = new Intl.NumberFormat('en',{style:'currency',currency:'USD',currencyDisplay:'name',maximumFractionDigits:0});
        if (names.formatRange(1,2) !== '1–2 US dollars') return false;
        const p = en.formatRangeToParts(1,2);
        return p.at(-1).type === 'unit' && p.at(-1).value === 'meters' && p.at(-1).source === 'shared' &&
            p.filter(p=>p.type==='integer').map(p=>p.source).join(',') === 'startRange,endRange';
    })()"#,
    );
}

#[test]
fn range_does_not_collapse_compact_or_scientific_numeric_notation() {
    assert_script(
        r#"(() => {
        const scientific = new Intl.NumberFormat('en',{notation:'scientific'});
        const compact = new Intl.NumberFormat('en',{notation:'compact'});
        for (const [nf,a,b,expected,kind] of [
            [scientific,3,5,'3E0 – 5E0','exponentSeparator'],
            [compact,3000,5000,'3K – 5K','compact']]) {
            const parts = nf.formatRangeToParts(a,b);
            if (nf.formatRange(a,b) !== expected || parts.filter(p=>p.type===kind).length !== 2 ||
                parts.filter(p=>p.type===kind).map(p=>p.source).join(',') !== 'startRange,endRange') return false;
        }
        return compact.formatRange(3,5) === '3–5';
    })()"#,
    );
}

#[test]
fn range_value_conversion_reenters_and_collects_before_native_state_is_borrowed() {
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
        const nf = new Intl.NumberFormat('en',{style:'currency',currency:'USD',maximumFractionDigits:0});
        const bound = nf.format;
        let calls = 0;
        const start = {valueOf(){ ++calls; collect();
            if (nf.formatRange(3,5) !== '$3 – $5' || bound(2) !== '$2') throw new Error('reentry'); return 1; }};
        const end = {valueOf(){ ++calls; collect(); return 2; }};
        if (nf.formatRangeToParts(start,end).map(p=>p.value).join('') !== '$1 – $2' || calls !== 2) return false;
        const sentinel = {};
        try { nf.formatRange({valueOf(){ collect(); throw sentinel; }},end); return false; }
        catch(e) { if (e !== sentinel) throw e; }
        collect();
        return calls === 2 && bound(3) === '$3' && nf.formatRange(3,5) === '$3 – $5';
    })()"#)).unwrap();
    assert_eq!(result, JsValue::new(true));
}

#[test]
fn range_plural_override_does_not_change_scalar_rounding_or_compact_patterns() {
    use super::{CompactDisplay, Notation, UnitDisplay, UnitFormatOptions};
    use super::{MathematicalValue, NativeNumberFormatter, NumericFormatOptions};
    use boa_intl_data::PluralCategory;
    use fixed_decimal::{Decimal, SignDisplay, SignedRoundingMode, UnsignedRoundingMode};
    use icu_decimal::options::GroupingStrategy;

    let context = Context::default();
    let style = UnitFormatOptions::Unit {
        unit: "meter".parse().unwrap(),
        display: UnitDisplay::Long,
    };
    for (locale, display) in [
        (icu_locale::locale!("en"), CompactDisplay::Short),
        (icu_locale::locale!("fr"), CompactDisplay::Long),
    ] {
        let formatter = NativeNumberFormatter::new_with_notation(
            context.intl_provider(),
            &locale,
            Some("latn"),
            &style,
            GroupingStrategy::Auto,
            Notation::Compact { display },
        )
        .unwrap();
        let options = NumericFormatOptions::fraction(
            1,
            0,
            1,
            SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand),
            SignDisplay::Auto,
        );
        for input in ["1000", "1999.9", "999500", "-1000"] {
            let value = MathematicalValue::Finite(Decimal::try_from_str(input).unwrap());
            let scalar = formatter.format(value.clone(), &options);
            let ordinary = formatter.format_endpoint(value.clone(), &options, None);
            let forced = formatter.format_endpoint(value, &options, Some(PluralCategory::One));
            assert_eq!(scalar, ordinary.parts);
            assert_eq!(ordinary.category, forced.category);
            let numeric = |parts: Vec<super::NumberPart>| {
                parts
                    .into_iter()
                    .filter(|part| !matches!(part.kind, "unit" | "literal"))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                numeric(ordinary.parts),
                numeric(forced.parts),
                "{locale} / {input}"
            );
        }
    }
}

#[test]
fn native_plural_rules_are_loaded_only_for_compact_or_grammatical_styles() {
    use super::{
        CompactDisplay, CurrencyDisplay, CurrencySign, NativeNumberFormatter, Notation,
        UnitDisplay, UnitFormatOptions,
    };
    use icu_decimal::options::GroupingStrategy;

    let context = Context::default();
    let locale = icu_locale::locale!("en");
    let formatter = |style: &UnitFormatOptions, notation| {
        NativeNumberFormatter::new_with_notation(
            context.intl_provider(),
            &locale,
            Some("latn"),
            style,
            GroupingStrategy::Auto,
            notation,
        )
        .unwrap()
    };

    for (style, notation) in [
        (UnitFormatOptions::Decimal, Notation::Standard),
        (UnitFormatOptions::Decimal, Notation::Scientific),
        (UnitFormatOptions::Percent, Notation::Standard),
    ] {
        assert!(!formatter(&style, notation).has_plural_rules());
    }
    for display in [
        CurrencyDisplay::Symbol,
        CurrencyDisplay::Code,
        CurrencyDisplay::NarrowSymbol,
    ] {
        let style = UnitFormatOptions::Currency {
            currency: "USD".parse().unwrap(),
            display,
            sign: CurrencySign::Standard,
        };
        assert!(!formatter(&style, Notation::Standard).has_plural_rules());
    }

    let unit = UnitFormatOptions::Unit {
        unit: "meter".parse().unwrap(),
        display: UnitDisplay::Long,
    };
    assert!(formatter(&unit, Notation::Standard).has_plural_rules());
    assert!(
        formatter(
            &UnitFormatOptions::Decimal,
            Notation::Compact {
                display: CompactDisplay::Short,
            },
        )
        .has_plural_rules()
    );
    let currency_name = UnitFormatOptions::Currency {
        currency: "USD".parse().unwrap(),
        display: CurrencyDisplay::Name,
        sign: CurrencySign::Standard,
    };
    assert!(
        formatter(&currency_name, Notation::Standard).has_plural_rules(),
        "currency names and their range collapse still require plural grammar"
    );
}

#[test]
fn range_template_rejects_missing_duplicate_and_invalid_endpoint_fields() {
    use super::range_pattern::RangePattern;
    for raw in [
        "{0}",
        "{0}..{0}",
        "{0}..{2}",
        "{0}..{1}}",
        "}{0}..{1}",
        "{0}}..{1}",
        "none",
        "{0}..{1}{{",
    ] {
        assert!(RangePattern::parse(raw, 2).is_err(), "{raw}");
    }
    for raw in ["{0}–{1}", "{0} - {1}", "\u{200f}{0}～{1}", "{1} to {0}"] {
        assert!(RangePattern::parse(raw, 2).is_ok(), "{raw}");
    }
    assert!(RangePattern::parse("{0}", 0).is_err());
    assert!(RangePattern::parse("{0}", 3).is_err());
}

fn assert_script(script: &str) {
    let mut context = Context::default();
    let result = context
        .eval(Source::from_bytes(script))
        .unwrap_or_else(|error| {
            let error = error.to_opaque(&mut context);
            let message = error
                .to_string(&mut context)
                .map(|message| message.to_std_string_escaped());
            panic!("NumberFormat script failed: {message:?}");
        });
    assert_eq!(result, JsValue::new(true));
}

#[test]
fn u16_to_rounding_increment_sunny_day() {
    #[rustfmt::skip]
    let valid_cases: [(u16, RoundingIncrement); 15] = [
        // Singles
        (1, RoundingIncrement::from_parts(MultiplesOf1, 0).unwrap()),
        (2, RoundingIncrement::from_parts(MultiplesOf2, 0).unwrap()),
        (5, RoundingIncrement::from_parts(MultiplesOf5, 0).unwrap()),
        // Tens
        (10, RoundingIncrement::from_parts(MultiplesOf1, 1).unwrap()),
        (20, RoundingIncrement::from_parts(MultiplesOf2, 1).unwrap()),
        (25, RoundingIncrement::from_parts(MultiplesOf25, 0).unwrap()),
        (50, RoundingIncrement::from_parts(MultiplesOf5, 1).unwrap()),
        // Hundreds
        (100, RoundingIncrement::from_parts(MultiplesOf1, 2).unwrap()),
        (200, RoundingIncrement::from_parts(MultiplesOf2, 2).unwrap()),
        (250, RoundingIncrement::from_parts(MultiplesOf25, 1).unwrap()),
        (500, RoundingIncrement::from_parts(MultiplesOf5, 2).unwrap()),
        // Thousands
        (1000, RoundingIncrement::from_parts(MultiplesOf1, 3).unwrap()),
        (2000, RoundingIncrement::from_parts(MultiplesOf2, 3).unwrap()),
        (2500, RoundingIncrement::from_parts(MultiplesOf25, 2).unwrap()),
        (5000, RoundingIncrement::from_parts(MultiplesOf5, 3).unwrap()),
    ];

    for (num, increment) in valid_cases {
        assert_eq!(RoundingIncrement::from_u16(num), Some(increment));
    }
}

#[test]
fn u16_to_rounding_increment_rainy_day() {
    const INVALID_CASES: [u16; 9] = [0, 4, 6, 24, 10000, 65535, 7373, 140, 1500];

    for num in INVALID_CASES {
        assert!(RoundingIncrement::from_u16(num).is_none());
    }
}

#[test]
fn decimal_parts_are_icu_fields_with_fresh_ordinary_objects() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('en', {minimumFractionDigits: 2});
        const actual = nf.formatToParts(-12345.6);
        const expected = [['minusSign','-'],['integer','12'],['group',','],
            ['integer','345'],['decimal','.'],['fraction','60']];
        if (JSON.stringify(actual.map(p => [p.type,p.value])) !== JSON.stringify(expected)) return false;
        if (actual.map(p => p.value).join('') !== nf.format(-12345.6)) return false;
        const other = nf.formatToParts(-12345.6);
        return actual !== other && actual.every((part,i) => part !== other[i] &&
            Object.getPrototypeOf(part) === Object.prototype &&
            Object.keys(part).join(',') === 'type,value' &&
            ['type','value'].every(k => {
                const d = Object.getOwnPropertyDescriptor(part,k);
                return d.writable && d.enumerable && d.configurable;
            }));
    })()"#,
    );
}

#[test]
fn exact_strings_and_bigints_do_not_round_through_binary_float() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('en', {useGrouping: false, maximumFractionDigits: 20});
        return nf.format('9007199254740993.123456789') === '9007199254740993.123456789' &&
            nf.format(9007199254740993n) === '9007199254740993' &&
            nf.format('0x20000000000001') === '9007199254740993' &&
            nf.format('1e309') === '∞' && nf.format('-1e-9999') === '-0' &&
            nf.format('not a number') === 'NaN' && nf.format(undefined) === 'NaN';
    })()"#,
    );
}

#[test]
fn all_nonfinite_sign_display_categories_and_negative_zero_are_preserved() {
    assert_script(
        r#"(() => {
        const inputs = [NaN, Infinity, -Infinity, 0, -0, 1, -1];
        const expected = {
            auto: ['NaN','∞','-∞','0','-0','1','-1'],
            always: ['+NaN','+∞','-∞','+0','-0','+1','-1'],
            never: ['NaN','∞','∞','0','0','1','1'],
            exceptZero: ['NaN','+∞','-∞','0','0','+1','-1'],
            negative: ['NaN','∞','-∞','0','0','1','-1']
        };
        for (const signDisplay of Object.keys(expected)) {
            const nf = new Intl.NumberFormat('en', {signDisplay});
            if (inputs.map(x => nf.format(x)).join('|') !== expected[signDisplay].join('|'))
                throw new Error(signDisplay + ': sign mismatch');
            if (nf.formatToParts(NaN).find(p => p.type === 'nan').value !== 'NaN') return false;
            if (nf.formatToParts(Infinity).find(p => p.type === 'infinity').value !== '∞') return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn percent_scales_before_rounding_and_keeps_locale_prefix() {
    assert_script(
        r#"(() => {
        const en = new Intl.NumberFormat('en', {style:'percent', maximumFractionDigits:1});
        const tr = new Intl.NumberFormat('tr', {style:'percent'});
        return en.format('0.12345') === '12.3%' && en.format(Infinity) === '∞%' &&
            tr.format(.12) === '%12' && tr.formatToParts(.12)[0].type === 'percentSign' &&
            en.formatToParts(.12).at(-1).type === 'percentSign';
    })()"#,
    );
}

#[test]
fn cldr_currency_digits_include_zero_three_and_unknown_default() {
    assert_script(
        r#"(() => {
        for (const [currency,digits,expected] of [['JPY',0,'1,235'],['KWD',3,'1,234.567'],['ZZZ',2,'1,234.57']]) {
            const nf = new Intl.NumberFormat('en', {style:'currency',currency,currencyDisplay:'code'});
            const r = nf.resolvedOptions();
            if (r.minimumFractionDigits !== digits || r.maximumFractionDigits !== digits) return false;
            const number = nf.formatToParts('1234.567').filter(p => p.type !== 'currency' && p.type !== 'literal').map(p => p.value).join('');
            if (number !== expected) throw new Error(currency + ': digits mismatch');
        }
        return true;
    })()"#,
    );
}

#[test]
fn currency_display_width_name_and_accounting_are_typed() {
    assert_script(
        r#"(() => {
        const common = {style:'currency',currency:'USD',useGrouping:false};
        if (new Intl.NumberFormat('en', common).format(12.5) !== '$12.50') return false;
        if (new Intl.NumberFormat('en', {...common,currencyDisplay:'code'}).format(12.5) !== 'USD\u00a012.50') return false;
        const accounting = new Intl.NumberFormat('en', {...common,currencySign:'accounting'});
        if (accounting.format(-12.5) !== '($12.50)' || accounting.formatToParts(-12.5).some(p => p.type === 'minusSign')) return false;
        if (new Intl.NumberFormat('en', {...common,currencySign:'accounting',signDisplay:'never'}).format(-12.5) !== '$12.50') return false;
        if (new Intl.NumberFormat('en', {...common,currencySign:'accounting',signDisplay:'always'}).format(12.5) !== '+$12.50') return false;
        const names = new Intl.NumberFormat('en', {...common,currencyDisplay:'name',minimumFractionDigits:0,maximumFractionDigits:0});
        if (names.format(1) !== '1 US dollar' || names.format(2) !== '2 US dollars') return false;
        const cad = {style:'currency',currency:'CAD'};
        if (new Intl.NumberFormat('en-US',cad).format(1) !== 'CA$1.00' ||
            new Intl.NumberFormat('en-US',{...cad,currencyDisplay:'narrowSymbol'}).format(1) !== '$1.00') return false;
        for (const [currency,currencyDisplay,wanted] of [
            ['AED','symbol','AED'], ['AED','narrowSymbol','AED'],
            ['ADP','symbol','ADP'], ['ADP','narrowSymbol','ADP']
        ]) {
            const parts = new Intl.NumberFormat('en-US',{style:'currency',currency,currencyDisplay}).formatToParts(1);
            if (parts.find(part => part.type === 'currency').value !== wanted) return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn currency_locale_separators_and_suffix_placement_are_preserved() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('fr', {style:'currency',currency:'EUR'});
        const parts = nf.formatToParts('1234.5');
        return nf.format('1234.5') === '1\u202f234,50\u00a0€' &&
            parts.find(p => p.type === 'decimal').value === ',' &&
            parts.find(p => p.type === 'group').value === '\u202f' && parts.at(-1).type === 'currency';
    })()"#,
    );
}

#[test]
fn unknown_well_formed_currency_uses_code_and_default_digits() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('en', {style:'currency',currency:'zzz'});
        return nf.format(1.25) === 'ZZZ\u00a01.25' &&
            nf.resolvedOptions().currency === 'ZZZ' && nf.resolvedOptions().minimumFractionDigits === 2;
    })()"#,
    );
}

#[test]
fn unit_width_plural_and_compound_labels_use_cldr() {
    assert_script(
        r#"(() => {
        const long = new Intl.NumberFormat('en', {style:'unit',unit:'meter',unitDisplay:'long'});
        const narrow = new Intl.NumberFormat('en', {style:'unit',unit:'meter',unitDisplay:'narrow'});
        const compound = new Intl.NumberFormat('en', {style:'unit',unit:'meter-per-second',unitDisplay:'long'});
        if (long.format(1) !== '1 meter' || long.format(2) !== '2 meters' || narrow.format(2) !== '2m') return false;
        if (compound.format(2) !== '2 meters per second') return false;
        const labels = compound.formatToParts(2).filter(p => p.type === 'unit');
        if (labels.length !== 1 || labels[0].value !== 'meters per second') return false;
        const ru = new Intl.NumberFormat('ru', {style:'unit',unit:'meter',unitDisplay:'long'});
        return ru.format(2) === '2 метра' && ru.format(5) === '5 метров';
    })()"#,
    );
}

#[test]
fn all_duration_units_have_native_typed_text_patterns() {
    assert_script(
        r#"(() => {
        for (const unit of ['year','month','week','day','hour','minute','second','millisecond','microsecond','nanosecond']) {
            const nf = new Intl.NumberFormat('en', {style:'unit',unit,unitDisplay:'long'});
            if (nf.format(2) !== '2 ' + unit + 's') throw new Error(unit + ': unit mismatch');
            if (nf.formatToParts(2).find(p => p.type === 'unit').value !== unit + 's') return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn unit_plural_selection_uses_rounded_visible_fraction_digits() {
    assert_script(
        r#"(() => {
        const a = new Intl.NumberFormat('en', {style:'unit',unit:'meter',unitDisplay:'long',maximumFractionDigits:0});
        const b = new Intl.NumberFormat('en', {style:'unit',unit:'meter',unitDisplay:'long',minimumFractionDigits:1});
        return a.format('1.2') === '1 meter' && b.format(1) === '1.0 meters';
    })()"#,
    );
}

#[test]
fn signed_numeric_unit_placeholder_keeps_prefix_and_number_omission() {
    assert_script(
        r#"(() => {
        for (const [locale,value,expected] of [['ar',-1,'ثانية'],['ar',-2,'ثانيتان'],
            ['he',-2,'שתי שניות'],['sw',-1,'sekunde -1'],['ee-TG',-1,'sekend -1 wo']]) {
            const nf = new Intl.NumberFormat(locale,{style:'unit',unit:'second',unitDisplay:'long'});
            const parts = nf.formatToParts(value);
            if (nf.format(value) !== expected || parts.map(p => p.value).join('') !== expected)
                throw new Error(locale + ': ' + nf.format(value));
            if ((locale === 'ar' || locale === 'he') && parts.some(p => p.type === 'integer' || p.type === 'minusSign')) return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn unicode_numbering_option_and_locale_extension_have_distinct_resolution() {
    assert_script(
        r#"(() => {
        const cases = [['en',undefined,'en','latn'],['en-u-nu-arab',undefined,'en-u-nu-arab','arab'],
            ['en-u-nu-latn','arab','en','arab'],['en-u-nu-arab','arab','en-u-nu-arab','arab'],
            ['en-u-nu-arab','invalid','en-u-nu-arab','arab'],['en-u-nu-invalid','invalid2','en','latn']];
        for (const [locale,numberingSystem,expectedLocale,expectedSystem] of cases) {
            const nf = new Intl.NumberFormat(locale,{numberingSystem,useGrouping:false});
            const r = nf.resolvedOptions();
            if (r.locale !== expectedLocale || r.numberingSystem !== expectedSystem) throw new Error(locale + ': resolution mismatch');
            if (expectedSystem === 'arab' && nf.format(123) !== '١٢٣') return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn bidi_controls_are_literal_parts_and_formatted_text_is_unchanged() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('ar', {numberingSystem:'arab',style:'currency',currency:'USD',signDisplay:'always'});
        const parts = nf.formatToParts(-123.5);
        if (parts.map(p => p.value).join('') !== nf.format(-123.5)) return false;
        return parts.some(p => p.type === 'literal' && /[\u061c\u200e\u200f]/.test(p.value)) &&
            parts.filter(p => p.type !== 'literal').every(p => !/[\u061c\u200e\u200f]/.test(p.value));
    })()"#,
    );
}

#[test]
fn requested_numbering_system_uses_typed_separators_with_icu_digits() {
    assert_script(
        r#"(() => {
        for (const [numberingSystem,expected,decimal,group] of [
            ['arab','١٬٢٣٤٫٥','٫','٬'],['arabext','۱٬۲۳۴٫۵','٫','٬'],['beng','১,২৩৪.৫','.',',']]) {
            const nf = new Intl.NumberFormat('en',{numberingSystem});
            const parts = nf.formatToParts('1234.5');
            if (nf.format('1234.5') !== expected) throw new Error(numberingSystem + ': ' + nf.format('1234.5'));
            if (parts.find(p => p.type === 'decimal').value !== decimal ||
                parts.find(p => p.type === 'group').value !== group) return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn rounding_increment_mode_and_trailing_zero_metadata_match_behavior() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('en', {minimumFractionDigits:2,maximumFractionDigits:2,roundingIncrement:5,roundingMode:'halfEven'});
        const r = nf.resolvedOptions();
        const stripped = new Intl.NumberFormat('en', {minimumFractionDigits:2,trailingZeroDisplay:'stripIfInteger'});
        return nf.format('1.025') === '1.00' && nf.format('1.075') === '1.10' &&
            r.roundingMode === 'halfEven' && r.roundingIncrement === 5 &&
            stripped.format(1) === '1' && stripped.format('1.5') === '1.50';
    })()"#,
    );
}

#[test]
fn brand_checks_precede_coercion_and_bound_format_has_stable_identity() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('en');
        const method = Intl.NumberFormat.prototype.formatToParts;
        if (method.name !== 'formatToParts' || method.length !== 1 || nf.format.length !== 1 || nf.format !== nf.format) return false;
        let converted = false;
        for (const receiver of [undefined,null,{},Intl.NumberFormat.prototype]) {
            try { method.call(receiver,{valueOf(){converted=true;return 1;}}); return false; }
            catch(error) { if (!(error instanceof TypeError)) throw error; }
        }
        try { nf.format(Symbol()); return false; } catch(error) { if (!(error instanceof TypeError)) throw error; }
        try { new method(); return false; } catch(error) { if (!(error instanceof TypeError)) throw error; }
        return !converted;
    })()"#,
    );
}

#[test]
fn optional_legacy_wrapper_is_not_accepted_by_format_to_parts() {
    assert_script(
        r#"(() => {
        const wrapper = Object.create(Intl.NumberFormat.prototype);
        Intl.NumberFormat.call(wrapper,'en');
        if (wrapper.format(1) !== '1') return false;
        try { wrapper.formatToParts(1); return false; } catch(error) { return error instanceof TypeError; }
    })()"#,
    );
}

#[test]
fn legacy_receiver_capture_survives_options_reentry_gc_and_abrupt_return() {
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
    let result = context.eval(Source::from_bytes(r#"(() => {
        const wrapper = Object.create(Intl.NumberFormat.prototype);
        const other = Object.create(Intl.NumberFormat.prototype);
        const actual = Intl.NumberFormat.call(wrapper,'en',{currency:'USD',get style() {
            Intl.NumberFormat.call(other,'en'); collect(); return 'currency';
        }});
        if (actual !== wrapper || wrapper.format(1) !== '$1.00' || other.format(2) !== '2') return false;
        const failed = Object.create(Intl.NumberFormat.prototype), marker = new Error('getter failure');
        try { Intl.NumberFormat.call(failed,'en',{get style() {collect();throw marker;}}); return false; }
        catch(error) {if (error !== marker) return false;}
        const next = Object.create(Intl.NumberFormat.prototype);
        if (Intl.NumberFormat.call(next,'en') !== next || next.format(3) !== '3') return false;
        collect();
        try { wrapper.formatToParts(1); return false; } catch(error) {return error instanceof TypeError;}
    })()"#)).unwrap();
    assert_eq!(result, JsValue::new(true));
    assert!(context.native_call_receiver().is_none());
}

#[test]
fn value_coercion_can_reenter_and_collect_without_native_borrows() {
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
    assert_eq!(context.eval(Source::from_bytes(r#"(() => {
        const nf = new Intl.NumberFormat('en',{style:'currency',currency:'USD'});
        const bound = nf.format;
        let calls = 0;
        const input = {valueOf() { ++calls; collect(); if (nf.format(2) !== '$2.00') throw new Error('reentry'); return 1; }};
        const a = bound(input), b = nf.formatToParts(input);
        collect();
        return calls === 2 && a === '$1.00' && b.map(p => p.value).join('') === '$1.00' && nf.format === bound;
    })()"#)).unwrap(), JsValue::new(true));
}

#[test]
fn constructor_reads_options_once_in_specification_order() {
    assert_script(
        r#"(() => {
        const order = [];
        const options = new Proxy({}, {get(object,key) {order.push(key);return object[key];}});
        new Intl.NumberFormat('en',options);
        return order.join(',') === ['localeMatcher','numberingSystem','style','currency','currencyDisplay','currencySign',
            'unit','unitDisplay','notation','minimumIntegerDigits','minimumFractionDigits','maximumFractionDigits',
            'minimumSignificantDigits','maximumSignificantDigits','roundingIncrement','roundingMode','roundingPriority',
            'trailingZeroDisplay','compactDisplay','useGrouping','signDisplay'].join(',');
    })()"#,
    );
}

#[test]
fn native_duration_entry_retains_exact_decimal_and_explicit_truncation() {
    use super::{
        MathematicalValue, NativeNumberFormatter, NumericFormatOptions, UnitDisplay,
        UnitFormatOptions,
    };
    use fixed_decimal::{Decimal, SignDisplay, SignedRoundingMode, UnsignedRoundingMode};
    use icu_decimal::options::GroupingStrategy;
    let context = Context::default();
    let style = UnitFormatOptions::Unit {
        unit: "second".parse().unwrap(),
        display: UnitDisplay::Long,
    };
    let formatter = NativeNumberFormatter::new(
        context.intl_provider(),
        &"en".parse().unwrap(),
        None,
        &style,
        GroupingStrategy::Never,
    )
    .unwrap();
    let options = NumericFormatOptions::fraction(
        1,
        0,
        9,
        SignedRoundingMode::Unsigned(UnsignedRoundingMode::Trunc),
        SignDisplay::Never,
    );
    let value =
        MathematicalValue::Finite(Decimal::try_from_str("-9007199254740993.1234567899").unwrap());
    let parts = formatter.format(value.clone(), &options);
    assert_eq!(
        super::parts::parts_to_string(&parts),
        "9007199254740993.123456789 seconds"
    );
    assert_eq!(formatter.format(value, &options), parts);
}

#[test]
fn scientific_and_engineering_use_exact_exponents_across_locales() {
    assert_script(
        r#"(() => {
        const values = ['0.000345','0.345','3.45','34.5','543','5430','543000','543211.1'];
        const expected = {
            scientific: ['3.45E-4','3.45E-1','3.45E0','3.45E1','5.43E2','5.43E3','5.43E5','5.432E5'],
            engineering: ['345E-6','345E-3','3.45E0','34.5E0','543E0','5.43E3','543E3','543.211E3']
        };
        for (const locale of ['en-US','de-DE','ja-JP','zh-TW','ko-KR']) {
            for (const notation of Object.keys(expected)) {
                const nf = new Intl.NumberFormat(locale,{notation});
                for (let i = 0; i < values.length; ++i) {
                    const wanted = locale === 'de-DE' ? expected[notation][i].replace('.',',') : expected[notation][i];
                    const parts = nf.formatToParts(values[i]);
                    if (nf.format(values[i]) !== wanted || parts.map(p => p.value).join('') !== wanted)
                        throw new Error(locale + '/' + notation + '/' + values[i] + ': ' + nf.format(values[i]));
                    if (parts.filter(p => p.type === 'exponentSeparator').length !== 1 ||
                        parts.filter(p => p.type === 'exponentInteger').length !== 1) return false;
                }
                if (nf.resolvedOptions().notation !== notation) return false;
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn scientific_parts_localize_both_exponent_digits_and_bidi_symbols() {
    assert_script(
        r#"(() => {
        const en = new Intl.NumberFormat('en',{notation:'scientific'});
        const expected = [['minusSign','-'],['integer','3'],['decimal','.'],['fraction','45'],
            ['exponentSeparator','E'],['exponentMinusSign','-'],['exponentInteger','4']];
        if (JSON.stringify(en.formatToParts('-0.000345').map(p => [p.type,p.value])) !== JSON.stringify(expected)) return false;
        const arabic = [['integer','٣'],['decimal','٫'],['fraction','٤٥'],['exponentSeparator','أس'],
            ['literal','\u061c'],['exponentMinusSign','-'],['exponentInteger','٤']];
        for (const [locale,options] of [['ar',{numberingSystem:'arab'}],['ar-EG',{}]]) {
            const ar = new Intl.NumberFormat(locale,{notation:'scientific',...options});
            if (JSON.stringify(ar.formatToParts('0.000345').map(p => [p.type,p.value])) !== JSON.stringify(arabic))
                throw new Error(locale + ': ' + JSON.stringify(ar.formatToParts('0.000345')));
            if (ar.resolvedOptions().numberingSystem !== 'arab') return false;
        }
        const latin = [['integer','3'],['decimal','.'],['fraction','45'],['exponentSeparator','E'],
            ['literal','\u200e'],['exponentMinusSign','-'],['exponentInteger','4']];
        for (const [locale,options] of [['ar',{}],['ar-EG',{numberingSystem:'latn'}]]) {
            const ar = new Intl.NumberFormat(locale,{notation:'scientific',...options});
            if (JSON.stringify(ar.formatToParts('0.000345').map(p => [p.type,p.value])) !== JSON.stringify(latin))
                throw new Error(locale + ': ' + JSON.stringify(ar.formatToParts('0.000345')));
            if (ar.resolvedOptions().numberingSystem !== 'latn') return false;
        }
        const dev = new Intl.NumberFormat('en',{notation:'scientific',numberingSystem:'deva'});
        return dev.format('0.000345') === '३.४५E-४' &&
            dev.formatToParts('0.000345').at(-1).value === '४';
    })()"#,
    );
}

#[test]
fn notation_keeps_negative_zero_and_omits_exponents_for_nonfinite_values() {
    assert_script(
        r#"(() => {
        for (const notation of ['scientific','engineering']) {
            const nf = new Intl.NumberFormat('en',{notation});
            const inputs = [0,-0,NaN,Infinity,-Infinity];
            const expected = ['0E0','-0E0','NaN','∞','-∞'];
            for (let i = 0; i < inputs.length; ++i) {
                if (nf.format(inputs[i]) !== expected[i]) return false;
                const exponents = nf.formatToParts(inputs[i]).filter(p => p.type.startsWith('exponent'));
                if (exponents.length !== (i < 2 ? 2 : 0)) return false;
            }
            const never = new Intl.NumberFormat('en',{notation,signDisplay:'never'});
            if (never.format(-0) !== '0E0' || never.format('-.000345').indexOf('E-') < 0) return false;
            const always = new Intl.NumberFormat('en',{notation,signDisplay:'always'});
            if (always.format(1) !== '+1E0') return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn exponent_carries_round_original_exact_values_once() {
    assert_script(
        r#"(() => {
        const sci = new Intl.NumberFormat('en',{notation:'scientific',maximumFractionDigits:15});
        if (sci.format(9007199254740993n) !== '9.007199254740993E15' ||
            sci.format('9007199254740993') !== '9.007199254740993E15') return false;
        for (const [notation,input,expected] of [
            ['scientific','9.995','1E1'],['scientific','9.9949','9.99E0'],
            ['engineering','999.995','1E3'],['engineering','99.995','100E0']]) {
            const nf = new Intl.NumberFormat('en',{notation,maximumFractionDigits:2});
            if (nf.format(input) !== expected) throw new Error(notation + '/' + input + ': ' + nf.format(input));
        }
        const even = new Intl.NumberFormat('en',{notation:'scientific',maximumFractionDigits:2,roundingMode:'halfEven'});
        return even.format('1.005') === '1E0' && even.format('1.015') === '1.02E0';
    })()"#,
    );
}

#[test]
fn directed_rounding_uses_input_sign_when_selecting_exponents() {
    assert_script(
        r#"(() => {
        for (const [notation,input,ceil,floor] of [
            ['scientific','-9.99','-9.9E0','-1E1'],
            ['engineering','-999.99','-999.9E0','-1E3']]) {
            for (const [roundingMode,wanted] of [['ceil',ceil],['floor',floor]]) {
                const nf = new Intl.NumberFormat('en',{notation,roundingMode,maximumFractionDigits:1});
                if (nf.format(input) !== wanted) throw new Error(notation + '/' + roundingMode + ': ' + nf.format(input));
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn precision_priority_selects_notation_carries_from_the_winning_result() {
    assert_script(
        r#"(() => {
        const cases = [
            {
                notation: 'scientific', input: '9.99', significantDigits: 3,
                more: ['9.99E0', '-9.99E0'],
                ceil: ['1E1', '-9.9E0'], floor: ['9.9E0', '-1E1']
            },
            {
                notation: 'engineering', input: '999.99', significantDigits: 5,
                more: ['999.99E0', '-999.99E0'],
                ceil: ['1E3', '-999.9E0'], floor: ['999.9E0', '-1E3']
            },
            {
                notation: 'compact', input: '999.99', significantDigits: 5,
                more: ['999.99', '-999.99'],
                ceil: ['1K', '-999.9'], floor: ['999.9', '-1K']
            }
        ];
        for (const row of cases) {
            const common = {
                notation: row.notation,
                minimumSignificantDigits: row.significantDigits,
                maximumSignificantDigits: row.significantDigits,
                minimumFractionDigits: 0,
                maximumFractionDigits: 1
            };
            const inputs = [row.input, '-' + row.input];
            const more = new Intl.NumberFormat('en', {...common, roundingPriority:'morePrecision'});
            for (let i = 0; i < inputs.length; ++i) {
                const actual = more.format(inputs[i]);
                if (actual !== row.more[i])
                    throw new Error(row.notation + '/morePrecision/' + inputs[i] + ': ' + actual);
            }
            for (const roundingMode of ['ceil', 'floor']) {
                const nf = new Intl.NumberFormat('en', {
                    ...common, roundingPriority:'lessPrecision', roundingMode
                });
                for (let i = 0; i < inputs.length; ++i) {
                    const actual = nf.format(inputs[i]);
                    if (actual !== row[roundingMode][i])
                        throw new Error(row.notation + '/' + roundingMode + '/' + inputs[i] + ': ' + actual);
                }
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn signed_half_ties_select_notation_carries_in_the_directed_half_mode() {
    assert_script(
        r#"(() => {
        const cases = [
            {
                notation: 'scientific', input: '9.95',
                halfCeil: ['1E1', '-9.9E0'], halfFloor: ['9.9E0', '-1E1']
            },
            {
                notation: 'engineering', input: '999.95',
                halfCeil: ['1E3', '-999.9E0'], halfFloor: ['999.9E0', '-1E3']
            },
            {
                notation: 'compact', input: '999.95',
                halfCeil: ['1K', '-999.9'], halfFloor: ['999.9', '-1K']
            }
        ];
        for (const row of cases) {
            const inputs = [row.input, '-' + row.input];
            for (const roundingMode of ['halfCeil', 'halfFloor']) {
                const nf = new Intl.NumberFormat('en', {
                    notation: row.notation, maximumFractionDigits: 1, roundingMode
                });
                for (let i = 0; i < inputs.length; ++i) {
                    const actual = nf.format(inputs[i]);
                    if (actual !== row[roundingMode][i])
                        throw new Error(row.notation + '/' + roundingMode + '/' + inputs[i] + ': ' + actual);
                }
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn scientific_precision_padding_and_currency_defaults_remain_distinct() {
    assert_script(
        r#"(() => {
        const padded = new Intl.NumberFormat('en',{notation:'scientific',minimumIntegerDigits:2,
            minimumFractionDigits:2,maximumFractionDigits:2});
        if (padded.format('3.4') !== '03.40E0') return false;
        const stripped = new Intl.NumberFormat('en',{notation:'scientific',minimumFractionDigits:2,
            trailingZeroDisplay:'stripIfInteger'});
        if (stripped.format(1000) !== '1E3') return false;
        for (const notation of ['scientific','engineering']) {
            for (const currency of ['JPY','KWD','USD']) {
                const nf = new Intl.NumberFormat('en',{notation,style:'currency',currency});
                const r = nf.resolvedOptions();
                if (r.minimumFractionDigits !== 0 || r.maximumFractionDigits !== 3) return false;
                const numeric = nf.formatToParts('1234.56').filter(p => p.type !== 'currency' && p.type !== 'literal').map(p => p.value).join('');
                if (numeric !== '1.235E3') throw new Error(currency + '/' + notation + ': ' + numeric);
            }
            const percent = new Intl.NumberFormat('en',{notation,style:'percent'});
            const expected = notation === 'scientific' ? '3E-2%' : '35E-3%';
            if (percent.format('0.000345') !== expected) throw new Error(notation + ': ' + percent.format('0.000345'));
        }
        return true;
    })()"#,
    );
}

#[test]
fn scientific_plural_selection_uses_rounded_expanded_value() {
    assert_script(
        r#"(() => {
        for (const notation of ['scientific','engineering']) {
            const meter = new Intl.NumberFormat('en',{notation,style:'unit',unit:'meter',unitDisplay:'long'});
            const dollar = new Intl.NumberFormat('en',{notation,style:'currency',currency:'USD',currencyDisplay:'name'});
            for (const [value,number,suffix] of [['1','1E0',''],['1000','1E3','s'],['999.9995','1E3','s']]) {
                if (meter.format(value) !== number + ' meter' + suffix ||
                    dollar.format(value) !== number + ' US dollar' + suffix)
                    throw new Error(notation + '/' + value + ': ' + meter.format(value) + '/' + dollar.format(value));
            }
        }
        const visible = new Intl.NumberFormat('en',{notation:'scientific',style:'unit',unit:'meter',
            unitDisplay:'long',minimumFractionDigits:1});
        return visible.format(1) === '1.0E0 meters';
    })()"#,
    );
}

#[test]
fn scientific_uses_intl_template_instead_of_legacy_ldml_affixes() {
    assert_script(
        r#"(() => {
        for (const [locale,expected] of [['hi','3.45E-4'],['mr','३.४५E-४'],
            ['hi-u-nu-deva','३.४५E-४'],['lo','3,45E-4'],['si','3.45E-4']]) {
            const nf = new Intl.NumberFormat(locale,{notation:'scientific'});
            if (nf.format('0.000345') !== expected) throw new Error(locale + ': ' + nf.format('0.000345'));
        }
        return true;
    })()"#,
    );
}

#[test]
fn exponent_selection_and_operands_handle_decimal_magnitude_boundaries() {
    use super::{NumericFormatOptions, notation::NativeNotation, options::Notation};
    use fixed_decimal::{Decimal, SignDisplay, SignedRoundingMode, UnsignedRoundingMode};
    let context = Context::default();
    let locale = "en".parse().unwrap();
    let options = NumericFormatOptions::fraction(
        1,
        0,
        3,
        SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand),
        SignDisplay::Auto,
    );
    let data = super::data::locale_or_global::<
        boa_intl_data::OmoikaneNumberSymbolsV1,
        boa_intl_data::OmoikaneGlobalNumberSymbolsV1,
    >(context.intl_provider(), &locale, "latn")
    .unwrap();
    let notation = NativeNotation::new(
        context.intl_provider(),
        &locale,
        "latn",
        Notation::Engineering,
        &super::UnitFormatOptions::Decimal,
        Some("#E0"),
        data.get(),
    )
    .unwrap();
    let mut value = Decimal::from(1);
    value.multiply_pow10(i16::MIN);
    let exponent = notation.prepare(&mut value, &options.digits).exponent;
    assert_eq!(exponent, -32769);
    assert_eq!(value.to_string(), "10");
    let operands: icu_plurals::RawPluralOperands = notation.operands(&value, exponent).into();
    assert_eq!(
        (operands.i, operands.v, operands.f, operands.c),
        (0, 18, 0, 0)
    );
    let parts = notation.render(
        vec![super::NumberPart {
            kind: "integer",
            value: "10".into(),
        }],
        exponent,
        data.get(),
    );
    assert_eq!(super::parts::parts_to_string(&parts), "10E-32769");
}

#[test]
fn compact_thresholds_precision_and_parts_match_five_locale_contracts() {
    assert_script(
        r#"(() => {
        const values = [987654321,98765432,98765,9876];
        const rows = [
            ['en-US','short',['988M','99M','99K','9.9K']],
            ['en-US','long',['988 million','99 million','99 thousand','9.9 thousand']],
            ['de-DE','short',['988\u00a0Mio.','99\u00a0Mio.','98.765','9876']],
            ['de-DE','long',['988 Millionen','99 Millionen','99 Tausend','9,9 Tausend']],
            ['ja-JP','short',['9.9億','9877万','9.9万','9876']],
            ['ko-KR','short',['9.9억','9877만','9.9만','9.9천']],
            ['zh-TW','short',['9.9億','9877萬','9.9萬','9876']]
        ];
        for (const [locale,compactDisplay,expected] of rows) {
            const nf = new Intl.NumberFormat(locale,{notation:'compact',compactDisplay});
            for (let i = 0; i < values.length; ++i) {
                const parts = nf.formatToParts(values[i]);
                if (nf.format(values[i]) !== expected[i] || parts.map(p => p.value).join('') !== expected[i])
                    throw new Error(locale + '/' + compactDisplay + '/' + values[i] + ': ' + nf.format(values[i]));
            }
            const r = nf.resolvedOptions();
            if (r.notation !== 'compact' || r.compactDisplay !== compactDisplay || r.useGrouping !== 'min2') return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn compact_small_values_and_large_clamped_thresholds_remain_exact() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('en',{notation:'compact'});
        const values = ['159','15.9','1.59','0.159','0.0159','0.00159'];
        const expected = ['159','16','1.6','0.16','0.016','0.0016'];
        if (values.map(x => nf.format(x)).join('|') !== expected.join('|')) return false;
        if (nf.format(100000000000000000000n) !== '100,000,000T') return false;
        const ja = new Intl.NumberFormat('ja',{notation:'compact',compactDisplay:'long'});
        if (ja.format(100000000000000000000n) !== '10,000京') return false;
        const exact = new Intl.NumberFormat('en',{notation:'compact',maximumFractionDigits:15,useGrouping:false});
        return exact.format(9007199254740993n) === '9007.199254740993T' &&
            exact.format('9007199254740993') === '9007.199254740993T';
    })()"#,
    );
}

#[test]
fn compact_carries_round_original_signed_values_at_suffix_boundaries() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('en',{notation:'compact'});
        for (const [input, expected] of [['999999.5','1M'],['-999999.5','-1M']]) {
            const actual = nf.format(input);
            if (actual !== expected) throw new Error('en/' + input + ': ' + JSON.stringify(actual) + ' !== ' + JSON.stringify(expected));
        }
        for (const [roundingMode,input,expected] of [
            ['ceil','-999.99','-999.9'],['floor','-999.99','-1K'],
            ['ceil','999.99','1K'],['floor','999.99','999.9']]) {
            const fmt = new Intl.NumberFormat('en',{notation:'compact',roundingMode,maximumFractionDigits:1});
            if (fmt.format(input) !== expected) throw new Error(roundingMode + '/' + input + ': ' + fmt.format(input));
        }
        const de = new Intl.NumberFormat('de',{notation:'compact',maximumFractionDigits:0});
        const actual = de.format('999999.5');
        const expected = '1\u00a0Mio.';
        if (actual !== expected) throw new Error('de/999999.5: ' + JSON.stringify(actual) + ' !== ' + JSON.stringify(expected));
        return true;
    })()"#,
    );
}

#[test]
fn compact_quoted_affixes_and_literal_spacing_keep_typed_parts() {
    assert_script(
        r#"(() => {
        const de = new Intl.NumberFormat('de',{notation:'compact'});
        const expected = [['integer','99'],['literal','\u00a0'],['compact','Mio.']];
        const actual = de.formatToParts(98765432).map(p => [p.type,p.value]);
        if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error(JSON.stringify(actual));
        const long = new Intl.NumberFormat('en',{notation:'compact',compactDisplay:'long'});
        const parts = long.formatToParts(1200);
        return JSON.stringify(parts.map(p => [p.type,p.value])) === JSON.stringify([
            ['integer','1'],['decimal','.'],['fraction','2'],['literal',' '],['compact','thousand']]);
    })()"#,
    );
}

#[test]
fn compact_explicit_one_and_grammatical_number_omission_preserve_sign_and_precision() {
    assert_script(
        r#"(() => {
        for (const [locale,minimumFractionDigits,value,signDisplay,expected] of [
            ['fr',0,1000,'auto','mille'],['fr',2,1000,'auto','mille'],
            ['fr',2,-1000,'auto','-1,00 millier'],['fr',0,-1000,'never','1 millier'],
            ['fr',0,1000,'always','+mille'],['fr',0,1200,'auto','1,2 millier'],
            ['it',0,-1000,'auto','-mille'],['it',1,1000,'auto','1,0 mila'],
            ['ru',0,1000,'auto','1 тысяча'],['ru',1,1000,'auto','1,0 тысячи']]) {
            const nf = new Intl.NumberFormat(locale,{notation:'compact',compactDisplay:'long',minimumFractionDigits,signDisplay});
            if (nf.format(value) !== expected) throw new Error(locale + '/' + value + ': ' + nf.format(value));
        }
        const parts = new Intl.NumberFormat('fr',{notation:'compact',compactDisplay:'long'}).formatToParts(1000);
        return parts.length === 1 && parts[0].type === 'compact' && parts[0].value === 'mille';
    })()"#,
    );
}

#[test]
fn compact_prefix_signs_survive_unit_and_percent_outer_patterns() {
    assert_script(
        r#"(() => {
        const plain = new Intl.NumberFormat('sw',{notation:'compact',compactDisplay:'long'});
        if (plain.format(-1200) !== 'elfu -1.2') return false;
        const parts = plain.formatToParts(-1200);
        if (JSON.stringify(parts.map(p => [p.type,p.value])) !== JSON.stringify([
            ['compact','elfu'],['literal',' '],['minusSign','-'],['integer','1'],['decimal','.'],['fraction','2']])) return false;
        const meter = new Intl.NumberFormat('sw',{notation:'compact',compactDisplay:'long',style:'unit',unit:'meter',unitDisplay:'long'});
        if (meter.format(-1200) !== 'mita elfu -1.2') return false;
        const percent = new Intl.NumberFormat('sw',{notation:'compact',compactDisplay:'long',style:'percent'});
        if (percent.format(-12) !== 'asilimia elfu -1.2') return false;
        const pp = percent.formatToParts(-12);
        return pp[0].type === 'percentSign' && pp[0].value === 'asilimia' && !pp.some(p => p.type === 'unit');
    })()"#,
    );
}

#[test]
fn compact_long_percent_uses_rounded_russian_plural_grammar() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('ru', {
            notation:'compact', compactDisplay:'long', style:'percent'
        });
        for (const [input, expected] of [
            ['0.01', '1 процент'],
            ['0.02', '2 процента'],
            ['0.05', '5 процентов'],
            ['0.21', '21 процент']
        ]) {
            const actual = nf.format(input);
            if (actual !== expected) throw new Error(input + ': ' + actual);
        }
        return true;
    })()"#,
    );
}

#[test]
fn compact_currency_alpha_spacing_and_accounting_depend_on_actual_pattern() {
    assert_script(
        r#"(() => {
        for (const compactDisplay of ['short','long']) {
            for (const [currencyDisplay,wanted] of [['symbol','-$12K'],['code','-USD\u00a012K'],['narrowSymbol','-$12K']]) {
                const nf = new Intl.NumberFormat('en',{notation:'compact',compactDisplay,style:'currency',currency:'USD',currencyDisplay,currencySign:'accounting'});
                if (nf.format(-12345) !== wanted) throw new Error(currencyDisplay + ': ' + nf.format(-12345));
                if (nf.format(-12) !== (currencyDisplay === 'code' ? '(USD\u00a012)' : '($12)')) return false;
                if (nf.format(-Infinity) !== (currencyDisplay === 'code' ? '(USD\u00a0∞)' : '($∞)')) return false;
            }
        }
        const unknown = new Intl.NumberFormat('en',{notation:'compact',style:'currency',currency:'ZZZ'});
        return unknown.format(1200) === 'ZZZ\u00a01.2K' && unknown.formatToParts(1200).find(p => p.type === 'currency').value === 'ZZZ';
    })()"#,
    );
}

#[test]
fn compact_suffix_plural_and_expanded_currency_unit_plural_are_separate() {
    assert_script(
        r#"(() => {
        const unit = new Intl.NumberFormat('en',{notation:'compact',style:'unit',unit:'meter',unitDisplay:'long'});
        const short = new Intl.NumberFormat('en',{notation:'compact',style:'currency',currency:'USD',currencyDisplay:'name'});
        const long = new Intl.NumberFormat('en',{notation:'compact',compactDisplay:'long',style:'currency',currency:'USD',currencyDisplay:'name'});
        return unit.format(1000) === '1K meters' && short.format(1000) === '1K US dollars' &&
            long.format(1000) === '1 thousand US dollars' && long.format(-12345) === '-12 thousand US dollars';
    })()"#,
    );
}

#[test]
fn compact_missing_numbering_family_uses_same_locale_latin_patterns_with_native_digits() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('en',{notation:'compact',numberingSystem:'arabext'});
        if (nf.format(1200) !== '۱٫۲K' || nf.resolvedOptions().numberingSystem !== 'arabext') return false;
        const ar = new Intl.NumberFormat('ar-EG',{notation:'compact',numberingSystem:'latn',maximumFractionDigits:1});
        if (!ar.formatToParts(1200).some(p => p.type === 'compact' && p.value.includes('ألف'))) return false;
        for (const locale of ['bn','hi','es','pt','pl','he','sw','fa','ak']) {
            const fmt = new Intl.NumberFormat(locale,{notation:'compact',compactDisplay:'long'});
            const parts = fmt.formatToParts(1200000);
            if (!parts.some(p => p.type === 'compact') || parts.map(p => p.value).join('') !== fmt.format(1200000))
                throw new Error(locale + ': ' + fmt.format(1200000));
        }
        return true;
    })()"#,
    );
}

#[test]
fn compact_nonfinite_and_negative_zero_never_gain_compact_affixes() {
    assert_script(
        r#"(() => {
        for (const compactDisplay of ['short','long']) {
            const nf = new Intl.NumberFormat('en',{notation:'compact',compactDisplay});
            const values = [0,-0,NaN,Infinity,-Infinity],expected = ['0','-0','NaN','∞','-∞'];
            for (let i = 0; i < values.length; ++i) {
                if (nf.format(values[i]) !== expected[i] || nf.formatToParts(values[i]).some(p => p.type === 'compact')) return false;
            }
            const code = new Intl.NumberFormat('en',{notation:'compact',style:'currency',currency:'USD',currencySign:'accounting'});
            if (code.format(-0) !== '($0)') return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn compact_native_patterns_reject_invalid_thresholds_and_preserve_quotes() {
    use super::{compact::CompactFormatter, compact_pattern::CompactPattern};
    use boa_intl_data::{CompactPatterns, PluralCategory};
    let mut data = CompactPatterns {
        normal: Default::default(),
        alpha_next_to_number: Default::default(),
    };
    assert!(CompactFormatter::from_data(&data).is_err());
    drop(
        data.normal
            .insert(&1200, &(PluralCategory::Other as u8), "0K"),
    );
    assert!(CompactFormatter::from_data(&data).is_err());
    data.normal.clear();
    drop(
        data.normal
            .insert(&1000, &(PluralCategory::One as u8), "mille"),
    );
    assert!(CompactFormatter::from_data(&data).is_err());
    drop(
        data.normal
            .insert(&1000, &(PluralCategory::Other as u8), "0 thousand"),
    );
    assert!(CompactFormatter::from_data(&data).is_ok());
    drop(
        data.normal
            .insert(&1000, &(PluralCategory::Two as u8), "00 thousand"),
    );
    assert!(CompactFormatter::from_data(&data).is_err());
    for raw in ["0'broken", "0K0", "0K;-00K"] {
        assert!(CompactPattern::parse(raw).is_err());
    }
    assert_eq!(CompactPattern::parse("0 'zero0'").unwrap().zeros, Some(1));
    assert_eq!(CompactPattern::parse("mille").unwrap().zeros, None);
}

#[test]
fn compact_value_coercion_reenters_and_collects_without_losing_bound_state() {
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
        const nf = new Intl.NumberFormat('fr',{notation:'compact',compactDisplay:'long'});
        const bound = nf.format;
        let calls = 0;
        const input = {valueOf() { ++calls; collect(); if (bound(1200) !== '1,2 millier') throw new Error('reentry'); return 1000; }};
        if (bound(input) !== 'mille' || nf.formatToParts(input).map(p => p.value).join('') !== 'mille') return false;
        collect();
        return calls === 2 && nf.format === bound && bound(-1000) === '-1 millier';
    })()"#)).unwrap();
    assert_eq!(result, JsValue::new(true));
}

#[test]
fn significant_precision_carries_keep_minimum_digits_at_the_rounded_magnitude() {
    assert_script(
        r#"(() => {
        const nf = new Intl.NumberFormat('en', {
            minimumSignificantDigits:2, maximumSignificantDigits:2, useGrouping:false
        });
        for (const [input,expected] of [
            ['9.99','10'],['-9.99','-10'],['0.999','1.0'],['-0.999','-1.0'],
            ['1','1.0'],['-1','-1.0'],['0','0.0'],['-0','-0.0']]) {
            const actual = nf.format(input);
            if (actual !== expected) throw new Error(input + ': ' + JSON.stringify(actual) + ' !== ' + JSON.stringify(expected));
        }
        return JSON.stringify(nf.formatToParts('0.999').map(p => [p.type,p.value])) ===
            JSON.stringify([['integer','1'],['decimal','.'],['fraction','0']]);
    })()"#,
    );
}

#[test]
fn precision_priority_carry_ties_compare_the_rounded_significant_magnitude() {
    assert_script(
        r#"(() => {
        for (const [roundingPriority,positive,negative] of [
            ['morePrecision','1.0','-1.0'],['lessPrecision','1','-1']]) {
            const nf = new Intl.NumberFormat('en', {
                minimumSignificantDigits:2, maximumSignificantDigits:2,
                minimumFractionDigits:0, maximumFractionDigits:1, roundingPriority,
                useGrouping:false
            });
            for (const [input,expected] of [['0.999',positive],['-0.999',negative]]) {
                const actual = nf.format(input);
                if (actual !== expected) throw new Error(roundingPriority + '/' + input + ': ' + JSON.stringify(actual) + ' !== ' + JSON.stringify(expected));
            }
        }
        // A finer fixed result must win morePrecision after the carry;
        // returning the original significant magnitude would create a false tie.
        for (const [roundingPriority,positive,negative] of [
            ['morePrecision','1','-1'],['lessPrecision','1.0','-1.0']]) {
            const nf = new Intl.NumberFormat('en', {
                minimumSignificantDigits:2, maximumSignificantDigits:2,
                minimumFractionDigits:0, maximumFractionDigits:2, roundingPriority,
                useGrouping:false
            });
            for (const [input,expected] of [['0.999',positive],['-0.999',negative]]) {
                const actual = nf.format(input);
                if (actual !== expected) throw new Error(roundingPriority + '/maximumFractionDigits=2/' + input + ': ' + JSON.stringify(actual) + ' !== ' + JSON.stringify(expected));
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn rounding_increment_decimal_powers_preserve_options_and_exact_rounding() {
    assert_script(
        r#"(() => {
        for (const increment of [1,2,5,10,20,25,50,100,200,250,500,1000,2000,2500,5000]) {
            let reads = 0, conversions = 0;
            const nf = new Intl.NumberFormat('en', {
                get roundingIncrement() {
                    ++reads;
                    return {valueOf() { ++conversions; return increment; }};
                }, minimumFractionDigits:3, useGrouping:false
            });
            const r = nf.resolvedOptions();
            if (r.roundingIncrement !== increment || r.minimumFractionDigits !== 3 ||
                r.maximumFractionDigits !== 3 || reads !== 1 || conversions !== 1)
                throw new Error(JSON.stringify({increment,r,reads,conversions}));
        }
        for (const [increment,input,expected] of [
            [10,'1.245','1.250'],[100,'1.245','1.200'],[1000,'1.245','1.000'],
            [2000,'1.245','2.000'],[2500,'1.250','2.500'],[5000,'2.500','5.000']]) {
            const nf = new Intl.NumberFormat('en', {roundingIncrement:increment,
                minimumFractionDigits:3, maximumFractionDigits:3, useGrouping:false});
            for (const sign of ['','-']) {
                const actual = nf.format(sign + input), wanted = sign + expected;
                if (actual !== wanted || nf.formatToParts(sign + input).map(p => p.value).join('') !== wanted)
                    throw new Error(JSON.stringify({increment,input:sign + input,actual,wanted}));
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn rounding_increment_large_steps_keep_tie_modes_and_reject_unlisted_steps() {
    assert_script(
        r#"(() => {
        for (const [roundingMode,input,expected] of [
            ['halfExpand','1250','2500'],['halfExpand','-1250','-2500'],
            ['halfEven','1250','0'],['halfEven','3750','5000'],['trunc','3750','2500']]) {
            const nf = new Intl.NumberFormat('en', {roundingIncrement:2500, roundingMode,
                minimumFractionDigits:0, maximumFractionDigits:0, useGrouping:false});
            const actual = nf.format(input), r = nf.resolvedOptions();
            if (r.roundingIncrement !== 2500 || r.roundingMode !== roundingMode || actual !== expected)
                throw new Error(JSON.stringify({roundingMode,input,actual,expected,r}));
        }
        let rejected = false;
        try {
            new Intl.NumberFormat('en', {roundingIncrement:1250,
                minimumFractionDigits:0, maximumFractionDigits:0});
        } catch (error) {
            if (!(error instanceof RangeError)) throw error;
            rejected = true;
        }
        if (!rejected) throw new Error('1250 is not a permitted roundingIncrement');
        return true;
    })()"#,
    );
}

#[test]
fn canonical_kilometer_per_hour_patterns_preserve_all_widths_and_signed_values() {
    assert_script(
        r#"(() => {
        const cases = [
            ['ja-JP', {long:'時速 {0} キロメートル', short:'{0} km/h', narrow:'{0}km/h'}],
            ['ko-KR', {long:'시속 {0}킬로미터', short:'{0}km/h', narrow:'{0}km/h'}],
        ];
        const values = [[-987,'-987'], [-0.001,'-0.001'], [-0,'-0'],
            [0,'0'], [0.001,'0.001'], [987,'987']];
        for (const [locale, patterns] of cases) {
            for (const [unitDisplay, pattern] of Object.entries(patterns)) {
                const nf = new Intl.NumberFormat(locale, {
                    style:'unit', unit:'kilometer-per-hour', unitDisplay,
                });
                if (nf.resolvedOptions().unit !== 'kilometer-per-hour') return false;
                for (const [value, number] of values) {
                    const expected = pattern.replace('{0}', number);
                    if (nf.format(value) !== expected)
                        throw new Error(locale + '/' + unitDisplay + '/' + number + ': '
                            + nf.format(value) + ' !== ' + expected);
                }
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn canonical_kmh_parts_keep_prefixes_whitespace_and_primary_locale_grammar() {
    assert_script(
        r#"(() => {
        const number = [['minusSign','-'], ['integer','0'],
            ['decimal','.'], ['fraction','001']];
        const cases = [
            ['ja-JP','long',[['unit','時速'],['literal',' ']],
                [['literal',' '],['unit','キロメートル']]],
            ['ja-JP','short',[],[['literal',' '],['unit','km/h']]],
            ['ja-JP','narrow',[],[['unit','km/h']]],
            ['ko-KR','long',[['unit','시속'],['literal',' ']],[['unit','킬로미터']]],
            ['ko-KR','short',[],[['unit','km/h']]],
            ['ko-KR','narrow',[],[['unit','km/h']]],
        ];
        for (const [locale, unitDisplay, prefix, suffix] of cases) {
            const nf = new Intl.NumberFormat(locale, {
                style:'unit', unit:'kilometer-per-hour', unitDisplay,
            });
            const expected = prefix.concat(number, suffix);
            const parts = nf.formatToParts(-0.001);
            const actual = parts.map(p => [p.type,p.value]);
            if (JSON.stringify(actual) !== JSON.stringify(expected))
                throw new Error(locale + '/' + unitDisplay + ': ' + JSON.stringify(actual));
            if (parts.map(p => p.value).join('') !== nf.format(-0.001)) return false;
        }
        for (const [locale, unitDisplay, value, expected] of [
            ['en-US','long',1,'1 kilometer per hour'],
            ['en-US','long',2,'2 kilometers per hour'],
            ['en-US','short',2,'2 km/h'], ['en-US','narrow',2,'2km/h'],
            ['de-DE','long',1,'1 Kilometer pro Stunde'],
            ['de-DE','long',2,'2 Kilometer pro Stunde'],
            ['de-DE','short',2,'2 km/h'], ['de-DE','narrow',2,'2 km/h'],
        ]) {
            const nf = new Intl.NumberFormat(locale, {
                style:'unit', unit:'kilometer-per-hour', unitDisplay,
            });
            if (nf.format(value) !== expected)
                throw new Error(locale + '/' + unitDisplay + ': ' + nf.format(value));
        }
        // A different compound keeps the existing denominator-specific path.
        const general = new Intl.NumberFormat('ja-JP', {
            style:'unit', unit:'kilometer-per-minute', unitDisplay:'short',
        });
        return general.format(-1) === '-1 km/分' &&
            general.formatToParts(-1).find(p => p.type === 'unit').value === 'km/分';
    })()"#,
    );
}

#[test]
#[cfg(feature = "intl_bundled")]
fn pinned_numeric_digits_resolve_and_format_through_public_number_format() {
    let mut context = Context::default();
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../icu_provider/data/icu_decimal_digits.number-contract.js"
    ));
    let actual = context.eval(Source::from_bytes(source)).unwrap();
    assert_eq!(actual, JsValue::from(true));
}

#[test]
fn scalar_text_matches_typed_parts_across_styles_notations_and_semantic_edges() {
    assert_script(
        r#"(() => {
        const locales = ['en', 'ar-EG', 'fa', 'he', 'fr', 'de', 'bn', 'en-u-nu-mathbold'];
        const styles = [
            {style:'decimal'},
            {style:'percent'},
            {style:'currency',currency:'USD',currencyDisplay:'code',currencySign:'accounting'},
            {style:'currency',currency:'CHF',currencyDisplay:'symbol'},
            {style:'currency',currency:'USD',currencyDisplay:'name'},
            {style:'currency',currency:'ZZZ',currencyDisplay:'narrowSymbol'},
            {style:'unit',unit:'kilometer-per-hour',unitDisplay:'long'},
            {style:'unit',unit:'meter-per-second',unitDisplay:'narrow'},
        ];
        const values = ['-12345.625', '0.000000125', '999950', '1', '2', -0, NaN, Infinity, -Infinity];
        for (const locale of locales) {
            for (const style of styles) {
                for (const notation of ['standard','compact','scientific','engineering']) {
                    for (const signDisplay of ['auto','always','exceptZero','never','negative']) {
                        const nf = new Intl.NumberFormat(locale, {
                            ...style, notation, signDisplay, useGrouping:'always',
                            compactDisplay:'long', minimumFractionDigits:2, maximumFractionDigits:3,
                            roundingMode:'halfEven', trailingZeroDisplay:'stripIfInteger',
                        });
                        for (const value of values) {
                            const text = nf.format(value);
                            const joined = nf.formatToParts(value).map(part => part.value).join('');
                            if (text !== joined) throw new Error(
                                locale + '/' + JSON.stringify(style) + '/' + notation + '/' +
                                signDisplay + '/' + value + ': ' + text + ' !== ' + joined);
                        }
                    }
                }
            }
        }
        for (const locale of locales) {
            const nf = new Intl.NumberFormat(locale, {
                style:'currency', currency:'CHF', currencyDisplay:'code',
                minimumFractionDigits:2, maximumFractionDigits:2, roundingIncrement:5,
                roundingMode:'halfExpand', signDisplay:'always',
            });
            for (const value of ['1.025','-1.025','-0.001','1234567.875']) {
                if (nf.format(value) !== nf.formatToParts(value).map(p => p.value).join(''))
                    throw new Error('currency rounding increment ' + locale);
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn scalar_text_matches_grouped_sixteen_thousand_digit_bigints() {
    assert_script(
        r#"(() => {
        const large = BigInt('1234567890'.repeat(1638) + '1234');
        for (const locale of ['en', 'ar-EG', 'bn']) {
            const nf = new Intl.NumberFormat(locale, {useGrouping:'always',signDisplay:'always'});
            for (const value of [large, -large]) {
                const parts = nf.formatToParts(value);
                if (parts.filter(p => p.type === 'group').length < 5000) return false;
                if (nf.format(value) !== parts.map(p => p.value).join('')) return false;
            }
        }
        return true;
    })()"#,
    );
}
