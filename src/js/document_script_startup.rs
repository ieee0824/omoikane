//! Owned parser-script progress; only resource requests leave the page owner.
use super::*;
use crate::resource_worker::{ResourceOutcome, ResourceScope, ResourceTask};

type DeferredScript = (
    String,
    NodeHandle,
    String,
    Option<String>,
    bool,
    Option<String>,
);
type FetchedSource = (String, String, usize, bool);

/// Whether a host opportunity may wait for a script's resource response.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentScriptWait {
    /// Interactive hosts poll and retain progress while resource work runs.
    Poll,
    /// Synchronous embedding and static rendering wait without busy polling.
    Block,
}

/// Parser progress and pending resource lifetime owned by one installed document.
pub(crate) struct DocumentScriptStartup {
    document: NodeHandle,
    base: Option<crate::http::Url>,
    scripts: Vec<NodeHandle>,
    parser_policies: Vec<CspPolicy>,
    full_policy: CspPolicy,
    next: usize,
    deferred: VecDeque<DeferredSource>,
    pending: Option<PendingSource>,
    errors: Vec<String>,
    log_scripts: bool,
    finished: bool,
}

impl std::fmt::Debug for DocumentScriptStartup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DocumentScriptStartup")
            .field("document", &self.document.identity())
            .field("next", &self.next)
            .field("scripts", &self.scripts.len())
            .field("pending", &self.pending.is_some())
            .field("finished", &self.finished)
            .finish()
    }
}

impl DocumentScriptStartup {
    /// Drains recorded startup errors after the host observes completion.
    pub(crate) fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }
}

struct SourcePlan {
    node: NodeHandle,
    document: usize,
    source: InitialSource,
    index: usize,
    is_module: bool,
    is_defer: bool,
    nonce: Option<String>,
    crossorigin: Option<String>,
    requested_url: Option<String>,
}

enum InitialSource {
    Inline(String),
    External(String),
}

struct PendingSource {
    plan: SourcePlan,
    _scope: ResourceScope,
    task: ResourceTask<script_loading::Response>,
}

enum DeferredSource {
    Ready(DeferredScript),
    Fetching(PendingSource),
}

struct SourceResult {
    fetched: Option<FetchedSource>,
    elapsed_ms: f64,
}

struct ReadyScript {
    plan: SourcePlan,
    code: String,
    label: String,
    muted_errors: bool,
}

impl JsRuntime {
    /// Begins parser startup without fetching or executing external resources.
    pub(crate) fn begin_document_scripts(
        &mut self,
        base: Option<crate::http::Url>,
    ) -> DocumentScriptStartup {
        if let Some(base) = &base {
            self.host_state.borrow_mut().set_main_base_url(base.clone());
        }
        for source in [
            "__omoikane_install_window_named_properties()",
            "document.__readyState = 'loading'",
        ] {
            if self.eval(source).is_err() {
                self.record_document_script_failure("DOCUMENT_SCRIPT_INITIALIZATION_FAILED");
            }
        }
        let document = self.document();
        let scripts = collect_script_elements(&document);
        let full_policy = self.host_state.borrow().csp_policy_for_document(&document);
        let parser_policies = scripts
            .iter()
            .map(|node| full_policy.for_parser_script(&document, node))
            .collect();
        DocumentScriptStartup {
            document,
            base,
            scripts,
            parser_policies,
            full_policy,
            next: 0,
            deferred: VecDeque::new(),
            pending: None,
            errors: Vec::new(),
            log_scripts: std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some(),
            finished: false,
        }
    }

    /// Advances parser order until completion or an interactive resource wait.
    /// Script evaluation and response-policy application remain on this owner.
    pub(crate) fn poll_document_scripts(
        &mut self,
        startup: &mut DocumentScriptStartup,
        wait: DocumentScriptWait,
    ) -> bool {
        self.refresh_document_script_policy(startup);
        while !startup.finished {
            if let Some(pending) = startup.pending.take() {
                let response = match receive_source(&pending.task, wait) {
                    SourceReply::Waiting => {
                        startup.pending = Some(pending);
                        self.select_document_script_policy(startup, None);
                        return false;
                    }
                    SourceReply::Response(response) => Some(response),
                    SourceReply::Unavailable => None,
                };
                self.select_document_script_policy(startup, Some(pending.plan.index));
                if let Some(ready) =
                    self.resolve_pending_document_source(startup, pending.plan, response)
                {
                    self.run_ready_document_script(startup, ready);
                }
                continue;
            }
            let Some(node) = startup.scripts.get(startup.next).cloned() else {
                self.finish_document_script_sequence(startup, wait);
                break;
            };
            let index = startup.next;
            startup.next += 1;
            self.select_document_script_policy(startup, Some(index));
            let Some(plan) = self.prepare_initial_script_source(
                node,
                index,
                startup.base.as_ref(),
                startup.log_scripts,
            ) else {
                continue;
            };
            self.start_document_script_source(startup, plan, wait);
        }
        startup.finished
    }

