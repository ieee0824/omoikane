//! Opt-in JIT execution and matched timing evidence from an extracted release archive.
use std::{fs, path::PathBuf, time::Instant};

use omoikane::js::JsRuntime;
use serde_json::{Value, json};

fn main() {
    let directory = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("output directory argument"),
    );
    fs::create_dir_all(&directory).unwrap();
    let mut cases = Vec::<Value>::new();
    for seed in [305_i64, 541, 542, 543] {
        let iterations = 20_000 + seed;
        for shape in ["arith", "prop-mono"] {
            let (declaration, warmup, call, expected) = if shape == "arith" {
                (
                    format!(
                        "function f(n){{var s=1;for(var i=0;i<n;i++)s=(s+i*{seed})%1000003;return s}}"
                    ),
                    "f(200)".to_owned(),
                    format!("f({iterations})"),
                    (0..iterations).fold(1_i64, |sum, i| (sum + i * seed) % 1_000_003),
                )
            } else {
                (
                    format!(
                        "var object={{a:{seed},b:2,c:3}};function f(o,n){{var s=0;for(var i=0;i<n;i++){{o.b=o.a+i;s+=o.b+o.c}}return s}}"
                    ),
                    "f(object,200)".to_owned(),
                    format!("f(object,{iterations})"),
                    iterations * (seed + 3) + iterations * (iterations - 1) / 2,
                )
            };
            fs::write(
                directory.join(format!("{seed}-{shape}.js")),
                format!("{declaration}\n{warmup};\n{call};\n"),
            )
            .unwrap();
            // Alternate order to avoid always measuring JIT on after JIT off.
            for sample in 0..5 {
                for enabled in if sample % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let mut runtime = JsRuntime::new().unwrap();
                    runtime.set_baseline_jit_enabled(enabled);
                    let before = runtime.baseline_jit_diagnostics();
                    runtime.eval(&declaration).unwrap();
                    runtime.eval(&warmup).unwrap();
                    let warm = runtime.baseline_jit_diagnostics();
                    let start = Instant::now();
                    let actual = runtime.eval(&call).unwrap().as_number();
                    let elapsed_ns = start.elapsed().as_nanos();
                    let after = runtime.baseline_jit_diagnostics();
                    let entries = after.compiled_entries - warm.compiled_entries;
                    let hits = after.property_guard_hits - warm.property_guard_hits;
                    let success = actual == Some(expected as f64)
                        && if enabled {
                            entries > 0 && (shape != "prop-mono" || hits > 0)
                        } else {
                            entries == 0 && hits == 0
                        };
                    cases.push(json!({
                        "seed": seed, "shape": shape, "sample": sample, "jit_enabled": enabled,
                        "iterations": iterations, "expected": expected, "actual": actual,
                        "success": success, "elapsed_ns": elapsed_ns,
                        "compile_time_ns": warm.total_compile_time_ns - before.total_compile_time_ns,
                        "generated_code_bytes": warm.generated_code_bytes - before.generated_code_bytes,
                        "successful_compilations": warm.successful_compilations - before.successful_compilations,
                        "compile_rejections": warm.compile_rejections - before.compile_rejections,
                        "compiled_entries": entries, "property_guard_hits": hits,
                        "bailouts": after.bailouts - warm.bailouts,
                        "property_guard_misses": after.property_guard_misses - warm.property_guard_misses,
                        "interrupt_deopts": after.interrupt_deopts - warm.interrupt_deopts,
                        "shape_deopts": after.shape_deopts - warm.shape_deopts,
                        "type_deopts": after.type_deopts - warm.type_deopts,
                        "arithmetic_deopts": after.arithmetic_deopts - warm.arithmetic_deopts,
                    }));
                    if enabled && sample == 0 {
                        fs::write(
                            directory.join(format!("{seed}-{shape}-code.txt")),
                            runtime.baseline_jit_debug_snapshot(),
                        )
                        .unwrap();
                    }
                    // Preserve the failing case before asserting.
                    let report = json!({
                        "status": if success { "running" } else { "failed" },
                        "architecture": std::env::consts::ARCH, "os": std::env::consts::OS,
                        "measurement": "five alternating fresh-runtime samples; eval timed after declaration/warmup; compilation counters cover declaration/warmup",
                        "cases": cases,
                    });
                    fs::write(
                        directory.join("probe.json"),
                        serde_json::to_vec_pretty(&report).unwrap(),
                    )
                    .unwrap();
                    assert!(
                        success,
                        "seed={seed} shape={shape} sample={sample} jit={enabled}: {report}"
                    );
                }
            }
        }
    }
    let path = directory.join("probe.json");
    let mut report: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    report["status"] = json!("passed");
    fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
}
