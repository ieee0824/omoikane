//! Task execution diagnostics and retained host-task failures.

use std::{
    cell::Cell,
    time::{Duration, Instant},
};

use super::{
    JsResult, JsRuntime,
    event_loop::{TimerMetadata, TimerScheduleMetadata},
    frame_diagnostics,
};

const SLOW_TASK_THRESHOLD: Duration = Duration::from_millis(50);

/// Fixed-size style invalidation totals for one task turn.
///
/// Inline outcomes are bounded by the resolver's static labels. An unfamiliar
/// future label increments `unknown_inline` without retaining that label. Local
/// `data-*` mutations use one fixed counter independent of the attribute name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TaskStyleInvalidationCounts {
    local: u32,
    local_raw_style_subject: u32,
    local_data_attribute: u32,
    data_attribute_same_value: u32,
    data_attribute_changed_value: u32,
    local_user_action: u32,
    local_tree_mutation: u32,
    deferred_resolver: u32,
    fallback_raw_style_selector: u32,
    fallback_active_transition: u32,
    fallback_active_animation: u32,
    fallback_document_root: u32,
    fallback_non_scalar_dom_string: u32,
    fallback_csp_block_state_change: u32,
    fallback_unsupported_declaration_change: u32,
    fallback_shadow_or_slot: u32,
    unknown_inline: u32,
    ordinary_node_mutation_full: u32,
    document_dirty: u32,
    other_full: u32,
}

impl TaskStyleInvalidationCounts {
    fn increment(counter: &mut u32) {
        *counter = counter.saturating_add(1);
    }

    fn record_inline(&mut self, outcome: &str) {
        let counter = match outcome {
            "local" => &mut self.local,
            "local_raw_style_subject" => &mut self.local_raw_style_subject,
            "deferred_resolver" => &mut self.deferred_resolver,
            "fallback_raw_style_selector" => &mut self.fallback_raw_style_selector,
            "fallback_active_transition" => &mut self.fallback_active_transition,
            "fallback_active_animation" => &mut self.fallback_active_animation,
            "fallback_document_root" => &mut self.fallback_document_root,
            "fallback_non_scalar_dom_string" => &mut self.fallback_non_scalar_dom_string,
            "fallback_csp_block_state_change" => &mut self.fallback_csp_block_state_change,
            "fallback_unsupported_declaration_change" => {
                &mut self.fallback_unsupported_declaration_change
            }
            "fallback_shadow_or_slot" => &mut self.fallback_shadow_or_slot,
            _ => &mut self.unknown_inline,
        };
        Self::increment(counter);
    }

    fn into_json(self) -> serde_json::Value {
        serde_json::json!({
            "local": self.local,
            "local_raw_style_subject": self.local_raw_style_subject,
            "local_data_attribute": self.local_data_attribute,
            "data_attribute_same_value": self.data_attribute_same_value,
            "data_attribute_changed_value": self.data_attribute_changed_value,
            "local_user_action": self.local_user_action,
            "local_tree_mutation": self.local_tree_mutation,
            "deferred_resolver": self.deferred_resolver,
            "fallback_raw_style_selector": self.fallback_raw_style_selector,
            "fallback_active_transition": self.fallback_active_transition,
            "fallback_active_animation": self.fallback_active_animation,
            "fallback_document_root": self.fallback_document_root,
            "fallback_non_scalar_dom_string": self.fallback_non_scalar_dom_string,
            "fallback_csp_block_state_change": self.fallback_csp_block_state_change,
            "fallback_unsupported_declaration_change":
                self.fallback_unsupported_declaration_change,
            "fallback_shadow_or_slot": self.fallback_shadow_or_slot,
            "unknown_inline": self.unknown_inline,
            "ordinary_node_mutation_full": self.ordinary_node_mutation_full,
            "document_dirty": self.document_dirty,
            "other_full": self.other_full,
        })
    }
}

thread_local! {
    static ACTIVE_STYLE_INVALIDATIONS: Cell<Option<TaskStyleInvalidationCounts>> =
        const { Cell::new(None) };
}

/// Restores an enclosing task's counters after a nested task pump completes.
struct TaskStyleInvalidationScope {
    previous: Option<TaskStyleInvalidationCounts>,
    active: bool,
}

impl TaskStyleInvalidationScope {
    fn begin(enabled: bool) -> Self {
        if !enabled {
            return Self {
                previous: None,
                active: false,
            };
        }
        let previous = ACTIVE_STYLE_INVALIDATIONS
            .with(|counts| counts.replace(Some(TaskStyleInvalidationCounts::default())));
        Self {
            previous,
            active: true,
        }
    }