    fn finish_document_script_sequence(
        &mut self,
        startup: &mut DocumentScriptStartup,
        wait: DocumentScriptWait,
    ) {
        self.select_document_script_policy(startup, None);
        while let Some(source) = startup.deferred.pop_front() {
            let source = match source {
                DeferredSource::Ready(source) => Some(source),
                DeferredSource::Fetching(pending) => {
                    let response = match receive_source(&pending.task, wait) {
                        SourceReply::Waiting => {
                            startup
                                .deferred
                                .push_front(DeferredSource::Fetching(pending));
                            return;
                        }
                        SourceReply::Response(response) => Some(response),
                        SourceReply::Unavailable => None,
                    };
                    self.select_document_script_policy(startup, Some(pending.plan.index));
                    let ready =
                        self.resolve_pending_document_source(startup, pending.plan, response);
                    self.select_document_script_policy(startup, None);
                    ready.and_then(|ready| {
                        deferred_script(ready, startup.base.as_ref(), startup.log_scripts)
                    })
                }
            };
            if let Some(source) = source {
                self.run_deferred_document_scripts(
                    vec![source],
                    startup.log_scripts,
                    &mut startup.errors,
                );
            }
        }
        self.sync_module_csp_violations();
        if let Err(error) = self.fire_dom_content_loaded() {
            self.record_document_script_failure("DOCUMENT_SCRIPT_INITIALIZATION_FAILED");
            startup.errors.push(format!("{error}"));
        }
        startup.finished = true;
    }

    fn run_ready_document_script(
        &mut self,
        startup: &mut DocumentScriptStartup,
        ready: ReadyScript,
    ) {
        if ready.plan.is_defer {
            if let Some(source) = deferred_script(ready, startup.base.as_ref(), startup.log_scripts)
            {
                startup.deferred.push_back(DeferredSource::Ready(source));
            }
            return;
        }
        if ready.code.trim().is_empty() {
            return;
        }
        log_ready_script(&ready, startup.log_scripts);
        self.run_document_script_now(
            &ready.plan.node,
            &ready.code,
            &ready.label,
            ready.plan.requested_url.as_deref(),
            ready.muted_errors,
            startup.log_scripts,
            &mut startup.errors,
        );
    }

