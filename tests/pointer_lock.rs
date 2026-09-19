use omoikane::{
    html::TreeBuilder,
    js::{JsRuntime, PointerLockTransition},
};

fn runtime() -> JsRuntime {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<body><div id=target></div><div id=other></div></body>").document(),
    )
    .unwrap();
    runtime.eval("globalThis.target=document.getElementById('target'); globalThis.other=document.getElementById('other'); globalThis.log=[]; document.onpointerlockchange=()=>log.push('change'); document.onpointerlockerror=()=>log.push('error');").unwrap();
    runtime
}

fn check(runtime: &mut JsRuntime, expression: &str) {
    assert!(
        runtime.eval(expression).unwrap().to_boolean(),
        "{expression}"
    );
}

fn gesture(runtime: &mut JsRuntime, body: &str) {
    runtime.eval(&format!("document.addEventListener('keydown',()=>{{{body}}},{{once:true}}); __omoikane_dispatch_keyboard_input('keydown',{{key:'Enter',code:'Enter'}});")).unwrap();
    runtime.run_jobs().unwrap();
}

fn acquisition(runtime: &mut JsRuntime) -> u64 {
    match runtime.take_pointer_lock_transition() {
        Some(PointerLockTransition::Acquire { request_id, .. }) => request_id,
        other => panic!("expected acquisition, got {other:?}"),
    }
}

#[test]
fn pointer_lock_requires_input_and_delivers_document_events_as_tasks() {
    let doc = TreeBuilder::parse("<body><div id=target></div></body>").document();
    let mut runtime = JsRuntime::with_document(doc).unwrap();
    runtime.eval("globalThis.log=[]; document.onpointerlockerror=()=>log.push('error'); document.getElementById('target').requestPointerLock().catch(e=>log.push(e.name));").expect("Pointer Lock API must exist");
    assert_eq!(runtime.eval("log.length").unwrap().as_number(), Some(0.0));
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("log.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "error,NotAllowedError"
    );
}

#[test]
fn deferred_host_owns_success_and_rejection() {
    let mut runtime = runtime();
    runtime.set_pointer_lock_deferred(true);
    gesture(
        &mut runtime,
        "target.requestPointerLock().then(()=>log.push('resolved'),e=>log.push(e.name))",
    );
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "document.pointerLockElement===null && log.length===0",
    );
    let id = acquisition(&mut runtime);
    assert!(runtime.take_pointer_lock_transition().is_none());
    boa_gc::force_collect();
    assert!(runtime.complete_pointer_lock_request(id, false).unwrap());
    check(&mut runtime, "log.length===0");
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "document.pointerLockElement===null && log.join(',')==='error,NotSupportedError'",
    );
    gesture(
        &mut runtime,
        "log=[]; target.requestPointerLock().then(()=>log.push('resolved'))",
    );
    let id = acquisition(&mut runtime);
    assert!(runtime.complete_pointer_lock_request(id, true).unwrap());
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "document.pointerLockElement===target && log.join(',')==='change,resolved'",
    );
    assert!(!runtime.complete_pointer_lock_request(id, true).unwrap());
}

#[test]
fn explicit_exit_allows_reacquisition_but_escape_requires_new_input() {
    let mut runtime = runtime();
    gesture(&mut runtime, "target.requestPointerLock()");
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "document.pointerLockElement===target && document.exitPointerLock()===undefined && document.pointerLockElement===null",
    );
    runtime.run_until_idle().unwrap();
    runtime.eval("other.requestPointerLock()").unwrap();
    runtime.run_until_idle().unwrap();
    check(&mut runtime, "document.pointerLockElement===other");
    runtime.eval("document.addEventListener('keydown', e=>e.preventDefault()); __omoikane_dispatch_keyboard_input('keydown',{key:'Escape'});").unwrap();
    runtime.run_until_idle().unwrap();
    check(&mut runtime, "document.pointerLockElement===null");
    runtime
        .eval("log=[]; target.requestPointerLock().catch(e=>log.push(e.name))")
        .unwrap();
    runtime.run_until_idle().unwrap();
    check(&mut runtime, "log.join(',')==='error,NotAllowedError'");
}

