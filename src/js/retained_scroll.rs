//! Owner-local capture of retained pixels using the current document inputs.
use super::*;
use crate::paint::{
    RetainedOverscanFrame, RetainedOverscanOutcome, RetainedPaintError, VerticalOverscanPlan,
};

/// Pixels and explicit geometry only; this value may outlive its source runtime.
pub(crate) struct CapturedScrollScene {
    pub(crate) frame: Arc<RetainedOverscanFrame>,
    pub(crate) logical_scroll: (f64, f64),
    pub(crate) extent: (f64, f64),
    document_id: usize,
    generations: RenderGenerations,
    viewport: Rect,
}

#[derive(Default)]
pub(super) struct RetainedScrollCapture {
    configured: bool,
    interested: bool,
    trace: bool,
    last: Option<CapturedScrollScene>,
    reusable: Option<ReusableCapture>,
    reason: Option<RetainedPaintError>,
    refresh_rejected: Option<&'static str>,
    reuse_miss: Option<&'static str>,
    clip_diagnostic: Option<crate::paint::CrossSpaceClipDiagnostic>,
}

struct ReusableCapture {
    frame: Arc<RetainedOverscanFrame>,
    snapshots: HashMap<usize, crate::paint::Image>,
    document_id: usize,
    main_revision: DocumentPaintRevision,
    style_generation: u64,
    layout_generation: u64,
    viewport: Rect,
    extent: (f64, f64),
    visited_generation: u64,
    main_image_frames: Vec<(Arc<crate::paint::animation::ImagePlayback>, usize)>,
    document_url: Option<String>,
}

/// A single paint's owned raw layout, consumed before leaving the owner thread.
pub(super) struct CaptureRequest {
    layout: LayoutBox,
    viewport: Rect,
    plan: VerticalOverscanPlan,
    logical_scroll: (f64, f64),
    extent: (f64, f64),
    document_id: usize,
    generations: RenderGenerations,
    main_revision: DocumentPaintRevision,
    style_generation: u64,
    layout_generation: u64,
    visited_generation: u64,
    main_image_frames: Vec<(Arc<crate::paint::animation::ImagePlayback>, usize)>,
    document_url: Option<String>,
}

fn generations(state: &HostState) -> RenderGenerations {
    RenderGenerations {
        style: state.style_generation,
        layout: state.layout_generation,
        paint: state.paint_generation,
    }
}

fn snapshots_exact(
    left: &HashMap<usize, crate::paint::Image>,
    right: &HashMap<usize, crate::paint::Image>,
) -> bool {
    left.len() == right.len()
        && left.iter().all(|(owner, image)| {
            right
                .get(owner)
                .is_some_and(|candidate| image.retained_source_exact(candidate))
        })
}

fn snapshot_mismatch(
    left: &HashMap<usize, crate::paint::Image>,
    right: &HashMap<usize, crate::paint::Image>,
) -> Option<&'static str> {
    if left.len() != right.len() || left.keys().any(|owner| !right.contains_key(owner)) {
        return Some("snapshot_owner");
    }
    for (owner, image) in left {
        let candidate = &right[owner];
        if !image.retained_pixels_exact(candidate) {
            return Some("snapshot_pixels");
        }
        if !image.retained_source_exact(candidate) {
            return Some("snapshot_metadata");
        }
    }
    None
}

fn integer_scroll(scroll: (f64, f64)) -> Option<(i32, i32)> {
    (scroll.0.fract() == 0.0
        && scroll.1.fract() == 0.0
        && scroll.0 >= f64::from(i32::MIN)
        && scroll.0 <= f64::from(i32::MAX)
        && scroll.1 >= f64::from(i32::MIN)
        && scroll.1 <= f64::from(i32::MAX))
    .then_some((scroll.0 as i32, scroll.1 as i32))
}

fn main_image_frames(
    state: &HostState,
    document_id: usize,
) -> Vec<(Arc<crate::paint::animation::ImagePlayback>, usize)> {
    let animation_time = state.event_loop.rendering_time_ms() as u64;
    state
        .visible_image_playbacks
        .iter()
        .filter(|playback| playback.document == document_id)
        .map(|playback| (Arc::clone(playback), playback.frame_index(animation_time)))
        .collect()
}

