use super::*;
use crate::html::TreeBuilder;
use crate::paint::Color;
use crate::test_support::http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};
use std::io::Write;

#[test]
fn frameset_paints_both_live_documents_and_exposes_named_windows() {
    let listener = bind_loopback().unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let worker = FixtureWorker::spawn(move || {
        let mut paths = Vec::new();
        for _ in 0..2 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let path = request.split_whitespace().nth(1).unwrap().to_string();
            let color = if path == "/left" { "red" } else { "blue" };
            let body = format!(
                "<html><head><style>body {{margin:0;min-height:100vh;background:{color}}}</style></head><body><button id='button'>button</button><script>window.marker='{color}'</script></body></html>"
            );
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            paths.push(path);
        }
        paths
    });
    let document = TreeBuilder::parse(
        "<html><head></head><frameset cols='40%,*'><frame src='left' name='left'><frame src='right' name='right'><noframes>must not render</noframes></frameset></html>"
    ).document();
    let mut runtime = JsRuntime::with_document_and_url(document, &base).unwrap();
    runtime.set_viewport(200.0, 100.0);
    runtime.run_until_idle().unwrap();
    let canvas = runtime.paint_current_document().unwrap();
    assert_eq!(canvas.pixel(20, 80), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(120, 80), Some(Color::rgb(0, 0, 255)));
    assert_eq!(runtime.eval("frames.length === 2 && frames[0] === left && frames[1] === right && left.parent === window && right.top === window && left.marker === 'red' && right.marker === 'blue'").unwrap().as_boolean(), Some(true));
    let target = runtime.hit_test(90.0, 10.0).unwrap();
    assert_eq!(target.get_attribute("id").as_deref(), Some("button"));
    assert_eq!(
        owner_document_for_node(&target).unwrap().identity(),
        runtime
            .host_state
            .borrow()
            .iframe_documents
            .values()
            .find(|entry| entry
                .document
                .query_selector("button")
                .is_some_and(|node| node == target))
            .unwrap()
            .document
            .identity()
    );
    let mut paths = worker.join();
    paths.sort();
    assert_eq!(paths, ["/left", "/right"]);
}

#[test]
fn frameset_fixed_percent_star_tracks_and_nested_rectangles() {
    let document = TreeBuilder::parse(
        "<html><frameset cols='100,25%,*'><frame id='fixed'><frame id='percent'><frameset rows='30,*'><frame id='top'><frame id='bottom'></frameset></frameset></html>"
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(600.0, 200.0);
    for (id, expected) in [
        ("fixed", "0,0,100,200"),
        ("percent", "100,0,150,200"),
        ("top", "250,0,350,30"),
        ("bottom", "250,30,350,170"),
    ] {
        let script = format!(
            "(() => {{const r=document.getElementById('{id}').getBoundingClientRect();return [r.x,r.y,r.width,r.height].join(',')}})()"
        );
        assert_eq!(
            runtime
                .eval(&script)
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            expected
        );
    }
}

#[test]
fn nested_document_snapshots_follow_stacking_clipping_and_opacity() {
    for (style, wrapper, extra, inside, outside) in [
        (
            "",
            "",
            "<div style='position:absolute;left:20px;top:20px;width:40px;height:40px;background:blue;z-index:10'></div>",
            Color::rgb(0, 0, 255),
            Color::rgb(255, 0, 0),
        ),
        (
            "",
            "overflow:hidden;width:30px;height:30px",
            "",
            Color::rgb(255, 0, 0),
            Color::rgb(255, 255, 255),
        ),
        (
            "opacity:0.5",
            "",
            "",
            Color::rgb(255, 127, 127),
            Color::rgb(255, 127, 127),
        ),
    ] {
        let html = format!(
            "<html><head><style>body{{margin:0;min-height:100vh;background:white}}iframe{{display:block;border:0;width:80px;height:80px;{style}}}</style></head><body><div style='{wrapper}'><iframe srcdoc=\"<body style='margin:0;min-height:100vh;background:red'></body>\"></iframe></div>{extra}</body></html>"
        );
        let mut runtime = JsRuntime::with_document(TreeBuilder::parse(&html).document()).unwrap();
        runtime.set_viewport(120.0, 120.0);
        let canvas = runtime.paint_current_document().unwrap();
        assert_eq!(
            canvas.pixel(25, 25),
            Some(inside),
            "style={style}, wrapper={wrapper}"
        );
        assert_eq!(
            canvas.pixel(70, 70),
            Some(outside),
            "style={style}, wrapper={wrapper}"
        );
    }
}

#[test]
fn frame_has_its_own_interface_and_ignores_iframe_only_attributes() {
    let document = TreeBuilder::parse("<html><frameset><frame id='frame' name='child' sandbox srcdoc='<body>ignored</body>'></frameset></html>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert_eq!(runtime.eval("(() => {const f=document.getElementById('frame');const i=document.createElement('iframe');return i instanceof HTMLIFrameElement && !(i instanceof HTMLFrameElement) && 'srcdoc' in i && 'sandbox' in i && f instanceof HTMLFrameElement && f instanceof HTMLElement && !(f instanceof HTMLIFrameElement) && f.name==='child' && !('sandbox' in f) && !('srcdoc' in f) && f.contentWindow.document.body.textContent==='';})()").unwrap().as_boolean(), Some(true));
}

#[test]
fn frame_viewport_units_follow_frameset_track_mutations() {
    let document = TreeBuilder::parse("<html><frameset id='set' cols='20%,*'><frame id='first' name='childFrame'><frame></frameset></html>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(600.0, 200.0);
    runtime.run_until_idle().unwrap();
    runtime
        .eval("childFrame.document.body.style.width='100vw'")
        .unwrap();
    assert_eq!(
        runtime
            .eval("childFrame.getComputedStyle(childFrame.document.body).width")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "120px"
    );
    runtime
        .eval("document.getElementById('set').setAttribute('cols','50%,*')")
        .unwrap();
    assert_eq!(
        runtime
            .eval("childFrame.getComputedStyle(childFrame.document.body).width")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "300px"
    );
}

#[test]
fn find_in_page_visits_frames_without_joining_text_across_viewports() {
    let document = TreeBuilder::parse(
        "<html><frameset cols='50%,*'><frame name='left'><frame name='right'></frameset></html>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval("left.document.body.textContent='needle';right.document.body.textContent='needle'")
        .unwrap();
    assert_eq!(
        runtime
            .find_in_page(FindInPageAction::Start, "needle")
            .unwrap()
            .match_count,
        2
    );
    runtime
        .eval("left.document.body.textContent='ne';right.document.body.textContent='edle'")
        .unwrap();
    assert_eq!(
        runtime
            .find_in_page(FindInPageAction::Start, "needle")
            .unwrap()
            .match_count,
        0
    );
}
