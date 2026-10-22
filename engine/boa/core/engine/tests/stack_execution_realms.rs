#![allow(unused_crate_dependencies, missing_docs)]

use boa_engine::{
    Context, JsResult, JsValue, NativeFunction, Source, js_string, property::Attribute,
};

fn capture_execution_realms(
    _: &JsValue,
    _: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let frames: Vec<_> = context.stack_trace_with_realms().collect();
    assert_eq!(frames.first().unwrap().1, *context.realm());
    assert!(
        frames
            .iter()
            .skip(1)
            .any(|(_, realm)| realm != context.realm())
    );
    Ok(JsValue::from(true))
}

fn capture_native_cross_realm(
    _: &JsValue,
    _: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    // The native builtin entered its own Realm without adding a VM frame.
    // The closest author frame must still report the caller's execution Realm.
    let frame = context.stack_trace().next().unwrap();
    assert_ne!(frame.execution_realm(), context.realm());
    assert_eq!(
        Some(frame.execution_realm().clone()),
        frame.script_or_module_realm()
    );
    Ok(JsValue::from(true))
}

fn check_execution_realms(_jit: bool) {
    let mut context = Context::default();
    #[cfg(feature = "baseline-jit")]
    context.set_baseline_jit_enabled(_jit);
    let caller = context.realm().clone();
    let callee = context.create_realm().unwrap();
    context.enter_realm(callee);
    context
        .register_global_builtin_callable(
            js_string!("capture"),
            0,
            NativeFunction::from_fn_ptr(capture_execution_realms),
        )
        .unwrap();
    context
        .register_global_builtin_callable(
            js_string!("captureNative"),
            0,
            NativeFunction::from_fn_ptr(capture_native_cross_realm),
        )
        .unwrap();
    let native = context
        .global_object()
        .get(js_string!("captureNative"), &mut context)
        .unwrap();
    let eval = context
        .global_object()
        .get(js_string!("eval"), &mut context)
        .unwrap();
    let function = context
        .eval(Source::from_bytes(
            "(function foreign() { return capture(); })",
        ))
        .unwrap();
    context.enter_realm(caller);
    for (name, value) in [
        ("foreignEval", eval),
        ("foreignFunction", function),
        ("foreignNativeCapture", native),
    ] {
        context
            .register_global_property(js_string!(name), value, Attribute::all())
            .unwrap();
    }
    assert!(
        context
            .eval(Source::from_bytes("foreignFunction()"))
            .unwrap()
            .to_boolean()
    );
    assert!(
        context
            .eval(Source::from_bytes("foreignNativeCapture()"))
            .unwrap()
            .to_boolean()
    );
    assert!(
        context
            .eval(Source::from_bytes("foreignEval('capture()')"))
            .unwrap()
            .to_boolean()
    );
    let generator = context
        .eval(Source::from_bytes(
            "foreignEval('(function* foreignGenerator() { yield capture(); yield capture(); })()')",
        ))
        .unwrap();
    context
        .register_global_property(js_string!("foreignGenerator"), generator, Attribute::all())
        .unwrap();
    for _ in 0..2 {
        // Suspended frames store heap edges instead of active native roots.
        // Keep the execution Realm through both that conversion and a GC.
        boa_gc::force_collect();
        assert!(
            context
                .eval(Source::from_bytes("foreignGenerator.next().value"))
                .unwrap()
                .to_boolean()
        );
    }
}

#[test]
fn execution_realms_follow_cross_realm_calls_eval_and_suspended_generators() {
    check_execution_realms(false);
}

#[cfg(feature = "baseline-jit")]
#[test]
fn execution_realms_with_jit_follow_calls_eval_and_suspended_generators() {
    check_execution_realms(true);
}