    fn prepare_initial_script_source(
        &mut self,
        node: NodeHandle,
        index: usize,
        base: Option<&crate::http::Url>,
        log_scripts: bool,
    ) -> Option<SourcePlan> {
        let attrs = node.attributes().unwrap_or_default();
        let is_module = attrs
            .get("type")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("module"));
        if !is_module && !is_executable_classic_script_type(attrs.get("type").map(String::as_str)) {
            if log_scripts {
                eprintln!(
                    "[omoikane][script] skipped type={:?} src={:?}",
                    attrs.get("type"),
                    attrs.get("src")
                );
            }
            return None;
        }
        let reference = attrs.get("src").cloned();
        let requested_url = self.csp_requested_url_for_script(&node, reference.as_deref(), base);
        let nonce = attrs.get("nonce").cloned();
        let request = csp::ScriptRequest {
            nonce: nonce.as_deref(),
            parser_inserted: true,
        };
        let source = if let Some(reference) = reference {
            if !self
                .host_state
                .borrow_mut()
                .csp_allows_script_for_node(&node, &reference, request)
            {
                return None;
            }
            InitialSource::External(reference)
        } else {
            let source = collect_text_content(&node);
            if !self
                .host_state
                .borrow_mut()
                .csp_allows_inline_script_for_node(&node, &source)
            {
                return None;
            }
            InitialSource::Inline(source)
        };
        let is_defer = is_module
            || (attrs.contains_key("defer") && matches!(&source, InitialSource::External(_)));
        let crossorigin = if is_module {
            Some(attrs.get("crossorigin").cloned().unwrap_or_default())
        } else {
            attrs.get("crossorigin").cloned()
        };
        let document = document_root_for_node(&node)
            .map(|owner| owner.identity())
            .unwrap_or_else(|| self.document().identity());
        Some(SourcePlan {
            node,
            document,
            source,
            index,
            is_module,
            is_defer,
            nonce,
            crossorigin,
            requested_url,
        })
    }

    fn start_document_script_source(
        &mut self,
        startup: &mut DocumentScriptStartup,
        plan: SourcePlan,
        wait: DocumentScriptWait,
    ) {
        let InitialSource::External(reference) = &plan.source else {
            if let Some(ready) = self.complete_initial_script_source(
                plan,
                None,
                startup.base.as_ref(),
                startup.log_scripts,
                &mut startup.errors,
            ) {
                self.run_ready_document_script(startup, ready);
            }
            return;
        };
        if wait == DocumentScriptWait::Poll {
            let queued = self.host_state.borrow().queue_script_source_for_node(
                &plan.node,
                reference,
                if plan.is_module {
                    ScriptKind::Module
                } else {
                    ScriptKind::Classic
                },
                startup.base.clone(),
                plan.document,
            );
            if let Ok((scope, task)) = queued {
                let pending = PendingSource {
                    plan,
                    _scope: scope,
                    task,
                };
                if pending.plan.is_defer {
                    startup
                        .deferred
                        .push_back(DeferredSource::Fetching(pending));
                } else {
                    startup.pending = Some(pending);
                }
                return;
            }
            self.finish_initial_script_result(
                startup,
                plan,
                SourceResult {
                    fetched: None,
                    elapsed_ms: 0.0,
                },
            );
            return;
        }
        let result = self.fetch_initial_script_blocking(&plan, startup.base.as_ref());
        self.finish_initial_script_result(startup, plan, result);
    }

    fn fetch_initial_script_blocking(
        &mut self,
        plan: &SourcePlan,
        base: Option<&crate::http::Url>,
    ) -> SourceResult {
        let InitialSource::External(reference) = &plan.source else {
            unreachable!("only external scripts need source fetching");
        };
        let started = Instant::now();
        let fetched = self
            .host_state
            .borrow_mut()
            .fetch_classic_source_for_document(
                plan.document,
                reference,
                base,
                plan.crossorigin.as_deref(),
            );
        SourceResult {
            fetched,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        }
    }

    fn resolve_pending_document_source(
        &mut self,
        startup: &mut DocumentScriptStartup,
        plan: SourcePlan,
        response: Option<script_loading::Response>,
    ) -> Option<ReadyScript> {
        let owner_is_current = {
            let state = self.host_state.borrow();
            let owner = document_root_for_node(&plan.node)
                .or_else(|| state.node_lifetime_owner(plan.node.identity()));
            state.document_is_active(plan.document)
                && owner.is_some_and(|owner| owner.identity() == plan.document)
        };
        if !owner_is_current {
            return None;
        }
        let result = response.map_or(
            SourceResult {
                fetched: None,
                elapsed_ms: 0.0,
            },
            |mut response| {
                response.apply_violations(&mut self.host_state.borrow_mut(), plan.document);
                SourceResult {
                    fetched: response.fetched,
                    elapsed_ms: response.elapsed_ms,
                }
            },
        );
        self.complete_initial_script_source(
            plan,
            Some(result),
            startup.base.as_ref(),
            startup.log_scripts,
            &mut startup.errors,
        )
    }

    fn finish_initial_script_result(
        &mut self,
        startup: &mut DocumentScriptStartup,
        plan: SourcePlan,
        result: SourceResult,
    ) {
        if let Some(ready) = self.complete_initial_script_source(
            plan,
            Some(result),
            startup.base.as_ref(),
            startup.log_scripts,
            &mut startup.errors,
        ) {
            self.run_ready_document_script(startup, ready);
        }
    }

    fn complete_initial_script_source(
        &mut self,
        mut plan: SourcePlan,
        result: Option<SourceResult>,
        base: Option<&crate::http::Url>,
        log_scripts: bool,
        errors: &mut Vec<String>,
    ) -> Option<ReadyScript> {
        let (code, label, muted_errors) = match &mut plan.source {
            InitialSource::Inline(code) => (
                std::mem::take(code),
                format!("inline-script-{}", plan.index + 1),
                false,
            ),
            InitialSource::External(reference) => {
                let SourceResult {
                    fetched,
                    elapsed_ms,
                } = result.expect("external source has a response");
                let Some((effective_url, code, redirects, muted)) = fetched else {
                    let name = resource_reference_timing_name(reference, base);
                    self.record_initial_script_timing(&name, true, false, elapsed_ms);
                    errors.push(format!("failed to fetch script: {reference}"));
                    return None;
                };
                let request = csp::ScriptRequest {
                    nonce: plan.nonce.as_deref(),
                    parser_inserted: true,
                };
                if !self
                    .host_state
                    .borrow_mut()
                    .csp_allows_response_for_script_node(
                        &plan.node,
                        &effective_url,
                        redirects,
                        &resource_reference_timing_name(reference, base),
                        request,
                    )
                {
                    return None;
                }
                let redirected = resource_reference_was_redirected(reference, &effective_url, base);
                self.record_initial_script_timing(&effective_url, false, redirected, elapsed_ms);
                if log_scripts {
                    eprintln!("[omoikane][script] fetched {reference} elapsed_ms={elapsed_ms:.3}");
                }
                (code, effective_url, muted)
            }
        };
        Some(ReadyScript {
            plan,
            code,
            label,
            muted_errors,
        })
    }

    /// Resolves a source synchronously for owned page-task preparation.
    pub(super) fn fetch_or_inline_document_script_source(
        &mut self,
        node: &NodeHandle,
        index: usize,
        base: Option<&crate::http::Url>,
        log_scripts: bool,
        errors: &mut Vec<String>,
    ) -> Option<(bool, bool, bool, String, String, bool, Option<String>)> {
        let plan = self.prepare_initial_script_source(node.clone(), index, base, log_scripts)?;
        let has_src = matches!(&plan.source, InitialSource::External(_));
        let result = has_src.then(|| self.fetch_initial_script_blocking(&plan, base));
        let ready = self.complete_initial_script_source(plan, result, base, log_scripts, errors)?;
        Some((
            ready.plan.is_module,
            ready.plan.is_defer,
            has_src,
            ready.code,
            ready.label,
            ready.muted_errors,
            ready.plan.requested_url,
        ))
    }

    fn select_document_script_policy(
        &mut self,
        startup: &DocumentScriptStartup,
        index: Option<usize>,
    ) {
        let selected = index.map_or(&startup.full_policy, |index| {
            &startup.parser_policies[index]
        });
        if let Some(policy) = self
            .host_state
            .borrow_mut()
            .document_csp
            .get_mut(&startup.document.identity())
        {
            policy.use_parser_policies_from(selected);
        }
    }

    fn refresh_document_script_policy(&mut self, startup: &mut DocumentScriptStartup) {
        // Interactive waits restore the full policy before returning. Honor any
        // explicit response-policy change made between host opportunities.
        let current = {
            let state = self.host_state.borrow();
            match state.document_csp.get(&startup.document.identity()) {
                Some(current) if current.has_same_policy_rules(&startup.full_policy) => return,
                Some(current) => current.clone(),
                None => CspPolicy::default(),
            }
        };
        if !current.has_same_policy_rules(&startup.full_policy) {
            startup.parser_policies = startup
                .scripts
                .iter()
                .map(|node| current.for_parser_script(&startup.document, node))
                .collect();
            startup.full_policy = current;
        }
    }

    fn record_initial_script_timing(
        &mut self,
        name: &str,
        failed: bool,
        redirected: bool,
        elapsed_ms: f64,
    ) {
        let status = if failed { 0 } else { 200 };
        let _ = self.eval(&format!(
            "__omoikane_record_resource_timing({}, 'script', {status}, {failed}, {redirected}, {elapsed_ms})",
            serde_json::to_string(name).unwrap_or_else(|_| "\"\"".to_string()),
        ));
    }
}

