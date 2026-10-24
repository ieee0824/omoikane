//! Route child Window navigation to the owning iframe's history controller.

use super::*;
use crate::resource_worker::{
    ResourceKind, ResourceOutcome, ResourcePriority, ResourceScope, ResourceTask,
};
use std::sync::mpsc::TryRecvError;

#[derive(Default)]
pub(super) struct State {
    owners: HashMap<usize, JsValue>,
    loads: IframeNavigationLoads,
}

impl State {
    pub(super) fn owner(&self, document: usize) -> Option<JsValue> {
        self.owners.get(&document).cloned()
    }

    pub(super) unsafe fn trace(&self, tracer: &mut Tracer) {
        for callback in self.owners.values() {
            unsafe { callback.trace(tracer) };
        }
    }

    pub(super) fn retire_document(&mut self, document: usize) -> Vec<usize> {
        self.owners.remove(&document);
        self.loads.cancel_document(document)
    }

    pub(super) fn has_pending_loads(&self) -> bool {
        self.loads.has_pending()
    }

    pub(super) fn cancel_load(&mut self, iframe: usize) {
        self.loads.cancel(iframe);
    }

    pub(super) fn clear_loads(&mut self) {
        self.loads = IframeNavigationLoads::default();
    }
}

/// Scheduling class captured when navigation is requested.
///
/// Parser/DOM iframe discovery is ordinary resource work. A form submission,
/// Location navigation, reload, or history traversal is foreground work whose
/// response directly follows an explicit navigation action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IframeNavigationClass {
    Embedded,
    Foreground,
}

impl IframeNavigationClass {
    fn priority(self) -> ResourcePriority {
        match self {
            Self::Embedded => ResourcePriority::Normal,
            Self::Foreground => ResourcePriority::Critical,
        }
    }

    fn kind(self) -> ResourceKind {
        ResourceKind::Other
    }
}

#[derive(Clone)]
struct Reservation {
    epoch: u64,
    class: IframeNavigationClass,
    owner_document_id: usize,
    initiator_document_id: usize,
    restore: Option<form_state::PendingIframeHistoryRestore>,
    form_submission: bool,
}

struct ActiveNavigation {
    scope: ResourceScope,
    task: ResourceTask<NavigationWorkerResult>,
}

struct DrainingNavigation {
    class: IframeNavigationClass,
    task: ResourceTask<NavigationWorkerResult>,
}

struct PendingNavigation {
    epoch: u64,
    class: IframeNavigationClass,
    context_id: u64,
    owner_document_id: usize,
    initiator_document_id: usize,
    expected_document_id: usize,
    resource_attribute: &'static str,
    resource: String,
    resource_base: Option<crate::http::Url>,
    submission: Option<form_submission::Submission>,
    submission_source_document_id: Option<usize>,
    source: NavigationReferrerSource,
    storage_access_origin: Option<StorageOrigin>,
    restore: Option<form_state::PendingIframeHistoryRestore>,
    worker: Option<PreparedNavigationWorker>,
    active: Option<ActiveNavigation>,
    completion: Option<NavigationCompletion>,
    ready_task_queued: bool,
}

enum NavigationCompletion {
    Finished(NavigationWorkerResult),
    Cancelled,
    Panicked,
    Disconnected,
    AdmissionFailed(String),
}

struct PreparedNavigationWorker {
    transport: OwnedNavigationTransport,
    conversion: NavigationResponseConversion,
}

enum NavigationResponseConversion {
    Get {
        requested_reference: String,
        resolved_url: String,
    },
    Post {
        submission_url: String,
    },
}

struct NavigationWorkerResult {
    transport: NavigationTransportResult,
    conversion: NavigationResponseConversion,
    elapsed_ms: f64,
}

#[derive(Clone, Copy)]
struct ReadyNotice {
    iframe_id: usize,
    epoch: u64,
    owner_document_id: usize,
}

#[derive(Default)]
struct IframeNavigationLoads {
    next_epoch: u64,
    reservations: HashMap<usize, Reservation>,
    pending: HashMap<usize, PendingNavigation>,
    waiting: VecDeque<(usize, u64)>,
    draining: Vec<DrainingNavigation>,
}

impl IframeNavigationLoads {
    fn has_pending(&self) -> bool {
        !self.reservations.is_empty() || !self.pending.is_empty() || !self.draining.is_empty()
    }

    fn reserve(
        &mut self,
        iframe: usize,
        owner_document_id: usize,
        initiator_document_id: usize,
        class: IframeNavigationClass,
    ) -> Option<u64> {
        let epoch = self.next_epoch.checked_add(1)?;
        self.cancel(iframe);
        self.next_epoch = epoch;
        self.reservations.insert(
            iframe,
            Reservation {
                epoch,
                class,
                owner_document_id,
                initiator_document_id,
                restore: None,
                form_submission: false,
            },
        );
        Some(epoch)
    }

    fn current_epoch(&self, iframe: usize) -> Option<u64> {
        self.reservations
            .get(&iframe)
            .map(|reservation| reservation.epoch)
            .or_else(|| self.pending.get(&iframe).map(|pending| pending.epoch))
    }

    fn attach_restore(
        &mut self,
        iframe: usize,
        epoch: u64,
        restore: form_state::PendingIframeHistoryRestore,
    ) -> bool {
        if let Some(reservation) = self
            .reservations
            .get_mut(&iframe)
            .filter(|reservation| reservation.epoch == epoch)
        {
            reservation.restore = Some(restore);
            return true;
        }
        if let Some(pending) = self
            .pending
            .get_mut(&iframe)
            .filter(|pending| pending.epoch == epoch)
        {
            pending.restore = Some(restore);
            return true;
        }
        false
    }

    fn finish_local(
        &mut self,
        iframe: usize,
        epoch: u64,
    ) -> Option<Option<form_state::PendingIframeHistoryRestore>> {
        let current = self
            .reservations
            .get(&iframe)
            .is_some_and(|reservation| reservation.epoch == epoch);
        current
            .then(|| self.reservations.remove(&iframe))
            .flatten()
            .map(|reservation| reservation.restore)
    }

    fn cancel(&mut self, iframe: usize) {
        self.reservations.remove(&iframe);
        self.waiting.retain(|(id, _)| *id != iframe);
        let Some(mut pending) = self.pending.remove(&iframe) else {
            return;
        };
        if let Some(active) = pending.active.take() {
            active.scope.cancel();
            self.draining.push(DrainingNavigation {
                class: pending.class,
                task: active.task,
            });
        }
    }

    fn cancel_document(&mut self, document: usize) -> Vec<usize> {
        let mut frames = self
            .reservations
            .iter()
            .filter_map(|(iframe, reservation)| {
                (reservation.owner_document_id == document
                    || reservation.initiator_document_id == document)
                    .then_some(*iframe)
            })
            .collect::<Vec<_>>();
        frames.extend(self.pending.iter().filter_map(|(iframe, pending)| {
            (pending.owner_document_id == document || pending.initiator_document_id == document)
                .then_some(*iframe)
        }));
        frames.sort_unstable();
        frames.dedup();
        for iframe in frames.iter().copied() {
            self.cancel(iframe);
        }
        frames
    }

    fn begin(&mut self, iframe: usize, pending: PendingNavigation) -> bool {
        if self.current_epoch(iframe) != Some(pending.epoch) {
            return false;
        }
        self.reservations.remove(&iframe);
        self.waiting.push_back((iframe, pending.epoch));
        self.pending.insert(iframe, pending);
        true
    }

    fn active_counts(&self) -> (usize, usize) {
        let pending = self
            .pending
            .values()
            .filter_map(|pending| pending.active.as_ref().map(|_| pending.class));
        let draining = self.draining.iter().map(|pending| pending.class);
        let classes = pending.chain(draining).collect::<Vec<_>>();
        (
            classes.len(),
            classes
                .iter()
                .filter(|class| **class == IframeNavigationClass::Embedded)
                .count(),
        )
    }

    fn admission_limit(worker_count: usize) -> usize {
        worker_count.saturating_sub(1)
    }

    fn can_defer(worker_count: usize) -> bool {
        Self::admission_limit(worker_count) != 0
    }

