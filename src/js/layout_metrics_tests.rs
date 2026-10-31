use super::{JsRuntime, TEST_LAYOUT_METRICS_CALLS};
use crate::html::TreeBuilder;

fn runtime(html: &str) -> JsRuntime {
    let runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse(html).document(),
        "https://example.test/index.html",
    )
    .unwrap();
    TEST_LAYOUT_METRICS_CALLS.with(|count| count.set(0));
    runtime
}

fn metric_calls() -> usize {
    TEST_LAYOUT_METRICS_CALLS.with(std::cell::Cell::get)
}

fn number(runtime: &mut JsRuntime, source: &str) -> f64 {
    runtime.eval(source).unwrap().as_number().unwrap()
}

#[test]
fn zoom_scales_client_rects_and_hit_testing_but_not_other_cssom_geometry() {
    let mut runtime = runtime(
        "<html><head><style>html,body{margin:0}#outer{zoom:200%;width:50px;height:30px;overflow:auto;background:red}#inner{zoom:2;width:10px;height:5px;background:blue}#zero{zoom:0;width:7px;height:3px}#wide{zoom:2;width:30px;height:1px}</style></head><body><div id='outer'><div id='inner'></div><div id='zero'></div><div id='wide'></div></div></body></html>",
    );
    runtime.set_viewport(320.0, 200.0);
    runtime
        .eval("globalThis.outer=document.getElementById('outer');globalThis.inner=document.getElementById('inner');globalThis.zero=document.getElementById('zero')")
        .unwrap();

    assert_eq!(
        number(&mut runtime, "parseFloat(getComputedStyle(outer).zoom)"),
        2.0
    );
    assert_eq!(
        number(&mut runtime, "parseFloat(getComputedStyle(inner).zoom)"),
        2.0
    );
    assert_eq!(
        number(&mut runtime, "parseFloat(getComputedStyle(zero).zoom)"),
        1.0
    );
    assert_eq!(
        number(&mut runtime, "parseFloat(getComputedStyle(inner).width)"),
        10.0
    );
    assert_eq!(
        number(&mut runtime, "outer.getBoundingClientRect().width"),
        100.0
    );
    assert_eq!(
        number(&mut runtime, "inner.getBoundingClientRect().width"),
        40.0
    );
    assert_eq!(
        number(&mut runtime, "zero.getBoundingClientRect().width"),
        14.0
    );
    assert_eq!(number(&mut runtime, "outer.offsetWidth"), 50.0);
    assert_eq!(number(&mut runtime, "inner.offsetWidth"), 10.0);
    assert_eq!(number(&mut runtime, "zero.offsetWidth"), 7.0);
    assert_eq!(number(&mut runtime, "outer.clientWidth"), 50.0);
    assert_eq!(number(&mut runtime, "inner.clientWidth"), 10.0);
    assert_eq!(number(&mut runtime, "outer.scrollWidth"), 60.0);
    assert!(
        runtime
            .eval("CSS.supports('zoom','150%') && !CSS.supports('zoom','-1')")
            .unwrap()
            .to_boolean()
    );
    assert_eq!(
        runtime
            .hit_test(30.0, 5.0)
            .and_then(|node| node.get_attribute("id"))
            .as_deref(),
        Some("inner"),
    );
    assert_ne!(
        runtime
            .hit_test(45.0, 5.0)
            .and_then(|node| node.get_attribute("id"))
            .as_deref(),
        Some("inner"),
    );
    runtime
        .eval("inner.style.zoom='3';inner.style.translate='5px 0';inner.style.transform='translateX(5px)'")
        .unwrap();
    assert_eq!(
        number(&mut runtime, "inner.getBoundingClientRect().width"),
        60.0
    );
    assert_eq!(
        number(&mut runtime, "inner.getBoundingClientRect().x"),
        60.0
    );
    assert_eq!(number(&mut runtime, "inner.offsetWidth"), 10.0);
}

