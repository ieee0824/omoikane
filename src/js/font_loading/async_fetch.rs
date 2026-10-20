//! Owned font I/O and decoding on the shared resource queue.
use super::*;
#[cfg(test)]
mod tests;
use crate::resource_worker::{
    CancellationToken, ResourceKind, ResourceOutcome, ResourcePriority, ResourceTask,
};

#[derive(Clone)]
pub(super) struct Completion {
    pub loaded: TimerPayload,
    pub failed: TimerPayload,
}

pub(super) struct Pending {
    document: usize,
    task: ResourceTask<Response>,
    completion: Completion,
    source: Option<security_reporting::CspSourceLocation>,
}

struct Notice {
    report_reference: String,
    report_only: Vec<super::super::csp::PolicyViolation>,
    enforced: Vec<super::super::csp::PolicyViolation>,
}

struct Response {
    font: Result<Arc<Font>, String>,
    notices: Vec<Notice>,
}

struct Request {
    bytes: Option<Vec<u8>>,
    sources: Vec<(String, String)>,
    base: Option<crate::http::Url>,
    policy: CspPolicy,
    client: Client,
    cookies: crate::http::CookieRequestContext,
    origin: crate::http::cors::Origin,
    notices: Vec<Notice>,
}

impl Request {
    fn run(mut self, token: CancellationToken) -> Response {
        let font = self.load(&token).map(Arc::new);
        Response {
            font,
            notices: self.notices,
        }
    }

    fn load(&mut self, token: &CancellationToken) -> Result<Font, String> {
        if token.is_cancelled() {
            return Err("font request cancelled".into());
        }
        if let Some(bytes) = self.bytes.take() {
            return Font::load_from_bytes(bytes).map_err(|error| error.to_string());
        }
        for (kind, source) in self.sources.clone() {
            if token.is_cancelled() {
                return Err("font request cancelled".into());
            }
            let font = if kind == "local" {
                crate::font::load_system_font(&source).map_err(|error| error.to_string())
            } else {
                self.bytes_for(&source, token).and_then(|bytes| {
                    if token.is_cancelled() {
                        return Err("font request cancelled".into());
                    }
                    Font::load_from_bytes(bytes).map_err(|error| error.to_string())
                })
            };
            if let Ok(font) = font {
                return Ok(font);
            }
        }
        Err("no font source could be loaded".into())
    }

    fn check(&mut self, reference: &str, redirects: usize, initial: &str) -> Result<(), String> {
        let allowed =
            self.policy
                .allows_reference_after_redirects(ResourceType::Font, reference, redirects);
        let report_only = self.policy.report_only_policy_texts_after_redirects(
            ResourceType::Font,
            reference,
            redirects,
        );
        let enforced = if allowed {
            Vec::new()
        } else {
            self.policy.blocking_policy_texts_after_redirects(
                ResourceType::Font,
                reference,
                redirects,
            )
        };
        if !report_only.is_empty() || !enforced.is_empty() {
            self.notices.push(Notice {
                report_reference: initial.to_owned(),
                report_only,
                enforced,
            });
        }
        if allowed {
            Ok(())
        } else {
            Err("font request blocked by font-src".into())
        }
    }

    fn bytes_for(&mut self, source: &str, token: &CancellationToken) -> Result<Vec<u8>, String> {
        let data = source
            .get(..5)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"));
        let reference = if data {
            source.to_owned()
        } else if let Ok(url) = source.parse::<crate::http::Url>() {
            url.to_string()
        } else {
            crate::http::url::resolve_url(self.base.as_ref().ok_or("font URL has no base")?, source)
                .map_err(|error| error.to_string())?
                .to_string()
        };
        let bytes = if data {
            self.check(&reference, 0, &reference)?;
            let bytes = crate::http::parse_data_uri(&reference)
                .ok_or("invalid font data URL")?
                .data;
            self.check(&reference, 0, &reference)?;
            bytes
        } else {
            self.fetch(&reference, token)?
        };
        if bytes.len() > 10_000_000 {
            return Err("font exceeds the decoded font size limit".into());
        }
        Ok(bytes)
    }

