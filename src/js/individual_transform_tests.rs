//! Individual-transform contracts for the browser's live animation clock.
use super::JsRuntime;
use crate::html::TreeBuilder;

fn assert_js(runtime: &mut JsRuntime, script: &str) {
    assert_eq!(
        runtime.eval(script).unwrap().as_boolean(),
        Some(true),
        "{script}"
    );
}

#[test]
fn delayed_transform_animation_stacks_without_changing_containing_blocks() {
    let html = "<style>body{margin:0}@keyframes move{from,to{scale:1}}#parent{margin-left:40px;margin-top:30px;width:20px;height:20px;animation:move 1s 1s}#child{position:relative;z-index:999;width:20px;height:20px}#sibling{position:relative;z-index:1;margin-left:40px;margin-top:-20px;width:20px;height:20px}#fixed{position:fixed;left:10px;top:10px;width:5px;height:5px}</style><div id='parent'><div id='child'></div><div id='fixed'></div></div><div id='sibling'></div>";
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(html).document()).unwrap();
    runtime.enable_live_css_animations();
    assert_js(
        &mut runtime,
        "getComputedStyle(document.getElementById('parent')).scale==='none' && document.getElementById('fixed').getBoundingClientRect().left===10 && document.elementFromPoint(45,35).id==='sibling'",
    );
    runtime.run_animation_frame(2500).unwrap();
    assert_js(
        &mut runtime,
        "getComputedStyle(document.getElementById('parent')).scale==='none' && document.elementFromPoint(45,35).id==='child'",
    );
}

#[test]
fn live_individual_keyframes_sample_all_three_properties() {
    let html = "<style>body{margin:0}@keyframes move{from{translate:none;rotate:none;scale:none}to{translate:20px 10px;rotate:90deg;scale:2}}#item{width:10px;height:20px;transform-origin:0 0;animation:move 1s linear forwards}</style><div id='item'></div>";
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(html).document()).unwrap();
    runtime.enable_live_css_animations();
    runtime
        .eval("globalThis.item=document.getElementById('item');getComputedStyle(item).translate")
        .unwrap();
    runtime.run_animation_frame(500).unwrap();
    assert_js(
        &mut runtime,
        "(() => {const s=getComputedStyle(item),r=item.getBoundingClientRect();return s.translate==='10px 5px' && s.rotate==='45deg' && s.scale==='1.5' && r.width===31.82;})()",
    );
    runtime.run_animation_frame(500).unwrap();
    assert_js(
        &mut runtime,
        "getComputedStyle(item).translate==='20px 10px' && getComputedStyle(item).rotate==='90deg' && getComputedStyle(item).scale==='2'",
    );
}