#[test]
fn document_point_query_matches_pointer_hit_test_after_style_change() {
    let mut runtime = runtime(
        "<html><head><style>html,body{margin:0}#back,#front{position:absolute;left:10px;top:10px;width:60px;height:60px}#back{background:red}#front{background:blue}</style></head><body><div id='back'></div><div id='front'></div></body></html>",
    );
    runtime.set_viewport(160.0, 120.0);
    assert_eq!(
        runtime
            .hit_test(20.0, 20.0)
            .and_then(|node| node.get_attribute("id"))
            .as_deref(),
        Some("front"),
    );
    assert!(
        runtime
            .eval("document.elementFromPoint(20,20) === document.getElementById('front')")
            .unwrap()
            .to_boolean()
    );
    runtime
        .eval("document.getElementById('front').style.pointerEvents='none'")
        .unwrap();
    assert_eq!(
        runtime
            .hit_test(20.0, 20.0)
            .and_then(|node| node.get_attribute("id"))
            .as_deref(),
        Some("back"),
    );
    assert!(
        runtime
            .eval("document.elementFromPoint(20,20) === document.getElementById('back')")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn inline_block_uses_its_internal_line_box_for_border_box_geometry() {
    let mut runtime = runtime(
        "<html><head><style>html,body{margin:0}body{padding:24px}#before{font:10px/10px monospace}#target{display:inline-block;font:40px/50px monospace;padding:8px;border:2px solid}</style></head><body><span id='before'>A</span><span id='target'>ABBA BAAB</span></body></html>",
    );
    runtime.set_viewport(800.0, 300.0);
    runtime
        .eval("globalThis.target=document.getElementById('target');globalThis.before=document.getElementById('before')")
        .unwrap();

    assert_eq!(
        number(&mut runtime, "target.getBoundingClientRect().height"),
        70.0
    );
    assert_eq!(number(&mut runtime, "target.offsetHeight"), 70.0);
    assert_eq!(number(&mut runtime, "target.clientHeight"), 66.0);
    assert_eq!(number(&mut runtime, "target.getClientRects().length"), 1.0);
    assert_eq!(
        number(&mut runtime, "target.getBoundingClientRect().y"),
        24.0
    );
    assert!(
        number(&mut runtime, "target.getBoundingClientRect().x")
            > number(&mut runtime, "before.getBoundingClientRect().x"),
        "the atomic inline-level box must remain on the same line after preceding text",
    );
}

#[test]
fn inline_block_shrink_wraps_and_contains_block_children() {
    let mut runtime = runtime(
        "<html><head><style>html,body{margin:0}#target{display:inline-block;padding:3px;border:1px solid}#inner{display:block;width:40px;height:12px}</style></head><body><span id='target'><span id='inner'></span></span></body></html>",
    );
    runtime.set_viewport(320.0, 200.0);
    runtime
        .eval("globalThis.target=document.getElementById('target');globalThis.inner=document.getElementById('inner')")
        .unwrap();

    assert_eq!(
        number(&mut runtime, "target.getBoundingClientRect().width"),
        48.0
    );
    assert_eq!(
        number(&mut runtime, "target.getBoundingClientRect().height"),
        20.0
    );
    assert_eq!(
        number(
            &mut runtime,
            "inner.getBoundingClientRect().x-target.getBoundingClientRect().x"
        ),
        4.0,
    );
    assert_eq!(
        number(
            &mut runtime,
            "inner.getBoundingClientRect().y-target.getBoundingClientRect().y"
        ),
        4.0,
    );
}

#[test]
fn inline_block_honors_top_and_baseline_alignment() {
    let mut runtime = runtime(
        "<html><head><style>html,body{margin:0}.row{line-height:0}.box{display:inline-block;width:10px}.short{height:10px}.tall{height:30px}.top{vertical-align:top}</style></head><body><div class='row'><span id='baseline-tall' class='box tall'></span><span id='baseline-short' class='box short'></span></div><div class='row'><span id='top-tall' class='box tall top'></span><span id='top-short' class='box short top'></span></div></body></html>",
    );
    runtime.set_viewport(320.0, 200.0);

    let baseline_bottom_delta = number(
        &mut runtime,
        "(()=>{const a=document.getElementById('baseline-tall').getBoundingClientRect(),b=document.getElementById('baseline-short').getBoundingClientRect();return (a.y+a.height)-(b.y+b.height)})()",
    );
    assert_eq!(baseline_bottom_delta, 0.0);
    let top_delta = number(
        &mut runtime,
        "document.getElementById('top-tall').getBoundingClientRect().y-document.getElementById('top-short').getBoundingClientRect().y",
    );
    assert_eq!(top_delta, 0.0);
}

#[test]
fn inline_block_wraps_text_and_clips_its_transformed_subtree() {
    let mut runtime = runtime(
        "<html><head><style>html,body{margin:0}#wrap{display:inline-block;width:30px;font:10px/12px monospace;word-break:break-all}#clip{display:inline-block;width:30px;height:20px;overflow:hidden;transform:translate(10px,40px)}#inner{display:block;width:60px;height:20px}</style></head><body><span id='wrap'>ABCDEFGHIJKL</span><span id='clip'><span id='inner'></span></span></body></html>",
    );
    runtime.set_viewport(320.0, 200.0);

    assert!(
        number(
            &mut runtime,
            "document.getElementById('wrap').getBoundingClientRect().height",
        ) >= 24.0,
        "text must wrap inside the inline-block's independent formatting context",
    );
    let clip_x = number(
        &mut runtime,
        "document.getElementById('clip').getBoundingClientRect().x",
    );
    let clip_y = number(
        &mut runtime,
        "document.getElementById('clip').getBoundingClientRect().y",
    );
    assert_eq!(clip_x, 40.0);
    assert_eq!(
        runtime
            .hit_test(clip_x as f32 + 5.0, clip_y as f32 + 5.0)
            .and_then(|node| node.get_attribute("id"))
            .as_deref(),
        Some("inner"),
    );
    assert_ne!(
        runtime
            .hit_test(clip_x as f32 + 35.0, clip_y as f32 + 5.0)
            .and_then(|node| node.get_attribute("id"))
            .as_deref(),
        Some("inner"),
    );
}

#[test]
fn layout_metrics_cache_shares_geometry_for_one_element_and_keeps_rects_independent() {
    let mut runtime = runtime(
        "<html><body><div id='target' style='width:100px;height:20px'></div><div id='other' style='width:60px'></div></body></html>",
    );
    runtime.eval("globalThis.target = document.getElementById('target'); globalThis.other = document.getElementById('other'); for (let i=0;i<100;i++) { target.offsetWidth; target.offsetHeight; target.clientWidth; target.scrollHeight; target.getBoundingClientRect(); target.getClientRects(); }").unwrap();
    assert_eq!(metric_calls(), 1);
    assert_eq!(number(&mut runtime, "other.offsetWidth"), 60.0);
    assert_eq!(metric_calls(), 2);
    let independent = runtime
        .eval(
            r#"(() => {
        target.getBoundingClientRect().width = -1;
        const rects = target.getClientRects();
        rects[0].width = -1;
        if (!(rects instanceof DOMRectList) || typeof rects.push !== 'undefined') return false;
        try { Array.prototype.push.call(rects, {width:-1}); return false; }
        catch (error) { if (!(error instanceof TypeError)) return false; }
        return rects.length === 1 && rects[0].width === -1;
    })()"#,
        )
        .unwrap();
    assert_eq!(independent.as_boolean(), Some(true));
    assert_eq!(
        number(&mut runtime, "target.getClientRects()[0].width"),
        100.0
    );
    assert_eq!(number(&mut runtime, "target.getClientRects().length"), 1.0);
    assert_eq!(number(&mut runtime, "target.offsetWidth"), 100.0);
    assert_eq!(metric_calls(), 2);
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
    assert_eq!(metric_calls(), 3);
    runtime
        .eval("target.remove(); target.style.width='220px';")
        .unwrap();
    assert_eq!(number(&mut runtime, "target.offsetWidth"), 0.0);
    runtime.eval("document.body.appendChild(target);").unwrap();
    assert_eq!(number(&mut runtime, "target.offsetWidth"), 220.0);
    assert_eq!(number(&mut runtime, "target.clientWidth"), 220.0);
    assert_eq!(metric_calls(), 5);
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

#[test]
fn visible_background_image_cssom_change_rebuilds_layout_and_refreshes_inheritance() {
    let mut runtime = runtime(
        r#"<html><head><style>
          html,body{margin:0}#target{width:100px;height:20px}
          #child{background-image:inherit}
          #target::before{content:'cached';background-image:inherit}
        </style></head><body>
          <div id="target" style="background-image:none"><span id="child"></span></div>
        </body></html>"#,
    );
    runtime.set_viewport(320.0, 200.0);
    assert!(
        runtime
            .eval(
                r#"globalThis.target=document.getElementById('target');
                   globalThis.child=document.getElementById('child');
                   target.getBoundingClientRect().width===100&&
                   getComputedStyle(child).backgroundImage==='none'&&
                   getComputedStyle(target,'::before').backgroundImage==='none'"#,
            )
            .unwrap()
            .to_boolean(),
    );
    let layout_generation = runtime.render_generations().layout;

    runtime
        .eval("target.style.backgroundImage='linear-gradient(red, blue)'")
        .unwrap();
    assert!(
        runtime
            .eval(
                r#"(() => {
                  const targetImage=getComputedStyle(target).backgroundImage;
                  return target.style.backgroundImage.includes('linear-gradient')&&
                    targetImage.includes('linear-gradient')&&
                    getComputedStyle(child).backgroundImage===targetImage&&
                    getComputedStyle(target,'::before').backgroundImage===targetImage&&
                    target.getBoundingClientRect().width===100;
                })()"#,
            )
            .unwrap()
            .to_boolean(),
    );
    assert!(runtime.render_generations().layout > layout_generation);
}

