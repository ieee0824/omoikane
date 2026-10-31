//! `Page` domain: navigation, reload, session history, frame tree, and the
//! document install and commit steps shared by page tasks.

use super::*;
use crate::js::{DocumentScriptStartup, DocumentScriptWait};

#[derive(Debug)]
pub(super) struct PendingDocumentStartup {
    base_url: Option<crate::http::Url>,
    started: std::time::Instant,
    first_frame_due: bool,
    scripts: Option<DocumentScriptStartup>,
}

fn finish_document_startup(
    runtime: &mut JsRuntime,
    base_url: Option<&crate::http::Url>,
    started: std::time::Instant,
) -> Result<(), CdpSessionError> {
    let errors = runtime.execute_document_scripts(base_url);
    finish_document_startup_events(runtime, errors, started)
}

fn finish_document_startup_events(
    runtime: &mut JsRuntime,
    errors: Vec<String>,
    started: std::time::Instant,
) -> Result<(), CdpSessionError> {
    trace_startup_page("document_scripts_finished", started);
    if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
        for error in errors {
            eprintln!("[omoikane][js-error] {error}");
        }
    }
    runtime
        .wire_inline_event_handlers()
        .map_err(CdpSessionError::JavaScript)?;
    runtime.fire_load().map_err(CdpSessionError::JavaScript)?;
    trace_startup_page("load_finished", started);
    Ok(())
}

fn trace_startup_page(phase: &str, started: std::time::Instant) {
    if std::env::var_os("OMOIKANE_TRACE_STARTUP").is_some() {
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_STARTUP_PAGE",
                &json!({
                    "phase": phase,
                    "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0,
                })
            )
        );
    }
}

/// Returns a bounded URL suitable for startup diagnostics. Credentials, query,
/// fragment, and non-HTTP payloads are never retained.
pub(super) fn diagnostic_navigation_url(input: &str) -> String {
    let Ok(mut url) = url::Url::parse(input) else {
        return String::new();
    };
    if !matches!(url.scheme(), "http" | "https") {
        return url.scheme().to_owned();
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    let value = url.to_string();
    value.chars().take(512).collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn navigation_response_diagnostic(
    loader_id: &str,
    commit: NavigationCommit,
    method: Method,
    status: u16,
    body_bytes: usize,
    decoded_chars: usize,
    request_url: &str,
    effective_url: &str,
    redirects: usize,
    headers: &[(String, String)],
) -> Value {
    let header = |name: &str| {
        headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| {
                value
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(256)
                    .collect::<String>()
            })
    };
    json!({
        "phase": "response_detail",
        "loader_id": loader_id,
        "commit": navigation_commit_label(commit),
        "method": method.as_str(),
        "status": status,
        "body_bytes": body_bytes,
        "decoded_chars": decoded_chars,
        "request_url": diagnostic_navigation_url(request_url),
        "effective_url": diagnostic_navigation_url(effective_url),
        "redirects": redirects,
        "content_type": header("content-type"),
        "content_length": header("content-length"),
        "content_encoding": header("content-encoding"),
        "transfer_encoding": header("transfer-encoding"),
    })
}

fn navigation_commit_label(commit: NavigationCommit) -> &'static str {
    match commit {
        NavigationCommit::Push => "push",
        NavigationCommit::Replace => "replace",
        NavigationCommit::Reload => "reload",
        NavigationCommit::Traverse(_) => "traverse",
    }
}

pub(super) fn navigation_history_diagnostic(
    phase: &'static str,
    loader_id: &str,
    commit: NavigationCommit,
    requested_url: &str,
    effective_url: Option<&str>,
    outcome: Option<&'static str>,
) -> Value {
    json!({
        "phase": phase,
        "loader_id": loader_id,
        "commit": navigation_commit_label(commit),
        "requested_url": diagnostic_navigation_url(requested_url),
        "effective_url": effective_url.map(diagnostic_navigation_url),
        "outcome": outcome,
    })
}

fn trace_navigation_response(value: &Value) {
    if std::env::var_os("OMOIKANE_TRACE_STARTUP").is_some() {
        eprintln!(
            "{}",
            crate::diagnostics::json_record("OMOIKANE_STARTUP_PAGE", value)
        );
    }
}

fn trace_navigation_history(
    phase: &'static str,
    loader_id: &str,
    commit: NavigationCommit,
    requested_url: &str,
    effective_url: Option<&str>,
    outcome: Option<&'static str>,
) {
    if std::env::var_os("OMOIKANE_TRACE_STARTUP").is_some() {
        let value = navigation_history_diagnostic(
            phase,
            loader_id,
            commit,
            requested_url,
            effective_url,
            outcome,
        );
        eprintln!(
            "{}",
            crate::diagnostics::json_record("OMOIKANE_NAVIGATION_HISTORY", &value)
        );
    }
}

fn navigation_json_error(
    error: impl std::fmt::Display,
    loader_id: &str,
    commit: NavigationCommit,
    requested_url: &str,
    effective_url: Option<&str>,
) -> JsonRpcError {
    trace_navigation_history(
        "finished",
        loader_id,
        commit,
        requested_url,
        effective_url,
        Some("error"),
    );
    JsonRpcError {
        code: -32000,
        message: error.to_string(),
    }
}

