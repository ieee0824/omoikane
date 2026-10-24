//! Hover invalidation must preserve live animation sampling through the CDP frame owner.
use omoikane::{cdp::CdpSession, frame::render_browser_frame};
use serde_json::{Value, json};

fn evaluate(session: &mut CdpSession, expression: &str) -> Value {
    let response = session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression":expression,"returnByValue":true}),
        )
        .unwrap();
    assert!(response.get("exceptionDetails").is_none(), "{response}");
    response["result"]["value"].clone()
}

fn session() -> CdpSession {
    let html = "<style>body{margin:0;background:white}\
        a{display:block;width:40px;height:20px;background:blue}\
        a:hover{background:green}\
        @keyframes fade{from{opacity:0}to{opacity:1}}\
        #animated{position:absolute;left:80px;top:0;width:20px;height:20px;\
        background:red;animation:fade 1s linear forwards}\
        #animated::before{content:'';position:absolute;left:30px;top:0;\
        width:20px;height:20px;background:red;animation:fade 1s linear forwards}\
        </style><a id=link href=#></a><div id=animated></div>";
    let encoded = html
        .bytes()
        .map(|byte| format!("%{byte:02X}"))
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

#[test]
fn hover_changes_preserve_disjoint_live_animation_pixels() {
    let mut actual = session();
    let mut full = session();
    let initial = render_browser_frame(&mut actual, 160, 80, 0).unwrap();
    assert_eq!(
        initial.pixels(),
        render_browser_frame(&mut full, 160, 80, 0)
            .unwrap()
            .pixels()
    );
    let mut last_pixels = Vec::new();
    for (index, (delta, hovered)) in [(250, true), (250, false), (250, true), (500, false)]
        .into_iter()
        .enumerate()
    {
        // This unused inherited variable forces the reference's root invalidation.
        evaluate(
            &mut full,
            &format!(
                "document.documentElement.style.setProperty('--force-full-reference','{index}')"
            ),
        );
        let (x, y) = if hovered { (10, 10) } else { (60, 40) };
        for session in [&mut actual, &mut full] {
            session
                .dispatch(
                    "Input.dispatchMouseEvent",
                    json!({"type":"mouseMoved","x":x,"y":y}),
                )
                .unwrap();
        }
        let image = render_browser_frame(&mut actual, 160, 80, delta).unwrap();
        assert_eq!(
            image.pixels(),
            render_browser_frame(&mut full, 160, 80, delta)
                .unwrap()
                .pixels()
        );
        for session in [&mut actual, &mut full] {
            let value = evaluate(
                session,
                "getComputedStyle(document.getElementById('link')).backgroundColor",
            );
            assert_eq!(
                value.as_str(),
                Some(if hovered {
                    "rgb(0, 128, 0)"
                } else {
                    "rgb(0, 0, 255)"
                })
            );
            for pseudo in ["", ", '::before'"] {
                let opacity = evaluate(
                    session,
                    &format!(
                        "getComputedStyle(document.getElementById('animated'){pseudo}).opacity"
                    ),
                )
                .as_str()
                .unwrap()
                .parse::<f32>()
                .unwrap();
                if index < 3 {
                    assert!(opacity > 0.0 && opacity < 1.0, "{opacity}");
                } else {
                    assert_eq!(opacity, 1.0);
                }
            }
        }
        last_pixels = image.pixels().to_vec();
    }
    for x in [90, 120] {
        let offset = (10 * 160 + x) * 4;
        assert_ne!(
            &initial.pixels()[offset..offset + 4],
            &last_pixels[offset..offset + 4]
        );
        assert_eq!(&last_pixels[offset..offset + 4], &[255, 0, 0, 255]);
    }
}
