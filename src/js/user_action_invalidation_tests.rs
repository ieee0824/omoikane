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

fn cached_element_style(runtime: &mut JsRuntime, id: &str) -> Arc<ComputedStyle> {
    let identity = runtime
        .eval(&format!(
            "document.getElementById({}).__id",
            serde_json::to_string(id).unwrap()
        ))
        .unwrap()
        .as_number()
        .unwrap() as usize;
    let mut state = runtime.host_state.borrow_mut();
    let document = state.document.clone();
    let node = state.get_node(identity).unwrap();
    state.ensure_style_resolver(&document);
    document_style_resolver(&mut state, &document)
        .unwrap()
        .computed_style_shared(&node)
}

#[test]
fn observed_hover_updates_geometry_and_inheritance_without_discarding_unrelated_styles() {
    let mut runtime = runtime(
        "a{color:blue}a:hover{width:64px;color:red}\
        button:hover + label{color:green}",
    );
    check(
        &mut runtime,
        "document.getElementById('link').getBoundingClientRect().width===32",
    );
    let before = cached_element_style(&mut runtime, "other");
    check(
        &mut runtime,
        "(() => { const left=document.getElementById('left');\
        __omoikane_dispatch_mouse_input(left.__id,'mousemove',{},false);\
        return document.getElementById('link').getBoundingClientRect().width===64\
        && getComputedStyle(left).color==='rgb(255, 0, 0)'; })()",
    );
    let after = cached_element_style(&mut runtime, "other");
    assert!(
        Arc::ptr_eq(&before, &after),
        "the unrelated style survives the real hover/reflow path"
    );
    check(
        &mut runtime,
        "(() => {const other=document.getElementById('other');\
        __omoikane_dispatch_mouse_input(other.__id,'mousemove',{},false);\
        return document.getElementById('link').getBoundingClientRect().width===32\
        && getComputedStyle(document.getElementById('left')).color==='rgb(0, 0, 255)'; })()",
    );
}