fn log_ready_script(ready: &ReadyScript, enabled: bool) {
    if enabled {
        eprintln!(
            "[omoikane][script] queued {} ({} chars, defer={})",
            ready.label,
            ready.code.chars().count(),
            ready.plan.is_defer
        );
    }
}

fn deferred_script(
    ready: ReadyScript,
    base: Option<&crate::http::Url>,
    log: bool,
) -> Option<DeferredScript> {
    if ready.code.trim().is_empty() {
        return None;
    }
    log_ready_script(&ready, log);
    let has_src = matches!(&ready.plan.source, InitialSource::External(_));
    let module_url = ready
        .plan
        .is_module
        .then(|| module_script_url(&ready.label, base, !has_src));
    Some((
        ready.code,
        ready.plan.node,
        ready.label,
        module_url,
        ready.muted_errors,
        ready.plan.requested_url,
    ))
}

enum SourceReply {
    Waiting,
    Response(script_loading::Response),
    Unavailable,
}

fn receive_source(
    task: &ResourceTask<script_loading::Response>,
    wait: DocumentScriptWait,
) -> SourceReply {
    match wait {
        DocumentScriptWait::Block => match task.recv() {
            Ok(ResourceOutcome::Complete(response)) => SourceReply::Response(response),
            _ => SourceReply::Unavailable,
        },
        DocumentScriptWait::Poll => match task.try_recv() {
            Ok(ResourceOutcome::Complete(response)) => SourceReply::Response(response),
            Err(std::sync::mpsc::TryRecvError::Empty) => SourceReply::Waiting,
            _ => SourceReply::Unavailable,
        },
    }
}
