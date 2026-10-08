//! Execute one WPT smoke case and return its raw harness result.
use super::model::ActualStatus;
use omoikane::dom::{Node, NodeHandle};
use omoikane::html::TreeBuilder;
use omoikane::http::{Client, Url};
use omoikane::js::{JsRuntime, NavigationRequest, SandboxConfig};
use std::path::{Path, PathBuf};
use std::time::Duration;

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

fn case_timeout(source: &[u8], document: &NodeHandle, javascript: bool) -> Duration {
    // Pinned testharness.js defines 10 seconds for normal and 60 for long.
    // Use upstream metadata rather than an unrelated embedder's 5-second limit.
    let mut long = javascript
        && String::from_utf8_lossy(source)
            .lines()
            .any(|line| line.trim() == "// META: timeout=long");
    let mut pending = vec![document.clone()];
    while let Some(node) = pending.pop() {
        if node.local_name().as_deref() == Some("meta")
            && node
                .get_attribute("name")
                .is_some_and(|value| value.eq_ignore_ascii_case("timeout"))
            && node
                .get_attribute("content")
                .is_some_and(|value| value.eq_ignore_ascii_case("long"))
        {
            long = true;
        }
        pending.extend(node.child_nodes());
    }
    Duration::from_secs(if long { 60 } else { 10 })
}

/// The smoke runner owns one Document, so it only commits requests that keep
/// that Document alive. This mirrors the fragment/history path of CDP's
/// navigation driver; cross-Document loads remain outside this runner.
struct SameDocumentHistory {
    entries: Vec<String>,
    index: usize,
}

impl SameDocumentHistory {
    fn new(url: String) -> Self {
        Self {
            entries: vec![url],
            index: 0,
        }
    }

    fn current(&self) -> &str {
        &self.entries[self.index]
    }

    fn commit(&mut self, url: String, replace: bool) {
        if replace {
            self.entries[self.index] = url;
        } else {
            self.entries.truncate(self.index + 1);
            self.entries.push(url);
            self.index += 1;
        }
    }

    fn sync_length(&self, runtime: &mut JsRuntime) -> Result<(), String> {
        runtime
            .eval(&format!("__omoikane_sync_history({})", self.entries.len()))
            .map(|_| ())
            .map_err(|error| format!("sync WPT history: {error}"))
    }

    fn commit_fragment(
        &mut self,
        runtime: &mut JsRuntime,
        url: String,
        replace: bool,
    ) -> Result<(), String> {
        let previous = self.current().to_owned();
        self.commit(url.clone(), replace);
        runtime.commit_same_document_url(&url);
        self.sync_length(runtime)?;
        let url = serde_json::to_string(&url).expect("serialize navigation URL");
        let previous = serde_json::to_string(&previous).expect("serialize previous URL");
        runtime
            .eval(&format!(
                "__omoikane_commit_same_document_navigation({url}, {previous})"
            ))
            .map_err(|error| format!("commit WPT fragment navigation: {error}"))?;
        runtime
            .run_jobs()
            .map_err(|error| format!("run WPT fragment jobs: {error}"))
    }

