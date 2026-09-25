//! Execute one WPT smoke case and return its raw harness result.
use super::model::ActualStatus;
use omoikane::html::TreeBuilder;
use omoikane::http::{Client, Url};
use omoikane::js::JsRuntime;
use std::path::{Path, PathBuf};

pub(super) struct CaseExecution {
    pub(super) actual: ActualStatus,
    pub(super) script_errors: Vec<String>,
    pub(super) subtests: serde_json::Value,
    pub(super) details: String,
}

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

fn actual_status(errors: &[String], complete: bool, passed: bool) -> ActualStatus {
    if !errors.is_empty() {
        ActualStatus::Error
    } else if passed {
        ActualStatus::Pass
    } else if complete {
        ActualStatus::Fail
    } else {
        ActualStatus::Timeout
    }
}

pub(super) fn run_case(base_url: &str, path: &str) -> CaseExecution {
    let url = format!("{}/{}", base_url, path);
    let mut client = Client::new();
    let response = client
        .get(&url)
        .unwrap_or_else(|error| panic!("GET {url}: {error}"));
    assert_eq!(
        response.status_code(),
        200,
        "WPT resource missing: {}",
        path
    );
    let document_source = if path.ends_with(".any.js") || path.ends_with(".window.js") {
        let dependencies = script_dependencies(response.body(), &path)
            .into_iter()
            .map(|path| format!("<script src=\"{path}\"></script>"))
            .collect::<String>();
        format!(
            "<!doctype html><script src=\"/resources/testharness.js\"></script>\
                 <script src=\"/resources/testharnessreport.js\"></script>{dependencies}\
                 <script src=\"/{0}\"></script>",
            path
        )
    } else {
        String::from_utf8_lossy(response.body()).into_owned()
    };
    let document = TreeBuilder::parse(&document_source).document();
    let base: Url = url.parse().expect("parse WPT URL");
    let mut runtime = JsRuntime::with_document_and_url(document, &url).expect("create WPT runtime");
    // The bootstrap itself creates a large graph of host API constructors.
    // Collect its short-lived initialization temporaries before page code
    // starts allocating, keeping each WPT case's GC pressure bounded.
    boa_gc::force_collect();
    let mut errors = runtime.execute_document_scripts(Some(&base));
    runtime
        .wire_inline_event_handlers()
        .expect("wire WPT handlers");
    runtime.fire_load().expect("fire WPT load");
    if path == "page-visibility/visibility-state-entry.tentative.html" {
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
    let actual = actual_status(&errors, complete, passed);
    let details = runtime
        .eval("JSON.stringify(globalThis.__wpt_results||[])")
        .ok()
        .and_then(|value| value.as_string().map(|text| text.to_std_string_escaped()))
        .unwrap_or_else(|| "[]".to_string());
    let subtests = serde_json::from_str(&details).unwrap_or(serde_json::Value::Null);
    // Each WPT case uses a fresh Boa realm.  The main branch's expanded
    // bootstrap creates considerably more short-lived objects than the
    // original smoke set, so dropping the runtime alone can leave enough
    // allocator pressure accumulated across dozens of cases to make this
    // bounded integration test request an absurd allocation.  Collect only
    // after the provider and realm have been dropped; this keeps the test
    // deterministic without changing page-runtime behavior.
    drop(runtime);
    boa_gc::force_collect();
    CaseExecution {
        actual,
        script_errors: errors,
        subtests,
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_status_prioritizes_script_errors_then_harness_result() {
        let error = vec!["script failed".to_string()];
        assert_eq!(actual_status(&error, true, true), ActualStatus::Error);
        assert_eq!(actual_status(&[], true, true), ActualStatus::Pass);
        assert_eq!(actual_status(&[], true, false), ActualStatus::Fail);
        assert_eq!(actual_status(&[], false, false), ActualStatus::Timeout);
    }

    #[test]
    fn script_dependencies_resolve_relative_and_absolute_meta_paths() {
        let source = b"// META: script=lib/helper.js\n// META: script=/resources/shared.js\n// META: script=../outside.js\n// META: script=lib/../outside.js\n";
        assert_eq!(
            script_dependencies(source, "dom/tests/case.any.js"),
            ["/dom/tests/lib/helper.js", "/resources/shared.js"]
        );
    }
}