enum NavigationResponseDisposition {
    Install,
    KeepCurrent,
}

fn navigation_response_disposition(
    status: u16,
) -> Result<NavigationResponseDisposition, CdpSessionError> {
    match status {
        204 | 205 => Ok(NavigationResponseDisposition::KeepCurrent),
        304 => Err(CdpSessionError::MissingCachedNavigationResponse),
        _ => Ok(NavigationResponseDisposition::Install),
    }
}

fn resolve_navigation_response(
    status: u16,
    loader_id: &str,
    commit: NavigationCommit,
    requested_url: &str,
    effective_url: &str,
) -> Result<NavigationResponseDisposition, JsonRpcError> {
    navigation_response_disposition(status).map_err(|error| {
        navigation_json_error(error, loader_id, commit, requested_url, Some(effective_url))
    })
}

fn trace_pending_commit_error(pending: &PendingDocumentCommit) {
    trace_navigation_history(
        "finished",
        &pending.loader_id,
        pending.history_commit,
        &pending.requested_url,
        Some(&pending.url),
        Some("error"),
    );
}

fn is_fragment_only_navigation(current: &str, target: &str) -> bool {
    let (current_base, current_fragment) = current.split_once('#').unwrap_or((current, ""));
    let (target_base, target_fragment) = target.split_once('#').unwrap_or((target, ""));
    current_base == target_base && current_fragment != target_fragment
}

impl CdpSession {
    /// Lets a native host paint parsed HTML before running initial page scripts.
    /// The first opportunity exposes parsed HTML. Later opportunities preserve
    /// parser/defer execution order while polling external script responses,
    /// and deliver load events when startup completes.
    /// Embedders must keep driving opportunities while render demand is active.
    pub fn set_deferred_document_startup(&mut self, enabled: bool) {
        self.defer_document_startup = enabled;
    }

    /// Returns whether native presentation is still waiting for initial scripts.
    /// Hosts use completion to rebase their wall clock, so newly created timers
    /// do not inherit time spent fetching and executing startup scripts.
    pub fn has_pending_document_startup(&self) -> bool {
        self.pending_document_startup.is_some()
    }

    pub(super) fn advance_deferred_document_startup(
        &mut self,
        elapsed_ms: u64,
    ) -> Result<bool, JsonRpcError> {
        self.advance_deferred_document_startup_with_wait(elapsed_ms, DocumentScriptWait::Poll)
    }

    pub(super) fn advance_deferred_document_startup_with_wait(
        &mut self,
        elapsed_ms: u64,
        wait: DocumentScriptWait,
    ) -> Result<bool, JsonRpcError> {
        let Some(mut pending) = self.pending_document_startup.take() else {
            return Ok(false);
        };
        self.runtime.advance_clock_without_tasks(elapsed_ms);
        if pending.first_frame_due {
            pending.first_frame_due = false;
            trace_startup_page("parsed_document_frame", pending.started);
            self.pending_document_startup = Some(pending);
            return Ok(true);
        }
        let startup = pending
            .scripts
            .get_or_insert_with(|| self.runtime.begin_document_scripts(pending.base_url.take()));
        if self.runtime.poll_document_scripts(startup, wait) {
            finish_document_startup_events(
                &mut self.runtime,
                startup.take_errors(),
                pending.started,
            )
            .map_err(|error| JsonRpcError {
                code: -32000,
                message: error.to_string(),
            })?;
        } else {
            self.pending_document_startup = Some(pending);
            // A parser resource wait leaves the owner available for resource
            // completions and task checkpoints, before its next host opportunity.
            self.runtime.run_until_idle().map_err(js_error)?;
        }
        Ok(true)
    }

    fn is_javascript_url(url: &str) -> bool {
        url::Url::parse(url).is_ok_and(|url| url.scheme() == "javascript")
    }

    /// Runs a javascript: navigation in the active Document. HTML string
    /// completion replaces the current history entry at its existing URL.
    fn run_javascript_navigation(
        &mut self,
        url: &str,
        client: Option<&crate::js::NavigationSecuritySnapshot>,
    ) -> Result<(), JsonRpcError> {
        let Some(html) = self
            .runtime
            .evaluate_javascript_navigation_from_client(url, client)
            .map_err(js_error)?
        else {
            return Ok(());
        };
        let current_url = self.current_url.clone();
        self.install_document_with_headers(
            &current_url,
            &DecodedHtml::from_string(html.as_str()),
            self.history_entries.len(),
            "null",
            &[],
            0,
            None,
            true,
        )
        .map_err(|error| JsonRpcError {
            code: -32000,
            message: error.to_string(),
        })?;
        self.commit_history_url(&current_url, NavigationCommit::Replace, None);
        self.sync_history_length()?;
        self.emit(
            "Page.frameNavigated",
            json!({
                "frame": {
                    "id": self.frame_id,
                    "url": current_url,
                    "mimeType": "text/html",
                }
            }),
        );
        self.emit(
            "Page.loadEventFired",
            json!({ "timestamp": self.next_loader_id }),
        );
        Ok(())
    }