    fn fetch(&mut self, reference: &str, token: &CancellationToken) -> Result<Vec<u8>, String> {
        let mut url = reference
            .parse::<crate::http::Url>()
            .map_err(|error| error.to_string())?;
        let mut redirects = 0;
        let mut crossed_origin = false;
        let mut preflight = crate::http::cors::PreflightCache::default();
        let response = loop {
            if token.is_cancelled() {
                return Err("font request cancelled".into());
            }
            self.check(&url.to_string(), redirects, reference)?;
            let mut request = HttpRequest::new(Method::Get, url.clone());
            self.cookies.apply(&mut request);
            if crossed_origin {
                request.set_storage_access_eligible(false);
            }
            let response = crate::http::cors::fetch(
                &mut self.client,
                request,
                &self.origin,
                RequestMode::Cors,
                CredentialsMode::SameOrigin,
                RedirectMode::Manual,
                &mut preflight,
            )
            .map_err(|error| error.to_string())?
            .response;
            if token.is_cancelled() {
                return Err("font request cancelled".into());
            }
            if !matches!(response.status_code(), 301 | 302 | 303 | 307 | 308) {
                break response;
            }
            if redirects == 20 {
                return Err("too many font redirects".into());
            }
            let next = crate::http::url::resolve_url(
                &url,
                response
                    .header("location")
                    .ok_or("font redirect has no location")?,
            )
            .map_err(|error| error.to_string())?;
            crossed_origin |= !same_origin_url(&url, &next);
            url = next;
            redirects += 1;
        };
        let response_url = response.effective_url().unwrap_or(&url).to_string();
        self.check(&response_url, redirects, reference)?;
        if !(200..300).contains(&response.status_code()) {
            return Err("font HTTP request failed".into());
        }
        Ok(response.body().to_vec())
    }
}

pub(super) fn begin(
    state: &mut HostState,
    document: &NodeHandle,
    id: u64,
    completion: Completion,
    source: Option<security_reporting::CspSourceLocation>,
) -> JsResult<bool> {
    if !state.document_is_active(document.identity()) {
        return Err(error("font document is no longer live"));
    }
    let face = state
        .font_loading
        .faces
        .get(&id)
        .ok_or_else(|| error("unknown font"))?;
    if let Some(failure) = &face.failure {
        return Err(error(failure));
    }
    if face.font.is_some() {
        return Ok(false);
    }
    if state.font_loading.pending.contains_key(&id) {
        return Ok(true);
    }
    let mut request = Request {
        bytes: face.bytes.clone(),
        sources: face.sources.clone(),
        base: face.base.clone(),
        policy: state.csp_policy_for_document(document),
        client: state.http_client.fork_for_worker(),
        cookies: state.document_cookie_request_context(document.identity()),
        origin: script_errors::document_fetch_origin(state, document.identity()),
        notices: Vec::new(),
    };
    request
        .client
        .set_shared_cookie_store(Arc::clone(&state.cookie_store));
    let scope = state
        .font_loading
        .scopes
        .entry(document.identity())
        .or_default();
    let task = state
        .resource_queue
        .submit(
            scope,
            ResourcePriority::Critical,
            ResourceKind::Font,
            move |token| request.run(token),
        )
        .map_err(error)?;
    state.font_loading.pending.insert(
        id,
        Pending {
            document: document.identity(),
            task,
            completion,
            source,
        },
    );
    Ok(true)
}

