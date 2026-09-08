use omoikane::dom::NodeHandle;
use omoikane::js::{JsRuntime, SandboxConfig};

#[test]
fn recursion_depth_probe_catches_range_error_and_continues() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime
        .eval(
            r#"(() => {
        let depth = 0;
        function recurse() { depth++; recurse(); }
        try { recurse(); } catch (error) {
            return error instanceof RangeError && depth > 0;
        }
        return false;
    })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
    assert_eq!(runtime.eval("1 + 1").unwrap().as_number(), Some(2.0));
}

#[test]
fn catching_stack_overflow_cannot_disable_the_loop_limit() {
    let sandbox = SandboxConfig {
        max_loop_iterations: 32,
        ..SandboxConfig::default()
    };
    let mut runtime =
        JsRuntime::with_document_and_sandbox(NodeHandle::document(), sandbox).unwrap();
    let error = runtime
        .eval(
            r#"
        globalThis.caughtLoopLimit = false;
        try {
            for (;;) {
                function recurse() { recurse(); }
                try { recurse(); } catch (error) {
                    if (!(error instanceof RangeError)) throw error;
                }
            }
        } catch (error) { caughtLoopLimit = true; }
    "#,
        )
        .expect_err("the loop limit must escape JavaScript catch handlers");
    assert!(
        error
            .as_native()
            .is_some_and(|error| error.is_runtime_limit())
    );
    assert_eq!(
        runtime.eval("caughtLoopLimit").unwrap().as_boolean(),
        Some(false)
    );
    assert_eq!(runtime.eval("1 + 1").unwrap().as_number(), Some(2.0));
}
