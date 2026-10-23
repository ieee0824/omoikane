//! Cached hit order must match a fresh traversal after observable changes.
use super::*;

fn fixture(extra_style: &str) -> JsRuntime {
    let html = format!(
        "<style>html,body{{margin:0}}body{{height:1200px}}\
         #back,#front{{position:absolute;left:10px;top:10px;width:60px;height:60px}}\
         #back{{background:red;z-index:1}}#front{{background:blue;z-index:2}}\
         {extra_style}</style><div id='back'></div><div id='front'></div>"
    );
    let mut runtime =
        JsRuntime::with_document(crate::html::TreeBuilder::parse(&html).document()).unwrap();
    runtime.set_viewport(160.0, 120.0);
    runtime
        .eval("globalThis.front=document.getElementById('front')")
        .unwrap();
    runtime
}

fn assert_fresh(runtime: &mut JsRuntime, x: f32, y: f32, expected: &str) {
    assert_eq!(
        runtime
            .hit_test(x, y)
            .unwrap()
            .get_attribute("id")
            .as_deref(),
        Some(expected)
    );
    let mut state = runtime.host_state.borrow_mut();
    state.ensure_adjusted_layout();
    let viewport = state.viewport;
    let document_id = state.document.identity();
    let state = &mut *state;
    let cache = state.adjusted_layout_cache.as_mut().unwrap();
    let resolver = state
        .document_styles
        .get_mut(&document_id)
        .unwrap()
        .resolver
        .as_mut()
        .unwrap();
    let fresh = crate::paint::hit_test_layout_all(&cache.root, resolver, viewport, x, y);
    let cached = cache.hit_test.query(resolver, viewport, x, y, true);
    assert_eq!(cached, fresh, "all targets in paint order");
    assert_eq!(
        cache
            .hit_test
            .query(resolver, viewport, x, y, false)
            .first(),
        fresh.first(),
        "first and all queries share the same ordering"
    );
}

#[test]
fn hit_order_reuses_phases_and_rebuilds_after_style_and_tree_changes() {
    let mut runtime = fixture("");
    assert_fresh(&mut runtime, 20.0, 20.0, "front");
    let root = runtime
        .host_state
        .borrow()
        .adjusted_layout_cache
        .as_ref()
        .unwrap()
        .root
        .clone();
    let count = runtime
        .host_state
        .borrow()
        .adjusted_layout_cache
        .as_ref()
        .unwrap()
        .hit_test
        .prepared_phase_count();
    assert!(count > 0);
    for _ in 0..8 {
        assert_fresh(&mut runtime, 20.0, 20.0, "front");
    }
    assert!(Rc::ptr_eq(
        &root,
        &runtime
            .host_state
            .borrow()
            .adjusted_layout_cache
            .as_ref()
            .unwrap()
            .root,
    ));
    assert_eq!(
        count,
        runtime
            .host_state
            .borrow()
            .adjusted_layout_cache
            .as_ref()
            .unwrap()
            .hit_test
            .prepared_phase_count()
    );
    for (change, expected) in [
        ("front.style.zIndex='-1'", "back"),
        ("front.style.zIndex='3'", "front"),
        ("front.style.transform='translateX(80px)'", "back"),
        ("front.style.transform=''", "front"),
        ("front.style.pointerEvents='none'", "back"),
        ("front.style.pointerEvents='auto'", "front"),
        ("front.remove()", "back"),
        ("document.body.appendChild(front)", "front"),
    ] {
        runtime.eval(change).unwrap();
        assert_fresh(&mut runtime, 20.0, 20.0, expected);
    }
}

#[test]
fn hit_order_tracks_scroll_and_animation_generations() {
    let mut runtime = fixture("#front{top:110px}");
    assert_fresh(&mut runtime, 20.0, 20.0, "back");
    runtime.eval("window.scrollTo(0,100)").unwrap();
    assert_fresh(&mut runtime, 20.0, 20.0, "front");
    runtime.eval("window.scrollTo(0,0)").unwrap();
    assert_fresh(&mut runtime, 20.0, 20.0, "back");

    let mut animated = fixture(
        "@keyframes move{from{transform:translateX(0px)}to{transform:translateX(80px)}}\
         #front{animation:move 1s linear forwards}",
    );
    animated.enable_live_css_animations();
    assert_fresh(&mut animated, 20.0, 20.0, "front");
    animated.run_animation_frame(1000).unwrap();
    assert_fresh(&mut animated, 20.0, 20.0, "back");
}
