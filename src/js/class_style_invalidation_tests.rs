//! Class mutation cache reuse must preserve live style and layout behavior.
use super::*;

fn observer_style(runtime: &mut JsRuntime) -> Arc<ComputedStyle> {
    let mut state = runtime.host_state.borrow_mut();
    let observer = state
        .nodes
        .values()
        .find(|node| node.get_attribute("id").as_deref() == Some("observer"))
        .unwrap()
        .clone();
    let document_id = state.document.identity();
    state
        .document_styles
        .get_mut(&document_id)
        .unwrap()
        .resolver
        .as_mut()
        .unwrap()
        .computed_style_shared(&observer)
}

#[test]
fn class_change_updates_descendant_geometry_and_retains_unrelated_style() {
    let document = crate::html::TreeBuilder::parse(
        "<style>#target{width:10px}.child{width:5px}\
         #target.active{width:60px}.active .child{width:30px}\
         #observer{width:70px}</style>\
         <div id='target'><span class='child'></span></div><aside id='observer'></aside>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert!(
        runtime
            .eval(
                "document.getElementById('observer').getBoundingClientRect().width === 70 && \
         document.getElementById('target').getBoundingClientRect().width === 10"
            )
            .unwrap()
            .as_boolean()
            .unwrap()
    );
    let before = observer_style(&mut runtime);
    assert!(
        runtime
            .eval(
                "document.getElementById('target').className = 'active'; \
         getComputedStyle(document.getElementById('target')).width === '60px' && \
         getComputedStyle(document.querySelector('.child')).width === '30px' && \
         document.getElementById('target').getBoundingClientRect().width === 60"
            )
            .unwrap()
            .as_boolean()
            .unwrap()
    );
    let after = observer_style(&mut runtime);
    assert!(
        Arc::ptr_eq(&before, &after),
        "unrelated computed style should survive a subtree-local class change"
    );
}

#[test]
fn class_change_does_not_freeze_an_unrelated_running_animation() {
    let document = crate::html::TreeBuilder::parse(
        "<style>@keyframes fade{from{opacity:0}to{opacity:1}}\
         #observer{animation:fade 1s linear forwards}\
         #target.active{width:60px}</style>\
         <div id='target'></div><aside id='observer'></aside>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.enable_live_css_animations();
    assert!(
        runtime
            .eval("getComputedStyle(document.getElementById('observer')).opacity === '0'")
            .unwrap()
            .as_boolean()
            .unwrap()
    );
    runtime
        .eval("document.getElementById('target').className = 'active'")
        .unwrap();
    runtime.run_animation_frame(500).unwrap();
    assert!(
        runtime
            .eval(
                "getComputedStyle(document.getElementById('observer')).opacity === '0.5' && \
         getComputedStyle(document.getElementById('target')).width === '60px'"
            )
            .unwrap()
            .as_boolean()
            .unwrap()
    );
}

#[test]
fn class_change_starts_transitions_and_keeps_them_advancing() {
    let document = crate::html::TreeBuilder::parse(
        "<style>#observer{width:10px;transition:width 1s linear}\
         #observer.grow{width:30px}#target.active{width:60px}</style>\
         <div id='target'></div><aside id='observer'></aside>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.enable_live_css_animations();
    runtime
        .eval(
            "globalThis.observer = document.getElementById('observer'); \
         observer.getBoundingClientRect(); observer.className = 'grow'; \
         getComputedStyle(observer).width; \
         document.getElementById('target').className = 'active'",
        )
        .unwrap();
    runtime.run_animation_frame(500).unwrap();
    assert!(
        runtime
            .eval(
                "observer.getBoundingClientRect().width === 20 && \
         document.getElementById('target').getBoundingClientRect().width === 60"
            )
            .unwrap()
            .as_boolean()
            .unwrap()
    );
    runtime.run_animation_frame(500).unwrap();
    assert!(
        runtime
            .eval("observer.getBoundingClientRect().width === 30")
            .unwrap()
            .as_boolean()
            .unwrap()
    );
}
