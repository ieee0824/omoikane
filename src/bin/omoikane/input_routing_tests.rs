use super::*;
use crate::device_scale::DeviceScale;
use crate::render_browser_frame;
use omoikane::platform_input::InputModifiers;
use serde_json::json;

const START_URL: &str = "data:text/html,<input id=field><div id=target></div>";

fn app() -> BrowserApp {
    let mut app = BrowserApp::new(START_URL).unwrap();
    render_browser_frame(&mut app.session, 320, 200, 0).unwrap();
    evaluate(
        &mut app,
        r#"
        globalThis.log=[];
        const field=document.getElementById('field');
        field.focus();
        for (const type of ['keydown','keyup','mousedown','mouseup','wheel'])
            document.addEventListener(type,e=>log.push(type+':'+(e.key??e.button??'')));
    "#,
    );
    app
}

fn evaluate(app: &mut BrowserApp, expression: &str) -> serde_json::Value {
    app.session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression":expression,"returnByValue":true}),
        )
        .unwrap()["result"]["value"]
        .clone()
}

/// A 320x240 window at scale 1: toolbar rows 0..40, page below.
fn layout() -> ChromeLayout {
    ChromeLayout::new(320, 240, DeviceScale::new(1.0), true)
}

/// Feeds a key press and release through the chrome, then the page if unconsumed.
fn type_key(app: &mut BrowserApp, key: &str, text: Option<&str>) {
    for pressed in [true, false] {
        if !app.handle_chrome_key(key, text, pressed) {
            app.input
                .key_event(
                    &mut app.session,
                    omoikane::platform_input::PlatformKeyEvent {
                        pressed,
                        key: key.into(),
                        code: String::new(),
                        text: pressed.then(|| text.map(ToOwned::to_owned)).flatten(),
                        repeat: false,
                    },
                )
                .unwrap();
        }
    }
}

fn with_shortcut(app: &mut BrowserApp, key: &str) {
    app.modifiers = InputModifiers {
        control: true,
        ..InputModifiers::default()
    };
    app.input.set_modifiers(app.modifiers);
    type_key(app, key, None);
    app.modifiers = InputModifiers::default();
    app.input.set_modifiers(app.modifiers);
}

