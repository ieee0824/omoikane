//! JavaScript execution benchmark, reported per workload shape.
//!
//! The point of splitting the measurement by shape is that "JS is slow" is not
//! actionable. Per shape, the report separates two different questions: how far
//! the engine is from another engine's *interpreter*, which is the gap reachable
//! by improving this one, and how much further that engine's *JIT* pulls ahead,
//! which is the gap that needs a JIT of our own. Those two numbers point at
//! different work, and they differ sharply between shapes.
//!
//! Current figures live in `baseline.json` and are not repeated here, so that
//! improving the engine cannot leave this comment contradicting the data.
//!
//! Per-shape timings are **reported, never asserted**. A wall-clock assertion in CI
//! either fails on noise or is set so loose it catches nothing, so this follows the
//! same rule as `render_benchmark_fixture.rs`: the test checks structural invariants
//! and the numbers are printed for a human (or archived via
//! `OMOIKANE_JS_BENCH_REPORT`).
//! The JSON report also records target OS/architecture, profile, and pass count
//! so a result from another runner is not mistaken for a local measurement.
//!
//! One timing *is* asserted, and only because it is not a wall-clock number:
//! `appending_cost_does_not_grow_with_the_string_being_built` compares the cost per
//! operation at two string lengths within a single run, which the machine's speed
//! divides out of. Anything else added here belongs under the reported rule.
//!
//! Omoikane baselines are the median of five runs with a fresh `JsRuntime` for
//! each run on an idle machine. This estimates the typical runtime used by the
//! report and is stable because the interpreter has no optimizing-tier
//! transition.
//!
//! SpiderMonkey references use the **minimum of five independent Firefox
//! processes**, recorded by `scripts/record-spidermonkey-reference.sh`. A JIT run
//! can be bimodal depending on whether its optimizing tier was reached; a median
//! can therefore jump between tiers even when no code changed. The minimum gives
//! every recording the warmed tier, matching the min-of-four timed-pass rule in
//! `shapes.js`. It is also appropriate for the interpreter reference because
//! external contention can only make a process slower. The cheapest JIT shapes
//! can still be loop-overhead-bound rather than measuring the operation itself
//! (see `primitive-string-property` in `shapes.js`). Do not mix Firefox versions
//! or machines when judging reproducibility.
//!
//! When comparing two builds of the engine, `cargo clean -p boa_engine -p boa_gc`
//! between them is **required**. Swapping a `[patch]` path without it produced
//! numbers that were roughly 2x off across every shape, including `arith`, which
//! contains no property access at all and therefore cannot be affected by an
//! engine change in that area — that impossibility is what exposed the stale
//! build rather than any suspicion about the numbers themselves.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use omoikane::dom::NodeHandle;
use omoikane::js::{JsRuntime, SandboxConfig};
use serde::{Deserialize, Serialize};

const SHAPES_PATH: &str = "tests/js_benchmark/shapes.js";
const BASELINE_PATH: &str = "tests/js_benchmark/baseline.json";
const GATE2_SNAPSHOT_PATH: &str = "docs/jit/gate2-performance-snapshot.json";
const GATE3_SNAPSHOTS: &[(&str, bool, &str)] = &[
    (
        "docs/jit/gate3-performance-linux-local-jit-off.json",
        false,
        "linux",
    ),
    (
        "docs/jit/gate3-performance-linux-local-jit-on.json",
        true,
        "linux",
    ),
    (
        "docs/jit/gate3-performance-ubuntu-jit-off.json",
        false,
        "linux",
    ),
    (
        "docs/jit/gate3-performance-ubuntu-jit-on.json",
        true,
        "linux",
    ),
    (
        "docs/jit/gate3-performance-macos-intel-jit-off.json",
        false,
        "macos",
    ),
    (
        "docs/jit/gate3-performance-macos-intel-jit-on.json",
        true,
        "macos",
    ),
];

/// How far a measurement may drift from the baseline before it is called an
/// improvement or a regression.
///
/// Wide on purpose: a band narrower than the noise would report movement on
/// every run, which trains readers to ignore the report. On an idle two-core
/// container the run-to-run spread is under 5%, so this leaves room for a busier
/// machine while still resolving the kind of change worth acting on.
///
/// Contention is not fully absorbed by any band, and it is not meant to be: when
/// *every* shape drifts the same way by a similar amount, that is the signature
/// of a loaded machine rather than a code change, and the per-shape table makes
/// that obvious. A build competing for the same two cores once inflated every
/// shape at once, by 21-54%. `shapes.js` reports the fastest of several passes
/// specifically so that a single unimpeded pass is enough to recover the real
/// number.
const DRIFT_TOLERANCE: f64 = 0.20;

