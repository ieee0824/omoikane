//! Document-owned image loading and deferred security notifications.
use super::*;

#[cfg(test)]
mod tests;

pub(super) struct PendingImageEvent {
    node_id: usize,
    document_id: usize,
    source: String,
    dependencies: Vec<String>,
    started: Instant,
}

pub(super) struct DocumentImageEnvironment {
    document_id: usize,
    cache: Rc<RefCell<crate::layout::ImageDocumentState>>,
    policy: CspPolicy,
    client: Client,
    cookies: Arc<Mutex<crate::http::CookieJar>>,
    cookie_context: crate::http::CookieRequestContext,
    origin: crate::http::cors::Origin,
    embedder: crate::http::corp::EmbedderPolicyValue,
    report_only_embedder: crate::http::corp::EmbedderPolicyValue,
}

#[derive(Default)]
pub(super) struct ImageReports {
    coep: Vec<crate::http::cors::CorpViolation>,
    csp: Vec<crate::layout::ImagePolicyNotice>,
}

impl DocumentImageEnvironment {
    pub(super) fn run<T>(
        self,
        report_cached: bool,
        operation: impl FnOnce() -> T,
    ) -> (T, ImageReports) {
        let ((result, coep), csp) = crate::layout::with_image_security_policy(
            self.cache,
            self.policy,
            self.client,
            report_cached,
            || {
                crate::layout::with_image_cookie_store_reporting(
                    self.cookies,
                    self.cookie_context,
                    self.document_id,
                    self.origin,
                    self.embedder,
                    self.report_only_embedder,
                    operation,
                )
            },
        );
        (result, ImageReports { coep, csp })
    }
}

impl HostState {
    /// Drops pending downloads without waiting for in-flight transport I/O.
    pub(super) fn cancel_image_loads(&mut self) {
        self.image_resources.clear();
        self.pending_image_events.clear();
    }

    /// Cancels queued image work and notifications when a Document retires.
    pub(super) fn retire_document_images(&mut self, document_id: usize) {
        self.image_resources.remove(&document_id);
        self.pending_image_events
            .retain(|event| event.document_id != document_id);
    }

    pub(super) fn image_environment(&mut self, document_id: usize) -> DocumentImageEnvironment {
        let embedder = self.isolation_policy_for_document(document_id).embedder;
        let cache = Rc::clone(self.image_resources.entry(document_id).or_default());
        cache
            .borrow_mut()
            .set_async_loading(self.nonblocking_image_loading);
        DocumentImageEnvironment {
            document_id,
            cache,
            policy: self
                .document_csp
                .get(&document_id)
                .cloned()
                .unwrap_or_default(),
            client: self.http_client.fork_for_worker(),
            cookies: Arc::clone(&self.cookie_store),
            cookie_context: self.document_cookie_request_context(document_id),
            origin: script_errors::document_fetch_origin(self, document_id),
            embedder: embedder.value.into(),
            report_only_embedder: embedder.report_only_value.into(),
        }
    }

    pub(super) fn queue_image_reports(&mut self, document_id: usize, reports: ImageReports) {
        if !self.document_is_active(document_id) {
            return;
        }
        self.queue_coep_corp_violations(document_id, "image", reports.coep);
        for report in reports.csp {
            self.queue_policy_violation_events_with_source(
                document_id,
                ResourceType::Image,
                &report.reference,
                report.report_only,
                "report",
                None,
            );
            self.queue_policy_violation_events_with_source(
                document_id,
                ResourceType::Image,
                &report.reference,
                report.enforced,
                "enforce",
                None,
            );
        }
    }
}

impl JsRuntime {
    /// Keeps image network/decode work off an interactive embedder's thread.
    pub fn set_nonblocking_image_loading(&mut self, enabled: bool) {
        self.host_state.borrow_mut().nonblocking_image_loading = enabled;
    }

