use super::*;
use crate::paint::Color;
use crate::test_support::http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};
use std::io::Write;

fn evaluate(session: &mut CdpSession, script: &str) -> Value {
    session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression":script,"returnByValue":true}),
        )
        .unwrap()["result"]["value"]
        .clone()
}

fn click(session: &mut CdpSession, x: u32, y: u32) {
    for kind in ["mousePressed", "mouseReleased"] {
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":kind,"x":x,"y":y,"button":"left","clickCount":1}),
            )
            .unwrap();
    }
}

#[test]
fn frameset_cdp_routes_click_key_scroll_and_named_navigation() {
    let listener = bind_loopback().unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let worker = FixtureWorker::spawn(move || {
        let mut paths = Vec::new();
        for _ in 0..4 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let path = request.split_whitespace().nth(1).unwrap().to_string();
            let body = match path.as_str() {
                "/" => include_str!("../../tests/fixtures/frameset-live/index.html"),
                "/left" => include_str!("../../tests/fixtures/frameset-live/left.html"),
                "/right" => include_str!("../../tests/fixtures/frameset-live/right.html"),
                "/next" => include_str!("../../tests/fixtures/frameset-live/next.html"),
                _ => panic!("unexpected request {path}"),
            };
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            paths.push(path);
        }
        paths
    });
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({"url": format!("{base}/")}))
        .unwrap();
    session.set_viewport(200, 100);
    let top = session.document();
    assert_eq!(
        session.paint_current_document().unwrap().pixel(20, 80),
        Some(Color::rgb(255, 0, 0))
    );
    assert_eq!(
        session.paint_current_document().unwrap().pixel(120, 80),
        Some(Color::rgb(0, 0, 255))
    );
    assert_eq!(
        evaluate(
            &mut session,
            "window.savedLeft=left.document;window.savedRight=right.document;frames[0]===left && frames[1]===right"
        ),
        json!(true)
    );
    click(&mut session, 95, 10);
    session
        .dispatch(
            "Input.dispatchKeyEvent",
            json!({"type":"keyDown","text":"x","key":"x"}),
        )
        .unwrap();
    assert_eq!(
        evaluate(
            &mut session,
            "right.clicks===1 && right.client.join(',')==='15,10' && right.document.getElementById('field').value==='x'"
        ),
        json!(true)
    );
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseWheel","x":120,"y":80,"deltaY":40,"deltaX":0}),
        )
        .unwrap();
    assert_eq!(
        evaluate(
            &mut session,
            "right.scrollY>0 && left.scrollY===0 && window.scrollY===0"
        ),
        json!(true)
    );
    click(&mut session, 20, 10);
    session.drive_event_loop(16).unwrap();
    assert_eq!(
        evaluate(
            &mut session,
            "right.arrived===true && right.document!==savedRight && left.document===savedLeft && left.kept===17"
        ),
        json!(true)
    );
    assert_eq!(session.document(), top);
    assert_eq!(session.current_url(), format!("{base}/"));
    assert_eq!(
        session.paint_current_document().unwrap().pixel(120, 80),
        Some(Color::rgb(0, 128, 0))
    );
    let mut paths = worker.join();
    paths.sort();
    assert_eq!(paths, ["/", "/left", "/next", "/right"]);
}
