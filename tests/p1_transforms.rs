use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn check(html: &str, script: &str) {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(html).document()).unwrap();
    assert_script(&mut runtime, script);
}

fn assert_script(runtime: &mut JsRuntime, script: &str) {
    let result = runtime.eval(script).unwrap().as_boolean();
    let details = if result != Some(true) {
        runtime.eval("JSON.stringify(Array.from(document.querySelectorAll('[id]')).map(n => {const r=n.getBoundingClientRect(),s=getComputedStyle(n);return {id:n.id,x:r.left,y:r.top,width:r.width,height:r.height,translate:s.translate,rotate:s.rotate,scale:s.scale,animation:s.animation,offsetParent:n.offsetParent&&n.offsetParent.id};}))").ok().and_then(|value| value.as_string().map(|text| text.to_std_string_escaped()))
    } else {
        None
    };
    assert_eq!(result, Some(true), "{script}: {details:?}");
}

#[test]
fn individual_translation_changes_geometry_and_hit_testing() {
    check(
        "<style>body{margin:0}#item{width:10px;height:10px;translate:30px 20px;background:red}</style><div id='item'></div>",
        "(() => {const r=document.getElementById('item').getBoundingClientRect();return r.left===30 && r.top===20 && document.elementFromPoint(35,25).id==='item';})()",
    );
}

#[test]
fn individual_transform_values_validate_and_serialize() {
    check(
        "<div id='item' style='translate:2em 25%;rotate:.25turn;scale:50% 200%'></div>",
        "(() => {const s=getComputedStyle(document.getElementById('item'));return s.translate==='32px 25%' && s.rotate==='90deg' && s.scale==='0.5 2' && CSS.supports('translate','1px 2% 3px') && CSS.supports('rotate','1 2 3 40deg') && CSS.supports('rotate','x 40deg') && CSS.supports('scale','1 2 3') && !CSS.supports('translate','1px 2px 3%') && !CSS.supports('rotate','1 2 40deg') && !CSS.supports('scale','1px');})()",
    );
}

#[test]
fn individual_transforms_compose_before_the_transform_list() {
    check(
        "<style>body{margin:0}#item{width:10px;height:20px;transform-origin:0 0;translate:100px 40px;rotate:90deg;scale:2 3;transform:translateX(10px)}</style><div id='item'></div>",
        "(() => {const r=document.getElementById('item').getBoundingClientRect();return Math.abs(r.left-40)<0.0001 && Math.abs(r.top-60)<0.0001 && Math.abs(r.width-60)<0.0001 && Math.abs(r.height-20)<0.0001;})()",
    );
}

