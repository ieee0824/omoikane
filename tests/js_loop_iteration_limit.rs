//! Omoikane's loop sandbox counts executed bodies, not exit checks.

use std::time::Duration;

use boa_engine::JsNativeError;
use omoikane::{
    dom::NodeHandle,
    js::{JsRuntime, SandboxConfig},
};

const LIMIT: u64 = 3;

#[test]
fn loop_iteration_limit_counts_executed_bodies() {
    assert_loop_limits(false);
}

#[cfg(feature = "baseline-jit")]
#[test]
fn loop_iteration_limit_matches_with_jit_enabled_and_disabled() {
    for enabled in [false, true] {
        assert_loop_limits(enabled);
    }
}

fn assert_loop_limits(jit_enabled: bool) {
    for source in [
        "for (;;) { count++; }",
        "while (true) { count++; }",
        "do { count++; } while (true);",
        "for (const item of [1, 2, 3, 4, 5]) { count++; }",
        "for (const key in {a: 1, b: 1, c: 1, d: 1, e: 1}) { count++; }",
    ] {
        let mut runtime = runtime_with_limit(jit_enabled);
        runtime
            .eval("globalThis.count = 0")
            .expect("counter initialization");

        let error = runtime
            .eval(source)
            .expect_err("fourth body execution must exceed limit");
        assert!(
            error
                .as_native()
                .is_some_and(JsNativeError::is_runtime_limit),
            "{source}: {error}"
        );
        assert_eq!(
            runtime.eval("count").unwrap().as_number(),
            Some(3.0),
            "{source}"
        );
    }

    for source in [
        "for (let i = 0; i < 3; i++) { count++; }",
        "while (count < 3) { count++; }",
        "do { count++; } while (count < 3);",
        "for (const item of [1, 2, 3]) { count++; }",
        "for (const key in {a: 1, b: 1, c: 1}) { count++; }",
    ] {
        let mut runtime = runtime_with_limit(jit_enabled);
        runtime
            .eval("globalThis.count = 0")
            .expect("counter initialization");
        runtime
            .eval(source)
            .unwrap_or_else(|error| panic!("{source}: {error}"));
        assert_eq!(
            runtime.eval("count").unwrap().as_number(),
            Some(3.0),
            "{source}"
        );
    }
}

fn runtime_with_limit(jit_enabled: bool) -> JsRuntime {
    let sandbox = SandboxConfig {
        timeout: Duration::from_secs(5),
        max_loop_iterations: LIMIT,
    };
    let runtime = JsRuntime::with_document_and_sandbox(NodeHandle::document(), sandbox)
        .expect("runtime initialization");
    #[cfg(feature = "baseline-jit")]
    {
        let mut runtime = runtime;
        runtime.set_baseline_jit_enabled(jit_enabled);
        runtime
    }
    #[cfg(not(feature = "baseline-jit"))]
    {
        let _ = jit_enabled;
        runtime
    }
}
