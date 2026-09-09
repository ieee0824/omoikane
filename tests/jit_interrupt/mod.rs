//! Gate 4-6: the embedding and generated code share cooperative limits.

use std::time::{Duration, Instant};
use std::{
    future::Future,
    task::{Context as TaskContext, Poll, Waker},
};

use boa_engine::{Context, Source};
use omoikane::{
    html::TreeBuilder,
    js::{JsRuntime, SandboxConfig},
};

fn runtime(enabled: bool, timeout: Duration, iterations: u64) -> JsRuntime {
    let document =
        TreeBuilder::parse("<!doctype html><html><head></head><body></body></html>").document();
    let mut runtime = JsRuntime::with_document_and_sandbox(
        document,
        SandboxConfig {
            timeout,
            max_loop_iterations: iterations,
        },
    )
    .unwrap();
    runtime.set_baseline_jit_enabled(enabled);
    runtime
}

#[test]
fn synchronous_and_asynchronous_jit_loops_share_the_sandbox_timeout() {
    for enabled in [false, true] {
        for asynchronous in [false, true] {
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut runtime = runtime(enabled, Duration::from_millis(100), u64::MAX);
                runtime.eval("function timed(n){let s=1;for(let i=0;i<n;i++)s=(s+i*3)%1000003;return s}timed(200)").unwrap();
                let before = runtime.baseline_jit_diagnostics();
                let source = "globalThis.timeoutEffects=0;try{timed(1000000000000)}catch(e){timeoutEffects++}finally{timeoutEffects++}";
                let error = if asynchronous {
                    let mut evaluation = Box::pin(runtime.eval_async(source));
                    let mut cx = TaskContext::from_waker(Waker::noop());
                    loop {
                        if let Poll::Ready(result) = evaluation.as_mut().poll(&mut cx) {
                            break result.unwrap_err();
                        }
                        std::thread::yield_now();
                    }
                } else {
                    runtime.eval(source).unwrap_err()
                };
                let native = error.as_native().unwrap();
                assert!(native.is_runtime_limit());
                assert_eq!(native.message(), boa_engine::vm::WALL_CLOCK_TIMEOUT_MESSAGE);
                assert_eq!(
                    runtime.eval("timeoutEffects").unwrap().as_number(),
                    Some(0.0)
                );
                if enabled
                    && cfg!(all(
                        any(target_arch = "x86_64", target_arch = "aarch64"),
                        any(target_os = "linux", target_os = "macos")
                    ))
                {
                    assert!(
                        runtime.baseline_jit_diagnostics().compiled_entries
                            > before.compiled_entries
                    );
                }
                assert_eq!(runtime.eval("6*7").unwrap().as_number(), Some(42.0));
                sender.send(()).unwrap();
            });
            receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("JIT must return control at its deadline");
        }
    }
}

#[test]
fn generated_execution_preserves_the_deterministic_iteration_limit() {
    for enabled in [false, true] {
        let mut runtime = runtime(enabled, Duration::from_secs(10), 100);
        let error = runtime
            .eval("(function(n){let s=1;for(let i=0;i<n;i++)s=(s+i*3)%1000003;return s})(200)")
            .unwrap_err();
        let native = error.as_native().unwrap();
        assert!(native.is_runtime_limit());
        assert_ne!(native.message(), boa_engine::vm::WALL_CLOCK_TIMEOUT_MESSAGE);
        assert_eq!(runtime.eval("1+1").unwrap().as_number(), Some(2.0));
    }
}

#[test]
fn report_arithmetic_and_property_deadline_poll_overhead() {
    if !cfg!(all(
        target_arch = "x86_64",
        any(target_os = "linux", target_os = "macos")
    )) {
        return;
    }
    let mut rows = Vec::new();
    for (shape, source) in [
        (
            "arith",
            "function f(n){let s=1;for(let i=0;i<n;i++)s=(s+i*3)%1000003;return s}",
        ),
        (
            "prop-mono",
            "function f(n){let o={x:0};for(let i=0;i<n;i++)o.x=o.x+1;return o.x}",
        ),
    ] {
        let mut context = Context::default();
        context.eval(Source::from_bytes(source)).unwrap();
        let expected = context.eval(Source::from_bytes("f(100000)")).unwrap();
        let mut without_deadline = Vec::new();
        let mut with_deadline = Vec::new();
        for sample in 0..9 {
            // Alternate order to reduce warmup/drift bias. No timing threshold
            // is used as a correctness assertion.
            for enabled in [sample % 2 == 0, sample % 2 != 0] {
                context
                    .runtime_limits_mut()
                    .set_deadline(enabled.then(|| Instant::now() + Duration::from_secs(60)));
                let before = context.arithmetic_jit_diagnostics().compiled_entries;
                let start = Instant::now();
                let actual = context.eval(Source::from_bytes("f(100000)")).unwrap();
                let nanos = start.elapsed().as_nanos() as u64;
                assert_eq!(actual, expected);
                assert!(context.arithmetic_jit_diagnostics().compiled_entries > before);
                if enabled {
                    with_deadline.push(nanos);
                } else {
                    without_deadline.push(nanos);
                }
            }
        }
        without_deadline.sort_unstable();
        with_deadline.sort_unstable();
        rows.push(
            serde_json::json!({"shape":shape,"iterations":100000,"samples":9,
            "no_deadline_median_ns":without_deadline[4],"deadline_median_ns":with_deadline[4],
            "deadline_overhead_ratio":with_deadline[4] as f64 / without_deadline[4] as f64}),
        );
    }
    let report = serde_json::json!({"architecture":std::env::consts::ARCH,"os":std::env::consts::OS,
        "comparison":"same generated code, deadline disabled versus enabled; includes slice restoration and VM polls",
        "rows":rows});
    let path = std::path::Path::new(".artifacts/js-benchmark/jit-interrupt.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{report}");
}
