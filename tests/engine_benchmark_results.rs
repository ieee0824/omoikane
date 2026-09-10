//! Preserve the observable results of the fixed performance workloads.

use std::time::Duration;

use omoikane::dom::NodeHandle;
use omoikane::js::{JsRuntime, SandboxConfig};
use serde_json::json;

#[test]
fn fixed_benchmark_workloads_preserve_each_computed_result() {
    let mut runtime = JsRuntime::with_document_and_sandbox(
        NodeHandle::document(),
        SandboxConfig {
            max_loop_iterations: 100_000_000,
            timeout: Duration::from_secs(60),
        },
    )
    .unwrap();
    runtime
        .eval(include_str!("js_benchmark/shapes.js"))
        .unwrap();
    // Run each original body once, with its original iteration count, outside
    // the timing harness. Its shared sink becomes NaN after the array case and
    // cannot by itself check the remaining workloads' computed values.
    let values = runtime
        .eval(
            r#"
            var computedResults = {};
            bench = function (name, iterations, body) {
                computedResults[name] = String(body(iterations));
                return name;
            };
            runBenchmarks();
            JSON.stringify(computedResults);
            "#,
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    // Finite values were derived independently from the fixture's integer sums
    // and checked with V8. The existing array fixture resets after 1025 pushes
    // but masks its index to 1024 values, so it eventually reads undefined.
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&values).unwrap(),
        json!({
            "arith": "64",
            "prop-mono": "500003500000",
            "prop-mega": "1750000",
            "call": "1000000",
            "closure-alloc": "715003",
            "object-alloc": "730003",
            "string-concat": "2494",
            "array": "NaN",
            "primitive-string-property": "499997",
            "primitive-string-method": "999856",
            "proto-method": "750000",
        })
    );
}
