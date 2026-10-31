use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime() -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse("<div id='target'></div>").document())
        .expect("runtime")
}

fn eval_json(runtime: &mut JsRuntime, source: &str) -> String {
    runtime
        .eval(source)
        .expect("evaluate CSS Typed OM probe")
        .as_string()
        .expect("JSON string")
        .to_std_string_escaped()
}

#[test]
fn numeric_values_parse_convert_and_preserve_the_type_hierarchy() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            r#"(() => {
              const parsed = CSSNumericValue.parse('2.54cm');
              const converted = parsed.to('px');
              return JSON.stringify([
                parsed instanceof CSSUnitValue,
                parsed instanceof CSSNumericValue,
                parsed instanceof CSSStyleValue,
                parsed.value, parsed.unit, parsed.toString(),
                converted.value, converted.unit, converted.toString(),
                CSSStyleValue.parse('width', '12px') instanceof CSSUnitValue
              ]);
            })()"#,
        ),
        "[true,true,true,2.54,\"cm\",\"2.54cm\",96,\"px\",\"96px\",true]"
    );
}

#[test]
fn numeric_arithmetic_returns_units_or_math_values_without_mutating_inputs() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            r#"(() => {
              const base = CSS.px(10);
              const sum = base.add(CSS.cm(2.54));
              const scaled = base.mul(3);
              const mixed = base.add(CSS.percent(5));
              return JSON.stringify([
                sum instanceof CSSUnitValue, sum.value, sum.unit,
                scaled instanceof CSSUnitValue, scaled.value, scaled.unit,
                mixed instanceof CSSMathSum, mixed.operator, mixed.values.length,
                mixed.toString(), base.value, base.unit,
                CSS.number(4).div(2).toString()
              ]);
            })()"#,
        ),
        "[true,106,\"px\",true,30,\"px\",true,\"sum\",2,\"calc(10px + 5%)\",10,\"px\",\"2\"]"
    );
}

#[test]
fn numeric_factories_cover_typed_om_units_and_reject_unknown_units() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            r#"(() => {
              let rejected = false;
              try { new CSSUnitValue(1, 'unknown'); } catch (error) {
                rejected = error instanceof TypeError;
              }
              return JSON.stringify([
                CSS.percent(25).toString(), CSS.deg(90).toString(),
                CSS.ms(16).toString(), CSS.Hz(60).toString(),
                CSS.dppx(2).toString(), CSS.fr(1).toString(), rejected
              ]);
            })()"#,
        ),
        "[\"25%\",\"90deg\",\"16ms\",\"60Hz\",\"2dppx\",\"1fr\",true]"
    );
}

#[test]
fn attribute_style_map_stays_live_with_css_style_declaration() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            r#"(() => {
              const target = document.getElementById('target');
              const map = target.attributeStyleMap;
              map.set('width', CSS.px(12));
              target.style.height = '7px';
              map.set('display', new CSSKeywordValue('block'));
              const width = map.get('width');
              const height = map.get('height');
              map.delete('display');
              return JSON.stringify([
                map === target.attributeStyleMap,
                width instanceof CSSUnitValue, width.value, width.unit,
                height.toString(), target.style.width,
                map.has('display'), map.size, [...map.keys()].sort()
              ]);
            })()"#,
        ),
        "[true,true,12,\"px\",\"7px\",\"12px\",false,2,[\"height\",\"width\"]]"
    );
}

#[test]
fn computed_and_rule_style_maps_reify_their_authoritative_declarations() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse(
            "<style>#target { width: 20px; display: block; }</style><div id='target'></div>",
        )
        .document(),
    )
    .expect("runtime");
    assert_eq!(
        eval_json(
            &mut runtime,
            r#"(() => {
              const target = document.getElementById('target');
              const computed = target.computedStyleMap();
              const rule = document.styleSheets[0].cssRules[0];
              rule.styleMap.set('height', CSS.px(9));
              const width = computed.get('width');
              return JSON.stringify([
                computed instanceof StylePropertyMapReadOnly,
                !(computed instanceof StylePropertyMap),
                width instanceof CSSUnitValue, width.toString(),
                computed.get('display') instanceof CSSKeywordValue,
                rule.style.height, rule.styleMap.get('height').toString(),
                getComputedStyle(target).height
              ]);
            })()"#,
        ),
        "[true,true,true,\"20px\",true,\"9px\",\"9px\",\"9px\"]"
    );
}

#[test]
fn registered_custom_properties_reify_through_computed_style_map() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            r#"(() => {
              CSS.registerProperty({
                name: '--gap', syntax: '<length>', inherits: false, initialValue: '3px'
              });
              const target = document.getElementById('target');
              target.attributeStyleMap.set('--gap', CSS.px(11));
              const specified = target.attributeStyleMap.get('--gap');
              const computed = target.computedStyleMap().get('--gap');
              target.attributeStyleMap.set('--raw', new CSSUnparsedValue(['var(--gap)']));
              return JSON.stringify([
                specified instanceof CSSUnitValue, specified.toString(),
                computed instanceof CSSUnitValue, computed.toString(),
                target.style.getPropertyValue('--gap'),
                target.attributeStyleMap.get('--raw') instanceof CSSUnparsedValue,
                target.attributeStyleMap.get('--raw').toString()
              ]);
            })()"#,
        ),
        "[true,\"11px\",true,\"11px\",\"11px\",true,\"var(--gap)\"]"
    );
}