    fn finish(mut self) -> TaskStyleInvalidationCounts {
        if !self.active {
            return TaskStyleInvalidationCounts::default();
        }
        let counts = ACTIVE_STYLE_INVALIDATIONS
            .with(|active| active.replace(self.previous))
            .unwrap_or_default();
        self.active = false;
        counts
    }
}

impl Drop for TaskStyleInvalidationScope {
    fn drop(&mut self) {
        if self.active {
            ACTIVE_STYLE_INVALIDATIONS.with(|active| active.set(self.previous));
        }
    }
}

fn update_active_style_invalidations(update: impl FnOnce(&mut TaskStyleInvalidationCounts)) {
    ACTIVE_STYLE_INVALIDATIONS.with(|active| {
        let Some(mut counts) = active.get() else {
            return;
        };
        update(&mut counts);
        active.set(Some(counts));
    });
}

fn record_inline_style_invalidation_active(outcome: &str) {
    update_active_style_invalidations(|counts| counts.record_inline(outcome));
}

/// Adds one bounded inline-style cache policy outcome to the active task turn.
pub(super) fn record_inline_style_invalidation(outcome: &'static str) {
    if tracing_enabled() {
        record_inline_style_invalidation_active(outcome);
    }
}

fn record_local_user_action_invalidation_active() {
    update_active_style_invalidations(|counts| {
        TaskStyleInvalidationCounts::increment(&mut counts.local_user_action);
    });
}

/// Records one locally-evicted hover, active or focus state change.
pub(super) fn record_local_user_action_invalidation() {
    if tracing_enabled() {
        record_local_user_action_invalidation_active();
    }
}

fn record_local_data_attribute_invalidation_active() {
    update_active_style_invalidations(|counts| {
        TaskStyleInvalidationCounts::increment(&mut counts.local_data_attribute);
    });
}

/// Records one locally-evicted ordinary `data-*` attribute mutation.
pub(super) fn record_local_data_attribute_invalidation() {
    if tracing_enabled() {
        record_local_data_attribute_invalidation_active();
    }
}

fn record_data_attribute_assignment_active(same_value: bool) {
    update_active_style_invalidations(|counts| {
        let counter = if same_value {
            &mut counts.data_attribute_same_value
        } else {
            &mut counts.data_attribute_changed_value
        };
        TaskStyleInvalidationCounts::increment(counter);
    });
}

/// Records whether one `data-*` assignment preserves the exact UTF-16 value.
pub(super) fn record_data_attribute_assignment(same_value: bool) {
    if tracing_enabled() {
        record_data_attribute_assignment_active(same_value);
    }
}

fn record_local_tree_mutation_invalidation_active() {
    update_active_style_invalidations(|counts| {
        TaskStyleInvalidationCounts::increment(&mut counts.local_tree_mutation);
    });
}

/// Records one child-list change that retained styles outside its parents.
pub(super) fn record_local_tree_mutation_invalidation() {
    if tracing_enabled() {
        record_local_tree_mutation_invalidation_active();
    }
}

fn record_ordinary_node_mutation_full_active() {
    update_active_style_invalidations(|counts| {
        TaskStyleInvalidationCounts::increment(&mut counts.ordinary_node_mutation_full);
    });
}

/// Records an actual full resolver-cache clear caused by an ordinary node mutation.
pub(super) fn record_ordinary_node_mutation_full() {
    if tracing_enabled() {
        record_ordinary_node_mutation_full_active();
    }
}

fn record_document_dirty_active() {
    update_active_style_invalidations(|counts| {
        TaskStyleInvalidationCounts::increment(&mut counts.document_dirty);
    });
}

/// Records a request to rebuild a document's stylesheet-backed resolver.
pub(super) fn record_document_dirty() {
    if tracing_enabled() {
        record_document_dirty_active();
    }
}

fn record_other_full_active() {
    update_active_style_invalidations(|counts| {
        TaskStyleInvalidationCounts::increment(&mut counts.other_full);
    });
}

/// Records another actual full resolver-cache clear, such as user-action state.
pub(super) fn record_other_full() {
    if tracing_enabled() {
        record_other_full_active();
    }
}

/// Returns whether task timing and registration diagnostics are enabled.
pub(super) fn tracing_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_TASKS").is_some())
}

/// Captures an author timer's registration source only while task tracing is enabled.
pub(super) fn timer_schedule_metadata(
    context: &boa_engine::Context,
) -> Option<TimerScheduleMetadata> {
    tracing_enabled().then(|| TimerScheduleMetadata {
        source: frame_diagnostics::current_script_source(context),
    })
}