#[test]
fn removal_releases_lock_and_cancels_an_inflight_acquisition() {
    for removal in [
        "target.remove()",
        "document.body.textContent=''",
        "document.body.innerHTML=''",
        "other.appendChild(target)",
    ] {
        let mut runtime = runtime();
        gesture(&mut runtime, "target.requestPointerLock()");
        runtime.run_until_idle().unwrap();
        runtime.eval(removal).unwrap();
        check(&mut runtime, "document.pointerLockElement===null");
        runtime.run_until_idle().unwrap();
        check(&mut runtime, "log.join(',')==='change,change'");
    }
    let mut runtime = runtime();
    runtime.set_pointer_lock_deferred(true);
    gesture(
        &mut runtime,
        "target.requestPointerLock().catch(e=>log.push(e.name))",
    );
    let id = acquisition(&mut runtime);
    runtime.eval("target.remove()").unwrap();
    assert!(runtime.complete_pointer_lock_request(id, true).unwrap());
    assert_eq!(
        runtime.take_pointer_lock_transition(),
        Some(PointerLockTransition::Release)
    );
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "document.pointerLockElement===null && log.join(',')==='error,WrongDocumentError'",
    );
}

#[test]
fn shadow_root_exposure_retargets_and_rejects_unrelated_trees() {
    let mut runtime = runtime();
    runtime.eval("globalThis.root=target.attachShadow({mode:'closed'}); globalThis.unrelated=other.attachShadow({mode:'open'}); globalThis.inner=document.createElement('span'); root.appendChild(inner)").unwrap();
    gesture(&mut runtime, "inner.requestPointerLock()");
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "document.pointerLockElement===target && root.pointerLockElement===inner && unrelated.pointerLockElement===null",
    );
    runtime.eval("target.remove()").unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "document.pointerLockElement===null && root.pointerLockElement===null",
    );
}

#[test]
fn capture_is_released_and_synthetic_mouse_events_keep_their_values() {
    let mut runtime = runtime();
    runtime.eval("other.setPointerCapture(1); other.addEventListener('lostpointercapture',e=>log.push('lost:'+e.pointerId));").unwrap();
    gesture(&mut runtime, "target.requestPointerLock()");
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "!other.hasPointerCapture(1) && log.join(',')==='lost:1,change'",
    );
    check(
        &mut runtime,
        "(()=>{try {other.setPointerCapture(1);return false} catch(e){return e.name==='InvalidStateError'}})()",
    );
    check(
        &mut runtime,
        "(()=>{const e=new MouseEvent('mousedown',{movementX:12.5,movementY:-9}); e.movementX=3; return e.movementX===12.5 && e.movementY===-9 && new MouseEvent('mousemove').movementX===0})()",
    );
    runtime.eval("other.addEventListener('mousemove', e=>log.push('synthetic:'+e.movementX)); other.dispatchEvent(new MouseEvent('mousemove',{movementX:4}))").unwrap();
    check(&mut runtime, "log[log.length-1]==='synthetic:4'");
}

#[test]
fn focus_loss_cancels_pending_requests_and_ignores_late_completion() {
    let mut runtime = runtime();
    runtime.set_pointer_lock_deferred(true);
    gesture(
        &mut runtime,
        "target.requestPointerLock().catch(e=>log.push(e.name))",
    );
    let id = acquisition(&mut runtime);
    runtime.set_pointer_lock_focus(false).unwrap();
    assert_eq!(
        runtime.take_pointer_lock_transition(),
        Some(PointerLockTransition::Release)
    );
    assert!(!runtime.complete_pointer_lock_request(id, true).unwrap());
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "document.pointerLockElement===null && log.join(',')==='error,AbortError'",
    );
    runtime.set_pointer_lock_focus(true).unwrap();
    gesture(&mut runtime, "log=[]; target.requestPointerLock()");
    let id = acquisition(&mut runtime);
    runtime.complete_pointer_lock_request(id, true).unwrap();
    runtime.run_until_idle().unwrap();
    runtime.set_pointer_lock_focus(false).unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "document.pointerLockElement===null && log.join(',')==='change,change'",
    );
}