    pub(super) fn page_set_web_lifecycle_state(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
        let state = require_string(params, "state")?;
        self.lifecycle_frozen = match state.as_str() {
            "active" => false,
            "frozen" => true,
            _ => {
                return Err(JsonRpcError {
                    code: -32602,
                    message: format!("Unsupported lifecycle state: {state}"),
                });
            }
        };
        self.runtime
            .set_page_visibility(self.host_hidden || self.lifecycle_frozen);
        Ok(json!({}))
    }

    fn leave_fullscreen_for_navigation(&mut self) {
        let _ = self.release_pointer_lock_from_host();
        self.pending_pointer_release |= matches!(
            self.runtime.take_pointer_lock_transition(),
            Some(PointerLockTransition::Release)
        );
        let _ = self.runtime.exit_fullscreen_from_host();
        if let Some(transition) = self.runtime.take_fullscreen_transition() {
            self.pending_fullscreen_transition = Some(transition);
        }
    }

    fn dispatch_document_departure(&mut self) {
        let _ = self
            .runtime
            .eval("window.dispatchEvent(new Event('beforeunload'))");
        let _ = self.runtime.run_jobs();
        self.runtime.set_page_visibility(true);
        let _ = self.runtime.eval(
            "window.dispatchEvent(new Event('pagehide')); \
             window.dispatchEvent(new Event('unload')); \
             if (typeof globalThis.__omoikane_permission_teardown === 'function') \
             globalThis.__omoikane_permission_teardown();",
        );
        let _ = self.runtime.run_jobs();
    }

    pub(super) fn page_navigate(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let url = require_string(params, "url")?;
        if Self::is_javascript_url(&url) {
            self.run_javascript_navigation(&url, None)?;
            self.drive_navigation_requests()?;
            return Ok(json!({ "frameId": self.frame_id }));
        }
        // The first real navigation replaces the initial empty about:blank
        // entry. Later Page.navigate calls append normal session-history
        // entries.
        let commit = if self.current_url == "about:blank"
            && self.history_entries.len() == 1
            && self.history_index == 0
        {
            NavigationCommit::Replace
        } else {
            NavigationCommit::Push
        };
        let result = self.navigate_to(&url, commit)?;
        if self.pending_document_startup.is_none() {
            self.drive_navigation_requests()?;
        }
        Ok(result)
    }

    pub(crate) fn prepare_page_navigate(
        &mut self,
        params: &Value,
    ) -> Result<PreparedPageNavigation, JsonRpcError> {
        let url = require_string(params, "url")?;
        if Self::is_javascript_url(&url) {
            self.run_javascript_navigation(&url, None)?;
            self.drive_navigation_requests()?;
            return Ok(PreparedPageNavigation::Complete(json!({
                "frameId": self.frame_id
            })));
        }
        let commit = if self.current_url == "about:blank"
            && self.history_entries.len() == 1
            && self.history_index == 0
        {
            NavigationCommit::Replace
        } else {
            NavigationCommit::Push
        };
        if is_fragment_only_navigation(&self.current_url, &url) {
            return self
                .navigate_to(&url, commit)
                .map(PreparedPageNavigation::Complete);
        }
        self.prepare_page_navigation_request(&url, commit)
    }

    pub(crate) fn prepare_page_reload(&mut self) -> Result<PreparedPageNavigation, JsonRpcError> {
        let url = self.current_url.clone();
        self.prepare_page_navigation_request(&url, NavigationCommit::Reload)
    }

    fn prepare_page_navigation_request(
        &mut self,
        url: &str,
        history_commit: NavigationCommit,
    ) -> Result<PreparedPageNavigation, JsonRpcError> {
        let loader_id = self.next_loader_id.to_string();
        self.next_loader_id += 1;
        self.emit(
            "Network.requestWillBeSent",
            json!({
                "requestId": loader_id,
                "documentURL": url,
                "request": { "url": url, "method": "GET" },
                "type": "Document",
            }),
        );
        trace_navigation_history("requested", &loader_id, history_commit, url, None, None);
        let (html, status, document_url, response_headers) = self
            .load_page_request(url, Method::Get, None, None, &loader_id, history_commit)
            .map_err(|error| navigation_json_error(error, &loader_id, history_commit, url, None))?;
        let disposition =
            resolve_navigation_response(status, &loader_id, history_commit, url, &document_url)?;
        match disposition {
            NavigationResponseDisposition::KeepCurrent => {
                self.emit(
                    "Network.responseReceived",
                    json!({
                        "requestId": loader_id,
                        "type": "Document",
                        "response": {
                            "url": document_url,
                            "status": status,
                            "mimeType": "text/html",
                        },
                    }),
                );
                trace_navigation_history(
                    "finished",
                    &loader_id,
                    history_commit,
                    url,
                    Some(&document_url),
                    Some("preserved"),
                );
                return Ok(PreparedPageNavigation::Complete(json!({
                    "frameId": self.frame_id,
                    "loaderId": loader_id,
                })));
            }
            NavigationResponseDisposition::Install => {}
        }
        let form_state = self
            .navigation_form_state(history_commit, &document_url)
            .map_err(|error| {
                trace_navigation_history(
                    "finished",
                    &loader_id,
                    history_commit,
                    url,
                    Some(&document_url),
                    Some("error"),
                );
                error
            })?;
        let (history_length, history_state) = self.prospective_history_state(history_commit);
        let (task, mut commit) = self
            .prepare_document_page_task(
                &document_url,
                &html,
                history_length,
                &history_state,
                &response_headers,
                status,
                form_state.as_ref(),
            )
            .map_err(|error| {
                navigation_json_error(error, &loader_id, history_commit, url, Some(&document_url))
            })?;
        commit.history_commit = history_commit;
        commit.loader_id = loader_id.clone();
        commit.status = status;
        commit.requested_url = url.to_owned();
        Ok(PreparedPageNavigation::Pending {
            task,
            commit,
            response: json!({ "frameId": self.frame_id, "loaderId": loader_id }),
        })
    }

