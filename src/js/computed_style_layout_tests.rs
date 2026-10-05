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
