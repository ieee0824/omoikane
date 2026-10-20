//! Opt-in counters for node computed-style resolution during one layout attempt.
use std::{cell::RefCell, marker::PhantomData, rc::Rc, sync::OnceLock, time::Instant};

#[derive(Default)]
struct Profile {
    cache_hits: usize,
    cache_misses: usize,
    compute_depth: usize,
    outer_compute_ms: f64,
}

thread_local! {
    static PROFILES: RefCell<Vec<Profile>> = const { RefCell::new(Vec::new()) };
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_PAINT").is_some())
}

/// Owns a thread-local profiling scope; it never borrows the style resolver.
pub(crate) struct StyleProfileScope {
    started: Option<Instant>,
    document_id: usize,
    _owner_thread: PhantomData<Rc<()>>,
}

impl StyleProfileScope {
    pub(crate) fn begin(document_id: usize) -> Self {
        Self::begin_if(enabled(), document_id)
    }

    fn begin_if(enabled: bool, document_id: usize) -> Self {
        if enabled {
            PROFILES.with(|profiles| profiles.borrow_mut().push(Profile::default()));
        }
        Self {
            started: enabled.then(Instant::now),
            document_id,
            _owner_thread: PhantomData,
        }
    }
}

impl Drop for StyleProfileScope {
    fn drop(&mut self) {
        let Some(started) = self.started else { return };
        let profile = PROFILES.with(|profiles| profiles.borrow_mut().pop());
        if let Some(profile) = profile {
            eprintln!(
                "OMOIKANE_LAYOUT_STYLE {}",
                serde_json::json!({
                    "document_id": self.document_id,
                    "scope_ms": started.elapsed().as_secs_f64() * 1000.0,
                    "cache_hits": profile.cache_hits,
                    "cache_misses": profile.cache_misses,
                    "outer_compute_ms": profile.outer_compute_ms,
                })
            );
        }
    }
}

pub(super) fn record_cache_hit() {
    record_cache_hit_if(enabled());
}

fn record_cache_hit_if(enabled: bool) {
    if enabled {
        PROFILES.with(|profiles| {
            if let Some(profile) = profiles.borrow_mut().last_mut() {
                profile.cache_hits += 1;
            }
        });
    }
}

/// Times only cache-miss computation, excluding separately resolved parents.
/// Nested compute calls count as misses but their duration is not added twice.
pub(super) struct StyleComputationSample {
    started: Option<Instant>,
    _owner_thread: PhantomData<Rc<()>>,
}

impl StyleComputationSample {
    pub(super) fn begin() -> Self {
        Self::begin_if(enabled())
    }

    fn begin_if(enabled: bool) -> Self {
        let active = enabled
            && PROFILES.with(|profiles| {
                if let Some(profile) = profiles.borrow_mut().last_mut() {
                    profile.cache_misses += 1;
                    profile.compute_depth += 1;
                    true
                } else {
                    false
                }
            });
        Self {
            started: active.then(Instant::now),
            _owner_thread: PhantomData,
        }
    }
}

impl Drop for StyleComputationSample {
    fn drop(&mut self) {
        let Some(started) = self.started else { return };
        PROFILES.with(|profiles| {
            if let Some(profile) = profiles.borrow_mut().last_mut() {
                profile.compute_depth -= 1;
                if profile.compute_depth == 0 {
                    profile.outer_compute_ms += started.elapsed().as_secs_f64() * 1000.0;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_scopes_and_samples_do_not_touch_the_stack_or_clock() {
        let scope = StyleProfileScope::begin_if(false, 1);
        let sample = StyleComputationSample::begin_if(false);
        assert!(scope.started.is_none());
        assert!(sample.started.is_none());
        record_cache_hit_if(false);
        PROFILES.with(|profiles| assert!(profiles.borrow().is_empty()));
    }

    #[test]
    fn nested_computation_counts_misses_without_double_counting_time() {
        let _scope = StyleProfileScope::begin_if(true, 1);
        record_cache_hit_if(true);
        let outer = StyleComputationSample::begin_if(true);
        drop(StyleComputationSample::begin_if(true));
        PROFILES.with(|profiles| {
            let profiles = profiles.borrow();
            let profile = profiles.last().unwrap();
            assert_eq!(profile.compute_depth, 1);
            assert_eq!(profile.cache_hits, 1);
            assert_eq!(profile.cache_misses, 2);
            assert_eq!(profile.outer_compute_ms, 0.0);
        });
        drop(outer);
        PROFILES.with(|profiles| assert_eq!(profiles.borrow().last().unwrap().compute_depth, 0));
    }

    #[test]
    fn unwinding_nested_scope_restores_the_outer_profile() {
        let outer = StyleProfileScope::begin_if(true, 1);
        record_cache_hit_if(true);
        let result = std::panic::catch_unwind(|| {
            let _inner = StyleProfileScope::begin_if(true, 2);
            let _sample = StyleComputationSample::begin_if(true);
            panic!("exercise scope cleanup");
        });
        assert!(result.is_err());
        PROFILES.with(|profiles| {
            let profiles = profiles.borrow();
            assert_eq!(profiles.len(), 1);
            assert_eq!(profiles[0].cache_hits, 1);
            assert_eq!(profiles[0].cache_misses, 0);
        });
        drop(outer);
        PROFILES.with(|profiles| assert!(profiles.borrow().is_empty()));
    }
}
