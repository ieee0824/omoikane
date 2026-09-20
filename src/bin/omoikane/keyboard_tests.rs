use super::*;

fn app() -> BrowserApp {
    let mut app = BrowserApp::new("data:text/html,<input id=field><div id=target></div>").unwrap();
    render_browser_frame(&mut app.session, 320, 200, 0).unwrap();
    evaluate(
        &mut app,
        r#"
        globalThis.keys=[];globalThis.edits=0;globalThis.resolved=0;
        globalThis.denied='';globalThis.requested=0;
        const field=document.getElementById('field');
        const target=document.getElementById('target');
        field.focus();
        field.oninput=()=>edits++;
        document.onkeydown=e=>{
            keys.push([e.type,e.key,e.repeat]);
            if(e.key==='l'){
                requested++;
                target.requestPointerLock().then(()=>resolved++,e=>denied=e.name);
            }
        };
        document.onkeyup=e=>keys.push([e.type,e.key,e.repeat]);
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

fn key(key: &str, pressed: bool, repeat: bool) -> PlatformKeyEvent {
    PlatformKeyEvent {
        pressed,
        key: key.into(),
        code: format!("Key{}", key.to_uppercase()),
        text: pressed.then(|| key.into()),
        repeat,
    }
}

#[test]
fn synthetic_focus_keys_do_not_dispatch_edit_or_grant_activation() {
    let mut app = app();
    app.dispatch_input(WindowEvent::Focused(false));
    app.dispatch_key_input(key("l", false, false), true)
        .unwrap();
    app.dispatch_input(WindowEvent::Focused(true));
    app.dispatch_key_input(key("l", true, false), true).unwrap();
    app.dispatch_key_input(key("l", true, true), true).unwrap();
    app.dispatch_key_input(key("l", false, false), true)
        .unwrap();
    assert_eq!(
        evaluate(
            &mut app,
            "JSON.stringify([field.value,keys,edits,requested])"
        ),
        json!("[\"\",[],0,0]")
    );
    assert!(app.session.take_pointer_lock_transition().is_none());
    evaluate(
        &mut app,
        "target.requestPointerLock().catch(e=>denied=e.name)",
    );
    render_browser_frame(&mut app.session, 320, 200, 1).unwrap();
    assert_eq!(evaluate(&mut app, "denied"), json!("NotAllowedError"));
    assert!(app.session.take_pointer_lock_transition().is_none());
}

#[test]
fn real_keys_keep_text_repeat_and_keyup_with_interleaved_synthetic_events() {
    let mut app = app();
    app.dispatch_key_input(key("a", true, false), false)
        .unwrap();
    app.dispatch_key_input(key("a", true, false), true).unwrap();
    app.dispatch_key_input(key("a", false, false), true)
        .unwrap();
    app.dispatch_key_input(key("a", true, true), false).unwrap();
    app.dispatch_key_input(key("a", false, false), false)
        .unwrap();
    assert_eq!(evaluate(&mut app, "field.value"), json!("aa"));
    assert_eq!(evaluate(&mut app, "edits"), json!(2));
    assert_eq!(
        evaluate(&mut app, "JSON.stringify(keys)"),
        json!("[[\"keydown\",\"a\",false],[\"keydown\",\"a\",true],[\"keyup\",\"a\",false]]")
    );
}

#[test]
fn focus_loss_releases_lock_and_synthetic_restore_does_not_reacquire() {
    let mut app = app();
    app.dispatch_key_input(key("l", true, false), false)
        .unwrap();
    let Some(PointerLockTransition::Acquire { request_id, .. }) =
        app.session.take_pointer_lock_transition()
    else {
        panic!("real input must request Pointer Lock")
    };
    app.session
        .complete_pointer_lock_request(request_id, true)
        .unwrap();
    assert!(app.session.is_pointer_locked());
    app.dispatch_input(WindowEvent::Focused(false));
    app.dispatch_key_input(key("l", false, false), true)
        .unwrap();
    assert!(!app.session.is_pointer_locked());
    assert_eq!(
        app.session.take_pointer_lock_transition(),
        Some(PointerLockTransition::Release)
    );
    app.dispatch_input(WindowEvent::Focused(true));
    app.dispatch_key_input(key("l", true, false), true).unwrap();
    assert!(!app.session.is_pointer_locked());
    assert!(app.session.take_pointer_lock_transition().is_none());
    app.dispatch_key_input(key("l", false, false), false)
        .unwrap();
    assert_eq!(
        evaluate(&mut app, "JSON.stringify(keys)"),
        json!("[[\"keydown\",\"l\",false],[\"keyup\",\"l\",false]]")
    );
    assert_eq!(evaluate(&mut app, "requested"), json!(1));
    app.dispatch_key_input(key("l", true, false), false)
        .unwrap();
    assert!(matches!(
        app.session.take_pointer_lock_transition(),
        Some(PointerLockTransition::Acquire { .. })
    ));
    assert_eq!(evaluate(&mut app, "requested"), json!(2));
}
