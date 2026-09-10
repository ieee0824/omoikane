//! Native release-target evidence: execution policy, reproducible seeds and timings.
#![cfg(feature = "baseline-jit")]

// Reuse the same forced-GC/deopt/exception/timeout seed corpus on each native
// host. The separate Gate 4 job retains its profiled ten-minute requirement.
#[path = "jit_stress/mod.rs"]
mod stress;

use std::{fs, path::Path, time::Instant};

use boa_engine::{
    Context, Source,
    jit::{JitCacheKey, JitCodeCache, JitError},
};
use serde_json::json;

fn save(path: &Path, report: &serde_json::Value) {
    fs::write(path, serde_json::to_vec_pretty(report).unwrap()).unwrap();
}

#[test]
fn native_policy_and_seeded_workloads_produce_execution_evidence() {
    let directory = Path::new(".artifacts/jit-native");
    fs::create_dir_all(directory).unwrap();
    let report_path = directory.join("native-report.json");
    let mut report = json!({
        "status": "running", "architecture": std::env::consts::ARCH,
        "os": std::env::consts::OS, "seeds": [305, 541, 542, 543], "cases": [],
        "jit_policy": "not-yet-tested"
    });
    save(&report_path, &report);
    let mut cache = JitCodeCache::new();
    let handle = match cache.compile_fixed_return(
        JitCacheKey {
            code_id: 544,
            version: 1,
        },
        42,
    ) {
        Ok(handle) => handle,
        Err(error) => {
            let classification = match &error {
                JitError::UnsupportedPlatform => "unsupported-platform",
                JitError::Os(error) if matches!(error.raw_os_error(), Some(1 | 13)) => {
                    "jit-permission-denied"
                }
                JitError::Os(_) => "jit-os-error",
                _ => "jit-construction-error",
            };
            report["status"] = json!("failed");
            report["jit_policy"] = json!(classification);
            report["error"] = json!(error.to_string());
            save(&report_path, &report);
            panic!("native execution required: {classification}: {error}");
        }
    };
    fs::write(
        directory.join("fixed-code.txt"),
        cache.debug_code_dump(handle).unwrap(),
    )
    .unwrap();
    assert_eq!(cache.call_fixed_return(handle).unwrap(), 42);
    report["jit_policy"] = json!("execution-confirmed");
    save(&report_path, &report);

    for seed in [305_i64, 541, 542, 543] {
        let iterations = 20_000 + seed;
        for shape in ["arith", "prop-mono"] {
            let (declaration, expected) = if shape == "arith" {
                (
                    format!(
                        "function f(n){{var s=1;for(var i=0;i<n;i++)s=(s+i*{seed})%1000003;return s}}"
                    ),
                    (0..iterations).fold(1_i64, |sum, i| (sum + i * seed) % 1_000_003),
                )
            } else {
                (
                    format!(
                        "function f(n){{var o={{a:{seed},b:2,c:3}},s=0;for(var i=0;i<n;i++){{o.b=o.a+i;s+=o.b+o.c}}return s}}"
                    ),
                    iterations * (seed + 3) + iterations * (iterations - 1) / 2,
                )
            };
            fs::write(
                directory.join(format!("seed-{seed}-{shape}.js")),
                format!("{declaration}\nf(200);\nf({iterations});\n"),
            )
            .unwrap();
            for enabled in [false, true] {
                let mut context = Context::default();
                context.set_baseline_jit_enabled(enabled);
                context
                    .eval(Source::from_bytes(&format!("{declaration};f(200)")))
                    .unwrap();
                let before = context.arithmetic_jit_diagnostics();
                let start = Instant::now();
                let result = context.eval(Source::from_bytes(&format!("f({iterations})")));
                let elapsed = start.elapsed().as_nanos();
                let actual = result.as_ref().ok().and_then(|value| value.as_number());
                let diagnostics = context.arithmetic_jit_diagnostics();
                let entries = diagnostics.compiled_entries - before.compiled_entries;
                let hits = diagnostics.property_guard_hits - before.property_guard_hits;
                let success = actual == Some(expected as f64)
                    && if enabled {
                        entries > 0 && (shape != "prop-mono" || hits > 0)
                    } else {
                        entries == 0 && hits == 0
                    };
                report["cases"].as_array_mut().unwrap().push(json!({
                    "seed": seed, "shape": shape, "jit_enabled": enabled,
                    "iterations": iterations, "elapsed_ns": elapsed,
                    "expected": expected, "actual": actual, "success": success,
                    "compiled_entries": entries, "property_guard_hits": hits,
                    "error": result.err().map(|error| error.to_string())
                }));
                if enabled {
                    fs::write(
                        directory.join(format!("code-{seed}-{shape}.txt")),
                        context.jit_debug_snapshot(),
                    )
                    .unwrap();
                }
                if !success {
                    report["status"] = json!("failed");
                }
                save(&report_path, &report);
                assert!(success, "seed={seed} shape={shape} jit={enabled}: {report}");
            }
        }
    }
    report["status"] = json!("passed");
    save(&report_path, &report);
}