#[derive(Debug, Deserialize)]
struct Baseline {
    version: u32,
    /// Build profile the baseline numbers were recorded under. Dependencies are
    /// compiled at `opt-level = 2` even in dev builds (see `Cargo.toml`), so the
    /// engine itself is optimized either way, but the field keeps the comparison
    /// honest if that ever changes.
    profile: String,
    /// Pass count `shapes.js` used when the numbers were recorded. Reported so a
    /// baseline captured under a different `BENCH_PASSES` is identifiable rather
    /// than silently compared.
    passes: u32,
    /// Which engine and version the `reference` numbers came from.
    reference_engine: String,
    target_arch: String,
    target_os: String,
    environment: String,
    measurement_runs: usize,
    shapes: Vec<BaselineShape>,
}

#[derive(Debug, Deserialize)]
struct BaselineShape {
    id: String,
    description: String,
    /// Iteration count the baseline and the reference numbers were measured at.
    ///
    /// Part of the measurement conditions, not an implementation detail: a
    /// tiering JIT's cost per operation depends on reaching its optimizing tier,
    /// so a reference captured at a different count is not comparable. Checked
    /// against what `shapes.js` actually ran, because that mismatch is exactly
    /// how the recorded ratios went wrong once already.
    iterations: u64,
    /// Boa's measured cost, in nanoseconds per iteration.
    baseline_ns_per_op: f64,
    /// The same shape measured in another engine, for context on how much of the
    /// gap is engine quality and how much is the JIT.
    reference: Reference,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
struct Reference {
    /// SpiderMonkey with its JITs disabled: interpreter against interpreter.
    spidermonkey_interpreter: f64,
    /// SpiderMonkey as shipped, so the delta against the line above is what the
    /// JIT contributes.
    spidermonkey_jit: f64,
}

/// One shape's measurement, as parsed from the benchmark's output line.
#[derive(Debug, Clone, PartialEq)]
struct Measurement {
    id: String,
    iterations: u64,
    elapsed_ms: f64,
    ns_per_op: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Drift {
    Improved,
    Unchanged,
    Regressed,
}

#[derive(Debug, Serialize)]
struct ShapeResult {
    id: String,
    description: String,
    ns_per_op: f64,
    /// Fresh-runtime samples in execution order. `ns_per_op` is their median.
    samples_ns_per_op: Vec<f64>,
    min_ns_per_op: f64,
    max_ns_per_op: f64,
    baseline_ns_per_op: f64,
    /// Negative means faster than the baseline.
    delta_ratio: f64,
    drift: Drift,
    reference: Reference,
    /// How many times slower than SpiderMonkey's interpreter. This is the gap
    /// reachable without a JIT.
    versus_interpreter: f64,
    /// How many times slower than SpiderMonkey with its JIT.
    versus_jit: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
struct JitDiagnostics {
    enabled: bool,
    compile_requests: u64,
    successful_compilations: u64,
    compile_rejections: u64,
    total_compile_time_ns: u64,
    generated_code_bytes: u64,
    compiled_entries: u64,
    bailouts: u64,
    property_guard_hits: u64,
    property_guard_misses: u64,
    property_bailouts: u64,
}

impl JitDiagnostics {
    fn add_assign(&mut self, other: Self) {
        self.enabled |= other.enabled;
        self.compile_requests = self.compile_requests.saturating_add(other.compile_requests);
        self.successful_compilations = self
            .successful_compilations
            .saturating_add(other.successful_compilations);
        self.compile_rejections = self
            .compile_rejections
            .saturating_add(other.compile_rejections);
        self.total_compile_time_ns = self
            .total_compile_time_ns
            .saturating_add(other.total_compile_time_ns);
        self.generated_code_bytes = self
            .generated_code_bytes
            .saturating_add(other.generated_code_bytes);
        self.compiled_entries = self.compiled_entries.saturating_add(other.compiled_entries);
        self.bailouts = self.bailouts.saturating_add(other.bailouts);
        self.property_guard_hits = self
            .property_guard_hits
            .saturating_add(other.property_guard_hits);
        self.property_guard_misses = self
            .property_guard_misses
            .saturating_add(other.property_guard_misses);
        self.property_bailouts = self
            .property_bailouts
            .saturating_add(other.property_bailouts);
    }
}

#[derive(Debug, Serialize)]
struct Report {
    baseline_version: u32,
    baseline_profile: String,
    baseline_passes: u32,
    baseline_target_arch: String,
    baseline_target_os: String,
    baseline_environment: String,
    baseline_measurement_runs: usize,
    reference_engine: String,
    target_arch: &'static str,
    target_os: &'static str,
    measured_passes: u32,
    measured_profile: &'static str,
    measurement_runs: usize,
    revision: String,
    environment: String,
    baseline_comparable: bool,
    total: usize,
    improvements: Vec<String>,
    regressions: Vec<String>,
    jit_diagnostic_samples: Vec<JitDiagnostics>,
    jit_diagnostics: JitDiagnostics,
    shapes: Vec<ShapeResult>,
}

fn measured_profile() -> &'static str {
    if cfg!(debug_assertions) {
        "dev"
    } else {
        "release"
    }
}

fn measurement_run_count() -> usize {
    let Some(raw) = std::env::var_os("OMOIKANE_JS_BENCH_RUNS") else {
        return 1;
    };
    let count = raw
        .to_string_lossy()
        .parse::<usize>()
        .expect("OMOIKANE_JS_BENCH_RUNS must be an integer");
    assert!(
        (1..=9).contains(&count),
        "OMOIKANE_JS_BENCH_RUNS must be between 1 and 9"
    );
    count
}

fn load_baseline() -> Baseline {
    serde_json::from_slice(&fs::read(BASELINE_PATH).expect("read benchmark baseline"))
        .expect("parse benchmark baseline")
}

/// Parses the `name|iterations|elapsed_ms|ns_per_op` lines the benchmark emits.
fn parse_measurements(output: &str) -> Vec<Measurement> {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split('|').collect();
            assert_eq!(fields.len(), 4, "malformed benchmark line: {line}");
            Measurement {
                id: fields[0].to_string(),
                iterations: fields[1].parse().expect("iterations"),
                elapsed_ms: fields[2].parse().expect("elapsed"),
                ns_per_op: fields[3].parse().expect("ns per op"),
            }
        })
        .collect()
}