#[test]
fn optional_raw_movement_rejection_preserves_existing_lock() {
    let mut runtime = runtime();
    gesture(&mut runtime, "target.requestPointerLock()");
    runtime.run_until_idle().unwrap();
    gesture(
        &mut runtime,
        "log=[]; other.requestPointerLock({unadjustedMovement:true}).catch(e=>log.push(e.name))",
    );
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "document.pointerLockElement===target && log.join(',')==='error,NotSupportedError'",
    );
}

#[test]
fn fullscreen_consumes_shared_activation_in_call_order() {
    for pointer_first in [true, false] {
        let mut runtime = runtime();
        let body = if pointer_first {
            "target.requestPointerLock(); target.requestFullscreen();"
        } else {
            "target.requestFullscreen(); target.requestPointerLock().catch(e=>log.push(e.name));"
        };
        gesture(&mut runtime, body);
        runtime.run_until_idle().unwrap();
        check(&mut runtime, "document.fullscreenElement===target");
        check(
            &mut runtime,
            if pointer_first {
                "document.pointerLockElement===target"
            } else {
                "document.pointerLockElement===null && log.join(',')==='error,NotAllowedError'"
            },
        );
    }
}

#[test]
fn sandbox_permission_is_captured_at_navigation_and_inherited() {
    for allowed in [false, true] {
        let mut runtime = runtime();
        let sandbox = if allowed {
            "allow-scripts allow-same-origin allow-pointer-lock"
        } else {
            "allow-scripts allow-same-origin"
        };
        runtime.eval(&format!("globalThis.frame=document.createElement('iframe'); frame.setAttribute('sandbox',{sandbox:?}); document.body.appendChild(frame); globalThis.child=frame.contentDocument; globalThis.childTarget=child.createElement('div'); child.body.appendChild(childTarget); child.onpointerlockchange=()=>log.push('child-change'); child.onpointerlockerror=()=>log.push('child-error');")).unwrap();
        runtime.run_until_idle().unwrap();
        // Relaxing the attribute does not alter the current document policy.
        if !allowed {
            runtime
                .eval("frame.setAttribute('sandbox','allow-scripts allow-same-origin allow-pointer-lock')")
                .unwrap();
        }
        gesture(
            &mut runtime,
            "childTarget.requestPointerLock().catch(e=>log.push(e.name))",
        );
        runtime.run_until_idle().unwrap();
        if allowed {
            check(
                &mut runtime,
                "child.pointerLockElement===childTarget && document.pointerLockElement===null && log.join(',')==='child-change'",
            );
            runtime.eval("frame.remove()").unwrap();
            assert!(runtime.pointer_lock_target().is_none());
        } else {
            check(
                &mut runtime,
                "child.pointerLockElement===null && log.join(',')==='child-error,SecurityError'",
            );
            runtime.eval("globalThis.nested=child.createElement('iframe'); nested.setAttribute('sandbox','allow-scripts allow-same-origin allow-pointer-lock'); child.body.appendChild(nested); globalThis.deep=nested.contentDocument; globalThis.deepTarget=deep.createElement('div'); deep.body.appendChild(deepTarget); log=[];").unwrap();
            gesture(
                &mut runtime,
                "deepTarget.requestPointerLock().catch(e=>log.push(e.name))",
            );
            runtime.run_until_idle().unwrap();
            check(
                &mut runtime,
                "deep.pointerLockElement===null && log.join(',')==='SecurityError'",
            );
        }
    }
}

