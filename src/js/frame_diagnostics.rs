//! Optional per-rendering-opportunity diagnostics for synchronous page work.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::Instant;

use super::event_loop::ScriptSourceLocation;
use crate::css::{TransitionPropertyCounts, style::StyleClockEffects};

fn raf_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_RAF").is_some())
}

fn source_label(source: &str) -> String {
    match url::Url::parse(source) {
        Ok(mut url) if matches!(url.scheme(), "http" | "https") => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        Ok(url) => format!("{}:", url.scheme()),
        Err(_) => "<inline-or-local>".into(),
    }
}

/// Captures the first JavaScript caller using a sanitized, owned source label.
pub(super) fn current_script_source(context: &boa_engine::Context) -> Option<ScriptSourceLocation> {
    // requestAnimationFrame is a direct public native binding, so the first
    // JavaScript frame is its caller. setTimeout and setInterval use the same
    // direct binding shape, so no trusted-bootstrap frame is skipped.
    let frame = context.stack_trace().next()?;
    let location = frame.position();
    let position = location.position?;
    let file = match &location.path {
        boa_engine::vm::SourcePath::Path(path) => source_label(&path.to_string_lossy()),
        _ => "<anonymous>".into(),
    };
    Some(ScriptSourceLocation {
        file,
        line: position.line_number(),
        column: position.column_number(),
    })
}

pub(super) fn raf_source(context: &boa_engine::Context) -> Option<serde_json::Value> {
    if !raf_enabled() {
        return None;
    }
    let source = current_script_source(context)?;
    Some(serde_json::json!({
        "file": source.file, "line": source.line, "column": source.column,
    }))
}

pub(super) fn record_raf_schedule(
    id: u64,
    document: Option<usize>,
    source: Option<serde_json::Value>,
) {
    if raf_enabled() {
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_RAF_SCHEDULE",
                &serde_json::json!({
                    "id": id, "document_id": document, "source": source,
                })
            )
        );
    }
}

pub(super) fn start_raf_callback() -> Option<Instant> {
    raf_enabled().then(Instant::now)
}

pub(super) fn record_raf_execution(id: u64, document: Option<usize>, started: Option<Instant>) {
    if let Some(started) = started {
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_RAF_EXECUTE",
                &serde_json::json!({
                    "id": id, "document_id": document,
                    "duration_ms": started.elapsed().as_secs_f64() * 1000.0,
                })
            )
        );
    }
}

#[derive(Default)]
struct FrameSample {
    document_id: usize,
    scroll_events_ms: f64,
    media_ms: f64,
    raf_call_ms: f64,
    jobs_ms: f64,
    layout_calls: usize,
    layout_rebuilds: usize,
    layout_ms: f64,
}

thread_local! {
    // A stack keeps nested rendering opportunities and documents separate.
    static FRAMES: RefCell<Vec<FrameSample>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ScrollLayoutSummary {
    calls: usize,
    rebuilds: usize,
    total_ms: f64,
    max_ms: f64,
    rebuild_total_ms: f64,
    rebuild_max_ms: f64,
}

impl ScrollLayoutSummary {
    fn record(&mut self, rebuilt: bool, elapsed_ms: f64) {
        self.calls += 1;
        self.rebuilds += usize::from(rebuilt);
        self.total_ms += elapsed_ms;
        self.max_ms = self.max_ms.max(elapsed_ms);
        if rebuilt {
            self.rebuild_total_ms += elapsed_ms;
            self.rebuild_max_ms = self.rebuild_max_ms.max(elapsed_ms);
        }
    }

    fn cache_hits(&self) -> usize {
        self.calls - self.rebuilds
    }
}

#[derive(Clone, Copy)]
struct ScrollTarget {
    document_id: usize,
    node_id: usize,
    viewport: bool,
    layout: ScrollLayoutSummary,
}

thread_local! {
    // Nested event-loop calls restore the previous target on return.
    static SCROLL_TARGETS: RefCell<Vec<ScrollTarget>> = const { RefCell::new(Vec::new()) };
}

fn scroll_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("OMOIKANE_TRACE_PAINT").is_some()
            || std::env::var_os("OMOIKANE_TRACE_TASKS").is_some()
    })
}

fn style_cache_invalidation_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_ATTRIBUTE_SET").is_some())
}

fn style_clock_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_PAINT").is_some())
}