    fn next_waiting(&self, embedded_active: usize) -> Option<(usize, u64)> {
        let foreground = self.waiting.iter().copied().find(|(iframe, epoch)| {
            self.pending.get(iframe).is_some_and(|pending| {
                pending.epoch == *epoch
                    && pending.worker.is_some()
                    && pending.class == IframeNavigationClass::Foreground
            })
        });
        if foreground.is_some() || embedded_active != 0 {
            return foreground;
        }
        self.waiting.iter().copied().find(|(iframe, epoch)| {
            self.pending.get(iframe).is_some_and(|pending| {
                pending.epoch == *epoch
                    && pending.worker.is_some()
                    && pending.class == IframeNavigationClass::Embedded
            })
        })
    }

    fn poll(
        &mut self,
        queue: &crate::resource_worker::ResourceQueue,
        client: &Client,
        cookie_store: &Arc<Mutex<crate::http::CookieJar>>,
    ) -> Vec<ReadyNotice> {
        self.draining
            .retain(|draining| matches!(draining.task.try_recv(), Err(TryRecvError::Empty)));

        let completed = self
            .pending
            .iter()
            .filter_map(|(iframe, pending)| {
                let active = pending.active.as_ref()?;
                match active.task.try_recv() {
                    Ok(outcome) => Some((*iframe, pending.epoch, Ok(outcome))),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => Some((*iframe, pending.epoch, Err(()))),
                }
            })
            .collect::<Vec<_>>();
        for (iframe, epoch, outcome) in completed {
            let Some(pending) = self
                .pending
                .get_mut(&iframe)
                .filter(|pending| pending.epoch == epoch)
            else {
                continue;
            };
            pending.active = None;
            pending.completion = Some(match outcome {
                Ok(ResourceOutcome::Complete(result)) => NavigationCompletion::Finished(result),
                Ok(ResourceOutcome::Cancelled) => NavigationCompletion::Cancelled,
                Ok(ResourceOutcome::Panicked) => NavigationCompletion::Panicked,
                Err(()) => NavigationCompletion::Disconnected,
            });
        }

        loop {
            let (active, embedded_active) = self.active_counts();
            if active >= Self::admission_limit(queue.worker_count().get()) {
                break;
            }
            let Some((iframe, epoch)) = self.next_waiting(embedded_active) else {
                break;
            };
            self.waiting
                .retain(|candidate| *candidate != (iframe, epoch));
            let Some(pending) = self
                .pending
                .get_mut(&iframe)
                .filter(|pending| pending.epoch == epoch)
            else {
                continue;
            };
            let Some(worker) = pending.worker.take() else {
                continue;
            };
            let mut worker_client = client.fork_for_worker();
            worker_client.set_shared_cookie_store(Arc::clone(cookie_store));
            let scope = ResourceScope::default();
            let task = queue.submit(
                &scope,
                pending.class.priority(),
                pending.class.kind(),
                move |token| {
                    let started = Instant::now();
                    let transport = worker
                        .transport
                        .run_while(&mut worker_client, || !token.is_cancelled());
                    NavigationWorkerResult {
                        transport,
                        conversion: worker.conversion,
                        elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
                    }
                },
            );
            match task {
                Ok(task) => pending.active = Some(ActiveNavigation { scope, task }),
                Err(error) => {
                    pending.completion =
                        Some(NavigationCompletion::AdmissionFailed(error.to_string()));
                }
            }
        }

        let mut notices = Vec::new();
        for (iframe_id, pending) in &mut self.pending {
            if pending.completion.is_some() && !pending.ready_task_queued {
                pending.ready_task_queued = true;
                notices.push(ReadyNotice {
                    iframe_id: *iframe_id,
                    epoch: pending.epoch,
                    owner_document_id: pending.owner_document_id,
                });
            }
        }
        notices.sort_unstable_by_key(|notice| notice.epoch);
        notices
    }

    fn take_ready(&mut self, iframe: usize, epoch: u64) -> Option<PendingNavigation> {
        let ready = self.pending.get(&iframe).is_some_and(|pending| {
            pending.epoch == epoch && pending.ready_task_queued && pending.completion.is_some()
        });
        ready.then(|| self.pending.remove(&iframe)).flatten()
    }
}

pub(super) enum IframeNavigationStart {
    Stale,
    Local(Option<form_state::PendingIframeHistoryRestore>),
    Deferred,
}

pub(super) enum IframeNavigationReady {
    Ignored,
    Committed(Option<usize>),
}

fn current_resource(iframe: &NodeHandle) -> (&'static str, String) {
    if iframe.has_tag_name("iframe") && iframe.has_attribute("srcdoc") {
        ("srcdoc", iframe.get_attribute("srcdoc").unwrap_or_default())
    } else if iframe.with_tag_name(|tag| tag.is_some_and(is_nested_frame_tag)) {
        (
            "src",
            iframe
                .get_attribute("src")
                .map(|src| src.trim().to_owned())
                .unwrap_or_default(),
        )
    } else {
        (
            "data",
            iframe
                .get_attribute("data")
                .map(|data| data.trim().to_owned())
                .unwrap_or_default(),
        )
    }
}

fn build_iframe_navigation_request(
    attribute: &str,
    resource: &str,
    resource_base: Option<&crate::http::Url>,
    submission: Option<&form_submission::Submission>,
    submission_base: Option<&crate::http::Url>,
    source: &NavigationReferrerSource,
    cookie_context: &crate::http::CookieRequestContext,
    javascript: bool,
) -> Result<Option<(HttpRequest, NavigationResponseConversion)>, JsHostError> {
    let (mut request, conversion) = if let Some(submission) = submission {
        if submission.method.eq_ignore_ascii_case("GET") {
            let Some(ResolvedResource::Url(url)) =
                resolve_resource_ref(&submission.url, submission_base)
            else {
                return Ok(None);
            };
            let Ok(parsed) = url.parse::<crate::http::Url>() else {
                return Ok(None);
            };
            if !matches!(parsed.scheme(), "http" | "https") {
                return Ok(None);
            }
            let Ok(request) = HttpRequest::get(&url) else {
                return Ok(None);
            };
            (
                request,
                NavigationResponseConversion::Get {
                    requested_reference: submission.url.clone(),
                    resolved_url: url,
                },
            )
        } else {
            let Ok(url) = submission.url.parse::<crate::http::Url>() else {
                return Ok(None);
            };
            if !matches!(url.scheme(), "http" | "https") {
                return Ok(None);
            }
            let mut request = HttpRequest::new(Method::Post, url);
            if let Some(body) = &submission.body {
                request.set_body(body.clone());
            }
            if let Some(content_type) = &submission.content_type {
                request.set_header("Content-Type", content_type);
            }
            (
                request,
                NavigationResponseConversion::Post {
                    submission_url: submission.url.clone(),
                },
            )
        }
    } else {
        if attribute == "srcdoc"
            || javascript
            || resource.is_empty()
            || matches_about_blank_url(resource)
            || resource
                .get(..5)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("blob:"))
        {
            return Ok(None);
        }
        let Some(ResolvedResource::Url(url)) = resolve_resource_ref(resource, resource_base) else {
            return Ok(None);
        };
        let Ok(parsed) = url.parse::<crate::http::Url>() else {
            return Ok(None);
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            return Ok(None);
        }
        let Ok(request) = HttpRequest::get(&url) else {
            return Ok(None);
        };
        (
            request,
            NavigationResponseConversion::Get {
                requested_reference: resource.to_owned(),
                resolved_url: url,
            },
        )
    };
    if !source.url.is_empty() {
        request.set_referrer_source(source.url.clone(), source.policy);
    }
    cookie_context.apply(&mut request);
    Ok(Some((request, conversion)))
}

impl JsRuntime {
    /// Enables queued iframe HTTP transport for interactive embedders.
    ///
    /// Disabled by default to preserve synchronous embedding behavior. Parsing,
    /// script execution and document commit stay on the owner thread. Disabling
    /// affects new loads; already submitted transports continue to completion.
    pub fn set_nonblocking_iframe_loading(&mut self, enabled: bool) {
        self.host_state.borrow_mut().nonblocking_iframe_loading = enabled;
    }
}

impl HostState {
    pub(super) fn reserve_iframe_navigation(
        &mut self,
        iframe: &NodeHandle,
        class: IframeNavigationClass,
    ) -> Option<u64> {
        let owner = owner_document_for_node(iframe).unwrap_or_else(|| self.document.clone());
        let initiator = self
            .pending_iframe_referrers
            .get(&iframe.identity())
            .and_then(|source| source.document_id)
            .filter(|document| self.document_is_active(*document))
            .unwrap_or_else(|| owner.identity());
        self.reserve_iframe_navigation_from(iframe, class, initiator)
    }

