use super::*;

fn runtime() -> JsRuntime {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"document.body.innerHTML='<iframe id="f" style="width:200px;height:100px"></iframe>';globalThis.f=document.getElementById('f');f.srcdoc='<style>body{margin:0}div{width:20px;height:10px;background:red}</style><div id="a"></div><div id="b"></div><div id="c"></div>';globalThis.d=f.contentDocument;globalThis.a=d.getElementById('a');"#).unwrap();
    runtime
}

#[test]
fn child_layout_cache_reuses_geometry_across_queries_and_parent_paints() {
    let mut runtime = runtime();
    runtime.paint_current_document().unwrap();
    let builds = runtime.host_state.borrow().child_layout_builds;
    assert!(
        runtime
            .eval(
                "['a','b','c'].every(id=>d.getElementById(id).getBoundingClientRect().width===20)"
            )
            .unwrap()
            .to_boolean()
    );
    runtime.paint_current_document().unwrap();
    assert_eq!(runtime.host_state.borrow().child_layout_builds, builds);
}

#[test]
fn child_layout_cache_observes_style_dom_and_viewport_changes() {
    let mut runtime = runtime();
    runtime.paint_current_document().unwrap();
    assert!(
        runtime
            .eval("a.style.width='30px';a.getBoundingClientRect().width===30")
            .unwrap()
            .to_boolean()
    );
    assert!(runtime.eval("const next=d.createElement('div');next.id='next';next.style.width='40px';d.body.prepend(next);a.getBoundingClientRect().top===10 && next.getBoundingClientRect().width===40").unwrap().to_boolean());
    assert!(
        runtime
            .eval(
                "a.style.width='50vw';f.style.width='300px';a.getBoundingClientRect().width===150"
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn child_layout_cache_does_not_reuse_scrolled_geometry() {
    let mut runtime = runtime();
    runtime.eval("d.body.style.height='1000px'").unwrap();
    runtime.paint_current_document().unwrap();
    assert!(
        runtime
            .eval("f.contentWindow.scrollTo(0,20);a.getBoundingClientRect().top===-20")
            .unwrap()
            .to_boolean()
    );
    runtime.paint_current_document().unwrap();
    assert!(
        runtime
            .eval("a.getBoundingClientRect().top===-20")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn child_layout_cache_tracks_animated_geometry() {
    let mut runtime = runtime();
    runtime.enable_live_css_animations();
    runtime.eval("d.querySelector('style').textContent+='@keyframes grow{from{width:20px}to{width:40px}}#a{animation:grow 1s linear forwards}'").unwrap();
    runtime.paint_current_document().unwrap();
    runtime.run_animation_frame(500).unwrap();
    assert_eq!(
        runtime
            .eval("a.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(30.0)
    );
}

#[test]
fn child_layout_cache_is_discarded_when_the_document_retires() {
    let mut runtime = runtime();
    runtime.paint_current_document().unwrap();
    let old_document = runtime
        .host_state
        .borrow()
        .iframe_documents
        .values()
        .next()
        .unwrap()
        .document
        .identity();
    runtime.eval("f.srcdoc='<div id=next style=\"width:60px;height:10px\"></div>';globalThis.fresh=f.contentDocument;fresh.getElementById('next').getBoundingClientRect().width").unwrap();
    assert!(
        !runtime
            .host_state
            .borrow()
            .document_styles
            .contains_key(&old_document)
    );
    assert_eq!(
        runtime
            .eval("fresh.getElementById('next').getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(60.0)
    );
}

#[test]
fn child_layout_cache_survives_parent_scroll_without_reflow() {
    let mut runtime = runtime();
    runtime.eval("document.body.style.height='2000px'").unwrap();
    runtime.paint_current_document().unwrap();
    let builds = runtime.host_state.borrow().child_layout_builds;
    assert!(
        runtime
            .eval("window.scrollTo(0,40);window.scrollY===40")
            .unwrap()
            .to_boolean()
    );
    runtime.paint_current_document().unwrap();
    assert_eq!(runtime.host_state.borrow().child_layout_builds, builds);
}

#[test]
fn child_layout_cache_survives_paint_only_animation() {
    let mut runtime = runtime();
    runtime.enable_live_css_animations();
    runtime.eval("d.querySelector('style').textContent+='@keyframes fade{from{opacity:0}to{opacity:1}}#a{animation:fade 1s linear forwards}'").unwrap();
    runtime.paint_current_document().unwrap();
    let builds = runtime.host_state.borrow().child_layout_builds;
    runtime.run_animation_frame(500).unwrap();
    runtime.paint_current_document().unwrap();
    assert_eq!(runtime.host_state.borrow().child_layout_builds, builds);
}

fn canvas_runtime() -> JsRuntime {
    let mut runtime = runtime();
    runtime.eval("document.body.style.margin='0';f.style.border='0';d.body.innerHTML='<canvas width=20 height=20></canvas>';globalThis.ctx=d.querySelector('canvas').getContext('2d');ctx.fillStyle='red';ctx.fillRect(0,0,20,20)").unwrap();
    runtime
}

#[test]
fn child_layout_cache_repaints_latest_canvas_pixels_without_reflow() {
    let mut runtime = canvas_runtime();
    let red = crate::paint::Color::rgb(255, 0, 0);
    let blue = crate::paint::Color::rgb(0, 0, 255);
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(5, 5),
        Some(red)
    );
    let builds = runtime.host_state.borrow().child_layout_builds;
    runtime
        .eval("ctx.fillStyle='blue';ctx.fillRect(0,0,20,20)")
        .unwrap();
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(5, 5),
        Some(blue)
    );
    assert_eq!(runtime.host_state.borrow().child_layout_builds, builds);
}

#[test]
fn child_canvas_drawing_invalidates_paint_demand_without_reflow() {
    let mut runtime = canvas_runtime();
    runtime.paint_current_document().unwrap();
    let key = runtime.paint_state_key();
    let layout = runtime.render_generations().layout;
    runtime
        .eval("ctx.fillStyle='blue';ctx.fillRect(0,0,20,20)")
        .unwrap();
    assert_ne!(runtime.paint_state_key(), key);
    runtime.paint_current_document().unwrap();
    assert_eq!(runtime.render_generations().layout, layout);
}

#[test]
fn child_layout_cache_paints_a_canvas_first_draw_after_initial_layout() {
    let mut runtime = runtime();
    runtime.eval("document.body.style.margin='0';f.style.border='0';d.body.innerHTML='<canvas width=20 height=20></canvas>';globalThis.ctx=d.querySelector('canvas').getContext('2d')").unwrap();
    runtime.paint_current_document().unwrap();
    let builds = runtime.host_state.borrow().child_layout_builds;
    runtime
        .eval("ctx.fillStyle='red';ctx.fillRect(0,0,20,20)")
        .unwrap();
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(5, 5),
        Some(crate::paint::Color::rgb(255, 0, 0))
    );
    assert_eq!(runtime.host_state.borrow().child_layout_builds, builds);
}

#[test]
fn main_layout_cache_repaints_current_inline_canvas_pixels() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval("document.body.style.margin='0';document.body.innerHTML='<canvas width=20 height=20></canvas>';globalThis.ctx=document.querySelector('canvas').getContext('2d');ctx.fillStyle='red';ctx.fillRect(0,0,20,20)").unwrap();
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(5, 5),
        Some(crate::paint::Color::rgb(255, 0, 0))
    );
    let layout = runtime.render_generations().layout;
    runtime
        .eval("ctx.fillStyle='blue';ctx.fillRect(0,0,20,20)")
        .unwrap();
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(5, 5),
        Some(crate::paint::Color::rgb(0, 0, 255))
    );
    assert_eq!(runtime.render_generations().layout, layout);
}

#[test]
fn child_snapshot_attribution_connects_candidate_raster_style_and_pixels() {
    let runtime = runtime();
    let mut state = runtime.host_state.borrow_mut();
    state.ensure_adjusted_layout();
    let layout = state.adjusted_layout_cache.as_ref().unwrap().root.clone();
    let document_id = state.document.identity();
    let frame = state.document.query_selector("#f").unwrap();
    let child_document_id = state.iframe_content_document(&frame).unwrap().identity();
    let mut attribution =
        frame_diagnostics::PaintAttributionScope::begin_with_enabled(true, document_id);

    let snapshots =
        nested_rendering::collect_snapshots(&mut state, &layout, 0, &mut attribution).unwrap();
    let child = snapshots.get(&frame.identity()).unwrap();
    assert_eq!((child.width(), child.height()), (200, 100));
    let pixel_offset = (5 * child.width() as usize + 5) * 4;
    assert_eq!(
        &child.pixels()[pixel_offset..pixel_offset + 4],
        &[255, 0, 0, 255]
    );

    let data = attribution.finish_data_for_test(true).unwrap();
    assert_eq!(data["document_id"], document_id);
    assert_eq!(data["snapshot_candidate_owners"], 1);
    assert_eq!(data["child_rasters"], 1);
    assert_eq!(data["child_raster_detail_overflow"], 0);
    assert_eq!(data["child_details"][0]["document_id"], child_document_id);
    assert!(
        data["child_details"][0]["style_cache_hits"]
            .as_u64()
            .is_some_and(|hits| hits > 0)
    );
}
