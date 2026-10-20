//! Owned HTTP/decode jobs. DOM handles and Boa values stay on the GUI thread.
use super::*;
use crate::resource_worker::{
    ResourceKind, ResourceOutcome, ResourcePriority, ResourceQueue, ResourceScope, ResourceTask,
};
use std::sync::atomic::AtomicBool;
use std::sync::{Weak, mpsc};

#[cfg(test)]
mod tests;

pub(super) struct State {
    enabled: bool,
    scope: ResourceScope,
    owner: Option<ImageCacheOwner>,
    pending: HashMap<String, ResourceTask<Response>>,
    pool: ResourceQueue,
}

impl Default for State {
    fn default() -> Self {
        Self {
            enabled: false,
            scope: ResourceScope::default(),
            owner: None,
            pending: HashMap::new(),
            pool: ResourceQueue::default(),
        }
    }
}

impl State {
    pub fn reset(&mut self) {
        let enabled = self.enabled;
        let pool = self.pool.clone();
        *self = Self::default();
        self.enabled = enabled;
        self.pool = pool;
    }
}

impl ImageDocumentState {
    /// Shares resource execution slots with fonts and other document loaders.
    pub(crate) fn set_resource_queue(&mut self, queue: ResourceQueue) {
        self.asynchronous.pool = queue;
    }

    /// Selects nonblocking network images for a native interactive host.
    pub(crate) fn set_async_loading(&mut self, enabled: bool) {
        self.asynchronous.enabled = enabled;
    }

    /// Reports outstanding network/decode work without polling or blocking.
    pub(crate) fn has_pending_images(&self) -> bool {
        !self.asynchronous.pending.is_empty()
    }

    /// Checks whether an already resolved URL is still being downloaded.
    pub(crate) fn is_image_pending(&self, url: &str) -> bool {
        self.asynchronous.pending.contains_key(url)
    }
}

fn same_owner(left: &ImageCacheOwner, right: &ImageCacheOwner) -> bool {
    Arc::ptr_eq(&left.0, &right.0)
        && left.1 == right.1
        && left.2 == right.2
        && left.3 == right.3
        && left.4 == right.4
        && left.5 == right.5
}

/// Starts a checked download only in an explicitly asynchronous document scope.
pub(crate) fn start(url: &str) -> bool {
    ACTIVE_POLICY.with(|slot| {
        let slot = slot.borrow();
        let Some(active) = slot.as_ref() else {
            return false;
        };
        let owner = IMAGE_CACHE_OWNER.with(|slot| slot.borrow().clone());
        let Some(owner) = owner else { return false };
        let mut state = active.state.borrow_mut();
        let asynchronous = &mut state.asynchronous;
        if !asynchronous.enabled && asynchronous.pending.is_empty() {
            return false;
        }
        if !asynchronous
            .owner
            .as_ref()
            .is_some_and(|old| same_owner(old, &owner))
        {
            asynchronous.reset();
            asynchronous.owner = Some(owner.clone());
        }
        if asynchronous.pending.contains_key(url) {
            return true;
        }
        if !asynchronous.enabled {
            return false;
        }
        let request = Request {
            url: url.to_owned(),
            base: current_image_base_url(),
            blobs: crate::data::current_blob_url_store(),
            reported: active.reported.borrow().clone(),
            policy: active.policy.clone(),
            client: HTTP_CLIENT.with(|slot| slot.borrow().fork_for_worker()),
            owner,
            live: asynchronous.scope.weak_liveness(),
        };
        match request.start(asynchronous) {
            Ok(receiver) => {
                asynchronous.pending.insert(url.to_owned(), receiver);
            }
            Err(_) => {
                IMAGE_CACHE.with(|slot| slot.borrow_mut().insert(url.to_owned(), None));
            }
        }
        true
    })
}

