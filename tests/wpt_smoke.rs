//! WPT testharness.js smoke runner.
use std::fs;
use std::path::PathBuf;

#[path = "support/http_fixture.rs"]
mod http_fixture;

#[path = "wpt_smoke/case_runner.rs"]
mod case_runner;
#[path = "wpt_smoke/classification.rs"]
mod classification;
#[path = "wpt_smoke/cross_origin.rs"]
mod cross_origin;
#[cfg(test)]
#[path = "wpt_smoke/failure_modes.rs"]
mod failure_modes;
#[path = "wpt_smoke/manifest.rs"]
mod manifest;
#[path = "wpt_smoke/model.rs"]
mod model;
#[path = "wpt_smoke/report.rs"]
mod report;
#[path = "wpt_smoke/server.rs"]
mod server;
#[path = "wpt_smoke/summary.rs"]
mod summary;
#[path = "wpt_smoke/testdriver.rs"]
mod testdriver;
#[path = "wpt_smoke/upstream.rs"]
mod upstream;
#[path = "wpt_smoke/upstream_transport.rs"]
mod upstream_transport;
use case_runner::{CaseExecution, run_case, run_case_with_transport};
use classification::classify_with_subtests;
use manifest::validate_manifest;
use model::{
    ActualStatus, Classification, FixtureServer, KnownFailure, Manifest, WptReport, WptResult,
};
use report::{junit_xml, read_revision_report, write_revision_reports};
use server::StaticServer;
use summary::{area_for_path, diff_revision_reports, summarize};

fn mismatch_message(
    path: &str,
    actual: ActualStatus,
    classification: Classification,
    known: Option<&KnownFailure>,
    errors: &[String],
    details: &str,
) -> Option<String> {
    let kind = match classification {
        Classification::Regression => "regression",
        Classification::Improvement => "unexpected pass",
        Classification::Pass | Classification::KnownFailure => return None,
    };
    let expected = known.map(|known| known.status.as_str()).unwrap_or("PASS");
    Some(format!(
        "WPT {path} {kind}: expected={expected} actual={}; script errors={errors:?}; results={details}",
        actual.as_str()
    ))
}

struct CaseServers {
    static_server: StaticServer,
    upstream: Option<upstream::UpstreamServer>,
    force_upstream: bool,
}

impl CaseServers {
    fn start(root: PathBuf, manifest: &Manifest, force_upstream: bool) -> Self {
        let static_server = StaticServer::start(root.clone());
        let needs_upstream = force_upstream
            || manifest
                .tests
                .iter()
                .any(|case| case.server == FixtureServer::Wptserve);
        let upstream = needs_upstream.then(|| {
            upstream::UpstreamServer::start(&root).expect("start pinned upstream WPT server")
        });
        Self {
            static_server,
            upstream,
            force_upstream,
        }
    }

    fn run(&self, case: &model::WptCase) -> CaseExecution {
        if let Some(upstream) = self
            .upstream
            .as_ref()
            .filter(|_| case.server == FixtureServer::Wptserve || self.force_upstream)
        {
            run_case_with_transport(
                &upstream.base_url,
                &case.path,
                Some(std::sync::Arc::clone(&upstream.transport)),
            )
        } else {
            run_case(&self.static_server.base_url, &case.path)
        }
    }
}

#[test]
fn selected_wpt_testharness_cases_match_expectations() {
    let root = std::env::var("WPT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("target/wpt"));
    if !root.join("resources/testharness.js").is_file() {
        if std::env::var_os("WPT_REQUIRED").is_some() {
            panic!(
                "WPT checkout required but missing (WPT_ROOT={})",
                root.display()
            );
        }
        eprintln!(
            "WPT checkout missing; run scripts/fetch-wpt.sh (WPT_ROOT={})",
            root.display()
        );
        return;
    }
    let manifest_path =
        std::env::var("WPT_MANIFEST").unwrap_or_else(|_| "tests/wpt/manifest.json".to_string());
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(&manifest_path).expect("read WPT manifest"))
            .expect("parse WPT manifest");
    if let Err(errors) = validate_manifest(&manifest) {
        panic!("invalid WPT manifest:\n{}", errors.join("\n"));
    }
    let revision = fs::read_to_string("tests/wpt/revision.txt")
        .expect("read WPT revision")
        .trim()
        .to_string();
    let servers = CaseServers::start(
        root,
        &manifest,
        std::env::var("WPT_UPSTREAM").as_deref() == Ok("1"),
    );
    let mut results = Vec::new();
    let mut mismatches = Vec::new();
    for case in manifest.tests {
        let CaseExecution {
            actual,
            script_errors: errors,
            subtests,
            details,
        } = servers.run(&case);
        let classification = classify_with_subtests(actual, case.known_failure.as_ref(), &subtests);
        println!(
            "WPT {}: actual={} classification={classification:?}",
            case.path,
            actual.as_str()
        );
        if let Some(message) = mismatch_message(
            &case.path,
            actual,
            classification,
            case.known_failure.as_ref(),
            &errors,
            &details,
        ) {
            mismatches.push(message);
        }
        results.push(WptResult {
            area: area_for_path(&case.path),
            path: case.path,
            actual,
            classification,
            known_failure: case.known_failure,
            script_errors: errors,
            subtests,
        });
    }
    let summary = summarize(&results);
    println!(
        "WPT summary: total={} pass={} known-failure={} regression={} improvement={}",
        summary.total, summary.pass, summary.known_failure, summary.regression, summary.improvement
    );
    for (area, counts) in &summary.by_area {
        println!(
            "WPT area {area}: total={} pass={} known-failure={} regression={} improvement={}",
            counts.total, counts.pass, counts.known_failure, counts.regression, counts.improvement
        );
    }
    let report = WptReport {
        revision,
        summary,
        results,
    };
    let results_root = std::env::var("WPT_RESULTS_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("GITHUB_ACTIONS")
                .is_some()
                .then(|| PathBuf::from(".artifacts/wpt/results"))
        });
    if let Some(root) = results_root {
        write_revision_reports(&root, &report).expect("write revision-scoped WPT reports");
        if let Ok(previous_revision) = std::env::var("WPT_COMPARE_REVISION") {
            let previous = read_revision_report(&root, &previous_revision)
                .expect("read previous WPT revision report");
            println!(
                "WPT revision diff: {}",
                serde_json::to_string_pretty(&diff_revision_reports(&previous, &report))
                    .expect("serialize WPT revision diff")
            );
        }
    }
    if let Ok(path) = std::env::var("WPT_REPORT") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create report directory");
        }
        fs::write(
            &path,
            serde_json::to_vec_pretty(&report).expect("serialize WPT report"),
        )
        .expect("write WPT report");
    }
    if let Ok(path) = std::env::var("WPT_JUNIT") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create JUnit directory");
        }
        fs::write(&path, junit_xml(&report)).expect("write WPT JUnit report");
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