/// What one execution of `shapes.js` produced, including the conditions it ran
/// under so they can be checked against the recorded ones.
struct BenchmarkRun {
    passes: u32,
    measurements: Vec<Measurement>,
    jit_diagnostics: JitDiagnostics,
}

fn run_benchmarks() -> BenchmarkRun {
    let source = fs::read_to_string(SHAPES_PATH).expect("read benchmark shapes");
    // The benchmark intentionally executes tens of millions of loop
    // iterations across its four passes. Keep the production default strict,
    // but give this measurement harness an explicit budget large enough for
    // the workload it is designed to run.
    let mut runtime = JsRuntime::with_document_and_sandbox(
        NodeHandle::document(),
        SandboxConfig {
            max_loop_iterations: 100_000_000,
            ..SandboxConfig::default()
        },
    )
    .expect("create benchmark runtime");
    runtime.eval(&source).expect("load benchmark shapes");
    // Validated rather than cast: `as u32` would turn 4.5 into 4 and NaN into 0,
    // and a zero pass count would surface as a baffling "timed 0 passes"
    // mismatch instead of pointing at the edit that caused it.
    let raw_passes = runtime
        .eval("BENCH_PASSES")
        .expect("read the shapes' pass count")
        .as_number()
        .expect("shapes.js must set BENCH_PASSES to a number");
    assert!(
        raw_passes.is_finite() && raw_passes >= 1.0 && raw_passes.fract() == 0.0,
        "shapes.js set BENCH_PASSES to {raw_passes}; it must be a whole number of at least 1"
    );
    let passes = raw_passes as u32;
    let output = runtime
        .eval("runBenchmarks()")
        .expect("run benchmarks")
        .as_string()
        .expect("benchmark output is a string")
        .to_std_string_escaped();
    #[cfg(feature = "baseline-jit")]
    let jit_diagnostics = {
        let diagnostics = runtime.baseline_jit_diagnostics();
        JitDiagnostics {
            enabled: true,
            compile_requests: diagnostics.compile_requests,
            successful_compilations: diagnostics.successful_compilations,
            compile_rejections: diagnostics.compile_rejections,
            total_compile_time_ns: diagnostics.total_compile_time_ns,
            generated_code_bytes: diagnostics.generated_code_bytes,
            compiled_entries: diagnostics.compiled_entries,
            bailouts: diagnostics.bailouts,
            property_guard_hits: diagnostics.property_guard_hits,
            property_guard_misses: diagnostics.property_guard_misses,
            property_bailouts: diagnostics.property_bailouts,
        }
    };
    #[cfg(not(feature = "baseline-jit"))]
    let jit_diagnostics = JitDiagnostics::default();
    BenchmarkRun {
        passes,
        measurements: parse_measurements(&output),
        jit_diagnostics,
    }
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

fn build_report(baseline: &Baseline, runs: &[BenchmarkRun]) -> Report {
    assert!(!runs.is_empty(), "at least one benchmark run is required");
    for run in runs {
        assert_eq!(
            run.passes, baseline.passes,
            "shapes.js timed {} passes but the baseline and its reference numbers were \
         recorded over {}; re-record them rather than comparing across pass counts",
            run.passes, baseline.passes
        );
    }
    let mut shapes = Vec::new();
    let mut improvements = Vec::new();
    let mut regressions = Vec::new();

    for expected in &baseline.shapes {
        let measured = runs
            .iter()
            .map(|run| {
                let measurement = run
                    .measurements
                    .iter()
                    .find(|measurement| measurement.id == expected.id)
                    .unwrap_or_else(|| panic!("baseline shape {} was not measured", expected.id));
                assert_eq!(
                    measurement.iterations, expected.iterations,
                    "shape {} ran {} iterations but the baseline and its reference numbers \
                     were measured at {}; re-record the baseline (and the reference engine's \
                     numbers) rather than comparing across iteration counts",
                    expected.id, measurement.iterations, expected.iterations
                );
                assert!(
                    measurement.ns_per_op.is_finite() && measurement.ns_per_op > 0.0,
                    "shape {} produced no usable timing: {:?}",
                    measurement.id,
                    measurement
                );
                assert!(
                    measurement.elapsed_ms.is_finite() && measurement.elapsed_ms > 0.0,
                    "shape {} completed below timer resolution; raise its iteration count",
                    measurement.id
                );
                measurement
            })
            .collect::<Vec<_>>();
        let samples_ns_per_op = measured
            .iter()
            .map(|measurement| measurement.ns_per_op)
            .collect::<Vec<_>>();
        let ns_per_op = median(samples_ns_per_op.clone());
        let min_ns_per_op = samples_ns_per_op
            .iter()
            .copied()
            .min_by(f64::total_cmp)
            .expect("non-empty samples");
        let max_ns_per_op = samples_ns_per_op
            .iter()
            .copied()
            .max_by(f64::total_cmp)
            .expect("non-empty samples");

        let delta_ratio = ns_per_op / expected.baseline_ns_per_op - 1.0;
        let drift = if delta_ratio > DRIFT_TOLERANCE {
            regressions.push(expected.id.clone());
            Drift::Regressed
        } else if delta_ratio < -DRIFT_TOLERANCE {
            improvements.push(expected.id.clone());
            Drift::Improved
        } else {
            Drift::Unchanged
        };

        shapes.push(ShapeResult {
            id: expected.id.clone(),
            description: expected.description.clone(),
            ns_per_op,
            samples_ns_per_op,
            min_ns_per_op,
            max_ns_per_op,
            baseline_ns_per_op: expected.baseline_ns_per_op,
            delta_ratio,
            drift,
            reference: expected.reference,
            versus_interpreter: ns_per_op / expected.reference.spidermonkey_interpreter,
            versus_jit: ns_per_op / expected.reference.spidermonkey_jit,
        });
    }

    let environment = std::env::var("OMOIKANE_BENCH_ENVIRONMENT").unwrap_or_else(|_| {
        if std::env::var_os("GITHUB_ACTIONS").is_some() {
            "github-actions".to_string()
        } else {
            "local".to_string()
        }
    });
    let jit_diagnostic_samples = runs
        .iter()
        .map(|run| run.jit_diagnostics)
        .collect::<Vec<_>>();
    let mut jit_diagnostics = JitDiagnostics::default();
    for diagnostics in &jit_diagnostic_samples {
        jit_diagnostics.add_assign(*diagnostics);
    }
    Report {
        baseline_version: baseline.version,
        baseline_profile: baseline.profile.clone(),
        baseline_passes: baseline.passes,
        baseline_target_arch: baseline.target_arch.clone(),
        baseline_target_os: baseline.target_os.clone(),
        baseline_environment: baseline.environment.clone(),
        baseline_measurement_runs: baseline.measurement_runs,
        reference_engine: baseline.reference_engine.clone(),
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
        measured_passes: runs[0].passes,
        measured_profile: measured_profile(),
        measurement_runs: runs.len(),
        revision: std::env::var("OMOIKANE_BENCH_REVISION")
            .or_else(|_| std::env::var("GITHUB_SHA"))
            .unwrap_or_else(|_| "unknown".to_string()),
        baseline_comparable: baseline.profile == measured_profile()
            && baseline.passes == runs[0].passes
            && baseline.measurement_runs == runs.len()
            && baseline.target_arch == std::env::consts::ARCH
            && baseline.target_os == std::env::consts::OS
            && baseline.environment == environment,
        environment,
        total: shapes.len(),
        improvements,
        regressions,
        jit_diagnostic_samples,
        jit_diagnostics,
        shapes,
    }
}

fn print_report(report: &Report) {
    println!(
        "JS benchmark: shapes={} runs={} profile={} passes={} target={}-{} (baseline v{}; reference: {})",
        report.total,
        report.measurement_runs,
        report.measured_profile,
        report.measured_passes,
        report.target_os,
        report.target_arch,
        report.baseline_version,
        report.reference_engine
    );
    if !report.baseline_comparable {
        println!(
            "  note: baseline drift is advisory; current=profile:{} runs:{} target={}-{} environment={:?}; baseline=profile:{} runs:{} target={}-{} environment={:?}",
            report.measured_profile,
            report.measurement_runs,
            report.target_os,
            report.target_arch,
            report.environment,
            report.baseline_profile,
            report.baseline_measurement_runs,
            report.baseline_target_os,
            report.baseline_target_arch,
            report.baseline_environment
        );
    }
    println!(
        "  baseline JIT: enabled={} compile={}/{} rejected={} time={}ns code={}B entries={} bailouts={} property(hit/miss/bailout)={}/{}/{}",
        report.jit_diagnostics.enabled,
        report.jit_diagnostics.successful_compilations,
        report.jit_diagnostics.compile_requests,
        report.jit_diagnostics.compile_rejections,
        report.jit_diagnostics.total_compile_time_ns,
        report.jit_diagnostics.generated_code_bytes,
        report.jit_diagnostics.compiled_entries,
        report.jit_diagnostics.bailouts,
        report.jit_diagnostics.property_guard_hits,
        report.jit_diagnostics.property_guard_misses,
        report.jit_diagnostics.property_bailouts,
    );
    println!(
        "  {:<26} {:>10} {:>10} {:>8} {:>9}  {:>9} {:>8}",
        "shape", "median", "range", "delta", "vs SM-int", "vs SM-jit", ""
    );
    for shape in &report.shapes {
        println!(
            "  {:<26} {:>10.1} {:>4.0}-{:<4.0} {:>7.0}% {:>8.1}x {:>7.0}x  {}",
            shape.id,
            shape.ns_per_op,
            shape.min_ns_per_op,
            shape.max_ns_per_op,
            shape.delta_ratio * 100.0,
            shape.versus_interpreter,
            shape.versus_jit,
            match shape.drift {
                Drift::Improved => "improved",
                Drift::Regressed => "regressed",
                Drift::Unchanged => "",
            }
        );
    }
    if !report.improvements.is_empty() {
        println!("  improvements: {}", report.improvements.join(", "));
    }
    if !report.regressions.is_empty() {
        println!("  regressions: {}", report.regressions.join(", "));
    }
}

fn write_report_if_requested(report: &Report) {
    let Ok(path) = std::env::var("OMOIKANE_JS_BENCH_REPORT") else {
        return;
    };
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create JS benchmark report directory");
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(report).expect("serialize JS benchmark report"),
    )
    .expect("write JS benchmark report");
}

