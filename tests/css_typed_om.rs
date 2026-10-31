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
