//! Owned dynamic-script fetches on the common resource workers.
use super::*;
use crate::resource_worker::{
    ResourceKind, ResourceOutcome, ResourcePriority, ResourceScope, ResourceTask,
};
use std::collections::VecDeque;

#[cfg(test)]
mod tests;

type FetchedSource = (String, String, usize, bool);

pub(super) struct Response {
    pub fetched: Option<FetchedSource>,
    pub elapsed_ms: f64,
    pub nonce: Option<String>,
    violations: Vec<crate::http::cors::CorpViolation>,
}

pub(super) struct Prepared {
    pub script: DynamicScriptInfo,
    pub dispatch_document: usize,
    pub dispatch_load: bool,
    pub source: Option<security_reporting::CspSourceLocation>,
}

struct Pending {
    prepared: Prepared,
    nonce: Option<String>,
    _scope: ResourceScope,
    task: Option<ResourceTask<Response>>,
    ready: Option<Response>,
    queued: bool,
}

/// Only the owner keeps DOM handles; worker inputs and outputs own their data.
#[derive(Default)]
pub(super) struct ScriptLoads {
    pending: HashMap<usize, Pending>,
    ordered: HashMap<usize, VecDeque<usize>>,
}

struct Request {
    nonce: Option<String>,
    reference: String,
    base: Option<crate::http::Url>,
    crossorigin: Option<String>,
    origin: CorsOrigin,
    cookies: crate::http::CookieRequestContext,
    embedder: crate::http::corp::EmbedderPolicyValue,
    report_only: crate::http::corp::EmbedderPolicyValue,
    client: Client,
}

impl Request {
    fn run(mut self) -> Response {
        let started = Instant::now();
        let mut violations = Vec::new();
        let fetched = script_errors::fetch_classic_source(
            &self.reference,
            self.base.as_ref(),
            &self.origin,
            self.crossorigin.as_deref(),
            Some(&self.cookies),
            self.embedder,
            self.report_only,
            &mut self.client,
            &mut violations,
        );
        Response {
            fetched,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            nonce: self.nonce,
            violations,
        }
    }
}

impl ScriptLoads {
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn remove_document(&mut self, document: usize) {
        self.pending
            .retain(|_, load| load.prepared.script.4 != document);
        self.ordered.remove(&document);
    }
}

impl HostState {
    pub(super) fn prepare_inserted_external_script(
        &mut self,
        node_id: usize,
    ) -> Option<DynamicScriptInfo> {
        self.pending_resource_loads.remove(&node_id);
        self.get_node(node_id).and_then(|node| {
            if !self.node_is_in_active_document(&node) {
                return None;
            }
            let document_id = document_root_for_node(&node)?.identity();
            let src = node.get_attribute("src")?;
            if !self.begin_inserted_external_script(&node) {
                return None;
            }
            Some((
                node.clone(),
                src,
                ScriptKind::from_type_attribute(node.get_attribute("type").as_deref()),
                self.base_url_for_document(document_id),
                document_id,
            ))
        })
    }

    pub(super) fn fetch_dynamic_script(
        &mut self,
        document: usize,
        node: &NodeHandle,
        src: &str,
        kind: ScriptKind,
        base: Option<&crate::http::Url>,
    ) -> Response {
        let started = Instant::now();
        let nonce = node.get_attribute("nonce");
        let crossorigin = node.get_attribute("crossorigin");
        let crossorigin = if kind == ScriptKind::Module {
            Some(crossorigin.as_deref().unwrap_or(""))
        } else {
            crossorigin.as_deref()
        };
        let fetched = self.fetch_classic_source_for_document(document, src, base, crossorigin);
        Response {
            fetched,
            nonce,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            violations: Vec::new(),
        }
    }

    /// Called only after owner-thread sandbox and request CSP checks.
    pub(super) fn queue_dynamic_script(&mut self, prepared: Prepared) -> std::io::Result<()> {
        let (node, reference, kind, base, document) = &prepared.script;
        let node_id = node.identity();
        let document = *document;
        let crossorigin = if *kind == ScriptKind::Module {
            Some(node.get_attribute("crossorigin").unwrap_or_default())
        } else {
            node.get_attribute("crossorigin")
        };
        let policy = self.isolation_policy_for_document(document).embedder;
        let mut client = self.http_client.fork_for_worker();
        client.set_shared_cookie_store(Arc::clone(&self.cookie_store));
        let nonce = node.get_attribute("nonce");
        let request = Request {
            nonce: nonce.clone(),
            reference: reference.clone(),
            base: base.clone(),
            crossorigin,
            origin: script_errors::document_fetch_origin(self, document),
            cookies: self.document_cookie_request_context(document),
            embedder: policy.value.into(),
            report_only: policy.report_only_value.into(),
            client,
        };
        let scope = ResourceScope::default();
        let task = self.resource_queue.submit(
            &scope,
            ResourcePriority::Critical,
            ResourceKind::Script,
            move |_| request.run(),
        )?;
        if !node.script_is_async() {
            self.script_loads
                .ordered
                .entry(document)
                .or_default()
                .push_back(node_id);
        }
        self.script_loads.pending.insert(
            node_id,
            Pending {
                nonce,
                prepared,
                _scope: scope,
                task: Some(task),
                ready: None,
                queued: false,
            },
        );
        Ok(())
    }

