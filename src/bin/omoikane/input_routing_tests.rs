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

    type_key(&mut app, "Enter", None);
    assert_eq!(app.requested_navigation.as_deref(), Some("ho"));
    assert_eq!(app.input_target(), InputTarget::Page);
    type_key(&mut app, "b", Some("b"));
    assert_eq!(evaluate(&mut app, "field.value"), json!("ab"));
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