fn transition_property_data(counts: TransitionPropertyCounts) -> serde_json::Value {
    serde_json::json!({
        "nodes": counts.nodes,
        "properties": counts.properties,
        "geometry": counts.geometry,
        "transform": counts.transform,
        "color": counts.color,
        "background": counts.background,
        "filter": counts.filter,
        "opacity": counts.opacity,
        "other": counts.other,
    })
}

fn style_clock_data(
    document_id: usize,
    effects: StyleClockEffects,
    transition_properties: Option<TransitionPropertyCounts>,
) -> serde_json::Value {
    serde_json::json!({
        "document_id": document_id,
        "transition_changed": effects.transition_changed,
        "transition_requires_layout": effects.transition_requires_layout,
        "animation_changed": effects.animation_changed,
        "animation_requires_layout": effects.animation_requires_layout,
        "retained_animation_elements": effects.retained_animation_elements,
        "retained_animation_pseudos": effects.retained_animation_pseudos,
        "changed_animation_elements": effects.changed_animation_elements,
        "changed_animation_pseudos": effects.changed_animation_pseudos,
        "running_transition_properties": transition_properties.map(transition_property_data),
    })
}

/// Whether the current sample will emit an opt-in style-clock record.
///
/// Callers use this before collecting transition-property counts, keeping the
/// trace-disabled path free of an additional timeline scan.
pub(super) fn style_clock_record_requested(effects: StyleClockEffects) -> bool {
    style_clock_trace_enabled() && effects.changed() && effects.requires_layout()
}

/// Records a bounded summary when changed clocks require rebuilt geometry.
pub(super) fn record_style_clock_effects(
    document_id: usize,
    effects: StyleClockEffects,
    transition_properties: Option<TransitionPropertyCounts>,
) {
    if !style_clock_record_requested(effects) {
        return;
    }
    eprintln!(
        "{}",
        crate::diagnostics::json_record(
            "OMOIKANE_STYLE_CLOCK",
            &style_clock_data(document_id, effects, transition_properties)
        )
    );
}

/// Records only the bounded cache-policy outcome; CSS names and values are omitted.
pub(super) fn record_style_cache_invalidation(
    document_id: usize,
    node_id: usize,
    outcome: &'static str,
) {
    if style_cache_invalidation_trace_enabled() {
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_STYLE_CACHE_INVALIDATION",
                &serde_json::json!({
                    "document_id": document_id,
                    "node_id": node_id,
                    "outcome": outcome,
                })
            )
        );
    }
}

pub(super) struct ScrollTargetScope {
    started: Option<Instant>,
}

impl ScrollTargetScope {
    pub(super) fn begin(document_id: usize, node_id: usize, viewport: bool) -> Self {
        Self::begin_with_enabled(scroll_trace_enabled(), document_id, node_id, viewport)
    }

    fn begin_with_enabled(
        enabled: bool,
        document_id: usize,
        node_id: usize,
        viewport: bool,
    ) -> Self {
        if !enabled {
            return Self { started: None };
        }
        let target = ScrollTarget {
            document_id,
            node_id,
            viewport,
            layout: ScrollLayoutSummary::default(),
        };
        SCROLL_TARGETS.with(|targets| targets.borrow_mut().push(target));
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_SCROLL_TARGET",
                &serde_json::json!({
                    "phase": "begin", "document_id": document_id,
                    "target_id": node_id, "viewport": viewport,
                })
            )
        );
        Self {
            started: Some(Instant::now()),
        }
    }
}

impl Drop for ScrollTargetScope {
    fn drop(&mut self) {
        let Some(started) = self.started else { return };
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let target = SCROLL_TARGETS.with(|targets| targets.borrow_mut().pop());
        if let Some(target) = target {
            eprintln!(
                "{}",
                crate::diagnostics::json_record(
                    "OMOIKANE_SCROLL_TARGET",
                    &scroll_target_end_data(&target, elapsed_ms)
                )
            );
        }
    }
}

pub(super) fn record_scroll_invalidation(document_id: usize) {
    if !scroll_trace_enabled() {
        return;
    }
    SCROLL_TARGETS.with(|targets| {
        if let Some(target) = targets.borrow().last().copied() {
            eprintln!(
                "{}",
                crate::diagnostics::json_record(
                    "OMOIKANE_SCROLL_INVALIDATE",
                    &serde_json::json!({
                        "document_id": document_id, "target_id": target.node_id,
                        "target_document_id": target.document_id,
                    })
                )
            );
        }
    });
}