#[test]
fn localized_inline_style_cache_keeps_geometry_cssom_and_container_queries_fresh() {
    let mut runtime = runtime(
        "<html><head><style>html,body{margin:0}#target{position:relative;width:100px;height:20px;container-type:inline-size}#inside{height:5px}@container (min-width:80px){#inside{color:red}}</style></head><body><div id='target'><span id='inside'></span></div><aside id='unrelated'></aside></body></html>",
    );
    runtime.set_viewport(320.0, 200.0);
    assert!(runtime.eval("globalThis.target=document.getElementById('target');globalThis.inside=document.getElementById('inside');globalThis.before=target.getBoundingClientRect();getComputedStyle(inside).color==='rgb(255, 0, 0)'").unwrap().to_boolean());
    let layout_generation = runtime.render_generations().layout;

    runtime
        .eval("target.style.transform='translateX(20px)';target.style.bottom='5px'")
        .unwrap();
    assert!(runtime.eval("(()=>{const style=getComputedStyle(target),rect=target.getBoundingClientRect();return style.transform!=='none'&&style.bottom==='5px'&&rect.x-before.x===20&&rect.y-before.y===-5&&getComputedStyle(inside).color==='rgb(255, 0, 0)'})()").unwrap().to_boolean());
    assert!(runtime.render_generations().layout > layout_generation);
}