pub(super) fn poll(state: &mut HostState) {
    let finished: Vec<_> = state
        .font_loading
        .pending
        .iter()
        .filter_map(|(id, pending)| match pending.task.try_recv() {
            Ok(outcome) => Some((*id, outcome)),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some((*id, ResourceOutcome::Panicked))
            }
        })
        .collect();
    for (id, outcome) in finished {
        let pending = state.font_loading.pending.get(&id).unwrap();
        if !state.document_is_active(pending.document) {
            state.font_loading.pending.remove(&id);
            continue;
        }
        let delivery = Delivery {
            document: pending.document,
            completion: pending.completion.clone(),
            source: pending.source.clone(),
        };
        let response = match outcome {
            ResourceOutcome::Complete(response) => response,
            ResourceOutcome::Cancelled => Response {
                font: Err("font request cancelled".into()),
                notices: Vec::new(),
            },
            ResourceOutcome::Panicked => Response {
                font: Err("font worker failed".into()),
                notices: Vec::new(),
            },
        };
        // Keep both completion closures visible to HostState's GC tracer
        // until the chosen callback is rooted in the event loop.
        apply(state, id, delivery, response);
        state.font_loading.pending.remove(&id);
    }
}

struct Delivery {
    document: usize,
    completion: Completion,
    source: Option<security_reporting::CspSourceLocation>,
}

fn apply(state: &mut HostState, id: u64, pending: Delivery, response: Response) {
    let Some(document) = state.get_node(pending.document) else {
        return;
    };
    for notice in response.notices {
        state.queue_policy_violation_events_with_source(
            pending.document,
            ResourceType::Font,
            &notice.report_reference,
            notice.report_only,
            "report",
            pending.source.as_ref(),
        );
        let blocked = !notice.enforced.is_empty();
        state.queue_policy_violation_events_with_source(
            pending.document,
            ResourceType::Font,
            &notice.report_reference,
            notice.enforced,
            "enforce",
            pending.source.as_ref(),
        );
        if blocked {
            state.record_csp_violation_history(
                &document,
                ResourceType::Font,
                notice.report_reference,
            );
        }
    }
    let Some(face) = state.font_loading.faces.get_mut(&id) else {
        return;
    };
    face.bytes = None;
    let loaded = response.font.is_ok();
    match response.font {
        Ok(font) => face.font = Some(font),
        Err(failure) => face.failure = Some(failure),
    }
    let members: Vec<_> = face.documents.iter().copied().collect();
    state.mark_document_style_dirty(&document);
    for member in members {
        if let Some(document) = state.get_node(member) {
            state.mark_document_style_dirty(&document);
        }
    }
    let payload = if loaded {
        pending.completion.loaded
    } else {
        pending.completion.failed
    };
    state
        .event_loop
        .enqueue_font_loading(payload, pending.document);
}

pub(super) fn capture_completion(
    value: Option<&JsValue>,
    context: &mut Context,
) -> JsResult<Option<Completion>> {
    let Some(value) = value.filter(|value| !value.is_undefined()) else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .ok_or_else(|| error("invalid font completion callbacks"))?;
    let loaded = object.get(0_u32, context)?;
    let failed = object.get(1_u32, context)?;
    if !loaded.is_callable() || !failed.is_callable() {
        return Err(error("invalid font completion callbacks"));
    }
    Ok(Some(Completion {
        loaded: bind_timer_payload_to_current_realm(
            context,
            TimerPayload::Callback {
                callback: loaded,
                args: Vec::new(),
            },
        ),
        failed: bind_timer_payload_to_current_realm(
            context,
            TimerPayload::Callback {
                callback: failed,
                args: Vec::new(),
            },
        ),
    }))
}

pub(super) unsafe fn trace_pending(store: &FontStore, tracer: &mut Tracer) {
    for pending in store.pending.values() {
        unsafe { event_loop::trace_timer_payload(&pending.completion.loaded, tracer) };
        unsafe { event_loop::trace_timer_payload(&pending.completion.failed, tracer) };
    }
}

pub(super) fn retire(store: &mut FontStore, document: usize) {
    store.scopes.remove(&document);
    store
        .pending
        .retain(|_, pending| pending.document != document);
}