/// One timestamp shared by adjacent task-turn phases.
///
/// Capturing one boundary after each phase keeps the phase totals contiguous
/// while avoiding a second clock and GC-profile read at the next phase start.
#[derive(Clone, Copy)]
struct TaskDiagnosticBoundary {
    at: Instant,
    #[cfg(feature = "gc-profile")]
    gc: boa_gc::GcProfile,
}

impl TaskDiagnosticBoundary {
    fn capture() -> Self {
        Self {
            at: Instant::now(),
            #[cfg(feature = "gc-profile")]
            gc: boa_gc::profile(),
        }
    }

    fn until(self, end: Self) -> TaskPhaseDiagnostics {
        TaskPhaseDiagnostics {
            elapsed: end.at.duration_since(self.at),
            #[cfg(feature = "gc-profile")]
            gc_before: self.gc,
            #[cfg(feature = "gc-profile")]
            gc_after: end.gc,
        }
    }
}

#[derive(Clone, Copy)]
struct TaskPhaseDiagnostics {
    elapsed: Duration,
    #[cfg(feature = "gc-profile")]
    gc_before: boa_gc::GcProfile,
    #[cfg(feature = "gc-profile")]
    gc_after: boa_gc::GcProfile,
}

impl TaskPhaseDiagnostics {
    fn elapsed_ms(self) -> f64 {
        self.elapsed.as_secs_f64() * 1000.0
    }

    #[cfg(feature = "gc-profile")]
    fn gc_delta(self) -> serde_json::Value {
        let collection_delta = |before: boa_gc::CollectionProfile,
                                after: boa_gc::CollectionProfile| {
            serde_json::json!({
                "collections": after.collections.saturating_sub(before.collections),
                "total_ms": after.total.saturating_sub(before.total).as_secs_f64() * 1000.0,
                "mark_ms": after.mark.saturating_sub(before.mark).as_secs_f64() * 1000.0,
                "finalize_ms": after.finalize.saturating_sub(before.finalize).as_secs_f64()
                    * 1000.0,
                "sweep_ms": after.sweep.saturating_sub(before.sweep).as_secs_f64() * 1000.0,
            })
        };
        serde_json::json!({
            "minor": collection_delta(self.gc_before.minor, self.gc_after.minor),
            "major": collection_delta(self.gc_before.major, self.gc_after.major),
        })
    }
}

/// Diagnostics for one macrotask plus its HTML microtask checkpoint.
///
/// This owns all recorded data so it never extends a runtime or task borrow.
pub(super) struct TaskTurnDiagnostics {
    source: String,
    kind: String,
    timer: Option<TimerMetadata>,
    started: TaskDiagnosticBoundary,
    last_boundary: TaskDiagnosticBoundary,
    callback: Option<TaskPhaseDiagnostics>,
    background: Option<TaskPhaseDiagnostics>,
    jobs: Option<TaskPhaseDiagnostics>,
    cookie: Option<TaskPhaseDiagnostics>,
    callback_success: bool,
    style_invalidations: TaskStyleInvalidationScope,
}

impl TaskTurnDiagnostics {
    pub(super) fn new(source: String, kind: String, timer: Option<TimerMetadata>) -> Self {
        Self::from_boundary(source, kind, timer, TaskDiagnosticBoundary::capture())
    }

    fn from_boundary(
        source: String,
        kind: String,
        timer: Option<TimerMetadata>,
        started: TaskDiagnosticBoundary,
    ) -> Self {
        Self {
            source,
            kind,
            timer,
            started,
            last_boundary: started,
            callback: None,
            background: None,
            jobs: None,
            cookie: None,
            callback_success: false,
            style_invalidations: TaskStyleInvalidationScope::begin(true),
        }
    }

    fn finish_phase_at(&mut self, end: TaskDiagnosticBoundary) -> TaskPhaseDiagnostics {
        let phase = self.last_boundary.until(end);
        self.last_boundary = end;
        phase
    }

    fn finish_callback_at(&mut self, success: bool, end: TaskDiagnosticBoundary) {
        let phase = self.finish_phase_at(end);
        self.callback = Some(phase);
        self.callback_success = success;
    }

    pub(super) fn finish_callback(&mut self, success: bool) {
        self.finish_callback_at(success, TaskDiagnosticBoundary::capture());
    }

    fn finish_background_at(&mut self, end: TaskDiagnosticBoundary) {
        let phase = self.finish_phase_at(end);
        self.background = Some(phase);
    }

    pub(super) fn finish_background(&mut self) {
        self.finish_background_at(TaskDiagnosticBoundary::capture());
    }

    fn finish_jobs_at(&mut self, end: TaskDiagnosticBoundary) {
        let phase = self.finish_phase_at(end);
        self.jobs = Some(phase);
    }

    pub(super) fn finish_jobs(&mut self) {
        self.finish_jobs_at(TaskDiagnosticBoundary::capture());
    }

