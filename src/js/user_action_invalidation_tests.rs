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

#[test]
fn paint_only_hover_updates_cssom_pixels_and_hit_testing_without_relayout() {
    let mut runtime = runtime("a{height:32px;background:red;opacity:1}a:hover{opacity:0.25}");
    check(
        &mut runtime,
        "document.getElementById('link').getBoundingClientRect().width===32",
    );
    let before_pixels = runtime.paint_current_document().unwrap();
    let before = runtime.render_generations();
    check(
        &mut runtime,
        "(() => { const link=document.getElementById('link'); \
        __omoikane_dispatch_mouse_input(link.__id,'mousemove',{},false); \
        return getComputedStyle(link).opacity==='0.25' \
        && link.getBoundingClientRect().width===32 \
        && document.elementFromPoint(16,16)===link; })()",
    );
    let after_pixels = runtime.paint_current_document().unwrap();
    assert_ne!(before_pixels.pixels(), after_pixels.pixels());
    assert_eq!(runtime.render_generations().layout, before.layout);
}

#[test]
fn filter_hover_with_fixed_descendant_invalidates_geometry() {
    let html = "<style>\
        body{margin:0}\
        #link{display:block;margin-left:40px;width:32px;height:32px;filter:none}\
        #link:hover{filter:blur(1px)}\
        #fixed{position:fixed;left:5px;top:5px;width:5px;height:5px}\
        </style><a id=link><span id=fixed></span></a>";
    let mut runtime =
        JsRuntime::with_document(crate::html::TreeBuilder::parse(html).document()).unwrap();
    check(
        &mut runtime,
        "document.getElementById('fixed').getBoundingClientRect().left===5",
    );
    let before = runtime.render_generations();
    check(
        &mut runtime,
        "(() => { const link=document.getElementById('link'); \
        __omoikane_dispatch_mouse_input(link.__id,'mousemove',{},false); \
        return getComputedStyle(link).filter!=='none' \
        && Number.isFinite(document.getElementById('fixed').getBoundingClientRect().left); })()",
    );
    assert!(runtime.render_generations().layout > before.layout);
}

#[test]
fn opacity_hover_hit_order_matches_full_layout_reference() {
    let base = "a{position:absolute;left:0;top:0;height:32px;opacity:.25;background:red}\
        #left{display:block;position:absolute;left:0;top:0;width:32px;height:32px;z-index:2;background:green}\
        #other{position:absolute;left:0;top:0;width:16px;height:32px;z-index:1;background:blue}";
    let mut optimized = runtime(&format!("{base}a:hover{{opacity:.75}}"));
    // The unchanged width deliberately selects the conservative full-layout
    // path while producing the same final computed style and geometry.
    let mut reference = runtime(&format!("{base}a:hover{{opacity:.75;width:32px}}"));
    for runtime in [&mut optimized, &mut reference] {
        check(
            runtime,
            "document.elementFromPoint(8,16).id==='other' \
            && document.elementFromPoint(24,16).id==='left'",
        );
    }
    let optimized_pixels_before = optimized.paint_current_document().unwrap();
    let optimized_before = optimized.render_generations();
    let reference_before = reference.render_generations();
    let hover = "(() => { const link=document.getElementById('link'); \
        __omoikane_dispatch_mouse_input(link.__id,'mousemove',{},false); \
        return getComputedStyle(link).opacity==='0.75' \
        && document.elementFromPoint(8,16).id==='other' \
        && document.elementFromPoint(24,16).id==='left'; })()";
    check(&mut optimized, hover);
    check(&mut reference, hover);
    let optimized_pixels_after = optimized.paint_current_document().unwrap();
    let reference_pixels_after = reference.paint_current_document().unwrap();
    assert_ne!(
        optimized_pixels_before.pixel(24, 16),
        optimized_pixels_after.pixel(24, 16),
        "the unoccluded half must reflect the live opacity"
    );
    assert_eq!(
        optimized_pixels_after.pixels(),
        reference_pixels_after.pixels()
    );
    assert_eq!(
        optimized.render_generations().layout,
        optimized_before.layout
    );
    assert!(reference.render_generations().layout > reference_before.layout);
}

#[test]
fn nested_geometry_rule_under_hover_rebuilds_layout() {
    let mut runtime = runtime(
        "#left{display:block;width:32px;height:10px}\
         #link:hover{& > #left{width:100px}}",
    );
    check(
        &mut runtime,
        "document.getElementById('left').getBoundingClientRect().width===32",
    );
    let before = runtime.render_generations();
    check(
        &mut runtime,
        "(() => { const link=document.getElementById('link'); \
        __omoikane_dispatch_mouse_input(link.__id,'mousemove',{},false); \
        return document.getElementById('left').getBoundingClientRect().width===100; })()",
    );
    assert!(runtime.render_generations().layout > before.layout);
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
    let before_generations = runtime.render_generations();
    check(
        &mut runtime,
        "(() => { const left=document.getElementById('left');\
        __omoikane_dispatch_mouse_input(left.__id,'mousemove',{},false);\
        return document.getElementById('link').getBoundingClientRect().width===64\
        && getComputedStyle(left).color==='rgb(255, 0, 0)'; })()",
    );
    let after = cached_element_style(&mut runtime, "other");
    assert!(
        runtime.render_generations().layout > before_generations.layout,
        "geometry-changing hover must rebuild layout"
    );
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