    fn navigate_to(&mut self, url: &str, commit: NavigationCommit) -> Result<Value, JsonRpcError> {
        self.navigate_with_request(url, commit, Method::Get, None, None)
    }

    fn navigate_with_request(
        &mut self,
        url: &str,
        commit: NavigationCommit,
        method: Method,
        body: Option<Vec<u8>>,
        content_type: Option<String>,
    ) -> Result<Value, JsonRpcError> {
        let navigation_started = std::time::Instant::now();
        let loader_id = self.next_loader_id.to_string();
        self.next_loader_id += 1;

        let same_document_navigation = match commit {
            NavigationCommit::Traverse(index) => {
                self.history_entries[index].document_generation == self.document_generation
            }
            _ => is_fragment_only_navigation(&self.current_url, url),
        };
        if method == Method::Get && commit != NavigationCommit::Reload && same_document_navigation {
            trace_navigation_history("requested", &loader_id, commit, url, None, None);
            let form_state = self.navigation_form_state(commit, url)?;
            let previous_url = self.current_url.clone();
            self.commit_history_url(url, commit, None);
            self.current_url = url.to_string();
            self.runtime.commit_same_document_url(url);
            self.sync_history_length()?;
            self.runtime
                .eval(&format!(
                    "__omoikane_commit_same_document_navigation({url:?}, {previous_url:?})"
                ))
                .and_then(|_| self.runtime.run_jobs())
                .map_err(js_error)?;
            if let Some(state) = form_state {
                self.runtime
                    .restore_form_state(&state, FormStateRestoreMode::Restore)
                    .map_err(js_error)?;
            }
            self.emit(
                "Page.navigatedWithinDocument",
                json!({ "frameId": self.frame_id, "url": url, "navigationType": "fragment" }),
            );
            trace_navigation_history(
                "finished",
                &loader_id,
                commit,
                url,
                Some(url),
                Some("committed"),
            );
            return Ok(json!({ "frameId": self.frame_id }));
        }

        self.emit(
            "Network.requestWillBeSent",
            json!({
                "requestId": loader_id,
                "documentURL": url,
                "request": { "url": url, "method": method.as_str() },
                "type": "Document",
            }),
        );
        trace_navigation_history("requested", &loader_id, commit, url, None, None);

        let (html, status, document_url, response_headers) = self
            .load_page_request(url, method, body, content_type, &loader_id, commit)
            .map_err(|error| navigation_json_error(error, &loader_id, commit, url, None))?;
        trace_startup_page("response_ready", navigation_started);

        let disposition =
            resolve_navigation_response(status, &loader_id, commit, url, &document_url)?;
        match disposition {
            NavigationResponseDisposition::KeepCurrent => {
                self.emit(
                    "Network.responseReceived",
                    json!({
                        "requestId": loader_id,
                        "type": "Document",
                        "response": {
                            "url": document_url,
                            "status": status,
                            "mimeType": "text/html",
                        },
                    }),
                );
                trace_navigation_history(
                    "finished",
                    &loader_id,
                    commit,
                    url,
                    Some(&document_url),
                    Some("preserved"),
                );
                return Ok(json!({ "frameId": self.frame_id, "loaderId": loader_id }));
            }
            NavigationResponseDisposition::Install => {}
        }

        let form_state = self
            .navigation_form_state(commit, &document_url)
            .map_err(|error| {
                trace_navigation_history(
                    "finished",
                    &loader_id,
                    commit,
                    url,
                    Some(&document_url),
                    Some("error"),
                );
                error
            })?;
        let (next_history_length, next_history_state) = self.prospective_history_state(commit);
        self.install_document_with_headers(
            &document_url,
            &html,
            next_history_length,
            &next_history_state,
            &response_headers,
            status,
            form_state.as_ref(),
            false,
        )
        .map_err(|error| {
            navigation_json_error(error, &loader_id, commit, url, Some(&document_url))
        })?;
        trace_startup_page("document_installed", navigation_started);

        self.commit_history_url(&document_url, commit, None);
        trace_navigation_history(
            "finished",
            &loader_id,
            commit,
            url,
            Some(&document_url),
            Some("committed"),
        );
        self.sync_history_length()?;
        if matches!(commit, NavigationCommit::Traverse(_)) {
            self.runtime
                .eval(&format!(
                    "__omoikane_commit_same_document_navigation({url:?}, {url:?})"
                ))
                .and_then(|_| self.runtime.run_jobs())
                .map_err(js_error)?;
        }

        self.emit(
            "Network.responseReceived",
            json!({
                "requestId": loader_id,
                "type": "Document",
                "response": {
                    "url": document_url,
                    "status": status,
                    "mimeType": "text/html",
                },
            }),
        );
        self.emit(
            "Page.frameNavigated",
            json!({
                "frame": {
                    "id": self.frame_id,
                    "url": self.current_url,
                    "mimeType": "text/html",
                }
            }),
        );
        self.emit(
            "Page.loadEventFired",
            json!({ "timestamp": self.next_loader_id }),
        );

        Ok(json!({
            "frameId": self.frame_id,
            "loaderId": loader_id,
        }))
    }