    fn drive(&mut self, runtime: &mut JsRuntime) -> Result<bool, String> {
        const MAX_SCRIPT_NAVIGATIONS: usize = 32;
        let mut committed = false;
        for _ in 0..MAX_SCRIPT_NAVIGATIONS {
            runtime
                .run_until_idle()
                .map_err(|error| format!("run WPT navigation tasks: {error}"))?;
            // CDP accepts one request per checkpoint. A later script may
            // enqueue another request while the first commit dispatches events.
            let Some(request) = runtime.take_navigation_requests().into_iter().next() else {
                return Ok(committed);
            };
            match request {
                NavigationRequest::Navigate { url, replace }
                    if is_fragment_only_navigation(self.current(), &url) =>
                {
                    self.commit_fragment(runtime, url, replace)?;
                    committed = true;
                }
                NavigationRequest::UpdateHistory { url, replace, .. } => {
                    self.commit(url, replace);
                    self.sync_length(runtime)?;
                    committed = true;
                }
                NavigationRequest::Traverse { delta } => {
                    let Some(index) = self.index.checked_add_signed(delta as isize) else {
                        continue;
                    };
                    if index >= self.entries.len() || index == self.index {
                        continue;
                    }
                    let previous = self.current().to_owned();
                    self.index = index;
                    let url = self.current().to_owned();
                    runtime.commit_same_document_url(&url);
                    self.sync_length(runtime)?;
                    let url = serde_json::to_string(&url).expect("serialize history URL");
                    let previous =
                        serde_json::to_string(&previous).expect("serialize previous URL");
                    runtime
                        .eval(&format!(
                            "__omoikane_commit_same_document_navigation({url}, {previous})"
                        ))
                        .map_err(|error| format!("traverse WPT history: {error}"))?;
                    committed = true;
                }
                // A WPT case may request a new Document, but this smoke
                // runner has no browser session to install it. Preserve the
                // existing behavior for those requests.
                _ => {}
            }
        }
        Err("WPT same-document navigation limit exceeded".to_string())
    }
}

fn is_fragment_only_navigation(current: &str, target: &str) -> bool {
    let (current_base, current_fragment) = current.split_once('#').unwrap_or((current, ""));
    let (target_base, target_fragment) = target.split_once('#').unwrap_or((target, ""));
    current_base == target_base && current_fragment != target_fragment
}