fn plan(viewport: Rect, scroll: (f32, f32)) -> Result<VerticalOverscanPlan, RetainedPaintError> {
    let height = f64::from(viewport.height.ceil());
    let x = f64::from(scroll.0);
    let y = f64::from(scroll.1);
    if !height.is_finite()
        || height <= 0.0
        || height > f64::from(i32::MAX)
        || !x.is_finite()
        || x < 0.0
        || x.fract() != 0.0
        || x > f64::from(i32::MAX)
        || !y.is_finite()
        || y < 0.0
        || y > f64::from(i32::MAX)
    {
        return Err(RetainedPaintError::InvalidOverscanPlan);
    }
    let tile = (y / height).floor();
    let current_index = u8::from(tile >= 1.0);
    let first_y = ((tile - f64::from(current_index)) * height) as i32;
    VerticalOverscanPlan::new(x as i32, first_y, 3, current_index)?.with_focus_y(y.floor() as i32)
}

impl RetainedScrollCapture {
    fn active(&self) -> bool {
        self.configured && self.interested
    }

    pub(super) fn clear_frame(&mut self) {
        self.last = None;
        self.reason = None;
        self.refresh_rejected = None;
        self.reuse_miss = None;
        self.clip_diagnostic = None;
    }

    fn clear_all(&mut self) {
        self.clear_frame();
        self.reusable = None;
    }

    pub(super) fn prepare(&mut self, state: &mut HostState) -> Option<CaptureRequest> {
        if !self.active() {
            return None;
        }
        let document_id = state.document.identity();
        if self
            .reusable
            .as_ref()
            .is_some_and(|capture| capture.document_id != document_id)
        {
            self.reusable = None;
        }
        let plan = match plan(state.viewport, state.window_scroll) {
            Ok(plan) => plan,
            Err(reason) => {
                self.reason = Some(reason);
                return None;
            }
        };
        let extent = state.window_scroll_extent();
        let main_image_frames = main_image_frames(state, document_id);
        Some(CaptureRequest {
            layout: state.layout_root.as_ref()?.clone(),
            viewport: state.viewport,
            plan,
            logical_scroll: (
                f64::from(state.window_scroll.0),
                f64::from(state.window_scroll.1),
            ),
            extent: (f64::from(extent.0), f64::from(extent.1)),
            document_id,
            generations: generations(state),
            main_revision: state.document_paint_revision(state.document.identity()),
            style_generation: state.style_generation,
            layout_generation: state.layout_generation,
            visited_generation: state.storage_manager.visited_generation(),
            main_image_frames,
            document_url: state
                .base_url_for_document(document_id)
                .map(|url| url.to_string()),
        })
    }

    /// Child snapshot collection may advance child style clocks and therefore
    /// the global paint generation. Publish against that completed snapshot
    /// only when every main-document input captured by the request is still
    /// unchanged; otherwise force the ordinary fail-closed path.
    pub(super) fn refresh_after_snapshots(
        &mut self,
        request: &mut Option<CaptureRequest>,
        state: &HostState,
    ) {
        let Some(pending) = request.as_mut() else {
            return;
        };
        let document_id = state.document.identity();
        let mismatch = if pending.document_id != document_id {
            Some("document")
        } else if pending.main_revision != state.document_paint_revision(document_id) {
            Some("main_revision")
        } else if pending.style_generation != state.style_generation {
            Some("style_generation")
        } else if pending.layout_generation != state.layout_generation {
            Some("layout_generation")
        } else if pending.viewport != state.viewport {
            Some("viewport")
        } else if pending.logical_scroll
            != (
                f64::from(state.window_scroll.0),
                f64::from(state.window_scroll.1),
            )
        {
            Some("scroll")
        } else if pending.visited_generation != state.storage_manager.visited_generation() {
            Some("visited")
        } else if pending.main_image_frames != main_image_frames(state, document_id) {
            Some("main_images")
        } else if pending.document_url
            != state
                .base_url_for_document(document_id)
                .map(|url| url.to_string())
        {
            Some("document_url")
        } else {
            None
        };
        if mismatch.is_none() {
            pending.generations = generations(state);
            self.refresh_rejected = None;
        } else {
            self.refresh_rejected = mismatch;
            *request = None;
        }
    }