#[test]
fn serialized_requests_retarget_with_no_second_native_grab() {
    let mut runtime = runtime();
    runtime.set_pointer_lock_deferred(true);
    gesture(
        &mut runtime,
        "target.requestPointerLock().then(()=>log.push('first')); other.requestPointerLock().then(()=>log.push('second'));",
    );
    let id = acquisition(&mut runtime);
    assert!(runtime.take_pointer_lock_transition().is_none());
    runtime.complete_pointer_lock_request(id, true).unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_pointer_lock_transition().is_none());
    check(
        &mut runtime,
        "document.pointerLockElement===other && log.join(',')==='change,first,change,second'",
    );
    runtime
        .eval("try {target.removeChild(other)} catch(e) {}")
        .unwrap();
    check(&mut runtime, "document.pointerLockElement===other");
}

fn cdp_eval(session: &mut omoikane::cdp::CdpSession, expression: &str) -> serde_json::Value {
    let result = session
        .dispatch(
            "Runtime.evaluate",
            serde_json::json!({"expression":expression}),
        )
        .unwrap();
    assert!(result.get("exceptionDetails").is_none(), "{result}");
    result["result"]["value"].clone()
}

#[test]
fn platform_relative_input_freezes_coordinates_and_routes_outside_hit_test() {
    use omoikane::{
        cdp::CdpSession,
        platform_input::{PlatformInput, PlatformMouseButton},
    };
    let mut session = CdpSession::new().unwrap();
    cdp_eval(
        &mut session,
        "document.body.innerHTML='<div id=target></div>'; globalThis.target=document.getElementById('target'); globalThis.moves=[]; globalThis.buttons=[]; target.onmousemove=e=>moves.push([e.clientX,e.clientY,e.screenX,e.screenY,e.movementX,e.movementY]); target.onmousedown=e=>buttons.push(e.movementX); document.addEventListener('keydown',()=>target.requestPointerLock(),{once:true});",
    );
    let mut input = PlatformInput::new();
    input.cursor_moved(&mut session, 15.0, 25.0).unwrap();
    session
        .dispatch(
            "Input.dispatchKeyEvent",
            serde_json::json!({"type":"keyDown","key":"Enter","code":"Enter"}),
        )
        .unwrap();
    cdp_eval(&mut session, "0");
    assert!(session.is_pointer_locked());
    input
        .relative_motion(&mut session, 1500.5, -2300.25)
        .unwrap();
    input.cursor_moved(&mut session, 99.0, 88.0).unwrap(); // Duplicate absolute OS event is ignored.
    input.relative_motion(&mut session, -7.25, 8.5).unwrap();
    input
        .mouse_button(&mut session, PlatformMouseButton::Left, true)
        .unwrap();
    input
        .mouse_button(&mut session, PlatformMouseButton::Left, false)
        .unwrap();
    assert_eq!(
        cdp_eval(&mut session, "JSON.stringify(moves)"),
        serde_json::json!("[[15,25,15,25,1500.5,-2300.25],[15,25,15,25,-7.25,8.5]]")
    );
    assert_eq!(
        cdp_eval(&mut session, "JSON.stringify(buttons)"),
        serde_json::json!("[0]")
    );
    assert!(session.is_pointer_locked()); // Mouseup does not release pointer lock.
    input.focus_changed(&mut session, false).unwrap();
    assert!(!session.is_pointer_locked());
    input.relative_motion(&mut session, 300.0, 400.0).unwrap();
    assert_eq!(cdp_eval(&mut session, "moves.length"), serde_json::json!(2));
}

