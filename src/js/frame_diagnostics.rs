//! Optional per-rendering-opportunity diagnostics for synchronous page work.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::Instant;

use super::event_loop::ScriptSourceLocation;
use crate::css::{
    TransitionPropertyCounts, style::StyleClockEffects, style_profile::StyleProfileSummary,
};

const MAX_CHILD_RASTER_DETAILS: usize = 32;

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

fn paint_attribution_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        paint_attribution_requested_with_flags(
            std::env::var_os("OMOIKANE_TRACE_PAINT").is_some(),
            std::env::var_os("OMOIKANE_TRACE_PAINT_ATTRIBUTION").is_some(),
        )
    })
}

fn paint_attribution_requested_with_flags(paint: bool, attribution: bool) -> bool {
    paint && attribution
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ChildRasterDetail {
    document_id: usize,
    raster_ms: f64,
    style: StyleProfileSummary,
    raster: crate::paint::PaintRasterSummary,
}

#[derive(Debug, Default, PartialEq)]
struct PaintAttributionSample {
    document_id: usize,
    snapshot_collection_ms: f64,
    snapshot_candidate_owners: usize,
    child_rasters: usize,
    child_raster_ms: f64,
    child_style_scope_ms: f64,
    child_style_cache_hits: usize,
    child_style_cache_misses: usize,
    child_style_outer_compute_ms: f64,
    child_raster: crate::paint::PaintRasterSummary,
    child_details: Vec<ChildRasterDetail>,
    child_raster_detail_overflow: usize,
    main_raster_ms: f64,
    main_style: StyleProfileSummary,
    main_raster: crate::paint::PaintRasterSummary,
}

/// Owns one opt-in top-level paint attribution sample without borrowing the
/// runtime, style resolver, layout tree, or nested-document images.
pub(super) struct PaintAttributionScope {
    sample: Option<PaintAttributionSample>,
}

impl PaintAttributionScope {
    pub(super) fn begin(document_id: usize) -> Self {
        Self::begin_with_enabled(paint_attribution_trace_enabled(), document_id)
    }

    pub(super) fn begin_with_enabled(enabled: bool, document_id: usize) -> Self {
        Self {
            sample: enabled.then(|| PaintAttributionSample {
                document_id,
                ..PaintAttributionSample::default()
            }),
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.sample.is_some()
    }

    /// Starts a stage timer only while this opt-in sample is active.
    pub(super) fn stage_start(&self) -> Option<Instant> {
        self.enabled().then(Instant::now)
    }

    pub(super) fn record_snapshot_candidate(&mut self) {
        if let Some(sample) = &mut self.sample {
            sample.snapshot_candidate_owners = sample.snapshot_candidate_owners.saturating_add(1);
        }
    }

    pub(super) fn record_snapshot_collection(&mut self, started: Option<Instant>) {
        if let Some(started) = started {
            self.record_snapshot_collection_ms(started.elapsed().as_secs_f64() * 1000.0);
        }
    }

    fn record_snapshot_collection_ms(&mut self, elapsed_ms: f64) {
        if let Some(sample) = &mut self.sample {
            sample.snapshot_collection_ms += elapsed_ms;
        }
    }

    pub(super) fn record_child_raster(
        &mut self,
        document_id: usize,
        started: Option<Instant>,
        style: Option<StyleProfileSummary>,
        raster: crate::paint::PaintRasterSummary,
    ) {
        let Some(started) = started else { return };
        self.record_child_raster_ms(
            document_id,
            started.elapsed().as_secs_f64() * 1000.0,
            style.unwrap_or_default(),
            raster,
        );
    }

    fn record_child_raster_ms(
        &mut self,
        document_id: usize,
        raster_ms: f64,
        style: StyleProfileSummary,
        raster: crate::paint::PaintRasterSummary,
    ) {
        let Some(sample) = &mut self.sample else {
            return;
        };
        sample.child_rasters = sample.child_rasters.saturating_add(1);
        sample.child_raster_ms += raster_ms;
        sample.child_style_scope_ms += style.scope_ms;
        sample.child_style_cache_hits = sample
            .child_style_cache_hits
            .saturating_add(style.cache_hits);
        sample.child_style_cache_misses = sample
            .child_style_cache_misses
            .saturating_add(style.cache_misses);
        sample.child_style_outer_compute_ms += style.outer_compute_ms;
        sample.child_raster = sample.child_raster.merged(raster);
        let detail = ChildRasterDetail {
            document_id,
            raster_ms,
            style,
            raster,
        };
        if sample.child_details.len() < MAX_CHILD_RASTER_DETAILS {
            sample.child_details.push(detail);
        } else {
            sample.child_raster_detail_overflow =
                sample.child_raster_detail_overflow.saturating_add(1);
        }
    }

    pub(super) fn record_main_raster(
        &mut self,
        started: Option<Instant>,
        style: Option<StyleProfileSummary>,
        raster: crate::paint::PaintRasterSummary,
    ) {
        let Some(started) = started else { return };
        self.record_main_raster_ms(
            started.elapsed().as_secs_f64() * 1000.0,
            style.unwrap_or_default(),
            raster,
        );
    }

    fn record_main_raster_ms(
        &mut self,
        raster_ms: f64,
        style: StyleProfileSummary,
        raster: crate::paint::PaintRasterSummary,
    ) {
        if let Some(sample) = &mut self.sample {
            sample.main_raster_ms += raster_ms;
            sample.main_style = style;
            sample.main_raster = raster;
        }
    }

    pub(super) fn finish(mut self) {
        if let Some(sample) = self.sample.take() {
            emit_paint_attribution(&sample, true);
        }
    }

    #[cfg(test)]
    pub(super) fn finish_data_for_test(mut self, completed: bool) -> Option<serde_json::Value> {
        self.sample
            .take()
            .map(|sample| paint_attribution_data(&sample, completed))
    }
}

impl Drop for PaintAttributionScope {
    fn drop(&mut self) {
        if let Some(sample) = self.sample.take() {
            emit_paint_attribution(&sample, false);
        }
    }
}

fn child_raster_data(detail: &ChildRasterDetail) -> serde_json::Value {
    serde_json::json!({
        "document_id": detail.document_id,
        "raster_ms": detail.raster_ms,
        "style_scope_ms": detail.style.scope_ms,
        "style_cache_hits": detail.style.cache_hits,
        "style_cache_misses": detail.style.cache_misses,
        "style_outer_compute_ms": detail.style.outer_compute_ms,
        "nested_snapshot_draw_calls": detail.raster.nested_snapshot_draw_calls,
        "shadow_masks": shadow_mask_data(detail.raster.shadow_masks, false),
    })
}

fn shadow_mask_data(
    summary: crate::paint::border::ShadowMaskCacheSummary,
    child_aggregate: bool,
) -> serde_json::Value {
    let mut data = serde_json::json!({
        "eligible": summary.eligible,
        "generated": summary.generated,
        "hits": summary.hits,
        "same_raster_hits": summary.same_raster_hits,
        "prior_raster_hits": summary.prior_raster_hits,
        "generation_rgba_bytes_avoided": summary.generation_rgba_bytes_avoided,
        "bypassed_unkeyable": summary.bypassed_unkeyable,
        "bypassed_oversize": summary.bypassed_oversize,
        "bypassed_capacity": summary.bypassed_capacity,
    });
    let object = data.as_object_mut().expect("shadow mask data is an object");
    if child_aggregate {
        object.insert(
            "retained_entries_sum".to_owned(),
            summary.retained_entries.into(),
        );
        object.insert(
            "retained_rgba_bytes_sum".to_owned(),
            summary.retained_rgba_bytes.into(),
        );
    } else {
        object.insert(
            "retained_entries".to_owned(),
            summary.retained_entries.into(),
        );
        object.insert(
            "retained_rgba_bytes".to_owned(),
            summary.retained_rgba_bytes.into(),
        );
    }
    data
}

fn paint_attribution_data(sample: &PaintAttributionSample, completed: bool) -> serde_json::Value {
    let snapshot_draw_calls = sample
        .child_raster
        .nested_snapshot_draw_calls
        .saturating_add(sample.main_raster.nested_snapshot_draw_calls);
    serde_json::json!({
        "document_id": sample.document_id,
        "completed": completed,
        "style_counts_ordinary_only": true,
        "style_counts_exclude_nested_scopes": true,
        "snapshot_collection_ms": sample.snapshot_collection_ms,
        "snapshot_candidate_owners": sample.snapshot_candidate_owners,
        "snapshot_draw_calls": snapshot_draw_calls,
        "child_rasters": sample.child_rasters,
        "child_raster_ms": sample.child_raster_ms,
        "child_style_scope_ms": sample.child_style_scope_ms,
        "child_style_cache_hits": sample.child_style_cache_hits,
        "child_style_cache_misses": sample.child_style_cache_misses,
        "child_style_outer_compute_ms": sample.child_style_outer_compute_ms,
        "child_nested_snapshot_draw_calls": sample.child_raster.nested_snapshot_draw_calls,
        "child_shadow_masks": shadow_mask_data(sample.child_raster.shadow_masks, true),
        "child_details": sample.child_details.iter().map(child_raster_data).collect::<Vec<_>>(),
        "child_raster_detail_overflow": sample.child_raster_detail_overflow,
        "main_raster_ms": sample.main_raster_ms,
        "main_style_scope_ms": sample.main_style.scope_ms,
        "main_style_cache_hits": sample.main_style.cache_hits,
        "main_style_cache_misses": sample.main_style.cache_misses,
        "main_style_outer_compute_ms": sample.main_style.outer_compute_ms,
        "main_snapshot_draw_calls": sample.main_raster.nested_snapshot_draw_calls,
        "main_shadow_masks": shadow_mask_data(sample.main_raster.shadow_masks, false),
    })
}

fn emit_paint_attribution(sample: &PaintAttributionSample, completed: bool) {
    eprintln!(
        "{}",
        crate::diagnostics::json_record(
            "OMOIKANE_PAINT_ATTRIBUTION",
            &paint_attribution_data(sample, completed),
        )
    );
}

fn all_style_clocks_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_STYLE_CLOCK_ALL").is_some())
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
        "transition_cache_before_animation": {
            "outcome": effects.transition_cache_before_animation.outcome.as_str(),
            "ordinary_evicted": effects.transition_cache_before_animation.ordinary_evicted,
            "ordinary_retained": effects.transition_cache_before_animation.ordinary_retained,
            "pseudo_evicted": effects.transition_cache_before_animation.pseudo_evicted,
            "pseudo_retained": effects.transition_cache_before_animation.pseudo_retained,
        },
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
    style_clock_record_requested_with_flags(
        style_clock_trace_enabled(),
        all_style_clocks_trace_enabled(),
        effects,
    )
}

fn style_clock_record_requested_with_flags(
    default_enabled: bool,
    all_enabled: bool,
    effects: StyleClockEffects,
) -> bool {
    effects.changed() && (all_enabled || (default_enabled && effects.requires_layout()))
}

/// Records a bounded summary for geometry-changing clocks or the all-clock trace.
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

/// Bounded geometry-policy evidence under the existing attribute trace flag.
pub(super) fn record_leaf_paint_reuse(
    node_id: usize,
    mutation: &'static str,
    reason: &'static str,
) {
    if style_cache_invalidation_trace_enabled() {
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_LEAF_PAINT_REUSE",
                &serde_json::json!({"node_id": node_id, "mutation": mutation, "reason": reason, "kept_geometry": reason == "keep"}),
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

    fn raster_summary(nested_snapshot_draw_calls: usize) -> crate::paint::PaintRasterSummary {
        crate::paint::PaintRasterSummary {
            nested_snapshot_draw_calls,
            shadow_masks: crate::paint::border::ShadowMaskCacheSummary {
                eligible: 3,
                generated: 4,
                hits: 1,
                same_raster_hits: 1,
                prior_raster_hits: 0,
                generation_rgba_bytes_avoided: 4,
                bypassed_unkeyable: 1,
                bypassed_oversize: 1,
                bypassed_capacity: 1,
                retained_entries: 1,
                retained_rgba_bytes: 8,
            },
        }
    }

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
            transition_cache_before_animation: crate::css::style::TransitionCacheEffects {
                outcome: crate::css::style::TransitionCacheOutcome::Subtree,
                ordinary_evicted: 3,
                ordinary_retained: 5,
                pseudo_evicted: 2,
                pseudo_retained: 7,
            },
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
                "transition_cache_before_animation": {
                    "outcome": "subtree",
                    "ordinary_evicted": 3,
                    "ordinary_retained": 5,
                    "pseudo_evicted": 2,
                    "pseudo_retained": 7,
                },
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
    fn style_clock_trace_all_includes_paint_only_changes() {
        let paint_only = StyleClockEffects {
            transition_changed: true,
            ..StyleClockEffects::default()
        };
        assert!(!style_clock_record_requested_with_flags(
            true, false, paint_only
        ));
        assert!(style_clock_record_requested_with_flags(
            false, true, paint_only
        ));

        let layout = StyleClockEffects {
            transition_changed: true,
            transition_requires_layout: true,
            ..StyleClockEffects::default()
        };
        assert!(style_clock_record_requested_with_flags(true, false, layout));
        assert!(!style_clock_record_requested_with_flags(
            true,
            true,
            StyleClockEffects::default()
        ));
    }

    #[test]
    fn disabled_paint_attribution_has_no_sample_or_stage_timer() {
        let mut scope = PaintAttributionScope::begin_with_enabled(false, 5);
        assert!(!scope.enabled());
        assert!(scope.stage_start().is_none());
        scope.record_snapshot_candidate();
        scope.record_snapshot_collection_ms(10.0);
        scope.record_child_raster_ms(2, 20.0, StyleProfileSummary::default(), raster_summary(3));
        scope.record_main_raster_ms(30.0, StyleProfileSummary::default(), raster_summary(4));
        assert_eq!(scope.finish_data_for_test(true), None);
    }

    #[test]
    fn paint_attribution_requires_both_trace_flags() {
        assert!(!paint_attribution_requested_with_flags(false, false));
        assert!(!paint_attribution_requested_with_flags(true, false));
        assert!(!paint_attribution_requested_with_flags(false, true));
        assert!(paint_attribution_requested_with_flags(true, true));
    }

    #[test]
    fn paint_attribution_schema_aggregates_and_bounds_child_details_exactly() {
        let mut scope = PaintAttributionScope::begin_with_enabled(true, 5);
        for _ in 0..3 {
            scope.record_snapshot_candidate();
        }
        scope.record_snapshot_collection_ms(20.0);
        for index in 0..34 {
            scope.record_child_raster_ms(
                100 + index,
                1.0,
                StyleProfileSummary {
                    document_id: 100 + index,
                    scope_ms: 2.0,
                    cache_hits: 2,
                    cache_misses: 1,
                    outer_compute_ms: 0.5,
                },
                raster_summary(1),
            );
        }
        scope.record_main_raster_ms(
            7.0,
            StyleProfileSummary {
                document_id: 1,
                scope_ms: 8.0,
                cache_hits: 10,
                cache_misses: 3,
                outer_compute_ms: 1.5,
            },
            raster_summary(2),
        );
        let details = (0..MAX_CHILD_RASTER_DETAILS)
            .map(|index| {
                serde_json::json!({
                    "document_id": 100 + index,
                    "raster_ms": 1.0,
                    "style_scope_ms": 2.0,
                    "style_cache_hits": 2,
                    "style_cache_misses": 1,
                    "style_outer_compute_ms": 0.5,
                    "nested_snapshot_draw_calls": 1,
                    "shadow_masks": {
                        "eligible": 3,
                        "generated": 4,
                        "hits": 1,
                        "same_raster_hits": 1,
                        "prior_raster_hits": 0,
                        "generation_rgba_bytes_avoided": 4,
                        "bypassed_unkeyable": 1,
                        "bypassed_oversize": 1,
                        "bypassed_capacity": 1,
                        "retained_entries": 1,
                        "retained_rgba_bytes": 8,
                    },
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            scope.finish_data_for_test(true),
            Some(serde_json::json!({
                "document_id": 5,
                "completed": true,
                "style_counts_ordinary_only": true,
                "style_counts_exclude_nested_scopes": true,
                "snapshot_collection_ms": 20.0,
                "snapshot_candidate_owners": 3,
                "snapshot_draw_calls": 36,
                "child_rasters": 34,
                "child_raster_ms": 34.0,
                "child_style_scope_ms": 68.0,
                "child_style_cache_hits": 68,
                "child_style_cache_misses": 34,
                "child_style_outer_compute_ms": 17.0,
                "child_nested_snapshot_draw_calls": 34,
                "child_shadow_masks": {
                    "eligible": 102,
                    "generated": 136,
                    "hits": 34,
                    "same_raster_hits": 34,
                    "prior_raster_hits": 0,
                    "generation_rgba_bytes_avoided": 136,
                    "bypassed_unkeyable": 34,
                    "bypassed_oversize": 34,
                    "bypassed_capacity": 34,
                    "retained_entries_sum": 34,
                    "retained_rgba_bytes_sum": 272,
                },
                "child_details": details,
                "child_raster_detail_overflow": 2,
                "main_raster_ms": 7.0,
                "main_style_scope_ms": 8.0,
                "main_style_cache_hits": 10,
                "main_style_cache_misses": 3,
                "main_style_outer_compute_ms": 1.5,
                "main_snapshot_draw_calls": 2,
                "main_shadow_masks": {
                    "eligible": 3,
                    "generated": 4,
                    "hits": 1,
                    "same_raster_hits": 1,
                    "prior_raster_hits": 0,
                    "generation_rgba_bytes_avoided": 4,
                    "bypassed_unkeyable": 1,
                    "bypassed_oversize": 1,
                    "bypassed_capacity": 1,
                    "retained_entries": 1,
                    "retained_rgba_bytes": 8,
                },
            }))
        );
    }

    #[test]
    fn aborted_paint_attribution_uses_the_same_bounded_schema() {
        let mut scope = PaintAttributionScope::begin_with_enabled(true, 5);
        scope.record_snapshot_candidate();
        let data = scope.finish_data_for_test(false).unwrap();
        assert_eq!(data["document_id"], 5);
        assert_eq!(data["completed"], false);
        assert_eq!(data["style_counts_ordinary_only"], true);
        assert_eq!(data["style_counts_exclude_nested_scopes"], true);
        assert_eq!(data["snapshot_candidate_owners"], 1);
        assert_eq!(data["child_details"], serde_json::json!([]));
        assert_eq!(data["snapshot_draw_calls"], 0);
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
