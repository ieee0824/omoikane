//! Live animation cache contracts: reuse geometry while sampling paint values.
use super::*;

#[test]
fn opacity_keyframes_repaint_without_rebuilding_geometry() {
    let html = "<style>body{margin:0;background:white}@keyframes fade{from{opacity:0}to{opacity:1}}#item{width:40px;height:40px;background:red;animation:fade 1s linear forwards}</style><div id='item'></div>";
    let mut runtime =
        JsRuntime::with_document(crate::html::TreeBuilder::parse(html).document()).unwrap();
    runtime.enable_live_css_animations();
    let before = runtime
        .paint_current_document()
        .unwrap()
        .pixel(20, 20)
        .unwrap();
    assert_eq!(
        (before.r, before.g, before.b, before.a),
        (255, 255, 255, 255)
    );
    let layout = runtime.render_generations().layout;
    runtime.run_animation_frame(500).unwrap();
    let middle = runtime
        .paint_current_document()
        .unwrap()
        .pixel(20, 20)
        .unwrap();
    assert_eq!((middle.r, middle.a), (255, 255));
    assert!((127..=128).contains(&middle.g) && middle.g == middle.b);
    assert_eq!(runtime.render_generations().layout, layout);
    runtime.run_animation_frame(500).unwrap();
    let after = runtime
        .paint_current_document()
        .unwrap()
        .pixel(20, 20)
        .unwrap();
    assert_eq!((after.r, after.g, after.b, after.a), (255, 0, 0, 255));
    assert_eq!(runtime.render_generations().layout, layout);
}

#[test]
fn visibility_keyframes_repaint_and_retarget_hits_without_rebuilding_geometry() {
    let html = "<style>body{margin:0;background:white}@keyframes reveal{0%,49%{visibility:hidden}50%,100%{visibility:visible}}#item{width:40px;height:40px;background:red;animation:reveal 1s step-end forwards}</style><div id='item'></div>";
    let mut runtime =
        JsRuntime::with_document(crate::html::TreeBuilder::parse(html).document()).unwrap();
    runtime.enable_live_css_animations();
    let before = runtime
        .paint_current_document()
        .unwrap()
        .pixel(20, 20)
        .unwrap();
    assert_eq!(
        (before.r, before.g, before.b, before.a),
        (255, 255, 255, 255)
    );
    assert_eq!(
        runtime
            .eval("document.elementFromPoint(20,20)?.id || ''")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        ""
    );
    assert_ne!(
        runtime.hit_test(20.0, 20.0).unwrap().get_attribute("id"),
        Some("item".to_string())
    );
    let layout = runtime.render_generations().layout;

    runtime.run_animation_frame(750).unwrap();
    let after = runtime
        .paint_current_document()
        .unwrap()
        .pixel(20, 20)
        .unwrap();
    assert_eq!((after.r, after.g, after.b, after.a), (255, 0, 0, 255));
    assert_eq!(
        runtime
            .eval("document.elementFromPoint(20,20).id")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "item"
    );
    assert_eq!(
        runtime.hit_test(20.0, 20.0).unwrap().get_attribute("id"),
        Some("item".to_string())
    );
    assert_eq!(runtime.render_generations().layout, layout);
}

#[test]
fn inline_visibility_keyframes_refresh_fragment_hit_testing_without_reflow() {
    let html = "<style>body{margin:0}@keyframes reveal{0%,49%{visibility:hidden}50%,100%{visibility:visible}}#item{font:20px/24px monospace;animation:reveal 1s step-end forwards}</style><span id='item'>XXXX</span>";
    let mut runtime =
        JsRuntime::with_document(crate::html::TreeBuilder::parse(html).document()).unwrap();
    runtime.enable_live_css_animations();
    runtime.paint_current_document().unwrap();
    assert_eq!(
        runtime
            .eval("document.elementFromPoint(5,10)?.id || ''")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        ""
    );
    let layout = runtime.render_generations().layout;

    runtime.run_animation_frame(750).unwrap();
    assert_eq!(
        runtime
            .eval("document.elementFromPoint(5,10).id")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "item"
    );
    assert_eq!(runtime.render_generations().layout, layout);
}