    pub(super) fn page_reload(&mut self) -> Result<Value, JsonRpcError> {
        let url = self.current_url.clone();
        self.navigate_to(&url, NavigationCommit::Reload)?;
        self.drive_navigation_requests()?;
        Ok(json!({}))
    }

    fn record_script_visit(&self, source: Option<VisitSource>, requested_url: &str) {
        let Some(source) = source else {
            return;
        };
        self.storage_manager
            .record_page_navigation(requested_url, &self.current_url, source);
    }

    /// Commits navigation requests queued by the active Runtime.
    ///
    /// Startup scripts in a newly installed Document may queue another
    /// navigation, so the new Runtime is checked again after every commit. The
    /// cap prevents a script redirect loop from blocking the embedding API.
    pub(super) fn drive_navigation_requests(&mut self) -> Result<(), JsonRpcError> {
        self.drive_navigation_requests_inner(true)
    }

    /// Commits navigation already requested by script without running another
    /// page task during native input or after a rendering opportunity.
    pub(super) fn commit_pending_navigation_requests(&mut self) -> Result<(), JsonRpcError> {
        self.drive_navigation_requests_inner(false)
    }

    fn drive_navigation_requests_inner(&mut self, pump_tasks: bool) -> Result<(), JsonRpcError> {
        const MAX_SCRIPT_NAVIGATIONS: usize = 32;
        for _ in 0..MAX_SCRIPT_NAVIGATIONS {
            if pump_tasks && self.pending_document_startup.is_none() {
                self.runtime.run_until_idle().map_err(js_error)?;
            } else {
                self.runtime.promote_navigation_tasks_without_running();
            }
            // Surface failures from tasks that ran at an earlier opportunity.
            // A failed task must not hide an already queued navigation.
            for error in self.runtime.take_task_errors() {
                eprintln!("[omoikane][js-error] {error}");
            }
            let Some((request, source, security)) =
                self.runtime.pop_navigation_request_with_source()
            else {
                return Ok(());
            };
            match request {
                NavigationRequest::Navigate { url, replace } => {
                    if Self::is_javascript_url(&url) {
                        self.run_javascript_navigation(&url, security.as_ref())?;
                        continue;
                    }
                    let previous_url = self.current_url.clone();
                    if let Err(error) = self.navigate_to(
                        &url,
                        if replace {
                            NavigationCommit::Replace
                        } else {
                            NavigationCommit::Push
                        },
                    ) {
                        self.restore_active_location(&previous_url);
                        return Err(error);
                    }
                    self.record_script_visit(source, &url);
                }
                NavigationRequest::FormSubmit {
                    url,
                    method,
                    body,
                    content_type,
                } => {
                    if Self::is_javascript_url(&url) {
                        self.run_javascript_navigation(&url, security.as_ref())?;
                        continue;
                    }
                    let previous_url = self.current_url.clone();
                    let method = if method.eq_ignore_ascii_case("POST") {
                        Method::Post
                    } else {
                        Method::Get
                    };
                    if let Err(error) = self.navigate_with_request(
                        &url,
                        NavigationCommit::Push,
                        method,
                        body,
                        content_type,
                    ) {
                        self.restore_active_location(&previous_url);
                        return Err(error);
                    }
                    self.record_script_visit(source, &url);
                }
                NavigationRequest::UpdateHistory {
                    url,
                    replace,
                    state_json,
                } => {
                    // A same-document entry still needs a snapshot before
                    // later changes overwrite the live controls' state.
                    self.navigation_form_state(NavigationCommit::Push, &url)?;
                    self.commit_history_url(
                        &url,
                        if replace {
                            NavigationCommit::Replace
                        } else {
                            NavigationCommit::Push
                        },
                        Some(state_json),
                    );
                    self.current_url = url.clone();
                    self.runtime.commit_history_api_url(&url);
                    self.sync_history_length()?;
                    self.emit(
                        "Page.navigatedWithinDocument",
                        json!({ "frameId": self.frame_id, "url": url, "navigationType": "historyApi" }),
                    );
                }
                NavigationRequest::Reload => {
                    let url = self.current_url.clone();
                    self.navigate_to(&url, NavigationCommit::Reload)?;
                }
                NavigationRequest::Traverse { delta } => {
                    let target = self.history_index as i64 + i64::from(delta);
                    if target < 0 || target >= self.history_entries.len() as i64 {
                        continue;
                    }
                    let target = target as usize;
                    if target == self.history_index {
                        continue;
                    }
                    let url = self.history_entries[target].url.clone();
                    let previous_url = self.current_url.clone();
                    if let Err(error) = self.navigate_to(&url, NavigationCommit::Traverse(target)) {
                        self.restore_active_location(&previous_url);
                        return Err(error);
                    }
                }
            }
        }
        Err(JsonRpcError {
            code: -32000,
            message: "script navigation limit exceeded".to_string(),
        })
    }