fn drive_case_tasks(
    runtime: &mut JsRuntime,
    history: &mut SameDocumentHistory,
    errors: &mut Vec<String>,
    timeout: Duration,
) {
    const STEP_MS: u64 = 16;
    for _ in 0..timeout.as_millis() / u128::from(STEP_MS) {
        let committed = match history.drive(runtime) {
            Ok(committed) => committed,
            Err(error) => {
                errors.push(error);
                return;
            }
        };
        if js_bool(runtime, "globalThis.__wpt_complete === true") {
            return;
        }
        runtime.run_timers(STEP_MS, STEP_MS, 128);
        // Timers advance the virtual clock; the rendering opportunity must
        // share that timestamp rather than advance it a second time.
        if let Err(error) = runtime.run_animation_frame(0) {
            errors.push(format!("render WPT frame: {error}"));
            return;
        }
        if !committed && !runtime.has_pending_timers() && !runtime.has_pending_animation_frames() {
            // Give any non-timer tasks run by this tick one more checkpoint.
            if let Err(error) = history.drive(runtime) {
                errors.push(error);
            }
            return;
        }
    }
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
    let javascript = path.ends_with(".any.js") || path.ends_with(".window.js");
    let document_source = if javascript {
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
    let timeout = case_timeout(response.body(), &document, javascript);
    let base: Url = url.parse().expect("parse WPT URL");
    let mut runtime = JsRuntime::with_document_sandbox_and_url(
        document,
        SandboxConfig {
            timeout,
            ..SandboxConfig::default()
        },
        &url,
    )
    .expect("create WPT runtime");
    // The bootstrap itself creates a large graph of host API constructors.
    // Collect its short-lived initialization temporaries before page code
    // starts allocating, keeping each WPT case's GC pressure bounded.
    boa_gc::force_collect();
    let mut errors = runtime.execute_document_scripts(Some(&base));
    runtime
        .wire_inline_event_handlers()
        .expect("wire WPT handlers");
    runtime.fire_load().expect("fire WPT load");
    let mut history = SameDocumentHistory::new(url);
    if path == "page-visibility/visibility-state-entry.tentative.html" {
        drive_visibility_state_testdriver(&mut runtime, &mut errors);
    } else {
        drive_case_tasks(&mut runtime, &mut history, &mut errors, timeout);
    }
    // WPTs commonly observe rendering steps through nested
    // requestAnimationFrame callbacks. Drive a bounded number of explicit
    // opportunities after load so resize/scroll events queued for a frame
    // can settle without turning a self-rescheduling callback into an
    // unbounded test run.
    runtime.run_animation_frames(32, 16);
    runtime.run_jobs().expect("drain WPT jobs");
    if let Err(error) = history.drive(&mut runtime) {
        errors.push(error);
    }
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
    let harness = runtime
        .eval("JSON.stringify({status: globalThis.__wpt_harness_status, message: globalThis.__wpt_harness_message || ''})")
        .ok()
        .and_then(|value| value.as_string().map(|text| text.to_std_string_escaped()))
        .unwrap_or_default();
    let details = format!("{details}; harness={harness}");
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
    fn task_driver_runs_rendering_opportunities_before_harness_timeout() {
        let document = TreeBuilder::parse("<!doctype html><html><body></body></html>").document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(
                "globalThis.__wpt_complete = false; globalThis.timedOut = false; \
                 setTimeout(() => { timedOut = true; __wpt_complete = true; }, 80); \
                 requestAnimationFrame(() => requestAnimationFrame(() => { __wpt_complete = true; }));",
            )
            .unwrap();
        let mut history = SameDocumentHistory::new("about:blank".to_string());
        let mut errors = Vec::new();
        drive_case_tasks(
            &mut runtime,
            &mut history,
            &mut errors,
            Duration::from_secs(10),
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert!(js_bool(&mut runtime, "__wpt_complete && !timedOut"));
    }

    #[test]
    fn timeout_metadata_uses_pinned_harness_normal_and_long_budgets() {
        let document = TreeBuilder::parse("<html><head></head></html>").document();
        assert_eq!(case_timeout(b"", &document, false), Duration::from_secs(10));
        assert_eq!(
            case_timeout(b"// META: timeout=long\n", &document, true),
            Duration::from_secs(60)
        );
        assert_eq!(
            case_timeout(b"// META: timeout=long\n", &document, false),
            Duration::from_secs(10)
        );
        let long = TreeBuilder::parse("<meta name='timeout' content='long'>").document();
        assert_eq!(case_timeout(b"", &long, false), Duration::from_secs(60));
    }

    #[test]
    fn fragment_requests_commit_hashchange_target_and_history() {
        let url = "http://example.test/case";
        let document = TreeBuilder::parse(
            "<!doctype html><a href='#second'>go</a><div id='second'>target</div>",
        )
        .document();
        let mut runtime = JsRuntime::with_document_and_url(document, url).unwrap();
        runtime
            .eval("globalThis.changes=[]; addEventListener('hashchange', e => changes.push([e.oldURL,e.newURL])); document.querySelector('a').click()")
            .unwrap();
        let mut history = SameDocumentHistory::new(url.to_owned());
        assert!(history.drive(&mut runtime).unwrap());
        assert!(js_bool(
            &mut runtime,
            "location.hash === '#second' && document.querySelector('#second').matches(':target') && history.length === 2 && changes.length === 1 && changes[0][0] === 'http://example.test/case' && changes[0][1] === 'http://example.test/case#second'"
        ));

        runtime.eval("history.back()").unwrap();
        assert!(history.drive(&mut runtime).unwrap());
        assert!(js_bool(
            &mut runtime,
            "location.hash === '' && !document.querySelector('#second').matches(':target') && history.length === 2 && changes.length === 2"
        ));

        runtime.eval("history.forward()").unwrap();
        assert!(history.drive(&mut runtime).unwrap());
        assert!(js_bool(
            &mut runtime,
            "location.hash === '#second' && document.querySelector('#second').matches(':target') && history.length === 2 && changes.length === 3"
        ));

        runtime.eval("location.assign('#')").unwrap();
        assert!(history.drive(&mut runtime).unwrap());
        assert!(js_bool(
            &mut runtime,
            "location.href === 'http://example.test/case#' && location.hash === '' && !document.querySelector('#second').matches(':target') && history.length === 3 && changes.length === 4"
        ));
    }

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
