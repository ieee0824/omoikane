//! Owner-local capture of retained pixels using the current document inputs.
use super::*;
use crate::paint::{
    RetainedOverscanFrame, RetainedOverscanOutcome, RetainedPaintError, VerticalOverscanPlan,
};

/// Pixels and explicit geometry only; this value may outlive its source runtime.
pub(crate) struct CapturedScrollScene {
    pub(crate) frame: RetainedOverscanFrame,
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
    reason: Option<RetainedPaintError>,
    clip_diagnostic: Option<crate::paint::CrossSpaceClipDiagnostic>,
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
}

fn generations(state: &HostState) -> RenderGenerations {
    RenderGenerations {
        style: state.style_generation,
        layout: state.layout_generation,
        paint: state.paint_generation,
    }
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
        self.clip_diagnostic = None;
    }

    pub(super) fn prepare(&mut self, state: &mut HostState) -> Option<CaptureRequest> {
        if !self.active() {
            return None;
        }
        let plan = match plan(state.viewport, state.window_scroll) {
            Ok(plan) => plan,
            Err(reason) => {
                self.reason = Some(reason);
                return None;
            }
        };
        let extent = state.window_scroll_extent();
        Some(CaptureRequest {
            layout: state.layout_root.as_ref()?.clone(),
            viewport: state.viewport,
            plan,
            logical_scroll: (
                f64::from(state.window_scroll.0),
                f64::from(state.window_scroll.1),
            ),
            extent: (f64::from(extent.0), f64::from(extent.1)),
            document_id: state.document.identity(),
            generations: generations(state),
        })
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
        if let Some(canvas) =
            self.capture(request, resolver, web_fonts, visited, snapshots, shadows)
        {
            // These counters describe the ordinary raster pass, which reuse skips.
            return (canvas, crate::paint::PaintRasterSummary::default());
        }
        nested_rendering::paint_layout_with_snapshot_attribution(
            diagnostics_enabled,
            layout,
            resolver,
            viewport,
            web_fonts,
            visited.iter().copied(),
            snapshots,
            Some(shadows),
        )
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
            self.trace_result(None, false);
            return None;
        };
        let started = self.trace.then(Instant::now);
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
        self.trace_result(started, current_canvas.is_some());
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

    fn trace_result(&self, started: Option<Instant>, canonical_available: bool) {
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
                    "reason": self.reason.map(|reason| format!("{reason:?}")),
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
        self.retained_scroll.trace =
            enabled && std::env::var_os("OMOIKANE_TRACE_RETAINED_SCROLL").is_some();
        self.retained_scroll.clear_frame();
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