#[test]
fn baseline_shapes_are_unique_and_well_formed() {
    let baseline = load_baseline();
    assert!(baseline.version > 0);
    assert!(baseline.passes > 0);
    assert!(!baseline.reference_engine.trim().is_empty());
    assert!(!baseline.target_arch.trim().is_empty());
    assert!(!baseline.target_os.trim().is_empty());
    assert!(!baseline.environment.trim().is_empty());
    assert!(baseline.measurement_runs > 0);
    assert!(!baseline.shapes.is_empty());

    let mut ids = HashSet::new();
    for shape in &baseline.shapes {
        assert!(
            shape
                .id
                .chars()
                .all(|character| character.is_ascii_lowercase() || character == '-'),
            "invalid shape id: {}",
            shape.id
        );
        assert!(
            ids.insert(shape.id.clone()),
            "duplicate shape id: {}",
            shape.id
        );
        assert!(!shape.description.trim().is_empty());
        assert!(shape.iterations > 0);
        assert!(shape.baseline_ns_per_op > 0.0);
        assert!(shape.reference.spidermonkey_interpreter > 0.0);
        assert!(shape.reference.spidermonkey_jit > 0.0);
        // The reference engine's JIT must not be recorded as slower than its own
        // interpreter; that would mean the numbers were captured with the wrong
        // preferences.
        assert!(
            shape.reference.spidermonkey_jit <= shape.reference.spidermonkey_interpreter,
            "shape {} records a JIT slower than the interpreter",
            shape.id
        );
    }
}

