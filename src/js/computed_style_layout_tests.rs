use super::*;

fn eval_text(runtime: &mut JsRuntime, source: &str) -> String {
    runtime
        .eval(source)
        .unwrap()
        .as_string()
        .expect("string result")
        .to_std_string_escaped()
}

fn has_layout(runtime: &JsRuntime) -> bool {
    runtime.host_state.borrow().layout_root.is_some()
}

#[test]
fn computed_style_reads_lay_out_only_for_resolved_sizes() {
    let document = crate::html::TreeBuilder::parse(
        "<style>#target { color: red; width: 40px; }</style><div id='target'></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime
        .eval("globalThis.target = document.getElementById('target'); target.style.color = 'blue';")
        .unwrap();
    assert!(
        !has_layout(&runtime),
        "an inline style change drops the layout"
    );

    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(target).color"),
        "rgb(0, 0, 255)"
    );
    assert_eq!(
        eval_text(&mut runtime, "String('color' in getComputedStyle(target))"),
        "true"
    );
    assert!(
        !has_layout(&runtime),
        "getComputedStyle and non-size reads must not lay out the document"
    );

    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(target).width"),
        "40px"
    );
    assert!(has_layout(&runtime), "a width read resolves the used size");
    runtime.eval("target.style.color = 'green'").unwrap();
    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(target).inlineSize"),
        "40px"
    );
    assert!(
        has_layout(&runtime),
        "inline-size aliases the resolved width"
    );
}

#[test]
fn container_dependent_computed_values_use_current_layout() {
    let document = crate::html::TreeBuilder::parse(
        r#"<style>
            #shell { width: 600px; container-type: inline-size; container-name: shell; }
            @container shell (inline-size >= 400px) { #item { color: rgb(0, 128, 0); } }
            #tile { color: color(srgb calc(0.5 + (sign(2cqw - 10px) * 0.1)) 0 0); }
        </style><section id="shell"><p id="item"></p><p id="tile"></p></section>"#,
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime
        .eval(
            "globalThis.shell = document.getElementById('shell');
             globalThis.item = document.getElementById('item');
             globalThis.tile = document.getElementById('tile');",
        )
        .unwrap();
    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(item).color"),
        "rgb(0, 128, 0)"
    );
    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(tile).color"),
        "color(srgb 0.6 0 0)"
    );

    runtime.eval("shell.style.width = '300px'").unwrap();
    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(item).color"),
        "rgb(0, 0, 0)",
        "a container query must be re-evaluated against the new container size"
    );
    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(tile).color"),
        "color(srgb 0.4 0 0)",
        "container-relative colors must use the new container size"
    );
}

#[test]
fn named_logical_cssom_reads_match_full_snapshots_after_flow_changes() {
    let document = crate::html::TreeBuilder::parse(
        "<div id='target' style='width:80px;height:40px;color:rgb(10,20,30);\
         margin:1px 2px 3px 4px;padding:5px 6px 7px 8px;\
         border-style:solid;border-width:1px 2px 3px 4px;min-width:auto;min-height:auto'></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = eval_text(
        &mut runtime,
        r#"
        (() => {
            const element = document.getElementById('target');
            const style = getComputedStyle(element);
            for (const [writingMode, direction] of [
                ['horizontal-tb', 'ltr'], ['horizontal-tb', 'rtl'],
                ['vertical-rl', 'ltr'], ['vertical-lr', 'rtl']
            ]) {
                element.style.writingMode = writingMode;
                element.style.direction = direction;
                const snapshot = new Map(style.cssText.split(';').filter(Boolean).map(row => {
                    const colon = row.indexOf(':');
                    return [row.slice(0, colon).trim(), row.slice(colon + 1).trim()];
                }));
                for (const [name, value] of snapshot) {
                    if (style.getPropertyValue(name) !== value) {
                        throw new Error(writingMode + '/' + direction + ': ' + name);
                    }
                }
                if (style.color !== 'rgb(10, 20, 30)' || style.width !== '80px') {
                    throw new Error('physical value changed');
                }
                const inlineSize = writingMode === 'horizontal-tb' ? '80px' : '40px';
                if (style.inlineSize !== inlineSize || style.minInlineSize !== '0px'
                    || style.borderInlineStartColor !== 'rgb(10, 20, 30)') {
                    throw new Error('logical value changed');
                }
            }
            const shell = document.createElement('section');
            shell.style.display = 'flex';
            document.body.appendChild(shell);
            shell.appendChild(element);
            if (style.minInlineSize !== 'auto') throw new Error('flex auto minimum changed');
            return 'ok';
        })()
    "#,
    );
    assert_eq!(result, "ok");
}