    pub(super) fn reserve_iframe_navigation_from(
        &mut self,
        iframe: &NodeHandle,
        class: IframeNavigationClass,
        initiator_document_id: usize,
    ) -> Option<u64> {
        let owner = owner_document_for_node(iframe).unwrap_or_else(|| self.document.clone());
        self.iframe_navigation.loads.reserve(
            iframe.identity(),
            owner.identity(),
            initiator_document_id,
            class,
        )
    }

    /// Reserves navigation whose URL, method and body belong to a queued form task.
    pub(super) fn reserve_iframe_form_submission(
        &mut self,
        iframe: &NodeHandle,
        initiator_document_id: usize,
    ) -> Option<u64> {
        let epoch = self.reserve_iframe_navigation_from(
            iframe,
            IframeNavigationClass::Foreground,
            initiator_document_id,
        )?;
        self.iframe_navigation
            .loads
            .reservations
            .get_mut(&iframe.identity())?
            .form_submission = true;
        Some(epoch)
    }

    pub(super) fn iframe_form_submission_is_reserved(&self, iframe: usize) -> bool {
        self.iframe_navigation
            .loads
            .reservations
            .get(&iframe)
            .is_some_and(|reservation| reservation.form_submission)
    }

    pub(super) fn current_iframe_navigation_epoch(&self, iframe: usize) -> Option<u64> {
        self.iframe_navigation.loads.current_epoch(iframe)
    }

    pub(super) fn attach_iframe_history_restore(
        &mut self,
        iframe: usize,
        epoch: u64,
        restore: form_state::PendingIframeHistoryRestore,
    ) -> bool {
        self.iframe_navigation
            .loads
            .attach_restore(iframe, epoch, restore)
    }

    pub(super) fn cancel_iframe_navigation(&mut self, iframe: usize) {
        self.iframe_navigation.loads.cancel(iframe);
    }

    pub(super) fn finish_local_iframe_navigation(
        &mut self,
        iframe: usize,
        epoch: u64,
    ) -> Option<Option<form_state::PendingIframeHistoryRestore>> {
        self.iframe_navigation.loads.finish_local(iframe, epoch)
    }

    pub(super) fn clear_iframe_navigation_intent(&mut self, iframe: usize) {
        self.pending_resource_loads.remove(&iframe);
        self.pending_forced_iframe_navigations.remove(&iframe);
        self.pending_iframe_visits.remove(&iframe);
        self.pending_iframe_referrers.remove(&iframe);
        self.pending_iframe_clients.remove(&iframe);
    }

    fn abort_iframe_navigation(&mut self, iframe: usize) {
        self.cancel_iframe_navigation(iframe);
        self.clear_iframe_navigation_intent(iframe);
    }

    pub(super) fn poll_iframe_navigation_loads(&mut self) {
        let notices = self.iframe_navigation.loads.poll(
            &self.resource_queue,
            &self.http_client,
            &self.cookie_store,
        );
        for notice in notices {
            self.event_loop.enqueue_networking_for_document(
                TimerPayload::IframeNavigationReady {
                    node_id: notice.iframe_id,
                    epoch: notice.epoch,
                },
                notice.owner_document_id,
            );
        }
    }

    fn iframe_navigation_fetch_context(
        &self,
        iframe: usize,
        owner: usize,
        attribute: &str,
        submission: Option<&form_submission::Submission>,
        submission_source: Option<usize>,
        source: &NavigationReferrerSource,
        javascript: bool,
    ) -> (crate::http::CookieRequestContext, Option<StorageOrigin>) {
        let form_source = source
            .document_id
            .or(submission_source)
            .or_else(|| submission.and_then(|request| request.source_document_id))
            .filter(|id| self.document_is_active(*id))
            .unwrap_or(owner);
        let initiator = if submission.is_some() {
            Some(form_source)
        } else {
            source.document_id
        };
        let grant_source = (submission.is_some() || attribute != "srcdoc" && !javascript)
            .then(|| self.storage_access_navigation_source(iframe, initiator))
            .flatten();
        let storage_access_origin =
            grant_source.and_then(|id| self.document_origins.get(&id).cloned().flatten());
        let request_context = self.document_cookie_request_context(
            initiator
                .filter(|id| self.document_is_active(*id))
                .unwrap_or(owner),
        );
        let cookie_context = self
            .document_cookie_request_context(owner)
            .for_navigation(Some(&request_context));
        (cookie_context, storage_access_origin)
    }

    pub(super) fn navigation_parent_policy(
        &self,
        container_document_id: usize,
    ) -> NavigationParentPolicy {
        let embedder = self
            .isolation_policy_for_document(container_document_id)
            .embedder;
        NavigationParentPolicy {
            origin: script_errors::document_fetch_origin(self, container_document_id),
            enforced: embedder.value.into(),
            report_only: embedder.report_only_value.into(),
        }
    }

    fn prepare_iframe_navigation_worker(
        &self,
        iframe: usize,
        owner: usize,
        attribute: &str,
        resource: &str,
        base: Option<&crate::http::Url>,
        submission: Option<&form_submission::Submission>,
        submission_source: Option<usize>,
        source: &NavigationReferrerSource,
        javascript: bool,
    ) -> Result<Option<(PreparedNavigationWorker, Option<StorageOrigin>)>, JsHostError> {
        let (cookie_context, storage_access_origin) = self.iframe_navigation_fetch_context(
            iframe,
            owner,
            attribute,
            submission,
            submission_source,
            source,
            javascript,
        );
        let submission_base = self.location_href.parse::<crate::http::Url>().ok();
        let Some((request, conversion)) = build_iframe_navigation_request(
            attribute,
            resource,
            base,
            submission,
            submission_base.as_ref(),
            source,
            &cookie_context,
            javascript,
        )?
        else {
            return Ok(None);
        };
        let parent = Some(self.navigation_parent_policy(owner));
        Ok(Some((
            PreparedNavigationWorker {
                transport: OwnedNavigationTransport::new(request, parent),
                conversion,
            },
            storage_access_origin,
        )))
    }

