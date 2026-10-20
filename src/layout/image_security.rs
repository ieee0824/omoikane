//! Owned document image assets and CSP/client snapshots for short traversals.
use super::*;
use crate::js::csp::{CspPolicy, PolicyViolation, ResourceType};
use std::collections::HashSet;
use std::sync::Weak;
use std::sync::atomic::{AtomicBool, Ordering};

mod async_fetch;
pub(super) use async_fetch::start as start_async_image;
pub(crate) use async_fetch::{
    pending as image_asset_pending, poll as poll_async_images,
    poll_effects as poll_async_image_effects,
};

type ReportedChecks = Rc<RefCell<HashSet<(String, String, usize)>>>;

/// Images and notification identities live exactly as long as their Document.
#[derive(Default)]
pub(crate) struct ImageDocumentState {
    policy: Option<CspPolicy>,
    cache: ImageCache,
    layout_dependencies: ImageLayoutDependencies,
    reported: ReportedChecks,
    asynchronous: async_fetch::State,
}

#[derive(Default)]
struct ImageLayoutDependencies {
    owner: Option<ImageCacheOwner>,
    references: Option<HashSet<String>>,
    revision: u64,
}

impl ImageLayoutDependencies {
    fn select_owner(&mut self, owner: Option<&ImageCacheOwner>) {
        if !same_optional_image_cache_owner(self.owner.as_ref(), owner) {
            self.owner = owner.cloned();
            self.references = None;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    fn reset(&mut self) {
        self.owner = None;
        self.references = None;
        self.revision = self.revision.wrapping_add(1);
    }

    fn begin(&mut self, owner: &ImageCacheOwner) -> u64 {
        self.select_owner(Some(owner));
        // A failed or unwound rebuild must not publish partial coverage.
        self.references = None;
        self.revision
    }

    fn commit(&mut self, owner: &ImageCacheOwner, revision: u64, references: HashSet<String>) {
        if self.revision == revision
            && self
                .owner
                .as_ref()
                .is_some_and(|current| same_image_cache_owner(current, owner))
        {
            self.references = Some(references);
        }
    }

    fn abort(&mut self, owner: &ImageCacheOwner, revision: u64) {
        if self.revision == revision
            && self
                .owner
                .as_ref()
                .is_some_and(|current| same_image_cache_owner(current, owner))
        {
            self.references = None;
        }
    }

    fn completion_requires_layout(
        &self,
        owner: Option<&ImageCacheOwner>,
        affected: &HashSet<String>,
    ) -> bool {
        let Some((owner, references)) = owner.zip(self.references.as_ref()) else {
            return true;
        };
        self.owner
            .as_ref()
            .is_none_or(|covered| !same_image_cache_owner(covered, owner))
            || affected
                .iter()
                .any(|reference| references.contains(reference))
    }
}

struct ActiveLayoutDependencyCapture {
    state: Rc<RefCell<ImageDocumentState>>,
    owner: ImageCacheOwner,
    revision: u64,
    references: HashSet<String>,
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
    static ACTIVE_LAYOUT_DEPENDENCY_CAPTURES: RefCell<Vec<ActiveLayoutDependencyCapture>> = const { RefCell::new(Vec::new()) };
}

/// Compares every transport and security field that partitions image state.
pub(super) fn same_image_cache_owner(left: &ImageCacheOwner, right: &ImageCacheOwner) -> bool {
    Arc::ptr_eq(&left.0, &right.0)
        && left.1 == right.1
        && left.2 == right.2
        && left.3 == right.3
        && left.4 == right.4
        && left.5 == right.5
}

fn same_optional_image_cache_owner(
    left: Option<&ImageCacheOwner>,
    right: Option<&ImageCacheOwner>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => same_image_cache_owner(left, right),
        (None, None) => true,
        _ => false,
    }
}

/// Forgets layout coverage when the active image cache selects a new owner.
pub(super) fn synchronize_layout_dependency_owner(owner: Option<&ImageCacheOwner>) {
    let state = ACTIVE_POLICY.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|active| Rc::clone(&active.state))
    });
    if let Some(state) = state {
        state.borrow_mut().layout_dependencies.select_owner(owner);
    }
}

/// Records one canonical HTTP image reference read by an active cached layout.
pub(super) fn record_layout_image_dependency(reference: &str) {
    ACTIVE_LAYOUT_DEPENDENCY_CAPTURES.with(|stack| {
        let mut stack = stack.borrow_mut();
        if stack.is_empty() {
            return;
        }
        ACTIVE_POLICY.with(|policy| {
            let policy = policy.borrow();
            let Some(active) = policy.as_ref() else {
                return;
            };
            IMAGE_CACHE_OWNER.with(|owner| {
                let owner = owner.borrow();
                let Some(owner) = owner.as_ref() else {
                    return;
                };
                for capture in stack.iter_mut() {
                    if Rc::ptr_eq(&capture.state, &active.state)
                        && same_image_cache_owner(&capture.owner, owner)
                        && !capture.references.contains(reference)
                    {
                        capture.references.insert(reference.to_owned());
                    }
                }
            });
        });
    });
}

struct LayoutDependencyCaptureGuard {
    active: bool,
}

impl LayoutDependencyCaptureGuard {
    fn finish(mut self, success: bool) {
        let capture = self.pop();
        let mut dependencies = capture.state.borrow_mut();
        if success {
            dependencies.layout_dependencies.commit(
                &capture.owner,
                capture.revision,
                capture.references,
            );
        } else {
            dependencies
                .layout_dependencies
                .abort(&capture.owner, capture.revision);
        }
    }

    fn pop(&mut self) -> ActiveLayoutDependencyCapture {
        self.active = false;
        ACTIVE_LAYOUT_DEPENDENCY_CAPTURES.with(|stack| {
            stack
                .borrow_mut()
                .pop()
                .expect("layout image dependency capture stack underflow")
        })
    }
}

impl Drop for LayoutDependencyCaptureGuard {
    fn drop(&mut self) {
        if self.active {
            let capture = self.pop();
            capture
                .state
                .borrow_mut()
                .layout_dependencies
                .abort(&capture.owner, capture.revision);
        }
    }
}

/// Captures image reads for one cached full-Document layout.
///
/// The closure returns whether it produced a complete layout. Only a complete
/// result replaces the Document's previous dependency coverage.
pub(crate) fn with_image_layout_dependency_capture<T>(operation: impl FnOnce() -> (T, bool)) -> T {
    ensure_image_cache_context();
    let capture = ACTIVE_POLICY.with(|slot| {
        let slot = slot.borrow();
        let active = slot.as_ref()?;
        let owner = IMAGE_CACHE_OWNER.with(|owner| owner.borrow().clone())?;
        Some(ActiveLayoutDependencyCapture {
            state: Rc::clone(&active.state),
            owner,
            revision: 0,
            references: HashSet::new(),
        })
    });
    let Some(mut capture) = capture else {
        return operation().0;
    };
    capture.revision = capture
        .state
        .borrow_mut()
        .layout_dependencies
        .begin(&capture.owner);
    ACTIVE_LAYOUT_DEPENDENCY_CAPTURES.with(|stack| stack.borrow_mut().push(capture));
    let guard = LayoutDependencyCaptureGuard { active: true };
    let (result, success) = operation();
    guard.finish(success);
    result
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
            state.layout_dependencies.reset();
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
