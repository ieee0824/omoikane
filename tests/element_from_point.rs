use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime() -> JsRuntime {
    let document = TreeBuilder::parse(
        r#"<html><head><style>
            html, body { margin: 0; }
            #bottom { position: absolute; left: 10px; top: 10px; width: 80px; height: 80px; background: red; }
            #top { position: absolute; left: 20px; top: 20px; width: 40px; height: 40px; background: blue; }
        </style></head><body><div id="bottom"></div><div id="top"></div></body></html>"#,
    )
    .document();
    JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap()
}

#[test]
fn document_point_queries_use_paint_order_and_viewport_bounds() {
    let mut runtime = runtime();
    assert!(
        runtime
            .eval(
                r#"
            const bottom = document.getElementById('bottom');
            const top = document.getElementById('top');
            const hits = document.elementsFromPoint(30, 30);
            document.elementFromPoint(30, 30) === top &&
              hits[0] === top && hits[1] === bottom &&
              hits[hits.length - 1] === document.documentElement &&
              document.elementFromPoint(-1, 30) === null &&
              document.elementsFromPoint(-1, 30).length === 0;
        "#,
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn pointer_events_none_exposes_the_box_below_after_style_change() {
    let mut runtime = runtime();
    assert!(
        runtime
            .eval(
                r#"
            const bottom = document.getElementById('bottom');
            const top = document.getElementById('top');
            top.style.pointerEvents = 'none';
            document.elementFromPoint(30, 30) === bottom &&
              !document.elementsFromPoint(30, 30).includes(top);
        "#,
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn shadow_queries_retarget_for_each_root() {
    let mut runtime = runtime();
    assert!(
        runtime
            .eval(
                r#"
            const host = document.createElement('div');
            host.style.cssText = 'position:absolute;left:100px;top:10px;width:60px;height:60px';
            document.body.append(host);
            const root = host.attachShadow({mode:'open'});
            root.innerHTML = '<div id="inner" style="width:60px;height:60px;background:green"></div>';
            const inner = root.getElementById('inner');
            document.elementFromPoint(120, 30) === host &&
              root.elementFromPoint(120, 30) === inner &&
              root.elementsFromPoint(120, 30)[0] === inner &&
              root.elementFromPoint(15, 15) === document.getElementById('bottom');
        "#,
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn iframe_point_queries_use_each_documents_viewport() {
    let mut runtime = runtime();
    assert!(
        runtime
            .eval(
                r#"
            const frame = document.createElement('iframe');
            frame.style.cssText = 'position:absolute;left:100px;top:100px;width:80px;height:60px;border:0';
            document.body.append(frame);
            const child = frame.contentDocument;
            child.body.innerHTML = '<div id="inside" style="position:absolute;left:0;top:0;width:50px;height:50px;background:green"></div>';
            document.elementFromPoint(110, 110) === frame &&
              child.elementFromPoint(10, 10) === child.getElementById('inside') &&
              child.elementFromPoint(90, 10) === null;
        "#,
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn iframe_document_geometry_supplies_point_query_coordinates() {
    let mut runtime = runtime();
    let report = runtime
        .eval(
            r#"
            const frame = document.createElement('iframe');
            frame.style.cssText = 'position:absolute;left:100px;top:100px;width:120px;height:120px';
            document.body.append(frame);
            const child = frame.contentDocument;
            child.body.style.margin = '0';
            child.body.innerHTML = '<div id="inside" style="width:100px;height:100px;background:green"></div>';
            const inner = child.getElementById('inside');
            const rect = inner.getBoundingClientRect();
            JSON.stringify({left:rect.left,top:rect.top,width:rect.width,height:rect.height,
              hit:child.elementsFromPoint(rect.right - 1, rect.top + 1)[0] === inner});
        "#,
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    let report: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(report["left"], 0.0, "{report}");
    assert_eq!(report["top"], 0.0, "{report}");
    assert_eq!(report["width"], 100.0, "{report}");
    assert_eq!(report["height"], 100.0, "{report}");
    assert_eq!(report["hit"], true, "{report}");
}

#[test]
fn point_queries_follow_transform_clipping_and_visibility() {
    let mut runtime = runtime();
    assert!(
        runtime
            .eval(
                r#"
            const clip = document.createElement('div');
            clip.style.cssText = 'position:absolute;left:200px;top:10px;width:30px;height:30px;overflow:hidden';
            const box = document.createElement('div');
            box.style.cssText = 'position:absolute;left:0;top:0;width:60px;height:30px;background:green;transform:translateX(10px)';
            clip.append(box);
            document.body.append(clip);
            const hit = document.elementFromPoint(215, 20) === box;
            const clipped = document.elementFromPoint(235, 20) !== box;
            box.style.visibility = 'hidden';
            hit && clipped && document.elementFromPoint(215, 20) !== box;
        "#,
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn point_queries_follow_window_and_element_scroll() {
    let mut runtime = runtime();
    let report = runtime
        .eval(
                r#"
            document.body.style.height = '2000px';
            const target = document.createElement('div');
            target.style.cssText = 'position:absolute;left:100px;top:180px;width:40px;height:40px;background:green';
            document.body.append(target);
            window.scrollTo(0, 150);
            const windowHit = document.elementFromPoint(110, 40) === target;
            const scroller = document.createElement('div');
            scroller.style.cssText = 'position:fixed;left:200px;top:10px;width:80px;height:60px;overflow:auto';
            const inner = document.createElement('div');
            inner.style.cssText = 'margin-top:100px;width:50px;height:40px;background:blue';
            scroller.append(inner);
            document.body.append(scroller);
            scroller.scrollTop = 90;
            JSON.stringify({
              windowHit,
              elementHit: document.elementFromPoint(210, 35) === inner,
              windowTarget: document.elementFromPoint(110, 40)?.id,
              elementTarget: document.elementFromPoint(210, 35)?.id,
              scrollY: window.scrollY,
              scrollTop: scroller.scrollTop
            });
        "#,
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    let report: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(report["windowHit"], true, "{report}");
    assert_eq!(report["elementHit"], true, "{report}");
}

#[test]
fn point_queries_follow_z_index_order() {
    let mut runtime = runtime();
    assert!(
        runtime
            .eval(
                r#"
            const high = document.getElementById('bottom');
            const low = document.getElementById('top');
            high.style.zIndex = '10';
            low.style.zIndex = '1';
            const hits = document.elementsFromPoint(30, 30);
            document.elementFromPoint(30, 30) === high &&
              hits.indexOf(high) < hits.indexOf(low);
        "#,
            )
            .unwrap()
            .to_boolean()
    );
}