    /// Reuses exact capture pixels for display, or paints the current adjusted
    /// layout when the capture does not provide matching canonical pixels.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_current(
        &mut self,
        request: Option<CaptureRequest>,
        diagnostics_enabled: bool,
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        viewport: Rect,
        web_fonts: &crate::font::WebFontRegistry,
        visited: &[usize],
        snapshots: &HashMap<usize, crate::paint::Image>,
        shadows: &crate::paint::border::FrameShadowMaskCache,
    ) -> (crate::paint::Canvas, crate::paint::PaintRasterSummary) {
        let snapshot_draws = crate::paint::NestedSnapshotDrawScope::begin(diagnostics_enabled);
        let result = if let Some(canvas) =
            self.capture(request, resolver, web_fonts, visited, snapshots, shadows)
        {
            // These counters describe the ordinary raster pass, which reuse skips.
            (canvas, crate::paint::PaintRasterSummary::default())
        } else {
            nested_rendering::paint_layout_with_snapshot_attribution(
                diagnostics_enabled,
                layout,
                resolver,
                viewport,
                web_fonts,
                visited.iter().copied(),
                snapshots,
                Some(shadows),
                None,
            )
        };
        let (canvas, mut summary) = result;
        let draws = snapshot_draws.finish();
        summary.nested_snapshot_draw_calls = draws.nested_snapshot_draw_calls;
        summary.nested_snapshot_owner_ids = draws.nested_snapshot_owner_ids;
        summary.nested_snapshot_owner_count = draws.nested_snapshot_owner_count;
        summary.nested_snapshot_owner_overflow = draws.nested_snapshot_owner_overflow;
        (canvas, summary)
    }

    /// Returns an ordinary display image from the exact current capture raster
    /// when its integer scroll matches the requested display. Fractional scroll
    /// and nonmatching samples still require the ordinary painter.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn capture(
        &mut self,
        request: Option<CaptureRequest>,
        resolver: &mut StyleResolver,
        web_fonts: &crate::font::WebFontRegistry,
        visited: &[usize],
        snapshots: &HashMap<usize, crate::paint::Image>,
        shadows: &crate::paint::border::FrameShadowMaskCache,
    ) -> Option<crate::paint::Canvas> {
        let Some(request) = request else {
            self.trace_result(None, false, false);
            return None;
        };
        let started = self.trace.then(Instant::now);
        let reuse = self.reusable.as_ref().map(|reusable| {
            let mismatch = if reusable.document_id != request.document_id {
                Some("document")
            } else if reusable.main_revision != request.main_revision {
                Some("main_revision")
            } else if reusable.style_generation != request.style_generation {
                Some("style_generation")
            } else if reusable.layout_generation != request.layout_generation {
                Some("layout_generation")
            } else if reusable.visited_generation != request.visited_generation {
                Some("visited")
            } else if reusable.main_image_frames != request.main_image_frames {
                Some("main_images")
            } else if reusable.document_url != request.document_url {
                Some("document_url")
            } else if reusable.viewport != request.viewport {
                Some("viewport")
            } else if reusable.extent != request.extent {
                Some("extent")
            } else {
                snapshot_mismatch(&reusable.snapshots, snapshots)
            };
            if let Some(mismatch) = mismatch {
                return Err(mismatch);
            }
            let Some(scroll) = integer_scroll(request.logical_scroll) else {
                return Err("fractional_scroll");
            };
            reusable
                .frame
                .compose_integer(scroll)
                .map_err(|error| match error {
                    RetainedPaintError::OutsideOverscanCoverage => "coverage",
                    _ => "compose",
                })
        });
        match reuse {
            Some(Ok(canvas)) => {
                self.reuse_miss = None;
                let reusable = self.reusable.as_ref().expect("reuse source");
                self.last = Some(CapturedScrollScene {
                    frame: Arc::clone(&reusable.frame),
                    logical_scroll: request.logical_scroll,
                    extent: request.extent,
                    document_id: request.document_id,
                    generations: request.generations,
                    viewport: request.viewport,
                });
                self.trace_result(started, true, true);
                return Some(canvas);
            }
            Some(Err(reason)) => self.reuse_miss = Some(reason),
            None => self.reuse_miss = Some("cold"),
        }
        let painted = crate::paint::paint_layout_with_vertical_overscan_current(
            &request.layout,
            resolver,
            request.viewport,
            request.plan,
            &crate::paint::text::load_text_fonts(),
            Some(web_fonts),
            visited,
            snapshots,
            shadows,
            Some(request.logical_scroll),
        );
        let mut current_canvas = painted.canonical;
        match painted.outcome {
            RetainedOverscanOutcome::Extracted(frame) => {
                let frame = Arc::new(frame);
                self.reusable = Some(ReusableCapture {
                    frame: Arc::clone(&frame),
                    snapshots: snapshots.clone(),
                    document_id: request.document_id,
                    main_revision: request.main_revision,
                    style_generation: request.style_generation,
                    layout_generation: request.layout_generation,
                    visited_generation: request.visited_generation,
                    main_image_frames: request.main_image_frames.clone(),
                    document_url: request.document_url.clone(),
                    viewport: request.viewport,
                    extent: request.extent,
                });
                self.last = Some(CapturedScrollScene {
                    frame,
                    logical_scroll: request.logical_scroll,
                    extent: request.extent,
                    document_id: request.document_id,
                    generations: request.generations,
                    viewport: request.viewport,
                });
            }
            RetainedOverscanOutcome::Fallback { canvas, reason, .. } => {
                if painted.fallback_is_current {
                    current_canvas = Some(canvas);
                }
                self.record_failure(reason, &request.layout, resolver);
            }
            RetainedOverscanOutcome::Rejected { reason } => {
                self.record_failure(reason, &request.layout, resolver);
            }
        }
        self.trace_result(started, current_canvas.is_some(), false);
        current_canvas
    }

    fn record_failure(
        &mut self,
        reason: RetainedPaintError,
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
    ) {
        self.reason = Some(reason);
        if self.trace && reason == RetainedPaintError::CrossSpaceClip {
            self.clip_diagnostic = crate::paint::diagnose_cross_space_clip(layout, resolver);
        }
    }

    fn trace_result(&self, started: Option<Instant>, canonical_available: bool, reused: bool) {
        if !self.trace {
            return;
        }
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_RETAINED_CAPTURE",
                &serde_json::json!({
                    "extracted": self.last.is_some(),
                    "canonical_available": canonical_available,
                    "reused": reused,
                    "reason": self.reason.map(|reason| format!("{reason:?}")),
                    "refresh_rejected": self.refresh_rejected,
                    "reuse_miss": self.reuse_miss,
                    "capture_ms": started.map(|at| at.elapsed().as_secs_f64() * 1000.0),
                    "coverage": self.last.as_ref().map(|scene| scene.frame.coverage()),
                    "logical_scroll": self.last.as_ref().map(|scene| scene.logical_scroll),
                    "raster_scale": 1.0,
                "clip_diagnostic": self.clip_diagnostic,
                }),
            )
        );
    }
}