#[test]
fn parent_perspective_projects_individual_depth_for_geometry_and_paint() {
    use omoikane::layout::Rect;
    use omoikane::paint::{Color, render_document};
    let html = "<style>body{margin:0}#parent{width:100px;height:100px;perspective:200px;perspective-origin:0 0}#item{width:10px;height:10px;translate:0 0 100px;background:red}</style><div id='parent'><div id='item'></div></div>";
    let document = TreeBuilder::parse(html).document();
    let mut runtime = JsRuntime::with_document(document.clone()).unwrap();
    assert_script(
        &mut runtime,
        "(() => {const r=document.getElementById('item').getBoundingClientRect();return r.width===20 && r.height===20 && document.elementFromPoint(15,15).id==='item';})()",
    );
    let image = render_document(
        &document,
        Rect {
            width: 100.0,
            height: 100.0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(image.pixel(15, 15), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn none_and_identity_have_distinct_containing_blocks_and_stacking_contexts() {
    use omoikane::layout::Rect;
    use omoikane::paint::{Color, render_document};
    for value in ["none", "0px"] {
        let identity = value != "none";
        check(
            &format!(
                "<style>body{{margin:0}}#parent{{margin-left:40px;margin-top:30px;width:100px;height:50px;translate:{value}}}#fixed{{position:fixed;left:10px;top:10px;width:10px;height:10px}}#absolute{{position:absolute}}</style><div id='parent'><div id='fixed'></div><div id='absolute'></div></div>"
            ),
            &format!(
                "(() => {{const r=document.getElementById('fixed').getBoundingClientRect();return r.left==={} && r.top==={} && (document.getElementById('absolute').offsetParent.id==='parent')==={identity};}})()",
                if identity { 50 } else { 10 },
                if identity { 40 } else { 10 }
            ),
        );
        let html = format!(
            "<style>body{{margin:0}}#parent{{width:20px;height:20px;translate:{value}}}#child{{position:relative;z-index:999;width:20px;height:20px;background:red}}#sibling{{position:relative;z-index:1;margin-top:-20px;width:20px;height:20px;background:blue}}</style><div id='parent'><div id='child'></div></div><div id='sibling'></div>"
        );
        let document = TreeBuilder::parse(&html).document();
        let image = render_document(
            &document,
            Rect {
                width: 40.0,
                height: 40.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            image.pixel(10, 10),
            Some(if identity {
                Color::rgb(0, 0, 255)
            } else {
                Color::rgb(255, 0, 0)
            })
        );
        check(
            &html,
            &format!(
                "document.elementFromPoint(10,10).id==='{}'",
                if identity { "sibling" } else { "child" }
            ),
        );
    }
}

#[test]
fn individual_transitions_interpolate_none_and_update_geometry() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse("<style>body{margin:0}#item{width:10px;height:20px;transform-origin:0 0;transition:translate 1s linear, rotate 1s linear, scale 1s linear}</style><div id='item'></div>").document()).unwrap();
    runtime
        .eval("globalThis.item=document.getElementById('item'); getComputedStyle(item).translate")
        .unwrap();
    runtime.eval("item.style.translate='20px 10px';item.style.rotate='90deg';item.style.scale='2';getComputedStyle(item).translate").unwrap();
    runtime.run_animation_frame(500).unwrap();
    assert_script(
        &mut runtime,
        "(() => {const s=getComputedStyle(item),r=item.getBoundingClientRect();return s.translate==='10px 5px' && s.rotate==='45deg' && s.scale==='1.5' && r.width===Math.round(45*Math.SQRT1_2*1000)/1000;})()",
    );
    runtime.run_animation_frame(500).unwrap();
    assert_script(
        &mut runtime,
        "getComputedStyle(item).translate==='20px 10px' && getComputedStyle(item).rotate==='90deg' && getComputedStyle(item).scale==='2'",
    );
}

#[test]
fn keyframes_and_cross_axis_rotation_use_individual_interpolation() {
    check(
        "<style>@keyframes move{from{translate:none;rotate:none;scale:none}to{translate:20px;rotate:x 90deg;scale:3}}#item{animation:move 1s linear -0.5s paused both}</style><div id='item'></div>",
        "(() => {const s=getComputedStyle(document.getElementById('item'));return s.translate==='10px' && s.rotate==='x 45deg' && s.scale==='2';})()",
    );
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse("<style>body{margin:0}#item{width:10px;height:20px;transform-origin:0 0;rotate:x 90deg;transition:rotate 1s linear}</style><div id='item'></div>").document()).unwrap();
    runtime
        .eval("globalThis.item=document.getElementById('item');getComputedStyle(item).rotate")
        .unwrap();
    runtime
        .eval("item.style.rotate='y 90deg';getComputedStyle(item).rotate")
        .unwrap();
    runtime.run_animation_frame(500).unwrap();
    assert_script(
        &mut runtime,
        "(() => {const r=item.getBoundingClientRect();return r.width===Math.round(40/3*1000)/1000 && r.height===Math.round(50/3*1000)/1000;})()",
    );
}

#[test]
fn positioned_descendant_hit_order_preserves_ancestor_clipping() {
    check(
        "<style>body{margin:0}#parent{width:10px;height:10px;overflow:hidden}#child{position:relative;z-index:999;width:20px;height:20px}#sibling{position:relative;z-index:1;margin-top:-10px;width:20px;height:20px}</style><div id='parent'><div id='child'></div></div><div id='sibling'></div>",
        "document.elementFromPoint(5,5).id==='child' && document.elementFromPoint(15,15).id==='sibling' && document.elementsFromPoint(5,5)[0].id==='child'",
    );
}

#[test]
fn singular_depth_scale_preserves_geometry_but_excludes_pointer_targets() {
    for declaration in ["scale:1 1 0", "transform:scaleZ(0)"] {
        check(
            &format!(
                "<style>body{{margin:0}}#item{{width:20px;height:20px;{declaration}}}</style><div id='item'><div id='child' style='width:10px;height:10px'></div></div>"
            ),
            "(() => {const r=document.getElementById('item').getBoundingClientRect();return r.width===20 && r.height===20 && !['item','child'].includes(document.elementFromPoint(5,5).id);})()",
        );
    }
}
