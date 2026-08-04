//! Gate 4-4 integration contract for exact interpreter reconstruction.
#![cfg(feature = "baseline-jit")]

use boa_engine::{Context, JsValue, Source};

fn evaluate(
    source: &str,
    jit_enabled: bool,
) -> (JsValue, boa_engine::jit::ArithmeticJitDiagnostics) {
    let mut context = Context::default();
    context.set_baseline_jit_enabled(jit_enabled);
    let value = context
        .eval(Source::from_bytes(source))
        .expect("evaluate differential deopt workload");
    (value, context.arithmetic_jit_diagnostics())
}

#[test]
#[cfg(not(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos"))))]
fn unsupported_target_runs_the_interpreter_contract() {
    let (value, diagnostics) = evaluate(
        "(function(n){let s=0;for(let i=0;i<n;i++)s+=i;return s})(100)",
        true,
    );
    assert_eq!(value.as_number(), Some(4_950.0));
    assert_eq!(diagnostics.compiled_entries, 0);
}

#[test]
#[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
fn shape_type_and_arithmetic_guards_match_jit_off() {
    let workloads = [
        (
            "function f(o,n){let s=0;for(let i=0;i<n;i++){s+=o.x;o.x+=1}return s+o.x}\
             let a={x:1};f(a,200);let b={pad:0,x:7};f(b,100)",
            "shape",
        ),
        (
            "function f(n){let s=1;for(let i=0;i<n;i++)s=(s+i*3)%1000003;return s}\
             f(200);f('40')",
            "type",
        ),
        (
            "function f(n,s){for(let i=0;i<n;i++)s=s+i*3;return s}\
             f(200,1);f(100,9007199254740980)",
            "arithmetic",
        ),
    ];
    for (source, reason) in workloads {
        let (expected, _) = evaluate(source, false);
        let (actual, diagnostics) = evaluate(source, true);
        assert_eq!(actual, expected, "{reason} guard changed the result");
        assert!(
            diagnostics.compiled_entries >= 1,
            "{reason} never entered JIT"
        );
        match reason {
            "shape" => assert!(diagnostics.shape_deopts >= 1),
            "type" => assert!(diagnostics.type_deopts >= 1),
            "arithmetic" => assert!(diagnostics.arithmetic_deopts >= 1),
            _ => unreachable!(),
        }
    }
}

#[test]
#[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
fn nested_caller_and_immediate_exception_observe_reconstructed_frame() {
    const SOURCE: &str = "function f(n,s){for(let i=0;i<n;i++)s=s+i*3;return s}\
         function nested(s){return f(100,s)}f(200,1);\
         try{let value=nested(9007199254740980);throw Error('after:'+value)}\
         catch(error){error.message}";
    let (expected, _) = evaluate(SOURCE, false);
    let (actual, diagnostics) = evaluate(SOURCE, true);
    assert_eq!(actual, expected);
    assert!(diagnostics.arithmetic_deopts >= 1);
}

#[test]
#[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
fn collection_before_deopt_does_not_leave_stale_registers() {
    fn run(jit_enabled: bool) -> (JsValue, boa_engine::jit::ArithmeticJitDiagnostics) {
        let mut context = Context::default();
        context.set_baseline_jit_enabled(jit_enabled);
        context
            .eval(Source::from_bytes(
                "function f(n,s){for(let i=0;i<n;i++)s=s+i*3;return s}f(200,1)",
            ))
            .unwrap();
        boa_gc::force_collect();
        let value = context
            .eval(Source::from_bytes("f(100,9007199254740980)"))
            .unwrap();
        (value, context.arithmetic_jit_diagnostics())
    }
    let (expected, _) = run(false);
    let (actual, diagnostics) = run(true);
    assert_eq!(actual, expected);
    assert!(diagnostics.arithmetic_deopts >= 1);
}

#[test]
#[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
fn property_store_before_deopt_is_committed_exactly_once() {
    const SOURCE: &str =
        "function f(o,n,start){let s=start;for(let i=0;i<n;i++){o.x=o.x+1;s=s+o.x}return [o.x,s]}\
         let warm={x:0};f(warm,200,0);let target={x:0};f(target,100,9007199254740980).join(',')";
    let (expected, _) = evaluate(SOURCE, false);
    let (actual, diagnostics) = evaluate(SOURCE, true);
    assert_eq!(actual, expected);
    assert_eq!(actual.display().to_string(), "\"100,9007199254745984\"");
    assert!(diagnostics.property_bailouts >= 1);
    assert!(diagnostics.arithmetic_deopts >= 1);
}

#[test]
#[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
fn explicit_interrupt_matches_interpreter_failure() {
    fn run(jit_enabled: bool) -> (String, boa_engine::jit::ArithmeticJitDiagnostics) {
        let mut context = Context::default();
        context.set_baseline_jit_enabled(jit_enabled);
        context.runtime_limits_mut().set_loop_iteration_limit(1_000);
        context
            .eval(Source::from_bytes(
                "function interruptible(n){let s=0;for(let i=0;i<n;i++)s+=i;return s}\
                 interruptible(200)",
            ))
            .expect("warm-up must complete before lowering the interrupt limit");
        context.runtime_limits_mut().set_loop_iteration_limit(100);
        let error = context
            .eval(Source::from_bytes("interruptible(200)"))
            .expect_err("the loop iteration limit must interrupt execution");
        (error.to_string(), context.arithmetic_jit_diagnostics())
    }
    let (expected, _) = run(false);
    let (actual, diagnostics) = run(true);
    assert_eq!(actual, expected);
    assert!(diagnostics.interrupt_deopts >= 1);
}
