//! Interactive selector state must reach the painted output.
use omoikane::{cdp::CdpSession, frame::render_browser_frame};
use serde_json::json;

#[test]
fn scripted_focus_colors_match_selector_state_in_paint() {
    let html = include_str!("fixtures/anonymized-user-action-pseudos/focus.html");
    let url = format!(
        "data:text/html,{}",
        html.bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect::<String>()
    );
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({"url": url}))
        .unwrap();
    let frame = render_browser_frame(&mut session, 320, 180, 16).unwrap();
    if let Some(root) = std::env::var_os("OMOIKANE_USER_ACTION_IMAGES") {
        let root = std::path::PathBuf::from(root);
        std::fs::create_dir_all(&root).unwrap();
        let file =
            std::fs::File::create(root.join("anonymized-user-action-pseudos.focus.actual.png"))
                .unwrap();
        let mut encoder = png::Encoder::new(file, 320, 180);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(frame.pixels())
            .unwrap();
    }
    for ((x, y), expected) in [
        ((8, 8), [32, 80, 96, 255]),
        ((30, 30), [240, 88, 136, 255]),
        ((250, 150), [255, 255, 255, 255]),
    ] {
        let index = (y * 320 + x) * 4;
        assert_eq!(&frame.pixels()[index..index + 4], expected, "pixel {x},{y}");
    }
}