    fn commit_history_url(
        &mut self,
        url: &str,
        commit: NavigationCommit,
        state_json: Option<String>,
    ) {
        match commit {
            NavigationCommit::Push => {
                self.history_entries.truncate(self.history_index + 1);
                self.history_entries.push(SessionHistoryEntry {
                    url: url.to_string(),
                    state_json: state_json.unwrap_or_else(|| "null".to_string()),
                    form_state: None,
                    document_generation: self.document_generation,
                });
                self.history_index = self.history_entries.len() - 1;
            }
            NavigationCommit::Replace => {
                self.history_entries[self.history_index] = SessionHistoryEntry {
                    url: url.to_string(),
                    state_json: state_json.unwrap_or_else(|| "null".to_string()),
                    form_state: None,
                    document_generation: self.document_generation,
                };
            }
            NavigationCommit::Reload => {
                let entry = &mut self.history_entries[self.history_index];
                entry.document_generation = self.document_generation;
                if entry.url != url {
                    entry.url = url.to_string();
                    entry.form_state = None;
                }
            }
            NavigationCommit::Traverse(index) => {
                let previous_generation = self.history_entries[index].document_generation;
                self.history_index = index;
                // When a cross-Document traversal rebuilds one Document, all
                // of its same-Document history entries now share the new
                // live generation. The next back/forward between them must
                // reuse that Document rather than fetch it again.
                for entry in &mut self.history_entries {
                    if entry.document_generation == previous_generation {
                        entry.document_generation = self.document_generation;
                    }
                }
            }
        }
    }

    /// Saves values independently of the departing runtime. Only history
    /// traversal and reload may restore them, and a redirect to another URL
    /// must not receive the original document's private form state.
    fn navigation_form_state(
        &mut self,
        commit: NavigationCommit,
        document_url: &str,
    ) -> Result<Option<FormStateSnapshot>, JsonRpcError> {
        let current = self.runtime.capture_form_state().map_err(js_error)?;
        self.history_entries[self.history_index].form_state = Some(current);
        let entry = match commit {
            NavigationCommit::Reload => &self.history_entries[self.history_index],
            NavigationCommit::Traverse(index) => &self.history_entries[index],
            NavigationCommit::Push | NavigationCommit::Replace => return Ok(None),
        };
        let saved_url = entry.url.split('#').next().unwrap_or_default();
        let loaded_url = document_url.split('#').next().unwrap_or_default();
        if saved_url != loaded_url {
            return Ok(None);
        }
        Ok(entry.form_state.clone())
    }

    fn prospective_history_state(&self, commit: NavigationCommit) -> (usize, String) {
        match commit {
            NavigationCommit::Push => (self.history_index + 2, "null".to_string()),
            NavigationCommit::Replace => (self.history_entries.len(), "null".to_string()),
            NavigationCommit::Reload => (
                self.history_entries.len(),
                self.history_entries[self.history_index].state_json.clone(),
            ),
            NavigationCommit::Traverse(index) => (
                self.history_entries.len(),
                self.history_entries[index].state_json.clone(),
            ),
        }
    }

    fn sync_history_length(&mut self) -> Result<(), JsonRpcError> {
        let state_json = &self.history_entries[self.history_index].state_json;
        self.runtime
            .eval(&format!(
                "__omoikane_sync_history({}, {state_json:?})",
                self.history_entries.len(),
            ))
            .and_then(|_| self.runtime.run_jobs())
            .map_err(js_error)
    }

    fn restore_active_location(&mut self, url: &str) {
        let _ = self
            .runtime
            .eval(&format!("__omoikane_set_location({url:?})"));
        let _ = self.runtime.run_jobs();
    }

    pub(super) fn page_get_frame_tree(&self) -> Value {
        json!({
            "frameTree": {
                "frame": {
                    "id": self.frame_id,
                    "url": self.current_url,
                    "mimeType": "text/html",
                }
            }
        })
    }

