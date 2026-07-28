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
//! Timings are **reported, never asserted**. A wall-clock assertion in CI either
//! fails on noise or is set so loose it catches nothing, so this follows the same
//! rule as `render_benchmark_fixture.rs`: the test checks structural invariants
//! and the numbers are printed for a human (or archived via
//! `OMOIKANE_JS_BENCH_REPORT`).
//!
//! Baselines are the median of five runs on an idle machine. Run-to-run spread
//! was 2-10% for every shape except `string-concat`, which swings about 22%
//! because its cost depends on when collection happens; that shape can therefore
//! report drift on its own, and its noise floor is well below the scale of change
//! the harness exists to track.

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
/// Wide on purpose: a band narrower than the noise would report movement on
/// every run, which trains readers to ignore the report. On an idle two-core
/// container the run-to-run spread is under 5%, so this leaves room for a busier
/// machine while still resolving the kind of change worth acting on.
///
/// Contention is not fully absorbed by any band, and it is not meant to be: when
/// *every* shape drifts the same way by a similar amount, that is the signature
/// of a loaded machine rather than a code change, and the per-shape table makes
/// that obvious. A build competing for the same two cores inflated all nine
/// shapes by 21-54% at once. `shapes.js` reports the fastest of several passes
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
    baseline_passes: u32,
    reference_engine: String,
    measured_passes: u32,
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

/// What one execution of `shapes.js` produced, including the conditions it ran
/// under so they can be checked against the recorded ones.
struct BenchmarkRun {
    passes: u32,
    measurements: Vec<Measurement>,
}

fn run_benchmarks() -> BenchmarkRun {
    let source = fs::read_to_string(SHAPES_PATH).expect("read benchmark shapes");
    let mut runtime = JsRuntime::new().expect("create benchmark runtime");
    runtime.eval(&source).expect("load benchmark shapes");
    let passes = runtime
        .eval("BENCH_PASSES")
        .expect("read the shapes' pass count")
        .as_number()
        .expect("pass count is a number") as u32;
    let output = runtime
        .eval("runBenchmarks()")
        .expect("run benchmarks")
        .as_string()
        .expect("benchmark output is a string")
        .to_std_string_escaped();
    BenchmarkRun {
        passes,
        measurements: parse_measurements(&output),
    }
}

fn build_report(baseline: &Baseline, run: &BenchmarkRun) -> Report {
    assert_eq!(
        run.passes, baseline.passes,
        "shapes.js timed {} passes but the baseline and its reference numbers were \
         recorded over {}; re-record them rather than comparing across pass counts",
        run.passes, baseline.passes
    );
    let measurements = &run.measurements;
    let mut shapes = Vec::new();
    let mut improvements = Vec::new();
    let mut regressions = Vec::new();

    for expected in &baseline.shapes {
        let measured = measurements
            .iter()
            .find(|measurement| measurement.id == expected.id)
            .unwrap_or_else(|| panic!("baseline shape {} was not measured", expected.id));
        assert_eq!(
            measured.iterations, expected.iterations,
            "shape {} ran {} iterations but the baseline and its reference numbers \
             were measured at {}; re-record the baseline (and the reference engine's \
             numbers) rather than comparing across iteration counts",
            expected.id, measured.iterations, expected.iterations
        );

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
        baseline_passes: baseline.passes,
        reference_engine: baseline.reference_engine.clone(),
        measured_passes: run.passes,
        measured_profile: measured_profile(),
        total: shapes.len(),
        improvements,
        regressions,
        shapes,
    }
}

fn print_report(report: &Report) {
    println!(
        "JS benchmark: shapes={} profile={} passes={} (baseline v{} recorded under {}; reference: {})",
        report.total,
        report.measured_profile,
        report.measured_passes,
        report.baseline_version,
        report.baseline_profile,
        report.reference_engine
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
    assert!(baseline.passes > 0);
    assert!(!baseline.reference_engine.trim().is_empty());
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
fn report_classifies_drift_against_the_baseline() {
    let baseline = Baseline {
        version: 1,
        profile: "dev".to_string(),
        passes: 4,
        reference_engine: "test".to_string(),
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
        measurements: vec![
            measurement("faster", 70.0),
            measurement("slower", 130.0),
            // 10% off the baseline is inside the tolerance band.
            measurement("steady", 110.0),
        ],
    };

    let report = build_report(&baseline, &run);

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
        measurements: vec![Measurement {
            id: "arith".to_string(),
            iterations: 200,
            elapsed_ms: 0.02,
            ns_per_op: 100.0,
        }],
    };

    build_report(&baseline, &run);
}

#[test]
#[should_panic(expected = "recorded over 2")]
fn report_refuses_to_compare_across_pass_counts() {
    let baseline = Baseline {
        version: 3,
        profile: "dev".to_string(),
        passes: 2,
        reference_engine: "test".to_string(),
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
        measurements: vec![Measurement {
            id: "arith".to_string(),
            iterations: 1000,
            elapsed_ms: 0.1,
            ns_per_op: 100.0,
        }],
    };

    build_report(&baseline, &run);
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
    let run = run_benchmarks();
    let report = build_report(&baseline, &run);

    print_report(&report);
    write_report_if_requested(&report);

    // Structural invariants only: timings are reported, not asserted.
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
