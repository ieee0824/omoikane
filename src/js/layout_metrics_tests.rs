use super::JsRuntime;
use crate::html::TreeBuilder;

fn runtime(html: &str) -> JsRuntime {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse(html).document(),
        "https://example.test/index.html",
    )
    .unwrap();
    runtime.eval("const nativeMetrics = __omoikane_layout_metrics; globalThis.metricCalls = 0; globalThis.__omoikane_layout_metrics = id => { metricCalls++; return nativeMetrics(id); };").unwrap();
    runtime
}

fn number(runtime: &mut JsRuntime, source: &str) -> f64 {
    runtime.eval(source).unwrap().as_number().unwrap()
}

#[test]
fn layout_metrics_cache_shares_geometry_for_one_element_and_keeps_rects_independent() {
    let mut runtime = runtime(
        "<html><body><div id='target' style='width:100px;height:20px'></div><div id='other' style='width:60px'></div></body></html>",
    );
    runtime.eval("globalThis.target = document.getElementById('target'); globalThis.other = document.getElementById('other'); for (let i=0;i<100;i++) { target.offsetWidth; target.offsetHeight; target.clientWidth; target.scrollHeight; target.getBoundingClientRect(); target.getClientRects(); }").unwrap();
    assert_eq!(number(&mut runtime, "metricCalls"), 1.0);
    assert_eq!(number(&mut runtime, "other.offsetWidth"), 60.0);
    assert_eq!(number(&mut runtime, "metricCalls"), 2.0);
    runtime.eval("target.getBoundingClientRect().width = -1; const rects = target.getClientRects(); rects[0].width = -1; rects.push({width:-1});").unwrap();
    assert_eq!(
        number(&mut runtime, "target.getClientRects()[0].width"),
        100.0
    );
    assert_eq!(number(&mut runtime, "target.getClientRects().length"), 1.0);
    assert_eq!(number(&mut runtime, "target.offsetWidth"), 100.0);
    assert_eq!(number(&mut runtime, "metricCalls"), 2.0);
}

#[test]
fn layout_metrics_cache_invalidates_for_inline_style_cssom_and_tree_changes() {
    let mut runtime = runtime(
        "<html><head><style>#target {width:100px;height:20px}</style></head><body><div id='target'></div></body></html>",
    );
    runtime
        .eval("globalThis.target = document.getElementById('target');")
        .unwrap();
    assert_eq!(number(&mut runtime, "target.offsetWidth"), 100.0);
    runtime.eval("target.style.width='140px';").unwrap();
    assert_eq!(number(&mut runtime, "target.clientWidth"), 140.0);
    runtime.eval("target.style.removeProperty('width'); document.styleSheets[0].insertRule('#target {width:180px}', 1);").unwrap();
    assert_eq!(number(&mut runtime, "target.offsetWidth"), 180.0);
    assert_eq!(
        number(&mut runtime, "target.getBoundingClientRect().width"),
        180.0
    );
    assert_eq!(number(&mut runtime, "metricCalls"), 3.0);
    runtime
        .eval("target.remove(); target.style.width='220px';")
        .unwrap();
    assert_eq!(number(&mut runtime, "target.offsetWidth"), 0.0);
    runtime.eval("document.body.appendChild(target);").unwrap();
    assert_eq!(number(&mut runtime, "target.offsetWidth"), 220.0);
    assert_eq!(number(&mut runtime, "target.clientWidth"), 220.0);
    assert_eq!(number(&mut runtime, "metricCalls"), 5.0);
}