    pub(super) fn load_page_request(
        &mut self,
        url: &str,
        method: Method,
        body: Option<Vec<u8>>,
        content_type: Option<String>,
        loader_id: &str,
        commit: NavigationCommit,
    ) -> Result<(DecodedHtml, u16, String, Vec<(String, String)>), CdpSessionError> {
        if method == Method::Get {
            if url == "about:blank" {
                let decoded = DecodedHtml::from_string("<html><head></head><body></body></html>");
                if std::env::var_os("OMOIKANE_TRACE_STARTUP").is_some() {
                    trace_navigation_response(&navigation_response_diagnostic(
                        loader_id,
                        commit,
                        method,
                        200,
                        decoded.text.len(),
                        decoded.text.chars().count(),
                        url,
                        url,
                        0,
                        &[],
                    ));
                }
                return Ok((decoded, 200, url.to_string(), Vec::new()));
            }
            if let Some(data) = crate::http::parse_data_uri(url)
                && data.mime_type.eq_ignore_ascii_case("text/html")
            {
                let decoded =
                    crate::html::encoding::decode_html_bytes(&data.data, Some(&data.mime_type));
                let headers = vec![("Content-Type".to_owned(), data.mime_type)];
                if std::env::var_os("OMOIKANE_TRACE_STARTUP").is_some() {
                    trace_navigation_response(&navigation_response_diagnostic(
                        loader_id,
                        commit,
                        method,
                        200,
                        data.data.len(),
                        decoded.text.chars().count(),
                        url,
                        url,
                        0,
                        &headers,
                    ));
                }
                return Ok((decoded, 200, url.to_string(), headers));
            }
        }
        let parsed: crate::http::url::Url = url.parse()?;
        let mut request = HttpRequest::new(method, parsed);
        if let Ok(site) = self.current_url.parse::<crate::http::Url>() {
            request.set_cookie_context(site, true);
        }
        if let Some(content_type) = content_type {
            request.set_header("Content-Type", content_type);
        }
        if let Some(body) = body {
            request.set_body(body);
        }
        let response = self.http_client.send(request)?;
        let effective_url = response
            .effective_url()
            .map(ToString::to_string)
            .unwrap_or_else(|| url.to_string());
        let response_headers = response.headers().to_vec();
        let decoded = decode_html_response(&response);
        if std::env::var_os("OMOIKANE_TRACE_STARTUP").is_some() {
            trace_navigation_response(&navigation_response_diagnostic(
                loader_id,
                commit,
                method,
                response.status_code(),
                response.body().len(),
                decoded.text.chars().count(),
                url,
                &effective_url,
                response.redirect_count(),
                &response_headers,
            ));
        }
        Ok((
            decoded,
            response.status_code(),
            effective_url,
            response_headers,
        ))
    }

    #[cfg(test)]
    pub(super) fn install_document(
        &mut self,
        url: &str,
        html: &str,
        history_length: usize,
        history_state_json: &str,
    ) -> Result<(), CdpSessionError> {
        self.install_document_with_headers(
            url,
            &DecodedHtml::from_string(html),
            history_length,
            history_state_json,
            &[],
            0,
            None,
            false,
        )
    }

    fn install_document_with_headers(
        &mut self,
        url: &str,
        html: &DecodedHtml,
        history_length: usize,
        history_state_json: &str,
        response_headers: &[(String, String)],
        response_status: u16,
        form_state: Option<&FormStateSnapshot>,
        inherit_javascript_security: bool,
    ) -> Result<(), CdpSessionError> {
        let install_started = std::time::Instant::now();
        let document = TreeBuilder::parse_decoded(html).document();
        trace_startup_page("html_parsed", install_started);
        let mut runtime = JsRuntime::with_document_url_and_storage(
            document,
            url,
            self.storage_manager.clone(),
            self.storage_session_id,
        )
        .map_err(CdpSessionError::JavaScript)?;
        trace_startup_page("runtime_ready", install_started);
        let generation = self.document_generation.saturating_add(1);
        self.configure_replacement_runtime(&mut runtime, generation)?;
        runtime.install_response_security_headers(response_headers);
        runtime.set_document_response_status(response_status);
        runtime.install_isolation_policy(response_headers);
        runtime.install_navigation_server_timing(response_headers);
        if inherit_javascript_security {
            runtime.inherit_javascript_navigation_security(&self.runtime);
        }
        if let Some(state) = form_state {
            runtime
                .restore_form_state(state, FormStateRestoreMode::Restore)
                .map_err(CdpSessionError::JavaScript)?;
        }
        runtime
            .eval(&format!(
                "__omoikane_sync_history({history_length}, {history_state_json:?})"
            ))
            .and_then(|_| runtime.run_jobs())
            .map_err(CdpSessionError::JavaScript)?;

        // The response and replacement Runtime are ready, so the active
        // Document can now leave without risking an unload followed by a
        // network failure that keeps the same Document alive. Listener errors
        // are reported like browser event-handler errors and do not cancel the
        // commit; cancellation policy for beforeunload dialogs belongs to the
        // future GUI integration.
        self.leave_fullscreen_for_navigation();
        self.runtime.terminate_workers();
        self.dispatch_document_departure();

        let base_url = url.parse::<crate::http::Url>().ok();
        let pending_startup = if self.defer_document_startup {
            runtime
                .eval("document.__readyState = 'loading'")
                .map_err(CdpSessionError::JavaScript)?;
            Some(PendingDocumentStartup {
                base_url,
                started: install_started,
                first_frame_due: true,
                scripts: None,
            })
        } else {
            finish_document_startup(&mut runtime, base_url.as_ref(), install_started)?;
            None
        };

        self.pending_document_startup = pending_startup;
        self.runtime.native_scroll_handle().revoke_policy();
        self.runtime = runtime;
        self.mouse_pressed_target = None;
        self.touch_target = None;
        self.last_touch_position = None;
        self.touch_scroll_allowed = true;
        self.drag_candidate = false;
        self.drag_active = false;
        self.document_generation = generation;
        self.remote_object_generations.clear();
        self.current_url = url.to_string();
        self.last_html = html.text.clone();
        self.rebuild_node_index();
        Ok(())
    }

