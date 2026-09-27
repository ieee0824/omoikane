//! `with` call bindings must preserve the resolved base object and lookup count.

use omoikane::js::JsRuntime;

#[test]
fn with_method_call_uses_resolved_binding_object() {
    let mut runtime = JsRuntime::new().expect("runtime initialization");
    assert_with_binding_behavior(&mut runtime);
}

#[cfg(feature = "baseline-jit")]
#[test]
fn with_method_call_matches_with_jit_enabled_and_disabled() {
    for enabled in [false, true] {
        let mut runtime = JsRuntime::new().expect("runtime initialization");
        runtime.set_baseline_jit_enabled(enabled);
        assert_with_binding_behavior(&mut runtime);
    }
}

fn assert_with_binding_behavior(runtime: &mut JsRuntime) {
    for source in [
        "(() => { let count = 0; let receiver = null; const proxy = new Proxy({ method() { receiver = this; } }, { has(target, key) { if (key === 'method') count++; return Reflect.has(target, key); } }); with (proxy) { method(); } return count === 2 && receiver === proxy; })()",
        "(() => { let count = 0; const proxy = new Proxy({}, { has(target, key) { if (key === 'Object') count++; return Reflect.has(target, key); } }); with (proxy) { Object(); } return count === 1; })()",
        "(() => { let count = 0; const object = { binding: 42, get [Symbol.unscopables]() { count++; delete object.binding; return null; } }; let value = null; with (object) { value = binding; } return count === 1 && value === undefined; })()",
        "(() => { const object = { binding: 42, get [Symbol.unscopables]() { delete object.binding; return null; } }; let threw = false; with (object) { try { (function() { 'use strict'; return binding; })(); } catch (error) { threw = error instanceof ReferenceError; } } return threw; })()",
    ] {
        assert_eq!(
            runtime.eval(source).expect("valid JavaScript").as_boolean(),
            Some(true),
            "{source}"
        );
    }
}