#[test]
fn layout_metrics_cache_tracks_scroll_transforms_and_native_viewport_changes() {
    let mut runtime = runtime(
        "<html><body><div id='scroller' style='width:100px;height:80px;overflow:auto'><div id='target' style='width:200px;height:300px'></div></div></body></html>",
    );
    runtime.set_viewport(400.0, 300.0);
    runtime.eval("globalThis.target=document.getElementById('target'); globalThis.scroller=document.getElementById('scroller'); globalThis.before=target.getBoundingClientRect(); scroller.scrollTop=40; scroller.scrollLeft=20;").unwrap();
    assert_eq!(
        number(&mut runtime, "target.getBoundingClientRect().y-before.y"),
        -40.0
    );
    assert_eq!(
        number(&mut runtime, "target.getBoundingClientRect().x-before.x"),
        -20.0
    );
    runtime
        .eval("target.style.transform='translateX(30px)';")
        .unwrap();
    assert_eq!(
        number(&mut runtime, "target.getBoundingClientRect().x-before.x"),
        10.0
    );
    assert_eq!(
        number(&mut runtime, "document.documentElement.clientWidth"),
        400.0
    );
    runtime.set_viewport(600.0, 300.0);
    assert_eq!(
        number(&mut runtime, "document.documentElement.clientWidth"),
        600.0
    );
}

#[test]
fn layout_metrics_cache_samples_transitions_without_dom_mutations_between_frames() {
    let mut runtime = runtime(
        "<html><head><style>#target { width:100px;height:20px;transform:translateX(0px);transition:transform 1s linear; }</style></head><body><div id='target'></div></body></html>",
    );
    runtime.eval("globalThis.target=document.getElementById('target'); globalThis.startX=target.getBoundingClientRect().x; target.style.transform='translateX(100px)'; getComputedStyle(target).transform;").unwrap();
    runtime.run_animation_frame(250).unwrap();
    assert_eq!(
        number(&mut runtime, "target.getBoundingClientRect().x-startX"),
        25.0
    );
    runtime.run_animation_frame(250).unwrap();
    assert_eq!(
        number(&mut runtime, "target.getBoundingClientRect().x-startX"),
        50.0
    );
}

#[test]
fn layout_metrics_cache_invalidates_when_a_child_document_root_is_detached() {
    let mut runtime = runtime(
        "<html><body><iframe id='frame' style='width:120px;height:80px'></iframe></body></html>",
    );
    runtime.eval("globalThis.frame=document.getElementById('frame'); globalThis.childRoot=frame.contentDocument.documentElement;").unwrap();
    runtime.run_until_idle().unwrap();
    let width = number(&mut runtime, "childRoot.clientWidth");
    assert!(width > 0.0);
    runtime.eval("childRoot.remove();").unwrap();
    assert_eq!(number(&mut runtime, "childRoot.clientWidth"), 0.0);
}

#[test]
fn inline_form_controls_expose_painted_border_boxes_and_invalidate_when_hidden() {
    for tag in ["input", "textarea", "select", "button"] {
        let mut runtime = runtime(&format!(
            "<html><body><div><{tag} id='editor' style='width:80px;height:20px;padding:3px;border:2px solid;box-sizing:content-box'>Text</{tag}></div></body></html>"
        ));
        runtime.set_viewport(320.0, 240.0);
        runtime
            .eval("globalThis.editor=document.getElementById('editor')")
            .unwrap();
        assert_eq!(
            number(&mut runtime, "editor.getBoundingClientRect().width"),
            90.0,
            "{tag}"
        );
        assert_eq!(
            number(&mut runtime, "editor.getBoundingClientRect().height"),
            30.0,
            "{tag}"
        );
        assert_eq!(number(&mut runtime, "editor.offsetWidth"), 90.0);
        assert_eq!(number(&mut runtime, "editor.clientWidth"), 86.0);
        assert_eq!(number(&mut runtime, "editor.getClientRects().length"), 1.0);
        runtime.eval("globalThis.original=editor.getBoundingClientRect(); editor.parentElement.style.transform='translate(11px,13px)'").unwrap();
        assert_eq!(
            number(&mut runtime, "editor.getBoundingClientRect().x-original.x"),
            11.0
        );
        assert_eq!(
            number(&mut runtime, "editor.getBoundingClientRect().y-original.y"),
            13.0
        );
        runtime.eval("editor.style.display='none'").unwrap();
        assert_eq!(
            number(&mut runtime, "editor.getBoundingClientRect().width"),
            0.0
        );
        assert_eq!(number(&mut runtime, "editor.getClientRects().length"), 0.0);
    }
}
