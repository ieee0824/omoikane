//! Deletion of bindings introduced by direct eval through the browser runtime.

use omoikane::js::JsRuntime;

#[test]
fn direct_eval_bindings_are_deletable_but_regular_locals_are_not() {
    let mut runtime = JsRuntime::new().expect("runtime initialization");
    assert_eval_delete_behavior(&mut runtime);
}

fn assert_eval_delete_behavior(runtime: &mut JsRuntime) {
    for source in [
        "(function() {
            var local = 1;
            var deletedLocal = delete local;
            eval('var x; delete x;');
            var missing = false;
            try { x; } catch (error) { missing = error instanceof ReferenceError; }
            return !deletedLocal && local === 1 && missing;
        }())",
        "(function() {
            eval('function f() {}; delete f;');
            eval('function f() { return 2; }');
            return f() === 2 && typeof globalThis.f === 'undefined';
        }())",
    ] {
        assert_eq!(
            runtime.eval(source).expect("valid JavaScript").as_boolean(),
            Some(true),
            "{source}"
        );
    }
}

#[cfg(feature = "baseline-jit")]
#[test]
fn direct_eval_delete_matches_with_jit_enabled_and_disabled() {
    for enabled in [false, true] {
        let mut runtime = JsRuntime::new().expect("runtime initialization");
        runtime.set_baseline_jit_enabled(enabled);
        assert_eval_delete_behavior(&mut runtime);
    }
}