#[test]
fn navigation_releases_native_grab_and_rejects_stale_acknowledgement() {
    let mut session = omoikane::cdp::CdpSession::new().unwrap();
    session.set_pointer_lock_deferred(true);
    cdp_eval(
        &mut session,
        "document.addEventListener('keydown',()=>document.body.requestPointerLock().catch(()=>{}),{once:true}); __omoikane_dispatch_keyboard_input('keydown',{key:'Enter'});",
    );
    let Some(PointerLockTransition::Acquire { request_id, .. }) =
        session.take_pointer_lock_transition()
    else {
        panic!("pending host request")
    };
    session
        .dispatch(
            "Page.navigate",
            serde_json::json!({"url":"data:text/html,<body>next"}),
        )
        .unwrap();
    assert_eq!(
        session.take_pointer_lock_transition(),
        Some(PointerLockTransition::Release)
    );
    assert!(
        !session
            .complete_pointer_lock_request(request_id, true)
            .unwrap()
    );
    assert!(!session.is_pointer_locked());
    // The replacement runtime must retain the external-host configuration.
    cdp_eval(
        &mut session,
        "document.addEventListener('keydown',()=>document.body.requestPointerLock(),{once:true}); __omoikane_dispatch_keyboard_input('keydown',{key:'Enter'});",
    );
    assert!(!session.is_pointer_locked());
    let Some(PointerLockTransition::Acquire {
        request_id: next, ..
    }) = session.take_pointer_lock_transition()
    else {
        panic!("new host request")
    };
    assert_ne!(request_id, next);
    session.complete_pointer_lock_request(next, true).unwrap();
    assert!(session.is_pointer_locked());
    session
        .dispatch(
            "Page.navigate",
            serde_json::json!({"url":"data:text/html,<body>third"}),
        )
        .unwrap();
    assert_eq!(
        session.take_pointer_lock_transition(),
        Some(PointerLockTransition::Release)
    );
    assert!(!session.is_pointer_locked());
}

#[test]
fn unlocked_movement_resets_at_surface_and_lock_boundaries() {
    use omoikane::{cdp::CdpSession, platform_input::PlatformInput};
    let mut session = CdpSession::new().unwrap();
    cdp_eval(
        &mut session,
        "globalThis.moves=[]; document.addEventListener('mousemove',e=>moves.push([e.movementX,e.movementY]));",
    );
    let mut input = PlatformInput::new();
    input.cursor_moved(&mut session, 10.0, 20.0).unwrap();
    input.cursor_moved(&mut session, 12.5, 17.0).unwrap();
    input.cursor_left(&mut session);
    input.cursor_moved(&mut session, 100.0, 200.0).unwrap();
    cdp_eval(
        &mut session,
        "document.addEventListener('keydown',()=>document.body.requestPointerLock(),{once:true}); __omoikane_dispatch_keyboard_input('keydown',{key:'Enter'});",
    );
    input
        .relative_motion(&mut session, 3000.0, -4000.0)
        .unwrap();
    cdp_eval(&mut session, "document.exitPointerLock()");
    input.cursor_moved(&mut session, 120.0, 220.0).unwrap();
    assert_eq!(
        cdp_eval(&mut session, "JSON.stringify(moves)"),
        serde_json::json!("[[0,0],[2.5,-3],[0,0],[3000,-4000],[0,0]]")
    );
}

#[test]
fn background_navigation_preserves_focus_and_requires_fresh_input() {
    let mut session = omoikane::cdp::CdpSession::new().unwrap();
    session.set_pointer_lock_focus(false).unwrap();
    session
        .dispatch(
            "Page.navigate",
            serde_json::json!({"url":"data:text/html,<body>background"}),
        )
        .unwrap();
    cdp_eval(
        &mut session,
        "globalThis.denied=''; document.addEventListener('keydown',()=>document.body.requestPointerLock().catch(e=>denied=e.name),{once:true}); __omoikane_dispatch_keyboard_input('keydown',{key:'Enter'});",
    );
    assert_eq!(
        cdp_eval(&mut session, "denied"),
        serde_json::json!("WrongDocumentError")
    );
    assert!(!session.is_pointer_locked());
}

#[test]
fn pointer_lock_notifications_are_nonbubbling_and_use_internal_dispatch() {
    let mut runtime = runtime();
    runtime.eval("document.onpointerlockerror=e=>log.push([e.target===document,e.bubbles,e.cancelable,e.composed,e.eventPhase===Event.AT_TARGET].join(':')); window.addEventListener('pointerlockerror',()=>log.push('bubbled')); document.dispatchEvent=()=>{throw new Error('page override')}; target.requestPointerLock().catch(e=>log.push(e.name));").unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "log.join(',')==='true:false:false:false:true,NotAllowedError' && Event.NONE===0 && Event.CAPTURING_PHASE===1 && Event.AT_TARGET===2 && Event.BUBBLING_PHASE===3 && Event.prototype.NONE===0",
    );
}

