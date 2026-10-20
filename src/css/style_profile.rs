//! Opt-in counters for node computed-style resolution during one layout attempt.
use std::{cell::RefCell, marker::PhantomData, rc::Rc, sync::OnceLock, time::Instant};

#[derive(Default)]
struct Profile {
    cache_hits: usize,
    cache_misses: usize,
    compute_depth: usize,
    outer_compute_ms: f64,
}

/// Owned computed-style counters collected by one explicit profiling scope.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct StyleProfileSummary {
    pub(crate) document_id: usize,
    pub(crate) scope_ms: f64,
    pub(crate) cache_hits: usize,
    pub(crate) cache_misses: usize,
    pub(crate) outer_compute_ms: f64,
}

impl StyleProfileSummary {
    /// Combines disjoint scopes for the same document without retaining either
    /// scope or any resolver borrow.
    pub(crate) fn merged(mut self, other: Self) -> Self {
        debug_assert_eq!(self.document_id, other.document_id);
        self.scope_ms += other.scope_ms;
        self.cache_hits = self.cache_hits.saturating_add(other.cache_hits);
        self.cache_misses = self.cache_misses.saturating_add(other.cache_misses);
        self.outer_compute_ms += other.outer_compute_ms;
        self
    }
}

thread_local! {
    static PROFILES: RefCell<Vec<Profile>> = const { RefCell::new(Vec::new()) };
    #[cfg(test)]
    static EXPLICIT_MEASUREMENTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_PAINT").is_some()) {
        return true;
    }
    #[cfg(test)]
    {
        return EXPLICIT_MEASUREMENTS.with(|measurements| measurements.get() != 0);
    }
    #[cfg(not(test))]
    false
}

/// Owns a thread-local profiling scope; it never borrows the style resolver.
pub(crate) struct StyleProfileScope {
    started: Option<Instant>,
    document_id: usize,
    emit_on_drop: bool,
    #[cfg(test)]
    explicit_measurement: bool,
    _owner_thread: PhantomData<Rc<()>>,
}

impl StyleProfileScope {
    pub(crate) fn begin(document_id: usize) -> Self {
        Self::begin_if(enabled(), document_id)
    }

    fn begin_if(enabled: bool, document_id: usize) -> Self {
        Self::begin_with_mode(enabled, document_id, true, false)
    }

    /// Starts an explicitly enabled scope whose owned result is returned to
    /// its caller instead of being emitted as a layout-style record.
    pub(crate) fn begin_measurement(enabled: bool, document_id: usize) -> Self {
        Self::begin_with_mode(enabled, document_id, false, true)
    }

    fn begin_with_mode(
        enabled: bool,
        document_id: usize,
        emit_on_drop: bool,
        explicit_measurement: bool,
    ) -> Self {
        #[cfg(not(test))]
        let _ = explicit_measurement;
        if enabled {
            PROFILES.with(|profiles| profiles.borrow_mut().push(Profile::default()));
        }
        #[cfg(test)]
        if enabled && explicit_measurement {
            EXPLICIT_MEASUREMENTS.with(|measurements| {
                measurements.set(measurements.get().saturating_add(1));
            });
        }
        Self {
            started: enabled.then(Instant::now),
            document_id,
            emit_on_drop,
            #[cfg(test)]
            explicit_measurement: enabled && explicit_measurement,
            _owner_thread: PhantomData,
        }
    }

    /// Finishes a measurement scope and returns its owned counters.
    pub(crate) fn finish(mut self) -> Option<StyleProfileSummary> {
        self.take_summary()
    }

    fn take_summary(&mut self) -> Option<StyleProfileSummary> {
        let started = self.started.take()?;
        #[cfg(test)]
        if std::mem::take(&mut self.explicit_measurement) {
            EXPLICIT_MEASUREMENTS.with(|measurements| {
                measurements.set(measurements.get().saturating_sub(1));
            });
        }
        let profile = PROFILES.with(|profiles| profiles.borrow_mut().pop())?;
        Some(StyleProfileSummary {
            document_id: self.document_id,
            scope_ms: started.elapsed().as_secs_f64() * 1000.0,
            cache_hits: profile.cache_hits,
            cache_misses: profile.cache_misses,
            outer_compute_ms: profile.outer_compute_ms,
        })
    }
}