#[test]
fn gate2_snapshot_preserves_five_finite_samples_for_every_shape() {
    let snapshot: serde_json::Value = serde_json::from_slice(
        &fs::read(GATE2_SNAPSHOT_PATH).expect("read Gate 2 performance snapshot"),
    )
    .expect("parse Gate 2 performance snapshot");
    assert_eq!(snapshot["measurement_runs"], 5);
    assert_eq!(snapshot["measured_passes"], 4);
    assert_eq!(snapshot["baseline_comparable"], false);
    let shapes = snapshot["shapes"].as_array().expect("snapshot shapes");
    assert_eq!(shapes.len(), 11);
    for shape in shapes {
        let samples = shape["samples_ns_per_op"]
            .as_array()
            .expect("shape samples");
        assert_eq!(samples.len(), 5);
        assert!(samples.iter().all(|sample| {
            sample
                .as_f64()
                .is_some_and(|value| value.is_finite() && value > 0.0)
        }));
    }
}

#[test]
fn gate3_snapshots_preserve_matched_samples_and_native_diagnostics() {
    let baseline = load_baseline();
    let expected_ids = baseline
        .shapes
        .iter()
        .map(|shape| shape.id.as_str())
        .collect::<HashSet<_>>();
    for &(path, jit_enabled, target_os) in GATE3_SNAPSHOTS {
        let snapshot: serde_json::Value = serde_json::from_slice(
            &fs::read(path).unwrap_or_else(|error| panic!("read {path}: {error}")),
        )
        .unwrap_or_else(|error| panic!("parse {path}: {error}"));
        assert_eq!(snapshot["measurement_runs"], 5, "{path}");
        assert_eq!(snapshot["measured_passes"], 4, "{path}");
        assert_eq!(snapshot["target_arch"], "x86_64", "{path}");
        assert_eq!(snapshot["target_os"], target_os, "{path}");
        assert_eq!(
            snapshot["jit_diagnostics"]["enabled"], jit_enabled,
            "{path}"
        );

        let shapes = snapshot["shapes"].as_array().expect("snapshot shapes");
        assert_eq!(shapes.len(), expected_ids.len(), "{path}");
        let actual_ids = shapes
            .iter()
            .map(|shape| shape["id"].as_str().expect("shape id"))
            .collect::<HashSet<_>>();
        assert_eq!(actual_ids, expected_ids, "{path}");
        for shape in shapes {
            let samples = shape["samples_ns_per_op"]
                .as_array()
                .expect("shape samples");
            assert_eq!(samples.len(), 5, "{path}");
            assert!(samples.iter().all(|sample| {
                sample
                    .as_f64()
                    .is_some_and(|value| value.is_finite() && value > 0.0)
            }));
        }

        let diagnostics = &snapshot["jit_diagnostics"];
        if jit_enabled {
            assert!(
                diagnostics["successful_compilations"].as_u64().unwrap_or(0) > 0,
                "{path}"
            );
            assert!(
                diagnostics["generated_code_bytes"].as_u64().unwrap_or(0) > 0,
                "{path}"
            );
            assert!(
                diagnostics["property_guard_hits"].as_u64().unwrap_or(0) > 0,
                "{path}"
            );
            assert_eq!(diagnostics["property_guard_misses"], 0, "{path}");
            assert_eq!(diagnostics["property_bailouts"], 0, "{path}");
        } else {
            assert_eq!(diagnostics["compile_requests"], 0, "{path}");
            assert_eq!(diagnostics["generated_code_bytes"], 0, "{path}");
        }
    }
}