#[test]
fn child_realm_completion_and_other_document_exclusion() {
    let mut runtime = runtime();
    runtime.eval("globalThis.frame=document.createElement('iframe'); frame.srcdoc='<body tabindex=0>'; document.body.appendChild(frame);").unwrap();
    runtime.run_until_idle().unwrap();
    runtime.eval(r#"globalThis.childScript=frame.contentDocument.createElement('script');
        childScript.textContent="globalThis.childLog=[]; document.onpointerlockchange=()=>childLog.push(document.pointerLockElement===document.body?'locked':'unlocked'); document.body.onkeydown=()=>document.body.requestPointerLock().then(()=>childLog.push('resolved'));";
        frame.contentDocument.body.appendChild(childScript);"#).unwrap();
    runtime.run_until_idle().unwrap();
    check(&mut runtime, "Array.isArray(frame.contentWindow.childLog)");
    runtime.eval("frame.contentDocument.body.focus(); frame.contentWindow.__omoikane_dispatch_keyboard_input('keydown',{key:'Enter'});").unwrap();
    runtime.run_until_idle().unwrap();
    let observed = runtime.eval("JSON.stringify([frame.contentDocument.pointerLockElement===frame.contentDocument.body,frame.contentWindow.childLog,document.pointerLockElement===null])").unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(
        observed,
        "[true,[\"locked\",\"resolved\"],true]",
        "task errors: {:?}",
        runtime.take_task_errors()
    );
    // A different document cannot steal an existing lock, even with input.
    runtime
        .eval("target.setAttribute('tabindex','0'); target.focus()")
        .unwrap();
    check(&mut runtime, "document.activeElement===target");
    gesture(
        &mut runtime,
        "target.requestPointerLock().catch(e=>log.push(e.name))",
    );
    runtime.run_until_idle().unwrap();
    let excluded = runtime.eval("JSON.stringify([log,frame.contentDocument.pointerLockElement===frame.contentDocument.body,document.pointerLockElement===target])").unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(
        excluded,
        "[[\"error\",\"InvalidStateError\"],true,false]",
        "task errors: {:?}",
        runtime.take_task_errors()
    );
    runtime.eval("frame.srcdoc='<body>replacement'").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.pointer_lock_target().is_none());
    check(
        &mut runtime,
        "frame.contentDocument.pointerLockElement===null",
    );
}

#[test]
fn host_keyboard_input_reaches_child_realm_listener() {
    let mut runtime = runtime();
    runtime.eval("globalThis.frame=document.createElement('iframe'); frame.srcdoc='<body tabindex=0>'; document.body.appendChild(frame)").unwrap();
    runtime.run_until_idle().unwrap();
    runtime.eval("const script=frame.contentDocument.createElement('script'); script.textContent=\"globalThis.keys=0; document.body.onkeydown=()=>keys++;\"; frame.contentDocument.body.appendChild(script)").unwrap();
    runtime.run_until_idle().unwrap();
    check(&mut runtime, "frame.contentWindow.keys===0");
    runtime.eval("frame.contentDocument.body.focus(); __omoikane_dispatch_keyboard_input('keydown',{key:'Enter'})").unwrap();
    runtime.run_until_idle().unwrap();
    check(&mut runtime, "frame.contentWindow.keys===1");
}

fn child_input_session(script: &str) -> omoikane::cdp::CdpSession {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let body = "<!doctype html><body></body>";
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
    });
    let mut session = omoikane::cdp::CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", serde_json::json!({"url":url}))
        .unwrap();
    server.join().unwrap();
    cdp_eval(
        &mut session,
        "document.body.innerHTML='<input id=topField><iframe id=frame></iframe>'; globalThis.frame=document.getElementById('frame'); globalThis.child=frame.contentDocument; child.body.innerHTML='<input id=field><div id=lock></div>'; globalThis.field=child.getElementById('field'); globalThis.order=[];",
    );
    cdp_eval(
        &mut session,
        &format!(
            "const script=child.createElement('script'); script.textContent={}; child.body.appendChild(script)",
            serde_json::to_string(script).unwrap()
        ),
    );
    session
}

