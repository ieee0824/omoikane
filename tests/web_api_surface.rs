//! Machine-readable Web API surface and basic-behavior probe.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::PathBuf;

use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;
use serde::{Deserialize, Serialize};

const MANIFEST_PATH: &str = "tests/web_api_surface/manifest.json";

#[derive(Debug, Deserialize)]
struct Manifest {
    version: u32,
    features: Vec<Feature>,
}

#[derive(Debug, Deserialize)]
struct Feature {
    id: String,
    area: String,
    description: String,
    setup: Option<String>,
    #[serde(default)]
    run_animation_frame: bool,
    /// Drains queued tasks before probing. Needed for features whose results
    /// arrive from a task source rather than a microtask, such as `FileReader`.
    #[serde(default)]
    run_tasks: bool,
    expected_navigation_requests: Option<usize>,
    probe: String,
    baseline_supported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProbeStatus {
    Supported,
    Unsupported,
    Error,
}

#[derive(Debug, Serialize)]
struct ProbeResult {
    id: String,
    area: String,
    description: String,
    baseline_supported: bool,
    status: ProbeStatus,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct AreaSummary {
    total: usize,
    supported: usize,
    unsupported: usize,
    errors: usize,
}

#[derive(Debug, Serialize)]
struct Report {
    manifest_version: u32,
    total: usize,
    supported: usize,
    unsupported: usize,
    errors: usize,
    improvements: Vec<String>,
    regressions: Vec<String>,
    areas: BTreeMap<String, AreaSummary>,
    results: Vec<ProbeResult>,
}

fn load_manifest() -> Manifest {
    serde_json::from_slice(&fs::read(MANIFEST_PATH).expect("read Web API manifest"))
        .expect("parse Web API manifest")
}

fn run_probe(runtime: &mut JsRuntime, feature: &Feature) -> ProbeResult {
    if let Some(setup) = &feature.setup
        && let Err(error) = runtime.eval(setup)
    {
        return ProbeResult {
            id: feature.id.clone(),
            area: feature.area.clone(),
            description: feature.description.clone(),
            baseline_supported: feature.baseline_supported,
            status: ProbeStatus::Error,
            error: Some(format!("setup: {error}")),
        };
    }

    if let Err(error) = runtime.run_jobs() {
        return ProbeResult {
            id: feature.id.clone(),
            area: feature.area.clone(),
            description: feature.description.clone(),
            baseline_supported: feature.baseline_supported,
            status: ProbeStatus::Error,
            error: Some(format!("jobs: {error}")),
        };
    }

    if feature.run_tasks
        && let Err(error) = runtime.run_until_idle()
    {
        return ProbeResult {
            id: feature.id.clone(),
            area: feature.area.clone(),
            description: feature.description.clone(),
            baseline_supported: feature.baseline_supported,
            status: ProbeStatus::Error,
            error: Some(format!("tasks: {error}")),
        };
    }

    if feature.run_animation_frame
        && let Err(error) = runtime.run_animation_frame(16)
    {
        return ProbeResult {
            id: feature.id.clone(),
            area: feature.area.clone(),
            description: feature.description.clone(),
            baseline_supported: feature.baseline_supported,
            status: ProbeStatus::Error,
            error: Some(format!("animation frame: {error}")),
        };
    }

    if let Some(expected) = feature.expected_navigation_requests {
        if let Err(error) = runtime.run_until_idle() {
            return ProbeResult {
                id: feature.id.clone(),
                area: feature.area.clone(),
                description: feature.description.clone(),
                baseline_supported: feature.baseline_supported,
                status: ProbeStatus::Error,
                error: Some(format!("event loop: {error}")),
            };
        }
        let actual = runtime.take_navigation_requests().len();
        if actual != expected {
            return ProbeResult {
                id: feature.id.clone(),
                area: feature.area.clone(),
                description: feature.description.clone(),
                baseline_supported: feature.baseline_supported,
                status: ProbeStatus::Unsupported,
                error: Some(format!(
                    "navigation requests: expected {expected}, got {actual}"
                )),
            };
        }
    }

    match runtime.eval(&format!("Boolean(({}))", feature.probe)) {
        Ok(value) => ProbeResult {
            id: feature.id.clone(),
            area: feature.area.clone(),
            description: feature.description.clone(),
            baseline_supported: feature.baseline_supported,
            status: if value.as_boolean() == Some(true) {
                ProbeStatus::Supported
            } else {
                ProbeStatus::Unsupported
            },
            error: None,
        },
        Err(error) => ProbeResult {
            id: feature.id.clone(),
            area: feature.area.clone(),
            description: feature.description.clone(),
            baseline_supported: feature.baseline_supported,
            status: ProbeStatus::Error,
            error: Some(format!("probe: {error}")),
        },
    }
}

fn build_report(manifest: &Manifest, results: Vec<ProbeResult>) -> Report {
    let mut areas = BTreeMap::<String, AreaSummary>::new();
    for result in &results {
        let summary = areas.entry(result.area.clone()).or_insert(AreaSummary {
            total: 0,
            supported: 0,
            unsupported: 0,
            errors: 0,
        });
        summary.total += 1;
        match result.status {
            ProbeStatus::Supported => summary.supported += 1,
            ProbeStatus::Unsupported => summary.unsupported += 1,
            ProbeStatus::Error => summary.errors += 1,
        }
    }

    let supported = results
        .iter()
        .filter(|result| result.status == ProbeStatus::Supported)
        .count();
    let errors = results
        .iter()
        .filter(|result| result.status == ProbeStatus::Error)
        .count();
    let improvements = results
        .iter()
        .filter(|result| !result.baseline_supported && result.status == ProbeStatus::Supported)
        .map(|result| result.id.clone())
        .collect();
    let regressions = results
        .iter()
        .filter(|result| result.baseline_supported && result.status != ProbeStatus::Supported)
        .map(|result| result.id.clone())
        .collect();

    Report {
        manifest_version: manifest.version,
        total: results.len(),
        supported,
        unsupported: results.len() - supported - errors,
        errors,
        improvements,
        regressions,
        areas,
        results,
    }
}

fn print_report(report: &Report) {
    println!(
        "Web API surface: supported={}/{} unsupported={} errors={}",
        report.supported, report.total, report.unsupported, report.errors
    );
    for (area, summary) in &report.areas {
        println!(
            "  {area}: supported={}/{} unsupported={} errors={}",
            summary.supported, summary.total, summary.unsupported, summary.errors
        );
    }
    for result in &report.results {
        if result.status != ProbeStatus::Supported {
            println!(
                "  {:?}: {} ({}){}",
                result.status,
                result.id,
                result.description,
                result
                    .error
                    .as_deref()
                    .map(|error| format!(" - {error}"))
                    .unwrap_or_default()
            );
        }
    }
    if !report.improvements.is_empty() {
        println!("  improvements: {}", report.improvements.join(", "));
    }
}

fn write_report_if_requested(report: &Report) {
    let Ok(path) = std::env::var("OMOIKANE_WEB_API_REPORT") else {
        return;
    };
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create Web API report directory");
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(report).expect("serialize Web API report"),
    )
    .expect("write Web API report");
}

#[test]
fn manifest_ids_are_unique_and_well_formed() {
    let manifest = load_manifest();
    assert!(manifest.version > 0);
    assert!(!manifest.features.is_empty());

    let mut ids = HashSet::new();
    for feature in manifest.features {
        assert!(
            feature
                .id
                .chars()
                .all(|character| character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '.' | '-')),
            "invalid stable feature id: {}",
            feature.id
        );
        assert!(
            ids.insert(feature.id.clone()),
            "duplicate feature id: {}",
            feature.id
        );
        assert!(!feature.area.trim().is_empty());
        assert!(!feature.description.trim().is_empty());
        assert!(!feature.probe.trim().is_empty());
    }
}

