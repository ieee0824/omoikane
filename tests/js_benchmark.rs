//! JavaScript execution benchmark, reported per workload shape.
//!
//! The point of splitting the measurement by shape is that "JS is slow" is not
//! actionable, while "string building is 15x off and monomorphic property access
//! is 1.7x off" is: it says which part of the engine to work on, and which gap
//! cannot be closed without a JIT.
//!
//! Timings are **reported, never asserted**. A wall-clock assertion in CI either
//! fails on noise or is set so loose it catches nothing, so this follows the same
//! rule as `render_benchmark_fixture.rs`: the test checks structural invariants
//! and the numbers are printed for a human (or archived via
//! `OMOIKANE_JS_BENCH_REPORT`).

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use omoikane::js::JsRuntime;
use serde::{Deserialize, Serialize};

const SHAPES_PATH: &str = "tests/js_benchmark/shapes.js";
const BASELINE_PATH: &str = "tests/js_benchmark/baseline.json";

/// How far a measurement may drift from the baseline before it is called an
/// improvement or a regression.
///
/// Wide on purpose. Run-to-run spread on shared CI runners is easily tens of
/// percent, and a band narrower than the noise would report movement on every
/// run, which trains readers to ignore the report.
const DRIFT_TOLERANCE: f64 = 0.20;

#[derive(Debug, Deserialize)]
struct Baseline {
    version: u32,
    /// Build profile the baseline numbers were recorded under. Dependencies are
    /// compiled at `opt-level = 2` even in dev builds (see `Cargo.toml`), so the
    /// engine itself is optimized either way, but the field keeps the comparison
    /// honest if that ever changes.
    profile: String,
    shapes: Vec<BaselineShape>,
}

#[derive(Debug, Deserialize)]
struct BaselineShape {
    id: String,
    description: String,
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

#[derive(Debug, Serialize)]
struct Report {
    baseline_version: u32,
    baseline_profile: String,
    measured_profile: &'static str,
    total: usize,
    improvements: Vec<String>,
    regressions: Vec<String>,
    shapes: Vec<ShapeResult>,
}

fn measured_profile() -> &'static str {
    if cfg!(debug_assertions) { "dev" } else { "release" }
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

fn run_benchmarks() -> Vec<Measurement> {
    let source = fs::read_to_string(SHAPES_PATH).expect("read benchmark shapes");
    let mut runtime = JsRuntime::new().expect("create benchmark runtime");
    runtime.eval(&source).expect("load benchmark shapes");
    let output = runtime
        .eval("runBenchmarks()")
        .expect("run benchmarks")
        .as_string()
        .expect("benchmark output is a string")
        .to_std_string_escaped();
    parse_measurements(&output)
}

fn build_report(baseline: &Baseline, measurements: &[Measurement]) -> Report {
    let mut shapes = Vec::new();
    let mut improvements = Vec::new();
    let mut regressions = Vec::new();

    for expected in &baseline.shapes {
        let measured = measurements
            .iter()
            .find(|measurement| measurement.id == expected.id)
            .unwrap_or_else(|| panic!("baseline shape {} was not measured", expected.id));

        let delta_ratio = measured.ns_per_op / expected.baseline_ns_per_op - 1.0;
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
            ns_per_op: measured.ns_per_op,
            baseline_ns_per_op: expected.baseline_ns_per_op,
            delta_ratio,
            drift,
            reference: expected.reference,
            versus_interpreter: measured.ns_per_op / expected.reference.spidermonkey_interpreter,
            versus_jit: measured.ns_per_op / expected.reference.spidermonkey_jit,
        });
    }

    Report {
        baseline_version: baseline.version,
        baseline_profile: baseline.profile.clone(),
        measured_profile: measured_profile(),
        total: shapes.len(),
        improvements,
        regressions,
        shapes,
    }
}

fn print_report(report: &Report) {
    println!(
        "JS benchmark: shapes={} profile={} (baseline recorded under {})",
        report.total, report.measured_profile, report.baseline_profile
    );
    println!(
        "  {:<14} {:>10} {:>10} {:>8}  {:>9} {:>8}",
        "shape", "ns/op", "baseline", "delta", "vs SM-int", "vs SM-jit"
    );
    for shape in &report.shapes {
        println!(
            "  {:<14} {:>10.1} {:>10.1} {:>7.0}% {:>8.1}x {:>7.0}x  {}",
            shape.id,
            shape.ns_per_op,
            shape.baseline_ns_per_op,
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
        assert!(ids.insert(shape.id.clone()), "duplicate shape id: {}", shape.id);
        assert!(!shape.description.trim().is_empty());
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
fn report_classifies_drift_against_the_baseline() {
    let baseline = Baseline {
        version: 1,
        profile: "dev".to_string(),
        shapes: vec![
            BaselineShape {
                id: "faster".to_string(),
                description: "got faster".to_string(),
                baseline_ns_per_op: 100.0,
                reference: Reference {
                    spidermonkey_interpreter: 50.0,
                    spidermonkey_jit: 10.0,
                },
            },
            BaselineShape {
                id: "slower".to_string(),
                description: "got slower".to_string(),
                baseline_ns_per_op: 100.0,
                reference: Reference {
                    spidermonkey_interpreter: 50.0,
                    spidermonkey_jit: 10.0,
                },
            },
            BaselineShape {
                id: "steady".to_string(),
                description: "within tolerance".to_string(),
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
    let measurements = vec![
        measurement("faster", 70.0),
        measurement("slower", 130.0),
        // 10% off the baseline is inside the tolerance band.
        measurement("steady", 110.0),
    ];

    let report = build_report(&baseline, &measurements);

    assert_eq!(report.improvements, ["faster"]);
    assert_eq!(report.regressions, ["slower"]);
    assert_eq!(
        report.shapes.iter().map(|shape| shape.drift).collect::<Vec<_>>(),
        [Drift::Improved, Drift::Regressed, Drift::Unchanged]
    );
    // A shape 7x its own baseline is still only 1.4x SpiderMonkey's interpreter.
    let faster = &report.shapes[0];
    assert_eq!(faster.versus_interpreter, 1.4);
    assert_eq!(faster.versus_jit, 7.0);
}

#[test]
fn benchmark_output_lines_are_parsed_into_measurements() {
    let parsed = parse_measurements("arith|400000|12.5000|31.25\n\nprop-mono|200000|40.0000|200.00\n");

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
    let measurements = run_benchmarks();
    let report = build_report(&baseline, &measurements);

    print_report(&report);
    write_report_if_requested(&report);

    // Structural invariants only: timings are reported, not asserted.
    assert_eq!(measurements.len(), baseline.shapes.len());
    for measurement in &measurements {
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