#[test]
fn hidden_subject_raw_style_keeps_geometry_and_shared_layout_snapshots() {
    let mut runtime = runtime(
        r#"<html><head><style>
          html,body{margin:0}body{counter-reset:item 0}
          #target{display:none}#child{color:var(--tone,black)}
          #target[style*="20px"]{--tone:red;counter-reset:item 7;container-type:inline-size}
          #container{container-type:inline-size;width:100px}#consumer{color:black}
          @container (min-width:80px){#consumer{color:green}}
          #peer{width:80px;height:20px}#peer::before{content:counter(item)}
        </style></head><body>
          <div id="target" style="transform:translateX(0px)"><span id="child"></span></div>
          <section id="container"><span id="consumer"></span></section><div id="peer"></div>
        </body></html>"#,
    );
    runtime.set_viewport(320.0, 200.0);
    assert!(
        runtime
            .eval(
                r#"(() => {
                  globalThis.target=document.getElementById('target');
                  globalThis.child=document.getElementById('child');
                  globalThis.peer=document.getElementById('peer');
                  globalThis.beforePeer=peer.getBoundingClientRect();
                  const hidden=target.getBoundingClientRect();
                  return hidden.width===0&&hidden.height===0&&beforePeer.width===80&&
                    getComputedStyle(child).color==='rgb(0, 0, 0)'&&
                    getComputedStyle(document.getElementById('consumer')).color==='rgb(0, 128, 0)';
                })()"#,
            )
            .unwrap()
            .to_boolean(),
    );
    let generations = runtime.render_generations();
    let calls = metric_calls();
    let resolver_snapshot = |runtime: &JsRuntime| {
        let state = runtime.host_state.borrow();
        let document_id = state.document.identity();
        let peer = state.document.query_selector("#peer").unwrap();
        let resolver = state.document_styles[&document_id]
            .resolver
            .as_ref()
            .unwrap();
        (
            resolver
                .counter_values(&peer, crate::css::PseudoElement::Before, "item")
                .map(|values| values.to_vec()),
            resolver.container_contexts_snapshot(),
        )
    };
    let snapshot = resolver_snapshot(&runtime);
    assert_eq!(snapshot.0, Some(vec![0]));
    assert!(!snapshot.1.is_empty());

    runtime
        .eval("target.style.transform='translateX(20px)'")
        .unwrap();
    let invalidated = runtime.render_generations();
    assert!(invalidated.style > generations.style);
    assert_eq!(invalidated.layout, generations.layout);
    assert!(invalidated.paint > generations.paint);
    let observed = runtime
        .eval(
            r#"(() => {
        const style=getComputedStyle(target),after=peer.getBoundingClientRect();
        const hidden=target.getBoundingClientRect();
        return JSON.stringify({display:style.display,transform:style.transform,
          inlineTransform:target.style.transform,tone:style.getPropertyValue('--tone'),
          childColor:getComputedStyle(child).color,hidden:[hidden.width,hidden.height],
          peer:[after.x,after.y,after.width,after.height],
          before:[beforePeer.x,beforePeer.y,beforePeer.width,beforePeer.height]});
    })()"#,
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    let observed: serde_json::Value = serde_json::from_str(&observed).unwrap();
    assert_eq!(observed["display"], "none", "{observed}");
    assert_eq!(
        observed["inlineTransform"], "translateX(20px)",
        "{observed}"
    );
    assert_eq!(observed["tone"], "red", "{observed}");
    assert_eq!(observed["childColor"], "rgb(255, 0, 0)", "{observed}");
    assert_eq!(observed["hidden"], serde_json::json!([0, 0]), "{observed}");
    assert_eq!(observed["peer"], observed["before"], "{observed}");
    assert_eq!(runtime.render_generations().layout, generations.layout);
    assert!(metric_calls() > calls);
    assert_eq!(resolver_snapshot(&runtime), snapshot);
}

