//! A return pending across `finally` survives an inner return discarded by break.

use omoikane::js::JsRuntime;

#[test]
fn nested_finally_break_preserves_outer_return() {
    let mut runtime = JsRuntime::new().expect("runtime initialization");
    assert_pending_return_behavior(&mut runtime);
}

#[cfg(feature = "baseline-jit")]
#[test]
fn pending_return_matches_with_jit_enabled_and_disabled() {
    for enabled in [false, true] {
        let mut runtime = JsRuntime::new().expect("runtime initialization");
        runtime.set_baseline_jit_enabled(enabled);
        assert_pending_return_behavior(&mut runtime);
    }
}

fn assert_pending_return_behavior(runtime: &mut JsRuntime) {
    for (source, expected) in [
        (
            "(() => { function f() { try { return 42; } finally { do try { return 43; } finally { break; } while (0); } } return f(); })()",
            42.0,
        ),
        (
            "(() => { function f() { try { return 42; } finally { return 43; } } return f(); })()",
            43.0,
        ),
        (
            "(() => { function* f() { try { return 42; } finally { do try { return 43; } finally { break; } while (0); } } return f().next().value; })()",
            42.0,
        ),
    ] {
        assert_eq!(
            runtime.eval(source).expect("valid JavaScript").as_number(),
            Some(expected),
            "{source}"
        );
    }
}