    fn finish_cookie_at(&mut self, end: TaskDiagnosticBoundary) {
        let phase = self.finish_phase_at(end);
        self.cookie = Some(phase);
    }

    pub(super) fn finish_cookie(&mut self) {
        self.finish_cookie_at(TaskDiagnosticBoundary::capture());
    }

    fn into_record(self, turn_success: bool) -> Option<serde_json::Value> {
        let Self {
            source,
            kind,
            timer,
            started,
            last_boundary,
            callback,
            background,
            jobs,
            cookie,
            callback_success,
            style_invalidations,
        } = self;
        // Restore an enclosing task even when this turn is below the threshold.
        let style_invalidations = style_invalidations.finish();
        let callback = callback.expect("task diagnostics finish after callback");
        let turn = started.until(last_boundary);
        if turn.elapsed < SLOW_TASK_THRESHOLD {
            return None;
        }
        #[cfg(feature = "gc-profile")]
        let gc = Some(serde_json::json!({
            "callback": callback.gc_delta(),
            "background": background.map(TaskPhaseDiagnostics::gc_delta),
            "jobs": jobs.map(TaskPhaseDiagnostics::gc_delta),
            "cookie": cookie.map(TaskPhaseDiagnostics::gc_delta),
            "turn": turn.gc_delta(),
        }));
        #[cfg(not(feature = "gc-profile"))]
        let gc: Option<serde_json::Value> = None;
        let timer_id = timer.as_ref().map(|timer| timer.id);
        let timer_source = timer.and_then(|timer| timer.source).map(|source| {
            serde_json::json!({
                "file": source.file,
                "line": source.line,
                "column": source.column,
            })
        });
        Some(serde_json::json!({
            // Preserve the existing callback fields for current parsers.
            "source": source,
            "kind": kind,
            "timer_id": timer_id,
            "timer_source": timer_source,
            "elapsed_ms": callback.elapsed_ms(),
            "success": callback_success,
            "background_ms": background.map(TaskPhaseDiagnostics::elapsed_ms),
            "jobs_ms": jobs.map(TaskPhaseDiagnostics::elapsed_ms),
            "cookie_ms": cookie.map(TaskPhaseDiagnostics::elapsed_ms),
            "turn_ms": turn.elapsed_ms(),
            "turn_success": turn_success,
            "gc": gc,
            "style_invalidations": style_invalidations.into_json(),
        }))
    }

    pub(super) fn finish_and_emit(self, turn_success: bool) {
        if let Some(record) = self.into_record(turn_success) {
            eprintln!(
                "{}",
                crate::diagnostics::json_record("OMOIKANE_TASK", &record)
            );
        }
    }
}

impl JsRuntime {
    /// Retains a raw failure before subsequent jobs or collection can run.
    /// Script diagnostics additionally print the existing timer log format.
    pub(super) fn record_host_task_result(
        &mut self,
        kind: &str,
        phase: &str,
        result: JsResult<()>,
    ) {
        let Err(error) = result else { return };
        if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
            let category = if phase == "jobs" {
                "timer-job-error"
            } else {
                "timer-error"
            };
            eprintln!("[omoikane][{category}] {error}");
        }
        self.record_error_from::<()>(&format!("host task {kind} {phase}"), Err(error));
    }
}

#[cfg(test)]
mod tests {
    use crate::js::JsRuntime;
    use boa_engine::JsNativeError;
    use boa_engine::job::GenericJob;

