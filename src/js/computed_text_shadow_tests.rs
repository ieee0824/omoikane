use super::*;

#[test]
fn text_shadow_specified_cssom_preserves_units_and_rejects_invalid_updates() {
    let document = crate::html::TreeBuilder::parse("<div id='target'>text</div>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime.eval(r#"(() => {
        const element = document.getElementById('target');
        element.style.textShadow = '1em -2px red, blue 0 0';
        const values = [element.style.textShadow];
        element.style.setProperty('text-shadow', '1px 2px -3px');
        values.push(element.style.textShadow);
        element.style.cssText = 'font-size:20px;color:red;--shadow:1em 2px;text-shadow:var(--shadow)';
        values.push(getComputedStyle(element).textShadow);
        element.style.setProperty('--shadow', '0 0 blue');
        values.push(getComputedStyle(element).textShadow);
        return values.join('|');
    })()"#).unwrap().to_string(&mut runtime.context).unwrap().to_std_string_escaped();
    assert_eq!(
        result,
        "red 1em -2px, blue 0px 0px|red 1em -2px, blue 0px 0px|rgb(255, 0, 0) 20px 2px 0px|rgb(0, 0, 255) 0px 0px 0px"
    );
}

#[test]
fn text_shadow_cssom_inherits_absolute_lengths_and_live_currentcolor() {
    let document = crate::html::TreeBuilder::parse(
        "<div id='parent' style='color:red;font-size:20px;text-shadow:1em 2px'>\
         <span id='child' style='color:blue;font-size:5px'>text</span></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(320.0, 200.0);
    let result = runtime
        .eval(
            r#"(() => {
        const parent = document.getElementById('parent');
        const child = document.getElementById('child');
        const live = getComputedStyle(child);
        const values = [getComputedStyle(parent).textShadow, live.textShadow];
        child.style.color = 'lime';
        values.push(live.getPropertyValue('text-shadow'));
        child.style.textShadow = 'red 1px 2px 3px, 0 0 blue';
        values.push(live.textShadow);
        child.style.textShadow = '1px red 2px';
        values.push(live.textShadow);
        child.style.textShadow = 'calc(-2px) 1px calc(-3px)';
        values.push(live.textShadow);
        return values.join('|');
    })()"#,
        )
        .unwrap()
        .to_string(&mut runtime.context)
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        result,
        "rgb(255, 0, 0) 20px 2px 0px|rgb(0, 0, 255) 20px 2px 0px|rgb(0, 255, 0) 20px 2px 0px|rgb(255, 0, 0) 1px 2px 3px, rgb(0, 0, 255) 0px 0px 0px|rgb(255, 0, 0) 1px 2px 3px, rgb(0, 0, 255) 0px 0px 0px|rgb(0, 255, 0) -2px 1px 0px"
    );
}

#[test]
fn text_shadow_initial_none_and_explicit_alpha_are_serialized() {
    let document = crate::html::TreeBuilder::parse("<div id='target'>text</div>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime
        .eval(
            r#"(() => {
        const element = document.getElementById('target');
        const values = [getComputedStyle(element).textShadow];
        element.style.textShadow = 'rgb(0 0 0 / 0.5) 0 0';
        values.push(getComputedStyle(element).textShadow);
        element.style.textShadow = 'initial';
        values.push(getComputedStyle(element).textShadow);
        return values.join('|');
    })()"#,
        )
        .unwrap()
        .to_string(&mut runtime.context)
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(result, "none|rgba(0, 0, 0, 0.5) 0px 0px 0px|none");
}