#[test]
fn shortcut_focuses_url_bar_and_typing_edits_it_without_reaching_the_page() {
    let mut app = app();
    type_key(&mut app, "a", Some("a"));
    with_shortcut(&mut app, "l");
    assert_eq!(app.input_target(), InputTarget::UrlBar);
    for ch in ["h", "i"] {
        type_key(&mut app, ch, Some(ch));
    }
    type_key(&mut app, "Backspace", None);
    type_key(&mut app, "o", Some("o"));
    assert_eq!(app.url_bar.display_text(), "ho");
    assert_eq!(evaluate(&mut app, "field.value"), json!("a"));
    assert_eq!(
        evaluate(&mut app, "JSON.stringify(log)"),
        json!(r#"["keydown:a","keyup:a"]"#)
    );

    type_key(&mut app, "Escape", None);
    assert_eq!(app.input_target(), InputTarget::Page);
    type_key(&mut app, "b", Some("b"));
    assert_eq!(evaluate(&mut app, "field.value"), json!("ab"));
}

fn type_text(app: &mut BrowserApp, text: &str) {
    for ch in text.chars() {
        let ch = ch.to_string();
        type_key(app, &ch, Some(&ch));
    }
}

fn commit_url(app: &mut BrowserApp, url: &str) {
    with_shortcut(app, "l");
    type_text(app, url);
    type_key(app, "Enter", None);
}

#[test]
fn committing_a_url_navigates_once_and_page_input_keeps_working() {
    let mut app = app();
    let target = "data:text/html,<title>B</title><input id=next>";
    commit_url(&mut app, target);
    assert_eq!(app.session.current_url(), target);
    assert!(!app.url_bar.is_editing());
    assert_eq!(app.url_bar.display_text(), target);
    assert_eq!(evaluate(&mut app, "document.title"), json!("B"));
    // One history entry per commit: the start page plus the target.
    assert_eq!(evaluate(&mut app, "history.length"), json!(2));

    render_browser_frame(&mut app.session, 320, 200, 1).unwrap();
    evaluate(&mut app, "document.getElementById('next').focus()");
    type_key(&mut app, "z", Some("z"));
    assert_eq!(
        evaluate(&mut app, "document.getElementById('next').value"),
        json!("z")
    );
}

#[test]
fn rejected_and_failed_urls_stay_in_the_bar_without_navigating() {
    for text in [
        "example.test/path",
        "javascript:document.title='ran'",
        "http://",
    ] {
        let mut app = app();
        commit_url(&mut app, text);
        assert_eq!(app.session.current_url(), START_URL, "{text}");
        assert_eq!(app.input_target(), InputTarget::UrlBar, "{text}");
        assert_eq!(app.url_bar.display_text(), text);
        assert_eq!(app.url_bar.page_url(), START_URL);
        assert_ne!(evaluate(&mut app, "document.title"), json!("ran"));
        type_key(&mut app, "Escape", None);
        assert_eq!(app.url_bar.display_text(), START_URL);
    }
}

#[test]
fn page_initiated_and_redirected_navigation_updates_the_bar_but_not_a_draft() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let read = stream.read(&mut request).unwrap();
            let response = if String::from_utf8_lossy(&request[..read]).starts_with("GET /old ") {
                "HTTP/1.1 302 Found\r\nLocation: /new\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
            } else {
                let body = "<title>new</title>";
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    let mut app = app();
    commit_url(&mut app, &format!("{base}/old"));
    server.join().unwrap();
    assert_eq!(app.url_bar.display_text(), format!("{base}/new"));

    with_shortcut(&mut app, "l");
    type_text(&mut app, "draft");
    evaluate(&mut app, "history.pushState(null, '', '/pushed')");
    app.sync_url_bar();
    assert_eq!(app.url_bar.display_text(), "draft");
    assert_eq!(app.url_bar.page_url(), format!("{base}/pushed"));
    type_key(&mut app, "Escape", None);
    assert_eq!(app.url_bar.display_text(), format!("{base}/pushed"));
}

#[test]
fn ime_goes_only_to_the_url_bar_while_editing() {
    use winit::event::{Ime, WindowEvent};
    let mut app = app();
    with_shortcut(&mut app, "l");
    app.dispatch_input(WindowEvent::Ime(Ime::Preedit("に".into(), Some((0, 3)))));
    app.dispatch_input(WindowEvent::Ime(Ime::Commit("日本".into())));
    assert_eq!(app.url_bar.display_text(), "日本");
    assert_eq!(evaluate(&mut app, "field.value"), json!(""));
    type_key(&mut app, "Escape", None);
    assert_eq!(app.url_bar.display_text(), START_URL);
    app.dispatch_input(WindowEvent::Ime(Ime::Commit("x".into())));
    assert_eq!(evaluate(&mut app, "field.value"), json!("x"));
}

#[test]
fn url_bar_and_find_ui_take_key_input_exclusively() {
    let mut app = app();
    with_shortcut(&mut app, "f");
    assert_eq!(app.input_target(), InputTarget::Find);
    with_shortcut(&mut app, "l");
    assert_eq!(app.input_target(), InputTarget::UrlBar);
    assert!(app.find_ui.is_none());
    with_shortcut(&mut app, "f");
    assert_eq!(app.input_target(), InputTarget::Find);
    assert!(!app.url_bar.is_editing());
    assert_eq!(evaluate(&mut app, "JSON.stringify(log)"), json!("[]"));
}

#[test]
fn page_release_of_a_key_pressed_before_focusing_the_bar_still_reaches_the_page() {
    let mut app = app();
    assert!(!app.handle_chrome_key("Shift", None, true));
    app.input
        .key_event(
            &mut app.session,
            omoikane::platform_input::PlatformKeyEvent {
                pressed: true,
                key: "Shift".into(),
                code: String::new(),
                text: None,
                repeat: false,
            },
        )
        .unwrap();
    with_shortcut(&mut app, "l");
    assert!(!app.handle_chrome_key("Shift", None, false));
}

#[test]
fn clicks_focus_the_url_field_and_never_leak_toolbar_input_to_the_page() {
    let mut app = app();
    let layout = layout();
    assert_eq!(
        app.route_cursor(layout, 100.0, 100.0),
        PagePointer::Move(100.0, 60.0)
    );
    assert_eq!(app.route_cursor(layout, 100.0, 20.0), PagePointer::Leave);
    assert_eq!(app.route_cursor(layout, 110.0, 20.0), PagePointer::Ignore);
    assert!(!app.pointer_targets_page(layout));
    assert!(!app.route_mouse_button(layout, PlatformMouseButton::Left, true));
    assert!(app.url_bar.is_editing());
    assert!(!app.route_mouse_button(layout, PlatformMouseButton::Left, false));

    // Toolbar margin outside the field does not focus it.
    app.url_bar.cancel();
    app.route_cursor(layout, 1.0, 1.0);
    assert!(!app.route_mouse_button(layout, PlatformMouseButton::Left, true));
    assert!(!app.url_bar.is_editing());
    app.route_mouse_button(layout, PlatformMouseButton::Left, false);

    // A page click ends editing and goes to the page.
    with_shortcut(&mut app, "l");
    app.route_cursor(layout, 50.0, 150.0);
    assert!(app.pointer_targets_page(layout));
    assert!(app.route_mouse_button(layout, PlatformMouseButton::Left, true));
    assert!(!app.url_bar.is_editing());
}

#[test]
fn a_page_drag_keeps_the_page_as_target_over_the_toolbar() {
    let mut app = app();
    let layout = layout();
    app.route_cursor(layout, 50.0, 150.0);
    assert!(app.route_mouse_button(layout, PlatformMouseButton::Left, true));
    assert_eq!(
        app.route_cursor(layout, 50.0, 10.0),
        PagePointer::Move(50.0, -30.0)
    );
    assert!(app.route_mouse_button(layout, PlatformMouseButton::Left, false));
    assert!(!app.url_bar.is_editing());
    assert_eq!(app.route_cursor(layout, 50.0, 10.0), PagePointer::Leave);
}

#[test]
fn toolbar_touches_never_reach_the_page_even_after_entering_it() {
    use PlatformTouchPhase::*;
    let mut app = app();
    let layout = layout();
    assert_eq!(app.route_touch(layout, 7, Started, 100.0, 20.0), None);
    assert!(app.url_bar.is_editing());
    assert_eq!(app.route_touch(layout, 7, Moved, 100.0, 100.0), None);
    assert_eq!(app.route_touch(layout, 7, Ended, 100.0, 100.0), None);
    assert!(app.page_touches.is_empty());

    app.url_bar.cancel();
    assert_eq!(app.route_touch(layout, 8, Started, 1.0, 1.0), None);
    assert!(!app.url_bar.is_editing());
    assert_eq!(app.route_touch(layout, 8, Cancelled, 100.0, 100.0), None);
    assert_eq!(app.route_touch(layout, 9, Started, -1.0, 100.0), None);
    assert_eq!(app.route_touch(layout, 9, Moved, 100.0, 100.0), None);
}

#[test]
fn native_contacts_outside_the_window_do_not_dispatch_dom_touch_events() {
    let mut app = app();
    evaluate(
        &mut app,
        "for (const type of ['touchstart','touchmove','touchend','touchcancel']) \
         document.addEventListener(type, e => log.push(type));",
    );
    // No native window is attached in this unit test: all coordinates are
    // outside its empty layout. Exercise the actual WindowEvent dispatch path.
    for phase in [TouchPhase::Started, TouchPhase::Moved, TouchPhase::Ended] {
        assert!(app.dispatch_input(winit::event::WindowEvent::Touch(Touch {
            device_id: winit::event::DeviceId::dummy(),
            phase,
            location: winit::dpi::PhysicalPosition::new(100.0, 20.0),
            force: None,
            id: 7,
        })));
    }
    assert_eq!(evaluate(&mut app, "log"), json!([]));
}

#[test]
fn page_touches_keep_their_target_until_end_or_cancellation() {
    use PlatformTouchPhase::*;
    let mut app = app();
    let layout = layout();
    app.focus_url_bar();
    for terminal in [Ended, Cancelled] {
        assert_eq!(
            app.route_touch(layout, 7, Started, 100.0, 100.0),
            Some((100.0, 60.0))
        );
        assert!(!app.url_bar.is_editing());
        assert_eq!(
            app.route_touch(layout, 7, Moved, 100.0, 20.0),
            Some((100.0, -20.0))
        );
        assert_eq!(
            app.route_touch(layout, 7, terminal, 100.0, 20.0),
            Some((100.0, -20.0))
        );
        assert!(app.page_touches.is_empty());
        assert_eq!(app.route_touch(layout, 7, Moved, 100.0, 100.0), None);
        assert_eq!(app.route_touch(layout, 7, terminal, 100.0, 100.0), None);
    }
    // The platform may reuse a completed page contact's ID over the toolbar.
    assert_eq!(app.route_touch(layout, 7, Started, 100.0, 20.0), None);
    assert_eq!(app.route_touch(layout, 7, Ended, 100.0, 100.0), None);
}

#[test]
fn simultaneous_page_and_toolbar_touches_have_independent_targets() {
    use PlatformTouchPhase::*;
    let mut app = app();
    let layout = layout();
    assert_eq!(
        app.route_touch(layout, 1, Started, 100.0, 100.0),
        Some((100.0, 60.0))
    );
    assert_eq!(app.route_touch(layout, 2, Started, 100.0, 20.0), None);
    assert_eq!(app.route_touch(layout, 2, Moved, 100.0, 100.0), None);
    assert_eq!(
        app.route_touch(layout, 1, Moved, 100.0, 20.0),
        Some((100.0, -20.0))
    );
    assert_eq!(app.route_touch(layout, 2, Cancelled, 100.0, 100.0), None);
    assert_eq!(
        app.route_touch(layout, 1, Ended, 100.0, 20.0),
        Some((100.0, -20.0))
    );
    assert!(app.page_touches.is_empty());
}

#[test]
fn touch_routing_uses_css_pixels_and_the_current_fullscreen_layout() {
    use PlatformTouchPhase::*;
    let mut app = app();
    let scaled = ChromeLayout::new(640, 480, DeviceScale::new(2.0), true);
    assert_eq!(app.route_touch(scaled, 1, Started, 200.0, 40.0), None);
    assert_eq!(
        app.route_touch(scaled, 2, Started, 200.0, 200.0),
        Some((100.0, 60.0))
    );
    let fullscreen = ChromeLayout::new(640, 480, DeviceScale::new(2.0), false);
    assert_eq!(
        app.route_touch(fullscreen, 2, Ended, 200.0, 40.0),
        Some((100.0, 20.0))
    );
    assert_eq!(
        app.route_touch(fullscreen, 3, Started, 200.0, 40.0),
        Some((100.0, 20.0))
    );
}