    #[test]
    fn host_resource_failure_is_retained_while_later_timers_continue() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                "globalThis.frame=document.createElement('iframe'); \
             frame.srcdoc='<p>first</p>'; document.body.appendChild(frame); \
             globalThis.afterFailure=false;",
            )
            .unwrap();
        runtime.run_timers(100, 1, 10);
        assert!(runtime.take_task_errors().is_empty());
        runtime.eval("frame.srcdoc='<p>second</p>'").unwrap();
        runtime.host_state.borrow_mut().next_iframe_generation = u64::MAX;
        runtime.set_timeout("globalThis.afterFailure=true", 0);
        runtime.run_timers(100, 1, 10);
        assert_eq!(
            runtime.eval("afterFailure").unwrap().as_boolean(),
            Some(true)
        );
        let errors = runtime.take_task_errors();
        assert_eq!(
            errors.len(),
            1,
            "host failure must be retained exactly once: {errors:?}"
        );
        assert!(
            errors[0].contains("iframe document generation"),
            "{errors:?}"
        );
        assert!(runtime.take_task_errors().is_empty());
    }

    #[test]
    fn host_job_failure_is_retained_while_later_timers_continue() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                "globalThis.afterFailure=false; setTimeout(()=>{},0); \
             setTimeout(()=>{afterFailure=true},0);",
            )
            .unwrap();
        let realm = runtime.context.realm().clone();
        runtime.context.enqueue_job(
            GenericJob::new(
                |_| {
                    Err(JsNativeError::error()
                        .with_message("host-job-failure")
                        .into())
                },
                realm,
            )
            .into(),
        );
        runtime.run_timers(100, 1, 10);
        assert_eq!(
            runtime.eval("afterFailure").unwrap().as_boolean(),
            Some(true)
        );
        let errors = runtime.take_task_errors();
        assert_eq!(
            errors.len(),
            1,
            "host job failure must be retained exactly once: {errors:?}"
        );
        assert!(errors[0].contains("host-job-failure"), "{errors:?}");
    }

    #[test]
    fn author_timer_failure_is_not_recorded_twice() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                "globalThis.afterFailure=false; \
             setTimeout(()=>{throw new Error('author-callback')},0); \
             setTimeout(()=>{afterFailure=true},0);",
            )
            .unwrap();
        runtime.run_timers(100, 1, 10);
        assert_eq!(
            runtime.eval("afterFailure").unwrap().as_boolean(),
            Some(true)
        );
        let errors = runtime.take_task_errors();
        assert_eq!(
            errors.len(),
            1,
            "author failure must be retained exactly once: {errors:?}"
        );
        assert!(errors[0].contains("author-callback"), "{errors:?}");
    }
}

#[cfg(test)]
mod turn_timing_tests {
    use super::*;

    fn boundary(started: Instant, elapsed: Duration) -> TaskDiagnosticBoundary {
        TaskDiagnosticBoundary {
            at: started + elapsed,
            #[cfg(feature = "gc-profile")]
            gc: gc_profile(
                collection_profile(0, 0, 0, 0, 0),
                collection_profile(0, 0, 0, 0, 0),
            ),
        }
    }

    fn diagnostics(started: Instant) -> TaskTurnDiagnostics {
        TaskTurnDiagnostics::from_boundary(
            "Timers".to_string(),
            "timer".to_string(),
            None,
            boundary(started, Duration::ZERO),
        )
    }

    fn milliseconds(record: &serde_json::Value, key: &str) -> Option<f64> {
        record[key].as_f64()
    }

    #[test]
    fn slow_turn_emits_only_fixed_style_invalidation_labels() {
        let started = Instant::now();
        let mut diagnostics = diagnostics(started);
        for outcome in [
            "local",
            "local_raw_style_subject",
            "deferred_resolver",
            "fallback_raw_style_selector",
            "fallback_active_transition",
            "fallback_active_animation",
            "fallback_document_root",
            "fallback_non_scalar_dom_string",
            "fallback_csp_block_state_change",
            "fallback_unsupported_declaration_change",
            "fallback_shadow_or_slot",
            "future_outcome_must_not_be_retained",
        ] {
            record_inline_style_invalidation_active(outcome);
        }
        record_local_data_attribute_invalidation_active();
        record_data_attribute_assignment_active(true);
        record_data_attribute_assignment_active(true);
        record_data_attribute_assignment_active(false);
        record_local_user_action_invalidation_active();
        record_local_tree_mutation_invalidation_active();
        record_ordinary_node_mutation_full_active();
        record_ordinary_node_mutation_full_active();
        record_document_dirty_active();
        record_other_full_active();
        diagnostics.finish_callback_at(true, boundary(started, Duration::from_millis(60)));

        let record = diagnostics.into_record(true).expect("60 ms turn is slow");
        assert_eq!(
            record["style_invalidations"],
            serde_json::json!({
                "local": 1,
                "local_raw_style_subject": 1,
                "local_data_attribute": 1,
                "data_attribute_same_value": 2,
                "data_attribute_changed_value": 1,
                "local_user_action": 1,
                "local_tree_mutation": 1,
                "deferred_resolver": 1,
                "fallback_raw_style_selector": 1,
                "fallback_active_transition": 1,
                "fallback_active_animation": 1,
                "fallback_document_root": 1,
                "fallback_non_scalar_dom_string": 1,
                "fallback_csp_block_state_change": 1,
                "fallback_unsupported_declaration_change": 1,
                "fallback_shadow_or_slot": 1,
                "unknown_inline": 1,
                "ordinary_node_mutation_full": 2,
                "document_dirty": 1,
                "other_full": 1,
            })
        );
        assert!(
            !record
                .to_string()
                .contains("future_outcome_must_not_be_retained"),
            "unknown outcome text must not escape the bounded counter"
        );
    }

