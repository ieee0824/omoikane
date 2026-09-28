use super::*;

#[test]
fn computed_style_uses_before_and_after_pseudo_cascade() {
    let html = r#"
        <style>
            #target { color: red; --base: inherited; }
            #target::before { content: 'before'; color: blue; --x: before; }
            #target::after { content: 'after'; color: green; --x: after; }
            #target::after { @media (width > 0px) { --x: nested; } }
        </style>
        <div id='target'></div><div id='bare'></div>
    "#;
    let document = crate::html::TreeBuilder::parse(html).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(320.0, 200.0);
    let result = runtime
        .eval(
            r#"(() => {
                const target = document.getElementById('target');
                const bare = document.getElementById('bare');
                const before = getComputedStyle(target, '::before');
                const after = getComputedStyle(target, ':after');
                const ordinary = getComputedStyle(target);
                return [
                    before.color,
                    before.getPropertyValue('--x'),
                    before.getPropertyValue('--base'),
                    after.color,
                    after.getPropertyValue('--x'),
                    ordinary.color,
                    ordinary.getPropertyValue('--x'),
                    getComputedStyle(target, '::unknown').getPropertyValue('color'),
                    getComputedStyle(target, '::part(foo)').getPropertyValue('color'),
                    getComputedStyle(target, 'not-a-pseudo').color,
                    getComputedStyle(bare, '::before').getPropertyValue('--x'),
                    getComputedStyle(bare, '::before').color
                ].join('|');
            })()"#,
        )
        .unwrap()
        .to_string(&mut runtime.context)
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        result,
        "rgb(0, 0, 255)|before|inherited|rgb(0, 128, 0)|nested|rgb(255, 0, 0)||||rgb(255, 0, 0)||rgb(0, 0, 0)"
    );
}

#[test]
fn computed_pseudo_style_is_live_after_style_changes() {
    let document = crate::html::TreeBuilder::parse(
        r#"
            <style>
                #target { color: red; --base: inherited; }
                #target::before { content: 'before'; color: blue; --x: before; }
                #target::after { content: 'after'; color: green; --x: after; }
                #target::after { @media (width > 0px) { --x: nested; } }
            </style>
            <div id='target'></div><div id='bare'></div>
        "#,
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(320.0, 200.0);
    let value = runtime
        .eval(
            r#"(() => {
                const target = document.getElementById('target');
                const style = getComputedStyle(target, '::before');
                const before = style.getPropertyValue('--base');
                target.style.setProperty('--base', 'second');
                return before + '|' + style.getPropertyValue('--base');
            })()"#,
        )
        .unwrap()
        .to_string(&mut runtime.context)
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(value, "inherited|second");
}