pub(super) fn record_scroll_mutation(
    api: &'static str,
    node_id: usize,
    attribute_name: Option<&str>,
) {
    if !scroll_trace_enabled() {
        return;
    }
    SCROLL_TARGETS.with(|targets| {
        if let Some(target) = targets.borrow().last().copied() {
            let kind = attribute_name.map_or("text", attribute_kind);
            eprintln!(
                "{}",
                crate::diagnostics::json_record(
                    "OMOIKANE_SCROLL_MUTATION",
                    &serde_json::json!({
                        "api": api, "kind": kind, "node_id": node_id,
                        "target_id": target.node_id, "document_id": target.document_id,
                    })
                )
            );
        }
    });
}

pub(super) fn attribute_kind(name: &str) -> &'static str {
    if name.eq_ignore_ascii_case("style") {
        "style"
    } else if name.eq_ignore_ascii_case("class") {
        "class"
    } else if name.eq_ignore_ascii_case("id") {
        "id"
    } else {
        "other"
    }
}

fn scroll_target_end_data(target: &ScrollTarget, elapsed_ms: f64) -> serde_json::Value {
    serde_json::json!({
        "phase": "end", "document_id": target.document_id,
        "target_id": target.node_id, "viewport": target.viewport,
        "elapsed_ms": elapsed_ms,
        "ensure_layout_calls": target.layout.calls,
        "ensure_layout_rebuilds": target.layout.rebuilds,
        "ensure_layout_cache_hits": target.layout.cache_hits(),
        "ensure_layout_ms": target.layout.total_ms,
        "ensure_layout_max_ms": target.layout.max_ms,
        "ensure_layout_rebuild_ms": target.layout.rebuild_total_ms,
        "ensure_layout_rebuild_max_ms": target.layout.rebuild_max_ms,
    })
}

fn record_scroll_layout(rebuilt: bool, elapsed_ms: f64) {
    SCROLL_TARGETS.with(|targets| {
        if let Some(target) = targets.borrow_mut().last_mut() {
            target.layout.record(rebuilt, elapsed_ms);
        }
    });
}

pub(super) struct FrameScope {
    started: Option<Instant>,
}

impl FrameScope {
    pub(super) fn begin(enabled: bool, document_id: usize) -> Self {
        if enabled {
            FRAMES.with(|frames| {
                frames.borrow_mut().push(FrameSample {
                    document_id,
                    ..FrameSample::default()
                })
            });
        }
        Self {
            started: enabled.then(Instant::now),
        }
    }

    pub(super) fn stage_start(&self) -> Option<Instant> {
        self.started.map(|_| Instant::now())
    }

    pub(super) fn record_scroll_events(&self, started: Option<Instant>) {
        self.record(started, |sample, ms| sample.scroll_events_ms += ms);
    }

    pub(super) fn record_media(&self, started: Option<Instant>) {
        self.record(started, |sample, ms| sample.media_ms += ms);
    }

    pub(super) fn record_raf_call(&self, started: Option<Instant>) {
        self.record(started, |sample, ms| sample.raf_call_ms += ms);
    }

    pub(super) fn record_jobs(&self, started: Option<Instant>) {
        self.record(started, |sample, ms| sample.jobs_ms += ms);
    }

    fn record(&self, started: Option<Instant>, update: impl FnOnce(&mut FrameSample, f64)) {
        if let Some(started) = started {
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            FRAMES.with(|frames| {
                if let Some(sample) = frames.borrow_mut().last_mut() {
                    update(sample, ms);
                }
            });
        }
    }

    pub(super) fn finish(mut self, callbacks_ms: f64) {
        if self.started.take().is_some() {
            emit_frame_sample(callbacks_ms, true);
        }
    }
}

impl Drop for FrameScope {
    fn drop(&mut self) {
        if let Some(started) = self.started.take() {
            emit_frame_sample(started.elapsed().as_secs_f64() * 1000.0, false);
        }
    }
}

fn emit_frame_sample(callbacks_ms: f64, completed: bool) {
    let sample = FRAMES.with(|frames| frames.borrow_mut().pop());
    if let Some(sample) = sample {
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_FRAME_CALLBACK_DETAIL",
                &serde_json::json!({
                    "document_id": sample.document_id,
                    "callbacks_ms": callbacks_ms,
                    "completed": completed,
                    "scroll_events_ms": sample.scroll_events_ms,
                    "media_ms": sample.media_ms,
                    "raf_call_ms": sample.raf_call_ms,
                    "jobs_ms": sample.jobs_ms,
                    "ensure_layout_calls": sample.layout_calls,
                    "ensure_layout_rebuilds": sample.layout_rebuilds,
                    "ensure_layout_ms": sample.layout_ms,
                })
            )
        );
    }
}