#[test]
fn hidden_background_image_subject_raw_style_keeps_geometry_and_shared_layout_snapshots() {
    let mut runtime = runtime(
        r#"<html><head><style>
          html,body{margin:0}body{counter-reset:item 0}
          #target{display:none}#child{color:var(--tone,black)}
          #target[style*="linear-gradient"]{--tone:red;counter-reset:item 7;container-type:inline-size}
          #container{container-type:inline-size;width:100px}#consumer{color:black}
          @container (min-width:80px){#consumer{color:green}}
          #peer{width:80px;height:20px}#peer::before{content:counter(item)}
        </style></head><body>
          <div id="target" style="background-image:none"><span id="child"></span></div>
          <section id="container"><span id="consumer"></span></section><div id="peer"></div>
        </body></html>"#,
    );
    runtime.set_viewport(320.0, 200.0);
    assert!(
        runtime
            .eval(
                r#"(() => {
                  globalThis.target=document.getElementById('target');
                  globalThis.child=document.getElementById('child');
                  globalThis.peer=document.getElementById('peer');
                  globalThis.beforePeer=peer.getBoundingClientRect();
                  const hidden=target.getBoundingClientRect();
                  return hidden.width===0&&hidden.height===0&&beforePeer.width===80&&
                    getComputedStyle(child).color==='rgb(0, 0, 0)'&&
                    getComputedStyle(document.getElementById('consumer')).color==='rgb(0, 128, 0)';
                })()"#,
            )
            .unwrap()
            .to_boolean(),
    );
    let generations = runtime.render_generations();
    let calls = metric_calls();
    let resolver_snapshot = |runtime: &JsRuntime| {
        let state = runtime.host_state.borrow();
        let document_id = state.document.identity();
        let peer = state.document.query_selector("#peer").unwrap();
        let resolver = state.document_styles[&document_id]
            .resolver
            .as_ref()
            .unwrap();
        (
            resolver
                .counter_values(&peer, crate::css::PseudoElement::Before, "item")
                .map(|values| values.to_vec()),
            resolver.container_contexts_snapshot(),
        )
    };
    let snapshot = resolver_snapshot(&runtime);
    assert_eq!(snapshot.0, Some(vec![0]));
    assert!(!snapshot.1.is_empty());

    runtime
        .eval("target.style.backgroundImage='linear-gradient(red, blue)'")
        .unwrap();
    let invalidated = runtime.render_generations();
    assert!(invalidated.style > generations.style);
    assert_eq!(invalidated.layout, generations.layout);
    assert!(invalidated.paint > generations.paint);
    let observed = runtime
        .eval(
            r#"(() => {
        const style=getComputedStyle(target),after=peer.getBoundingClientRect();
        const hidden=target.getBoundingClientRect();
        return JSON.stringify({display:style.display,backgroundImage:style.backgroundImage,
          inlineBackgroundImage:target.style.backgroundImage,tone:style.getPropertyValue('--tone'),
          childColor:getComputedStyle(child).color,hidden:[hidden.width,hidden.height],
          peer:[after.x,after.y,after.width,after.height],
          before:[beforePeer.x,beforePeer.y,beforePeer.width,beforePeer.height]});
    })()"#,
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    let observed: serde_json::Value = serde_json::from_str(&observed).unwrap();
    assert_eq!(observed["display"], "none", "{observed}");
    assert!(
        observed["inlineBackgroundImage"]
            .as_str()
            .is_some_and(|value| value.contains("linear-gradient")),
        "{observed}",
    );
    assert!(
        observed["backgroundImage"]
            .as_str()
            .is_some_and(|value| value.contains("linear-gradient")),
        "{observed}",
    );
    assert_eq!(observed["tone"], "red", "{observed}");
    assert_eq!(observed["childColor"], "rgb(255, 0, 0)", "{observed}");
    assert_eq!(observed["hidden"], serde_json::json!([0, 0]), "{observed}");
    assert_eq!(observed["peer"], observed["before"], "{observed}");
    assert_eq!(runtime.render_generations().layout, generations.layout);
    assert!(metric_calls() > calls);
    assert_eq!(resolver_snapshot(&runtime), snapshot);
}

