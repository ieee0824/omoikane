//! Compare user-action cache retention with full invalidation through real frame rendering.
use omoikane::{cdp::CdpSession, frame::render_browser_frame};
use serde_json::json;

fn session(selector: &str) -> CdpSession {
    let html = format!(
        "<style>body{{margin:0;background:white}}\
         main{{width:160px;height:120px}}\
         .prefix{{width:10px;height:10px}}\
         a{{display:block;width:80px;height:40px;background:blue}}\
         span,em{{display:inline-block;width:20px;height:20px;background:gray}}\
         aside{{width:80px;height:20px;background:yellow}}\
         a::before,em::before,aside::before{{content:'';display:block;width:4px;height:4px;background:black}}\
         {selector}{{background:red;width:70px}}\
         {selector}::before{{background:lime}}\
         </style><main><div class=prefix></div>\
         <a href=#><span></span><em></em></a><aside></aside></main>"
    );
    let encoded = html
        .bytes()
        .map(|b| format!("%{b:02X}"))
        .collect::<String>();
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url":format!("data:text/html,{encoded}")}),
        )
        .unwrap();
    session
}

fn move_pointer(session: &mut CdpSession, hovered: bool) {
    let (x, y) = if hovered { (5, 15) } else { (120, 90) };
    session
        .dispatch(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseMoved","x":x,"y":y}),
        )
        .unwrap();
}

#[test]
fn sibling_selectors_keep_geometry_and_pseudo_pixels_equal_to_full_invalidation() {
    for selector in [
        ".prefix + a:hover",
        ".prefix + :is(a:hover)",
        "a:hover > span + em",
        ":is(a:hover > span) + em",
        ":not(a:hover > span) + em",
        "a:hover + aside",
        "a:hover ~ aside",
        ":is(a:hover + aside)",
        "main :where(a:hover ~ aside)",
        ":not(a:hover) + aside",
        ":is(a:hover, a:hover > span) + aside",
    ] {
        let mut actual = session(selector);
        let mut full = session(selector);
        for session in [&mut actual, &mut full] {
            move_pointer(session, false);
        }
        let initial = render_browser_frame(&mut actual, 160, 120, 0).unwrap();
        assert_eq!(
            initial.pixels(),
            render_browser_frame(&mut full, 160, 120, 0)
                .unwrap()
                .pixels(),
            "initial {selector}"
        );
        for (index, hovered) in [true, false, true, false].into_iter().enumerate() {
            let response = full.dispatch("Runtime.evaluate", json!({"expression":format!(
                "document.documentElement.style.setProperty('--force-full-reference','{index}')"
            )})).unwrap();
            assert!(response.get("exceptionDetails").is_none(), "{response}");
            for session in [&mut actual, &mut full] {
                move_pointer(session, hovered);
            }
            let image = render_browser_frame(&mut actual, 160, 120, 0).unwrap();
            let reference = render_browser_frame(&mut full, 160, 120, 0).unwrap();
            assert_eq!(
                image.pixels(),
                reference.pixels(),
                "{selector}, hovered={hovered}"
            );
            assert_eq!(
                image.pixels() != initial.pixels(),
                hovered,
                "fixture must visibly change and restore: {selector}"
            );
        }
    }
}
