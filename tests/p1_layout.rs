use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn check(html: &str, script: &str) {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(html).document()).unwrap();
    let result = runtime.eval(script).unwrap().as_boolean();
    let details = if result != Some(true) {
        runtime.eval("JSON.stringify(Array.from(document.querySelectorAll('[id]')).map(n => {const r=n.getBoundingClientRect();return {id:n.id,left:r.left,top:r.top,width:r.width,height:r.height};}))").ok().and_then(|value| value.as_string().map(|text| text.to_std_string_escaped()))
    } else {
        None
    };
    assert_eq!(result, Some(true), "{details:?}");
}

#[test]
fn flex_order_changes_both_axes_and_keeps_dom_order() {
    for direction in ["row", "column"] {
        check(
            &format!(
                "<div id='flex' style='display:flex;flex-direction:{direction}'><div id='a' style='width:30px;height:20px'>a</div><div id='b' style='width:30px;height:20px;order:-1'>b</div></div>"
            ),
            &format!(
                "(() => {{ const a=document.getElementById('a'), b=document.getElementById('b'); const axis='{direction}'==='row'?'left':'top'; return b.getBoundingClientRect()[axis] < a.getBoundingClientRect()[axis] && document.getElementById('flex').firstElementChild===a; }})()"
            ),
        );
    }
}

#[test]
fn implicit_grid_tracks_repeat_the_declared_sizes() {
    check(
        "<div style='display:grid;grid-template-columns:50px 50px;grid-auto-rows:20px 30px'><div id='a'></div><div></div><div id='b'></div><div></div><div id='c'></div></div>",
        "(() => { const height=id=>document.getElementById(id).getBoundingClientRect().height; return height('a')===20 && height('b')===30 && height('c')===20; })()",
    );
}

#[test]
fn column_auto_flow_uses_implicit_columns() {
    check(
        "<div style='display:grid;grid-template-rows:20px 20px;grid-auto-flow:column;grid-auto-columns:30px 40px'><div id='a'></div><div id='b'></div><div id='c'></div></div>",
        "(() => { const r=id=>document.getElementById(id).getBoundingClientRect(); return r('a').left===r('b').left && r('b').top-r('a').top===20 && r('c').left-r('a').left===30 && r('c').width===40; })()",
    );
}

#[test]
fn dense_flow_fills_holes_while_sparse_flow_preserves_the_cursor() {
    for (flow, axis, dense) in [
        ("row", "left", false),
        ("row dense", "left", true),
        ("column", "top", false),
        ("column dense", "top", true),
    ] {
        let tracks = if axis == "left" {
            "grid-template-columns:20px 20px 20px;grid-auto-rows:20px"
        } else {
            "grid-template-rows:20px 20px 20px;grid-auto-columns:20px"
        };
        let span = if axis == "left" {
            "grid-column:span 2"
        } else {
            "grid-row:span 2"
        };
        check(
            &format!(
                "<div style='display:grid;{tracks};grid-auto-flow:{flow}'><div id='a' style='{span}'></div><div style='{span}'></div><div id='c'></div></div>"
            ),
            &format!(
                "(() => {{ const r=id=>document.getElementById(id).getBoundingClientRect(); const cross='{axis}'==='left'?'top':'left'; return {} ? r('c')[cross]===r('a')[cross] && r('c')['{axis}']-r('a')['{axis}']===40 : r('c')[cross]-r('a')[cross]===20; }})()",
                dense
            ),
        );
    }
}

#[test]
fn implicit_tracks_before_the_explicit_grid_repeat_backwards() {
    check(
        "<div style='display:grid;grid-template-columns:10px 20px;grid-auto-columns:30px 40px;grid-auto-rows:20px'><div id='a' style='grid-column:-5/-4'></div><div id='b' style='grid-column:-4/-3'></div><div id='c' style='grid-column:1/2'></div></div>",
        "(() => { const r=id=>document.getElementById(id).getBoundingClientRect(); return r('a').width===30 && r('b').width===40 && r('c').width===10 && r('b').left-r('a').left===30 && r('c').left-r('a').left===70; })()",
    );
}