#[test]
fn report_classifies_drift_against_the_baseline() {
    let baseline = Baseline {
        version: 1,
        profile: "dev".to_string(),
        passes: 4,
        reference_engine: "test".to_string(),
        target_arch: std::env::consts::ARCH.to_string(),
        target_os: std::env::consts::OS.to_string(),
        environment: "test".to_string(),
        measurement_runs: 1,
        shapes: vec![
            BaselineShape {
                id: "faster".to_string(),
                description: "got faster".to_string(),
                iterations: 1000,
                baseline_ns_per_op: 100.0,
                reference: Reference {
                    spidermonkey_interpreter: 50.0,
                    spidermonkey_jit: 10.0,
                },
            },
            BaselineShape {
                id: "slower".to_string(),
                description: "got slower".to_string(),
                iterations: 1000,
                baseline_ns_per_op: 100.0,
                reference: Reference {
                    spidermonkey_interpreter: 50.0,
                    spidermonkey_jit: 10.0,
                },
            },
            BaselineShape {
                id: "steady".to_string(),
                description: "within tolerance".to_string(),
                iterations: 1000,
                baseline_ns_per_op: 100.0,
                reference: Reference {
                    spidermonkey_interpreter: 50.0,
                    spidermonkey_jit: 10.0,
                },
            },
        ],
    };
    let measurement = |id: &str, ns_per_op: f64| Measurement {
        id: id.to_string(),
        iterations: 1000,
        elapsed_ms: ns_per_op / 1000.0,
        ns_per_op,
    };
    let run = BenchmarkRun {
        passes: 4,
        jit_diagnostics: JitDiagnostics::default(),
        measurements: vec![
            measurement("faster", 70.0),
            measurement("slower", 130.0),
            // 10% off the baseline is inside the tolerance band.
            measurement("steady", 110.0),
        ],
    };

    let report = build_report(&baseline, &[run]);

    assert_eq!(report.improvements, ["faster"]);
    assert_eq!(report.regressions, ["slower"]);
    assert_eq!(
        report
            .shapes
            .iter()
            .map(|shape| shape.drift)
            .collect::<Vec<_>>(),
        [Drift::Improved, Drift::Regressed, Drift::Unchanged]
    );
    // A shape 7x its own baseline is still only 1.4x SpiderMonkey's interpreter.
    let faster = &report.shapes[0];
    assert_eq!(faster.versus_interpreter, 1.4);
    assert_eq!(faster.versus_jit, 7.0);
}

