//! Reproducible probes for issue #307 Gate 1.
//!
//! The probes have two intentionally separate layers:
//!
//! - the first uses Boa's public `Script`/`CodeBlock` API and proves that the
//!   current bytecode pipeline can compile and execute an arithmetic workload;
//! - the second drives Omoikane's public `JsRuntime`, including host bindings,
//!   jobs, timers, shape mutation, WeakMap state, and forced GC.
//!
//! Boa's bytecode bytes, opcode enum, IC entries, VM stack, and frame program
//! counter are private to `boa_engine`. Consequently this test is a public
//! embedding probe, not a fake claim that an external crate can already enter
//! a native JIT entry point. The missing private API boundary is recorded in
//! `docs/jit/gate1-baseline.md`.

use boa_engine::{Context, Script, Source};
use omoikane::js::JsRuntime;

const RUNTIME_PROBE: &str = include_str!("fixtures/js/jit_gate1_runtime_probe.js");
const BOA_REVISION: &str = "1674beed49e671b991d092a9c4448fd019c275f5";

fn eval_number(runtime: &mut JsRuntime, source: &str) -> f64 {
    let value = runtime
        .eval(source)
        .unwrap_or_else(|error| panic!("probe expression should evaluate ({source}): {error}"));
    value
        .as_number()
        .unwrap_or_else(|| panic!("probe expression should return a number ({source}): {value:?}"))
}
#[test]
fn public_boa_bytecode_pipeline_compiles_and_runs_arithmetic() {
    let mut context = Context::default();
    let script = Script::parse(
        Source::from_bytes("let sum = 0; for (let i = 0; i < 100; i++) sum += i; sum"),
        None,
        &mut context,
    )
    .expect("Boa should parse the arithmetic probe");
    let codeblock = script
        .codeblock(&mut context)
        .expect("Boa should compile the arithmetic probe to a code block");
    assert!(
        !codeblock.name().to_std_string_escaped().is_empty(),
        "the public code-block handle should retain source identity"
    );

    let value = script
        .evaluate(&mut context)
        .expect("Boa should execute the arithmetic probe");
    assert_eq!(value.as_number(), Some(4_950.0));
}

#[test]
fn omoikane_runtime_survives_shape_jobs_and_forced_gc() {
    let mut runtime = JsRuntime::new().expect("Omoikane runtime should build");
    runtime
        .eval(RUNTIME_PROBE)
        .expect("Omoikane should execute the Gate 1 fixture");

    // Promote the first object graph before adding a new child. The second
    // collection checks the runtime's old-to-young remembered edge through the
    // same public behavior a future JIT frame must preserve.
    boa_gc::force_collect();
    runtime
        .eval("globalThis.__gate1Old.children.push({ marker: 'young' });")
        .expect("Omoikane should write a young child after collection");
    boa_gc::force_collect();

    // Promise jobs and timer callbacks cross Omoikane's host/runtime boundary.
    runtime
        .run_jobs()
        .expect("Omoikane should drain the Promise job before the timer");
    assert_eq!(runtime.run_timers(1, 1, 10), 1);
    runtime
        .run_until_idle()
        .expect("Omoikane should drain jobs and timers");
    boa_gc::force_collect();

    assert_eq!(
        eval_number(&mut runtime, "__omoikane_gate1_result.arithmetic"),
        4_950.0
    );
    assert_eq!(
        eval_number(&mut runtime, "__omoikane_gate1_result.prototypeProperty"),
        700.0
    );
    assert_eq!(
        eval_number(
            &mut runtime,
            "__omoikane_gate1_result.shapeMutation.join(',') === '3,11,13' ? 1 : 0",
        ),
        1.0
    );
    assert_eq!(
        eval_number(&mut runtime, "__omoikane_gate1_result.promise"),
        45.0
    );
    assert_eq!(
        eval_number(&mut runtime, "__omoikane_gate1_result.timer"),
        46.0
    );
    assert_eq!(
        eval_number(
            &mut runtime,
            "__gate1Root.nested.value + __gate1Root.array.length"
        ),
        45.0
    );
    assert_eq!(
        eval_number(
            &mut runtime,
            "__gate1Old.children[0].marker === 'young' ? 1 : 0"
        ),
        1.0
    );
    assert_eq!(
        eval_number(
            &mut runtime,
            "__gate1WeakMap.get(__gate1WeakKey).marker === 'weak-value' ? 1 : 0",
        ),
        1.0
    );
}

#[test]
fn gate1_scope_covers_every_current_benchmark_shape() {
    let scope: serde_json::Value =
        serde_json::from_str(include_str!("../docs/jit/gate1-scope.json"))
            .expect("Gate 1 scope manifest should be valid JSON");
    let baseline: serde_json::Value =
        serde_json::from_str(include_str!("js_benchmark/baseline.json"))
            .expect("JS benchmark baseline should be valid JSON");

    assert_eq!(scope["boa_revision"], BOA_REVISION);
    let scope_ids: Vec<&str> = scope["benchmark_workloads"]
        .as_array()
        .expect("scope workloads should be an array")
        .iter()
        .map(|entry| entry["id"].as_str().expect("scope workload id"))
        .collect();
    let baseline_ids: Vec<&str> = baseline["shapes"]
        .as_array()
        .expect("baseline shapes should be an array")
        .iter()
        .map(|entry| entry["id"].as_str().expect("baseline shape id"))
        .collect();
    assert_eq!(
        scope_ids, baseline_ids,
        "add a Gate 1 migration entry whenever the benchmark shape set changes"
    );
}