#[test]
fn report_classifies_regressions_and_improvements() {
    let manifest = Manifest {
        version: 1,
        features: Vec::new(),
    };
    let results = vec![
        ProbeResult {
            id: "stable.feature".to_string(),
            area: "test".to_string(),
            description: "regressed feature".to_string(),
            baseline_supported: true,
            status: ProbeStatus::Unsupported,
            error: None,
        },
        ProbeResult {
            id: "new.feature".to_string(),
            area: "test".to_string(),
            description: "newly supported feature".to_string(),
            baseline_supported: false,
            status: ProbeStatus::Supported,
            error: None,
        },
    ];

    let report = build_report(&manifest, results);

    assert_eq!(report.regressions, ["stable.feature"]);
    assert_eq!(report.improvements, ["new.feature"]);
}

#[test]
fn web_api_surface_does_not_regress() {
    let manifest = load_manifest();
    let document = TreeBuilder::parse(
        "<html><head><style>#probe { display: block; }</style></head><body><main id='probe'></main></body></html>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).expect("create Web API probe runtime");
    let results = manifest
        .features
        .iter()
        .map(|feature| run_probe(&mut runtime, feature))
        .collect();
    let report = build_report(&manifest, results);

    print_report(&report);
    write_report_if_requested(&report);

    assert!(
        report.regressions.is_empty(),
        "previously supported Web API probes regressed: {}",
        report.regressions.join(", ")
    );
}