    pub(super) fn poll_script_loads(&mut self) {
        let mut loads = std::mem::take(&mut self.script_loads);
        loads
            .pending
            .retain(|_, load| self.document_is_active(load.prepared.script.4));
        for load in loads.pending.values_mut() {
            let Some(task) = &load.task else { continue };
            let response = match task.try_recv() {
                Ok(ResourceOutcome::Complete(response)) => response,
                Err(std::sync::mpsc::TryRecvError::Empty) => continue,
                _ => Response {
                    nonce: load.nonce.clone(),
                    fetched: None,
                    elapsed_ms: 0.0,
                    violations: Vec::new(),
                },
            };
            load.task = None;
            load.ready = Some(response);
        }
        let mut ordered = HashSet::new();
        let mut runnable = Vec::new();
        for order in loads.ordered.values_mut() {
            order.retain(|id| loads.pending.contains_key(id));
            ordered.extend(order.iter().copied());
            while let Some(id) = order.front().copied() {
                if loads.pending[&id].ready.is_none() {
                    break;
                }
                runnable.push(id);
                order.pop_front();
            }
        }
        runnable.extend(loads.pending.iter().filter_map(|(id, load)| {
            (load.ready.is_some() && !ordered.contains(id)).then_some(*id)
        }));
        for id in runnable {
            let load = loads.pending.get_mut(&id).expect("pending script exists");
            if !load.queued {
                load.queued = true;
                self.event_loop.enqueue_timer_for_document(
                    TimerPayload::ScriptResourceReady { node_id: id },
                    Some(load.prepared.dispatch_document),
                );
            }
        }
        self.script_loads = loads;
    }

    pub(super) fn take_ready_script(&mut self, id: usize) -> Option<(Prepared, Response)> {
        if !self.script_loads.pending.get(&id)?.queued {
            return None;
        }
        let mut load = self.script_loads.pending.remove(&id)?;
        let (node, _, _, _, document) = &load.prepared.script;
        let owner = document_root_for_node(node).or_else(|| self.node_lifetime_owner(id));
        if !self.document_is_active(*document)
            || owner.is_none_or(|owner| owner.identity() != *document)
        {
            return None;
        }
        let mut response = load.ready.take()?;
        self.queue_coep_corp_violations(
            load.prepared.script.4,
            "script",
            std::mem::take(&mut response.violations),
        );
        Some((load.prepared, response))
    }
}

impl JsRuntime {
    pub(super) fn defer_dynamic_script(&mut self, prepared: Prepared) {
        let document = prepared.dispatch_document;
        let node = prepared.script.0.identity();
        let src = prepared.script.1.clone();
        let timing_name = resource_reference_timing_name(&src, prepared.script.3.as_ref());
        let queued = self.host_state.borrow_mut().queue_dynamic_script(prepared);
        if let Err(error) = queued {
            self.record_task_error(format!("[dynamic script: {src}] failed to queue: {error}"));
            let result = self.eval_in_document_realm(
                document,
                &dispatch_resource_timing_script("error", node, &timing_name, false, 0.0),
            );
            self.record_error_from(&src, result);
        }
    }
}

pub(super) fn trace_fetch(elapsed_ms: f64, kind: ScriptKind) {
    if std::env::var_os("OMOIKANE_TRACE_TASKS").is_some() && elapsed_ms >= 50.0 {
        eprintln!(
            "OMOIKANE_SCRIPT_PHASE {}",
            serde_json::json!({
                "phase": "fetch",
                "elapsed_ms": elapsed_ms,
                "kind": format!("{kind:?}"),
            })
        );
    }
}

pub(super) struct Execution {
    pub script: DynamicScriptInfo,
    pub source: String,
    pub effective_url: String,
    pub requested_url: String,
    pub muted_errors: bool,
}

