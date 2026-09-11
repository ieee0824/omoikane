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
    // Run every body once outside the timing harness and its expected-value
    // table, so an incorrect expected value cannot make the fixture self-validate.
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
    // Independent closed forms (M = 1_000_003): arith = (1+3*n*(n-1)/2)%M;
    // prop-mono = n*(n-1)/2+4*n; prop-mega = (n/8)*28; call = n%M;
    // closure-alloc = (n*(n-1)/2)%M; object-alloc = n*n%M;
    // string-concat = 2*(n%2049); array = n*(n-1)/2;
    // primitive-string-property = 3*n%M; primitive-string-method = 98*n%M
    // (a,a,c,c repeating); proto-method = (n/4)*6.
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
            "array": "124999750000",
            "primitive-string-property": "499997",
            "primitive-string-method": "999856",
            "proto-method": "750000",
        })
    );
}

#[test]
fn array_workload_resets_at_the_index_period() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(include_str!("js_benchmark/shapes.js"))
        .unwrap();
    runtime
        .eval(
            r#"
        var arrayBody;
        bench = function (name, iterations, body) {
            if (name === "array") arrayBody = body;
            return name;
        };
        runBenchmarks();
    "#,
        )
        .unwrap();
    for n in [0_u64, 1, 1023, 1024, 1025, 1026, 2047, 2048, 2049, 4097] {
        let actual = runtime
            .eval(&format!("arrayBody({n})"))
            .unwrap()
            .as_number()
            .unwrap();
        let expected = n * n.saturating_sub(1) / 2;
        assert_eq!(actual, expected as f64, "array length {n}");
    }
}

#[test]
fn benchmark_rejects_incorrect_results_on_every_pass() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(include_str!("js_benchmark/shapes.js"))
        .unwrap();
    for bad in [
        "NaN",
        "Infinity",
        "-Infinity",
        "undefined",
        "null",
        "'64'",
        "65",
    ] {
        for pass in 1..=4 {
            let error = runtime
                .eval(&format!(
                    r#"
                var calls = 0;
                bench("arith", 2000000, function () {{
                    return ++calls === {pass} ? {bad} : 64;
                }});
            "#
                ))
                .unwrap_err();
            assert!(
                error.to_string().contains(&format!("arith pass {pass}")),
                "{error}"
            );
            // Failed values never enter the sink and cannot affect another body.
            assert_eq!(
                runtime
                    .eval("__benchSink.arith.length")
                    .unwrap()
                    .as_number(),
                Some((pass - 1) as f64)
            );
        }
    }
    runtime
        .eval(
            r#"
        bench("array", 500000, function () { return 124999750000; });
        bench("primitive-string-property", 500000, function () { return 499997; });
    "#,
        )
        .unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(__benchSink['primitive-string-property'])")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[499997,499997,499997,499997]"
    );
    assert!(
        runtime
            .eval("bench('arith', 1, function () { return 64; })")
            .is_err()
    );
    assert!(
        runtime
            .eval("bench('unknown', 1, function () { return 64; })")
            .is_err()
    );
}

#[test]
fn string_length_keeps_utf16_and_polymorphic_receiver_semantics() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime
        .eval(
            r#"
        (() => {
            function read(value) { return value.length; }
            let getterCalls = 0, proxyCalls = 0;
            const inherited = { get length() {
                if (this !== child) throw new Error("getter receiver");
                getterCalls++;
                return 31;
            }};
            const child = Object.create(inherited);
            const proxy = new Proxy({ length: 23 }, { get(target, key, receiver) {
                if (receiver !== proxy) throw new Error("proxy receiver");
                if (key === "length") proxyCalls++;
                return Reflect.get(target, key, receiver);
            }});
            Object.defineProperty(Number.prototype, "length", { get() {
                "use strict";
                if (this !== 99) throw new Error("number receiver");
                return 42;
            }, configurable: true });
            const values = ["", "abc", "日本", "\u{1F600}", "\uD800",
                            new String("box"), [], { length: 17 }, child, proxy, 99];
            const expected = [0, 3, 2, 2, 1, 3, 0, 17, 31, 23, 42];
            for (let pass = 0; pass < 300; pass++) {
                for (let i = 0; i < values.length; i++) {
                    if (read(values[i]) !== expected[i]) throw new Error("length " + i);
                }
            }
            Object.setPrototypeOf(child, { length: 47 });
            if (read(child) !== 47) throw new Error("prototype change");
            let nullishThrows = 0;
            for (const value of [null, undefined]) {
                try { read(value); } catch (error) {
                    if (!(error instanceof TypeError)) throw error;
                    nullishThrows++;
                }
            }
            const sentinel = {};
            let caughtGetter = false;
            try { read({ get length() { throw sentinel; } }); }
            catch (error) { caughtGetter = error === sentinel; }
            class Base { get length() { return this.marker; } }
            class Derived extends Base { read() { return super.length; } }
            const derived = new Derived();
            derived.marker = 19;
            delete Number.prototype.length;
            return JSON.stringify([getterCalls, proxyCalls, nullishThrows,
                                   caughtGetter, derived.read(), read("after")]);
        })()
    "#,
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(result, "[300,300,2,true,19,5]");
}

#[test]
fn string_append_length_keeps_retained_aliases() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        var appended = "", savedStrings = [], savedLengths = [];
        function appendChunk() {
            for (let i = 0; i < 1000; i++) {
                appended += "\u{1F600}a";
                if (i % 100 === 0) {
                    savedStrings.push(appended);
                    savedLengths.push(appended.length);
                }
            }
        }
        appendChunk();
    "#,
        )
        .unwrap();
    boa_gc::force_collect();
    runtime.eval("appendChunk();").unwrap();
    boa_gc::force_collect();
    let result = runtime
        .eval(
            r#"
        appended.length === 6000 && savedStrings.length === 20 &&
        savedStrings.every((value, i) => value.length === 3*(100*i + 1) &&
            savedLengths[i] === 3*(100*i + 1) && value.endsWith("\u{1F600}a"))
    "#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}
