//! CSP notifications owned by their document and queued as networking tasks.
use super::*;
#[path = "security_reporting/endpoints.rs"]
mod endpoints;
pub(super) use endpoints::parse_reporting_endpoints;
#[cfg(test)]
#[path = "security_reporting/sample_tests.rs"]
mod sample_tests;

/// Response metadata owned by one environment, independent of history changes.
#[derive(Debug)]
pub(super) struct DocumentReportingMetadata {
    pub(super) creation_url: String,
    pub(super) response_status: u16,
}

/// Owned author-script provenance captured before an asynchronous Web API
/// wrapper loses its caller stack.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(super) struct CspSourceLocation {
    pub(super) source_file: String,
    pub(super) line_number: u32,
    pub(super) column_number: u32,
}

/// Finds the immediate classic author-script frame whose immutable metadata
/// and source position belong to that same frame. Only frames from this
/// bootstrap invocation are skipped; an unmarked author frame stops capture.
pub(super) fn capture_csp_source(context: &Context) -> Option<CspSourceLocation> {
    let mut frames = context.stack_trace();
    // This private binding is invoked by the trusted bootstrap module. Skip
    // only frames belonging to that exact runnable; the first different
    // runnable is the author boundary. If it has no classic provenance, stop
    // instead of attributing a module/eval/document.write call to an outer
    // classic caller.
    let bootstrap = frames.next()?.script_or_module_host_defined()?;
    for frame in frames {
        let host_defined = frame.script_or_module_host_defined()?;
        if std::ptr::eq(host_defined, bootstrap) {
            continue;
        }
        let location = frame.position();
        if !matches!(location.path, boa_engine::vm::SourcePath::Path(_)) {
            return None;
        }
        let position = location.position?;
        let metadata = host_defined
            .get::<boa_engine::error::ScriptErrorMetadata>()?
            .get::<script_errors::ClassicScriptMetadata>()?;
        let source_file = metadata.csp_requested_url.clone()?;
        return Some(CspSourceLocation {
            source_file,
            line_number: position.line_number(),
            column_number: position.column_number(),
        });
    }
    None
}

/// A module check retains the initial request URL separately from its target.
#[derive(Debug)]
pub(super) struct ModuleCspViolation {
    pub(super) document_id: usize,
    pub(super) reference: String,
    pub(super) redirects: usize,
    pub(super) report_reference: String,
}

/// Removes URL credentials and fragments; non-HTTP URLs disclose their scheme.
fn strip_url_for_report(input: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(input) else {
        return input.to_owned();
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return parsed.scheme().to_owned();
    }
    parsed.set_fragment(None);
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.to_string()
}

/// Converts the runtime's inline markers and relative request references into
/// the standard resource serialization, before the asynchronous task is queued.
fn blocked_url_for_report(reference: &str, base_url: &str) -> String {
    if csp::is_inline_reference(reference) {
        return "inline".to_owned();
    }
    let absolute = url::Url::parse(reference)
        .or_else(|_| url::Url::parse(base_url).and_then(|base| base.join(reference)));
    strip_url_for_report(absolute.as_ref().map_or(reference, |url| url.as_str()))
}

impl JsRuntime {
    /// Resolves the event target at task delivery, after possible detach or
    /// adoption, then uses the same queued payload for endpoint delivery.
    pub(super) fn dispatch_csp_violation_task(
        &mut self,
        document_id: usize,
        element_id: Option<usize>,
        payload: &str,
    ) -> JsResult<()> {
        let (target_id, worker_global) = {
            let state = self.host_state.borrow();
            let target = element_id
                .and_then(|id| state.get_node(id))
                .filter(|node| {
                    document_root_for_node(node)
                        .is_some_and(|document| document.identity() == document_id)
                })
                .map_or(document_id, |node| node.identity());
            (
                target,
                state.worker_id.is_some() || state.shared_worker.is_global(),
            )
        };
        let result = self.eval_in_document_realm(
            document_id,
            &format!("__omoikane_dispatch_csp_violation({target_id}, {payload}, {worker_global})"),
        );
        self.deliver_csp_report(document_id, payload);
        result.map(|_| ())
    }

    /// Sends legacy CSP reports after the violation's event has been dispatched.
    /// Delivery is best-effort and bypasses page connect-src checks to avoid
    /// recursively reporting the report request itself.
    pub(super) fn deliver_csp_report(&mut self, document_id: usize, payload: &str) {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(payload) else {
            return;
        };
        let Some(policy) = event["originalPolicy"].as_str() else {
            return;
        };
        if let Some(group) = report_to_group(policy) {
            self.deliver_modern_csp_report(document_id, &group, &event);
            return;
        }
        let endpoints =
            legacy_report_endpoints(policy, event["documentURI"].as_str().unwrap_or(""));
        let mut body = serde_json::json!({"csp-report": {
            "document-uri": event["documentURI"],
            "referrer": event["referrer"].as_str().unwrap_or(""),
            "blocked-uri": event["blockedURI"],
            "effective-directive": event["effectiveDirective"],
            "violated-directive": event["violatedDirective"],
            "original-policy": policy,
            "disposition": event["disposition"],
            "status-code": event["statusCode"].as_u64().unwrap_or(0),
            "script-sample": event["sample"].as_str().unwrap_or(""),
        }});
        if let Some(report) = body["csp-report"].as_object_mut()
            && let Some(source_file) = event["sourceFile"].as_str()
        {
            report.insert("source-file".into(), source_file.into());
            report.insert("line-number".into(), event["lineNumber"].clone());
            report.insert("column-number".into(), event["columnNumber"].clone());
        }
        let body = body.to_string().into_bytes();
        for endpoint in endpoints {
            let Ok(url) = endpoint.parse::<crate::http::Url>() else {
                continue;
            };
            let mut state = self.host_state.borrow_mut();
            let base = state.base_url_for_document(document_id);
            let origin = script_errors::document_fetch_origin(&state, document_id);
            let credentials = origin == CorsOrigin::from_url(&url);
            let mut request = HttpRequest::new(Method::Post, url.clone());
            request.set_header("Content-Type", "application/csp-report");
            request.set_body(body.clone());
            if let Some(base) = base.as_ref() {
                request.set_cookie_context(base.clone(), false);
            }
            request.set_header("Origin", origin.serialize());
            if requires_public_fetch(&url, base.as_ref()) {
                request.require_public_ip();
            }
            let _ = state.http_client.send_once_with_timeout(
                request,
                credentials,
                Some(Duration::from_secs(2)),
            );
        }
    }

    fn deliver_modern_csp_report(
        &mut self,
        document_id: usize,
        group: &str,
        event: &serde_json::Value,
    ) {
        let body = {
            let state = self.host_state.borrow();
            serde_json::json!({
                "type": "csp-violation", "url": event["reportURL"],
                "age": state.event_loop.now_ms().saturating_sub(event["generatedAt"].as_u64().unwrap_or(0)),
                "user_agent": event["userAgent"],
                "body": {
                    "documentURL": event["documentURI"], "referrer": event["referrer"],
                    "blockedURL": event["blockedURI"], "effectiveDirective": event["effectiveDirective"],
                    "originalPolicy": event["originalPolicy"], "sourceFile": event["sourceFile"],
                    "sample": event["sample"], "disposition": event["disposition"],
                    "statusCode": event["statusCode"].as_u64().unwrap_or(0),
                    "lineNumber": event["lineNumber"], "columnNumber": event["columnNumber"],
                }
            })
        };
        self.deliver_modern_report(document_id, group, body);
    }

    fn deliver_modern_report(
        &mut self,
        document_id: usize,
        group: &str,
        report: serde_json::Value,
    ) {
        let mut state = self.host_state.borrow_mut();
        let Some(endpoint) = state
            .document_csp
            .get(&document_id)
            .and_then(|policy| policy.reporting_endpoint(group))
        else {
            return;
        };
        let Ok(url) = endpoint.parse::<crate::http::Url>() else {
            return;
        };
        let base = state.base_url_for_document(document_id);
        // Reporting groups delivery by the origin of each report's creation
        // URL. Unlike legacy CSP delivery, this is not the global's origin.
        let origin = report["url"]
            .as_str()
            .and_then(|url| url.parse::<crate::http::Url>().ok())
            .as_ref()
            .map(CorsOrigin::from_url)
            .unwrap_or_else(CorsOrigin::opaque);
        let body = serde_json::json!([report]).to_string().into_bytes();
        let mut request = HttpRequest::new(Method::Post, url.clone());
        request.set_header("Content-Type", "application/reports+json");
        request.set_body(body);
        if let Some(base) = base.as_ref() {
            request.set_cookie_context(base.clone(), false);
        }
        if requires_public_fetch(&url, base.as_ref()) {
            request.require_public_ip();
        }
        let HostState {
            http_client,
            cors_preflight_cache,
            ..
        } = &mut *state;
        let result = crate::http::cors::fetch_with_timeout_restricting_redirects(
            http_client,
            request,
            &origin,
            RequestMode::Cors,
            CredentialsMode::SameOrigin,
            RedirectMode::Follow,
            cors_preflight_cache,
            Some(Duration::from_secs(2)),
        );
        if result.is_ok_and(|fetched| fetched.response.status_code() == 410) {
            if let Some(policy) = state.document_csp.get_mut(&document_id) {
                policy.remove_reporting_endpoint(group);
            }
        }
    }
    /// Installs CSP response policies before the first document script runs.
    pub(crate) fn install_response_security_headers(&mut self, headers: &[(String, String)]) {
        let document = self.document();
        let mut state = self.host_state.borrow_mut();
        let policy =
            CspPolicy::from_response_headers_and_document(headers, &document, &state.location_href);
        state.document_csp.insert(document.identity(), policy);
    }

