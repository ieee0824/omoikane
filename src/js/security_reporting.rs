//! CSP notifications owned by their document and queued as networking tasks.
use super::*;
#[path = "security_reporting/endpoints.rs"]
mod endpoints;
pub(super) use crate::http::{
    ReferrerPolicy, navigation_referrer_with_policy, response_referrer_policy,
};
pub(super) use endpoints::parse_reporting_endpoints;
#[cfg(test)]
#[path = "security_reporting/sample_tests.rs"]
mod sample_tests;

/// Response metadata owned by one environment, independent of history changes.
#[derive(Debug)]
pub(super) struct DocumentReportingMetadata {
    pub(super) creation_url: String,
    pub(super) response_status: u16,
    /// The referrer assigned when this Document was created, not its current URL.
    pub(super) referrer: String,
    pub(super) referrer_policy: ReferrerPolicy,
    /// Changes whenever this Document receives a new reporting endpoint map.
    pub(super) endpoint_revision: u64,
}

/// Owned author-script provenance captured before an asynchronous Web API
/// wrapper loses its caller stack.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(super) struct CspSourceLocation {
    pub(super) source_file: String,
    pub(super) line_number: u32,
    pub(super) column_number: u32,
}

/// Immutable requested URL retained by one author module runnable.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub(super) struct ModuleCspMetadata {
    #[unsafe_ignore_trace]
    requested_url: String,
}

pub(super) fn module_csp_metadata(requested_url: &str) -> boa_engine::HostDefined {
    let mut metadata = boa_engine::HostDefined::default();
    metadata.insert(ModuleCspMetadata {
        requested_url: requested_url.to_owned(),
    });
    metadata
}

#[cfg(test)]
std::thread_local! {
    static CSP_SOURCE_CAPTURE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn take_csp_source_capture_calls() -> usize {
    CSP_SOURCE_CAPTURE_CALLS.with(|calls| calls.replace(0))
}

/// Finds the immediate author-script frame whose immutable metadata
/// and source position belong to that same frame. Only frames from this
/// bootstrap invocation are skipped; an unmarked author frame stops capture.
pub(super) fn capture_csp_source(context: &Context) -> Option<CspSourceLocation> {
    #[cfg(test)]
    CSP_SOURCE_CAPTURE_CALLS.with(|calls| calls.set(calls.get() + 1));

    let mut frames = context.stack_trace();
    // This private binding is invoked by the trusted bootstrap module. Skip
    // only frames belonging to that exact runnable; the first different
    // runnable is the author boundary. If it has no author provenance, stop
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
        let classic = host_defined
            .get::<boa_engine::error::ScriptErrorMetadata>()
            .and_then(|metadata| metadata.get::<script_errors::ClassicScriptMetadata>());
        let source_file = classic
            .and_then(|metadata| metadata.csp_requested_url.clone())
            .or_else(|| {
                host_defined
                    .get::<ModuleCspMetadata>()
                    .map(|metadata| metadata.requested_url.clone())
            })?;
        return Some(CspSourceLocation {
            source_file,
            line_number: position.line_number(),
            column_number: position.column_number(),
        });
    }
    None
}

/// A module check retains the policies that rejected its request. The module
/// loader cannot dispatch these events until Boa returns control to the page.
#[derive(Debug)]
pub(super) struct ModuleCspViolation {
    pub(super) document_id: usize,
    pub(super) report_reference: String,
    pub(super) worklet: Option<WorkletModuleViolation>,
    pub(super) policies: Vec<csp::PolicyViolation>,
}

#[derive(Debug)]
pub(super) struct WorkletModuleViolation {
    pub(super) owner: worklet::ModuleOwner,
    pub(super) report_only: Vec<csp::PolicyViolation>,
    pub(super) enforced: Vec<csp::PolicyViolation>,
}

#[cfg(test)]
pub(super) fn wait_for_report_delivery(runtime: &mut JsRuntime) {
    let mut state = runtime.host_state.borrow_mut();
    let completions = state
        .report_delivery
        .as_mut()
        .map(report_delivery::ReportDelivery::wait_for_pending)
        .unwrap_or_default();
    state.apply_report_completions(completions);
}

/// Removes URL credentials and fragments; non-HTTP URLs disclose their scheme.
/// Unparseable source-location sentinels, such as `inline`, are preserved, so
/// callers handling untrusted URLs must validate them before calling this.
pub(super) fn strip_url_for_report(input: &str) -> String {
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

/// Computes the default navigation referrer for policy unit tests.
#[cfg(test)]
pub(super) fn navigation_referrer(source: &str, destination: &str) -> String {
    navigation_referrer_with_policy(source, destination, ReferrerPolicy::default())
}

/// Derives the Document referrer from the final navigation request. Initial
/// `about:blank` uses the creator URL, while `srcdoc` has no navigation request.
pub(super) fn loaded_document_referrer(
    final_request_referrer: Option<&str>,
    destination: Option<&str>,
    source: &str,
    policy: ReferrerPolicy,
    initial_about_blank_creator_url: Option<&str>,
) -> String {
    if let Some(creator_url) = initial_about_blank_creator_url {
        return creator_url.to_owned();
    }
    if destination == Some("about:srcdoc") {
        return String::new();
    }
    final_request_referrer
        .map(str::to_owned)
        .unwrap_or_else(|| {
            navigation_referrer_with_policy(source, destination.unwrap_or("about:blank"), policy)
        })
}

/// Converts the runtime's inline markers and relative request references into
/// the standard resource serialization, before the asynchronous task is queued.
fn blocked_url_for_report(reference: &str, base_url: &str) -> String {
    if matches!(
        reference,
        "trusted-types-sink" | "trusted-types-policy" | "eval"
    ) {
        return reference.to_owned();
    }
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
        if !self.host_state.borrow().document_is_active(document_id) {
            return Ok(());
        }
        // Reports belong to the Document's global even if it has never run
        // author code. Falling back to the caller would notify its observers.
        self.realm_for_document(document_id)?;
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

    /// Queues legacy CSP reports after the violation's event has been
    /// dispatched. Delivery is best-effort and bypasses page connect-src
    /// checks to avoid recursively reporting the report request itself.
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
            state.set_document_cookie_request_context(document_id, &mut request);
            request.set_header("Origin", origin.serialize());
            if requires_public_fetch(&url, base.as_ref()) {
                request.require_public_ip();
            }
            let client = state.http_client.fork_for_reporting();
            state.queue_report(report_delivery::ReportRequest::Legacy {
                client,
                request,
                credentials,
            });
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

    /// Queues a Reporting API envelope for the named endpoint group.
    /// COOP, COEP, CSP, and test reports share this HTTP delivery path.
    pub(super) fn deliver_modern_report(
        &mut self,
        document_id: usize,
        group: &str,
        report: serde_json::Value,
    ) {
        self.host_state
            .borrow_mut()
            .queue_modern_report(document_id, group, report);
    }

    /// Adds a platform report to this Document's ReportingObserver queue.
    /// Its HTTP delivery can be queued separately before a navigation retires
    /// the old Document's endpoint configuration.
    pub(super) fn queue_reporting_observer_report(
        &mut self,
        document_id: usize,
        report: &serde_json::Value,
    ) -> JsResult<()> {
        let realm = self.realm_for_document(document_id)?;
        if document_id != self.document().identity() && realm.is_none() {
            return Ok(());
        }
        let dispatcher = self
            .host_state
            .borrow()
            .report_observer_dispatchers
            .get(&document_id)
            .cloned();
        let Some(dispatcher) = dispatcher else {
            return Ok(());
        };
        let old_realm = realm.map(|realm| self.context.enter_realm(realm));
        let result = self.with_active_host(|context| {
            let value = JsValue::from_json(report, context)?;
            let callable = dispatcher.as_callable().ok_or_else(|| {
                JsNativeError::typ().with_message("report observer dispatcher is not callable")
            })?;
            callable.call(&JsValue::undefined(), &[value], context)?;
            Ok(())
        });
        let result = result.and_then(|()| self.run_jobs());
        if let Some(old_realm) = old_realm {
            self.context.enter_realm(old_realm);
        }
        result
    }

    /// Installs CSP and supported Permissions-Policy response directives before
    /// the first document script runs.
    pub(crate) fn install_response_security_headers(&mut self, headers: &[(String, String)]) {
        let document = self.document();
        let mut state = self.host_state.borrow_mut();
        let policy =
            CspPolicy::from_response_headers_and_document(headers, &document, &state.location_href);
        state.advance_report_endpoint_revision(document.identity());
        state.document_csp.insert(document.identity(), policy);
        if let Some(policy) = response_referrer_policy(headers)
            && let Some(metadata) = state.document_reporting.get_mut(&document.identity())
        {
            metadata.referrer_policy = policy;
        }
        state.apply_connected_referrer_metas(&document);
        state.document_permissions.insert(
            document.identity(),
            DocumentPolicy::from_response_headers(headers),
        );
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
            state.advance_report_endpoint_revision(document_id);
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
        let args = [js_string!(message).into(), js_string!(group).into()];
        self.with_active_host(|context| {
            generate_test_report_native(&JsValue::undefined(), &args, context).map(|_| ())
        })
    }

    /// Enables a report-generation command binding for an automation driver.
    ///
    /// This is opt-in: ordinary runtimes do not expose the automation binding.
    /// Commands enqueue genuine observer notifications and endpoint delivery
    /// immediately, so page microtasks cannot overtake a queued host command.
    pub fn enable_reporting_automation(&mut self) -> JsResult<()> {
        self.with_active_host(|context| {
            context.register_global_builtin_callable(
                js_string!("__omoikane_automation_generate_test_report"),
                1,
                NativeFunction::from_copy_closure(generate_test_report_native),
            )
        })
    }
}

fn generate_test_report_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let message = string_argument(args.first(), "", context)?;
    let group = string_argument(args.get(1), "default", context)?;
    let (document_id, report_url) = with_host_state(|state| {
        let state = state.borrow();
        let document_id = state.document.identity();
        Ok((document_id, state.report_url_for_document(document_id)))
    })?;
    let dispatcher = context
        .global_object()
        .get(js_string!("__omoikane_generate_test_report"), context)?;
    let callable = dispatcher.as_callable().ok_or_else(|| {
        JsNativeError::typ().with_message("test report observer dispatcher is not callable")
    })?;
    callable.call(
        &JsValue::undefined(),
        &[
            js_string!(message.as_str()).into(),
            js_string!(report_url.as_str()).into(),
        ],
        context,
    )?;
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let report = serde_json::json!({
            "type": "test", "url": report_url, "age": 0,
            "user_agent": state.navigator_user_agent,
            "body": {"body_message": message}
        });
        state.queue_modern_report(document_id, &group, report);
        Ok(())
    })?;
    Ok(JsValue::undefined())
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
        graph: &ModuleGraphContext,
        reference: &str,
        redirects: usize,
        report_reference: &str,
        stage: ModulePolicyStage,
    ) {
        let request = csp::ScriptRequest {
            nonce: None,
            parser_inserted: false,
        };
        let policies = graph
            .csp
            .script_policy_violations(reference, redirects, request, true);
        if graph.worklet_owner.is_some() {
            if !policies.is_empty() {
                self.record_csp_violation(
                    document_id,
                    reference.to_owned(),
                    redirects,
                    report_reference,
                    graph,
                    stage,
                );
            }
            return;
        }
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
        graph: &ModuleGraphContext,
        url: &crate::http::Url,
        redirects: usize,
        report_reference: &str,
    ) -> bool {
        self.observe_module_policy(
            document_id,
            graph,
            &url.to_string(),
            redirects,
            report_reference,
            ModulePolicyStage::Response,
        );
        graph
            .csp
            .script_policy_violations(
                &url.to_string(),
                redirects,
                csp::ScriptRequest {
                    nonce: None,
                    parser_inserted: false,
                },
                false,
            )
            .is_empty()
    }
}

