//! Contracts retained when host input stops looking up page globals.

use super::*;

#[test]
fn cdp_keyboard_uses_captured_dispatcher_when_page_replaces_globals() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/",
            "<html><body><input id='target'></body></html>",
            1,
            "null",
        )
        .unwrap();
    session
        .runtime
        .eval(
            r#"
            globalThis.privateInputTrace = [];
            const control = document.getElementById('target');
            control.focus();
            control.addEventListener('keydown', event => {
                privateInputTrace.push('keydown:' + event.isTrusted);
                queueMicrotask(() => privateInputTrace.push('job'));
            });
            for (const name of [
                '__omoikane_dispatch_keyboard_input',
                '__omoikane_dispatch_mouse_input',
                '__omoikane_dispatch_wheel_input',
                '__omoikane_dispatch_touch_input',
                '__omoikane_dispatch_composition_input',
                '__omoikane_prepare_drag_input',
                '__omoikane_dispatch_drag_input',
                '__omoikane_release_pointer_capture',
            ]) {
                globalThis[name] = () => { throw new Error('page input replacement'); };
            }
            "#,
        )
        .unwrap();
    let response = session
        .dispatch(
            "Input.dispatchKeyEvent",
            json!({"type":"keyDown","key":"x","code":"KeyX","text":"x"}),
        )
        .unwrap();
    assert_eq!(response, json!({"defaultPrevented":false}));
    let snapshot = session
        .runtime
        .eval("JSON.stringify({trace:privateInputTrace,value:document.getElementById('target').value})")
        .unwrap();
    let snapshot: Value =
        serde_json::from_str(&snapshot.as_string().unwrap().to_std_string_escaped()).unwrap();
    assert_eq!(
        snapshot,
        json!({"trace":["keydown:true","job"],"value":"x"})
    );
}

fn boundary_snapshot(session: &mut CdpSession) -> Value {
    let value = session
        .runtime
        .eval(
            "JSON.stringify({trace:inputBoundaryTrace, trusted:inputBoundaryTrusted, \
             active:inputBoundaryActive, value:document.getElementById('target').value, \
             selection:document.getElementById('target').selectionStart})",
        )
        .unwrap();
    serde_json::from_str(&value.as_string().unwrap().to_std_string_escaped()).unwrap()
}

#[test]
fn synchronous_key_dialog_preserves_the_existing_input_boundary() {
    for (kind, call) in [
        ("alert", "alert('input boundary')"),
        ("confirm", "confirm('input boundary')"),
        ("prompt", "prompt('input boundary', 'default')"),
    ] {
        let mut session = CdpSession::new().unwrap();
        session
            .install_document(
                "https://example.test/",
                "<html><body><input id='target' value='ab'></body></html>",
                1,
                "null",
            )
            .unwrap();
        let setup = r#"
            globalThis.inputBoundaryTrace = [];
            globalThis.inputBoundaryTrusted = null;
            globalThis.inputBoundaryActive = null;
            const control = document.getElementById('target');
            control.focus();
            control.setSelectionRange(1, 1);
            control.addEventListener('keydown', event => {
                inputBoundaryTrace.push('before');
                inputBoundaryTrusted = event.isTrusted;
                inputBoundaryActive = navigator.userActivation.isActive;
                queueMicrotask(() => inputBoundaryTrace.push('microtask'));
                DIALOG_CALL;
                inputBoundaryTrace.push('after');
            });
        "#
        .replace("DIALOG_CALL", call);
        session.runtime.eval(&setup).unwrap();
        let result = session.dispatch(
            "Input.dispatchKeyEvent",
            json!({"type":"keyDown", "key":"Backspace", "code":"Backspace",
                   "windowsVirtualKeyCode":8}),
        );
        // Capture before any eval: eval itself clears rejected dialog metadata.
        let pending = session.runtime.pending_javascript_dialog();
        let response = match &result {
            Ok(value) => json!({"ok":value}),
            Err(error) => json!({"error_code":error.code, "message":error.message}),
        };
        let immediate = boundary_snapshot(&mut session);
        session.runtime.run_jobs().unwrap();
        let after_jobs = boundary_snapshot(&mut session);
        eprintln!(
            "INPUT_SYNC_BOUNDARY {}",
            json!({"dialog":kind, "response":response, "pending":pending.is_some(),
                   "immediate":immediate, "after_explicit_jobs":after_jobs}),
        );
        assert_eq!(pending, None, "{kind} left a rejected synchronous dialog");
        assert_eq!(response, json!({"ok":{"defaultPrevented":false}}));
        assert_eq!(
            immediate,
            json!({"trace":["before","microtask"], "trusted":true,
                   "active":true,"value":"b","selection":0}),
        );
        assert_eq!(after_jobs, immediate);
        assert_eq!(
            session.runtime.eval("6 * 7").unwrap().as_number(),
            Some(42.0)
        );
    }
}

#[test]
fn synchronous_input_time_hook_error_preserves_cleanup_and_skips_jobs() {
    let mut session = CdpSession::new().unwrap();
    session
        .install_document(
            "https://example.test/",
            "<html><body><input id='target' value='ab'></body></html>",
            1,
            "null",
        )
        .unwrap();
    session
        .runtime
        .eval(
            r#"
            globalThis.inputTimeHookReached = false;
            globalThis.inputTimeHookJob = false;
            globalThis.inputTimeHookListener = false;
            const control = document.getElementById('target');
            control.focus();
            control.setSelectionRange(1, 1);
            control.addEventListener('keydown', () => inputTimeHookListener = true);
            queueMicrotask(() => inputTimeHookJob = true);
            globalThis.inputTimeHookSavedNow = Date.now;
            Date.now = function () {
                Date.now = inputTimeHookSavedNow;
                inputTimeHookReached = true;
                alert('input time hook');
            };
            "#,
        )
        .unwrap();
    let result = session.dispatch(
        "Input.dispatchKeyEvent",
        json!({"type":"keyDown", "key":"Backspace", "code":"Backspace",
               "windowsVirtualKeyCode":8}),
    );
    // Follow-up eval also clears modal metadata, so inspect the boundary first.
    let pending = session.runtime.pending_javascript_dialog();
    let response = match &result {
        Ok(value) => json!({"ok":value}),
        Err(error) => json!({"error_code":error.code,"message":error.message}),
    };
    let snapshot = session
        .runtime
        .eval(
            "Date.now = inputTimeHookSavedNow; \
             JSON.stringify({reached:inputTimeHookReached,job:inputTimeHookJob, \
             listener:inputTimeHookListener,value:document.getElementById('target').value})",
        )
        .unwrap();
    let snapshot: Value =
        serde_json::from_str(&snapshot.as_string().unwrap().to_std_string_escaped()).unwrap();
    eprintln!(
        "INPUT_SYNC_TIME_HOOK {}",
        json!({"response":response,"pending":pending.is_some(),"immediate":snapshot}),
    );
    assert!(result.is_err());
    assert_eq!(pending, None);
    assert_eq!(
        snapshot,
        json!({"reached":true,"job":false,"listener":false,"value":"ab"}),
    );
    session.runtime.run_jobs().unwrap();
    assert_eq!(
        session
            .runtime
            .eval("inputTimeHookJob")
            .unwrap()
            .as_boolean(),
        Some(true),
    );
}