    /// Sets the actual document response status before its first script runs.
    pub(crate) fn set_document_response_status(&mut self, status: u16) {
        let mut state = self.host_state.borrow_mut();
        let document_id = state.document.identity();
        if let Some(metadata) = state.document_reporting.get_mut(&document_id) {
            metadata.response_status = status;
        }
    }

    /// Installs the worker response policy before any author code executes.
    /// Local data workers inherit CSP rules, while endpoint lists remain local
    /// to the new global and HTTP workers use their own response policies.
    pub(super) fn install_worker_security_metadata(
        &mut self,
        loaded: &worker_loading::LoadedWorkerScript,
        creator_policy: &CspPolicy,
    ) {
        self.set_document_response_status(loaded.response_status);
        if loaded
            .effective_url
            .get(..5)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
        {
            let mut state = self.host_state.borrow_mut();
            let document_id = state.document.identity();
            state
                .document_csp
                .insert(document_id, creator_policy.inherited_for_new_global());
        } else {
            self.install_response_security_headers(&loaded.security_headers);
        }
    }

    /// Generates a Reporting API `test` report for automation in this document.
    ///
    /// This uses the same buffer and asynchronous observer notification path
    /// as platform reports. The message is data, never executable source.
    pub fn generate_test_report(&mut self, message: &str) -> JsResult<()> {
        self.generate_test_report_for_group(message, "default")
    }

    /// Generates a Reporting API test report for the named endpoint group.
    ///
    /// Observer notification is queued in the document realm. Delivery is
    /// best-effort, using the same document endpoints and CORS rules as CSP.
    pub fn generate_test_report_for_group(&mut self, message: &str, group: &str) -> JsResult<()> {
        let document_id = self.document().identity();
        let report_url = self
            .host_state
            .borrow()
            .report_url_for_document(document_id);
        let serialized_message = serde_json::to_string(message).map_err(|error| {
            JsNativeError::error().with_message(format!("report message serialization: {error}"))
        })?;
        self.eval(&format!(
            "__omoikane_generate_test_report({serialized_message}, {report_url:?})"
        ))?;
        let report = {
            let state = self.host_state.borrow();
            serde_json::json!({
                "type": "test", "url": report_url, "age": 0,
                "user_agent": state.navigator_user_agent,
                "body": {"body_message": message}
            })
        };
        self.deliver_modern_report(document_id, group, report);
        Ok(())
    }
}

fn report_to_group(policy: &str) -> Option<String> {
    policy.split(';').find_map(|directive| {
        let mut tokens = directive.split_ascii_whitespace();
        tokens
            .next()?
            .eq_ignore_ascii_case("report-to")
            .then(|| tokens.next().unwrap_or("").to_owned())
    })
}

/// Resolves first-occurrence report-uri tokens, unless report-to overrides it.
fn legacy_report_endpoints(policy: &str, document_url: &str) -> Vec<String> {
    let mut directives = std::collections::BTreeMap::new();
    for directive in policy.split(';') {
        let mut words = directive.split_ascii_whitespace();
        if let Some(name) = words.next() {
            directives
                .entry(name.to_ascii_lowercase())
                .or_insert_with(|| words.map(str::to_owned).collect::<Vec<_>>());
        }
    }
    if directives.contains_key("report-to") {
        return Vec::new();
    }
    let base = url::Url::parse(document_url).ok();
    directives
        .get("report-uri")
        .into_iter()
        .flatten()
        .filter_map(|value| {
            url::Url::parse(value)
                .ok()
                .or_else(|| base.as_ref().and_then(|base| base.join(value).ok()))
        })
        .filter(|value| matches!(value.scheme(), "http" | "https"))
        .map(|value| value.to_string())
        .collect()
}

impl HttpModuleLoader {
    pub(super) fn observe_module_policy(
        &self,
        document_id: usize,
        policy: &CspPolicy,
        reference: &str,
        redirects: usize,
        report_reference: &str,
    ) {
        let policies = policy.report_only_policy_texts_after_redirects(
            ResourceType::Script,
            reference,
            redirects,
        );
        if let Some(owner) = self.owner.upgrade() {
            owner.borrow_mut().queue_policy_violation_events(
                document_id,
                ResourceType::Script,
                report_reference,
                policies,
                "report",
            );
        }
    }

    pub(super) fn module_csp_allows_response(
        &self,
        document_id: usize,
        policy: &CspPolicy,
        url: &crate::http::Url,
        redirects: usize,
        report_reference: &str,
    ) -> bool {
        self.observe_module_policy(
            document_id,
            policy,
            &url.to_string(),
            redirects,
            report_reference,
        );
        policy.allows_url_after_redirects(ResourceType::Script, url, redirects)
    }
}

impl HostState {
    pub(super) fn register_document_reporting(
        &mut self,
        document_id: usize,
        url: &str,
        response_status: u16,
    ) {
        self.document_reporting.insert(
            document_id,
            DocumentReportingMetadata {
                creation_url: url.to_owned(),
                response_status,
            },
        );
    }

    fn report_url_for_document(&self, document_id: usize) -> String {
        let url = self
            .document_reporting
            .get(&document_id)
            .map(|metadata| metadata.creation_url.as_str())
            .unwrap_or("");
        strip_url_for_report(url)
    }

    /// Converts loader checks into document notifications after the loader has
    /// released its own mutable state. Cached enforcement remains independent
    /// of whether this source generation has already produced notifications.
    pub(super) fn observe_stylesheet_policy_checks(
        &mut self,
        document: &NodeHandle,
        node: &NodeHandle,
        checks: Vec<stylesheet::StylePolicyCheck>,
    ) {
        let policy = self.csp_policy_for_document(document);
        for check in checks {
            let monitored = policy.report_only_policy_texts_after_redirects(
                ResourceType::Style,
                &check.reference,
                check.redirects,
            );
            self.queue_policy_violation_events_for_node_with_sample(
                node,
                ResourceType::Style,
                &check.report_reference,
                monitored,
                "report",
                &check.sample,
            );
            if check.blocked {
                let enforced = policy.blocking_policy_texts_after_redirects(
                    ResourceType::Style,
                    &check.reference,
                    check.redirects,
                );
                self.queue_policy_violation_events_for_node_with_sample(
                    node,
                    ResourceType::Style,
                    &check.report_reference,
                    enforced,
                    "enforce",
                    &check.sample,
                );
                let legacy_reference = if check.reference == "inline" {
                    format!("style-element:{}", node.identity())
                } else {
                    check.report_reference
                };
                self.record_csp_violation_history(document, ResourceType::Style, legacy_reference);
            }
        }
    }

    /// Checks one request/response stage. Matching uses the current URL, while
    /// notifications retain the initial URL to avoid exposing redirect targets
    /// (CSP §2.4.2). Only enforced policies can reject the resource.
    pub(super) fn csp_allows_response_for_document(
        &mut self,
        document: &NodeHandle,
        resource_type: ResourceType,
        reference: &str,
        redirects: usize,
        report_reference: &str,
    ) -> bool {
        self.csp_allows_response_for_document_with_source(
            document,
            resource_type,
            reference,
            redirects,
            report_reference,
            None,
        )
    }

    pub(super) fn csp_allows_response_for_document_with_source(
        &mut self,
        document: &NodeHandle,
        resource_type: ResourceType,
        reference: &str,
        redirects: usize,
        report_reference: &str,
        source: Option<&CspSourceLocation>,
    ) -> bool {
        let policy = self.csp_policy_for_document(document);
        let monitored =
            policy.report_only_policy_texts_after_redirects(resource_type, reference, redirects);
        self.queue_policy_violation_events_with_source(
            document.identity(),
            resource_type,
            report_reference,
            monitored,
            "report",
            source,
        );
        if policy.allows_reference_after_redirects(resource_type, reference, redirects) {
            return true;
        }
        let enforced =
            policy.blocking_policy_texts_after_redirects(resource_type, reference, redirects);
        self.queue_policy_violation_events_with_source(
            document.identity(),
            resource_type,
            report_reference,
            enforced,
            "enforce",
            source,
        );
        self.record_csp_violation_history(document, resource_type, report_reference.to_owned());
        false
    }

    pub(super) fn csp_allows_response_for_node(
        &mut self,
        node: &NodeHandle,
        resource_type: ResourceType,
        reference: &str,
        redirects: usize,
        report_reference: &str,
    ) -> bool {
        let Some(document) = document_root_for_node(node) else {
            return true;
        };
        let policy = self.csp_policy_for_document(&document);
        let monitored =
            policy.report_only_policy_texts_after_redirects(resource_type, reference, redirects);
        self.queue_policy_violation_events_for_node(
            node,
            resource_type,
            report_reference,
            monitored,
            "report",
        );
        if policy.allows_reference_after_redirects(resource_type, reference, redirects) {
            return true;
        }
        let enforced =
            policy.blocking_policy_texts_after_redirects(resource_type, reference, redirects);
        self.queue_policy_violation_events_for_node(
            node,
            resource_type,
            report_reference,
            enforced,
            "enforce",
        );
        self.record_csp_violation_history(&document, resource_type, report_reference.to_owned());
        false
    }

    pub(super) fn csp_allows_script_for_node(
        &mut self,
        node: &NodeHandle,
        reference: &str,
    ) -> bool {
        let policy = self.csp_policy_for_node(node);
        self.queue_policy_violation_events_for_node(
            node,
            ResourceType::Script,
            reference,
            policy.report_only_policy_texts(ResourceType::Script, reference),
            "report",
        );
        if reference == "inline" {
            policy.allows_inline(ResourceType::Script)
        } else {
            policy.allows_reference(ResourceType::Script, reference)
        }
    }

