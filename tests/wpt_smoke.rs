//! WPT testharness.js smoke runner.
use omoikane::html::TreeBuilder;
use omoikane::http::{Client, Url};
use omoikane::js::JsRuntime;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "wpt_smoke/classification.rs"]
mod classification;
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
use classification::classify_with_subtests;
use manifest::validate_manifest;
use model::{ActualStatus, Classification, KnownFailure, Manifest, WptReport, WptResult};
use report::{junit_xml, read_revision_report, write_revision_reports};
use server::StaticServer;
use summary::{area_for_path, diff_revision_reports, summarize};

fn script_dependencies(source: &[u8], test_path: &str) -> Vec<String> {
    let parent = Path::new(test_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    String::from_utf8_lossy(source)
        .lines()
        .filter_map(|line| line.strip_prefix("// META: script="))
        .filter_map(|script| {
            let path = if let Some(absolute) = script.strip_prefix('/') {
                PathBuf::from(absolute)
            } else {
                parent.join(script)
            };
            if path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::RootDir
                )
            }) {
                return None;
            }
            Some(format!("/{}", path.to_string_lossy()))
        })
        .collect()
}
fn js_bool(runtime: &mut JsRuntime, source: &str) -> bool {
    runtime
        .eval(source)
        .ok()
        .and_then(|value| value.as_boolean())
        .unwrap_or(false)
}

fn drive_visibility_state_testdriver(runtime: &mut JsRuntime, errors: &mut Vec<String>) {
    // Keep this bridge scoped to the one WPT that needs host window-state
    // automation. Each queued Promise resolves only after host visibility has
    // changed, so its observer can see the matching entry before continuing.
    for _ in 0..64 {
        if js_bool(runtime, "globalThis.__wpt_complete === true") {
            break;
        }
        if js_bool(runtime, "globalThis.__wpt_window_commands.length > 0") {
            let hidden = js_bool(runtime, "__wpt_window_commands[0].hidden");
            runtime.set_page_visibility(hidden);
            if let Err(error) = runtime
                .eval("__wpt_window_commands.shift().resolve({x:0,y:0,width:800,height:600})")
            {
                errors.push(format!("window-state testdriver: {error}"));
                break;
            }
        }
        runtime.run_timers(500, 10, 500);
        if let Err(error) = runtime.run_jobs() {
            errors.push(format!("window-state testdriver jobs: {error}"));
            break;
        }
    }
}

fn known_failure(status: ActualStatus) -> KnownFailure {
    KnownFailure {
        status,
        reason: "not implemented <yet>".to_string(),
        issue: "#123&tracking".to_string(),
        expires: None,
        failed_subtests: None,
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
    let manifest: Manifest =
        serde_json::from_slice(&fs::read("tests/wpt/manifest.json").expect("read WPT manifest"))
            .expect("parse WPT manifest");
    if let Err(errors) = validate_manifest(&manifest) {
        panic!("invalid WPT manifest:\n{}", errors.join("\n"));
    }
    let revision = fs::read_to_string("tests/wpt/revision.txt")
        .expect("read WPT revision")
        .trim()
        .to_string();
    let server = StaticServer::start(root);
    let mut results = Vec::new();
    let mut regressions = Vec::new();
    for case in manifest.tests {
        let url = format!("{}/{}", server.base_url, case.path);
        let mut client = Client::new();
        let response = client
            .get(&url)
            .unwrap_or_else(|error| panic!("GET {url}: {error}"));
        assert_eq!(
            response.status_code(),
            200,
            "WPT resource missing: {}",
            case.path
        );
        let document_source = if case.path.ends_with(".any.js") || case.path.ends_with(".window.js")
        {
            let dependencies = script_dependencies(response.body(), &case.path)
                .into_iter()
                .map(|path| format!("<script src=\"{path}\"></script>"))
                .collect::<String>();
            format!(
                "<!doctype html><script src=\"/resources/testharness.js\"></script>\
                 <script src=\"/resources/testharnessreport.js\"></script>{dependencies}\
                 <script src=\"/{0}\"></script>",
                case.path
            )
        } else {
            String::from_utf8_lossy(response.body()).into_owned()
        };
        let document = TreeBuilder::parse(&document_source).document();
        let base: Url = url.parse().expect("parse WPT URL");
        let mut runtime =
            JsRuntime::with_document_and_url(document, &url).expect("create WPT runtime");
        // The bootstrap itself creates a large graph of host API constructors.
        // Collect its short-lived initialization temporaries before page code
        // starts allocating, keeping each WPT case's GC pressure bounded.
        boa_gc::force_collect();
        let mut errors = runtime.execute_document_scripts(Some(&base));
        runtime
            .wire_inline_event_handlers()
            .expect("wire WPT handlers");
        runtime.fire_load().expect("fire WPT load");
        if case.path == "page-visibility/visibility-state-entry.tentative.html" {
            drive_visibility_state_testdriver(&mut runtime, &mut errors);
        } else {
            runtime.run_timers(5_000, 10, 2_000);
        }
        // WPTs commonly observe rendering steps through nested
        // requestAnimationFrame callbacks. Drive a bounded number of explicit
        // opportunities after load so resize/scroll events queued for a frame
        // can settle without turning a self-rescheduling callback into an
        // unbounded test run.
        runtime.run_animation_frames(32, 16);
        runtime.run_jobs().expect("drain WPT jobs");
        errors.extend(runtime.take_task_errors());
        let complete = js_bool(&mut runtime, "globalThis.__wpt_complete === true");
        let passed = js_bool(
            &mut runtime,
            "__wpt_complete===true && __wpt_harness_status===0 && __wpt_results.length>0 && __wpt_results.every(test=>test.status===0)",
        );
        let actual = if !errors.is_empty() {
            ActualStatus::Error
        } else if passed {
            ActualStatus::Pass
        } else if complete {
            ActualStatus::Fail
        } else {
            ActualStatus::Timeout
        };
        let details = runtime
            .eval("JSON.stringify(globalThis.__wpt_results||[])")
            .ok()
            .and_then(|value| value.as_string().map(|text| text.to_std_string_escaped()))
            .unwrap_or_else(|| "[]".to_string());
        let subtests = serde_json::from_str(&details).unwrap_or(serde_json::Value::Null);
        let classification = classify_with_subtests(actual, case.known_failure.as_ref(), &subtests);
        // Each WPT case uses a fresh Boa realm.  The main branch's expanded
        // bootstrap creates considerably more short-lived objects than the
        // original smoke set, so dropping the runtime alone can leave enough
        // allocator pressure accumulated across dozens of cases to make this
        // bounded integration test request an absurd allocation.  Collect only
        // after the provider and realm have been dropped; this keeps the test
        // deterministic without changing page-runtime behavior.
        drop(runtime);
        boa_gc::force_collect();
        println!(
            "WPT {}: actual={} classification={classification:?}",
            case.path,
            actual.as_str()
        );
        if classification == Classification::Regression {
            let expected = case
                .known_failure
                .as_ref()
                .map(|known| known.status.as_str())
                .unwrap_or("PASS");
            regressions.push(format!(
                "WPT {} regression: expected={} actual={}; script errors={errors:?}; results={details}",
                case.path, expected, actual.as_str()
            ));
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
    assert!(regressions.is_empty(), "{}", regressions.join("\n"));
}