/// Merges completed jobs under the same document/security traversal scope.
pub(crate) fn poll() -> bool {
    ensure_image_cache_context();
    ACTIVE_POLICY.with(|slot| {
        let slot = slot.borrow();
        let Some(active) = slot.as_ref() else {
            return false;
        };
        let owner = IMAGE_CACHE_OWNER.with(|slot| slot.borrow().clone());
        let mut state = active.state.borrow_mut();
        if !state.asynchronous.pending.is_empty()
            && !owner
                .as_ref()
                .zip(state.asynchronous.owner.as_ref())
                .is_some_and(|(current, requested)| same_owner(current, requested))
        {
            // A storage-access or embedder-policy change selects a new cache
            // context. Discard even failed completions from the old context.
            state.asynchronous.reset();
            return true;
        }
        let mut completed = Vec::new();
        state
            .asynchronous
            .pending
            .retain(|url, receiver| match receiver.try_recv() {
                Ok(ResourceOutcome::Complete(response)) => {
                    completed.push((url.clone(), Some(response)));
                    false
                }
                Ok(ResourceOutcome::Cancelled | ResourceOutcome::Panicked)
                | Err(mpsc::TryRecvError::Disconnected) => {
                    completed.push((url.clone(), None));
                    false
                }
                Err(mpsc::TryRecvError::Empty) => true,
            });
        let mut changed = false;
        for (url, response) in completed {
            let Some(response) = response else {
                IMAGE_CACHE.with(|slot| slot.borrow_mut().insert(url, None));
                changed = true;
                continue;
            };
            if !owner
                .as_ref()
                .zip(response.cache.owner.as_ref())
                .is_some_and(|(current, fetched)| same_owner(current, fetched))
            {
                continue;
            }
            IMAGE_CACHE.with(|slot| slot.borrow_mut().extend(response.cache.images));
            IMAGE_ANIMATION_CACHE.with(|slot| slot.borrow_mut().extend(response.cache.animations));
            active.reported.borrow_mut().extend(response.reported);
            active.notices.borrow_mut().extend(response.csp);
            IMAGE_COOKIE_CONTEXT.with(|slot| {
                if let Some(context) = slot.borrow().as_ref() {
                    context.coep_reports.borrow_mut().extend(response.coep);
                }
            });
            changed = true;
        }
        changed
    })
}

pub(crate) fn pending(url: &str) -> bool {
    ACTIVE_POLICY.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|active| active.state.borrow().asynchronous.pending.contains_key(url))
    })
}

pub(super) struct Request {
    pub url: String,
    pub base: Option<Url>,
    pub blobs: crate::data::BlobUrlStore,
    pub reported: HashSet<(String, String, usize)>,
    pub policy: CspPolicy,
    pub client: Client,
    pub owner: ImageCacheOwner,
    pub live: Weak<AtomicBool>,
}

pub(super) struct Response {
    pub cache: ImageCache,
    pub reported: HashSet<(String, String, usize)>,
    pub csp: Vec<ImagePolicyNotice>,
    pub coep: Vec<CorpViolation>,
}

impl Request {
    pub fn start(self, state: &mut State) -> Result<ResourceTask<Response>, &'static str> {
        state
            .pool
            .submit(
                &state.scope,
                ResourcePriority::Normal,
                ResourceKind::Image,
                move |_| self.run(),
            )
            .map_err(|_| "resource workers unavailable")
    }

    fn run(self) -> Response {
        let state = Rc::new(RefCell::new(ImageDocumentState::default()));
        state.borrow_mut().policy = Some(self.policy.clone());
        state.borrow().reported.borrow_mut().extend(self.reported);
        let (store, cookies, document, origin, embedder, report_only) = self.owner;
        let ((_, coep), csp) = with_image_request_liveness(self.live, || {
            with_image_security_policy(Rc::clone(&state), self.policy, self.client, false, || {
                with_image_cookie_store_reporting(
                    store,
                    cookies,
                    document,
                    origin,
                    embedder,
                    report_only,
                    || {
                        crate::data::with_blob_url_store(self.blobs, || {
                            with_image_base_url(self.base, || {
                                super::super::decode_or_fetch_image_asset(&self.url, None)
                            })
                        })
                    },
                )
            })
        });
        let cache = std::mem::take(&mut state.borrow_mut().cache);
        let reported = std::mem::take(&mut *state.borrow().reported.borrow_mut());
        Response {
            cache,
            reported,
            csp,
            coep,
        }
    }
}