pub(super) struct LayoutSample {
    started: Option<Instant>,
    rebuilt: bool,
}

impl LayoutSample {
    pub(super) fn begin() -> Self {
        let enabled = FRAMES.with(|frames| !frames.borrow().is_empty());
        Self {
            started: enabled.then(Instant::now),
            rebuilt: false,
        }
    }

    pub(super) fn mark_rebuild(&mut self) {
        self.rebuilt = true;
    }
}

impl Drop for LayoutSample {
    fn drop(&mut self) {
        let Some(started) = self.started else {
            return;
        };
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        FRAMES.with(|frames| {
            if let Some(sample) = frames.borrow_mut().last_mut() {
                sample.layout_calls += 1;
                sample.layout_rebuilds += usize::from(self.rebuilt);
                sample.layout_ms += ms;
            }
        });
        record_scroll_layout(self.rebuilt, ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raf_source_labels_omit_credentials_queries_fragments_and_inline_code() {
        assert_eq!(
            source_label("https://user:password@example.test/render.js?token=secret#part"),
            "https://example.test/render.js"
        );
        assert_eq!(
            source_label("data:text/javascript,secret_inline_code"),
            "data:"
        );
        assert_eq!(source_label("/private/user/path.js"), "<inline-or-local>");
    }

    fn frame_depth() -> usize {
        FRAMES.with(|frames| frames.borrow().len())
    }

    fn scroll_target_ids() -> Vec<usize> {
        SCROLL_TARGETS.with(|targets| {
            targets
                .borrow()
                .iter()
                .map(|target| target.node_id)
                .collect()
        })
    }

    fn active_scroll_target() -> ScrollTarget {
        SCROLL_TARGETS.with(|targets| *targets.borrow().last().unwrap())
    }

    #[test]
    fn disabled_scroll_scope_does_not_create_timer_or_stack_entry() {
        assert!(scroll_target_ids().is_empty());
        let scope = ScrollTargetScope::begin_with_enabled(false, 1, 2, true);
        assert!(scope.started.is_none());
        assert!(scroll_target_ids().is_empty());
        drop(scope);
        assert!(scroll_target_ids().is_empty());
    }

    #[test]
    fn nested_scroll_scope_restores_outer_target_after_early_return() {
        fn inner_scope_returns_early(skip_remaining_work: bool) {
            let _inner = ScrollTargetScope::begin_with_enabled(true, 1, 22, false);
            assert_eq!(scroll_target_ids(), vec![11, 22]);
            record_scroll_layout(false, 2.0);
            record_scroll_layout(true, 4.0);
            assert_eq!(
                active_scroll_target().layout,
                ScrollLayoutSummary {
                    calls: 2,
                    rebuilds: 1,
                    total_ms: 6.0,
                    max_ms: 4.0,
                    rebuild_total_ms: 4.0,
                    rebuild_max_ms: 4.0,
                }
            );
            if skip_remaining_work {
                return;
            }
            panic!("this work should be skipped");
        }
        assert!(scroll_target_ids().is_empty());
        let outer = ScrollTargetScope::begin_with_enabled(true, 1, 11, true);
        record_scroll_layout(false, 1.0);
        inner_scope_returns_early(true);
        assert_eq!(scroll_target_ids(), vec![11]);
        assert_eq!(
            active_scroll_target().layout,
            ScrollLayoutSummary {
                calls: 1,
                total_ms: 1.0,
                max_ms: 1.0,
                ..ScrollLayoutSummary::default()
            }
        );
        record_scroll_layout(true, 8.0);
        assert_eq!(
            active_scroll_target().layout,
            ScrollLayoutSummary {
                calls: 2,
                rebuilds: 1,
                total_ms: 9.0,
                max_ms: 8.0,
                rebuild_total_ms: 8.0,
                rebuild_max_ms: 8.0,
            }
        );
        drop(outer);
        assert!(scroll_target_ids().is_empty());
    }

    #[test]
    fn scroll_target_aggregates_many_cache_hits_and_one_rebuild_exactly() {
        assert!(scroll_target_ids().is_empty());
        let scope = ScrollTargetScope::begin_with_enabled(true, 7, 77, false);
        for _ in 0..512 {
            record_scroll_layout(false, 0.25);
        }
        record_scroll_layout(true, 8.0);

        let target = active_scroll_target();
        assert_eq!(
            target.layout,
            ScrollLayoutSummary {
                calls: 513,
                rebuilds: 1,
                total_ms: 136.0,
                max_ms: 8.0,
                rebuild_total_ms: 8.0,
                rebuild_max_ms: 8.0,
            }
        );
        assert_eq!(
            scroll_target_end_data(&target, 150.0),
            serde_json::json!({
                "phase": "end", "document_id": 7,
                "target_id": 77, "viewport": false,
                "elapsed_ms": 150.0,
                "ensure_layout_calls": 513,
                "ensure_layout_rebuilds": 1,
                "ensure_layout_cache_hits": 512,
                "ensure_layout_ms": 136.0,
                "ensure_layout_max_ms": 8.0,
                "ensure_layout_rebuild_ms": 8.0,
                "ensure_layout_rebuild_max_ms": 8.0,
            })
        );
        drop(scope);
        assert!(scroll_target_ids().is_empty());
    }

    #[test]
    fn attribute_categories_are_bounded_labels() {
        assert_eq!(attribute_kind("STYLE"), "style");
        assert_eq!(attribute_kind("Class"), "class");
        assert_eq!(attribute_kind("id"), "id");
        assert_eq!(attribute_kind("data-private-value"), "other");
    }

    #[test]
    fn style_clock_record_has_only_bounded_effects_and_counts() {
        let effects = StyleClockEffects {
            transition_changed: true,
            transition_requires_layout: false,
            animation_changed: true,
            animation_requires_layout: true,
            retained_animation_elements: 2,
            retained_animation_pseudos: 3,
            changed_animation_elements: 1,
            changed_animation_pseudos: 2,
        };
        let transition_properties = TransitionPropertyCounts {
            nodes: 2,
            properties: 7,
            geometry: 1,
            transform: 1,
            color: 1,
            background: 1,
            filter: 1,
            opacity: 1,
            other: 1,
        };
        assert_eq!(
            style_clock_data(41, effects, Some(transition_properties)),
            serde_json::json!({
                "document_id": 41,
                "transition_changed": true,
                "transition_requires_layout": false,
                "animation_changed": true,
                "animation_requires_layout": true,
                "retained_animation_elements": 2,
                "retained_animation_pseudos": 3,
                "changed_animation_elements": 1,
                "changed_animation_pseudos": 2,
                "running_transition_properties": {
                    "nodes": 2,
                    "properties": 7,
                    "geometry": 1,
                    "transform": 1,
                    "color": 1,
                    "background": 1,
                    "filter": 1,
                    "opacity": 1,
                    "other": 1,
                },
            })
        );
        assert_eq!(
            style_clock_data(41, effects, None)["running_transition_properties"],
            serde_json::Value::Null
        );
        assert!(effects.changed());
        assert!(effects.requires_layout());
    }

    #[test]
    fn disabled_scope_has_no_frame_or_clock() {
        assert_eq!(frame_depth(), 0);
        let scope = FrameScope::begin(false, 1);
        assert!(scope.stage_start().is_none());
        assert!(LayoutSample::begin().started.is_none());
        scope.finish(0.0);
        assert_eq!(frame_depth(), 0);
    }

    #[test]
    fn nested_scopes_and_aborted_scope_pop_once() {
        assert_eq!(frame_depth(), 0);
        let outer = FrameScope::begin(true, 11);
        {
            let _aborted = FrameScope::begin(true, 22);
            assert_eq!(frame_depth(), 2);
            FRAMES.with(|frames| assert_eq!(frames.borrow().last().unwrap().document_id, 22));
        }
        assert_eq!(frame_depth(), 1);
        FRAMES.with(|frames| assert_eq!(frames.borrow().last().unwrap().document_id, 11));
        outer.finish(0.0);
        assert_eq!(frame_depth(), 0);
    }

    #[test]
    fn layout_sample_counts_calls_and_rebuilds_in_active_frame() {
        assert_eq!(frame_depth(), 0);
        let scope = FrameScope::begin(true, 33);
        drop(LayoutSample::begin());
        let mut rebuild = LayoutSample::begin();
        rebuild.mark_rebuild();
        drop(rebuild);
        FRAMES.with(|frames| {
            let frames = frames.borrow();
            let sample = frames.last().unwrap();
            assert_eq!(sample.layout_calls, 2);
            assert_eq!(sample.layout_rebuilds, 1);
        });
        scope.finish(0.0);
        assert_eq!(frame_depth(), 0);
    }
}