#[test]
fn hidden_subject_raw_style_that_reveals_target_rebuilds_peer_flow() {
    let mut runtime = runtime(
        r#"<html><head><style>
          html,body{margin:0}#target{display:block;height:30px}
          #target[style*="0px"]{display:none}#peer{height:10px}
        </style></head><body><div id="target" style="transform:translateX(0px)"></div><div id="peer"></div></body></html>"#,
    );
    runtime.set_viewport(320.0, 200.0);
    assert!(
        runtime
            .eval(
                "globalThis.target=document.getElementById('target');globalThis.peer=document.getElementById('peer');target.getBoundingClientRect().height===0&&peer.getBoundingClientRect().y===0",
            )
            .unwrap()
            .to_boolean(),
    );
    let layout_generation = runtime.render_generations().layout;

    runtime
        .eval("target.style.transform='translateX(1px)'")
        .unwrap();
    assert!(
        runtime
            .eval(
                "getComputedStyle(target).display!=='none'&&target.getBoundingClientRect().height===30&&peer.getBoundingClientRect().y===30",
            )
            .unwrap()
            .to_boolean(),
    );
    assert!(runtime.render_generations().layout > layout_generation);
}

#[test]
fn hidden_background_image_subject_raw_style_that_reveals_target_rebuilds_peer_flow() {
    let mut runtime = runtime(
        r#"<html><head><style>
          html,body{margin:0}#target{display:block;height:30px}
          #target[style*="background-image: none"]{display:none}#peer{height:10px}
        </style></head><body><div id="target" style="background-image: none"></div><div id="peer"></div></body></html>"#,
    );
    runtime.set_viewport(320.0, 200.0);
    assert!(
        runtime
            .eval(
                "globalThis.target=document.getElementById('target');globalThis.peer=document.getElementById('peer');target.getBoundingClientRect().height===0&&peer.getBoundingClientRect().y===0",
            )
            .unwrap()
            .to_boolean(),
    );
    let layout_generation = runtime.render_generations().layout;

    runtime
        .eval("target.style.backgroundImage='linear-gradient(red, blue)'")
        .unwrap();
    assert!(
        runtime
            .eval(
                "getComputedStyle(target).display!=='none'&&target.getBoundingClientRect().height===30&&peer.getBoundingClientRect().y===30",
            )
            .unwrap()
            .to_boolean(),
    );
    assert!(runtime.render_generations().layout > layout_generation);
}