#[test]
fn width_keyframes_update_geometry_and_the_painted_edge() {
    let html = "<style>body{margin:0;background:white}@keyframes grow{from{width:10px}to{width:30px}}#item{height:40px;background:red;animation:grow 1s linear forwards}</style><div id='item'></div>";
    let mut runtime =
        JsRuntime::with_document(crate::html::TreeBuilder::parse(html).document()).unwrap();
    runtime.enable_live_css_animations();
    let before = runtime
        .paint_current_document()
        .unwrap()
        .pixel(15, 20)
        .unwrap();
    assert_eq!((before.r, before.g, before.b), (255, 255, 255));
    let layout = runtime.render_generations().layout;
    runtime.run_animation_frame(500).unwrap();
    assert_eq!(
        runtime
            .eval("document.getElementById('item').getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(20.0)
    );
    let middle = runtime
        .paint_current_document()
        .unwrap()
        .pixel(15, 20)
        .unwrap();
    assert_eq!((middle.r, middle.g, middle.b), (255, 0, 0));
    assert!(runtime.render_generations().layout > layout);
}

#[test]
fn pseudo_width_animation_rebuilds_geometry_and_repaints_the_changed_edge() {
    let html = "<style>body{margin:0;background:white}@keyframes grow{from{width:10px}to{width:30px}}#host::before{content:'';position:absolute;left:0;top:0;height:40px;background:red;animation:grow 1s linear forwards}</style><div id='host'></div>";
    let mut runtime =
        JsRuntime::with_document(crate::html::TreeBuilder::parse(html).document()).unwrap();
    runtime.enable_live_css_animations();
    let before = runtime
        .paint_current_document()
        .unwrap()
        .pixel(15, 20)
        .unwrap();
    assert_eq!((before.r, before.g, before.b), (255, 255, 255));
    let layout = runtime.render_generations().layout;
    runtime.run_animation_frame(500).unwrap();
    assert_eq!(
        runtime
            .eval("getComputedStyle(document.getElementById('host'), '::before').width")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "20px"
    );
    let middle = runtime
        .paint_current_document()
        .unwrap()
        .pixel(15, 20)
        .unwrap();
    assert_eq!((middle.r, middle.g, middle.b), (255, 0, 0));
    assert!(runtime.render_generations().layout > layout);
}

#[test]
fn color_transition_runtime_retains_unrelated_ordinary_and_pseudo_style_arcs() {
    let html = r#"
        <style>
          #target { color:red; transition:color 1s linear }
          #unrelated::before { content:'stable'; color:green }
        </style>
        <div id="target"><span>target</span></div>
        <aside id="unrelated"></aside>
    "#;
    let mut runtime =
        JsRuntime::with_document(crate::html::TreeBuilder::parse(html).document()).unwrap();
    runtime.enable_live_css_animations();
    assert!(
        runtime
            .eval(
                "globalThis.target=document.getElementById('target');\
                 target.getBoundingClientRect();\
                 target.style.color='blue';\
                 getComputedStyle(target).color==='rgb(255, 0, 0)'",
            )
            .unwrap()
            .to_boolean(),
    );

    let unrelated = runtime
        .host_state
        .borrow()
        .document
        .query_selector("#unrelated")
        .unwrap();
    let unrelated_styles = |runtime: &JsRuntime| {
        let mut state = runtime.host_state.borrow_mut();
        let document_id = state.document.identity();
        let resolver = state
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| entry.resolver.as_mut())
            .unwrap();
        (
            resolver.computed_style_shared(&unrelated),
            resolver
                .computed_pseudo_style_shared(&unrelated, crate::css::PseudoElement::Before)
                .unwrap(),
        )
    };
    let before = unrelated_styles(&runtime);

    runtime.run_animation_frame(500).unwrap();
    assert_eq!(
        runtime
            .eval("getComputedStyle(target).color")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "rgb(128, 0, 128)",
    );
    let after = unrelated_styles(&runtime);
    assert!(std::sync::Arc::ptr_eq(&before.0, &after.0));
    assert!(std::sync::Arc::ptr_eq(&before.1, &after.1));
}