    pub(super) fn begin_reserved_iframe_navigation(
        &mut self,
        iframe: &NodeHandle,
        expected_epoch: Option<u64>,
        submission: Option<&form_submission::Submission>,
        submission_source_document_id: Option<usize>,
    ) -> Result<IframeNavigationStart, JsHostError> {
        let iframe_id = iframe.identity();
        if expected_epoch.is_some_and(|epoch| {
            self.iframe_navigation.loads.current_epoch(iframe_id) != Some(epoch)
        }) {
            return Ok(IframeNavigationStart::Stale);
        }
        if self
            .iframe_navigation
            .loads
            .pending
            .contains_key(&iframe_id)
        {
            return Ok(IframeNavigationStart::Deferred);
        }
        let Some(reservation) = self
            .iframe_navigation
            .loads
            .reservations
            .get(&iframe_id)
            .cloned()
        else {
            return Ok(IframeNavigationStart::Local(None));
        };
        let owner_document =
            owner_document_for_node(iframe).unwrap_or_else(|| self.document.clone());
        let owner_document_id = owner_document.identity();
        if owner_document_id != reservation.owner_document_id {
            self.abort_iframe_navigation(iframe_id);
            return Ok(IframeNavigationStart::Stale);
        }
        if !self.nonblocking_iframe_loading
            || !IframeNavigationLoads::can_defer(self.resource_queue.worker_count().get())
        {
            let restore = self
                .iframe_navigation
                .loads
                .finish_local(iframe_id, reservation.epoch)
                .flatten();
            return Ok(IframeNavigationStart::Local(restore));
        }
        if !self.iframe_context_ids.contains_key(&iframe_id)
            || !self.iframe_documents.contains_key(&iframe_id)
        {
            if let Err(error) = self.iframe_document_with_submission(iframe, None, None, true) {
                self.abort_iframe_navigation(iframe_id);
                return Err(error);
            }
        }
        let Some(context_id) = self.iframe_context_ids.get(&iframe_id).copied() else {
            let restore = self
                .iframe_navigation
                .loads
                .finish_local(iframe_id, reservation.epoch)
                .flatten();
            return Ok(IframeNavigationStart::Local(restore));
        };
        let Some(expected_document_id) = self
            .iframe_documents
            .get(&iframe_id)
            .map(|entry| entry.document.identity())
        else {
            let restore = self
                .iframe_navigation
                .loads
                .finish_local(iframe_id, reservation.epoch)
                .flatten();
            return Ok(IframeNavigationStart::Local(restore));
        };
        let (resource_attribute, resource) = current_resource(iframe);
        let resource_base = if owner_document_id == self.document.identity() {
            self.base_url.clone()
        } else {
            self.document_base_urls.get(&owner_document_id).cloned()
        };
        let source = self.referrer_source_for_iframe_navigation(
            iframe,
            owner_document_id,
            submission.is_some(),
        );
        let javascript = submission.is_none()
            && resource_attribute == "src"
            && url::Url::parse(&resource).is_ok_and(|url| url.scheme() == "javascript");
        let prepared = self.prepare_iframe_navigation_worker(
            iframe_id,
            owner_document_id,
            resource_attribute,
            &resource,
            resource_base.as_ref(),
            submission,
            submission_source_document_id,
            &source,
            javascript,
        );
        let Some((worker, storage_access_origin)) = (match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.abort_iframe_navigation(iframe_id);
                return Err(error);
            }
        }) else {
            let restore = self
                .iframe_navigation
                .loads
                .finish_local(iframe_id, reservation.epoch)
                .flatten();
            return Ok(IframeNavigationStart::Local(restore));
        };
        let pending = PendingNavigation {
            epoch: reservation.epoch,
            class: reservation.class,
            context_id,
            owner_document_id,
            initiator_document_id: reservation.initiator_document_id,
            expected_document_id,
            resource_attribute,
            resource,
            resource_base,
            submission: submission.cloned(),
            submission_source_document_id,
            source,
            storage_access_origin,
            restore: reservation.restore,
            worker: Some(worker),
            active: None,
            completion: None,
            ready_task_queued: false,
        };
        if !self.iframe_navigation.loads.begin(iframe_id, pending) {
            return Ok(IframeNavigationStart::Stale);
        }
        self.poll_iframe_navigation_loads();
        Ok(IframeNavigationStart::Deferred)
    }

    fn iframe_navigation_is_current(
        &self,
        iframe: usize,
        pending: &PendingNavigation,
    ) -> Option<NodeHandle> {
        let frame = self.get_node(iframe)?;
        if !self.node_is_in_active_document(&frame)
            || owner_document_for_node(&frame).map(|owner| owner.identity())
                != Some(pending.owner_document_id)
            || self.iframe_context_ids.get(&iframe).copied() != Some(pending.context_id)
            || self
                .iframe_documents
                .get(&iframe)
                .map(|entry| entry.document.identity())
                != Some(pending.expected_document_id)
            || current_resource(&frame) != (pending.resource_attribute, pending.resource.clone())
        {
            return None;
        }
        Some(frame)
    }

    fn loaded_from_navigation_worker(
        &mut self,
        owner_document_id: usize,
        result: NavigationWorkerResult,
    ) -> Result<LoadedChildDocument, String> {
        let NavigationWorkerResult {
            transport:
                NavigationTransportResult {
                    response,
                    redirects,
                    violations,
                },
            conversion,
            elapsed_ms,
        } = result;
        self.queue_coep_corp_violations(owner_document_id, "document", violations);
        let parse_started = Instant::now();
        let loaded = match conversion {
            NavigationResponseConversion::Get {
                requested_reference,
                resolved_url,
            } => match response.ok().flatten() {
                None => LoadedChildDocument::fetch_failed(),
                Some(response) => {
                    let mut effective_url = response
                        .effective_url()
                        .map_or_else(|| resolved_url.clone(), ToString::to_string);
                    if let Some((_, fragment)) = requested_reference.split_once('#')
                        && !effective_url.contains('#')
                    {
                        effective_url.push('#');
                        effective_url.push_str(fragment);
                    }
                    FetchedChildResource {
                        mime_type: response.header("Content-Type").unwrap_or("").to_owned(),
                        security_headers: child_document::response_security_headers(&response),
                        response_status: response.status_code(),
                        final_request_referrer: response
                            .final_request_referrer()
                            .unwrap_or_default()
                            .to_owned(),
                        response_headers: response.headers().to_vec(),
                        redirects,
                        body: response.into_body(),
                        effective_url,
                    }
                    .into_get_document()
                }
            },
            NavigationResponseConversion::Post { submission_url } => {
                match response.map_err(|error| error.to_string())? {
                    None => LoadedChildDocument::fetch_failed(),
                    Some(response) => {
                        let mime_type = response.header("Content-Type").unwrap_or("");
                        let document = child_document::parse_child_response(
                            mime_type,
                            response.body(),
                            response.headers(),
                        )
                        .unwrap_or_else(blank_html_document);
                        let url = response
                            .effective_url()
                            .map_or(submission_url, ToString::to_string);
                        LoadedChildDocument {
                            document,
                            response_status: response.status_code(),
                            final_request_referrer: Some(
                                response
                                    .final_request_referrer()
                                    .unwrap_or_default()
                                    .to_owned(),
                            ),
                            security_headers: child_document::response_security_headers(&response),
                            response_headers: response.headers().to_vec(),
                            redirects,
                            url: Some(url),
                        }
                    }
                }
            }
        };
        if std::env::var_os("OMOIKANE_TRACE_TASKS").is_some() {
            eprintln!(
                "{}",
                crate::diagnostics::json_record(
                    "OMOIKANE_IFRAME_RESOURCE",
                    &serde_json::json!({
                        "fetch_ms": elapsed_ms,
                        "parse_ms": parse_started.elapsed().as_secs_f64() * 1_000.0,
                        "loaded": loaded.url.is_some(),
                    })
                )
            );
        }
        Ok(loaded)
    }

    pub(super) fn commit_ready_iframe_navigation(
        &mut self,
        iframe: usize,
        epoch: u64,
    ) -> Result<IframeNavigationReady, String> {
        let Some(mut pending) = self.iframe_navigation.loads.take_ready(iframe, epoch) else {
            return Ok(IframeNavigationReady::Ignored);
        };
        let Some(frame) = self.iframe_navigation_is_current(iframe, &pending) else {
            self.clear_iframe_navigation_intent(iframe);
            return Ok(IframeNavigationReady::Ignored);
        };
        let completion = pending
            .completion
            .take()
            .expect("ready iframe navigation has completion");
        let post = pending
            .submission
            .as_ref()
            .is_some_and(|submission| !submission.method.eq_ignore_ascii_case("GET"));
        let loaded_result = match completion {
            NavigationCompletion::Finished(result) => {
                self.loaded_from_navigation_worker(pending.owner_document_id, result)
            }
            NavigationCompletion::Cancelled => {
                self.clear_iframe_navigation_intent(iframe);
                return Ok(IframeNavigationReady::Ignored);
            }
            NavigationCompletion::Panicked => {
                if post {
                    Err("iframe navigation worker panicked".to_owned())
                } else {
                    Ok(LoadedChildDocument::fetch_failed())
                }
            }
            NavigationCompletion::Disconnected => {
                if post {
                    Err("iframe navigation worker disconnected".to_owned())
                } else {
                    Ok(LoadedChildDocument::fetch_failed())
                }
            }
            NavigationCompletion::AdmissionFailed(error) => {
                if post {
                    Err(error)
                } else {
                    Ok(LoadedChildDocument::fetch_failed())
                }
            }
        };
        let loaded = match loaded_result {
            Ok(loaded) => loaded,
            Err(error) => {
                self.clear_iframe_navigation_intent(iframe);
                return Err(error);
            }
        };
        let storage_access_origin = pending.storage_access_origin.filter(|origin| {
            loaded
                .url
                .as_deref()
                .and_then(StorageOrigin::from_document_url)
                .as_ref()
                == Some(origin)
                && loaded
                    .redirects
                    .iter()
                    .all(|hop| StorageOrigin::from_url(&hop.url).as_ref() == Some(origin))
        });
        if let Err(error) = self.iframe_document_with_preloaded_submission(
            &frame,
            pending.submission.as_ref(),
            pending.submission_source_document_id,
            false,
            (loaded, storage_access_origin),
            pending.source,
            pending.resource_base,
        ) {
            self.clear_iframe_navigation_intent(iframe);
            return Err(error.to_string());
        }
        let restore_document = pending
            .restore
            .and_then(|restore| form_state::queue_iframe_history_restore(self, iframe, restore));
        Ok(IframeNavigationReady::Committed(restore_document))
    }

    /// Loads one navigation and retains an eligible origin through same-origin
    /// redirects. Network I/O finishes before any old Document is retired.
    pub(super) fn load_iframe_navigation_resource(
        &mut self,
        iframe: usize,
        owner: usize,
        attribute: &str,
        resource: &str,
        base: Option<&crate::http::Url>,
        submission: Option<&form_submission::Submission>,
        submission_source: Option<usize>,
        source: &NavigationReferrerSource,
        javascript: bool,
    ) -> Result<(LoadedChildDocument, Option<StorageOrigin>), JsHostError> {
        let (cookie_context, origin) = self.iframe_navigation_fetch_context(
            iframe,
            owner,
            attribute,
            submission,
            submission_source,
            source,
            javascript,
        );
        let loaded = if let Some(request) = submission {
            self.load_iframe_form_submission_with_cookie_context(
                request,
                &cookie_context,
                owner,
                &source.url,
                source.policy,
            )?
        } else if attribute == "srcdoc" {
            LoadedChildDocument::srcdoc(resource)
        } else if javascript {
            LoadedChildDocument::about_blank("about:blank".to_owned())
        } else {
            self.load_iframe_document_with_cookie_context(
                resource,
                base,
                Some(&cookie_context),
                Some(owner),
                false,
                &source.url,
                source.policy,
            )
        };
        let inherited = origin.filter(|origin| {
            loaded
                .url
                .as_deref()
                .and_then(StorageOrigin::from_document_url)
                .as_ref()
                == Some(origin)
                && loaded
                    .redirects
                    .iter()
                    .all(|hop| StorageOrigin::from_url(&hop.url).as_ref() == Some(origin))
        });
        Ok((loaded, inherited))
    }
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_register_iframe_navigation"),
        2,
        NativeFunction::from_copy_closure(|_, args, context| {
            let document = parse_node_id(args.first(), context)?;
            ensure_same_origin_document(context, document)?;
            let callback = args
                .get(1)
                .filter(|value| value.is_callable())
                .cloned()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("iframe navigation handler required")
                })?;
            with_host_state(|host| {
                host.borrow_mut()
                    .iframe_navigation
                    .owners
                    .insert(document, callback);
                Ok(JsValue::undefined())
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_child_navigation"),
        4,
        NativeFunction::from_copy_closure(|_, args, context| {
            let document = parse_node_id(args.first(), context)?;
            ensure_history_document_active(document, args.get(1), context)?;
            let navigates = args
                .get(1)
                .and_then(JsValue::as_string)
                .is_some_and(|kind| {
                    matches!(
                        kind.to_std_string_escaped().as_str(),
                        "assign" | "replace" | "reload"
                    )
                });
            if navigates {
                let destination = args
                    .get(2)
                    .cloned()
                    .unwrap_or_default()
                    .to_string(context)?
                    .to_std_string_escaped();
                if url::Url::parse(&destination).is_ok_and(|url| url.scheme() == "javascript") {
                    // The active child Document supplies the request client,
                    // policy, and execution Realm. No resource load or iframe
                    // attribute mutation occurs for a non-string completion.
                    let completion = evaluate_javascript_url_in_context(
                        &destination,
                        document,
                        document,
                        None,
                        context,
                    );
                    if let Some(markup) = completion {
                        with_host_state(|host| {
                            let frame_id = host.borrow().iframe_documents.iter().find_map(
                                |(frame_id, entry)| {
                                    (entry.document.identity() == document).then_some(*frame_id)
                                },
                            );
                            if let Some(frame_id) = frame_id {
                                host.borrow_mut()
                                    .commit_javascript_iframe_document(frame_id, document, &markup)
                                    .map_err(|error| {
                                        JsNativeError::error().with_message(error.to_string())
                                    })?;
                            }
                            Ok(())
                        })?;
                    }
                    return Ok(JsValue::undefined());
                }
            }
            let navigation_initiator = navigates
                .then(|| navigation_client_document_id(context))
                .flatten();
            with_host_state(|host| {
                let (frame, callback, document_url, event_state) = {
                    let mut state = host.borrow_mut();
                    let frame = state
                        .iframe_documents
                        .iter()
                        .find_map(|(frame, entry)| {
                            (entry.document.identity() == document).then_some(*frame)
                        })
                        .ok_or_else(|| {
                            JsNativeError::typ().with_message("Document is no longer active")
                        })?;
                    let node = state
                        .get_node(frame)
                        .filter(|node| state.node_is_in_active_document(node))
                        .ok_or_else(|| {
                            JsNativeError::typ().with_message("iframe is no longer active")
                        })?;
                    // Resolving the facade may commit a src/srcdoc change.
                    // Recheck after that lazy load, before forwarding any
                    // operation from the departing Document to its replacement.
                    let active = state
                        .iframe_content_document(&node)
                        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
                    if active.identity() != document {
                        return Err(JsNativeError::typ()
                            .with_message("Document is no longer active")
                            .into());
                    }
                    let owner = owner_document_for_node(&node).ok_or_else(|| {
                        JsNativeError::typ().with_message("iframe has no owner document")
                    })?;
                    let callback = state
                        .iframe_navigation
                        .owners
                        .get(&owner.identity())
                        .cloned()
                        .ok_or_else(|| {
                            JsNativeError::typ().with_message("iframe owner is no longer active")
                        })?;
                    let document_url =
                        state.document_urls.get(&document).cloned().ok_or_else(|| {
                            JsNativeError::typ().with_message("Document URL is no longer active")
                        })?;
                    let event_state = state.shared_event_state_for_node(document);
                    (frame, callback, document_url, event_state)
                };
                let mut forwarded =
                    vec![JsValue::from(frame as f64), JsValue::from(document as f64)];
                forwarded.extend_from_slice(&args[1..]);
                forwarded.push(JsValue::from(js_string!(document_url.as_str())));
                forwarded.push(event_state.map_or_else(JsValue::null, JsValue::from));
                let (previous_active, source) = if navigates {
                    let mut state = host.borrow_mut();
                    let request_client = navigation_initiator.unwrap_or(document);
                    let source = state.visit_source_for_document(request_client);
                    state.pending_iframe_visits.remove(&frame);
                    state.capture_iframe_referrer_source(frame, request_client);
                    if let Some(source) = &source {
                        state.pending_iframe_visits.insert(frame, source.clone());
                    }
                    let previous_active = state.active_child_navigation_frame.replace(frame);
                    (previous_active, source)
                } else {
                    (None, None)
                };
                let result = callback
                    .as_callable()
                    .expect("registered navigation handler")
                    .call(&JsValue::undefined(), &forwarded, context);
                if navigates {
                    let mut state = host.borrow_mut();
                    state.active_child_navigation_frame = previous_active;
                    if result.is_ok() && state.pending_resource_loads.contains(&frame) {
                        if let Some(source) = source {
                            state.pending_iframe_visits.insert(frame, source);
                        } else {
                            state.pending_iframe_visits.remove(&frame);
                        }
                    } else {
                        state.pending_iframe_visits.remove(&frame);
                        state.pending_iframe_referrers.remove(&frame);
                    }
                }
                result
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_iframe_document_url"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let frame = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, frame)?;
            with_host_state(|host| {
                Ok(host
                    .borrow()
                    .iframe_documents
                    .get(&frame)
                    .map_or_else(JsValue::null, |entry| {
                        js_string!(entry.document_url.as_str()).into()
                    }))
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_commit_iframe_fragment"),
        2,
        NativeFunction::from_copy_closure(|_, args, context| {
            let frame = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, frame)?;
            let url = args
                .get(1)
                .ok_or_else(|| JsNativeError::typ().with_message("iframe URL required"))?
                .to_string(context)?
                .to_std_string_escaped();
            with_host_state(|host| {
                let mut state = host.borrow_mut();
                let entry = state.iframe_documents.get(&frame).ok_or_else(|| {
                    JsNativeError::typ().with_message("iframe is no longer active")
                })?;
                let document = entry.document.clone();
                let previous_url =
                    state
                        .document_urls
                        .get(&document.identity())
                        .ok_or_else(|| {
                            JsNativeError::typ().with_message("Document URL is no longer active")
                        })?;
                if previous_url.split('#').next() != url.split('#').next() {
                    return Err(JsNativeError::typ()
                        .with_message("iframe navigation is not fragment-only")
                        .into());
                }
                state.document_urls.insert(document.identity(), url.clone());
                if let Some(entry) = state.iframe_documents.get_mut(&frame) {
                    entry.document_url = url.clone();
                }
                state.update_document_target(&document, &url);
                Ok(JsValue::undefined())
            })
        }),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::TreeBuilder;
    use crate::http::{CookieRequestContext, Method};

    fn referrer_source() -> NavigationReferrerSource {
        NavigationReferrerSource {
            document_id: Some(7),
            url: "https://source.example.test/form".to_owned(),
            policy: security_reporting::ReferrerPolicy::default(),
        }
    }

    fn cookie_context() -> CookieRequestContext {
        CookieRequestContext::new(
            Some("https://site.example.test/".parse().unwrap()),
            None,
            true,
        )
    }

    fn submission(url: &str, method: &str) -> form_submission::Submission {
        form_submission::Submission {
            url: url.to_owned(),
            method: method.to_owned(),
            body: None,
            content_type: None,
            source_document_id: Some(7),
        }
    }

    fn pending_navigation(
        epoch: u64,
        class: IframeNavigationClass,
        owner_document_id: usize,
        initiator_document_id: usize,
        with_worker: bool,
    ) -> PendingNavigation {
        let worker = with_worker.then(|| PreparedNavigationWorker {
            transport: OwnedNavigationTransport::new(
                HttpRequest::get("https://child.example.test/frame").unwrap(),
                None,
            ),
            conversion: NavigationResponseConversion::Get {
                requested_reference: "/frame".to_owned(),
                resolved_url: "https://child.example.test/frame".to_owned(),
            },
        });
        PendingNavigation {
            epoch,
            class,
            context_id: 11,
            owner_document_id,
            initiator_document_id,
            expected_document_id: 12,
            resource_attribute: "src",
            resource: "/frame".to_owned(),
            resource_base: Some("https://child.example.test/base/".parse().unwrap()),
            submission: None,
            submission_source_document_id: None,
            source: referrer_source(),
            storage_access_origin: None,
            restore: None,
            worker,
            active: None,
            completion: None,
            ready_task_queued: false,
        }
    }

    #[test]
    fn form_request_classification_uses_submission_instead_of_container_resource() {
        let source = referrer_source();
        let cookies = cookie_context();
        let base: crate::http::Url = "https://site.example.test/base/".parse().unwrap();
        let get = submission("https://target.example.test/search?q=one", "GET");
        let (get_request, get_conversion) = build_iframe_navigation_request(
            "srcdoc",
            "<p>old local document</p>",
            None,
            Some(&get),
            Some(&base),
            &source,
            &cookies,
            false,
        )
        .unwrap()
        .expect("remote GET submission from a srcdoc target");
        assert_eq!(get_request.method(), Method::Get);
        assert_eq!(get_request.url().to_string(), get.url);
        assert_eq!(get_request.site_for_cookies(), cookies.cookie_site());
        assert_eq!(
            get_request.referrer_source().map(|value| value.0),
            Some(source.url.as_str())
        );
        assert!(matches!(
            get_conversion,
            NavigationResponseConversion::Get {
                requested_reference,
                resolved_url,
            } if requested_reference == get.url && resolved_url == get.url
        ));

        let mut post = submission("https://target.example.test/save", "POST");
        post.body = Some(b"answer=forty-two".to_vec());
        post.content_type = Some("application/x-www-form-urlencoded".to_owned());
        let (post_request, post_conversion) = build_iframe_navigation_request(
            "src",
            "blob:https://site.example.test/old",
            Some(&base),
            Some(&post),
            Some(&base),
            &source,
            &cookies,
            false,
        )
        .unwrap()
        .expect("remote POST submission from a blob-backed target");
        assert_eq!(post_request.method(), Method::Post);
        assert_eq!(post_request.body(), post.body.as_deref());
        assert_eq!(
            post_request.header("Content-Type"),
            post.content_type.as_deref()
        );
        assert_eq!(post_request.site_for_cookies(), cookies.cookie_site());
        assert_eq!(
            post_request.referrer_source().map(|value| value.0),
            Some(source.url.as_str())
        );
        assert!(matches!(
            post_conversion,
            NavigationResponseConversion::Post { submission_url }
                if submission_url == post.url
        ));
    }

    #[test]
    fn non_http_attribute_and_submission_resources_stay_owner_local() {
        let source = referrer_source();
        let cookies = cookie_context();
        let base: crate::http::Url = "https://site.example.test/base/".parse().unwrap();
        for (attribute, resource, javascript) in [
            ("srcdoc", "<p>inline</p>", false),
            ("src", "", false),
            ("src", "about:blank", false),
            ("src", "blob:https://site.example.test/id", false),
            ("src", "data:text/html,local", false),
            ("src", "javascript:void 0", true),
        ] {
            assert!(
                build_iframe_navigation_request(
                    attribute,
                    resource,
                    Some(&base),
                    None,
                    Some(&base),
                    &source,
                    &cookies,
                    javascript,
                )
                .unwrap()
                .is_none(),
                "{attribute}={resource} must not enter the HTTP worker"
            );
        }
        for local in [
            submission("data:text/html,form", "GET"),
            submission("blob:https://site.example.test/form", "POST"),
        ] {
            assert!(
                build_iframe_navigation_request(
                    "srcdoc",
                    "<p>container resource is irrelevant</p>",
                    None,
                    Some(&local),
                    Some(&base),
                    &source,
                    &cookies,
                    false,
                )
                .unwrap()
                .is_none()
            );
        }
    }

    struct BaseChangeTransport {
        started: std::sync::mpsc::Sender<String>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl crate::http::HttpTransport for BaseChangeTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<std::time::Duration>,
        ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
            self.started.send(request.url().to_string()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            Ok(crate::http::HttpResponse::new(
                200,
                "OK",
                vec![("Content-Type".into(), "text/html".into())],
                b"<!doctype html><title>captured base</title>".to_vec(),
            ))
        }
    }

    #[test]
    fn queued_navigation_commit_retains_the_request_time_base_url() {
        let document = TreeBuilder::parse(
            "<base id=base href='https://child-a.example.test/request-base/'>\
             <iframe id=target src='child.html'></iframe>",
        )
        .document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://owner.example.test/top/index.html")
                .unwrap();
        runtime.set_nonblocking_iframe_loading(true);
        runtime.install_isolation_policy(&[(
            "Cross-Origin-Embedder-Policy-Report-Only".into(),
            "require-corp; report-to=\"base-change\"".into(),
        )]);
        runtime
            .eval(
                "globalThis.baseChangeReports=[]; \
                 new ReportingObserver(batch => baseChangeReports.push(...batch), \
                   {types:['coep']}).observe();",
            )
            .unwrap();
        let request_base: crate::http::Url = "https://child-a.example.test/request-base/"
            .parse()
            .unwrap();
        let later_base: crate::http::Url =
            "https://child-b.example.test/later-base/".parse().unwrap();
        runtime.set_base_url(request_base.clone());
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        runtime.set_http_resource_transport(Some(Arc::new(BaseChangeTransport {
            started: started_tx,
            release: std::sync::Mutex::new(release_rx),
        })));
        let (storage, visit_source, iframe) = {
            let state = runtime.host_state.borrow();
            let owner = state.document.identity();
            (
                state.storage_manager.clone(),
                state.visit_source_for_document(owner).unwrap(),
                state.document.query_selector("#target").unwrap().identity(),
            )
        };

        runtime.run_until_idle().unwrap();
        assert_eq!(
            started_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            "https://child-a.example.test/request-base/child.html"
        );
        runtime
            .eval(
                "document.getElementById('base').href = \
                 'https://child-b.example.test/later-base/'",
            )
            .unwrap();
        // The embedding hook keeps HostState's effective resource base in sync
        // with the mutated HTMLBaseElement.
        runtime.set_base_url(later_base);
        release_tx.send(()).unwrap();

        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            runtime.run_until_idle().unwrap();
            let committed = runtime
                .host_state
                .borrow()
                .iframe_documents
                .get(&iframe)
                .is_some_and(|entry| {
                    entry.document_url == "https://child-a.example.test/request-base/child.html"
                        && entry.initial_resource_task_completed
                });
            if committed {
                break;
            }
            assert!(Instant::now() < deadline, "iframe worker did not commit");
            std::thread::yield_now();
        }
        assert!(storage.has_visited_url(
            "https://child-a.example.test/request-base/child.html",
            &visit_source,
        ));
        assert!(!storage.has_visited_url(
            "https://child-b.example.test/later-base/child.html",
            &visit_source,
        ));
        assert!(
            runtime
                .eval(
                    "baseChangeReports.some(report => \
                       report.body.blockedURL === \
                       'https://child-a.example.test/request-base/child.html')"
                )
                .unwrap()
                .to_boolean(),
            "COEP policy report must use the request-time base"
        );
        assert!(
            !runtime
                .eval(
                    "baseChangeReports.some(report => \
                       report.body.blockedURL === \
                       'https://child-b.example.test/later-base/child.html')"
                )
                .unwrap()
                .to_boolean()
        );
    }

    #[test]
    fn iframe_admission_keeps_embeds_normal_and_foreground_first() {
        assert_eq!(
            IframeNavigationClass::Embedded.priority(),
            ResourcePriority::Normal
        );
        assert_eq!(
            IframeNavigationClass::Foreground.priority(),
            ResourcePriority::Critical
        );
        assert_eq!(IframeNavigationClass::Embedded.kind(), ResourceKind::Other);
        assert!(!IframeNavigationLoads::can_defer(1));
        assert_eq!(IframeNavigationLoads::admission_limit(1), 0);
        assert!(IframeNavigationLoads::can_defer(2));
        assert_eq!(IframeNavigationLoads::admission_limit(2), 1);
        assert_eq!(IframeNavigationLoads::admission_limit(4), 3);

        let mut loads = IframeNavigationLoads::default();
        let embedded_epoch = loads
            .reserve(1, 10, 10, IframeNavigationClass::Embedded)
            .unwrap();
        assert!(loads.begin(
            1,
            pending_navigation(
                embedded_epoch,
                IframeNavigationClass::Embedded,
                10,
                10,
                true,
            ),
        ));
        let foreground_epoch = loads
            .reserve(2, 10, 20, IframeNavigationClass::Foreground)
            .unwrap();
        assert!(loads.begin(
            2,
            pending_navigation(
                foreground_epoch,
                IframeNavigationClass::Foreground,
                10,
                20,
                true,
            ),
        ));
        assert_eq!(loads.next_waiting(0), Some((2, foreground_epoch)));
        loads
            .waiting
            .retain(|candidate| *candidate != (2, foreground_epoch));
        assert_eq!(loads.next_waiting(1), None);
        assert_eq!(loads.next_waiting(0), Some((1, embedded_epoch)));
    }

    #[test]
    fn one_worker_keeps_embedded_and_foreground_navigation_owner_local() {
        let document = TreeBuilder::parse("<iframe id=target></iframe>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://owner.example.test/page").unwrap();
        runtime.set_nonblocking_iframe_loading(true);
        runtime.run_until_idle().unwrap();
        let frame = runtime
            .host_state
            .borrow()
            .document
            .query_selector("#target")
            .unwrap();
        let iframe = frame.identity();
        let restore = captured_restore(&mut runtime, iframe);
        frame.set_attribute("src", "https://child.example.test/frame");
        let owner = owner_document_for_node(&frame).unwrap().identity();
        let mut state = runtime.host_state.borrow_mut();
        state.resource_queue =
            crate::resource_worker::ResourceQueue::new(std::num::NonZeroUsize::new(1).unwrap());

        for class in [
            IframeNavigationClass::Embedded,
            IframeNavigationClass::Foreground,
        ] {
            let epoch = state
                .reserve_iframe_navigation_from(&frame, class, owner)
                .unwrap();
            assert!(state.attach_iframe_history_restore(iframe, epoch, restore.clone()));
            assert!(matches!(
                state
                    .begin_reserved_iframe_navigation(&frame, Some(epoch), None, None)
                    .unwrap(),
                IframeNavigationStart::Local(Some(_))
            ));
            assert_eq!(state.current_iframe_navigation_epoch(iframe), None);
            assert!(!state.iframe_navigation.has_pending_loads());
        }
    }

    #[test]
    fn replacement_and_document_retirement_invalidate_only_exact_epochs() {
        let mut loads = IframeNavigationLoads::default();
        let first = loads
            .reserve(4, 10, 20, IframeNavigationClass::Embedded)
            .unwrap();
        let second = loads
            .reserve(4, 10, 21, IframeNavigationClass::Foreground)
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(loads.current_epoch(4), Some(second));
        assert!(loads.finish_local(4, first).is_none());
        assert_eq!(loads.current_epoch(4), Some(second));

        loads.next_epoch = u64::MAX;
        assert!(
            loads
                .reserve(4, 10, 22, IframeNavigationClass::Foreground)
                .is_none()
        );
        assert_eq!(loads.current_epoch(4), Some(second));
        loads.next_epoch = second;

        assert_eq!(loads.cancel_document(21), vec![4]);
        assert_eq!(loads.current_epoch(4), None);

        let pending_epoch = loads
            .reserve(5, 10, 30, IframeNavigationClass::Foreground)
            .unwrap();
        assert!(loads.begin(
            5,
            pending_navigation(
                pending_epoch,
                IframeNavigationClass::Foreground,
                10,
                30,
                false,
            ),
        ));
        assert_eq!(loads.cancel_document(30), vec![5]);
        assert_eq!(loads.current_epoch(5), None);
    }

    #[test]
    fn ready_commit_validation_rejects_owner_context_document_and_resource_changes() {
        let document = TreeBuilder::parse("<iframe id=target></iframe>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://owner.example.test/page").unwrap();
        runtime.run_until_idle().unwrap();
        let state = runtime.host_state.borrow();
        let frame = state.document.query_selector("#target").unwrap();
        let iframe = frame.identity();
        let owner = owner_document_for_node(&frame).unwrap().identity();
        let context = state.iframe_context_ids[&iframe];
        let child = state.iframe_documents[&iframe].document.identity();
        let (attribute, resource) = current_resource(&frame);
        let mut pending =
            pending_navigation(1, IframeNavigationClass::Embedded, owner, owner, false);
        pending.context_id = context;
        pending.expected_document_id = child;
        pending.resource_attribute = attribute;
        pending.resource = resource;
        assert!(
            state
                .iframe_navigation_is_current(iframe, &pending)
                .is_some()
        );

        pending.owner_document_id = owner.wrapping_add(1);
        assert!(
            state
                .iframe_navigation_is_current(iframe, &pending)
                .is_none()
        );
        pending.owner_document_id = owner;
        pending.context_id = context.wrapping_add(1);
        assert!(
            state
                .iframe_navigation_is_current(iframe, &pending)
                .is_none()
        );
        pending.context_id = context;
        pending.expected_document_id = child.wrapping_add(1);
        assert!(
            state
                .iframe_navigation_is_current(iframe, &pending)
                .is_none()
        );
        pending.expected_document_id = child;
        pending.resource.push_str("-changed");
        assert!(
            state
                .iframe_navigation_is_current(iframe, &pending)
                .is_none()
        );
    }

    #[test]
    fn retiring_form_source_clears_the_sibling_target_reservation_and_intent() {
        let document =
            TreeBuilder::parse("<iframe id=source></iframe><iframe id=target></iframe>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://owner.example.test/page").unwrap();
        runtime.run_until_idle().unwrap();
        let mut state = runtime.host_state.borrow_mut();
        let source = state.document.query_selector("#source").unwrap();
        let target = state.document.query_selector("#target").unwrap();
        let source_document = state.iframe_documents[&source.identity()]
            .document
            .identity();
        let target_id = target.identity();
        let epoch = state
            .reserve_iframe_navigation_from(
                &target,
                IframeNavigationClass::Foreground,
                source_document,
            )
            .unwrap();
        let owner_document = state.document.identity();
        state.event_loop.enqueue_networking_for_document(
            TimerPayload::IframeNavigationReady {
                node_id: target_id,
                epoch,
            },
            owner_document,
        );
        state.pending_resource_loads.insert(target_id);
        state
            .pending_iframe_referrers
            .insert(target_id, referrer_source());
        if let Some(client) = state.navigation_security_snapshot(source_document) {
            state.pending_iframe_clients.insert(target_id, client);
        }

        state.retire_iframe_document(source.identity());

        assert_eq!(state.current_iframe_navigation_epoch(target_id), None);
        assert!(!state.pending_resource_loads.contains(&target_id));
        assert!(!state.pending_iframe_referrers.contains_key(&target_id));
        assert!(!state.pending_iframe_clients.contains_key(&target_id));
        while let Some((_, task)) = state.event_loop.pop_task() {
            assert!(!matches!(
                task,
                Task::Timer {
                    payload: TimerPayload::IframeNavigationReady { node_id, .. },
                    ..
                } if node_id == target_id
            ));
        }
    }

    fn javascript_restore_runtime() -> (JsRuntime, NodeHandle, usize) {
        let document = TreeBuilder::parse("<iframe id=target></iframe>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://owner.example.test/page").unwrap();
        let markup = r#"<!doctype html><form><x-restored name=answer></x-restored></form><script>
            globalThis.restorations=[];
            customElements.define('x-restored', class extends HTMLElement {
                static formAssociated=true;
                constructor(){super();this.i=this.attachInternals();this.i.setFormValue('initial');}
                formStateRestoreCallback(value,mode){restorations.push([value,mode]);this.i.setFormValue(value,value);}
            });
            globalThis.control=document.querySelector('x-restored');
        </script>"#;
        runtime
            .eval(&format!(
                "globalThis.frame=document.getElementById('target');frame.srcdoc={};globalThis.proxy=frame.contentWindow",
                serde_json::to_string(markup).unwrap()
            ))
            .unwrap();
        runtime.run_until_idle().unwrap();
        let frame = runtime
            .host_state
            .borrow()
            .document
            .query_selector("#target")
            .unwrap();
        let iframe = frame.identity();
        (runtime, frame, iframe)
    }

    fn captured_restore(
        runtime: &mut JsRuntime,
        iframe: usize,
    ) -> form_state::PendingIframeHistoryRestore {
        let _guard = activate_host_state(Rc::clone(&runtime.host_state));
        form_state::capture_iframe_history_restore_for_test(
            &runtime.host_state,
            iframe,
            &mut runtime.context,
        )
        .unwrap()
    }

    fn run_javascript_restore(
        runtime: &mut JsRuntime,
        frame: &NodeHandle,
        iframe: usize,
        url: String,
        restore: form_state::PendingIframeHistoryRestore,
    ) {
        let source_document = runtime.host_state.borrow().document.identity();
        let (epoch, client) = {
            let mut state = runtime.host_state.borrow_mut();
            let epoch = state
                .reserve_iframe_navigation_from(
                    frame,
                    IframeNavigationClass::Foreground,
                    source_document,
                )
                .unwrap();
            assert!(state.attach_iframe_history_restore(iframe, epoch, restore));
            (epoch, state.navigation_security_snapshot(source_document))
        };
        let request = form_submission::Submission {
            url,
            method: "GET".to_owned(),
            body: None,
            content_type: None,
            source_document_id: Some(source_document),
        };
        runtime
            .run_javascript_iframe_form_submission(
                frame,
                iframe,
                epoch,
                &request,
                source_document,
                client.as_ref(),
            )
            .unwrap();
    }

    #[test]
    fn javascript_non_string_history_restore_delivers_to_the_existing_realm_once() {
        let (mut runtime, frame, iframe) = javascript_restore_runtime();
        let old_document = runtime.host_state.borrow().iframe_documents[&iframe]
            .document
            .identity();
        runtime
            .eval("proxy.control.i.setFormValue('submitted','saved')")
            .unwrap();
        let restore = captured_restore(&mut runtime, iframe);
        runtime
            .eval("proxy.control.i.setFormValue('changed','changed')")
            .unwrap();

        run_javascript_restore(
            &mut runtime,
            &frame,
            iframe,
            "javascript:void 0".to_owned(),
            restore,
        );

        assert_eq!(
            runtime.host_state.borrow().iframe_documents[&iframe]
                .document
                .identity(),
            old_document
        );
        // Restoration uses a DOM-manipulation task even when javascript:
        // keeps the existing Document. It must run once at the next task pump.
        assert_eq!(
            runtime
                .eval("proxy.restorations.length")
                .unwrap()
                .as_number(),
            Some(0.0)
        );
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime
                .eval("JSON.stringify(proxy.restorations)+'|'+new proxy.FormData(proxy.control.i.form).get('answer')")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "[[\"saved\",\"restore\"]]|saved"
        );
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime
                .eval("proxy.restorations.length")
                .unwrap()
                .as_number(),
            Some(1.0)
        );
    }

    #[test]
    fn javascript_string_history_restore_follows_the_replacement_document() {
        let (mut runtime, frame, iframe) = javascript_restore_runtime();
        let old_document = runtime.host_state.borrow().iframe_documents[&iframe]
            .document
            .identity();
        runtime
            .eval("proxy.control.i.setFormValue('submitted','saved replacement')")
            .unwrap();
        let restore = captured_restore(&mut runtime, iframe);
        let replacement = r#"<!doctype html><form><x-restored name=answer></x-restored></form><script>
            globalThis.restorations=[];
            customElements.define('x-restored', class extends HTMLElement {
                static formAssociated=true;
                constructor(){super();this.i=this.attachInternals();this.i.setFormValue('initial');}
                formStateRestoreCallback(value,mode){restorations.push([value,mode]);this.i.setFormValue(value,value);}
            });
            globalThis.control=document.querySelector('x-restored');
        </script>"#;
        run_javascript_restore(
            &mut runtime,
            &frame,
            iframe,
            format!("javascript:{}", serde_json::to_string(replacement).unwrap()),
            restore,
        );
        runtime.run_until_idle().unwrap();

        assert_ne!(
            runtime.host_state.borrow().iframe_documents[&iframe]
                .document
                .identity(),
            old_document
        );
        assert_eq!(
            runtime
                .eval("JSON.stringify(proxy.restorations)+'|'+new proxy.FormData(proxy.control.i.form).get('answer')")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "[[\"saved replacement\",\"restore\"]]|saved replacement"
        );
    }

    #[test]
    fn stale_javascript_epoch_neither_evaluates_nor_restores() {
        let (mut runtime, frame, iframe) = javascript_restore_runtime();
        runtime
            .eval("proxy.control.i.setFormValue('submitted','stale saved')")
            .unwrap();
        let restore = captured_restore(&mut runtime, iframe);
        let source_document = runtime.host_state.borrow().document.identity();
        let (stale_epoch, client) = {
            let mut state = runtime.host_state.borrow_mut();
            let stale = state
                .reserve_iframe_navigation_from(
                    &frame,
                    IframeNavigationClass::Foreground,
                    source_document,
                )
                .unwrap();
            assert!(state.attach_iframe_history_restore(iframe, stale, restore));
            state
                .reserve_iframe_navigation_from(
                    &frame,
                    IframeNavigationClass::Foreground,
                    source_document,
                )
                .unwrap();
            (stale, state.navigation_security_snapshot(source_document))
        };
        let request = form_submission::Submission {
            url: "javascript:parent.staleJavascriptRan=true".to_owned(),
            method: "GET".to_owned(),
            body: None,
            content_type: None,
            source_document_id: Some(source_document),
        };
        runtime
            .run_javascript_iframe_form_submission(
                &frame,
                iframe,
                stale_epoch,
                &request,
                source_document,
                client.as_ref(),
            )
            .unwrap();
        assert_eq!(
            runtime
                .eval(
                    "globalThis.staleJavascriptRan === undefined && proxy.restorations.length === 0"
                )
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }
}

/// History operations require a fully active Document, including reads.
/// Construct the exception after releasing the host-state borrow.
fn ensure_history_document_active(
    document: usize,
    operation: Option<&JsValue>,
    context: &mut Context,
) -> JsResult<()> {
    let history_operation = operation
        .and_then(JsValue::as_string)
        .is_some_and(|operation| {
            matches!(
                operation.to_std_string_escaped().as_str(),
                "length"
                    | "state"
                    | "scroll-restoration"
                    | "set-scroll-restoration"
                    | "push-state"
                    | "replace-state"
                    | "go"
            )
        });
    if history_operation && !with_host_state(|host| Ok(host.borrow().document_is_active(document)))?
    {
        return Err(cross_origin_access_error(context)?);
    }
    Ok(())
}