#[test]
fn report_uses_the_median_and_preserves_each_sample() {
    let baseline = Baseline {
        version: 1,
        profile: "dev".to_string(),
        passes: 4,
        reference_engine: "test".to_string(),
        target_arch: std::env::consts::ARCH.to_string(),
        target_os: std::env::consts::OS.to_string(),
        environment: "test".to_string(),
        measurement_runs: 1,
        shapes: vec![BaselineShape {
            id: "arith".to_string(),
            description: "median probe".to_string(),
            iterations: 1_000,
            baseline_ns_per_op: 100.0,
            reference: Reference {
                spidermonkey_interpreter: 50.0,
                spidermonkey_jit: 10.0,
            },
        }],
    };
    let run = |ns_per_op| BenchmarkRun {
        passes: 4,
        jit_diagnostics: JitDiagnostics {
            enabled: true,
            compile_requests: 1,
            successful_compilations: 1,
            total_compile_time_ns: 7,
            generated_code_bytes: 11,
            compiled_entries: 2,
            property_guard_hits: 3,
            ..JitDiagnostics::default()
        },
        measurements: vec![Measurement {
            id: "arith".to_string(),
            iterations: 1_000,
            elapsed_ms: ns_per_op / 1_000.0,
            ns_per_op,
        }],
    };

    let report = build_report(&baseline, &[run(120.0), run(80.0), run(100.0)]);
    let shape = &report.shapes[0];
    assert_eq!(report.measurement_runs, 3);
    assert_eq!(report.jit_diagnostic_samples.len(), 3);
    assert_eq!(report.jit_diagnostics.compile_requests, 3);
    assert_eq!(report.jit_diagnostics.total_compile_time_ns, 21);
    assert_eq!(report.jit_diagnostics.generated_code_bytes, 33);
    assert_eq!(report.jit_diagnostics.property_guard_hits, 9);
    assert_eq!(shape.ns_per_op, 100.0);
    assert_eq!(shape.samples_ns_per_op, [120.0, 80.0, 100.0]);
    assert_eq!((shape.min_ns_per_op, shape.max_ns_per_op), (80.0, 120.0));
}

/// The guard that makes the mismatch this schema field exists to prevent
/// impossible rather than merely documented.
#[test]
#[should_panic(expected = "were measured at 1000")]
fn report_refuses_to_compare_across_iteration_counts() {
    let baseline = Baseline {
        version: 3,
        profile: "dev".to_string(),
        passes: 4,
        reference_engine: "test".to_string(),
        target_arch: std::env::consts::ARCH.to_string(),
        target_os: std::env::consts::OS.to_string(),
        environment: "test".to_string(),
        measurement_runs: 1,
        shapes: vec![BaselineShape {
            id: "arith".to_string(),
            description: "recorded at a different count".to_string(),
            iterations: 1000,
            baseline_ns_per_op: 100.0,
            reference: Reference {
                spidermonkey_interpreter: 50.0,
                spidermonkey_jit: 10.0,
            },
        }],
    };
    let run = BenchmarkRun {
        passes: 4,
        jit_diagnostics: JitDiagnostics::default(),
        measurements: vec![Measurement {
            id: "arith".to_string(),
            iterations: 200,
            elapsed_ms: 0.02,
            ns_per_op: 100.0,
        }],
    };

    build_report(&baseline, &[run]);
}

#[test]
#[should_panic(expected = "recorded over 2")]
fn report_refuses_to_compare_across_pass_counts() {
    let baseline = Baseline {
        version: 3,
        profile: "dev".to_string(),
        passes: 2,
        reference_engine: "test".to_string(),
        target_arch: std::env::consts::ARCH.to_string(),
        target_os: std::env::consts::OS.to_string(),
        environment: "test".to_string(),
        measurement_runs: 1,
        shapes: vec![BaselineShape {
            id: "arith".to_string(),
            description: "recorded over fewer passes".to_string(),
            iterations: 1000,
            baseline_ns_per_op: 100.0,
            reference: Reference {
                spidermonkey_interpreter: 50.0,
                spidermonkey_jit: 10.0,
            },
        }],
    };
    let run = BenchmarkRun {
        passes: 4,
        jit_diagnostics: JitDiagnostics::default(),
        measurements: vec![Measurement {
            id: "arith".to_string(),
            iterations: 1000,
            elapsed_ms: 0.1,
            ns_per_op: 100.0,
        }],
    };

    build_report(&baseline, &[run]);
}

#[test]
fn benchmark_output_lines_are_parsed_into_measurements() {
    let parsed =
        parse_measurements("arith|400000|12.5000|31.25\n\nprop-mono|200000|40.0000|200.00\n");

    assert_eq!(
        parsed,
        vec![
            Measurement {
                id: "arith".to_string(),
                iterations: 400_000,
                elapsed_ms: 12.5,
                ns_per_op: 31.25,
            },
            Measurement {
                id: "prop-mono".to_string(),
                iterations: 200_000,
                elapsed_ms: 40.0,
                ns_per_op: 200.0,
            },
        ]
    );
}

#[test]
fn js_execution_benchmark_reports_every_shape() {
    let baseline = load_baseline();
    let runs = (0..measurement_run_count())
        .map(|_| run_benchmarks())
        .collect::<Vec<_>>();
    let report = build_report(&baseline, &runs);

    print_report(&report);
    write_report_if_requested(&report);

    // Structural invariants only: timings are reported, not asserted.
    for run in &runs {
        assert_eq!(run.measurements.len(), baseline.shapes.len());
        for measurement in &run.measurements {
            assert!(
                measurement.ns_per_op.is_finite() && measurement.ns_per_op > 0.0,
                "shape {} produced no usable timing: {:?}",
                measurement.id,
                measurement
            );
            assert!(measurement.iterations > 0);
            assert!(
                measurement.elapsed_ms > 0.0,
                "shape {} completed below timer resolution; raise its iteration count",
                measurement.id
            );
        }
    }
}