impl HostState {
    pub(super) fn record_worklet_module_violation(
        &mut self,
        document: &NodeHandle,
        violation: ModuleCspViolation,
    ) {
        let Some(notice) = violation.worklet else {
            return;
        };
        self.queue_policy_violation_events_with_source(
            document.identity(),
            ResourceType::Script,
            &violation.report_reference,
            notice.report_only,
            "report",
            notice.owner.source.as_ref(),
        );
        let blocked = !notice.enforced.is_empty();
        self.queue_policy_violation_events_with_source(
            document.identity(),
            ResourceType::Script,
            &violation.report_reference,
            notice.enforced,
            "enforce",
            notice.owner.source.as_ref(),
        );
        if blocked {
            self.record_csp_violation_history(
                document,
                ResourceType::Script,
                violation.report_reference,
            );
        }
    }

    /// Snapshots one report's endpoint and HTTP policy while its owner
    /// Document is still live, then queues owned data for background delivery.
    pub(super) fn queue_modern_report(
        &mut self,
        document_id: usize,
        group: &str,
        report: serde_json::Value,
    ) {
        let Some(endpoint) = self
            .document_csp
            .get(&document_id)
            .and_then(|policy| policy.reporting_endpoint(group))
        else {
            return;
        };
        let revision = self
            .document_reporting
            .get(&document_id)
            .map_or(0, |metadata| metadata.endpoint_revision);
        self.queue_modern_report_to_endpoint(document_id, revision, group, &endpoint, report);
    }

    /// Queues a report for an explicitly captured response endpoint. Redirect
    /// responses have no Document or CSP container in which to store it.
    pub(super) fn queue_modern_report_to_endpoint(
        &mut self,
        document_id: usize,
        revision: u64,
        group: &str,
        endpoint: &str,
        report: serde_json::Value,
    ) {
        self.drain_report_completions();
        let Ok(url) = endpoint.parse::<crate::http::Url>() else {
            return;
        };
        let base = self.base_url_for_document(document_id);
        // Reporting groups delivery by the report creation URL's origin.
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
        self.set_document_cookie_request_context(document_id, &mut request);
        if requires_public_fetch(&url, base.as_ref()) {
            request.require_public_ip();
        }
        let endpoint = report_delivery::EndpointKey {
            document_id,
            revision,
            group: group.to_owned(),
            url: endpoint.to_owned(),
        };
        let client = self.http_client.fork_for_reporting();
        self.queue_report(report_delivery::ReportRequest::Modern {
            client,
            request,
            origin,
            endpoint,
        });
    }

    pub(super) fn advance_report_endpoint_revision(&mut self, document_id: usize) {
        if let Some(metadata) = self.document_reporting.get_mut(&document_id) {
            metadata.endpoint_revision = metadata.endpoint_revision.wrapping_add(1);
        }
    }

    fn apply_report_completions(&mut self, completions: Vec<report_delivery::ReportCompletion>) {
        for completion in completions {
            let report_delivery::ReportCompletion::Gone(endpoint) = completion else {
                continue;
            };
            if !self
                .document_reporting
                .get(&endpoint.document_id)
                .is_some_and(|metadata| metadata.endpoint_revision == endpoint.revision)
            {
                continue;
            }
            if let Some(policy) = self.document_csp.get_mut(&endpoint.document_id)
                && policy.reporting_endpoint(&endpoint.group).as_deref()
                    == Some(endpoint.url.as_str())
            {
                policy.remove_reporting_endpoint(&endpoint.group);
            }
        }
    }

    fn drain_report_completions(&mut self) {
        let completions = self
            .report_delivery
            .as_mut()
            .map(report_delivery::ReportDelivery::drain)
            .unwrap_or_default();
        self.apply_report_completions(completions);
    }

