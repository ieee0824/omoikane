//! Promise construction must retain its result while the executor runs JavaScript.

use boa_engine::{Context, JsValue, NativeFunction, Source, js_string};

#[test]
fn constructor_keeps_settled_promise_alive_until_executor_returns() {
    for settlement in ["resolve('retained')", "reject('retained')"] {
        let mut context = Context::default();
        context
            .register_global_builtin_callable(
                js_string!("forceCollect"),
                0,
                NativeFunction::from_fn_ptr(|_, _, _| {
                    boa_gc::force_collect();
                    Ok(JsValue::undefined())
                }),
            )
            .unwrap();

        // Settlement clears the resolver's shared Promise capture. The result
        // is still a native constructor local, before it reaches the VM stack.
        let script = format!(
            "globalThis.result = 'not delivered'; \
             const promise = new Promise((resolve, reject) => {{ \
                 {settlement}; forceCollect(); \
             }}); \
             promise.then(value => result = 'fulfilled:' + value, \
                          reason => result = 'rejected:' + reason);"
        );
        context.eval(Source::from_bytes(&script)).unwrap();
        context.run_jobs().unwrap();
        let expected = if settlement.starts_with("resolve") {
            "fulfilled:retained"
        } else {
            "rejected:retained"
        };
        assert_eq!(
            context.eval(Source::from_bytes("result")).unwrap(),
            JsValue::from(js_string!(expected)),
            "{settlement}"
        );
    }
}
