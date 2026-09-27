//! Nested script execution must not inherit the initiating script's scope.

use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn inserted_inline_script_reads_global_not_initiators_with_binding() {
    let document = TreeBuilder::parse("<html><head></head><body></body></html>").document();
    let mut runtime = JsRuntime::with_document(document).expect("runtime initialization");
    assert_script_environment_is_global(&mut runtime);
}

#[cfg(feature = "baseline-jit")]
#[test]
fn inserted_inline_script_uses_global_environment_with_jit_enabled_and_disabled() {
    for enabled in [false, true] {
        let document = TreeBuilder::parse("<html><head></head><body></body></html>").document();
        let mut runtime = JsRuntime::with_document(document).expect("runtime initialization");
        runtime.set_baseline_jit_enabled(enabled);
        assert_script_environment_is_global(&mut runtime);
    }
}

fn assert_script_environment_is_global(runtime: &mut JsRuntime) {
    runtime
        .eval(
            "globalThis.marker = 'global';
             with ({marker: 'with'}) {
                 const script = document.createElement('script');
                 script.textContent = 'globalThis.seen = marker';
                 document.head.appendChild(script);
             }",
        )
        .expect("valid JavaScript");
    runtime.run_until_idle().expect("script tasks");

    assert_eq!(
        runtime
            .eval("globalThis.seen === 'global'")
            .expect("valid JavaScript")
            .as_boolean(),
        Some(true)
    );
}