    /// Checks an already captured inline source, including both monitoring and
    /// enforcement notifications. Written scripts can retain their source even
    /// when their element's text or owning tree changes before execution. The
    /// preparation document owns the policy and notifications; the element ID
    /// is only a candidate event target, rechecked when the event is delivered.
    pub(super) fn check_inline_script_source(
        &mut self,
        document: &NodeHandle,
        node: &NodeHandle,
        source: &str,
    ) -> bool {
        let policy = self.csp_policy_for_document(document);
        let sample = csp::inline_report_sample(source);
        let target = (document.identity(), Some(node.identity()));
        self.enqueue_policy_violation_events(
            target,
            ResourceType::Script,
            "inline",
            policy.report_only_policy_texts(ResourceType::Script, "inline"),
            "report",
            &sample,
        );
        if policy.allows_inline(ResourceType::Script) {
            return true;
        }
        self.enqueue_policy_violation_events(
            target,
            ResourceType::Script,
            "inline",
            policy.blocking_policy_texts_after_redirects(ResourceType::Script, "inline", 0),
            "enforce",
            &sample,
        );
        self.record_csp_violation_history(document, ResourceType::Script, "inline".into());
        false
    }

    /// Observes monitoring policies and then evaluates enforced connect-src.
    pub(super) fn csp_allows_connect(
        &mut self,
        document: &NodeHandle,
        reference: &str,
        source: Option<&CspSourceLocation>,
    ) -> bool {
        self.queue_report_only_violation_events_with_source(
            document.identity(),
            ResourceType::Connect,
            reference,
            source,
        );
        self.csp_policy_for_document(document)
            .allows_reference(ResourceType::Connect, reference)
    }

    fn queue_report_only_violation_events_with_source(
        &mut self,
        document_id: usize,
        resource_type: ResourceType,
        blocked_uri: &str,
        source: Option<&CspSourceLocation>,
    ) {
        let Some(policy) = self.document_csp.get(&document_id) else {
            return;
        };
        let policies = policy.report_only_policy_texts(resource_type, blocked_uri);
        self.queue_policy_violation_events_with_source(
            document_id,
            resource_type,
            blocked_uri,
            policies,
            "report",
            source,
        );
    }

    pub(super) fn record_csp_violation_for_request(
        &mut self,
        document: &NodeHandle,
        resource_type: ResourceType,
        reference: &str,
        redirects: usize,
        report_reference: &str,
    ) {
        self.record_csp_violation_for_request_with_source(
            document,
            resource_type,
            reference,
            redirects,
            report_reference,
            None,
        );
    }

    pub(super) fn record_csp_violation_for_request_with_source(
        &mut self,
        document: &NodeHandle,
        resource_type: ResourceType,
        reference: &str,
        redirects: usize,
        report_reference: &str,
        source: Option<&CspSourceLocation>,
    ) {
        let document_id = document.identity();
        let Some(policy) = self.document_csp.get(&document_id) else {
            return;
        };
        let policies =
            policy.blocking_policy_texts_after_redirects(resource_type, reference, redirects);
        self.queue_policy_violation_events_with_source(
            document_id,
            resource_type,
            report_reference,
            policies,
            "enforce",
            source,
        );
        self.record_csp_violation_history(document, resource_type, report_reference.to_owned());
    }

    fn queue_policy_violation_events(
        &mut self,
        document_id: usize,
        resource_type: ResourceType,
        blocked_uri: &str,
        policies: Vec<csp::PolicyViolation>,
        disposition: &str,
    ) {
        self.queue_policy_violation_events_with_source(
            document_id,
            resource_type,
            blocked_uri,
            policies,
            disposition,
            None,
        );
    }

    fn queue_policy_violation_events_with_source(
        &mut self,
        document_id: usize,
        resource_type: ResourceType,
        blocked_uri: &str,
        policies: Vec<csp::PolicyViolation>,
        disposition: &str,
        source: Option<&CspSourceLocation>,
    ) {
        self.enqueue_policy_violation_events_with_source(
            (document_id, None),
            resource_type,
            blocked_uri,
            policies,
            disposition,
            "",
            source,
        );
    }

    pub(super) fn queue_policy_violation_events_for_node(
        &mut self,
        node: &NodeHandle,
        resource_type: ResourceType,
        blocked_uri: &str,
        policies: Vec<csp::PolicyViolation>,
        disposition: &str,
    ) {
        // Capture the checked element's current source now; networking tasks
        // must never reread DOM text when they eventually dispatch the report.
        let inline_element = match node.tag_name().as_deref() {
            Some("script") => node.get_attribute("src").is_none(),
            Some("style") => true,
            _ => false,
        };
        let sample = if inline_element
            && csp::is_inline_reference(blocked_uri)
            && policies.iter().any(|policy| policy.report_sample)
        {
            csp::inline_report_sample(&collect_text_content(node))
        } else {
            String::new()
        };
        self.queue_policy_violation_events_for_node_with_sample(
            node,
            resource_type,
            blocked_uri,
            policies,
            disposition,
            &sample,
        );
    }

    fn queue_policy_violation_events_for_node_with_sample(
        &mut self,
        node: &NodeHandle,
        resource_type: ResourceType,
        blocked_uri: &str,
        policies: Vec<csp::PolicyViolation>,
        disposition: &str,
        sample: &str,
    ) {
        let Some(document) = document_root_for_node(node) else {
            return;
        };
        // CSP request violations keep element=null. Only the inline check
        // assigns an element, even when an external request came from a node.
        let element_id = (csp::is_inline_reference(blocked_uri)
            && node.node_type() == NodeType::Element)
            .then(|| node.identity());
        self.enqueue_policy_violation_events(
            (document.identity(), element_id),
            resource_type,
            blocked_uri,
            policies,
            disposition,
            sample,
        );
    }

    fn enqueue_policy_violation_events(
        &mut self,
        target: (usize, Option<usize>),
        resource_type: ResourceType,
        blocked_uri: &str,
        policies: Vec<csp::PolicyViolation>,
        disposition: &str,
        sample: &str,
    ) {
        self.enqueue_policy_violation_events_with_source(
            target,
            resource_type,
            blocked_uri,
            policies,
            disposition,
            sample,
            None,
        );
    }