    /// Builds a replacement document and moves its runtime into a suspendable
    /// startup task without disturbing the currently committed page.
    pub(crate) fn prepare_document_page_task(
        &mut self,
        url: &str,
        html: &DecodedHtml,
        history_length: usize,
        history_state_json: &str,
        response_headers: &[(String, String)],
        response_status: u16,
        form_state: Option<&FormStateSnapshot>,
    ) -> Result<(OwnedPageTask, PendingDocumentCommit), CdpSessionError> {
        let document = TreeBuilder::parse_decoded(html).document();
        let mut runtime = JsRuntime::with_document_url_and_storage(
            document,
            url,
            self.storage_manager.clone(),
            self.storage_session_id,
        )
        .map_err(CdpSessionError::JavaScript)?;
        let generation = self.document_generation.saturating_add(1);
        self.configure_replacement_runtime(&mut runtime, generation)?;
        runtime.install_response_security_headers(response_headers);
        runtime.set_document_response_status(response_status);
        runtime.install_isolation_policy(response_headers);
        runtime.install_navigation_server_timing(response_headers);
        if let Some(state) = form_state {
            runtime
                .restore_form_state(state, FormStateRestoreMode::Restore)
                .map_err(CdpSessionError::JavaScript)?;
        }
        runtime
            .eval(&format!(
                "__omoikane_sync_history({history_length}, {history_state_json:?})"
            ))
            .and_then(|_| runtime.run_jobs())
            .map_err(CdpSessionError::JavaScript)?;

        let base_url = url.parse::<crate::http::Url>().ok();
        let task = runtime.into_document_page_task(generation, base_url);
        Ok((
            task,
            PendingDocumentCommit {
                requested_url: url.to_string(),
                url: url.to_string(),
                html: html.text.clone(),
                generation,
                history_commit: NavigationCommit::Replace,
                loader_id: String::new(),
                status: 0,
            },
        ))
    }

    /// Installs a runtime returned by a completed page-startup task.
    ///
    /// Cancelled and stale tasks leave the currently committed page unchanged.
    pub(crate) fn commit_document_page_task(
        &mut self,
        mut completed: CompletedPageTask,
        pending: PendingDocumentCommit,
    ) -> Result<(), CdpSessionError> {
        if completed.generation != pending.generation
            || pending.generation != self.document_generation.saturating_add(1)
        {
            self.report_cdp_failure(CdpFailure::PageTask);
            trace_pending_commit_error(&pending);
            return Err(CdpSessionError::StalePageTaskCompletion);
        }
        match &completed.result {
            Err(PageTaskError::Cancelled) => {
                self.report_cdp_failure(CdpFailure::PageTask);
                trace_pending_commit_error(&pending);
                return Err(CdpSessionError::PageTaskCancelled);
            }
            Err(PageTaskError::TimedOut) => {
                self.report_cdp_failure(CdpFailure::PageTask);
                trace_pending_commit_error(&pending);
                return Err(CdpSessionError::PageTaskTimedOut);
            }
            Ok(_) => {}
        }

        let script_error_lines = take_page_task_script_error_lines(&mut completed);
        if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
            for line in script_error_lines {
                eprintln!("{line}");
            }
        }
        let mut runtime = completed.runtime;
        let (width, height) = self.runtime.presentation_viewport();
        runtime.set_viewport(width, height);
        // The presentation host can change while startup is suspended. Adopt
        // its latest snapshot before replacing the committed document so live
        // query lists in the new runtime observe the intervening update.
        runtime
            .set_media_environment(self.runtime.media_environment())
            .map_err(|error| {
                trace_pending_commit_error(&pending);
                CdpSessionError::JavaScript(error)
            })?;

        // Teardown is delayed until the replacement runtime has completed its
        // startup work, so cancellation or startup setup failure keeps the old
        // document intact.
        self.leave_fullscreen_for_navigation();
        self.runtime.terminate_workers();
        self.dispatch_document_departure();

        self.pending_document_startup = None;
        self.runtime.native_scroll_handle().revoke_policy();
        self.runtime = runtime;
        self.mouse_pressed_target = None;
        self.touch_target = None;
        self.last_touch_position = None;
        self.touch_scroll_allowed = true;
        self.drag_candidate = false;
        self.drag_active = false;
        self.document_generation = pending.generation;
        self.remote_object_generations.clear();
        self.current_url = pending.url;
        self.last_html = pending.html;
        self.rebuild_node_index();
        let current_url = self.current_url.clone();
        self.commit_history_url(&current_url, pending.history_commit, None);
        trace_navigation_history(
            "finished",
            &pending.loader_id,
            pending.history_commit,
            &pending.requested_url,
            Some(&current_url),
            Some("committed"),
        );
        self.emit(
            "Network.responseReceived",
            json!({
                "requestId": pending.loader_id,
                "type": "Document",
                "response": {
                    "url": self.current_url,
                    "status": pending.status,
                    "mimeType": "text/html",
                },
            }),
        );
        self.emit(
            "Page.frameNavigated",
            json!({ "frame": { "id": self.frame_id, "url": self.current_url, "mimeType": "text/html" } }),
        );
        self.emit(
            "Page.loadEventFired",
            json!({ "timestamp": self.next_loader_id }),
        );
        Ok(())
    }
}