impl Drop for StyleProfileScope {
    fn drop(&mut self) {
        if let Some(summary) = self.take_summary()
            && self.emit_on_drop
        {
            eprintln!(
                "{}",
                crate::diagnostics::json_record(
                    "OMOIKANE_LAYOUT_STYLE",
                    &serde_json::json!({
                        "document_id": summary.document_id,
                        "scope_ms": summary.scope_ms,
                        "cache_hits": summary.cache_hits,
                        "cache_misses": summary.cache_misses,
                        "outer_compute_ms": summary.outer_compute_ms,
                    })
                )
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
        let measurement = StyleProfileScope::begin_measurement(false, 2);
        let sample = StyleComputationSample::begin_if(false);
        assert!(scope.started.is_none());
        assert!(measurement.started.is_none());
        assert!(sample.started.is_none());
        record_cache_hit_if(false);
        PROFILES.with(|profiles| assert!(profiles.borrow().is_empty()));
        EXPLICIT_MEASUREMENTS.with(|measurements| assert_eq!(measurements.get(), 0));
        assert_eq!(measurement.finish(), None);
    }

    #[test]
    fn measurement_returns_owned_counts_without_emitting_or_leaking_the_stack() {
        let measurement = StyleProfileScope::begin_measurement(true, 42);
        record_cache_hit();
        record_cache_hit();
        drop(StyleComputationSample::begin());
        let summary = measurement.finish().unwrap();
        assert_eq!(summary.document_id, 42);
        assert_eq!(summary.cache_hits, 2);
        assert_eq!(summary.cache_misses, 1);
        assert!(summary.scope_ms >= 0.0);
        assert!(summary.outer_compute_ms >= 0.0);
        PROFILES.with(|profiles| assert!(profiles.borrow().is_empty()));
        EXPLICIT_MEASUREMENTS.with(|measurements| assert_eq!(measurements.get(), 0));
    }

    #[test]
    fn disjoint_document_scopes_merge_each_counter_exactly() {
        let summary = StyleProfileSummary {
            document_id: 7,
            scope_ms: 2.0,
            cache_hits: 3,
            cache_misses: 5,
            outer_compute_ms: 1.0,
        }
        .merged(StyleProfileSummary {
            document_id: 7,
            scope_ms: 11.0,
            cache_hits: 13,
            cache_misses: 17,
            outer_compute_ms: 4.0,
        });
        assert_eq!(
            summary,
            StyleProfileSummary {
                document_id: 7,
                scope_ms: 13.0,
                cache_hits: 16,
                cache_misses: 22,
                outer_compute_ms: 5.0,
            }
        );
    }

    #[test]
    fn explicit_measurement_leaves_nested_scope_counts_to_the_innermost_profile() {
        let measurement = StyleProfileScope::begin_measurement(true, 7);
        record_cache_hit();
        {
            let _nested_layout = StyleProfileScope::begin_with_mode(true, 7, false, false);
            record_cache_hit();
            drop(StyleComputationSample::begin());
        }
        record_cache_hit();
        let summary = measurement.finish().unwrap();
        assert_eq!(summary.cache_hits, 2);
        assert_eq!(summary.cache_misses, 0);
        PROFILES.with(|profiles| assert!(profiles.borrow().is_empty()));
        EXPLICIT_MEASUREMENTS.with(|measurements| assert_eq!(measurements.get(), 0));
    }

    #[test]
    fn explicit_measurement_override_is_released_during_unwind() {
        let result = std::panic::catch_unwind(|| {
            let _measurement = StyleProfileScope::begin_measurement(true, 7);
            panic!("exercise explicit measurement cleanup");
        });
        assert!(result.is_err());
        PROFILES.with(|profiles| assert!(profiles.borrow().is_empty()));
        EXPLICIT_MEASUREMENTS.with(|measurements| assert_eq!(measurements.get(), 0));
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