#[test]
fn hidden_nonlocal_raw_selector_rebuilds_peer_geometry() {
    let mut runtime = runtime(
        r#"<html><head><style>
          html,body{margin:0}#target{display:none}#peer{width:20px;height:10px}
          body:has(#target[style]) #peer{width:80px}
        </style></head><body><div id="target"></div><div id="peer"></div></body></html>"#,
    );
    runtime.set_viewport(320.0, 200.0);
    assert_eq!(
        number(
            &mut runtime,
            "globalThis.target=document.getElementById('target');document.getElementById('peer').getBoundingClientRect().width",
        ),
        20.0,
    );
    let layout_generation = runtime.render_generations().layout;

    runtime
        .eval("target.style.transform='translateX(1px)'")
        .unwrap();
    assert_eq!(
        number(
            &mut runtime,
            "document.getElementById('peer').getBoundingClientRect().width",
        ),
        80.0,
    );
    assert!(runtime.render_generations().layout > layout_generation);
}

#[test]
fn hidden_background_image_nonlocal_raw_selector_rebuilds_peer_geometry() {
    let mut runtime = runtime(
        r#"<html><head><style>
          html,body{margin:0}#target{display:none}#peer{width:20px;height:10px}
          body:has(#target[style]) #peer{width:80px}
        </style></head><body><div id="target"></div><div id="peer"></div></body></html>"#,
    );
    runtime.set_viewport(320.0, 200.0);
    assert_eq!(
        number(
            &mut runtime,
            "globalThis.target=document.getElementById('target');document.getElementById('peer').getBoundingClientRect().width",
        ),
        20.0,
    );
    let layout_generation = runtime.render_generations().layout;

    runtime
        .eval("target.style.backgroundImage='linear-gradient(red, blue)'")
        .unwrap();
    assert_eq!(
        number(
            &mut runtime,
            "document.getElementById('peer').getBoundingClientRect().width",
        ),
        80.0,
    );
    assert!(runtime.render_generations().layout > layout_generation);
}

#[test]
fn hidden_subtree_with_top_layer_box_rebuilds_geometry() {
    let mut runtime = runtime(
        r#"<html><head><style>
          html,body{margin:0}#target{display:none;--popover-width:20px}
          #target[style*="20px"]{--popover-width:40px}
          #popover{position:fixed;left:0;top:0;box-sizing:border-box;margin:0;padding:0;border:0;width:var(--popover-width);height:10px}
        </style></head><body><div id="target" style="transform:translateX(0px)"><div id="popover" popover="manual"></div></div></body></html>"#,
    );
    runtime.set_viewport(320.0, 200.0);
    assert!(
        runtime
            .eval(
                r#"globalThis.target=document.getElementById('target');
                   globalThis.popover=document.getElementById('popover');
                   popover.showPopover();
                   target.getBoundingClientRect().width===0&&popover.getBoundingClientRect().width===20"#,
            )
            .unwrap()
            .to_boolean(),
    );
    let layout_generation = runtime.render_generations().layout;

    runtime
        .eval("target.style.transform='translateX(20px)'")
        .unwrap();
    assert!(
        runtime
            .eval(
                "getComputedStyle(target).display==='none'&&target.getBoundingClientRect().width===0&&popover.getBoundingClientRect().width===40",
            )
            .unwrap()
            .to_boolean(),
    );
    assert!(runtime.render_generations().layout > layout_generation);
}

#[test]
fn hidden_local_change_that_starts_transition_rebuilds_geometry() {
    let mut runtime = runtime(
        r#"<html><head><style>#target{display:none}</style></head><body><div id="target" style="transform:translateX(0px);transition:transform 1s linear"></div></body></html>"#,
    );
    assert_eq!(
        number(
            &mut runtime,
            "globalThis.target=document.getElementById('target');target.getBoundingClientRect().width",
        ),
        0.0,
    );
    let layout_generation = runtime.render_generations().layout;

    runtime
        .eval("target.style.transform='translateX(20px)'")
        .unwrap();
    let transition_started = {
        let state = runtime.host_state.borrow();
        let document_id = state.document.identity();
        state.document_styles[&document_id]
            .resolver
            .as_ref()
            .is_some_and(|resolver| resolver.has_running_transitions())
    };
    assert!(transition_started);
    assert_eq!(
        number(&mut runtime, "target.getBoundingClientRect().width"),
        0.0
    );
    assert!(runtime.render_generations().layout > layout_generation);
}

#[test]
#[ignore = "diagnostic A/B timing fixture; run explicitly with --ignored --nocapture"]
fn profile_hidden_subtree_geometry_retention_against_full_rebuild() {
    use std::time::{Duration, Instant};

    const SIBLINGS: usize = 6_000;
    const WARMUPS: usize = 2;
    const SAMPLES: usize = 9;

    fn fixture() -> JsRuntime {
        let mut html = String::from(
            r#"<html><head><style>
              html,body{margin:0}.row{height:1px;width:100px}
              #target{display:none}#target[style*="20px"]{--probe:1;counter-reset:item 7;container-type:inline-size}
            </style></head><body><div id="target" style="transform:translateX(0px)"></div>"#,
        );
        for index in 0..SIBLINGS {
            html.push_str(&format!(
                "<div class='row' data-bucket='{}'></div>",
                index % 32
            ));
        }
        html.push_str("<div id='probe' class='row'></div></body></html>");
        let mut runtime = runtime(&html);
        runtime.set_viewport(320.0, 200.0);
        assert!(
            runtime
                .eval(
                    "globalThis.target=document.getElementById('target');globalThis.probe=document.getElementById('probe');target.getBoundingClientRect().width===0&&probe.getBoundingClientRect().width===100",
                )
                .unwrap()
                .to_boolean(),
        );
        runtime
    }

    fn measure(runtime: &mut JsRuntime, force_full_layout: bool, step: usize) -> Duration {
        let generations = runtime.render_generations();
        let transform = if step % 2 == 0 { 20 } else { 0 };
        let started = Instant::now();
        runtime
            .eval(&format!(
                "target.style.transform='translateX({transform}px)'"
            ))
            .unwrap();
        if force_full_layout {
            let mut state = runtime.host_state.borrow_mut();
            state.capture_scroll_offsets_before_layout();
            state.invalidate_layout_root("hidden_geometry_profile_full_rebuild");
        }
        assert_eq!(
            number(runtime, "probe.getBoundingClientRect().width"),
            100.0
        );
        let elapsed = started.elapsed();
        if force_full_layout {
            assert!(runtime.render_generations().layout > generations.layout);
        } else {
            assert_eq!(runtime.render_generations().layout, generations.layout);
        }
        elapsed
    }

    fn median(mut samples: Vec<Duration>) -> Duration {
        samples.sort_unstable();
        samples[samples.len() / 2]
    }

    let mut retained = fixture();
    let mut rebuilt = fixture();
    let mut retained_samples = Vec::with_capacity(SAMPLES);
    let mut rebuilt_samples = Vec::with_capacity(SAMPLES);
    for sample in 0..WARMUPS + SAMPLES {
        let (retained_elapsed, rebuilt_elapsed) = if sample % 2 == 0 {
            (
                measure(&mut retained, false, sample),
                measure(&mut rebuilt, true, sample),
            )
        } else {
            let rebuilt_elapsed = measure(&mut rebuilt, true, sample);
            let retained_elapsed = measure(&mut retained, false, sample);
            (retained_elapsed, rebuilt_elapsed)
        };
        if sample >= WARMUPS {
            retained_samples.push(retained_elapsed);
            rebuilt_samples.push(rebuilt_elapsed);
        }
    }

    let retained_median = median(retained_samples);
    let rebuilt_median = median(rebuilt_samples);
    eprintln!(
        "hidden geometry A/B: siblings={SIBLINGS} samples={SAMPLES} retained_median_ms={:.3} rebuilt_median_ms={:.3} ratio={:.3}",
        retained_median.as_secs_f64() * 1_000.0,
        rebuilt_median.as_secs_f64() * 1_000.0,
        retained_median.as_secs_f64() / rebuilt_median.as_secs_f64(),
    );
}

#[test]
fn inline_style_hash_change_recomputes_the_whole_blocked_declaration_set() {
    use base64::Engine as _;
    use sha2::Digest as _;

    let previous = "display:none;counter-reset:item 7;--tone:red;transform:translateX(0px)";
    let current = "display:none;counter-reset:item 7;--tone:red;transform:translateX(1px)";
    let digest =
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(previous.as_bytes()));
    let mut runtime = runtime(&format!(
        "<html><head><style>#child{{color:var(--tone, black)}}</style></head><body><div id='target' style='{previous}'><span id='child'></span></div><aside id='unrelated'></aside></body></html>",
    ));
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        format!("style-src-attr 'unsafe-hashes' 'sha256-{digest}'"),
    )]);
    assert!(
        runtime
            .eval(
                "globalThis.target=document.getElementById('target');globalThis.child=document.getElementById('child');getComputedStyle(target).display==='none'&&getComputedStyle(child).color==='rgb(255, 0, 0)'",
            )
            .unwrap()
            .to_boolean(),
    );

    runtime
        .eval(&format!("target.setAttribute('style','{current}')"))
        .unwrap();
    assert!(
        runtime
            .eval(
                "getComputedStyle(target).display!=='none'&&getComputedStyle(child).color==='rgb(0, 0, 0)'",
            )
            .unwrap()
            .to_boolean(),
        "a transform-only source edit that misses the CSP hash blocks every inline declaration",
    );
}