impl JsRuntime {
    /// Enables bounded retained capture on subsequent paints. No input is
    /// accepted by this switch; native policy and acknowledgement remain separate.
    pub(crate) fn set_retained_scroll_capture(&mut self, enabled: bool) {
        if self.retained_scroll.configured == enabled {
            return;
        }
        let was_active = self.retained_scroll.active();
        self.retained_scroll.configured = enabled;
        self.retained_scroll.interested = enabled;
        // The legacy trace switch also asks the GUI owner to force capture.
        // The reuse-only switch is deliberately passive: it emits diagnostics
        // only for captures selected by the normal native-scroll interest path.
        self.retained_scroll.trace = enabled
            && (std::env::var_os("OMOIKANE_TRACE_RETAINED_SCROLL").is_some()
                || std::env::var_os("OMOIKANE_TRACE_RETAINED_REUSE").is_some());
        if enabled {
            self.retained_scroll.clear_frame();
        } else {
            self.retained_scroll.clear_all();
        }
        if self.retained_scroll.active() && !was_active {
            // A native frame cache must not skip the first requested capture
            // merely because the ordinary page pixels are unchanged.
            self.host_state.borrow_mut().invalidate_paint_cache();
        }
    }

    /// Updates transient capture interest without changing persistent capture
    /// configuration. Re-enabling interest forces exactly one fresh paint;
    /// disabling it only discards an unpublished capture.
    pub(crate) fn set_retained_scroll_capture_interest(&mut self, interested: bool) {
        if self.retained_scroll.interested == interested {
            return;
        }
        let was_active = self.retained_scroll.active();
        self.retained_scroll.interested = interested;
        let active = self.retained_scroll.active();
        if active == was_active {
            return;
        }
        self.retained_scroll.clear_frame();
        if active {
            self.host_state.borrow_mut().invalidate_paint_cache();
        }
    }

    /// Takes one owned capture only if its source state still matches this owner.
    pub(crate) fn take_retained_scroll_capture(&mut self) -> Option<CapturedScrollScene> {
        let scene = self.retained_scroll.last.take()?;
        let state = self.host_state.borrow();
        (scene.document_id == state.document.identity()
            && scene.generations == generations(&state)
            && scene.viewport == state.viewport
            && scene.logical_scroll
                == (
                    f64::from(state.window_scroll.0),
                    f64::from(state.window_scroll.1),
                ))
            .then_some(scene)
    }
}

#[cfg(test)]
mod tests;
