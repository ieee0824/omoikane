//! `Page` domain: navigation, reload, session history, frame tree, and the
//! document install and commit steps shared by page tasks.

use super::*;

fn is_fragment_only_navigation(current: &str, target: &str) -> bool {
    let (current_base, current_fragment) = current.split_once('#').unwrap_or((current, ""));
    let (target_base, target_fragment) = target.split_once('#').unwrap_or((target, ""));
    current_base == target_base && current_fragment != target_fragment
}

impl CdpSession {
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
        self.drive_navigation_requests()?;
        Ok(result)
    }

    pub(crate) fn prepare_page_navigate(
        &mut self,
        params: &Value,
    ) -> Result<PreparedPageNavigation, JsonRpcError> {
        let url = require_string(params, "url")?;
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
        let (html, status, document_url, csp_headers) = self
            .load_page_request(url, Method::Get, None, None)
            .map_err(|error| JsonRpcError {
                code: -32000,
                message: error.to_string(),
            })?;
        let form_state = self.navigation_form_state(history_commit, &document_url)?;
        let (history_length, history_state) = self.prospective_history_state(history_commit);
        let (task, mut commit) = self
            .prepare_document_page_task(
                &document_url,
                &html,
                history_length,
                &history_state,
                &csp_headers,
                form_state.as_ref(),
            )
            .map_err(|error| JsonRpcError {
                code: -32000,
                message: error.to_string(),
            })?;
        commit.history_commit = history_commit;
        commit.loader_id = loader_id.clone();
        commit.status = status;
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
        let loader_id = self.next_loader_id.to_string();
        self.next_loader_id += 1;

        let same_document_navigation = match commit {
            NavigationCommit::Traverse(index) => {
                self.history_entries[index].document_generation == self.document_generation
            }
            _ => is_fragment_only_navigation(&self.current_url, url),
        };
        if method == Method::Get && commit != NavigationCommit::Reload && same_document_navigation {
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

        let (html, status, document_url, csp_headers) = self
            .load_page_request(url, method, body, content_type)
            .map_err(|error| JsonRpcError {
                code: -32000,
                message: error.to_string(),
            })?;

        let form_state = self.navigation_form_state(commit, &document_url)?;
        let (next_history_length, next_history_state) = self.prospective_history_state(commit);
        self.install_document_with_csp(
            &document_url,
            &html,
            next_history_length,
            &next_history_state,
            &csp_headers,
            form_state.as_ref(),
        )
        .map_err(|error| JsonRpcError {
            code: -32000,
            message: error.to_string(),
        })?;

        self.commit_history_url(&document_url, commit, None);
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
        const MAX_SCRIPT_NAVIGATIONS: usize = 32;
        for _ in 0..MAX_SCRIPT_NAVIGATIONS {
            self.runtime.run_until_idle().map_err(js_error)?;
            // Tasks record page-script failures rather than aborting the loop, so
            // a navigation is not lost to one broken script. Surface them here,
            // where document script errors are already reported.
            for error in self.runtime.take_task_errors() {
                eprintln!("[omoikane][js-error] {error}");
            }
            let Some((request, source)) = self
                .runtime
                .take_navigation_requests_with_source()
                .into_iter()
                .next()
            else {
                return Ok(());
            };
            match request {
                NavigationRequest::Navigate { url, replace } => {
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
    ) -> Result<(String, u16, String, Vec<String>), CdpSessionError> {
        if method == Method::Get {
            if url == "about:blank" {
                return Ok((
                    "<html><head></head><body></body></html>".to_string(),
                    200,
                    url.to_string(),
                    Vec::new(),
                ));
            }
            if let Some(data) = crate::http::parse_data_uri(url)
                && data.mime_type.eq_ignore_ascii_case("text/html")
            {
                return Ok((
                    String::from_utf8_lossy(&data.data).into_owned(),
                    200,
                    url.to_string(),
                    Vec::new(),
                ));
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
        let csp_headers = response
            .headers()
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-security-policy"))
            .map(|(_, value)| value.clone())
            .collect();
        Ok((
            decode_html_response(&response),
            response.status_code(),
            effective_url,
            csp_headers,
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
        self.install_document_with_csp(url, html, history_length, history_state_json, &[], None)
    }

    fn install_document_with_csp(
        &mut self,
        url: &str,
        html: &str,
        history_length: usize,
        history_state_json: &str,
        csp_headers: &[String],
        form_state: Option<&FormStateSnapshot>,
    ) -> Result<(), CdpSessionError> {
        let document = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document_url_and_storage(
            document,
            url,
            self.storage_manager.clone(),
            self.storage_session_id,
        )
        .map_err(CdpSessionError::JavaScript)?;
        self.configure_replacement_runtime(&mut runtime)?;
        runtime.install_csp_policy(csp_headers);
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
        let script_errors = runtime.execute_document_scripts(base_url.as_ref());
        if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
            for error in script_errors {
                eprintln!("[omoikane][js-error] {error}");
            }
        }
        runtime
            .wire_inline_event_handlers()
            .map_err(CdpSessionError::JavaScript)?;
        runtime.fire_load().map_err(CdpSessionError::JavaScript)?;

        self.runtime = runtime;
        self.mouse_pressed_target = None;
        self.touch_target = None;
        self.last_touch_position = None;
        self.touch_scroll_allowed = true;
        self.drag_candidate = false;
        self.drag_active = false;
        self.document_generation = self.document_generation.saturating_add(1);
        self.remote_object_generations.clear();
        self.current_url = url.to_string();
        self.last_html = html.to_string();
        self.rebuild_node_index();
        Ok(())
    }

    /// Builds a replacement document and moves its runtime into a suspendable
    /// startup task without disturbing the currently committed page.
    pub(crate) fn prepare_document_page_task(
        &mut self,
        url: &str,
        html: &str,
        history_length: usize,
        history_state_json: &str,
        csp_headers: &[String],
        form_state: Option<&FormStateSnapshot>,
    ) -> Result<(OwnedPageTask, PendingDocumentCommit), CdpSessionError> {
        let document = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document_url_and_storage(
            document,
            url,
            self.storage_manager.clone(),
            self.storage_session_id,
        )
        .map_err(CdpSessionError::JavaScript)?;
        self.configure_replacement_runtime(&mut runtime)?;
        runtime.install_csp_policy(csp_headers);
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

        let generation = self.document_generation.saturating_add(1);
        let base_url = url.parse::<crate::http::Url>().ok();
        let task = runtime.into_document_page_task(generation, base_url);
        Ok((
            task,
            PendingDocumentCommit {
                url: url.to_string(),
                html: html.to_string(),
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
            return Err(CdpSessionError::StalePageTaskCompletion);
        }
        match &completed.result {
            Err(PageTaskError::Cancelled) => {
                self.report_cdp_failure(CdpFailure::PageTask);
                return Err(CdpSessionError::PageTaskCancelled);
            }
            Err(PageTaskError::TimedOut) => {
                self.report_cdp_failure(CdpFailure::PageTask);
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
            .map_err(CdpSessionError::JavaScript)?;

        // Teardown is delayed until the replacement runtime has completed its
        // startup work, so cancellation or startup setup failure keeps the old
        // document intact.
        self.leave_fullscreen_for_navigation();
        self.runtime.terminate_workers();
        self.dispatch_document_departure();

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