#[test]
fn cdp_child_listener_order_cancellation_and_focus_return() {
    use omoikane::platform_input::{PlatformImeEvent, PlatformInput, PlatformKeyEvent};
    let mut session = child_input_session(
        "globalThis.events=[]; globalThis.field=document.getElementById('field'); field.addEventListener('keydown', e=>{parent.order.push('child:'+e.key+':'+(e.target===field)+':'+(e instanceof KeyboardEvent)); events.push(e.key)}); field.focus();",
    );
    // Parent and child registrations must form one ordered listener list.
    cdp_eval(
        &mut session,
        "field.addEventListener('keydown',e=>{order.push('parent:'+e.key);if(e.key==='x'){e.preventDefault();e.stopImmediatePropagation()}}); document.getElementById('topField').onkeydown=e=>order.push('top:'+e.key);",
    );
    cdp_eval(
        &mut session,
        "const extra=child.createElement('script'); extra.textContent=\"field.addEventListener('keydown',e=>parent.order.push('after:'+e.key)); field.addEventListener('input',()=>parent.order.push('input:'+field.value));\"; child.body.appendChild(extra)",
    );
    let mut input = PlatformInput::new();
    // A parent callback must keep its global environment when called by a child.
    assert_eq!(
        cdp_eval(
            &mut session,
            "globalThis.parentCallback=()=>order.length; frame.contentWindow.eval('parent.parentCallback()')"
        ),
        serde_json::json!(0)
    );
    for key in ["x", "y"] {
        input
            .key_event(
                &mut session,
                PlatformKeyEvent {
                    pressed: true,
                    key: key.into(),
                    code: format!("Key{}", key.to_uppercase()),
                    text: Some(key.into()),
                    repeat: false,
                },
            )
            .unwrap();
    }
    assert_eq!(
        cdp_eval(&mut session, "field.value"),
        serde_json::json!("y")
    );
    assert_eq!(
        cdp_eval(&mut session, "order.join('|')"),
        serde_json::json!("child:x:true:true|parent:x|child:y:true:true|parent:y|after:y|input:y")
    );
    input
        .ime_event(&mut session, PlatformImeEvent::Commit("語".into()))
        .unwrap();
    assert_eq!(
        cdp_eval(&mut session, "field.value"),
        serde_json::json!("y語")
    );
    cdp_eval(
        &mut session,
        "document.getElementById('topField').focus(); order=[]",
    );
    input
        .key_event(
            &mut session,
            PlatformKeyEvent {
                pressed: true,
                key: "z".into(),
                code: "KeyZ".into(),
                text: Some("z".into()),
                repeat: false,
            },
        )
        .unwrap();
    assert_eq!(
        cdp_eval(&mut session, "order.join('|')"),
        serde_json::json!("top:z")
    );
    assert_eq!(
        cdp_eval(&mut session, "field.value"),
        serde_json::json!("y語")
    );
}

