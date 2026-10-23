//! Dynamic selector state must only discard geometry when a rule can observe it.
use super::*;

fn runtime(css: &str) -> JsRuntime {
    let html = format!(
        "<style>a{{display:block;width:32px}}{css}</style>\
         <a id=link><span id=left></span><span id=right></span></a>\
         <div id=other></div>"
    );
    JsRuntime::with_document(crate::html::TreeBuilder::parse(&html).document()).unwrap()
}

fn check(runtime: &mut JsRuntime, source: &str) {
    assert_eq!(
        runtime.eval(source).unwrap().as_boolean(),
        Some(true),
        "{source}"
    );
}

#[test]
fn moving_inside_a_hovered_anchor_keeps_layout_and_updates_selector_state() {
    let mut runtime = runtime("a:hover{width:64px}");
    check(
        &mut runtime,
        "(() => { const left=document.getElementById('left'); \
        __omoikane_dispatch_mouse_input(left.__id,'mousemove',{},false); \
        return document.getElementById('link').getBoundingClientRect().width===64; })()",
    );
    let before = runtime.render_generations();
    check(
        &mut runtime,
        "(() => { const right=document.getElementById('right'); \
        __omoikane_dispatch_mouse_input(right.__id,'mousemove',{},false); \
        return right.matches(':hover') && !document.getElementById('left').matches(':hover') \
        && document.getElementById('link').matches(':hover') \
        && document.getElementById('link').getBoundingClientRect().width===64; })()",
    );
    let after = runtime.render_generations();
    assert_eq!(
        after.style, before.style,
        "only unobserved span flags changed"
    );
    assert_eq!(
        after.layout, before.layout,
        "the anchor geometry is still valid"
    );
    check(
        &mut runtime,
        "(() => { const other=document.getElementById('other'); \
        __omoikane_dispatch_mouse_input(other.__id,'mousemove',{},false); \
        return !document.getElementById('link').matches(':hover') \
        && document.getElementById('link').getBoundingClientRect().width===32; })()",
    );
    assert!(runtime.render_generations().layout > after.layout);
}

#[test]
fn hovering_without_observing_rules_keeps_geometry_and_live_matches() {
    let mut runtime = runtime("");
    check(
        &mut runtime,
        "document.getElementById('link').getBoundingClientRect().width===32",
    );
    let before = runtime.render_generations();
    check(
        &mut runtime,
        "(() => { const left=document.getElementById('left'); \
        __omoikane_dispatch_mouse_input(left.__id,'mousemove',{},false); \
        return left.matches(':hover') && document.getElementById('link').matches(':hover') \
        && document.getElementById('link').getBoundingClientRect().width===32; })()",
    );
    let after = runtime.render_generations();
    assert_eq!(after.style, before.style);
    assert_eq!(after.layout, before.layout);
}
