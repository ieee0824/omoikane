//! Owned document image assets and CSP/client snapshots for short traversals.
use super::*;
use crate::js::csp::{CspPolicy, PolicyViolation, ResourceType};
use std::collections::HashSet;
use std::sync::Weak;
use std::sync::atomic::{AtomicBool, Ordering};

mod async_fetch;
pub(super) use async_fetch::start as start_async_image;
pub(crate) use async_fetch::{pending as image_asset_pending, poll as poll_async_images};

type ReportedChecks = Rc<RefCell<HashSet<(String, String, usize)>>>;

/// Images and notification identities live exactly as long as their Document.
#[derive(Default)]
pub(crate) struct ImageDocumentState {
    policy: Option<CspPolicy>,
    cache: ImageCache,
    reported: ReportedChecks,
    asynchronous: async_fetch::State,
}

#[derive(Default)]
struct ImageCache {
    images: HashMap<String, Option<Image>>,
    animations: HashMap<String, Arc<crate::paint::ImageAnimation>>,
    owner: Option<ImageCacheOwner>,
}

impl ImageCache {
    fn swap_with_thread(self) -> Self {
        Self {
            images: IMAGE_CACHE.with(|slot| slot.replace(self.images)),
            animations: IMAGE_ANIMATION_CACHE.with(|slot| slot.replace(self.animations)),
            owner: IMAGE_CACHE_OWNER.with(|slot| slot.replace(self.owner)),
        }
    }
}

/// A checked image request, returned for delivery by the owning JS runtime.
#[derive(Debug)]
pub(crate) struct ImagePolicyNotice {
    pub(crate) reference: String,
    pub(crate) enforced: Vec<PolicyViolation>,
    pub(crate) report_only: Vec<PolicyViolation>,
}

struct ActiveImagePolicy {
    state: Rc<RefCell<ImageDocumentState>>,
    policy: CspPolicy,
    report_cached: bool,
    notices: Rc<RefCell<Vec<ImagePolicyNotice>>>,
    reported: ReportedChecks,
}

thread_local! {
    static ACTIVE_POLICY: RefCell<Option<ActiveImagePolicy>> = const { RefCell::new(None) };
    static IMAGE_REQUEST_LIVENESS: RefCell<Option<Weak<AtomicBool>>> = const { RefCell::new(None) };
}

/// Uses an owned cancellation token during a worker's image fetch and decode.
/// Restores the previous token on return or unwind; transport I/O is not interrupted.
pub(crate) fn with_image_request_liveness<T>(
    live: Weak<AtomicBool>,
    operation: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<Weak<AtomicBool>>);

    impl Drop for Restore {
        fn drop(&mut self) {
            IMAGE_REQUEST_LIVENESS.with(|slot| slot.replace(self.0.take()));
        }
    }

    let _restore = Restore(IMAGE_REQUEST_LIVENESS.with(|slot| slot.replace(Some(live))));
    operation()
}

/// Returns whether the active request still belongs to a live Document context.
/// Image operations outside a cancellation scope remain enabled.
pub(crate) fn image_request_is_live() -> bool {
    IMAGE_REQUEST_LIVENESS.with(|slot| {
        slot.borrow().as_ref().is_none_or(|live| {
            live.upgrade()
                .is_some_and(|live| live.load(Ordering::Acquire))
        })
    })
}

struct ImagePolicyGuard {
    previous_policy: Option<ActiveImagePolicy>,
    previous_client: Option<Client>,
    previous_cache: Option<ImageCache>,
}

impl Drop for ImagePolicyGuard {
    fn drop(&mut self) {
        let active = ACTIVE_POLICY.with(|slot| slot.replace(self.previous_policy.take()));
        if let (Some(previous), Some(active)) = (self.previous_cache.take(), active) {
            let cache = previous.swap_with_thread();
            let mut state = active.state.borrow_mut();
            state.cache = cache;
            state.policy = Some(active.policy);
            state.reported = active.reported;
        }
        if let Some(client) = self.previous_client.take() {
            HTTP_CLIENT.with(|slot| slot.replace(client));
        }
    }
}

/// Runs real image resolution with explicit document CSP and transport policy.
/// The owned state isolates decoded assets and notification identities between
/// Documents; no RefCell borrow remains held during decoding or network I/O.
/// Returns notices for the caller to deliver after the traversal has finished.
pub(crate) fn with_image_security_policy<T>(
    state: Rc<RefCell<ImageDocumentState>>,
    policy: CspPolicy,
    client: Client,
    report_cached: bool,
    operation: impl FnOnce() -> T,
) -> (T, Vec<ImagePolicyNotice>) {
    let shared = ACTIVE_POLICY.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|active| Rc::ptr_eq(&active.state, &state) && active.policy == policy)
    });
    let (previous_cache, reported) = {
        let mut state = state.borrow_mut();
        if state.policy.as_ref() != Some(&policy) {
            state.cache = ImageCache::default();
            state.asynchronous.reset();
            state.reported = ReportedChecks::default();
            state.policy = Some(policy.clone());
        }
        let previous = if shared {
            None
        } else {
            Some(std::mem::take(&mut state.cache).swap_with_thread())
        };
        (previous, Rc::clone(&state.reported))
    };
    let notices = Rc::new(RefCell::new(Vec::new()));
    let previous_policy = ACTIVE_POLICY.with(|slot| {
        slot.replace(Some(ActiveImagePolicy {
            state,
            policy,
            report_cached,
            notices: Rc::clone(&notices),
            reported,
        }))
    });
    let previous_client = HTTP_CLIENT.with(|slot| slot.replace(client));
    let _guard = ImagePolicyGuard {
        previous_policy,
        previous_client: Some(previous_client),
        previous_cache,
    };
    let result = operation();
    let reports = std::mem::take(&mut *notices.borrow_mut());
    (result, reports)
}

pub(super) fn allows_image_request(reference: &str, redirects: usize, initial: &str) -> bool {
    if !image_request_is_live() {
        return false;
    }
    ACTIVE_POLICY.with(|slot| {
        let slot = slot.borrow();
        let Some(active) = slot.as_ref() else {
            return true;
        };
        let enforced = active.policy.blocking_policy_texts_after_redirects(
            ResourceType::Image,
            reference,
            redirects,
        );
        let report_only = active.policy.report_only_policy_texts_after_redirects(
            ResourceType::Image,
            reference,
            redirects,
        );
        let allowed = enforced.is_empty();
        if !enforced.is_empty() || !report_only.is_empty() {
            let fresh = active.reported.borrow_mut().insert((
                initial.to_owned(),
                reference.to_owned(),
                redirects,
            ));
            if active.report_cached || fresh {
                active.notices.borrow_mut().push(ImagePolicyNotice {
                    reference: initial.to_owned(),
                    enforced,
                    report_only,
                });
            }
        }
        allowed
    })
}