impl JsRuntime {
    pub(super) fn execute_dynamic_source(&mut self, execution: Execution, log_scripts: bool) {
        let Execution {
            script: (script_node, src, kind, base_url, dispatch_document_id),
            source,
            effective_url,
            requested_url: timing_name,
            muted_errors,
        } = execution;
        let execution_started = std::time::Instant::now();
        let result = match kind {
            ScriptKind::Module => {
                self.eval_module_in_document_realm_timed(
                    dispatch_document_id,
                    script_node.identity(),
                    &source,
                    &module_script_url(&src, base_url.as_ref(), false),
                    document_root_for_node(&script_node).unwrap_or_else(|| self.document()),
                )
                .0
            }
            _ => self
                .eval_script_in_document_realm(
                    dispatch_document_id,
                    script_node.identity(),
                    &source,
                    &effective_url,
                    &timing_name,
                    muted_errors,
                    (1, 1),
                    true,
                )
                .map(|_| JsValue::undefined())
                .map_err(JsEvaluationError::JavaScript),
        };
        let execution_ms = execution_started.elapsed().as_secs_f64() * 1_000.0;
        if std::env::var_os("OMOIKANE_TRACE_TASKS").is_some() && execution_ms >= 50.0 {
            eprintln!(
                "OMOIKANE_SCRIPT_PHASE {}",
                serde_json::json!({
                    "phase": "execute",
                    "elapsed_ms": execution_ms,
                    "kind": format!("{kind:?}"),
                    "success": result.is_ok(),
                })
            );
        }
        if let Err(error) = result {
            let context = script_source_context(&source);
            self.record_task_error(format!("[dynamic script: {src}; {context}] {error}"));
        }
        if log_scripts {
            eprintln!("[omoikane][script] completed dynamic {src}");
        }
    }

    pub(super) async fn execute_dynamic_source_async(
        &mut self,
        execution: Execution,
        redirected: bool,
        elapsed_ms: f64,
    ) -> JsResult<()> {
        let Execution {
            script: (script_node, src, kind, base_url, document_id),
            source,
            effective_url,
            requested_url: timing_name,
            muted_errors,
        } = execution;
        let node_id = script_node.identity();
        let marked = self
            .eval_async_for_document(
                &format!("__omoikane_set_current_script({node_id})"),
                document_id,
            )
            .await;
        if let Err(error) = marked {
            if is_wall_clock_timeout(&error) {
                return Err(error);
            }
            self.record_error_from::<()>(&src, Err(error));
        }
        let result = match kind {
            ScriptKind::Module => {
                let module_document =
                    document_root_for_node(&script_node).unwrap_or_else(|| self.document());
                self.eval_module_async(
                    &source,
                    &module_script_url(&src, base_url.as_ref(), false),
                    module_document,
                    module_script_credentials(&script_node),
                )
                .await
            }
            _ => {
                let host_state = Rc::clone(&self.host_state);
                ActiveDocumentFuture {
                    future: Box::pin(self.eval_async_source(
                        &source,
                        Some(&effective_url),
                        Some(&timing_name),
                        muted_errors,
                        (1, 1),
                    )),
                    host_state,
                    document_id,
                }
                .await
            }
        };
        let _ = self.eval("__omoikane_set_current_script(null)");
        if let Err(error) = result {
            if is_wall_clock_timeout(&error) {
                return Err(error);
            }
            if kind != ScriptKind::Module {
                let _ = self.with_active_host(|context| {
                    script_errors::report_exception_with_muting(
                        context,
                        &error,
                        document_id,
                        muted_errors,
                    )
                });
            }
            let context = script_source_context(&source);
            self.record_task_error(format!("[dynamic script: {src}; {context}] {error}"));
        }
        let dispatched = self
            .eval_async_for_document(
                &dispatch_resource_timing_script(
                    "load",
                    node_id,
                    &effective_url,
                    redirected,
                    elapsed_ms,
                ),
                document_id,
            )
            .await;
        if let Err(error) = dispatched {
            if is_wall_clock_timeout(&error) {
                return Err(error);
            }
            self.record_error_from::<()>("resource load", Err(error));
        }
        self.sync_module_csp_violations();
        Ok(())
    }
}

pub(super) fn is_async_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    with_host_state(|state| {
        Ok(state
            .borrow()
            .get_node(id)
            .is_some_and(|node| node.script_is_async())
            .into())
    })
}

pub(super) fn clear_force_async_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    with_host_state(|state| {
        if let Some(node) = state.borrow().get_node(id) {
            node.set_script_force_async(false);
        }
        Ok(JsValue::undefined())
    })
}
