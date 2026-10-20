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