    #[test]
    fn disabled_style_scope_never_installs_counters() {
        assert!(
            ACTIVE_STYLE_INVALIDATIONS
                .with(|active| active.get())
                .is_none()
        );
        let disabled = TaskStyleInvalidationScope::begin(false);
        record_inline_style_invalidation_active("local");
        record_ordinary_node_mutation_full_active();
        assert_eq!(disabled.finish(), TaskStyleInvalidationCounts::default());
        assert!(
            ACTIVE_STYLE_INVALIDATIONS
                .with(|active| active.get())
                .is_none()
        );
    }

    #[test]
    fn nested_style_scope_restores_outer_counters() {
        let outer = TaskStyleInvalidationScope::begin(true);
        record_inline_style_invalidation_active("local");

        let inner = TaskStyleInvalidationScope::begin(true);
        record_inline_style_invalidation_active("fallback_document_root");
        record_document_dirty_active();
        let inner_counts = inner.finish();
        assert_eq!(inner_counts.fallback_document_root, 1);
        assert_eq!(inner_counts.document_dirty, 1);

        record_ordinary_node_mutation_full_active();
        let outer_counts = outer.finish();
        assert_eq!(outer_counts.local, 1);
        assert_eq!(outer_counts.ordinary_node_mutation_full, 1);
        assert_eq!(outer_counts.fallback_document_root, 0);
        assert_eq!(outer_counts.document_dirty, 0);
        assert!(
            ACTIVE_STYLE_INVALIDATIONS
                .with(|active| active.get())
                .is_none()
        );
    }

    #[test]
    fn early_scope_drop_restores_outer_counters() {
        fn return_from_nested_scope() {
            let _inner = TaskStyleInvalidationScope::begin(true);
            record_inline_style_invalidation_active("fallback_active_animation");
        }

        let outer = TaskStyleInvalidationScope::begin(true);
        record_inline_style_invalidation_active("local");
        return_from_nested_scope();
        record_other_full_active();

        let outer_counts = outer.finish();
        assert_eq!(outer_counts.local, 1);
        assert_eq!(outer_counts.other_full, 1);
        assert_eq!(outer_counts.fallback_active_animation, 0);
        assert!(
            ACTIVE_STYLE_INVALIDATIONS
                .with(|active| active.get())
                .is_none()
        );
    }

    #[test]
    fn panic_unwind_restores_outer_style_scope() {
        let outer = TaskStyleInvalidationScope::begin(true);
        record_inline_style_invalidation_active("local_raw_style_subject");
        let unwound = std::panic::catch_unwind(|| {
            let _inner = TaskStyleInvalidationScope::begin(true);
            record_inline_style_invalidation_active("fallback_shadow_or_slot");
            panic!("synthetic nested task failure");
        });
        assert!(unwound.is_err());
        record_document_dirty_active();

        let outer_counts = outer.finish();
        assert_eq!(outer_counts.local_raw_style_subject, 1);
        assert_eq!(outer_counts.document_dirty, 1);
        assert_eq!(outer_counts.fallback_shadow_or_slot, 0);
        assert!(
            ACTIVE_STYLE_INVALIDATIONS
                .with(|active| active.get())
                .is_none()
        );
    }

    #[test]
    fn fast_turn_early_return_restores_outer_style_scope() {
        let started = Instant::now();
        let outer = TaskStyleInvalidationScope::begin(true);
        record_inline_style_invalidation_active("local");

        let mut fast = diagnostics(started);
        record_inline_style_invalidation_active("fallback_active_transition");
        fast.finish_callback_at(true, boundary(started, Duration::from_millis(1)));
        assert!(fast.into_record(true).is_none());

        record_ordinary_node_mutation_full_active();
        let outer_counts = outer.finish();
        assert_eq!(outer_counts.local, 1);
        assert_eq!(outer_counts.ordinary_node_mutation_full, 1);
        assert_eq!(outer_counts.fallback_active_transition, 0);
        assert!(
            ACTIVE_STYLE_INVALIDATIONS
                .with(|active| active.get())
                .is_none()
        );
    }

    #[test]
    fn phase_totals_keep_callback_fields_and_cover_the_turn() {
        let started = Instant::now();
        let mut diagnostics = diagnostics(started);
        diagnostics.finish_callback_at(true, boundary(started, Duration::from_millis(10)));
        diagnostics.finish_background_at(boundary(started, Duration::from_millis(20)));
        diagnostics.finish_jobs_at(boundary(started, Duration::from_millis(80)));
        diagnostics.finish_cookie_at(boundary(started, Duration::from_millis(100)));

        let record = diagnostics.into_record(true).expect("100 ms turn is slow");
        assert_eq!(record["source"], "Timers");
        assert_eq!(record["kind"], "timer");
        assert!(record["timer_id"].is_null());
        assert!(record["timer_source"].is_null());
        assert_eq!(milliseconds(&record, "elapsed_ms"), Some(10.0));
        assert_eq!(record["success"], true);
        assert_eq!(milliseconds(&record, "background_ms"), Some(10.0));
        assert_eq!(milliseconds(&record, "jobs_ms"), Some(60.0));
        assert_eq!(milliseconds(&record, "cookie_ms"), Some(20.0));
        assert_eq!(milliseconds(&record, "turn_ms"), Some(100.0));
        assert_eq!(record["turn_success"], true);
    }

