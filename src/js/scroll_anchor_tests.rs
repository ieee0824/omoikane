use super::*;

fn runtime_with_scroller(overflow_anchor: &str) -> JsRuntime {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(&format!(
            r#"document.body.innerHTML = `
                <style>body{{margin:0}}#sc{{width:80px;height:40px;overflow:auto;overflow-anchor:{overflow_anchor}}}
                .spacer{{height:40px}}#anchor{{height:20px}}#tail{{height:80px}}</style>
                <div id=sc><div class=spacer></div><div id=anchor></div><div id=tail></div></div>`;
                globalThis.sc = document.getElementById('sc');
                globalThis.anchor = document.getElementById('anchor');"#
        ))
        .unwrap();
    runtime
}

#[test]
fn layout_rebuild_keeps_the_visible_anchor_stable_after_dom_insertion() {
    let mut runtime = runtime_with_scroller("auto");
    assert_eq!(
        runtime
            .eval("sc.scrollTop=40; sc.scrollTop")
            .unwrap()
            .as_number(),
        Some(40.0)
    );
    let result = runtime
        .eval(
            "const inserted=document.createElement('div');\
             inserted.style.height='20px';\
             anchor.before(inserted);\
             sc.scrollTop",
        )
        .unwrap();
    assert_eq!(result.as_number(), Some(60.0));
}

#[test]
fn overflow_anchor_none_suppresses_layout_scroll_adjustment() {
    let mut runtime = runtime_with_scroller("none");
    let result = runtime
        .eval(
            "sc.scrollTop=40;\
             const inserted=document.createElement('div');\
             inserted.style.height='20px';\
             anchor.before(inserted);\
             sc.scrollTop",
        )
        .unwrap();
    assert_eq!(result.as_number(), Some(40.0));
}