    fn queue_report(&mut self, report: report_delivery::ReportRequest) {
        self.drain_report_completions();
        if self.report_delivery.is_none() {
            self.report_delivery = report_delivery::ReportDelivery::new().ok();
        }
        if let Some(delivery) = self.report_delivery.as_mut() {
            delivery.enqueue(report);
        }
    }

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
                referrer: String::new(),
                referrer_policy: ReferrerPolicy::default(),
                endpoint_revision: 0,
            },
        );
    }

    /// Returns the policy container value updated by headers and connected meta
    /// elements. Removing a meta element does not restore an older policy.
    pub(super) fn referrer_policy_for_document(&self, document_id: usize) -> ReferrerPolicy {
        self.document_reporting
            .get(&document_id)
            .map_or(ReferrerPolicy::default(), |metadata| {
                metadata.referrer_policy
            })
    }

    /// Applies a connected meta element at insertion or relevant attribute
    /// change time, preserving its effect after the element is removed.
    pub(super) fn apply_connected_referrer_meta(&mut self, node: &NodeHandle) {
        if !node.is_html_element()
            || !node.has_tag_name("meta")
            || !node
                .get_attribute("name")
                .is_some_and(|name| name.eq_ignore_ascii_case("referrer"))
        {
            return;
        }
        let Some(policy) = node
            .get_attribute("content")
            .and_then(|content| ReferrerPolicy::parse_meta(&content))
        else {
            return;
        };
        // A shadow tree has a Document as its shadow-including root, but is
        // not itself a document tree. Follow parent links only here.
        let mut root = node.clone();
        while let Some(parent) = root.parent_node() {
            root = parent;
        }
        if root.node_type() != NodeType::Document {
            return;
        }
        if let Some(metadata) = self.document_reporting.get_mut(&root.identity()) {
            metadata.referrer_policy = policy;
        }
    }

    /// Applies parser-created or newly inserted meta elements in insertion
    /// order, including descendants of an inserted subtree.
    pub(super) fn apply_connected_referrer_metas(&mut self, root: &NodeHandle) {
        self.apply_connected_referrer_meta(root);
        for child in root.child_nodes() {
            self.apply_connected_referrer_metas(&child);
        }
    }

    /// Returns the Document's creation URL for Reporting, without credentials
    /// or fragments. Invalid creation URLs are redacted rather than echoed.
    pub(super) fn report_url_for_document(&self, document_id: usize) -> String {
        let url = self
            .document_reporting
            .get(&document_id)
            .map(|metadata| metadata.creation_url.as_str())
            .unwrap_or("");
        if url::Url::parse(url).is_err() {
            return String::new();
        }
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
            let (enforced, monitored) = if let Some(source) = &check.inline_source {
                let nonce = node.get_attribute("nonce");
                policy.inline_policy_violations(csp::InlineType::Style, source, nonce.as_deref())
            } else {
                (
                    policy.blocking_policy_texts_after_redirects(
                        ResourceType::Style,
                        &check.reference,
                        check.redirects,
                    ),
                    policy.report_only_policy_texts_after_redirects(
                        ResourceType::Style,
                        &check.reference,
                        check.redirects,
                    ),
                )
            };
            self.queue_policy_violation_events_for_node_with_sample(
                node,
                ResourceType::Style,
                &check.report_reference,
                monitored,
                "report",
                &check.sample,
            );
            if check.blocked {
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

    /// Checks an inline style attribute after each mutation. Viewport and
    /// cascade rebuilds reuse the result without duplicate reports.
    pub(super) fn csp_allows_inline_style_attribute(
        &mut self,
        document: &NodeHandle,
        node: &NodeHandle,
        source: &str,
    ) -> bool {
        let policy = self.csp_policy_for_document(document);
        let (enforced, reported) =
            policy.inline_policy_violations(csp::InlineType::StyleAttribute, source, None);
        let allowed = enforced.is_empty();
        let unchanged = self
            .observed_style_attributes
            .get(&node.identity())
            .is_some_and(|previous| {
                previous.node.upgrade().as_ref() == Some(node)
                    && previous.document_id == document.identity()
                    && previous.source == source
                    && previous.policy.has_same_policy_rules(&policy)
            });
        if unchanged {
            return allowed;
        }
        self.observed_style_attributes.insert(
            node.identity(),
            super::InlineStyleCspSnapshot {
                node: node.downgrade(),
                document_id: document.identity(),
                source: source.to_owned(),
                policy,
            },
        );
        let reference = format!("style-attribute:{}", node.identity());
        let sample = csp::inline_report_sample(source);
        self.queue_policy_violation_events_for_node_with_sample(
            node,
            ResourceType::Style,
            &reference,
            reported,
            "report",
            &sample,
        );
        if !allowed {
            self.queue_policy_violation_events_for_node_with_sample(
                node,
                ResourceType::Style,
                &reference,
                enforced,
                "enforce",
                &sample,
            );
            self.record_csp_violation_history(document, ResourceType::Style, reference);
        }
        allowed
    }

    /// Delivers the creator Document's checks collected during worker I/O.
    /// The policy was evaluated before each request and after its response;
    /// notifications are queued only after the HTTP client borrow ends.
    pub(super) fn observe_worker_policy_checks(
        &mut self,
        document_id: usize,
        checks: Vec<worker_loading::WorkerPolicyCheck>,
        source: Option<&CspSourceLocation>,
    ) {
        let Some(document) = self.get_node(document_id) else {
            return;
        };
        for check in checks {
            self.csp_allows_response_for_document_with_source(
                &document,
                ResourceType::Worker,
                &check.reference,
                check.redirects,
                &check.report_reference,
                source,
            );
        }
    }

    /// Checks one request/response stage. Matching uses the current URL, while
    /// notifications retain the initial URL to avoid exposing redirect targets
    /// (CSP §2.4.2). Only enforced policies can reject the resource.
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

    /// Applies script request metadata for `importScripts()` in a Worker
    /// global. These requests are script-like but never parser-inserted.
    pub(super) fn csp_allows_imported_script_with_source(
        &mut self,
        document: &NodeHandle,
        reference: &str,
        redirects: usize,
        report_reference: &str,
        source: Option<&CspSourceLocation>,
    ) -> bool {
        let policy = self.csp_policy_for_document(document);
        let request = csp::ScriptRequest {
            nonce: None,
            parser_inserted: false,
        };
        let monitored = policy.script_policy_violations(reference, redirects, request, true);
        self.queue_policy_violation_events_with_source(
            document.identity(),
            ResourceType::Script,
            report_reference,
            monitored,
            "report",
            source,
        );
        let enforced = policy.script_policy_violations(reference, redirects, request, false);
        if enforced.is_empty() {
            return true;
        }
        self.queue_policy_violation_events_with_source(
            document.identity(),
            ResourceType::Script,
            report_reference,
            enforced,
            "enforce",
            source,
        );
        self.record_csp_violation_history(document, ResourceType::Script, report_reference.into());
        false
    }

    pub(super) fn csp_allows_response_for_script_node(
        &mut self,
        node: &NodeHandle,
        reference: &str,
        redirects: usize,
        report_reference: &str,
        request: csp::ScriptRequest<'_>,
    ) -> bool {
        self.csp_allows_response_for_script_node_with_source(
            node,
            reference,
            redirects,
            report_reference,
            request,
            None,
        )
    }

    pub(super) fn csp_allows_response_for_script_node_with_source(
        &mut self,
        node: &NodeHandle,
        reference: &str,
        redirects: usize,
        report_reference: &str,
        request: csp::ScriptRequest<'_>,
        source: Option<&CspSourceLocation>,
    ) -> bool {
        let Some(document) = document_root_for_node(node) else {
            return true;
        };
        let policy = self.csp_policy_for_document(&document);
        self.check_external_script_request(
            &document,
            node,
            &policy,
            reference,
            redirects,
            report_reference,
            request,
            source,
        )
    }

    /// Checks a prepared script against its owning Document even after detachment.
    pub(super) fn csp_allows_prepared_script_response(
        &mut self,
        document_id: usize,
        node: &NodeHandle,
        reference: &str,
        redirects: usize,
        report_reference: &str,
        request: csp::ScriptRequest<'_>,
        source: Option<&CspSourceLocation>,
    ) -> bool {
        let Some(document) = self.get_node(document_id) else {
            return false;
        };
        let policy = self.csp_policy_for_document(&document);
        self.check_external_script_request(
            &document,
            node,
            &policy,
            reference,
            redirects,
            report_reference,
            request,
            source,
        )
    }

    pub(super) fn csp_allows_script_for_node(
        &mut self,
        node: &NodeHandle,
        reference: &str,
        request: csp::ScriptRequest<'_>,
    ) -> bool {
        self.csp_allows_script_for_node_with_source(node, reference, request, None)
    }

    pub(super) fn csp_allows_script_for_node_with_source(
        &mut self,
        node: &NodeHandle,
        reference: &str,
        request: csp::ScriptRequest<'_>,
        source: Option<&CspSourceLocation>,
    ) -> bool {
        let Some(document) = document_root_for_node(node) else {
            return true;
        };
        let policy = self.csp_policy_for_node(node);
        let base = self.base_url_for_document(document.identity());
        let report_reference = resource_reference_timing_name(reference, base.as_ref());
        self.check_external_script_request(
            &document,
            node,
            &policy,
            reference,
            0,
            &report_reference,
            request,
            source,
        )
    }

    pub(super) fn csp_allows_written_script_request(
        &mut self,
        document: &NodeHandle,
        script: &NodeHandle,
        reference: &str,
        redirects: usize,
        report_reference: &str,
        request: csp::ScriptRequest<'_>,
    ) -> bool {
        let policy = self.csp_policy_for_document(document);
        self.check_external_script_request(
            document,
            script,
            &policy,
            reference,
            redirects,
            report_reference,
            request,
            None,
        )
    }

    fn check_external_script_request(
        &mut self,
        document: &NodeHandle,
        script: &NodeHandle,
        policy: &CspPolicy,
        reference: &str,
        redirects: usize,
        report_reference: &str,
        request: csp::ScriptRequest<'_>,
        source: Option<&CspSourceLocation>,
    ) -> bool {
        let target = (document.identity(), Some(script.identity()));
        self.enqueue_policy_violation_events_with_source(
            target,
            ResourceType::Script,
            report_reference,
            policy.script_policy_violations(reference, redirects, request, true),
            "report",
            "",
            source,
        );
        let enforced = policy.script_policy_violations(reference, redirects, request, false);
        if enforced.is_empty() {
            return true;
        }
        self.enqueue_policy_violation_events_with_source(
            target,
            ResourceType::Script,
            report_reference,
            enforced,
            "enforce",
            "",
            source,
        );
        self.record_csp_violation_history(document, ResourceType::Script, report_reference.into());
        false
    }

    /// Checks parser-owned inline script text before evaluation. The source
    /// bytes, rather than the `inline` report URL, determine hash matching.
    pub(super) fn csp_allows_inline_script_for_node(
        &mut self,
        node: &NodeHandle,
        source: &str,
    ) -> bool {
        let Some(document) = document_root_for_node(node) else {
            return false;
        };
        let policy = self.csp_policy_for_document(&document);
        let nonce = node.get_attribute("nonce");
        let (enforced, reported) =
            policy.inline_policy_violations(csp::InlineType::Script, source, nonce.as_deref());
        let sample = csp::inline_report_sample(source);
        self.queue_policy_violation_events_for_node_with_sample(
            node,
            ResourceType::Script,
            "inline",
            reported,
            "report",
            &sample,
        );
        if enforced.is_empty() {
            return true;
        }
        self.queue_policy_violation_events_for_node_with_sample(
            node,
            ResourceType::Script,
            "inline",
            enforced,
            "enforce",
            &sample,
        );
        self.record_csp_violation_history(&document, ResourceType::Script, "inline".into());
        false
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
        let nonce = node.get_attribute("nonce");
        let kind = if self.parser_inserted_scripts.contains(&node.identity()) {
            csp::InlineType::Script
        } else {
            csp::InlineType::DynamicScript
        };
        let (enforced, reported) = policy.inline_policy_violations(kind, source, nonce.as_deref());
        self.enqueue_policy_violation_events(
            target,
            ResourceType::Script,
            "inline",
            reported,
            "report",
            &sample,
        );
        if enforced.is_empty() {
            return true;
        }
        self.enqueue_policy_violation_events(
            target,
            ResourceType::Script,
            "inline",
            enforced,
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

    pub(super) fn queue_policy_violation_events(
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

    pub(super) fn queue_policy_violation_events_with_source(
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
        let directive = match resource_type {
            ResourceType::Script => "script-src-elem",
            ResourceType::Style if blocked_uri.starts_with("style-attribute:") => "style-src-attr",
            ResourceType::Style => "style-src-elem",
            _ => resource_type.directive(),
        };
        self.enqueue_policy_violation_events_with_directive(
            target,
            directive,
            blocked_uri,
            policies,
            disposition,
            sample,
            source,
        );
    }

    /// Queues Trusted Types violations through the ordinary CSP event and
    /// reporting delivery path, retaining the directive's distinct URI.
    pub(super) fn queue_trusted_types_violation(
        &mut self,
        document_id: usize,
        directive: &str,
        blocked_uri: &str,
        policies: Vec<csp::PolicyViolation>,
        disposition: &str,
        sample: &str,
        source: Option<&CspSourceLocation>,
    ) {
        self.enqueue_policy_violation_events_with_directive(
            (document_id, None),
            directive,
            blocked_uri,
            policies,
            disposition,
            sample,
            source,
        );
    }

    /// Queues an inline handler violation with the attribute as event target.
    pub(super) fn queue_inline_handler_violation(
        &mut self,
        document_id: usize,
        element_id: usize,
        policies: Vec<csp::PolicyViolation>,
        disposition: &str,
        source: &str,
        location: Option<&CspSourceLocation>,
    ) {
        self.enqueue_policy_violation_events_with_directive(
            (document_id, Some(element_id)),
            "script-src-attr",
            "inline",
            policies,
            disposition,
            &csp::inline_report_sample(source),
            location,
        );
    }

    fn enqueue_policy_violation_events_with_directive(
        &mut self,
        target: (usize, Option<usize>),
        directive: &str,
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
                "referrer": self.document_reporting.get(&document_id)
                    .map_or("", |metadata| metadata.referrer.as_str()),
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

    fn assert_slow_report_endpoint_does_not_block_following_task(policy: &str) {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc::{Receiver, Sender, channel};

        struct SlowTransport {
            started: Sender<()>,
            release: Mutex<Receiver<()>>,
            completed: Arc<AtomicBool>,
        }

        impl crate::http::HttpTransport for SlowTransport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                assert_eq!(request.url().to_string(), "https://example.test/reports");
                self.started.send(()).unwrap();
                // The timeout only releases a failing, synchronous implementation.
                let _ = self
                    .release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2));
                self.completed.store(true, Ordering::Release);
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }

        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[
            ("Content-Security-Policy".into(), policy.into()),
            ("Reporting-Endpoints".into(), "reports=\"/reports\"".into()),
        ]);
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = channel();
        let completed = Arc::new(AtomicBool::new(false));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(SlowTransport {
                started: started_tx,
                release: Mutex::new(release_rx),
                completed: Arc::clone(&completed),
            })));

        runtime.eval("globalThis.afterReport = false; globalThis.violations = 0; document.addEventListener('securitypolicyviolation', () => violations++); fetch('/blocked').catch(() => {}); setTimeout(() => afterReport = true, 0);").unwrap();
        runtime.run_jobs().unwrap();
        runtime.run_timers(10, 1, 30);
        assert!(
            runtime
                .eval("violations === 1 && afterReport")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!completed.load(Ordering::Acquire));
        release_tx.send(()).unwrap();
    }

    #[test]
    fn slow_legacy_report_endpoint_does_not_block_following_task() {
        assert_slow_report_endpoint_does_not_block_following_task(
            "connect-src 'none'; report-uri /reports",
        );
    }

    #[test]
    fn slow_modern_report_endpoint_does_not_block_following_task() {
        assert_slow_report_endpoint_does_not_block_following_task(
            "connect-src 'none'; report-to reports",
        );
    }

    #[test]
    fn full_report_queue_keeps_violation_events_and_observer_reports() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc::{Receiver, Sender, channel};

        struct SlowFirstReport {
            started: Sender<()>,
            release: Mutex<Receiver<()>>,
            first: AtomicBool,
            sent: Arc<Mutex<Vec<String>>>,
        }

        impl crate::http::HttpTransport for SlowFirstReport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.sent.lock().unwrap().push(request.url().to_string());
                if self.first.swap(false, Ordering::AcqRel) {
                    self.started.send(()).unwrap();
                    let _ = self
                        .release
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(2));
                }
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }

        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[
            (
                "Content-Security-Policy".into(),
                "connect-src 'none'; report-to reports".into(),
            ),
            ("Reporting-Endpoints".into(), "reports=\"/reports\"".into()),
        ]);
        runtime.host_state.borrow_mut().report_delivery =
            Some(report_delivery::ReportDelivery::with_capacity(2).unwrap());
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = channel();
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(SlowFirstReport {
                started: started_tx,
                release: Mutex::new(release_rx),
                first: AtomicBool::new(true),
                sent: Arc::clone(&sent),
            })));
        runtime.eval("globalThis.events = 0; globalThis.observed = 0; document.addEventListener('securitypolicyviolation', () => events++); new ReportingObserver(batch => observed += batch.length).observe();").unwrap();
        runtime.eval("fetch('/first').catch(() => {});").unwrap();
        runtime.run_jobs().unwrap();
        runtime.run_timers(10, 1, 30);
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        for path in ["/second", "/third"] {
            runtime
                .eval(&format!("fetch('{path}').catch(() => {{}});"))
                .unwrap();
            runtime.run_jobs().unwrap();
            runtime.run_timers(10, 1, 30);
        }
        assert!(
            runtime
                .eval("events === 3 && observed === 3")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        assert_eq!(
            runtime
                .host_state
                .borrow()
                .report_delivery
                .as_ref()
                .unwrap()
                .dropped(),
            1
        );
        assert_eq!(sent.lock().unwrap().len(), 1);
        release_tx.send(()).unwrap();
        wait_for_report_delivery(&mut runtime);
        assert_eq!(sent.lock().unwrap().len(), 2);
    }

    #[test]
    fn navigation_referrer_obeys_default_origin_and_downgrade_rules() {
        let source = "https://user:password@example.test/path?q=1#private";
        assert_eq!(
            navigation_referrer(source, "https://example.test/child"),
            "https://example.test/path?q=1"
        );
        assert_eq!(
            navigation_referrer(source, "https://other.test/child"),
            "https://example.test/"
        );
        assert_eq!(navigation_referrer(source, "http://other.test/child"), "");
        assert_eq!(
            navigation_referrer("http://127.0.0.2/private", "http://other.test/child"),
            ""
        );
        assert_eq!(
            navigation_referrer("http://localhost/private", "http://other.test/child"),
            ""
        );
        assert_eq!(
            navigation_referrer("http://[::1]/private", "http://other.test/child"),
            ""
        );
        assert_eq!(
            navigation_referrer(source, "http://localhost/child"),
            "https://example.test/"
        );
        assert_eq!(navigation_referrer(source, "about:srcdoc"), "");
        assert_eq!(
            navigation_referrer("data:text/html,source", "https://example.test/"),
            ""
        );
        assert_eq!(
            navigation_referrer_with_policy(
                source,
                "https://other.test/child",
                ReferrerPolicy::NoReferrer,
            ),
            ""
        );
        assert_eq!(
            navigation_referrer_with_policy(
                source,
                "https://other.test/child",
                ReferrerPolicy::UnsafeUrl,
            ),
            "https://example.test/path?q=1"
        );
        assert_eq!(
            navigation_referrer_with_policy(
                source,
                "https://other.test/child",
                ReferrerPolicy::SameOrigin,
            ),
            ""
        );
        assert_eq!(
            response_referrer_policy(&[
                (
                    "Referrer-Policy".into(),
                    "no-referrer, future-policy".into()
                ),
                ("referrer-policy".into(), "origin".into()),
            ]),
            Some(ReferrerPolicy::Origin)
        );
        assert_eq!(
            ReferrerPolicy::parse_meta("never"),
            Some(ReferrerPolicy::NoReferrer)
        );
        assert_eq!(
            ReferrerPolicy::parse_meta("ALWAYS"),
            Some(ReferrerPolicy::UnsafeUrl)
        );
        assert_eq!(
            ReferrerPolicy::parse_meta("default"),
            Some(ReferrerPolicy::default())
        );
        assert_eq!(
            ReferrerPolicy::parse_meta("origin-when-crossorigin"),
            Some(ReferrerPolicy::OriginWhenCrossOrigin)
        );
        assert_eq!(ReferrerPolicy::parse_meta(" never "), None);
    }

    #[test]
    fn meta_referrer_policy_follows_mutation_time_and_survives_removal() {
        struct Transport(Arc<Mutex<Vec<Option<String>>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0
                    .lock()
                    .unwrap()
                    .push(request.header("Referer").map(str::to_owned));
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/private").unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.eval("const first=document.createElement('meta'); first.name='referrer'; first.content='origin'; document.head.appendChild(first); const second=document.createElement('meta'); second.setAttribute('name','referrer'); second.setAttribute('content','unsafe-url'); document.head.appendChild(second); first.content='never'; first.remove(); const frame=document.createElement('iframe'); frame.src='https://other.test/child'; document.body.appendChild(frame)").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(sent.lock().unwrap().as_slice(), &[None]);
        runtime.eval("const host=document.createElement('div'); document.body.appendChild(host); const shadow=host.attachShadow({mode:'open'}); const hidden=document.createElement('meta'); hidden.setAttribute('name','referrer'); hidden.setAttribute('content','unsafe-url'); shadow.appendChild(hidden); const next=document.createElement('iframe'); next.src='https://other.test/next'; document.body.appendChild(next)").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(sent.lock().unwrap().as_slice(), &[None, None]);
        runtime.eval("const moved=document.createElement('meta'); moved.setAttribute('name','referrer'); moved.setAttribute('content','no-referrer'); document.head.appendChild(moved); const last=document.createElement('meta'); last.setAttribute('name','referrer'); last.setAttribute('content','origin'); document.head.appendChild(last); document.head.moveBefore(moved, null); const afterMove=document.createElement('iframe'); afterMove.src='https://other.test/after-move'; document.body.appendChild(afterMove)").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            &[None, None, Some("https://example.test/".into())]
        );
    }

    #[test]
    fn initial_about_blank_uses_the_creator_url_even_with_no_referrer_policy() {
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/parent#fragment")
                .unwrap();
        runtime
            .install_response_security_headers(&[("Referrer-Policy".into(), "no-referrer".into())]);
        assert!(runtime
            .eval("const frame=document.createElement('iframe'); document.body.appendChild(frame); frame.contentDocument.referrer === 'https://example.test/parent#fragment'")
            .unwrap()
            .as_boolean()
            .unwrap());
        assert!(runtime
            .eval("window.open('', 'blank').document.referrer === 'https://example.test/parent#fragment'")
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn source_referrer_policy_suppresses_child_navigation_and_csp_referrer() {
        struct Transport(Arc<Mutex<Vec<Option<String>>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0
                    .lock()
                    .unwrap()
                    .push(request.header("Referer").map(str::to_owned));
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Security-Policy: connect-src 'none'\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        for source_policy in ["header", "meta", "attribute"] {
            let document = crate::html::TreeBuilder::parse("<body></body>").document();
            let mut runtime =
                JsRuntime::with_document_and_url(document, "https://example.test/parent#secret")
                    .unwrap();
            if source_policy == "header" {
                runtime.install_response_security_headers(&[(
                    "Referrer-Policy".into(),
                    "no-referrer".into(),
                )]);
            }
            let sent = Arc::new(Mutex::new(Vec::new()));
            runtime
                .host_state
                .borrow_mut()
                .http_client
                .set_transport(Some(Arc::new(Transport(sent.clone()))));
            runtime
                .eval(&format!(
                    "{}globalThis.frame=document.createElement('iframe'); {}frame.src='/child'; document.body.appendChild(frame);",
                    if source_policy == "meta" {
                        "const meta=document.createElement('meta'); meta.setAttribute('name','referrer'); meta.setAttribute('content','no-referrer'); document.head.appendChild(meta);"
                    } else {
                        ""
                    },
                    if source_policy == "attribute" {
                        "frame.setAttribute('referrerpolicy','no-referrer'); "
                    } else {
                        ""
                    }
                ))
                .unwrap();
            runtime.run_until_idle().unwrap();
            assert!(
                runtime
                    .eval("frame.contentDocument.referrer === ''")
                    .unwrap()
                    .as_boolean()
                    .unwrap(),
                "source_policy={source_policy}"
            );
            assert_eq!(sent.lock().unwrap().as_slice(), &[None], "{source_policy}");
        }
    }

    #[test]
    fn same_origin_redirect_keeps_the_navigation_referrer() {
        type Sent = (String, Option<String>);
        struct Transport(Arc<Mutex<Vec<Sent>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                let url = request.url().to_string();
                self.0
                    .lock()
                    .unwrap()
                    .push((url.clone(), request.header("Referer").map(str::to_owned)));
                let response = if url.ends_with("/redirect") {
                    "HTTP/1.1 302 Found\r\nLocation: /child\r\nContent-Length: 0\r\n\r\n"
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n"
                };
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/parent#secret")
                .unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime
            .eval("globalThis.frame=document.createElement('iframe'); frame.src='/redirect'; document.body.appendChild(frame);")
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert!(
            runtime
                .eval("frame.contentDocument.referrer === 'https://example.test/parent'")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            &[
                (
                    "https://example.test/redirect".into(),
                    Some("https://example.test/parent".into()),
                ),
                (
                    "https://example.test/child".into(),
                    Some("https://example.test/parent".into()),
                ),
            ]
        );
    }

    #[test]
    fn redirect_response_policy_recomputes_the_final_request_referrer() {
        struct Transport(Arc<Mutex<Vec<Option<String>>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0
                    .lock()
                    .unwrap()
                    .push(request.header("Referer").map(str::to_owned));
                let response = if request.url().path() == "/redirect" {
                    "HTTP/1.1 302 Found\r\nLocation: /child\r\nReferrer-Policy: no-referrer\r\nContent-Length: 0\r\n\r\n"
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n"
                };
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/parent").unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.eval("globalThis.frame=document.createElement('iframe'); frame.src='/redirect'; document.body.appendChild(frame)").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            &[Some("https://example.test/parent".into()), None]
        );
        assert!(
            runtime
                .eval("frame.contentDocument.referrer === ''")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn cross_origin_redirect_reduces_the_default_referrer_to_origin() {
        struct Transport(Arc<Mutex<Vec<(String, Option<String>)>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0.lock().unwrap().push((
                    request.url().to_string(),
                    request.header("Referer").map(str::to_owned),
                ));
                let response = if request.url().path() == "/redirect" {
                    "HTTP/1.1 302 Found\r\nLocation: https://other.test/child\r\nContent-Length: 0\r\n\r\n"
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n"
                };
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/parent/path#private")
                .unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.eval("globalThis.frame=document.createElement('iframe'); frame.src='/redirect'; document.body.appendChild(frame)").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            &[
                (
                    "https://example.test/redirect".into(),
                    Some("https://example.test/parent/path".into())
                ),
                (
                    "https://other.test/child".into(),
                    Some("https://example.test/".into())
                ),
            ]
        );
    }

    #[test]
    fn redirect_does_not_restore_a_removed_or_reduced_referrer() {
        struct Transport {
            sent: Arc<Mutex<Vec<Option<String>>>>,
            destination: &'static str,
        }
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.sent
                    .lock()
                    .unwrap()
                    .push(request.header("Referer").map(str::to_owned));
                let response = if request.url().path() == "/redirect" {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: {}\r\nReferrer-Policy: unsafe-url\r\nContent-Length: 0\r\n\r\n",
                        self.destination
                    )
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n"
                        .to_owned()
                };
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }
        for (policy, initial, destination, expected) in [
            (
                Some("no-referrer"),
                "/redirect",
                "https://evil.test/final",
                None,
            ),
            (
                None,
                "https://middle.test/redirect",
                "https://last.test/final",
                Some("https://example.test/"),
            ),
        ] {
            let document = crate::html::TreeBuilder::parse("<body></body>").document();
            let mut runtime =
                JsRuntime::with_document_and_url(document, "https://example.test/private#fragment")
                    .unwrap();
            if let Some(policy) = policy {
                runtime.install_response_security_headers(&[(
                    "Referrer-Policy".into(),
                    policy.into(),
                )]);
            }
            let sent = Arc::new(Mutex::new(Vec::new()));
            runtime
                .host_state
                .borrow_mut()
                .http_client
                .set_transport(Some(Arc::new(Transport {
                    sent: sent.clone(),
                    destination,
                })));
            runtime
                .eval(&format!("const frame=document.createElement('iframe'); frame.src={initial:?}; document.body.appendChild(frame)"))
                .unwrap();
            runtime.run_until_idle().unwrap();
            assert_eq!(
                sent.lock().unwrap().as_slice(),
                &[expected.map(str::to_owned), expected.map(str::to_owned)],
                "initial={initial}"
            );
        }
    }

    #[test]
    fn queued_child_navigation_uses_the_departing_document_as_referrer() {
        struct Transport(Arc<Mutex<Vec<(String, Option<String>)>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0.lock().unwrap().push((
                    request.url().to_string(),
                    request.header("Referer").map(str::to_owned),
                ));
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/parent").unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.eval("globalThis.frame=document.createElement('iframe'); frame.src='/child'; document.body.appendChild(frame)").unwrap();
        runtime.run_until_idle().unwrap();
        runtime
            .eval("frame.contentWindow.eval(\"location.assign('/next')\")")
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            &[
                (
                    "https://example.test/child".into(),
                    Some("https://example.test/parent".into())
                ),
                (
                    "https://example.test/next".into(),
                    Some("https://example.test/child".into())
                ),
            ]
        );
    }

    #[test]
    fn srcdoc_descendant_request_uses_the_container_url_after_fragment_navigation() {
        struct Transport(Arc<Mutex<Vec<Option<String>>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0
                    .lock()
                    .unwrap()
                    .push(request.header("Referer").map(str::to_owned));
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/parent").unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.eval("globalThis.frame=document.createElement('iframe'); frame.srcdoc='<body></body>'; document.body.appendChild(frame)").unwrap();
        runtime.run_until_idle().unwrap();
        assert!(
            runtime
                .eval("frame.contentDocument.referrer === ''")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        runtime.eval("frame.contentWindow.eval(\"location.hash='#x'; const nested=document.createElement('iframe'); nested.src='/grandchild'; document.body.appendChild(nested)\")").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            &[Some("https://example.test/parent".into())]
        );
    }

    #[test]
    fn sibling_frame_form_uses_the_form_documents_referrer() {
        struct Transport(Arc<Mutex<Vec<(String, Option<String>)>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0.lock().unwrap().push((
                    request.url().to_string(),
                    request.header("Referer").map(str::to_owned),
                ));
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse(
            "<body><iframe id=a name=a src=/a></iframe><iframe id=b name=b src=/b></iframe></body>",
        )
        .document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/parent").unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.run_until_idle().unwrap();
        runtime.eval("document.querySelector('#a').contentWindow.eval(\"const form=document.createElement('form'); form.action='/submitted'; form.target='b'; document.body.appendChild(form); form.submit()\")").unwrap();
        runtime.run_until_idle().unwrap();
        let sent = sent.lock().unwrap();
        let submission = sent
            .iter()
            .find(|(url, _)| url.ends_with("/submitted"))
            .expect("sibling frame form submission");
        assert_eq!(submission.1.as_deref(), Some("https://example.test/a"));
    }

    #[test]
    fn opaque_sandbox_document_does_not_send_a_referrer() {
        struct Transport(Arc<Mutex<Vec<Option<String>>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0
                    .lock()
                    .unwrap()
                    .push(request.header("Referer").map(str::to_owned));
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse(
            r#"<body><iframe sandbox="allow-scripts" srcdoc="<script>const nested=document.createElement('iframe'); nested.src='/opaque-child'; document.body.appendChild(nested);</script>"></iframe></body>"#,
        )
        .document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/parent").unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.run_until_idle().unwrap();
        assert_eq!(sent.lock().unwrap().as_slice(), &[None]);
    }

    #[test]
    fn reused_popup_navigation_uses_the_calling_document_referrer() {
        struct Transport(Arc<Mutex<Vec<(String, Option<String>)>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                self.0.lock().unwrap().push((
                    request.url().to_string(),
                    request.header("Referer").map(str::to_owned),
                ));
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/opener").unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime.eval("window.open('/old', 'named')").unwrap();
        runtime.run_until_idle().unwrap();
        runtime.eval("window.open('/new', 'named')").unwrap();
        runtime.run_until_idle().unwrap();
        let sent = sent.lock().unwrap();
        let new = sent
            .iter()
            .find(|(url, _)| url.ends_with("/new"))
            .expect("reused popup navigation");
        assert_eq!(new.1.as_deref(), Some("https://example.test/opener"));
    }

    #[test]
    fn popup_classic_script_connect_violation_keeps_its_source_position() {
        struct Transport;
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                assert_eq!(request.url().to_string(), "https://example.test/popup");
                let body = "<html>\n<body>\n<script>\nwindow.events=[];\ndocument.addEventListener('securitypolicyviolation', e => events.push([e.sourceFile,e.lineNumber,e.columnNumber]));\nfetch('/blocked').catch(()=>{});\n</script>\n</body>\n</html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Security-Policy: script-src 'unsafe-inline'; connect-src 'none'\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/opener").unwrap();
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport)));
        runtime
            .eval("globalThis.child=window.open('/popup', 'view')")
            .unwrap();
        runtime.run_until_idle().unwrap();
        let observed = runtime
            .eval("JSON.stringify(child.events)")
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&observed).unwrap(),
            serde_json::json!([["https://example.test/popup", 6, 1]])
        );
    }

    #[test]
    fn popup_later_classic_scripts_run_after_page_overwrites_current_script_setter() {
        struct Transport;
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                assert_eq!(request.url().to_string(), "https://example.test/popup");
                let body = "<script>window.__omoikane_set_current_script=()=>{throw Error('page override')}</script><script>window.laterScriptRan=true</script>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/opener").unwrap();
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport)));
        runtime
            .eval("globalThis.child=window.open('/popup', 'view')")
            .unwrap();
        runtime.run_until_idle().unwrap();
        let observed = runtime
            .eval("child.laterScriptRan === true")
            .unwrap()
            .to_boolean();
        assert!(observed);
    }

    #[test]
    fn document_write_classic_script_connect_violation_keeps_its_source_position() {
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[(
            "Content-Security-Policy".into(),
            "script-src 'unsafe-inline'; connect-src 'none'".into(),
        )]);
        runtime.eval(r#"globalThis.events=[];
            document.addEventListener('securitypolicyviolation', e => events.push([e.sourceFile,e.lineNumber,e.columnNumber]));
            document.write('<script>\nfetch("/blocked").catch(()=>{});\n</script>');"#).unwrap();
        runtime.run_until_idle().unwrap();
        let observed = runtime
            .eval("JSON.stringify(events)")
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&observed).unwrap(),
            serde_json::json!([["https://example.test/document", 2, 1]])
        );
    }

    #[test]
    fn inserted_inline_classic_script_connect_violation_keeps_its_source_position() {
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[(
            "Content-Security-Policy".into(),
            "script-src 'unsafe-inline'; connect-src 'none'".into(),
        )]);
        runtime
            .eval(
                r#"globalThis.events=[];
                document.addEventListener('securitypolicyviolation', e => events.push([e.sourceFile,e.lineNumber,e.columnNumber]));
                const inserted=document.createElement('script');
                inserted.textContent='\nfetch("/blocked").catch(()=>{});';
                document.body.appendChild(inserted);"#,
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
        let observed = runtime
            .eval("JSON.stringify(events)")
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&observed).unwrap(),
            serde_json::json!([["https://example.test/document", 2, 1]])
        );
    }

    #[test]
    fn inserted_inline_script_evaluator_is_private_to_bootstrap() {
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        let result = runtime
            .eval(
                "JSON.stringify([typeof globalThis.__omoikane_eval_inserted_inline_script, \
                 typeof __omoikane_eval_inserted_inline_script])",
            )
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "[\"undefined\",\"undefined\"]");
    }

    #[test]
    fn inserted_inline_script_uses_its_iframe_realm_and_document_url() {
        struct Transport;
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                assert_eq!(request.url().to_string(), "https://example.test/child");
                let body = "<body><script>globalThis.events=[];document.addEventListener('securitypolicyviolation',e=>events.push([e.sourceFile,e.lineNumber,e.columnNumber]));</script></body>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Security-Policy: script-src 'unsafe-inline'; connect-src 'none'\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/parent").unwrap();
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport)));
        runtime
            .eval("globalThis.frame=document.createElement('iframe');frame.src='/child';document.body.appendChild(frame)")
            .unwrap();
        runtime.run_until_idle().unwrap();
        runtime
            .eval(
                r#"const childDoc=frame.contentDocument;
                const inserted=childDoc.createElement('script');
                inserted.textContent='\nfetch("/blocked").catch(()=>{});';
                childDoc.body.appendChild(inserted);"#,
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
        let observed = runtime
            .eval("JSON.stringify(frame.contentWindow.events)")
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&observed).unwrap(),
            serde_json::json!([["https://example.test/child", 2, 1]])
        );
    }

    #[test]
    fn inserted_inline_script_cannot_run_in_a_different_csp_document() {
        struct Transport;
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                assert_eq!(request.url().to_string(), "https://example.test/child");
                let body = "<html><body></body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Security-Policy: script-src 'none'\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/parent").unwrap();
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport)));
        runtime
            .eval("globalThis.frame=document.createElement('iframe');frame.src='/child';document.body.appendChild(frame)")
            .unwrap();
        runtime.run_until_idle().unwrap();
        runtime
            .eval(
                r#"const script=document.createElement('script');
                   script.textContent='globalThis.cspBypassRan=true';
                   window.__omoikane_set_current_script=()=>{
                     frame.contentDocument.body.appendChild(script);
                   };
                   document.body.appendChild(script);"#,
            )
            .unwrap();
        let result = runtime
            .eval("typeof frame.contentWindow.cspBypassRan")
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "undefined");
    }

    #[test]
    fn interrupted_inserted_inline_script_does_not_retain_its_source() {
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/parent").unwrap();
        runtime
            .eval(
                r#"const script=document.createElement('script');
                   script.textContent='globalThis.interruptedScriptRan=true';
                   window.__omoikane_set_current_script=()=>{throw new Error('interrupted')};
                   document.body.appendChild(script);"#,
            )
            .unwrap();
        assert!(
            runtime
                .host_state
                .borrow()
                .prepared_inserted_inline_scripts
                .is_empty()
        );
        let result = runtime
            .eval("typeof globalThis.interruptedScriptRan")
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "undefined");
    }

    #[test]
    fn child_csp_reports_the_navigation_referrer_after_parent_history_changes() {
        type Sent = (String, Option<String>, Option<serde_json::Value>);
        struct Transport(Arc<Mutex<Vec<Sent>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                let url = request.url().to_string();
                self.0.lock().unwrap().push((
                    url.clone(),
                    request.header("Referer").map(str::to_owned),
                    request
                        .body()
                        .map(|body| serde_json::from_slice(body).unwrap()),
                ));
                let response = if url.ends_with("/child") {
                    let body = "<html><body></body></html>";
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Security-Policy: connect-src 'none'; report-uri /collector\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                } else {
                    "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_owned()
                };
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }

        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime = JsRuntime::with_document_and_url(
            document,
            "https://example.test/parent/path?q=1#private",
        )
        .unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(sent.clone()))));
        runtime
            .eval("globalThis.frame=document.createElement('iframe'); frame.src='/child'; document.body.appendChild(frame);")
            .unwrap();
        runtime.run_until_idle().unwrap();
        runtime
            .eval(
                "globalThis.events=[]; const child=frame.contentDocument; child.addEventListener('securitypolicyviolation', event => events.push(event.referrer)); history.pushState(null,'','/changed'); frame.contentWindow.eval(\"fetch('/blocked').catch(()=>{})\");",
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
        wait_for_report_delivery(&mut runtime);
        assert!(runtime
            .eval("frame.contentDocument.referrer === 'https://example.test/parent/path?q=1' && events.length === 1 && events[0] === frame.contentDocument.referrer")
            .unwrap()
            .as_boolean()
            .unwrap());
        let sent = sent.lock().unwrap();
        assert_eq!(sent[0].0, "https://example.test/child");
        assert_eq!(
            sent[0].1.as_deref(),
            Some("https://example.test/parent/path?q=1")
        );
        let report = sent
            .iter()
            .find(|(url, _, _)| url.ends_with("/collector"))
            .and_then(|(_, _, body)| body.as_ref())
            .expect("legacy CSP report");
        assert_eq!(
            report["csp-report"]["referrer"],
            "https://example.test/parent/path?q=1"
        );
    }

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
        wait_for_report_delivery(&mut runtime);
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
            wait_for_report_delivery(&mut runtime);
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
            wait_for_report_delivery(&mut runtime);
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
        wait_for_report_delivery(&mut runtime);
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
                wait_for_report_delivery(&mut runtime);
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
        runtime.eval("globalThis.reports=[]; globalThis.targets=[]; document.addEventListener('securitypolicyviolation', e=>targets.push(e.target === document.querySelector('script'))); new ReportingObserver(batch=>reports.push(...batch)).observe();").unwrap();
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
                child.addEventListener('securitypolicyviolation', e=>received.push([e.blockedURI,e.target===script]));
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
    fn constructed_stylesheets_ignore_inline_style_policies_but_style_elements_do_not() {
        let document = crate::html::TreeBuilder::parse(
            "<body><p id=constructed>x</p><p id=inline>x</p><div id=host></div></body>",
        )
        .document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[
            (
                "Content-Security-Policy".into(),
                "style-src 'none'; report-to observations".into(),
            ),
            (
                "Content-Security-Policy-Report-Only".into(),
                "style-src 'none'; report-to observations".into(),
            ),
        ]);
        runtime
            .eval(
                r#"
            globalThis.events = []; globalThis.reports = [];
            document.addEventListener('securitypolicyviolation', event => events.push({
              target: event.target.id,
              directive: event.effectiveDirective,
              blockedURI: event.blockedURI,
              disposition: event.disposition
            }));
            new ReportingObserver(batch => reports.push(...batch)).observe();
            globalThis.sheet = new CSSStyleSheet();
            sheet.replaceSync('#constructed { color: red; } #shadow { color: blue; }');
            document.adoptedStyleSheets = [sheet];
            globalThis.shadow = document.getElementById('host').attachShadow({mode: 'open'});
            shadow.innerHTML = '<p id=shadow>x</p>';
            shadow.adoptedStyleSheets = [sheet];
            globalThis.first = getComputedStyle(document.getElementById('constructed')).color;
            globalThis.second = getComputedStyle(shadow.querySelector('#shadow')).color;
        "#,
            )
            .unwrap();
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("first === 'rgb(255, 0, 0)' && second === 'rgb(0, 0, 255)' && events.length === 0 && reports.length === 0 && document.cspViolations.length === 0").unwrap().as_boolean().unwrap());

        runtime.set_viewport(420.0, 320.0);
        runtime
            .eval("sheet.replaceSync('#constructed { color: blue; } #shadow { color: red; }'); globalThis.updated = getComputedStyle(document.getElementById('constructed')).color; globalThis.shadowUpdated = getComputedStyle(shadow.querySelector('#shadow')).color")
            .unwrap();
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("updated === 'rgb(0, 0, 255)' && shadowUpdated === 'rgb(255, 0, 0)' && events.length === 0 && reports.length === 0 && document.cspViolations.length === 0").unwrap().as_boolean().unwrap());

        runtime
            .eval(
                r#"
            const blocked = document.createElement('style');
            blocked.id = 'inlineStyle';
            blocked.textContent = '#inline { color: blue; }';
            document.body.appendChild(blocked);
            globalThis.inlineColor = getComputedStyle(document.getElementById('inline')).color;
        "#,
            )
            .unwrap();
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("inlineColor !== 'rgb(0, 0, 255)' && events.length === 2 && events.every(event => event.target === 'inlineStyle' && event.directive === 'style-src-elem' && event.blockedURI === 'inline') && events.map(event => event.disposition).sort().join(',') === 'enforce,report' && reports.length === 2 && reports.every(report => report.body.blockedURL === 'inline') && document.cspViolations.length === 1").unwrap().as_boolean().unwrap());
    }

    #[test]
    fn inline_style_hash_allows_matching_css_and_reports_other_policy() {
        use base64::Engine as _;
        use sha2::Digest as _;

        let css = "#target { color: red; }";
        let digest =
            base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(css.as_bytes()));
        let document = crate::html::TreeBuilder::parse(&format!(
            "<style id=sheet>{css}</style><body><p id=target>x</p></body>"
        ))
        .document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[
            (
                "Content-Security-Policy".into(),
                format!("style-src-elem 'sha256-{digest}'"),
            ),
            (
                "Content-Security-Policy-Report-Only".into(),
                "style-src-elem 'none' 'report-sample'; report-to reports".into(),
            ),
        ]);
        runtime.eval("globalThis.events=[]; globalThis.reports=[]; document.addEventListener('securitypolicyviolation', e => events.push([e.disposition,e.sample])); new ReportingObserver(batch => reports.push(...batch)).observe(); globalThis.color=getComputedStyle(document.getElementById('target')).color;").unwrap();
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("color === 'rgb(255, 0, 0)' && events.length === 1 && events[0][0] === 'report' && events[0][1] === '#target { color: red; }' && reports.length === 1 && document.cspViolations.length === 0").unwrap().as_boolean().unwrap());
    }

    #[test]
    fn parser_inline_script_hash_allows_matching_source_and_reports_other_policy() {
        use base64::Engine as _;
        use sha2::Digest as _;

        let source = "globalThis.hashRan = true;";
        let digest = base64::engine::general_purpose::STANDARD
            .encode(sha2::Sha256::digest(source.as_bytes()));
        for matches_hash in [true, false] {
            let document = crate::html::TreeBuilder::parse(&format!(
                "<script id=source>{source}</script><body></body>"
            ))
            .document();
            let mut runtime =
                JsRuntime::with_document_and_url(document, "https://example.test/document")
                    .unwrap();
            let allowed_hash = if matches_hash {
                digest.clone()
            } else {
                base64::engine::general_purpose::STANDARD
                    .encode(sha2::Sha256::digest(b"different script"))
            };
            runtime.install_response_security_headers(&[
                (
                    "Content-Security-Policy".into(),
                    format!(
                        "script-src-elem 'sha256-{allowed_hash}' 'report-sample'; report-to reports"
                    ),
                ),
                (
                    "Content-Security-Policy-Report-Only".into(),
                    "script-src-elem 'none' 'report-sample'; report-to reports".into(),
                ),
            ]);
            runtime.eval("globalThis.hashRan=false; globalThis.events=[]; globalThis.reports=[]; document.addEventListener('securitypolicyviolation', e => events.push([e.disposition,e.sample])); new ReportingObserver(batch => reports.push(...batch)).observe();").unwrap();
            assert!(runtime.execute_document_scripts(None).is_empty());
            runtime.run_timers(10, 1, 30);
            assert!(runtime.eval(if matches_hash {
                "hashRan && events.length === 1 && events[0].join('|') === 'report|globalThis.hashRan = true;' && reports.length === 1 && document.cspViolations.length === 0"
            } else {
                "!hashRan && events.length === 2 && events.map(e => e[0]).sort().join(',') === 'enforce,report' && events.every(e => e[1] === 'globalThis.hashRan = true;') && reports.length === 2 && document.cspViolations.length === 1"
            }).unwrap().as_boolean().unwrap());
        }
    }

    #[test]
    fn style_attribute_hash_reports_each_mutation_without_resolver_duplicates() {
        use base64::Engine as _;
        use sha2::Digest as _;

        let source = "color: red;";
        let digest = base64::engine::general_purpose::STANDARD
            .encode(sha2::Sha256::digest(source.as_bytes()));
        let document = crate::html::TreeBuilder::parse(
            "<body><p id=target style=\"color: red;\">x</p></body>",
        )
        .document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[
            (
                "Content-Security-Policy".into(),
                format!("style-src-attr 'unsafe-hashes' 'sha256-{digest}' 'report-sample'; report-to reports"),
            ),
            (
                "Content-Security-Policy-Report-Only".into(),
                "style-src-attr 'none' 'report-sample'; report-to reports".into(),
            ),
        ]);
        runtime.eval("globalThis.events=[]; globalThis.reports=[]; document.addEventListener('securitypolicyviolation', e => events.push([e.target.id,e.disposition,e.sample])); new ReportingObserver(batch => reports.push(...batch)).observe(); globalThis.color=getComputedStyle(document.getElementById('target')).color;").unwrap();
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("color === 'rgb(255, 0, 0)' && events.length === 1 && events[0].join('|') === 'target|report|color: red;' && reports.length === 1 && document.cspViolations.length === 0").unwrap().as_boolean().unwrap());

        runtime.set_viewport(420.0, 320.0);
        runtime
            .eval("getComputedStyle(document.getElementById('target')).color")
            .unwrap();
        runtime.run_timers(10, 1, 30);
        assert_eq!(
            runtime.eval("events.length").unwrap().as_number(),
            Some(1.0)
        );

        runtime.eval("document.getElementById('target').setAttribute('style', 'color: blue;'); globalThis.blockedColor=getComputedStyle(document.getElementById('target')).color;").unwrap();
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("blockedColor !== 'rgb(0, 0, 255)' && events.length === 3 && events.slice(1).map(e => e[1]).sort().join(',') === 'enforce,report' && reports.length === 3 && document.cspViolations.length === 1").unwrap().as_boolean().unwrap());

        runtime
            .eval("document.getElementById('target').setAttribute('style', 'color: blue;');")
            .unwrap();
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("events.length === 5 && events.slice(3).map(e => e[1]).sort().join(',') === 'enforce,report' && events.slice(3).every(e => e[2] === 'color: blue;') && reports.length === 5 && document.cspViolations.length === 1").unwrap().as_boolean().unwrap());

        runtime.set_viewport(421.0, 321.0);
        runtime
            .eval("getComputedStyle(document.getElementById('target')).color")
            .unwrap();
        runtime.run_timers(10, 1, 30);
        assert_eq!(
            runtime.eval("events.length").unwrap().as_number(),
            Some(5.0)
        );
    }

    #[test]
    fn moving_style_attribute_to_another_document_rechecks_same_policy() {
        let first = crate::html::TreeBuilder::parse(
            "<body><p id=target style=\"color: red;\">x</p></body>",
        )
        .document();
        let second = crate::html::TreeBuilder::parse("<body></body>").document();
        let element = first.query_selector("#target").unwrap();
        let second_body = second.query_selector("body").unwrap();
        let mut runtime =
            JsRuntime::with_document_and_url(first.clone(), "https://example.test/first").unwrap();
        runtime.install_csp_policy(&["style-src-attr 'none'".into()]);
        let mut state = runtime.host_state.borrow_mut();
        let policy = state.csp_policy_for_document(&first);
        state.document_csp.insert(second.identity(), policy);

        assert!(!state.csp_allows_inline_style_attribute(&first, &element, "color: red;"));
        while state.event_loop.pop_task().is_some() {}

        second_body.append_child(element.clone());
        assert!(!state.csp_allows_inline_style_attribute(&second, &element, "color: red;"));
        let second_violation_document = std::iter::from_fn(|| state.event_loop.pop_task())
            .find_map(|(_, task)| match task {
                Task::SecurityPolicyViolation { document_id, .. } => Some(document_id),
                _ => None,
            });
        assert_eq!(
            second_violation_document,
            Some(second.identity()),
            "moving an unchanged style attribute must report against the new Document"
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
            wait_for_report_delivery(&mut runtime);
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
        wait_for_report_delivery(&mut runtime);
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
            wait_for_report_delivery(&mut runtime);
        }
        assert_eq!(*sent.lock().unwrap(), vec!["https://example.test/reports"]);
        let observed = runtime.eval("JSON.stringify({events, reports})").unwrap();
        assert!(runtime.eval("events.length === 2 && reports.length === 2 && events[0] === 'https://example.test/first' && events[1] === 'https://example.test/second' && reports[1].body.blockedURL === events[1]").unwrap().as_boolean().unwrap(), "{observed:?}");
    }

    #[test]
    fn old_gone_response_does_not_remove_reinstalled_endpoint() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc::{Receiver, Sender, channel};

        struct GoneFirst {
            started: Sender<()>,
            release: Mutex<Receiver<()>>,
            first: AtomicBool,
            sent: Arc<Mutex<usize>>,
        }

        impl crate::http::HttpTransport for GoneFirst {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                assert_eq!(request.url().to_string(), "https://example.test/reports");
                *self.sent.lock().unwrap() += 1;
                let response = if self.first.swap(false, Ordering::AcqRel) {
                    self.started.send(()).unwrap();
                    let _ = self
                        .release
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(2));
                    "HTTP/1.1 410 Gone\r\nContent-Length: 0\r\n\r\n"
                } else {
                    "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"
                };
                crate::http::HttpResponse::parse(&mut response.as_bytes())
            }
        }

        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        let headers = [("Reporting-Endpoints".into(), "reports=\"/reports\"".into())];
        runtime.install_response_security_headers(&headers);
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = channel();
        let sent = Arc::new(Mutex::new(0));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(GoneFirst {
                started: started_tx,
                release: Mutex::new(release_rx),
                first: AtomicBool::new(true),
                sent: Arc::clone(&sent),
            })));
        runtime
            .generate_test_report_for_group("old", "reports")
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        runtime
            .generate_test_report_for_group("also old", "reports")
            .unwrap();
        runtime.install_response_security_headers(&headers);
        release_tx.send(()).unwrap();
        wait_for_report_delivery(&mut runtime);
        assert_eq!(*sent.lock().unwrap(), 1);
        let document_id = runtime.document().identity();
        assert_eq!(
            runtime.host_state.borrow().document_csp[&document_id].reporting_endpoint("reports"),
            Some("https://example.test/reports".into())
        );
        runtime
            .generate_test_report_for_group("new", "reports")
            .unwrap();
        wait_for_report_delivery(&mut runtime);
        assert_eq!(*sent.lock().unwrap(), 2);
    }

    #[test]
    fn queued_reports_finish_after_runtime_teardown() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc::{Receiver, Sender, channel};

        struct SlowFirst {
            first: AtomicBool,
            started: Sender<()>,
            release: Mutex<Receiver<()>>,
            finished: Sender<()>,
        }

        impl crate::http::HttpTransport for SlowFirst {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                assert_eq!(request.url().to_string(), "https://example.test/reports");
                if self.first.swap(false, Ordering::AcqRel) {
                    self.started.send(()).unwrap();
                    let _ = self
                        .release
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(2));
                }
                self.finished.send(()).unwrap();
                crate::http::HttpResponse::parse(
                    &mut &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }

        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_response_security_headers(&[(
            "Reporting-Endpoints".into(),
            "reports=\"/reports\"".into(),
        )]);
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = channel();
        let (finished_tx, finished_rx) = channel();
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(SlowFirst {
                first: AtomicBool::new(true),
                started: started_tx,
                release: Mutex::new(release_rx),
                finished: finished_tx,
            })));
        runtime
            .generate_test_report_for_group("first", "reports")
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        runtime
            .generate_test_report_for_group("second", "reports")
            .unwrap();
        drop(runtime);
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
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
        wait_for_report_delivery(&mut runtime);
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
        wait_for_report_delivery(&mut runtime);
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
        wait_for_report_delivery(&mut runtime);
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
    fn third_party_reports_use_document_storage_access_for_cookies() {
        type Sent = (String, Option<String>);
        struct Transport(Arc<Mutex<Vec<Sent>>>);
        impl crate::http::HttpTransport for Transport {
            fn send(
                &self,
                request: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
                let path = request.url().path();
                if path == "/frame" {
                    return Ok(crate::http::HttpResponse::new(
                        200,
                        "OK",
                        vec![
                            ("Content-Type".into(), "text/html".into()),
                            (
                                "Reporting-Endpoints".into(),
                                "reports=\"https://third.test/modern\"".into(),
                            ),
                        ],
                        b"<html><body><script>globalThis.frameReady = true</script></body></html>"
                            .to_vec(),
                    ));
                }
                if request.method() == Method::Post {
                    self.0
                        .lock()
                        .unwrap()
                        .push((path.to_owned(), request.header("Cookie").map(str::to_owned)));
                }
                Ok(crate::http::HttpResponse::new(
                    200,
                    "OK",
                    vec![
                        (
                            "Access-Control-Allow-Origin".into(),
                            "https://third.test".into(),
                        ),
                        ("Access-Control-Allow-Methods".into(), "POST".into()),
                        ("Access-Control-Allow-Headers".into(), "content-type".into()),
                        (
                            "Set-Cookie".into(),
                            "report=stored; Path=/; Secure; SameSite=None".into(),
                        ),
                    ],
                    vec![],
                ))
            }
        }

        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime = JsRuntime::with_document_and_url(document, "https://top.test/").unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime.set_http_resource_transport(Some(Arc::new(Transport(Arc::clone(&sent)))));
        let third_url: crate::http::Url = "https://third.test/frame".parse().unwrap();
        let store = runtime.host_state.borrow().cookie_store.clone();
        store
            .lock()
            .unwrap()
            .add_from_header_for_url("session=seed; Path=/; Secure; SameSite=None", &third_url);
        runtime
            .eval("const frame = document.createElement('iframe'); frame.src = 'https://third.test/frame'; document.body.appendChild(frame)")
            .unwrap();
        runtime.tick(0).unwrap();
        let child = runtime
            .host_state
            .borrow()
            .iframe_documents
            .values()
            .next()
            .unwrap()
            .document
            .clone();
        let child_id = child.identity();
        assert_eq!(
            runtime.host_state.borrow().document_urls.get(&child_id),
            Some(&"https://third.test/frame".to_owned())
        );
        assert!(runtime.existing_realm_for_document(child_id).is_some());
        let legacy = serde_json::json!({
            "documentURI": "https://third.test/frame", "referrer": "",
            "blockedURI": "https://third.test/blocked", "effectiveDirective": "connect-src",
            "violatedDirective": "connect-src",
            "originalPolicy": "connect-src 'none'; report-uri https://third.test/legacy",
            "disposition": "enforce", "statusCode": 200, "sample": ""
        });
        let modern = serde_json::json!({
            "type": "test", "url": "https://third.test/frame", "body": {}
        });

        runtime.deliver_csp_report(child_id, &legacy.to_string());
        wait_for_report_delivery(&mut runtime);
        runtime.deliver_modern_report(child_id, "reports", modern.clone());
        wait_for_report_delivery(&mut runtime);
        assert_eq!(
            store
                .lock()
                .unwrap()
                .document_cookie(&third_url, &third_url),
            "session=seed"
        );

        runtime
            .with_trusted_storage_access_input(|runtime| {
                runtime.eval_in_document_realm(
                    child_id,
                    "document.addEventListener('click', () => document.requestStorageAccess().then( \
                       () => document.documentElement.setAttribute('data-granted', 'yes'), \
                       e => document.documentElement.setAttribute('data-granted', e.name)), {once:true}); \
                     __omoikane_dispatch_mouse_input(document.body.__id, 'click', {button:0}, false)",
                )
            })
            .unwrap();
        assert_eq!(
            child
                .child_nodes()
                .into_iter()
                .find(|node| node.node_type() == NodeType::Element)
                .and_then(|html| html.get_attribute("data-granted"))
                .as_deref(),
            Some("yes")
        );
        {
            let state = runtime.host_state.borrow();
            let mut request = HttpRequest::new(Method::Post, third_url.clone());
            state.set_document_cookie_request_context(child_id, &mut request);
            assert!(
                request.storage_access_eligible(),
                "grant missing from report request"
            );
            assert_eq!(
                script_errors::document_fetch_origin(&state, child_id),
                CorsOrigin::from_url(&third_url),
                "legacy report credentials need the child security origin"
            );
        }
        runtime.deliver_csp_report(child_id, &legacy.to_string());
        wait_for_report_delivery(&mut runtime);
        runtime.deliver_modern_report(child_id, "reports", modern);
        wait_for_report_delivery(&mut runtime);

        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 4, "both report delivery paths must send twice");
        assert_eq!(sent[0], ("/legacy".into(), None));
        assert_eq!(sent[1], ("/modern".into(), None));
        assert_eq!(sent[2], ("/legacy".into(), Some("session=seed".into())));
        assert_eq!(
            sent[3],
            ("/modern".into(), Some("session=seed; report=stored".into()))
        );
        assert!(
            store
                .lock()
                .unwrap()
                .document_cookie(&third_url, &third_url)
                .contains("report=stored")
        );
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
        wait_for_report_delivery(&mut runtime);

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
    fn generic_reporting_observer_entry_queues_coop_and_coep() {
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        let document_id = runtime.document().identity();
        runtime.eval("globalThis.received = []; new ReportingObserver(batch => received.push(...batch), {types: ['coop', 'coep']}).observe();").unwrap();
        assert!(
            runtime
                .eval("typeof __omoikane_queue_reporting_api_report === 'undefined'")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        runtime.eval("globalThis.__omoikane_queue_reporting_api_report = () => {}; __omoikane_queue_reporting_api_report({type: 'coop', url: 'https://fake.test/', body: {}}); delete globalThis.__omoikane_queue_reporting_api_report;").unwrap();
        runtime.run_timers(10, 1, 30);
        assert_eq!(
            runtime.eval("received.length").unwrap().as_number(),
            Some(0.0)
        );
        for report_type in ["coop", "coep"] {
            let report = serde_json::json!({
                "type": report_type,
                "url": "https://example.test/document",
                "age": 0,
                "user_agent": "test",
                "body": {"reason": report_type},
            });
            runtime
                .queue_reporting_observer_report(document_id, &report)
                .unwrap();
        }
        runtime.run_timers(10, 1, 30);
        assert!(runtime.eval("received.length === 2 && received[0].type === 'coop' && received[0].body.reason === 'coop' && received[1].type === 'coep' && received[1].body.reason === 'coep' && received.every(report => report.url === 'https://example.test/document')").unwrap().as_boolean().unwrap());
        runtime.eval("globalThis.buffered = []; new ReportingObserver(batch => buffered.push(...batch), {types: ['coep'], buffered: true}).observe();").unwrap();
        runtime.run_timers(10, 1, 30);
        assert!(
            runtime
                .eval("buffered.length === 1 && buffered[0].body.reason === 'coep'")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
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
    fn attribute_mutations_capture_csp_source_only_for_resource_attributes() {
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime
            .eval(
                "globalThis.captureOrdinary=document.createElement('div');\
                 globalThis.captureScript=document.createElement('script');",
            )
            .unwrap();
        take_csp_source_capture_calls();

        runtime
            .eval(
                "captureOrdinary.setAttribute('style','color:red');\
                 captureOrdinary.removeAttribute('style');\
                 captureOrdinary.setAttribute('data-loaded','true');\
                 captureOrdinary.removeAttribute('data-loaded');\
                 captureOrdinary.setAttributeNS(null,'data-state','ready');\
                 captureOrdinary.removeAttributeNS(null,'data-state');\
                 captureScript.setAttribute('unknown','value');\
                 captureScript.removeAttribute('unknown');\
                 captureScript.setAttributeNS('urn:test','src','namespaced');\
                 captureScript.removeAttributeNS('urn:test','src');\
                 captureScript.setAttributeNS(null,'SRC','case-sensitive');\
                 captureScript.removeAttributeNS(null,'SRC');",
            )
            .unwrap();
        assert_eq!(take_csp_source_capture_calls(), 0);
        assert!(
            runtime
                .eval(
                    "!captureOrdinary.hasAttribute('style') &&\
                     !captureOrdinary.hasAttribute('data-loaded') &&\
                     !captureOrdinary.hasAttributeNS(null,'data-state') &&\
                     !captureScript.hasAttribute('unknown') &&\
                     !captureScript.hasAttributeNS('urn:test','src') &&\
                     !captureScript.hasAttributeNS(null,'SRC')",
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );

        runtime
            .eval(
                "captureScript.setAttribute('src','first.js');\
                 captureScript.removeAttribute('src');\
                 captureScript.setAttributeNS(null,'src','second.js');\
                 captureScript.removeAttributeNS(null,'src');",
            )
            .unwrap();
        assert_eq!(take_csp_source_capture_calls(), 4);
        assert!(
            runtime
                .eval("!captureScript.hasAttributeNS(null,'src')")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn connected_script_attribute_capture_keeps_author_source_position() {
        let document = crate::html::TreeBuilder::parse(
            "<html><body><script id='capture-target'></script></body></html>",
        )
        .document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        let script_id = {
            let mut state = runtime.host_state.borrow_mut();
            let script = state.document.query_selector("#capture-target").unwrap();
            let script_id = script.identity();
            state.runnable_inserted_scripts.insert(script_id);
            state.started_inserted_scripts.remove(&script_id);
            script_id
        };
        take_csp_source_capture_calls();

        let (evaluated, ..) = runtime.eval_safe_timed_source(
            "const target=document.getElementById('capture-target');\n\
             target.setAttribute('src','https://blocked.test/script.js');",
            Some("https://private.test/effective-attribute.js"),
            Some("https://example.test/requested-attribute.js"),
            (41, 9),
            false,
        );
        evaluated.unwrap();

        assert_eq!(take_csp_source_capture_calls(), 1);
        let state = runtime.host_state.borrow();
        let source = &state.pending_script_csp_sources[&script_id];
        assert_eq!(
            source.source_file,
            "https://example.test/requested-attribute.js"
        );
        assert_eq!(source.line_number, 42);
        assert!(source.column_number > 0);
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
    fn dynamic_script_request_reports_insertion_source_position() {
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_csp_policy(&["script-src 'none'; report-to observations".into()]);
        runtime.eval("globalThis.dynamicSourceEvents=[];globalThis.dynamicSourceReports=[];\
            document.addEventListener('securitypolicyviolation',event=>dynamicSourceEvents.push(event));\
            new ReportingObserver(batch=>dynamicSourceReports.push(...batch),{types:['csp-violation']}).observe();").unwrap();
        let (evaluated, ..) = runtime.eval_safe_timed_source(
            "const script=document.createElement('script');\nscript.src='https://blocked.test/script.js';\ndocument.body.appendChild(script);",
            Some("https://private.test/effective.js"),
            Some("https://example.test/requested.js#fragment"),
            (31, 5),
            false,
        );
        evaluated.unwrap();
        runtime.run_until_idle().unwrap();
        let observed = runtime.eval("JSON.stringify({events:dynamicSourceEvents.map(e=>[e.sourceFile,e.lineNumber,e.columnNumber]),reports:dynamicSourceReports.map(r=>[r.body.sourceFile,r.body.lineNumber,r.body.columnNumber])})").unwrap().as_string().unwrap().to_std_string_escaped();
        let observed: serde_json::Value = serde_json::from_str(&observed).unwrap();
        assert_eq!(
            observed["events"].as_array().map(Vec::len),
            Some(1),
            "{observed}"
        );
        assert_eq!(observed["reports"], observed["events"]);
        assert_eq!(
            observed["events"][0][0],
            "https://example.test/requested.js"
        );
        assert_eq!(observed["events"][0][1], 33);
        assert!(
            observed["events"][0][2]
                .as_u64()
                .is_some_and(|column| column > 0)
        );
    }

    #[test]
    fn moved_pending_script_uses_new_document_insertion_source() {
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_csp_policy(&["script-src 'none'; report-to observations".into()]);
        runtime.eval("globalThis.frame=document.createElement('iframe');frame.srcdoc='<body></body>';document.body.appendChild(frame);").unwrap();
        runtime.run_until_idle().unwrap();
        let (first, ..) = runtime.eval_safe_timed_source(
            "globalThis.movingScript=document.createElement('script');movingScript.src='https://blocked.test/script.js';document.body.appendChild(movingScript);",
            Some("https://example.test/first-effective.js"),
            Some("https://example.test/first-requested.js"),
            (12, 1),
            false,
        );
        first.unwrap();
        let (second, ..) = runtime.eval_safe_timed_source(
            "frame.contentDocument.body.appendChild(movingScript);",
            Some("https://example.test/second-effective.js"),
            Some("https://example.test/second-requested.js"),
            (25, 2),
            false,
        );
        second.unwrap();
        let (document_id, script_id) = {
            let state = runtime.host_state.borrow();
            let frame = state.document.query_selector("iframe").unwrap();
            let child = &state.iframe_documents[&frame.identity()].document;
            let script = child.query_selector("script").unwrap();
            let source = &state.pending_script_csp_sources[&script.identity()];
            assert_eq!(
                source.source_file,
                "https://example.test/second-requested.js"
            );
            assert_eq!(source.line_number, 25);
            (child.identity(), script.identity())
        };
        runtime.run_until_idle().unwrap();
        let state = runtime.host_state.borrow();
        assert!(!state.pending_script_csp_sources.contains_key(&script_id));
        assert_eq!(
            state
                .csp_violations
                .iter()
                .filter(|v| v.document_id == document_id)
                .count(),
            1
        );
    }

    #[test]
    fn adopted_script_csp_event_bubbles_in_destination_document() {
        let document = crate::html::TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_csp_policy(&["script-src 'none'; report-to observations".into()]);
        runtime.eval("globalThis.frame=document.createElement('iframe');frame.srcdoc='<body></body>';document.body.appendChild(frame);").unwrap();
        runtime.run_until_idle().unwrap();
        runtime.eval("globalThis.adoptedEvents=[];globalThis.adoptedReports=[];\
            const child=frame.contentDocument;\
            child.addEventListener('securitypolicyviolation',event=>adoptedEvents.push([event.target===adoptedScript,event.effectiveDirective]));\
            new frame.contentWindow.ReportingObserver(batch=>adoptedReports.push(...batch.map(report=>report.body.effectiveDirective))).observe();\
            globalThis.adoptedScript=document.createElement('script');\
            adoptedScript.src='https://blocked.test/script.js';\
            document.body.appendChild(adoptedScript);child.body.appendChild(adoptedScript);").unwrap();
        runtime.run_until_idle().unwrap();
        let observed = runtime.eval("JSON.stringify([adoptedEvents,adoptedReports,frame.contentDocument.cspViolations.length])").unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(
            observed,
            r#"[[[true,"script-src-elem"]],["script-src-elem"],1]"#
        );
    }

    #[test]
    fn legacy_report_uri_matches_dynamic_script_event_source() {
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
        runtime.install_csp_policy(&["script-src 'none'; report-uri /reports".into()]);
        let sent = Arc::new(Mutex::new(Vec::new()));
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .set_transport(Some(Arc::new(Transport(Arc::clone(&sent)))));
        runtime.eval("globalThis.legacySourceEvent=null;document.addEventListener('securitypolicyviolation',event=>legacySourceEvent=event);").unwrap();
        let (evaluated, ..) = runtime.eval_safe_timed_source(
            "const script=document.createElement('script');script.src='https://blocked.test/script.js';document.body.appendChild(script);",
            Some("https://private.test/effective.js"),
            Some("https://example.test/requested.js#fragment"),
            (59, 3),
            false,
        );
        evaluated.unwrap();
        runtime.run_until_idle().unwrap();
        wait_for_report_delivery(&mut runtime);
        let event = runtime.eval("JSON.stringify([legacySourceEvent.sourceFile,legacySourceEvent.lineNumber,legacySourceEvent.columnNumber])").unwrap().as_string().unwrap().to_std_string_escaped();
        let event: serde_json::Value = serde_json::from_str(&event).unwrap();
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let report = &sent[0]["csp-report"];
        assert_eq!(event[0], "https://example.test/requested.js");
        assert_eq!(event[1], 59);
        assert_eq!(report["source-file"], event[0]);
        assert_eq!(report["line-number"], event[1]);
        assert_eq!(report["column-number"], event[2]);
        assert!(!report.to_string().contains("private.test"));
        assert!(!report.to_string().contains("fragment"));
    }

    #[test]
    fn inline_handler_violation_reports_author_attribute_call_position() {
        let document = crate::html::TreeBuilder::parse(
            "<html><body><button id='target'>go</button></body></html>",
        )
        .document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_csp_policy(&["script-src-attr 'none'; report-to observations".into()]);
        runtime.eval("globalThis.handlerEvents=[];globalThis.handlerReports=[];\
            document.addEventListener('securitypolicyviolation',event=>handlerEvents.push(event));\
            new ReportingObserver(batch=>handlerReports.push(...batch),{types:['csp-violation']}).observe();").unwrap();
        let (evaluated, ..) = runtime.eval_safe_timed_source(
            "document.getElementById('target').setAttribute('onclick', 'globalThis.clicked=true');",
            Some("https://private.test/effective.js"),
            Some("https://example.test/requested.js#fragment"),
            (47, 2),
            false,
        );
        evaluated.unwrap();
        runtime.run_until_idle().unwrap();
        let observed = runtime.eval("JSON.stringify({events:handlerEvents.map(e=>[e.sourceFile,e.lineNumber,e.columnNumber]),reports:handlerReports.map(r=>[r.body.sourceFile,r.body.lineNumber,r.body.columnNumber])})").unwrap().as_string().unwrap().to_std_string_escaped();
        let observed: serde_json::Value = serde_json::from_str(&observed).unwrap();
        assert_eq!(
            observed["events"].as_array().map(Vec::len),
            Some(1),
            "{observed}"
        );
        assert_eq!(observed["reports"], observed["events"]);
        assert_eq!(
            observed["events"][0][0],
            "https://example.test/requested.js"
        );
        assert_eq!(observed["events"][0][1], 47);
        assert!(
            observed["events"][0][2]
                .as_u64()
                .is_some_and(|column| column > 0)
        );
    }

    #[test]
    fn module_connect_violation_reports_module_source_position() {
        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
        runtime.install_csp_policy(&["connect-src 'none'; report-to observations".into()]);
        runtime
            .eval(
                "globalThis.moduleSourceEvents=[];globalThis.moduleSourceReports=[];\
                 document.addEventListener('securitypolicyviolation',event=>\
                   moduleSourceEvents.push(event));\
                 new ReportingObserver(batch=>moduleSourceReports.push(...batch),{\
                   types:['csp-violation']\
                 }).observe();",
            )
            .unwrap();
        let (evaluated, ..) = runtime.eval_module_timed(
            "const marker = 1;\nfetch('https://blocked.test/module').catch(()=>{});",
            "https://example.test/module.js",
            runtime.document(),
            crate::http::cors::CredentialsMode::SameOrigin,
        );
        assert!(evaluated.is_ok(), "module evaluation: {evaluated:?}");
        runtime.run_until_idle().unwrap();
        let observed = runtime
            .eval(
                "JSON.stringify({events:moduleSourceEvents.map(event=>[event.sourceFile,event.lineNumber,event.columnNumber]),\
                 reports:moduleSourceReports.map(report=>[report.body.sourceFile,report.body.lineNumber,report.body.columnNumber])})",
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        let observed: serde_json::Value = serde_json::from_str(&observed).unwrap();
        assert_eq!(
            observed["events"].as_array().map(Vec::len),
            Some(1),
            "{observed}"
        );
        assert_eq!(observed["reports"], observed["events"]);
        assert_eq!(observed["events"][0][0], "https://example.test/module.js");
        assert_eq!(observed["events"][0][1], 2);
        assert!(
            observed["events"][0][2]
                .as_u64()
                .is_some_and(|column| column > 0)
        );
    }

    #[test]
    fn imported_module_connect_violation_uses_imported_runnable() {
        use crate::test_support::http_fixture::{
            ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
            read_request_headers,
        };
        use std::io::Write;

        let listener = bind_loopback().unwrap();
        let address = listener.local_addr().unwrap();
        let server = FixtureWorker::spawn(move || {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            assert!(request.starts_with("GET /dep.js HTTP/1.1"), "{request}");
            stream
                .write_all(b"HTTP/1.1 302 Found\r\nLocation: /private.js?secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            drop(stream);
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            assert!(
                request.starts_with("GET /private.js?secret HTTP/1.1"),
                "{request}"
            );
            let body = b"fetch('http://blocked.test/from-dep').catch(()=>{});";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });

        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, &format!("http://{address}/document"))
                .unwrap();
        runtime.install_csp_policy(&["connect-src 'none'; report-to observations".into()]);
        runtime
            .eval(
                "globalThis.importEvents=[];globalThis.importReports=[];\
                 document.addEventListener('securitypolicyviolation',event=>\
                   importEvents.push(event));\
                 new ReportingObserver(batch=>importReports.push(...batch),{\
                   types:['csp-violation']\
                 }).observe();",
            )
            .unwrap();
        let (evaluated, ..) = runtime.eval_module_timed(
            "import './dep.js';",
            &format!("http://{address}/root.js"),
            runtime.document(),
            crate::http::cors::CredentialsMode::SameOrigin,
        );
        server.join();
        assert!(evaluated.is_ok(), "module evaluation: {evaluated:?}");
        runtime.run_until_idle().unwrap();
        let observed = runtime
            .eval(
                "JSON.stringify({events:importEvents.map(event=>[event.sourceFile,event.lineNumber,event.columnNumber]),\
                 reports:importReports.map(report=>[report.body.sourceFile,report.body.lineNumber,report.body.columnNumber])})",
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        let observed: serde_json::Value = serde_json::from_str(&observed).unwrap();
        assert_eq!(
            observed["events"].as_array().map(Vec::len),
            Some(1),
            "{observed}"
        );
        assert_eq!(observed["reports"], observed["events"]);
        assert_eq!(observed["events"][0][0], format!("http://{address}/dep.js"));
        assert!(!observed.to_string().contains("private.js"));
        assert!(!observed.to_string().contains("secret"));
        assert_eq!(observed["events"][0][1], 1);
        assert!(
            observed["events"][0][2]
                .as_u64()
                .is_some_and(|column| column > 0)
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

        let document = crate::html::TreeBuilder::parse("<body>").document();
        let runtime = JsRuntime::with_document_and_url(document, "https://example.test/").unwrap();
        let document_id = runtime.document().identity();
        let mut state = runtime.host_state.borrow_mut();
        state
            .document_reporting
            .get_mut(&document_id)
            .unwrap()
            .creation_url = "not a URL with a secret".into();
        assert_eq!(state.report_url_for_document(document_id), "");
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