    fn enqueue_policy_violation_events_with_source(
        &mut self,
        target: (usize, Option<usize>),
        resource_type: ResourceType,
        blocked_uri: &str,
        policies: Vec<csp::PolicyViolation>,
        disposition: &str,
        sample: &str,
        source: Option<&CspSourceLocation>,
    ) {
        let (document_id, element_id) = target;
        let response_status = self
            .document_reporting
            .get(&document_id)
            .map_or(0, |metadata| metadata.response_status);
        let report_url = self.report_url_for_document(document_id);
        let document_url = self
            .document_urls
            .get(&document_id)
            .cloned()
            .unwrap_or_default();
        let request_base = self
            .base_url_for_document(document_id)
            .map_or_else(|| document_url.clone(), |url| url.to_string());
        let directive = match resource_type {
            ResourceType::Script => "script-src-elem",
            ResourceType::Style if blocked_uri.starts_with("style-attribute:") => "style-src-attr",
            ResourceType::Style => "style-src-elem",
            _ => resource_type.directive(),
        };
        let source_file = source.map(|source| strip_url_for_report(&source.source_file));
        let line_number = source.map(|source| source.line_number);
        let column_number = source.map(|source| source.column_number);
        for policy in policies {
            // CSP 5.5 creates a Reporting API report only for report-to;
            // violation events and legacy delivery remain independent.
            let report_to_present = report_to_group(&policy.serialized).is_some();
            let payload = serde_json::json!({
                "documentURI": strip_url_for_report(&document_url),
                "reportURL": report_url,
                "reportToPresent": report_to_present,
                "blockedURI": blocked_url_for_report(blocked_uri, &request_base),
                "effectiveDirective": directive,
                "violatedDirective": directive,
                "originalPolicy": policy.serialized,
                "disposition": disposition,
                "referrer": "",
                "sample": if policy.report_sample { sample } else { "" },
                "sourceFile": source_file.as_deref(),
                "lineNumber": line_number,
                "columnNumber": column_number,
                "statusCode": response_status,
                "bubbles": true,
                "composed": true,
                "generatedAt": self.event_loop.now_ms(),
                "userAgent": self.navigator_user_agent,
            });
            self.event_loop.enqueue_security_policy_violation(
                document_id,
                element_id,
                payload.to_string(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_worker_inherits_csp_rules_but_not_the_creators_reporting_endpoints() {
        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[
            (
                "Content-Security-Policy".into(),
                "connect-src 'none'; report-to default".into(),
            ),
            (
                "Reporting-Endpoints".into(),
                "default=\"/collector\"".into(),
            ),
        ]);
        let script = r#"
            let target = false, rejected = false;
            addEventListener('securitypolicyviolation', event => {
                target = event.target === self && event.currentTarget === self && event.isTrusted;
            });
            new ReportingObserver(batch => postMessage({
                target, rejected,
                reports: batch.map(report => [report.url, report.body.blockedURL, report.body.statusCode])
            })).observe();
            fetch('https://blocked.test/resource').catch(error => rejected = error instanceof TypeError);
        "#;
        let worker_url = format!(
            "data:text/javascript,{}",
            script
                .bytes()
                .map(|byte| format!("%{byte:02X}"))
                .collect::<String>()
        );
        let worker_url = serde_json::to_string(&worker_url).unwrap();
        runtime
            .eval(&format!(
                r#"
            globalThis.received=[]; globalThis.parentReports=[]; globalThis.parentEvents=0;
            new ReportingObserver(batch => parentReports.push(...batch)).observe();
            document.onsecuritypolicyviolation=()=>parentEvents++;
            globalThis.worker=new Worker({worker_url});
            worker.onmessage=event=>received.push(event.data);
        "#
            ))
            .unwrap();
        runtime.run_until_idle().unwrap();
        let observed = runtime
            .eval("JSON.stringify({received,parentReports,parentEvents})")
            .unwrap();
        let observed: serde_json::Value =
            serde_json::from_str(&observed.as_string().unwrap().to_std_string_escaped()).unwrap();
        assert_eq!(
            observed,
            serde_json::json!({
                "received": [{"target": true, "rejected": true,
                    "reports": [["data", "https://blocked.test/resource", 200]]}],
                "parentReports": [], "parentEvents": 0,
            })
        );
        let state = runtime.host_state.borrow();
        assert_eq!(state.workers.len(), 1);
        let worker = state.workers.values().next().unwrap().borrow();
        let worker_state = worker.runtime.host_state.borrow();
        assert_eq!(
            worker_state.document_csp[&worker_state.document.identity()]
                .reporting_endpoint("default"),
            None
        );
    }

    #[test]
    fn sandboxed_document_legacy_reports_use_opaque_origin_without_cookies() {
        type Sent = (Option<String>, Option<String>, serde_json::Value);
        struct Transport(Arc<Mutex<Vec<Sent>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                let response = if request.method() == Method::Get {
                    let body = "<body><script>fetch('/blocked').catch(()=>{})</script>";
                    format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: text/html\r\nContent-Security-Policy: connect-src 'none'; report-uri /collector\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                } else {
                    assert_eq!(request.method(), Method::Post);
                    assert_eq!(request.url().to_string(), "https://example.test/collector");
                    self.0.lock().unwrap().push((
                        request.header("Origin").map(str::to_owned),
                        request.header("Cookie").map(str::to_owned),
                        serde_json::from_slice(request.body().unwrap()).unwrap(),
                    ));
                    "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".into()
                };
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }
        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        let url = "https://example.test/collector"
            .parse::<crate::http::Url>()
            .unwrap();
        {
            let state = runtime.host_state.borrow();
            let mut jar = state.cookie_store.lock().unwrap();
            jar.add_from_document("session=secret; Secure; SameSite=None; Path=/", &url, &url);
            assert_eq!(jar.cookie_header(&url).as_deref(), Some("session=secret"));
        }
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.eval("const frame=document.createElement('iframe'); frame.setAttribute('sandbox','allow-scripts'); frame.src='/frame'; document.body.appendChild(frame);").unwrap();
        runtime.run_until_idle().unwrap();
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0.as_deref(), Some("null"));
        assert_eq!(sent[0].1, None);
        assert_eq!(sent[0].2["csp-report"]["status-code"], 404);
    }

    #[test]
    fn inherited_csp_does_not_copy_reporting_endpoints_into_new_globals() {
        struct Transport(Arc<Mutex<usize>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                assert_eq!(request.url().to_string(), "https://example.test/collector");
                *self.0.lock().unwrap() += 1;
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        for (create, destroy) in [
            (
                "globalThis.frame=document.createElement('iframe'); document.body.appendChild(frame); globalThis.child=frame.contentWindow",
                "frame.remove()",
            ),
            (
                "globalThis.frame=document.createElement('iframe'); frame.srcdoc='<body>'; document.body.appendChild(frame); globalThis.child=frame.contentWindow",
                "frame.remove()",
            ),
            (
                "globalThis.child=window.open('', '_blank')",
                "child.close()",
            ),
            (
                "globalThis.child=window.open('about:blank', '_blank')",
                "child.close()",
            ),
        ] {
            let document = crate::html::TreeBuilder::parse("<body>").document();
            let mut runtime =
                JsRuntime::with_document_and_url(document, "https://example.test/document")
                    .unwrap();
            runtime.install_response_security_headers(&[
                (
                    "Content-Security-Policy".into(),
                    "script-src 'none'; report-to default".into(),
                ),
                (
                    "Reporting-Endpoints".into(),
                    "default=\"/collector\"".into(),
                ),
            ]);
            let sent = Arc::new(Mutex::new(0));
            runtime
                .host_state
                .borrow_mut()
                .http_client
                .set_transport(Some(Arc::new(Transport(sent.clone()))));
            runtime.eval(create).unwrap();
            runtime.run_until_idle().unwrap();
            runtime
                .eval(
                    r#"
                globalThis.reports=[];
                new child.ReportingObserver(batch=>reports.push(...batch)).observe();
                const script=child.document.createElement('script');
                script.src='/blocked.js'; child.document.body.appendChild(script);
            "#,
                )
                .unwrap();
            runtime.run_until_idle().unwrap();
            let observed = runtime.eval("JSON.stringify(reports)").unwrap();
            assert!(runtime.eval("reports.length===1 && reports[0].url==='about' && reports[0].body.statusCode===200").unwrap().as_boolean().unwrap(), "{create}: {observed:?}");
            assert_eq!(*sent.lock().unwrap(), 0, "{create}");
            runtime.eval(destroy).unwrap();
            runtime.run_until_idle().unwrap();
            assert_eq!(
                runtime.host_state.borrow().document_reporting.len(),
                1,
                "{create}"
            );
            runtime.eval("const own=document.createElement('script'); own.src='/parent.js'; document.body.appendChild(own)").unwrap();
            runtime.run_until_idle().unwrap();
            assert_eq!(*sent.lock().unwrap(), 1, "{create}");
        }
    }

    #[test]
    fn reports_keep_creation_url_while_csp_body_tracks_current_document() {
        struct Transport(Arc<Mutex<Vec<serde_json::Value>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0
                    .lock()
                    .unwrap()
                    .push(serde_json::from_slice(request.body().unwrap()).unwrap());
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/original?x=1#secret")
                .unwrap();
        runtime.install_response_security_headers(&[
            (
                "Content-Security-Policy".into(),
                "connect-src 'none'; report-to default".into(),
            ),
            (
                "Reporting-Endpoints".into(),
                "default=\"/collector\"".into(),
            ),
        ]);
        runtime.set_document_response_status(404);
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.eval(r#"
            globalThis.reports = []; globalThis.events = [];
            new ReportingObserver(batch => reports.push(...batch)).observe();
            document.onsecuritypolicyviolation = event => events.push([event.documentURI, event.statusCode]);
            history.pushState(null, '', '/changed#visible');
            fetch('/blocked').catch(() => {});
        "#).unwrap();
        runtime.run_jobs().unwrap();
        runtime.run_timers(10, 1, 30);
        runtime
            .generate_test_report("test after pushState")
            .unwrap();
        runtime.run_timers(10, 1, 30);
        let observed = runtime.eval("JSON.stringify({events, reports})").unwrap();
        let observed: serde_json::Value =
            serde_json::from_str(&observed.as_string().unwrap().to_std_string_escaped()).unwrap();
        assert_eq!(
            observed["events"],
            serde_json::json!([["https://example.test/changed", 404]])
        );
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(observed["reports"].as_array().unwrap().len(), 2);
        for (index, report) in sent.iter().enumerate() {
            assert_eq!(report[0]["url"], "https://example.test/original?x=1");
            assert_eq!(observed["reports"][index]["url"], report[0]["url"]);
        }
        assert_eq!(
            sent[0][0]["body"]["documentURL"],
            "https://example.test/changed"
        );
        assert_eq!(sent[0][0]["body"]["statusCode"], 404);
        assert_eq!(observed["reports"][0]["body"], sent[0][0]["body"]);
        assert_eq!(sent[1][0]["type"], "test");
    }

    #[test]
    fn modern_report_follows_redirects_and_removes_gone_endpoint() {
        type Sent = (Method, String, Vec<u8>);
        struct Transport(Arc<Mutex<Vec<Sent>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0.lock().unwrap().push((
                    request.method(),
                    request.url().to_string(),
                    request.body().unwrap_or_default().to_vec(),
                ));
                let response = if request.url().to_string().ends_with("/redirect") {
                    "HTTP/1.1 307 Temporary Redirect\r\nLocation: /destination\r\nContent-Length: 0\r\n\r\n"
                } else {
                    "HTTP/1.1 410 Gone\r\nContent-Length: 0\r\n\r\n"
                };
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }
        for modern in [true, false] {
            let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
            let mut runtime =
                JsRuntime::with_document_and_url(document, "https://example.test/page").unwrap();
            runtime.install_response_security_headers(&[
                (
                    "Content-Security-Policy".into(),
                    if modern {
                        "connect-src 'none'; report-to selected"
                    } else {
                        "connect-src 'none'; report-uri /redirect"
                    }
                    .into(),
                ),
                (
                    "Reporting-Endpoints".into(),
                    "selected=\"/redirect\"".into(),
                ),
            ]);
            let sent = Arc::new(Mutex::new(Vec::new()));
            runtime
                .host_state
                .borrow_mut()
                .http_client
                .set_transport(Some(Arc::new(Transport(sent.clone()))));
            for _ in 0..2 {
                runtime.eval("fetch('/blocked').catch(() => {});").unwrap();
                runtime.run_jobs().unwrap();
                runtime.run_timers(10, 1, 30);
            }
            let sent = sent.lock().unwrap();
            assert_eq!(sent.len(), 2, "modern={modern}: {sent:?}");
            assert_eq!(sent[0].0, Method::Post);
            assert_eq!(sent[1].0, Method::Post);
            assert_eq!(sent[0].1, "https://example.test/redirect");
            assert_eq!(
                sent[1].1,
                if modern {
                    "https://example.test/destination"
                } else {
                    "https://example.test/redirect"
                }
            );
            assert_eq!(sent[0].2, sent[1].2);
        }
    }

    #[test]
    fn blocked_relative_script_reports_the_absolute_initial_request() {
        let document = crate::html::TreeBuilder::parse(
            "<html><body><script src='../blocked.js#fragment'></script></body></html>",
        )
        .document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/path/document")
                .unwrap();
        runtime.install_csp_policy(&["script-src 'none'; report-to observations".into()]);
        runtime.eval("globalThis.reports=[]; globalThis.targets=[]; document.addEventListener('securitypolicyviolation', e=>targets.push(e.target === document)); new ReportingObserver(batch=>reports.push(...batch)).observe();").unwrap();
        assert!(runtime.execute_document_scripts(None).is_empty());
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("reports.length === 1 && reports[0].body.blockedURL === 'https://example.test/blocked.js' && targets.length === 1 && targets[0]").unwrap().as_boolean().unwrap());
    }

    #[test]
    fn child_script_reports_resolve_relative_urls_against_the_inherited_base() {
        for srcdoc in [false, true] {
            let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
            let mut runtime =
                JsRuntime::with_document_and_url(document, "https://example.test/path/document")
                    .unwrap();
            runtime.install_csp_policy(&["script-src 'none'; report-to observations".into()]);
            runtime.eval(&format!(
                "globalThis.frame=document.createElement('iframe'); if ({srcdoc}) frame.srcdoc='<html><body></body></html>'; document.body.appendChild(frame);"
            )).unwrap();
            runtime.run_until_idle().unwrap();
            runtime.eval(r#"
                globalThis.received=[]; globalThis.childReports=[];
                const child=frame.contentDocument;
                child.addEventListener('securitypolicyviolation', e=>received.push([e.blockedURI,e.target===child]));
                new frame.contentWindow.ReportingObserver(batch=>childReports.push(...batch)).observe();
                const script=child.createElement('script');
                script.src='blocked.js#fragment'; child.body.appendChild(script);
            "#).unwrap();
            runtime.run_until_idle().unwrap();
            let observed = runtime.eval("JSON.stringify([received,childReports.map(r=>r.body.blockedURL),document.cspViolations.length])").unwrap();
            assert_eq!(
                observed.as_string().unwrap().to_std_string_escaped(),
                r#"[[["https://example.test/path/blocked.js",true]],["https://example.test/path/blocked.js"],0]"#,
                "srcdoc={srcdoc}"
            );
        }
    }

    #[test]
    fn report_only_styles_apply_and_notify_without_repeating_on_cascade_rebuild() {
        let document = crate::html::TreeBuilder::parse(r#"<html><head>
            <style id="sheet">@import url("data:text/css,%23a%7Bcolor%3Argb(1%2C2%2C3)%7D"); #b{width:23px}</style>
            <link id="linked" rel="stylesheet" href="data:text/css,%23b%7Bcolor%3Argb(4%2C5%2C6)%7D">
            </head><body><div id="a"></div><div id="b"></div></body></html>"#).document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[(
            "Content-Security-Policy-Report-Only".into(),
            "style-src 'none'; report-to observations".into(),
        )]);
        runtime.eval(r#"
            globalThis.reports=[]; globalThis.targets=[];
            new ReportingObserver(batch => reports.push(...batch)).observe();
            document.addEventListener('securitypolicyviolation', e => targets.push(e.target === document ? 'document' : e.target.id));
            globalThis.colors=[getComputedStyle(document.getElementById('a')).color, getComputedStyle(document.getElementById('b')).color];
        "#).unwrap();
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("colors.join('|') === 'rgb(1, 2, 3)|rgb(4, 5, 6)' && reports.length === 5 && reports.every(r => r.body.disposition === 'report' && r.body.effectiveDirective === 'style-src-elem') && targets.join(',') === 'sheet,document,document,document,document' && document.cspViolations.length === 0").unwrap().as_boolean().unwrap());
        // Changing the viewport rebuilds the cascade, not the stylesheet load.
        runtime.set_viewport(420.0, 320.0);
        runtime
            .eval("getComputedStyle(document.getElementById('a')).color")
            .unwrap();
        runtime.run_timers(10, 1, 30);
        assert_eq!(
            runtime.eval("reports.length").unwrap().as_number(),
            Some(5.0)
        );
    }

    #[test]
    fn csp_events_target_connected_elements_and_fall_back_after_tree_changes() {
        for (after, expected) in [
            ("", serde_json::json!(["element", "element"])),
            ("source.remove()", serde_json::json!(["document"])),
            (
                "document.implementation.createHTMLDocument('').body.appendChild(source)",
                serde_json::json!(["document"]),
            ),
            (
                "host.attachShadow({mode:'closed'}).appendChild(source)",
                serde_json::json!(["element", "host"]),
            ),
        ] {
            let document = crate::html::TreeBuilder::parse(
                "<html><body><div id='host'></div><script id='source'>globalThis.scriptRan=true</script></body></html>",
            ).document();
            let mut runtime =
                JsRuntime::with_document_and_url(document, "https://example.test/document")
                    .unwrap();
            runtime.install_csp_policy(&["script-src 'none'".into()]);
            runtime.eval(r#"
                globalThis.scriptRan = false;
                globalThis.seen = [];
                globalThis.source = document.getElementById('source');
                globalThis.host = document.getElementById('host');
                source.addEventListener('securitypolicyviolation', e => {
                    if (e.target === source && e.isTrusted && e.bubbles && e.composed) seen.push('element');
                });
                document.addEventListener('securitypolicyviolation', e => {
                    seen.push(e.target === source ? 'element' : e.target === host ? 'host' : e.target === document ? 'document' : 'wrong');
                });
            "#).unwrap();
            assert!(runtime.execute_document_scripts(None).is_empty());
            runtime.eval(after).unwrap();
            runtime.run_timers(10, 1, 30);
            assert_eq!(runtime.eval("scriptRan").unwrap().as_boolean(), Some(false));
            let actual = runtime.eval("JSON.stringify(seen)").unwrap();
            let actual: serde_json::Value =
                serde_json::from_str(&actual.as_string().unwrap().to_std_string_escaped()).unwrap();
            assert_eq!(actual, expected, "after={after}");
        }
    }

    #[test]
    fn redirect_reports_use_initial_url_for_events_observers_and_delivery() {
        struct Transport(Arc<Mutex<Vec<serde_json::Value>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                let response = match request.url().to_string().as_str() {
                    "https://example.test/redirect" => {
                        "HTTP/1.1 302 Found\r\nLocation: https://other.test/private-target\r\nContent-Length: 0\r\n\r\n"
                    }
                    "https://other.test/private-target" => {
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 2\r\n\r\nok"
                    }
                    "https://example.test/report" => {
                        self.0
                            .lock()
                            .unwrap()
                            .push(serde_json::from_slice(request.body().unwrap()).unwrap());
                        "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"
                    }
                    other => panic!("unexpected request: {other}"),
                };
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }
        for (header, disposition, modern) in [
            ("Content-Security-Policy", "enforce", false),
            ("Content-Security-Policy", "enforce", true),
            ("Content-Security-Policy-Report-Only", "report", false),
            ("Content-Security-Policy-Report-Only", "report", true),
        ] {
            let document = crate::html::TreeBuilder::parse("<body>").document();
            let mut runtime =
                JsRuntime::with_document_and_url(document, "https://example.test/document")
                    .unwrap();
            runtime.install_response_security_headers(&[
                (
                    header.into(),
                    if modern {
                        "connect-src 'self'; report-to redirect"
                    } else {
                        "connect-src 'self'; report-uri /report"
                    }
                    .into(),
                ),
                ("Reporting-Endpoints".into(), "redirect=\"/report\"".into()),
            ]);
            let delivered = Arc::new(Mutex::new(Vec::new()));
            runtime
                .host_state
                .borrow_mut()
                .http_client
                .set_transport(Some(Arc::new(Transport(delivered.clone()))));
            runtime.eval("globalThis.result='pending'; globalThis.events=[]; globalThis.reports=[]; document.addEventListener('securitypolicyviolation', e => events.push(e.blockedURI)); new ReportingObserver(batch => reports.push(...batch)).observe(); fetch('/redirect').then(() => result='loaded', error => result=error instanceof TypeError ? 'blocked' : 'wrong-error');").unwrap();
            runtime.run_jobs().unwrap();
            runtime.run_timers(10, 1, 30);
            let observed = runtime.eval("JSON.stringify([result,events,reports.map(r=>[r.body.blockedURL,r.body.disposition]),document.cspViolations.map(v=>v.blockedURI)])").unwrap();
            let observed: serde_json::Value =
                serde_json::from_str(&observed.as_string().unwrap().to_std_string_escaped())
                    .unwrap();
            let initial = "https://example.test/redirect";
            assert_eq!(
                observed[0],
                if disposition == "enforce" {
                    "blocked"
                } else {
                    "loaded"
                }
            );
            assert_eq!(observed[1], serde_json::json!([initial]));
            assert_eq!(
                observed[2],
                if modern {
                    serde_json::json!([[initial, disposition]])
                } else {
                    serde_json::json!([])
                }
            );
            assert_eq!(
                observed[3],
                if disposition == "enforce" {
                    serde_json::json!([initial])
                } else {
                    serde_json::json!([])
                }
            );
            let delivered = delivered.lock().unwrap();
            assert_eq!(delivered.len(), 1);
            let blocked = if modern {
                &delivered[0][0]["body"]["blockedURL"]
            } else {
                &delivered[0]["csp-report"]["blocked-uri"]
            };
            assert_eq!(blocked, initial);
            assert!(!delivered[0].to_string().contains("private-target"));
        }
    }

    #[test]
    fn report_only_redirects_check_host_but_ignore_redirected_path() {
        struct Transport;
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                let url = request.url().to_string();
                let response = match url.as_str() {
                    "https://example.test/allowed/local" => {
                        "HTTP/1.1 302 Found\r\nLocation: /changed\r\nContent-Length: 0\r\n\r\n"
                    }
                    "https://example.test/allowed/foreign" => {
                        "HTTP/1.1 302 Found\r\nLocation: https://blocked.test/final\r\nContent-Length: 0\r\n\r\n"
                    }
                    "https://example.test/changed" | "https://blocked.test/final" => {
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 2\r\n\r\nok"
                    }
                    _ => panic!("unexpected request: {url}"),
                };
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }
        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[(
            "Content-Security-Policy-Report-Only".into(),
            "connect-src https://example.test/allowed/; report-to observations".into(),
        )]);
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport)));
        runtime.eval("globalThis.reports=[]; globalThis.completed=0; new ReportingObserver(batch => reports.push(...batch)).observe(); for (const path of ['local','foreign']) fetch('/allowed/'+path).then(r => r.text()).then(body => { if (body === 'ok') completed++; });").unwrap();
        runtime.run_jobs().unwrap();
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("completed === 2 && reports.length === 1 && reports[0].body.blockedURL === 'https://example.test/allowed/foreign' && reports[0].body.disposition === 'report' && document.cspViolations.length === 0").unwrap().as_boolean().unwrap());
    }

    #[test]
    fn report_only_script_policy_reports_without_blocking_inline_execution() {
        let document = crate::html::TreeBuilder::parse(
            "<html><body><script>globalThis.scriptRan = true</script></body></html>",
        )
        .document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[(
            "Content-Security-Policy-Report-Only".into(),
            "script-src 'none'; report-to observations".into(),
        )]);
        runtime.eval("globalThis.scriptRan = false; globalThis.reports = []; new ReportingObserver(batch => reports.push(...batch), {types:['csp-violation']}).observe();").unwrap();
        assert!(runtime.execute_document_scripts(None).is_empty());
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("scriptRan && reports.length === 1 && reports[0].body.disposition === 'report' && reports[0].body.effectiveDirective === 'script-src-elem' && document.cspViolations.length === 0").unwrap().as_boolean().unwrap());
    }

    #[test]
    fn report_only_policy_observes_document_write_script() {
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[(
            "Content-Security-Policy-Report-Only".into(),
            "script-src 'none'; report-to observations".into(),
        )]);
        runtime.eval("globalThis.writtenRan = false; globalThis.reports = []; new ReportingObserver(batch => reports.push(...batch), {types:['csp-violation']}).observe(); document.write('<script>globalThis.writtenRan = true<\\/script>');").unwrap();
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("writtenRan && reports.length === 1 && reports[0].body.disposition === 'report' && document.cspViolations.length === 0").unwrap().as_boolean().unwrap());
    }

    #[test]
    fn test_reports_reach_default_and_named_endpoints_and_notify_observers() {
        struct Transport(Arc<Mutex<Vec<(String, Vec<u8>)>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                assert_eq!(request.method(), Method::Post);
                assert_eq!(request.header("Origin"), Some("https://example.test"));
                assert_eq!(
                    request.header("Content-Type"),
                    Some("application/reports+json")
                );
                self.0.lock().unwrap().push((
                    request.url().to_string(),
                    request.body().unwrap_or_default().to_vec(),
                ));
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document#private")
                .unwrap();
        runtime.install_response_security_headers(&[(
            "Reporting-Endpoints".into(),
            "default=\"/default\", custom=\"/custom\"".into(),
        )]);
        // A resource base override must not change the report's origin or URL.
        runtime.set_base_url("https://unrelated.test/resources/".parse().unwrap());
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.eval("globalThis.received = []; new ReportingObserver(batch => received.push(...batch), {types:['test']}).observe();").unwrap();
        runtime.generate_test_report("alpha").unwrap();
        runtime
            .generate_test_report_for_group("beta", "custom")
            .unwrap();
        runtime
            .generate_test_report_for_group("missing", "unconfigured")
            .unwrap();
        runtime.run_timers(10, 1, 30);
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        for (index, (endpoint, body)) in sent.iter().enumerate() {
            assert_eq!(
                endpoint,
                if index == 0 {
                    "https://example.test/default"
                } else {
                    "https://example.test/custom"
                }
            );
            let reports: serde_json::Value = serde_json::from_slice(body).unwrap();
            assert_eq!(reports.as_array().unwrap().len(), 1);
            assert_eq!(reports[0]["type"], "test");
            assert_eq!(reports[0]["url"], "https://example.test/document");
            assert_eq!(
                reports[0]["body"]["body_message"],
                if index == 0 { "alpha" } else { "beta" }
            );
        }
        assert!(runtime.eval("received.length === 3 && received.map(r => r.body.message).join(',') === 'alpha,beta,missing' && received.every(r => r.url === 'https://example.test/document')").unwrap().as_boolean().unwrap());
    }

    #[test]
    fn gone_endpoint_stops_delivery_but_keeps_events_and_observer_reports() {
        struct GoneTransport(Arc<Mutex<Vec<String>>>);
        impl crate::http::HttpTransport for GoneTransport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0.lock().unwrap().push(request.url().to_string());
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 410 Gone\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[
            (
                "Content-Security-Policy".into(),
                "connect-src 'none'; report-to selected; report-uri /legacy".into(),
            ),
            ("Reporting-Endpoints".into(), "selected=\"/reports\"".into()),
        ]);
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(GoneTransport(sent.clone()))));
        runtime.eval("globalThis.events = []; globalThis.reports = []; document.onsecuritypolicyviolation = e => events.push(e.blockedURI); new ReportingObserver(batch => reports.push(...batch)).observe();").unwrap();
        for path in ["/first", "/second"] {
            runtime
                .eval(&format!("fetch('{path}').catch(() => {{}});"))
                .unwrap();
            runtime.run_jobs().unwrap();
            runtime.run_timers(10, 1, 30);
        }
        assert_eq!(*sent.lock().unwrap(), vec!["https://example.test/reports"]);
        let observed = runtime.eval("JSON.stringify({events, reports})").unwrap();
        assert!(runtime.eval("events.length === 2 && reports.length === 2 && events[0] === 'https://example.test/first' && events[1] === 'https://example.test/second' && reports[1].body.blockedURL === events[1]").unwrap().as_boolean().unwrap(), "{observed:?}");
    }

    #[test]
    fn named_report_to_endpoint_receives_modern_json_and_overrides_report_uri() {
        struct Transport(Arc<Mutex<Vec<(String, String, Vec<u8>)>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0.lock().unwrap().push((
                    request.url().to_string(),
                    request.header("Content-Type").unwrap_or("").to_owned(),
                    request.body().unwrap_or_default().to_vec(),
                ));
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.set_user_agent("reporting-test-agent");
        runtime.install_response_security_headers(&[
            (
                "Content-Security-Policy".into(),
                "connect-src 'none'; report-uri /legacy; report-to selected".into(),
            ),
            (
                "Reporting-Endpoints".into(),
                "ignored=\"/wrong\", selected=\"/modern\";enabled".into(),
            ),
        ]);
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.eval("fetch('/blocked').catch(() => {});").unwrap();
        runtime.run_jobs().unwrap();
        assert!(sent.lock().unwrap().is_empty());
        runtime.run_timers(10, 1, 30);
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "https://example.test/modern");
        assert_eq!(sent[0].1, "application/reports+json");
        let body: serde_json::Value = serde_json::from_slice(&sent[0].2).unwrap();
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert_eq!(body[0]["type"], "csp-violation");
        assert_eq!(body[0]["url"], "https://example.test/document");
        assert_eq!(body[0]["user_agent"], "reporting-test-agent");
        assert!(body[0]["age"].as_u64().unwrap() >= 1);
        assert_eq!(
            body[0]["body"]["blockedURL"],
            "https://example.test/blocked"
        );
        assert_eq!(body[0]["body"]["disposition"], "enforce");
    }

    #[test]
    fn cross_origin_endpoint_uses_cors_preflight_without_sending_cookies() {
        type Sent = (
            Method,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        struct Transport(Arc<Mutex<Vec<Sent>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0.lock().unwrap().push((
                    request.method(),
                    request.url().to_string(),
                    request.header("Origin").map(str::to_owned),
                    request
                        .header("Access-Control-Request-Headers")
                        .map(str::to_owned),
                    request.header("Cookie").map(str::to_owned),
                ));
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: https://example.test\r\nAccess-Control-Allow-Methods: POST\r\nAccess-Control-Allow-Headers: content-type\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[
            (
                "Content-Security-Policy".into(),
                "connect-src 'none'; report-to foreign".into(),
            ),
            (
                "Reporting-Endpoints".into(),
                "foreign=\"https://other.test/reports\"".into(),
            ),
        ]);
        let other_url = "https://other.test/reports"
            .parse::<crate::http::Url>()
            .unwrap();
        runtime
            .host_state
            .borrow()
            .cookie_store
            .lock()
            .unwrap()
            .add_from_document(
                "foreign=secret; Secure; SameSite=None; Path=/",
                &other_url,
                &other_url,
            );
        assert!(
            runtime
                .host_state
                .borrow()
                .cookie_store
                .lock()
                .unwrap()
                .cookie_header(&other_url)
                .is_some()
        );
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.eval("fetch('/blocked').catch(() => {});").unwrap();
        runtime.run_jobs().unwrap();
        runtime.run_timers(10, 1, 30);
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0, Method::Options);
        assert_eq!(sent[1].0, Method::Post);
        for request in sent.iter() {
            assert_eq!(request.1, "https://other.test/reports");
            assert_eq!(request.2.as_deref(), Some("https://example.test"));
            assert_eq!(request.4, None);
        }
        assert_eq!(sent[0].3.as_deref(), Some("content-type"));
        assert_eq!(sent[1].3, None);
    }

    #[test]
    fn report_uri_posts_real_violation_and_omits_cross_origin_cookies() {
        type Sent = (Method, String, String, Option<String>, Vec<u8>);
        struct Transport(Arc<Mutex<Vec<Sent>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0.lock().unwrap().push((
                    request.method(),
                    request.url().to_string(),
                    request.header("Content-Type").unwrap_or("").to_owned(),
                    request.header("Cookie").map(str::to_owned),
                    request.body().unwrap_or_default().to_vec(),
                ));
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_csp_policy(&[
            "connect-src 'none'; report-uri /reports https://other.test/reports".into(),
        ]);
        let sent = Arc::new(Mutex::new(Vec::new()));
        let other_url = "https://other.test/reports"
            .parse::<crate::http::Url>()
            .unwrap();
        runtime
            .host_state
            .borrow()
            .cookie_store
            .lock()
            .unwrap()
            .add_from_document(
                "foreign=secret; Secure; SameSite=None; Path=/",
                &other_url,
                &other_url,
            );
        assert!(
            runtime
                .host_state
                .borrow()
                .cookie_store
                .lock()
                .unwrap()
                .cookie_header(&other_url)
                .is_some()
        );
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.eval("document.cookie = 'secret=value; Secure; Path=/'; fetch('https://example.test/blocked').catch(() => {});").unwrap();
        runtime.run_jobs().unwrap();
        assert!(sent.lock().unwrap().is_empty());
        runtime.run_timers(10, 1, 30);
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0, Method::Post);
        assert_eq!(sent[0].1, "https://example.test/reports");
        assert_eq!(sent[0].2, "application/csp-report");
        assert_eq!(sent[0].3.as_deref(), Some("secret=value"));
        assert_eq!(sent[1].1, "https://other.test/reports");
        assert_eq!(sent[1].3, None);
        let body: serde_json::Value = serde_json::from_slice(&sent[0].4).unwrap();
        assert_eq!(
            body["csp-report"]["blocked-uri"],
            "https://example.test/blocked"
        );
        assert_eq!(body["csp-report"]["disposition"], "enforce");
        assert_eq!(body["csp-report"]["effective-directive"], "connect-src");
    }

    #[test]
    fn legacy_reports_include_complete_source_fields_only_when_captured() {
        struct Transport(Arc<Mutex<Vec<serde_json::Value>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0
                    .lock()
                    .unwrap()
                    .push(serde_json::from_slice(request.body().unwrap()).unwrap());
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_csp_policy(&["connect-src 'none'; report-uri /reports".into()]);
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(Arc::clone(&sent)))));
        let policy = "connect-src 'none'; report-uri /reports";
        let mut event = serde_json::json!({
            "documentURI": "https://example.test/document",
            "referrer": "",
            "blockedURI": "https://blocked.test/resource",
            "effectiveDirective": "connect-src",
            "violatedDirective": "connect-src",
            "originalPolicy": policy,
            "disposition": "enforce",
            "statusCode": 200,
            "sample": "",
            "sourceFile": "https://example.test/requested.js",
            "lineNumber": 41,
            "columnNumber": 17,
        });
        let document_id = runtime.document().identity();
        runtime.deliver_csp_report(document_id, &event.to_string());
        event["sourceFile"] = serde_json::Value::Null;
        runtime.deliver_csp_report(document_id, &event.to_string());

        let sent = sent.lock().unwrap();
        let with_source = &sent[0]["csp-report"];
        assert_eq!(
            with_source["source-file"],
            "https://example.test/requested.js"
        );
        assert_eq!(with_source["line-number"], 41);
        assert_eq!(with_source["column-number"], 17);
        let without_source = sent[1]["csp-report"].as_object().unwrap();
        assert!(!without_source.contains_key("source-file"));
        assert!(!without_source.contains_key("line-number"));
        assert!(!without_source.contains_key("column-number"));
    }

    #[test]
    fn report_to_suppresses_legacy_endpoints_and_duplicate_directives_use_first() {
        assert!(
            legacy_report_endpoints(
                "report-uri /old; report-to missing",
                "https://example.test/document"
            )
            .is_empty()
        );
        assert_eq!(
            legacy_report_endpoints(
                "report-uri /first; report-uri /ignored",
                "https://example.test/document"
            ),
            ["https://example.test/first"]
        );
    }

    #[test]
    fn report_only_xhr_and_event_source_keep_transport_and_deliver_reports() {
        struct Transport;
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                _: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                crate::http::HttpResponse::parse(&mut &b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 13\r\n\r\ndata: hello\n\n"[..])
            }
        }
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[(
            "Content-Security-Policy-Report-Only".into(),
            "connect-src 'none'; report-to observations".into(),
        )]);
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport)));
        runtime.eval("globalThis.roReports = []; new ReportingObserver(reports => roReports.push(...reports)).observe(); globalThis.xhrDone = false; let xhr = new XMLHttpRequest(); xhr.open('GET', '/xhr'); xhr.onload = () => { xhrDone = xhr.status === 200 && xhr.responseText === 'data: hello\\n\\n'; }; xhr.send(); globalThis.sseMessage = ''; let source = new EventSource('/sse'); source.onmessage = event => { sseMessage = event.data; source.close(); };").unwrap();
        runtime.run_jobs().unwrap();
        runtime.run_timers(50, 1, 100);
        // CSP checks both the outgoing requests and the delivered responses.
        let passed = runtime.eval("xhrDone && sseMessage === 'hello' && roReports.length === 4 && roReports.every(r => r.body.disposition === 'report' && r.body.effectiveDirective === 'connect-src')").unwrap().as_boolean().unwrap();
        let observed = runtime
            .eval("JSON.stringify([xhrDone, sseMessage, roReports])")
            .unwrap();
        assert!(
            passed,
            "{}",
            observed.as_string().unwrap().to_std_string_escaped()
        );
        assert!(runtime.eval("delete globalThis.document; let workerSource = new EventSource('/worker-stream'); workerSource.close(); workerSource.url === 'https://example.test/worker-stream'").unwrap().as_boolean().unwrap());
    }

    #[test]
    fn report_only_fetch_reaches_transport_and_notifies_without_enforcing() {
        struct Transport(Arc<Mutex<usize>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                _: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                *self.0.lock().unwrap() += 1;
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[(
            "Content-Security-Policy-Report-Only".into(),
            "connect-src 'none'; report-to observations".into(),
        )]);
        let requests = Arc::new(Mutex::new(0));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(requests.clone()))));
        runtime.eval("globalThis.roEvents = []; globalThis.roReports = []; document.onsecuritypolicyviolation = e => roEvents.push(e.disposition); new ReportingObserver(reports => roReports.push(...reports)).observe(); globalThis.fetchResult = ''; fetch('https://example.test/resource').then(r => r.text()).then(text => fetchResult = text);").unwrap();
        runtime.run_jobs().unwrap();
        runtime.run_timers(10, 1, 30);
        assert_eq!(*requests.lock().unwrap(), 1);
        // Both the pre-request and post-request checks violate this policy.
        assert!(runtime.eval("fetchResult === 'ok' && roEvents.join(',') === 'report,report' && roReports.length === 2 && roReports.every(r => r.body.disposition === 'report') && document.cspViolations.length === 0").unwrap().as_boolean().unwrap());
    }

    #[test]
    fn reporting_observers_receive_real_csp_reports_and_buffered_copies() {
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_csp_policy(&["connect-src 'none'; report-to observations".into()]);
        runtime.eval(r#"
            globalThis.deliveries = [];
            globalThis.observer = new ReportingObserver(function(reports, owner) {
                deliveries.push([this === observer, owner === observer, reports]);
            }, {types: ['csp-violation']});
            observer.observe();
            globalThis.filtered = new ReportingObserver(() => { throw new Error('wrong type'); }, {types: ['deprecation']});
            filtered.observe();
            fetch('https://blocked.test/resource').catch(() => {});
            fetch('https://blocked.test/resource').catch(() => {});
        "#).unwrap();
        runtime.run_jobs().unwrap();
        assert_eq!(
            runtime.eval("deliveries.length").unwrap().as_number(),
            Some(0.0)
        );
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("deliveries.length === 1 && deliveries[0][0] && deliveries[0][1] && deliveries[0][2].length === 2 && deliveries[0][2].every(r => r.type === 'csp-violation' && r.url === 'https://example.test/document' && r.body.blockedURL === 'https://blocked.test/resource' && r.body.originalPolicy === \"connect-src 'none'; report-to observations\") && filtered.takeRecords().length === 0").unwrap().as_boolean().unwrap());
        runtime.eval("observer.disconnect(); deliveries[0][2][0].body.blockedURL = 'tampered'; globalThis.replayed = []; globalThis.buffered = new ReportingObserver(reports => replayed.push(...reports), {buffered: true}); buffered.observe();").unwrap();
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("replayed.length === 2 && replayed[0].body.blockedURL === 'https://blocked.test/resource' && replayed[0] !== deliveries[0][2][0]").unwrap().as_boolean().unwrap());
        runtime
            .eval("buffered.disconnect(); buffered.observe();")
            .unwrap();
        runtime.run_timers(10, 1, 30);
        assert_eq!(
            runtime.eval("replayed.length").unwrap().as_number(),
            Some(2.0)
        );
    }

    #[test]
    fn connect_api_reports_keep_author_call_sites_and_requested_script_url() {
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_csp_policy(&["connect-src 'none'; report-to observations".into()]);
        runtime
            .eval(
                "globalThis.sourceEvents=[];globalThis.sourceReports=[];\
                 document.addEventListener('securitypolicyviolation',event=>sourceEvents.push({\
                   sourceFile:event.sourceFile,lineNumber:event.lineNumber,columnNumber:event.columnNumber\
                 }));\
                 new ReportingObserver(batch=>sourceReports.push(...batch),{types:['csp-violation']}).observe();",
            )
            .unwrap();
        let (evaluated, _, _, _) = runtime.eval_safe_timed_source(
            concat!(
                "fetch('https://blocked.test/fetch').catch(()=>{});\n",
                "const xhr = new XMLHttpRequest();\n",
                "xhr.open('GET','https://blocked.test/xhr');\n",
                "  xhr.send();\n",
                "    new WebSocket('wss://blocked.test/socket');\n",
                "      new EventSource('https://blocked.test/events').close();",
            ),
            Some("https://private.test/final.js?secret"),
            Some("https://user:password@example.test/requested.js?visible#hidden"),
            (41, 9),
            false,
        );
        evaluated.unwrap();
        runtime.run_until_idle().unwrap();
        let observed = runtime
            .eval(
                "JSON.stringify({\
                   events:sourceEvents.slice().sort((a,b)=>a.lineNumber-b.lineNumber),\
                   reports:sourceReports.map(report=>({\
                     sourceFile:report.body.sourceFile,lineNumber:report.body.lineNumber,\
                     columnNumber:report.body.columnNumber\
                   })).sort((a,b)=>a.lineNumber-b.lineNumber)\
                 })",
            )
            .unwrap();
        let observed: serde_json::Value =
            serde_json::from_str(&observed.as_string().unwrap().to_std_string_escaped()).unwrap();
        let events = observed["events"].as_array().unwrap();
        let reports = observed["reports"].as_array().unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(reports.len(), 4);
        for (index, (line, column)) in [(41, 9), (44, 7), (45, 5), (46, 7)].into_iter().enumerate()
        {
            let event = &events[index];
            let report = &reports[index];
            assert_eq!(
                event["sourceFile"],
                "https://example.test/requested.js?visible"
            );
            assert_eq!(report["sourceFile"], event["sourceFile"]);
            assert_eq!(report["lineNumber"], event["lineNumber"]);
            assert_eq!(report["columnNumber"], event["columnNumber"]);
            assert_eq!(event["lineNumber"], line);
            assert_eq!(event["columnNumber"], column);
        }
        assert!(!observed.to_string().contains("private.test"));
        assert!(!observed.to_string().contains("password"));
        assert!(!observed.to_string().contains("hidden"));
    }

    #[test]
    fn unmarked_author_frames_stop_source_attribution() {
        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_csp_policy(&["connect-src 'none'; report-to observations".into()]);
        runtime
            .eval(
                "globalThis.unmarkedSourceEvent=null;globalThis.unmarkedSourceReport=null;\
                 globalThis.unmarkedFetch=()=>fetch('https://blocked.test/unmarked');\
                 document.addEventListener('securitypolicyviolation',event=>unmarkedSourceEvent=event);\
                 new ReportingObserver(batch=>unmarkedSourceReport=batch[0],{types:['csp-violation']}).observe();",
            )
            .unwrap();
        let (evaluated, _, _, _) = runtime.eval_safe_timed_source(
            "unmarkedFetch().catch(()=>{});",
            Some("https://example.test/outer-effective.js"),
            Some("https://example.test/outer-requested.js"),
            (12, 3),
            false,
        );
        evaluated.unwrap();
        runtime.run_until_idle().unwrap();
        assert!(
            runtime
                .eval(
                    "unmarkedSourceEvent.sourceFile==='' && \
                     unmarkedSourceEvent.lineNumber===0 && unmarkedSourceEvent.columnNumber===0 && \
                     unmarkedSourceReport.body.sourceFile===null && \
                     unmarkedSourceReport.body.lineNumber===null && \
                     unmarkedSourceReport.body.columnNumber===null",
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn generated_code_frames_stop_source_attribution() {
        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_csp_policy(&["connect-src 'none'; report-to observations".into()]);
        runtime
            .eval(
                "globalThis.generatedSourceReports=[];\
                 new ReportingObserver(batch=>generatedSourceReports.push(...batch),{\
                   types:['csp-violation']\
                 }).observe();",
            )
            .unwrap();
        let (evaluated, _, _, _) = runtime.eval_safe_timed_source(
            "eval(\"fetch('https://blocked.test/eval').catch(()=>{})\");\
             new Function(\"fetch('https://blocked.test/function').catch(()=>{})\")();",
            Some("https://example.test/outer-effective.js"),
            Some("https://example.test/outer-requested.js"),
            (17, 7),
            false,
        );
        evaluated.unwrap();
        runtime.run_until_idle().unwrap();
        assert!(
            runtime
                .eval(
                    "generatedSourceReports.length===2 && \
                     generatedSourceReports.every(report=>\
                       report.body.sourceFile===null && report.body.lineNumber===null && \
                       report.body.columnNumber===null)",
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn event_source_keeps_captured_source_out_of_page_properties() {
        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_csp_policy(&["connect-src 'none'; report-to observations".into()]);
        runtime
            .eval(
                "globalThis.eventSourceReport=null;\
                 globalThis.cspHooksHidden=\
                   !('__omoikane_capture_csp_source' in globalThis) && \
                   !('__omoikane_event_source_fetch' in globalThis);\
                 globalThis.cspHookCalls=0;\
                 globalThis.__omoikane_capture_csp_source=()=>{cspHookCalls++;return JSON.stringify({\
                   source_file:'https://forged.test/global.js',line_number:3,column_number:4\
                 })};\
                 globalThis.__omoikane_event_source_fetch=()=>{cspHookCalls++;return ''};\
                 Object.defineProperty(EventSource.prototype,'__cspSource',{\
                   configurable:true,get(){return JSON.stringify({\
                     source_file:'https://forged.test/source.js',line_number:1,column_number:2\
                   })},set(_){}\
                 });\
                 new ReportingObserver(batch=>eventSourceReport=batch[0],{\
                   types:['csp-violation']\
                 }).observe();",
            )
            .unwrap();
        let (evaluated, _, _, _) = runtime.eval_safe_timed_source(
            "const source=new EventSource('https://blocked.test/events');source.close();",
            Some("https://example.test/effective.js"),
            Some("https://example.test/requested.js"),
            (23, 5),
            false,
        );
        evaluated.unwrap();
        runtime.run_until_idle().unwrap();
        assert!(
            runtime
                .eval(
                    "eventSourceReport.body.sourceFile==='https://example.test/requested.js' && \
                     eventSourceReport.body.lineNumber>=23 && \
                     !eventSourceReport.body.sourceFile.includes('forged.test') && \
                     cspHooksHidden && cspHookCalls===0",
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn report_urls_remove_credentials_fragments_and_non_http_payloads() {
        assert_eq!(
            strip_url_for_report("https://user:secret@example.test/path?q=1#secret"),
            "https://example.test/path?q=1"
        );
        assert_eq!(strip_url_for_report("data:text/plain,secret"), "data");
        assert_eq!(
            strip_url_for_report("blob:https://example.test/secret"),
            "blob"
        );
        assert_eq!(strip_url_for_report("wss://example.test/private"), "wss");
        assert_eq!(strip_url_for_report("inline"), "inline");
    }

    #[test]
    fn enforced_violations_queue_events_for_each_policy_and_occurrence() {
        let document = crate::html::TreeBuilder::parse(
            "<html><body><script>globalThis.blockedScriptRan = true</script></body></html>",
        )
        .document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_csp_policy(&[
            "script-src 'none'; connect-src 'none'".into(),
            "script-src 'none'; connect-src 'self'".into(),
        ]);
        runtime.eval("globalThis.events = []; document.onsecuritypolicyviolation = event => events.push([event instanceof SecurityPolicyViolationEvent, event.isTrusted, event.target === document ? 'document' : event.target === document.querySelector('script') ? 'script' : 'wrong', event.documentURI, event.effectiveDirective, event.blockedURI, event.originalPolicy]);").unwrap();
        assert!(runtime.execute_document_scripts(None).is_empty());
        assert_eq!(
            runtime.eval("events.length").unwrap().as_number(),
            Some(0.0)
        );
        assert!(
            runtime
                .eval("typeof blockedScriptRan === 'undefined'")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        runtime.eval("fetch('https://blocked.test/resource').catch(() => {}); fetch('https://blocked.test/resource').catch(() => {});").unwrap();
        runtime.run_jobs().unwrap();
        assert_eq!(
            runtime.eval("events.length").unwrap().as_number(),
            Some(0.0)
        );
        runtime.run_timers(10, 1, 20);
        assert_eq!(
            runtime.eval("events.length").unwrap().as_number(),
            Some(6.0)
        );
        assert!(runtime.eval("events.every(e => e[0] && e[1] && e[3] === 'https://example.test/document') && events.slice(0,2).every(e => e[2] === 'script') && events.slice(2).every(e => e[2] === 'document') && events[0][4] === 'script-src-elem' && events[0][5] === 'inline' && events[0][6] === \"script-src 'none'; connect-src 'none'\" && events[1][6] === \"script-src 'none'; connect-src 'self'\" && events.slice(2).every(e => e[4] === 'connect-src')").unwrap().as_boolean().unwrap());
    }
}