#[test]
fn full_transition_runtime_skips_node_map_but_advances_and_clears_cached_styles() {
    let html = r#"
        <style>
          #target { width:10px; height:10px; transition:width 1s linear }
          #unrelated::before { content:'stable' }
        </style>
        <div id="target"></div>
        <aside id="unrelated"></aside>
    "#;
    let mut runtime =
        JsRuntime::with_document(crate::html::TreeBuilder::parse(html).document()).unwrap();
    runtime.enable_live_css_animations();
    runtime
        .eval(
            "globalThis.target=document.getElementById('target');\
             target.getBoundingClientRect();\
             target.style.width='30px';getComputedStyle(target).width",
        )
        .unwrap();

    let (document_id, target, unrelated) = {
        let state = runtime.host_state.borrow();
        (
            state.document.identity(),
            state.document.query_selector("#target").unwrap(),
            state.document.query_selector("#unrelated").unwrap(),
        )
    };
    let before = {
        let mut state = runtime.host_state.borrow_mut();
        let resolver = state
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| entry.resolver.as_mut())
            .unwrap();
        (
            resolver.computed_style_shared(&unrelated),
            resolver
                .computed_pseudo_style_shared(&unrelated, crate::css::PseudoElement::Before)
                .unwrap(),
        )
    };

    assert!(
        runtime
            .host_state
            .borrow()
            .transition_nodes_for_document(document_id, 500.0)
            .is_empty(),
        "a Full transition plan must not build the document-scoped node map",
    );
    let effects = runtime
        .host_state
        .borrow_mut()
        .advance_document_style_clocks(document_id, 500.0, false)
        .0;
    assert!(effects.transition_changed);
    assert!(effects.transition_requires_layout);
    assert_eq!(
        effects.transition_cache_before_animation.outcome,
        crate::css::style::TransitionCacheOutcome::Full,
    );
    assert!(effects.transition_cache_before_animation.ordinary_evicted > 0);
    assert_eq!(
        effects.transition_cache_before_animation.ordinary_retained,
        0
    );
    assert!(effects.transition_cache_before_animation.pseudo_evicted > 0);
    assert_eq!(effects.transition_cache_before_animation.pseudo_retained, 0);

    let (middle, after) = {
        let mut state = runtime.host_state.borrow_mut();
        let resolver = state
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| entry.resolver.as_mut())
            .unwrap();
        (
            resolver.computed_style_shared(&target),
            (
                resolver.computed_style_shared(&unrelated),
                resolver
                    .computed_pseudo_style_shared(&unrelated, crate::css::PseudoElement::Before)
                    .unwrap(),
            ),
        )
    };
    assert_eq!(middle.get("width"), Some(&ComputedValue::Px(20.0)));
    assert!(!std::sync::Arc::ptr_eq(&before.0, &after.0));
    assert!(!std::sync::Arc::ptr_eq(&before.1, &after.1));
}