    #[test]
    fn author_timer_registration_is_emitted_as_owned_sanitized_metadata() {
        let started = Instant::now();
        let timer = crate::js::event_loop::TimerMetadata {
            id: 41,
            source: Some(crate::js::event_loop::ScriptSourceLocation {
                file: "https://example.test/app.js".to_string(),
                line: 12,
                column: 34,
            }),
        };
        let mut diagnostics = TaskTurnDiagnostics::from_boundary(
            "Timer".to_string(),
            "timer".to_string(),
            Some(timer),
            boundary(started, Duration::ZERO),
        );
        diagnostics.finish_callback_at(true, boundary(started, Duration::from_millis(60)));

        let record = diagnostics
            .into_record(true)
            .expect("60 ms author timer is slow");
        assert_eq!(record["timer_id"].as_u64(), Some(41));
        assert_eq!(
            record["timer_source"]["file"],
            "https://example.test/app.js"
        );
        assert_eq!(record["timer_source"]["line"].as_u64(), Some(12));
        assert_eq!(record["timer_source"]["column"].as_u64(), Some(34));
        assert_eq!(milliseconds(&record, "elapsed_ms"), Some(60.0));
    }

    #[test]
    fn author_timer_without_a_position_keeps_its_id() {
        let started = Instant::now();
        let timer = crate::js::event_loop::TimerMetadata {
            id: 42,
            source: None,
        };
        let mut diagnostics = TaskTurnDiagnostics::from_boundary(
            "Timer".to_string(),
            "timer".to_string(),
            Some(timer),
            boundary(started, Duration::ZERO),
        );
        diagnostics.finish_callback_at(true, boundary(started, Duration::from_millis(60)));

        let record = diagnostics
            .into_record(true)
            .expect("60 ms author timer is slow");
        assert_eq!(record["timer_id"].as_u64(), Some(42));
        assert!(record["timer_source"].is_null());
    }

    #[test]
    fn turn_threshold_is_inclusive_and_catches_slow_jobs() {
        let started = Instant::now();
        let mut below = diagnostics(started);
        below.finish_callback_at(true, boundary(started, Duration::from_millis(1)));
        below.finish_background_at(boundary(started, Duration::from_millis(2)));
        below.finish_jobs_at(boundary(started, Duration::from_nanos(49_999_999)));
        below.finish_cookie_at(boundary(started, Duration::from_nanos(49_999_999)));
        assert!(below.into_record(true).is_none());

        let mut boundary_case = diagnostics(started);
        boundary_case.finish_callback_at(true, boundary(started, Duration::from_millis(1)));
        boundary_case.finish_background_at(boundary(started, Duration::from_millis(2)));
        boundary_case.finish_jobs_at(boundary(started, Duration::from_millis(50)));
        boundary_case.finish_cookie_at(boundary(started, Duration::from_millis(50)));

        let record = boundary_case
            .into_record(true)
            .expect("the 50 ms boundary is included");
        assert_eq!(milliseconds(&record, "elapsed_ms"), Some(1.0));
        assert_eq!(milliseconds(&record, "jobs_ms"), Some(48.0));
        assert_eq!(milliseconds(&record, "turn_ms"), Some(50.0));
    }

    #[test]
    fn skipped_phases_are_null_and_success_values_stay_distinct() {
        let started = Instant::now();
        let mut callback_error = diagnostics(started);
        callback_error.finish_callback_at(false, boundary(started, Duration::from_millis(60)));
        let record = callback_error
            .into_record(false)
            .expect("failed callback is slow");
        assert_eq!(record["success"], false);
        assert_eq!(record["turn_success"], false);
        assert!(record["background_ms"].is_null());
        assert!(record["jobs_ms"].is_null());
        assert!(record["cookie_ms"].is_null());

        let mut jobs_error = diagnostics(started);
        jobs_error.finish_callback_at(true, boundary(started, Duration::from_millis(10)));
        jobs_error.finish_background_at(boundary(started, Duration::from_millis(20)));
        jobs_error.finish_jobs_at(boundary(started, Duration::from_millis(60)));
        let record = jobs_error.into_record(false).expect("failed jobs are slow");
        assert_eq!(record["success"], true);
        assert_eq!(record["turn_success"], false);
        assert_eq!(milliseconds(&record, "jobs_ms"), Some(40.0));
        assert!(record["cookie_ms"].is_null());

        let mut terminated_worker = diagnostics(started);
        terminated_worker.finish_callback_at(true, boundary(started, Duration::from_millis(30)));
        terminated_worker.finish_background_at(boundary(started, Duration::from_millis(60)));
        let record = terminated_worker
            .into_record(true)
            .expect("partial worker turn is slow");
        assert_eq!(record["success"], true);
        assert_eq!(record["turn_success"], true);
        assert!(record["jobs_ms"].is_null());
        assert!(record["cookie_ms"].is_null());
    }