/// How the cost of `s += x` scales with the length being built.
///
/// This is the one timing in the suite that is *asserted* rather than reported,
/// because it is not a wall-clock threshold: it compares the cost per operation at
/// two lengths in the same run, on the same machine, moments apart. Reallocating and
/// copying the prefix on every append makes that ratio grow with the length; writing
/// into the string's own spare capacity keeps it flat (issues #314, #318-#320).
/// Before the append path this shape cost 4.0x more per operation at 65,536 chars
/// than at 256; after it, 1.04x, and six consecutive runs gave 0.96x to 1.00x.
///
/// The band is wide because the two lengths differ in cache behaviour as well —
/// 65,536 UTF-16 chars do not sit in L2 the way 256 do — so a flat implementation
/// still measures somewhat worse at the top. It is nowhere near the 4x that
/// reintroducing the copy would produce, which is what this is here to catch.
const APPEND_SCALING_TOLERANCE: f64 = 2.0;

/// How many ticks of the clock each timed pass must span.
///
/// A ratio of two durations is only meaningful if neither is near the resolution
/// floor, so rather than picking an iteration count large enough today, the
/// measurement reports what the clock's granularity actually is and this asserts
/// against it. If `+=` becomes fast enough for the passes to shrink toward the floor,
/// or the clock coarsens, the test says so instead of quietly going flaky.
///
/// A measurement spanning `n` ticks carries up to one tick of error, so the floor of
/// a thousand bounds each duration's error at about 0.1%. Measured here the passes are
/// far above it — `performance.now()` resolves to 334 ns and a pass spans 6.5 ms, about
/// 19,400 ticks, or 19x the floor, putting the error near 0.005%. Either figure is
/// orders of magnitude below what could move a ratio read against 2.0.
const MIN_TICKS_PER_PASS: f64 = 1000.0;

#[test]
fn appending_cost_does_not_grow_with_the_string_being_built() {
    let source = r#"
        var PASSES = 4;
        var ITERATIONS = 50000;

        // The smallest step this clock can report, so the assertions can be made
        // against the real granularity rather than an assumed one.
        function clockResolution() {
          var smallest = Infinity;
          for (var i = 0; i < 200000; i++) {
            var a = performance.now();
            var step = performance.now() - a;
            if (step > 0 && step < smallest) smallest = step;
          }
          return smallest;
        }

        // Resets on a counter rather than by reading `s.length`, so that the
        // primitive property read is not folded into the measurement.
        function build(limit) {
          var period = limit >> 1;
          var best = Infinity;
          for (var pass = 0; pass < PASSES; pass++) {
            var start = performance.now();
            var s = "";
            var c = 0;
            for (var i = 0; i < ITERATIONS; i++) {
              s += "ab";
              c++;
              if (c > period) { s = ""; c = 0; }
            }
            var elapsed = performance.now() - start;
            globalThis.__benchSink = s.length;
            if (elapsed < best) best = elapsed;
          }
          return best;
        }

        var resolution = clockResolution();
        var shortMs = build(256);
        var longMs = build(65536);
        [resolution, shortMs, longMs, ITERATIONS].join("|")
    "#;

    let mut runtime = JsRuntime::new().expect("runtime should start");
    let measured = runtime
        .eval_safe(source)
        .expect("the scaling measurement should evaluate")
        .as_string()
        .expect("the measurement returns a string")
        .to_std_string_escaped();

    let fields: Vec<f64> = measured
        .split('|')
        .map(|field| field.parse().expect("the measurement returns numbers"))
        .collect();
    let [resolution, short_ms, long_ms, iterations] = fields[..]
        .try_into()
        .expect("the measurement returns four values");

    assert!(
        resolution > 0.0 && resolution.is_finite(),
        "could not determine the clock's resolution: {resolution}"
    );
    for (label, elapsed) in [("256", short_ms), ("65536", long_ms)] {
        assert!(
            elapsed / resolution >= MIN_TICKS_PER_PASS,
            "a pass at {label} chars spanned {elapsed:.3} ms against a clock resolving to \
             {resolution:.6} ms, only {:.0} ticks. Below {MIN_TICKS_PER_PASS:.0} the ratio below \
             is quantization noise: raise ITERATIONS in the measurement",
            elapsed / resolution
        );
    }

    let short = short_ms * 1e6 / iterations;
    let long = long_ms * 1e6 / iterations;
    let ratio = long / short;
    println!(
        "append scaling: 256 chars {short:.1} ns/op, 65536 chars {long:.1} ns/op, ratio {ratio:.2}x \
         ({:.0} clock ticks per pass)",
        short_ms / resolution
    );
    assert!(
        ratio < APPEND_SCALING_TOLERANCE,
        "appending cost {long:.1} ns/op at 65,536 chars against {short:.1} at 256, a ratio of \
         {ratio:.2}x. Above {APPEND_SCALING_TOLERANCE:.1}x the cost is growing with the length \
         being built, which is what copying the prefix on every append does — the in-place \
         append path is no longer being taken"
    );
}