#[test]
fn definite_grid_items_may_overlap_and_end_lines_resolve_spans() {
    check(
        "<div style='display:grid;grid-template-columns:20px 20px 20px;grid-template-rows:20px'><div id='a' style='grid-column:1;grid-row:1'></div><div id='b' style='grid-column:1;grid-row:1'></div><div id='c' style='grid-column:span 2/4;grid-row:1'></div></div>",
        "(() => { const r=id=>document.getElementById(id).getBoundingClientRect(); return r('a').left===r('b').left && r('a').top===r('b').top && r('c').width===40 && r('c').left-r('a').left===20; })()",
    );
}

#[test]
fn ordering_changes_paint_and_hit_testing_with_z_index_precedence() {
    use omoikane::layout::Rect;
    use omoikane::paint::{Color, render_document};
    for elevated in [false, true] {
        let html = format!(
            "<style>body{{margin:0}}#flex{{display:flex}}#a,#b{{width:20px;height:20px;flex-shrink:0}}#a{{order:2;margin-left:-20px;background:red}}#b{{order:1;background:blue;z-index:{}}}</style><div id='flex'><div id='a'></div><div id='b'></div></div>",
            if elevated { "1" } else { "auto" }
        );
        let document = TreeBuilder::parse(&html).document();
        let image = render_document(
            &document,
            Rect {
                width: 40.0,
                height: 40.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(
            image.pixel(10, 10),
            Some(if elevated {
                Color::rgb(0, 0, 255)
            } else {
                Color::rgb(255, 0, 0)
            })
        );
        let mut runtime = JsRuntime::with_document(document).unwrap();
        assert_eq!(
            runtime
                .eval(&format!(
                    "document.elementFromPoint(10,10).id==='{}'",
                    if elevated { "b" } else { "a" }
                ))
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }
}

#[test]
fn computed_grid_and_order_values_validate_and_normalize() {
    check(
        "<div id='a' style='font-size:10px;order:calc(-1.5);grid-auto-flow:dense column;grid-auto-rows:2em 30px;grid-auto-columns:minmax(0px, 1fr) fit-content(20px)'></div><div id='b' style='order:1.5;grid-auto-flow:row column;grid-auto-rows:-1px'></div>",
        "(() => { const a=getComputedStyle(document.getElementById('a')), b=getComputedStyle(document.getElementById('b')); return a.order==='-1' && a.gridAutoFlow==='column dense' && a.gridAutoRows==='20px 30px' && a.gridAutoColumns==='minmax(0px, 1fr) fit-content(20px)' && b.order==='0' && b.gridAutoFlow==='row' && b.gridAutoRows==='auto' && !CSS.supports('order','1.5') && !CSS.supports('grid-auto-flow','dense dense') && !CSS.supports('grid-auto-rows','repeat(2,20px)') && CSS.supports('grid-auto-rows','minmax(0,1fr)'); })()",
    );
}

#[test]
fn reference_fixture_has_expected_geometry_dom_order_and_hit_target() {
    let document = TreeBuilder::parse(include_str!("fixtures/p1-layout/index.html")).document();
    let mut runtime = JsRuntime::with_document(document.clone()).unwrap();
    let probe = include_str!("fixtures/p1-layout/probe.js");
    let value = runtime.eval(&format!("JSON.stringify({probe})")).unwrap();
    let actual: serde_json::Value =
        serde_json::from_str(&value.as_string().unwrap().to_std_string_escaped()).unwrap();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/p1-layout/expected.json")).unwrap();
    assert_eq!(actual, expected);
    if let Some(directory) = std::env::var_os("P1_LAYOUT_OUTPUT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("omoikane.geometry.json"),
            serde_json::to_vec_pretty(&actual).unwrap(),
        )
        .unwrap();
        let image = omoikane::paint::render_document(
            &document,
            omoikane::layout::Rect {
                width: 300.0,
                height: 320.0,
                ..Default::default()
            },
        )
        .unwrap();
        std::fs::write(directory.join("omoikane.original.png"), image.encode_png()).unwrap();
    }
}

#[test]
fn ordered_overlapping_content_items_stretch_and_hit_in_reverse_paint_order() {
    check(
        "<style>#container{width:600px;display:flex}#left{width:300px;overflow:hidden;white-space:nowrap;order:0}#right{width:300px;margin-left:-100px;order:1}</style><div id='container'><div id='right'></div><a id='left' href='#'>foofoofoofoofoofoofoofoofoofoofoofoofoofoofoofoofoofoofoofoofoofoofoofoofoofoo</a></div>",
        "(() => { const p=document.getElementById('container').getBoundingClientRect(), left=document.getElementById('left').getBoundingClientRect(), right=document.getElementById('right').getBoundingClientRect(); return right.height===left.height && document.elementFromPoint(p.left+250,p.top+5).id==='right'; })()",
    );
}

#[test]
fn grid_normal_stretches_only_auto_tracks_in_both_axes() {
    for alignment in ["normal", "stretch", "start"] {
        let expected = if alignment == "start" { 10 } else { 170 };
        check(
            &format!(
                "<div style='display:grid;width:200px;height:200px;line-height:10px;justify-content:{alignment};align-content:{alignment};grid-template-columns:30px auto;grid-template-rows:30px auto'><div></div><div></div><div></div><div id='auto'><span style='display:block;width:10px;height:10px'></span></div></div>"
            ),
            &format!(
                "(() => {{const r=document.getElementById('auto').getBoundingClientRect();return r.width==={expected} && r.height==={expected};}})()"
            ),
        );
    }
    check(
        "<div style='display:grid;width:200px;grid-template-columns:max-content auto'><div id='fixed'><span style='display:inline-block;width:30px;height:10px'></span></div><div id='auto'></div></div>",
        "document.getElementById('fixed').getBoundingClientRect().width===30 && document.getElementById('auto').getBoundingClientRect().width===170",
    );
}

#[test]
fn implicit_track_grammar_rejects_nested_sizes_and_number_math() {
    check(
        "<div></div>",
        "['calc(3)', 'minmax(minmax(1px, 2px), auto)', 'minmax(auto, fit-content(20px))', 'fit-content(minmax(1px, 2px))'].every(value => !CSS.supports('grid-auto-columns', value) && !CSS.supports('grid-auto-rows', value))",
    );
}

#[test]
fn minmax_intrinsic_maximum_sizes_implicit_tracks_without_stretch() {
    check(
        "<div style='display:grid;width:200px;grid-auto-flow:column;grid-auto-columns:minmax(0px, max-content) auto'><div id='fixed'><span style='display:block;width:30px;height:10px'></span></div><div id='auto'></div></div>",
        "document.getElementById('fixed').getBoundingClientRect().width===30 && document.getElementById('auto').getBoundingClientRect().width===170",
    );
}

#[test]
fn implicit_intrinsic_tracks_match_firefox() {
    for (value, width) in [
        ("min-content", 30),
        ("max-content", 60),
        ("fit-content(40px)", 40),
        ("fit-content(20px)", 30),
        ("fit-content(25%)", 50),
    ] {
        check(
            &format!(
                "<div style='display:grid;width:200px;grid-auto-flow:column;justify-content:start;grid-auto-rows:20px;grid-auto-columns:{value}'><article id='item' style='font-size:0'><span style='display:inline-block;width:30px;height:10px'></span> <span style='display:inline-block;width:30px;height:10px'></span></article></div>"
            ),
            &format!("document.getElementById('item').getBoundingClientRect().width==={width}"),
        );
    }
}

#[test]
fn fixed_positions_do_not_advance_the_row_locked_placement_cursor() {
    for flow in ["row", "row dense", "column", "column dense"] {
        let (tracks, locked, axis) = if flow.starts_with("row") {
            (
                "grid-template-columns:20px 20px 20px;grid-auto-columns:20px;grid-auto-rows:20px",
                "grid-row:1",
                "left",
            )
        } else {
            (
                "grid-template-rows:20px 20px 20px;grid-auto-columns:20px;grid-auto-rows:20px",
                "grid-column:1",
                "top",
            )
        };
        let fixed = if axis == "left" {
            "grid-column:3;grid-row:1"
        } else {
            "grid-column:1;grid-row:3"
        };
        check(
            &format!(
                "<style>body{{margin:0}}</style><div style='display:grid;width:100px;{tracks};grid-auto-flow:{flow}'><div style='{fixed}'></div><div id='item' style='{locked}'></div></div>"
            ),
            &format!("document.getElementById('item').getBoundingClientRect().{axis}===0"),
        );
    }
}