#[test]
fn inherited_color_transition_repaints_dependants_without_rebuilding_geometry() {
    let html = r#"
        <style>
          html,body { margin:0; background:white }
          #source {
            position:relative; width:180px; height:40px;
            color:rgb(255, 0, 0); transition:color 1s linear;
          }
          #border {
            position:absolute; left:0; top:0; box-sizing:border-box;
            width:20px; height:20px; color:inherit;
            border:5px solid currentColor;
          }
          #icon {
            position:absolute; left:30px; top:0;
            width:20px; height:20px; color:inherit;
          }
          #decoration {
            position:absolute; left:60px; top:0; color:inherit;
            font:20px/24px sans-serif;
            text-decoration-line:underline;
            text-decoration-color:currentColor;
            text-decoration-thickness:4px;
            text-underline-offset:0;
            text-decoration-skip-ink:none;
          }
          #decoration > span { color:transparent }
        </style>
        <div id="source">
          <div id="border"></div>
          <svg id="icon" viewBox="0 0 20 20" fill="currentColor">
            <rect width="20" height="20"/>
          </svg>
          <span id="decoration"><span>MMMM</span></span>
        </div>
    "#;
    let mut runtime =
        JsRuntime::with_document(crate::html::TreeBuilder::parse(html).document()).unwrap();
    runtime.enable_live_css_animations();

    fn geometry(runtime: &mut JsRuntime) -> String {
        runtime
            .eval(
                "JSON.stringify(['source','border','icon','decoration'].map(id=>{\
                 const r=document.getElementById(id).getBoundingClientRect();\
                 return [r.x,r.y,r.width,r.height]}))",
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped()
    }

    let initial = runtime.paint_current_document().unwrap();
    let initial_geometry = geometry(&mut runtime);
    let rgba = |canvas: &crate::paint::Canvas, x, y| {
        let pixel = canvas.pixel(x, y).unwrap();
        (pixel.r, pixel.g, pixel.b, pixel.a)
    };
    assert_eq!(rgba(&initial, 2, 2), (255, 0, 0, 255));
    assert_eq!(rgba(&initial, 35, 5), (255, 0, 0, 255));
    let decoration_pixel = (0_u32..40)
        .find_map(|y| {
            (60_u32..180)
                .find(|&x| rgba(&initial, x, y) == (255, 0, 0, 255))
                .map(|x| (x, y))
        })
        .expect("the propagated underline must paint an opaque red pixel");

    runtime
        .eval(
            "globalThis.transitionEnds=[];\
             const source=document.getElementById('source');\
             source.addEventListener('transitionend',event=>transitionEnds.push(event.propertyName));\
             source.style.color='rgb(0, 0, 255)';getComputedStyle(source).color",
        )
        .unwrap();
    // Establish the geometry invalidated by the declaration change before
    // measuring whether subsequent transition samples cause additional reflow.
    runtime.paint_current_document().unwrap();
    let before_layout = runtime.render_generations().layout;

    runtime.run_animation_frame(500).unwrap();
    let middle = runtime.paint_current_document().unwrap();
    assert_eq!(rgba(&middle, 2, 2), (128, 0, 128, 255));
    assert_eq!(rgba(&middle, 35, 5), (128, 0, 128, 255));
    assert_eq!(
        rgba(&middle, decoration_pixel.0, decoration_pixel.1),
        (128, 0, 128, 255),
    );
    let middle_layout = runtime.render_generations().layout;
    assert_eq!(middle_layout, before_layout);
    assert_eq!(geometry(&mut runtime), initial_geometry);

    runtime.run_animation_frame(500).unwrap();
    let final_canvas = runtime.paint_current_document().unwrap();
    assert_eq!(rgba(&final_canvas, 2, 2), (0, 0, 255, 255));
    assert_eq!(rgba(&final_canvas, 35, 5), (0, 0, 255, 255));
    assert_eq!(
        rgba(&final_canvas, decoration_pixel.0, decoration_pixel.1,),
        (0, 0, 255, 255),
    );
    let final_layout = runtime.render_generations().layout;
    assert_eq!(final_layout, middle_layout);
    assert_eq!(geometry(&mut runtime), initial_geometry);
    assert_eq!(
        runtime.eval("transitionEnds.length").unwrap().as_number(),
        Some(1.0),
    );
    assert_eq!(
        runtime
            .eval("transitionEnds[0]")
            .unwrap()
            .as_string()
            .map(|value| value.to_std_string_escaped()),
        Some("color".into()),
    );

    runtime.run_animation_frame(250).unwrap();
    let after_completion = runtime.paint_current_document().unwrap();
    assert_eq!(rgba(&after_completion, 2, 2), (0, 0, 255, 255));
    assert_eq!(rgba(&after_completion, 35, 5), (0, 0, 255, 255));
    assert_eq!(
        rgba(&after_completion, decoration_pixel.0, decoration_pixel.1,),
        (0, 0, 255, 255),
    );
    assert_eq!(runtime.render_generations().layout, final_layout);
    assert_eq!(geometry(&mut runtime), initial_geometry);
    assert_eq!(
        runtime.eval("transitionEnds.length").unwrap().as_number(),
        Some(1.0),
    );
}