#[test]
fn cdp_child_lock_receives_relative_input_and_escape_releases_it() {
    use omoikane::platform_input::{PlatformInput, PlatformKeyEvent, PlatformMouseButton};
    let mut session = child_input_session(
        "globalThis.target=document.getElementById('lock'); globalThis.moves=[]; const field=document.getElementById('field'); field.onlostpointercapture=e=>parent.order.push('lost:'+e.pointerId+':'+(e.target===field)); field.setPointerCapture(1); target.onmousedown=e=>parent.order.push('down:'+e.buttons+':'+(e.target===target)); target.onwheel=e=>{parent.order.push('wheel:'+e.deltaY+':'+(e instanceof WheelEvent));e.preventDefault()}; target.onmousemove=e=>moves.push([e.movementX,e.movementY,e.target===target,e instanceof MouseEvent]); document.onpointerlockchange=()=>parent.order.push(document.pointerLockElement===target?'locked':'unlocked'); document.getElementById('field').onkeydown=e=>{if(e.key==='Enter')target.requestPointerLock().then(()=>parent.order.push('resolved'))}; document.getElementById('field').focus();",
    );
    let mut input = PlatformInput::new();
    input
        .key_event(
            &mut session,
            PlatformKeyEvent {
                pressed: true,
                key: "Enter".into(),
                code: "Enter".into(),
                text: None,
                repeat: false,
            },
        )
        .unwrap();
    cdp_eval(&mut session, "0");
    assert!(session.is_pointer_locked());
    input.relative_motion(&mut session, 920.25, -600.5).unwrap();
    assert_eq!(
        cdp_eval(&mut session, "JSON.stringify(frame.contentWindow.moves)"),
        serde_json::json!("[[920.25,-600.5,true,true]]")
    );
    assert_eq!(
        cdp_eval(&mut session, "order.join('|')"),
        serde_json::json!("lost:1:true|locked|resolved")
    );
    input
        .mouse_button(&mut session, PlatformMouseButton::Left, true)
        .unwrap();
    input
        .mouse_button(&mut session, PlatformMouseButton::Left, false)
        .unwrap();
    input.wheel(&mut session, 0.0, 80.0).unwrap();
    assert_eq!(
        cdp_eval(&mut session, "order.join('|')"),
        serde_json::json!("lost:1:true|locked|resolved|down:1:true|wheel:80:true")
    );
    assert_eq!(
        cdp_eval(&mut session, "field.hasPointerCapture(1)"),
        serde_json::json!(false)
    );
    cdp_eval(
        &mut session,
        "child.addEventListener('keydown',e=>e.preventDefault())",
    );
    input
        .key_event(
            &mut session,
            PlatformKeyEvent {
                pressed: true,
                key: "Escape".into(),
                code: "Escape".into(),
                text: None,
                repeat: false,
            },
        )
        .unwrap();
    assert!(!session.is_pointer_locked());
    cdp_eval(&mut session, "0");
    assert_eq!(
        cdp_eval(&mut session, "order.join('|')"),
        serde_json::json!("lost:1:true|locked|resolved|down:1:true|wheel:80:true|unlocked")
    );
}

#[test]
fn nested_input_navigation_drops_old_dispatcher_and_keeps_parent_focus() {
    let mut session = child_input_session("globalThis.ready=true");
    cdp_eval(
        &mut session,
        "globalThis.nested=child.createElement('iframe'); child.body.appendChild(nested); globalThis.deep=nested.contentDocument; deep.body.innerHTML='<input id=deep>'; document.getElementById('topField').onkeydown=e=>order.push('top:'+e.key);",
    );
    cdp_eval(
        &mut session,
        "const nestedScript=deep.createElement('script'); nestedScript.textContent=\"document.getElementById('deep').onkeydown=e=>top.order.push('deep:'+e.key); document.getElementById('deep').focus()\"; deep.body.appendChild(nestedScript);",
    );
    session
        .dispatch(
            "Input.dispatchKeyEvent",
            serde_json::json!({"type":"keyDown","key":"k","code":"KeyK"}),
        )
        .unwrap();
    assert_eq!(
        cdp_eval(&mut session, "order.join('|')"),
        serde_json::json!("deep:k")
    );
    assert_eq!(
        cdp_eval(
            &mut session,
            "document.activeElement===frame && child.activeElement===nested"
        ),
        serde_json::json!(true)
    );
    cdp_eval(
        &mut session,
        "globalThis.oldInput=nested.contentWindow.__omoikane_dispatch_keyboard_input; nested.srcdoc='<p>new generation</p>'; void nested.contentDocument; document.getElementById('topField').focus();",
    );
    session
        .dispatch(
            "Input.dispatchKeyEvent",
            serde_json::json!({"type":"keyDown","key":"z","code":"KeyZ"}),
        )
        .unwrap();
    cdp_eval(&mut session, "oldInput('keydown',{key:'stale'})");
    assert_eq!(
        cdp_eval(&mut session, "order.join('|')"),
        serde_json::json!("deep:k|top:z")
    );
}