    #[cfg(feature = "gc-profile")]
    fn collection_profile(
        collections: usize,
        total_ms: u64,
        mark_ms: u64,
        finalize_ms: u64,
        sweep_ms: u64,
    ) -> boa_gc::CollectionProfile {
        boa_gc::CollectionProfile {
            collections,
            total: Duration::from_millis(total_ms),
            mark: Duration::from_millis(mark_ms),
            finalize: Duration::from_millis(finalize_ms),
            sweep: Duration::from_millis(sweep_ms),
        }
    }

    #[cfg(feature = "gc-profile")]
    fn gc_profile(
        minor: boa_gc::CollectionProfile,
        major: boa_gc::CollectionProfile,
    ) -> boa_gc::GcProfile {
        boa_gc::GcProfile { minor, major }
    }

    #[cfg(feature = "gc-profile")]
    fn boundary_with_gc(
        started: Instant,
        elapsed: Duration,
        minor: boa_gc::CollectionProfile,
        major: boa_gc::CollectionProfile,
    ) -> TaskDiagnosticBoundary {
        TaskDiagnosticBoundary {
            at: started + elapsed,
            gc: gc_profile(minor, major),
        }
    }

    #[cfg(feature = "gc-profile")]
    #[test]
    fn synthetic_boundaries_report_per_phase_and_turn_gc_deltas() {
        let started = Instant::now();
        let start_boundary = boundary_with_gc(
            started,
            Duration::ZERO,
            collection_profile(10, 20, 5, 2, 13),
            collection_profile(2, 10, 3, 1, 6),
        );
        let mut diagnostics = TaskTurnDiagnostics::from_boundary(
            "Timers".to_string(),
            "timer".to_string(),
            None,
            start_boundary,
        );
        diagnostics.finish_callback_at(
            true,
            boundary_with_gc(
                started,
                Duration::from_millis(10),
                collection_profile(11, 24, 6, 3, 15),
                collection_profile(2, 10, 3, 1, 6),
            ),
        );
        diagnostics.finish_background_at(boundary_with_gc(
            started,
            Duration::from_millis(20),
            collection_profile(11, 24, 6, 3, 15),
            collection_profile(2, 10, 3, 1, 6),
        ));
        diagnostics.finish_jobs_at(boundary_with_gc(
            started,
            Duration::from_millis(80),
            collection_profile(13, 35, 10, 5, 20),
            collection_profile(3, 18, 6, 2, 10),
        ));
        diagnostics.finish_cookie_at(boundary_with_gc(
            started,
            Duration::from_millis(100),
            collection_profile(13, 35, 10, 5, 20),
            collection_profile(3, 18, 6, 2, 10),
        ));

        let record = diagnostics.into_record(true).expect("100 ms turn is slow");
        let callback_minor = &record["gc"]["callback"]["minor"];
        assert_eq!(callback_minor["collections"].as_u64(), Some(1));
        assert_eq!(callback_minor["total_ms"].as_f64(), Some(4.0));
        let jobs_minor = &record["gc"]["jobs"]["minor"];
        assert_eq!(jobs_minor["collections"].as_u64(), Some(2));
        assert_eq!(jobs_minor["total_ms"].as_f64(), Some(11.0));
        let jobs_major = &record["gc"]["jobs"]["major"];
        assert_eq!(jobs_major["collections"].as_u64(), Some(1));
        assert_eq!(jobs_major["total_ms"].as_f64(), Some(8.0));
        let turn_minor = &record["gc"]["turn"]["minor"];
        assert_eq!(turn_minor["collections"].as_u64(), Some(3));
        assert_eq!(turn_minor["total_ms"].as_f64(), Some(15.0));
        let turn_major = &record["gc"]["turn"]["major"];
        assert_eq!(turn_major["collections"].as_u64(), Some(1));
        assert_eq!(turn_major["total_ms"].as_f64(), Some(8.0));
    }
}