#[test]
fn color_transition_repaints_text_overflow_marker_without_relayout() {
    let html = r#"
        <style>
          html,body { margin:0; background:white }
          #box {
            width:90px; height:30px; overflow:hidden; white-space:nowrap;
            text-overflow:ellipsis; font:20px/30px sans-serif;
            color:rgb(255, 0, 0); transition:color 1s linear;
          }
          #box span { color:rgb(0, 0, 255) }
        </style>
        <div id=box><span>AB AB AB AB AB AB AB</span></div>
    "#;
    let mut runtime =
        JsRuntime::with_document(crate::html::TreeBuilder::parse(html).document()).unwrap();
    runtime.set_viewport(120.0, 40.0);
    runtime.enable_live_css_animations();

    let initial = runtime.paint_current_document().unwrap();
    let marker_pixel = (0..30)
        .find_map(|y| {
            (45..90).rev().find_map(|x| {
                let pixel = initial.pixel(x, y)?;
                (pixel.r > pixel.g.saturating_add(80) && pixel.r > pixel.b.saturating_add(80))
                    .then_some((x, y))
            })
        })
        .expect("the overflow marker must use the containing block's red color");

    runtime
        .eval("const box=document.getElementById('box');box.style.color='rgb(0, 255, 0)';getComputedStyle(box).color")
        .unwrap();
    runtime.paint_current_document().unwrap();
    let before_layout = runtime.render_generations().layout;

    runtime.run_animation_frame(500).unwrap();
    let middle = runtime.paint_current_document().unwrap();
    let pixel = middle.pixel(marker_pixel.0, marker_pixel.1).unwrap();
    assert!(pixel.r > pixel.b.saturating_add(40));
    assert!(pixel.g > pixel.b.saturating_add(40));
    assert!(pixel.r.abs_diff(pixel.g) <= 2);
    assert_eq!(runtime.render_generations().layout, before_layout);

    runtime.run_animation_frame(500).unwrap();
    let final_canvas = runtime.paint_current_document().unwrap();
    let pixel = final_canvas.pixel(marker_pixel.0, marker_pixel.1).unwrap();
    assert!(pixel.g > pixel.r.saturating_add(80));
    assert!(pixel.g > pixel.b.saturating_add(80));
    assert_eq!(runtime.render_generations().layout, before_layout);
}

#[test]
fn background_color_transition_repaints_without_rebuilding_geometry() {
    let html = "<style>body{margin:0;background:white}#item{width:40px;height:40px;background-color:red;transition:background-color 1s linear}</style><div id='item'></div>";
    let mut runtime =
        JsRuntime::with_document(crate::html::TreeBuilder::parse(html).document()).unwrap();
    runtime.enable_live_css_animations();
    runtime.paint_current_document().unwrap();
    runtime
        .eval(
            "const item=document.getElementById('item');\
             item.style.backgroundColor='blue';getComputedStyle(item).backgroundColor",
        )
        .unwrap();
    // The declaration change may invalidate geometry once. Measure only the
    // subsequent transition samples after the new style has been laid out.
    runtime.paint_current_document().unwrap();
    let layout = runtime.render_generations().layout;

    runtime.run_animation_frame(500).unwrap();
    let middle = runtime
        .paint_current_document()
        .unwrap()
        .pixel(20, 20)
        .unwrap();
    assert_eq!((middle.r, middle.g, middle.b, middle.a), (128, 0, 128, 255));
    assert_eq!(runtime.render_generations().layout, layout);

    runtime.run_animation_frame(500).unwrap();
    let after = runtime
        .paint_current_document()
        .unwrap()
        .pixel(20, 20)
        .unwrap();
    assert_eq!((after.r, after.g, after.b, after.a), (0, 0, 255, 255));
    assert_eq!(runtime.render_generations().layout, layout);
}