    pub(super) fn poll_image_resource_loads(&mut self) -> JsResult<()> {
        let documents: Vec<_> = self
            .host_state
            .borrow()
            .image_resources
            .iter()
            .filter_map(|(id, images)| images.borrow().has_pending_images().then_some(*id))
            .collect();
        for document_id in documents {
            let environment = {
                let mut state = self.host_state.borrow_mut();
                if !state.document_is_active(document_id) {
                    continue;
                }
                state.image_environment(document_id)
            };
            let (changed, reports) = environment.run(false, crate::layout::poll_async_images);
            let mut state = self.host_state.borrow_mut();
            state.queue_image_reports(document_id, reports);
            if changed && let Some(document) = state.get_node(document_id) {
                state.mark_document_style_dirty(&document);
            }
        }
        let waiting = std::mem::take(&mut self.host_state.borrow_mut().pending_image_events);
        for pending in waiting {
            let current = {
                let state = self.host_state.borrow();
                state.document_is_active(pending.document_id)
                    && state.get_node(pending.node_id).is_some_and(|node| {
                        node.get_attribute("src").unwrap_or_default() == pending.source
                            && document_root_for_node(&node)
                                .or_else(|| state.node_lifetime_owner(pending.node_id))
                                .is_some_and(|doc| doc.identity() == pending.document_id)
                    })
            };
            if current {
                let still_pending = self
                    .host_state
                    .borrow()
                    .image_resources
                    .get(&pending.document_id)
                    .is_some_and(|images| {
                        let images = images.borrow();
                        pending
                            .dependencies
                            .iter()
                            .any(|url| images.is_image_pending(url))
                    });
                if still_pending {
                    self.host_state
                        .borrow_mut()
                        .pending_image_events
                        .push(pending);
                } else {
                    self.run_image_resource_load_started(pending.node_id, pending.started, false)?;
                }
            }
        }
        Ok(())
    }

    /// Reuses this document's decoded assets and security policy during rendering.
    pub(crate) fn with_document_images<T>(
        &mut self,
        document_id: usize,
        operation: impl FnOnce() -> T,
    ) -> T {
        let environment = self.host_state.borrow_mut().image_environment(document_id);
        let (result, reports) = environment.run(false, operation);
        self.host_state
            .borrow_mut()
            .queue_image_reports(document_id, reports);
        result
    }

    pub(super) fn run_image_resource_load(&mut self, node_id: usize) -> JsResult<bool> {
        self.run_image_resource_load_started(node_id, Instant::now(), true)
    }

    fn run_image_resource_load_started(
        &mut self,
        node_id: usize,
        start: Instant,
        report_cached: bool,
    ) -> JsResult<bool> {
        let (node, source, base, environment, document_id) = {
            let mut state = self.host_state.borrow_mut();
            let Some(node) = state.get_node(node_id) else {
                return Ok(false);
            };
            if !node.has_tag_name("img") {
                return Ok(false);
            }
            state.pending_resource_loads.remove(&node_id);
            let Some(document) =
                document_root_for_node(&node).or_else(|| state.node_lifetime_owner(node_id))
            else {
                return Ok(true);
            };
            if !state.document_is_active(document.identity()) {
                return Ok(true);
            }
            let document_id = document.identity();
            let source = node.get_attribute("src").unwrap_or_default();
            let base = crate::paint::stylesheet::extract_document_base_url(
                &document,
                state.base_url_for_document(document_id).as_ref(),
            );
            let environment = state.image_environment(document_id);
            (node, source, base, environment, document_id)
        };
        let ((image, dependencies), reports) = environment.run(report_cached, || {
            crate::layout::decode_or_fetch_image_asset_with_pending_dependencies(
                &source,
                base.as_ref(),
            )
        });
        {
            let mut state = self.host_state.borrow_mut();
            state.queue_image_reports(document_id, reports);
            if !state.document_is_active(document_id)
                || node.get_attribute("src").unwrap_or_default() != source
            {
                return Ok(true);
            }
            if !dependencies.is_empty() {
                if !state.pending_image_events.iter().any(|event| {
                    event.node_id == node_id
                        && event.document_id == document_id
                        && event.source == source
                }) {
                    state.pending_image_events.push(PendingImageEvent {
                        node_id,
                        document_id,
                        source,
                        dependencies,
                        started: start,
                    });
                }
                return Ok(true);
            }
        }
        self.host_state
            .borrow_mut()
            .event_loop
            .enqueue_timer_for_document(
                TimerPayload::ImageResourceEvent {
                    node_id,
                    document_id,
                    timing_name: resource_reference_timing_name(&source, base.as_ref()),
                    source,
                    loaded: image.is_some(),
                    elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                },
                Some(document_id),
            );
        Ok(true)
    }

    pub(super) fn dispatch_image_resource_event(
        &mut self,
        node_id: usize,
        document_id: usize,
        source: &str,
        timing_name: &str,
        loaded: bool,
        elapsed_ms: f64,
    ) -> JsResult<()> {
        {
            let state = self.host_state.borrow();
            let Some(node) = state.get_node(node_id) else {
                return Ok(());
            };
            if !state.document_is_active(document_id)
                || document_root_for_node(&node)
                    .or_else(|| state.node_lifetime_owner(node_id))
                    .is_none_or(|doc| doc.identity() != document_id)
                || node.get_attribute("src").unwrap_or_default() != source
            {
                return Ok(());
            }
        }

        self.realm_for_document(document_id)?;
        self.eval_in_document_realm(
            document_id,
            &dispatch_resource_timing_script(
                if loaded { "load" } else { "error" },
                node_id,
                timing_name,
                false,
                elapsed_ms,
            ),
        )
    }
}
