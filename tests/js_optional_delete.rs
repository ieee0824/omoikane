//! Optional-chain deletion through the browser JavaScript runtime.

use omoikane::js::JsRuntime;

#[test]
fn optional_chain_delete_removes_properties_and_short_circuits() {
    let mut runtime = JsRuntime::new().expect("runtime initialization");
    assert_optional_delete_behavior(&mut runtime);
}

#[cfg(feature = "baseline-jit")]
#[test]
fn optional_chain_delete_matches_with_jit_enabled_and_disabled() {
    for enabled in [false, true] {
        let mut runtime = JsRuntime::new().expect("runtime initialization");
        runtime.set_baseline_jit_enabled(enabled);
        assert_optional_delete_behavior(&mut runtime);
    }
}

fn assert_optional_delete_behavior(runtime: &mut JsRuntime) {
    for source in [
        "(() => { const obj = { value: 1 }; return delete obj?.value && !('value' in obj); })()",
        "(() => { const obj = { child: { value: 1 } }; return delete obj?.child.value && !('value' in obj.child); })()",
        "(() => { const obj = { value: 1 }; let keys = 0; return delete obj?.[(keys++, 'value')] && keys === 1 && !('value' in obj); })()",
        "(() => { const obj = null; let keys = 0; return delete obj?.[keys++] && keys === 0; })()",
        "(() => { const obj = { child: null }; let keys = 0; return delete obj?.child?.[keys++] && keys === 0; })()",
        "(() => { const obj = { value: 1 }; return delete (obj?.value) && !('value' in obj); })()",
        "(() => { const obj = { child: { value: 1 }, get() { return this.child; } }; return delete obj?.get().value && !('value' in obj.child); })()",
        "(() => { const obj = {}; Object.defineProperty(obj, 'value', { value: 1 }); return !delete obj?.value && 'value' in obj; })()",
        "(() => { let calls = 0; const obj = { method() { calls++; } }; return delete obj?.method() && calls === 1; })()",
        "(() => { const obj = { child: null }; try { delete obj?.child.value; return false; } catch (error) { return error instanceof TypeError; } })()",
    ] {
        assert_eq!(
            runtime.eval(source).expect("valid JavaScript").as_boolean(),
            Some(true),
            "{source}"
        );
    }
}
