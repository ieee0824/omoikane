//! Browser frame advancement, painting and native rendering scheduling.

use std::time::{Duration, Instant};

use crate::cdp::{CdpSession, JsonRpcError, NextRendering, PaintStateKey, RenderDemand};
use crate::paint::{Canvas, Color, PaintError, RetainedOverscanFrame, RetainedPaintError};

/// Coordinates a platform window's redraw requests with browser rendering
/// opportunities.
///
/// Window event loops may enter their idle callback repeatedly while input or
/// other OS events are being delivered. This scheduler coalesces those wakeups
/// into one pending redraw, keeps the next frame deadline explicit, and skips
/// missed intervals instead of trying to render a burst of stale frames.
#[derive(Debug, Clone)]
pub struct PlatformFrameScheduler {
    origin: Instant,
    last_elapsed_ms: u64,
    interval: Duration,
    next_deadline: Instant,
    redraw_pending: bool,
    externally_invalidated: bool,
}

impl PlatformFrameScheduler {
    /// Creates a scheduler whose first rendering opportunity is immediately due.
    ///
    /// A zero interval is clamped to one nanosecond so every presented frame has
    /// a future deadline.
    pub fn new(origin: Instant, interval: Duration) -> Self {
        Self {
            origin,
            last_elapsed_ms: 0,
            interval: interval.max(Duration::from_nanos(1)),
            next_deadline: origin,
            redraw_pending: false,
            externally_invalidated: true,
        }
    }

    /// Returns the next instant at which the platform event loop should wake.
    pub fn deadline(&self) -> Instant {
        self.next_deadline
    }

    /// Returns whether a platform redraw has already been requested and has not
    /// yet delivered a rendering opportunity.
    pub fn redraw_pending(&self) -> bool {
        self.redraw_pending
    }

    /// Pulls the next deadline forward for an external invalidation such as
    /// input or resize. An already pending platform redraw remains coalesced.
    pub fn request_rendering_opportunity(&mut self, now: Instant) {
        self.externally_invalidated = true;
        if now < self.next_deadline {
            self.next_deadline = now;
        }
    }

    /// Marks one platform redraw as pending when the current deadline is due.
    ///
    /// The caller should invoke `Window::request_redraw()` only when this method
    /// returns `true`.
    pub fn queue_redraw_if_due(&mut self, now: Instant) -> bool {
        if self.redraw_pending || now < self.next_deadline {
            return false;
        }
        self.redraw_pending = true;
        true
    }

    /// Begins delivery and returns milliseconds since the previous opportunity.
    /// The first delivery measures from the scheduler origin. Callback timestamps
    /// are accumulated by the page event loop, rather than by the host caller.
    ///
    /// The next deadline is based on the actual delivery time. This avoids
    /// catch-up bursts after the window was blocked, occluded, or suspended.
    pub fn begin_frame(&mut self, now: Instant) -> u64 {
        self.redraw_pending = false;
        self.externally_invalidated = false;
        self.next_deadline = now + self.interval;
        self.advance_time(now)
    }

    /// Consumes elapsed host time without delivering an animation frame.
    /// Native hosts use this before input, so timers created by input callbacks
    /// start at the current page time even after a long idle interval.
    pub fn advance_time(&mut self, now: Instant) -> u64 {
        let elapsed = now
            .saturating_duration_since(self.origin)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let delta = elapsed.saturating_sub(self.last_elapsed_ms);
        self.last_elapsed_ms = self.last_elapsed_ms.max(elapsed);
        delta
    }

    /// Selects the next host action without changing state or reading a clock.
    /// Timer delays are anchored to the last consumed page time, so repeated
    /// OS wakeups cannot move the same timer's deadline into the future. Wall
    /// delays are measured from `now`, so fresh remaining delays do not move
    /// cookie expiry deadlines into the past.
    pub fn plan(&self, now: Instant, demand: RenderDemand) -> PlatformFramePlan {
        if self.redraw_pending {
            return PlatformFramePlan::Wait;
        }
        if self.externally_invalidated || demand.needs_paint {
            return PlatformFramePlan::Redraw;
        }
        let deadline = match demand.next {
            NextRendering::Idle => return PlatformFramePlan::Wait,
            NextRendering::EveryFrame => self.next_deadline,
            NextRendering::After(delay) => {
                self.origin + Duration::from_millis(self.last_elapsed_ms) + delay
            }
            NextRendering::AfterWall(delay) => now + delay,
            NextRendering::AfterBoth {
                page_delay,
                wall_delay,
            } => (self.origin + Duration::from_millis(self.last_elapsed_ms) + page_delay)
                .min(now + wall_delay),
        };
        if now >= deadline {
            PlatformFramePlan::Redraw
        } else {
            PlatformFramePlan::WaitUntil(deadline)
        }
    }

    /// Coalesces a redraw selected by [`Self::plan`].
    pub fn queue_redraw(&mut self) -> bool {
        if self.redraw_pending {
            return false;
        }
        self.redraw_pending = true;
        true
    }
}

/// A platform-independent decision for a native window event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformFramePlan {
    /// Sleep until an external event arrives.
    Wait,
    /// Sleep until the explicit page or frame deadline.
    WaitUntil(Instant),
    /// Request one rendering opportunity immediately.
    Redraw,
}

/// An opaque, row-major RGBA browser frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl BrowserFrame {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Returns `width * height * 4` bytes in RGBA order.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    fn from_canvas(mut canvas: Canvas) -> Self {
        canvas.composite_over(Color::rgb(255, 255, 255));
        Self {
            width: canvas.width(),
            height: canvas.height(),
            pixels: canvas.into_pixels(),
        }
    }
}

/// Failure to validate or compose an owned retained scroll scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedScrollComposeError {
    /// The capture and adapter disagree about logical-to-raster scale.
    InvalidRasterScale,
    /// Owned raster geometry is incomplete or internally inconsistent.
    InvalidScene,
    /// A requested logical scroll coordinate is NaN or infinite.
    NonFiniteScroll,
    /// A requested logical scroll is outside the retained raster coverage.
    OutsideCoverage,
    /// A requested logical scroll is outside the document scroll extent.
    OutsideExtent,
}

impl std::fmt::Display for RetainedScrollComposeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRasterScale => "retained raster scale does not match its capture",
            Self::InvalidScene => "retained raster scene is invalid",
            Self::NonFiniteScroll => "retained scroll offset is not finite",
            Self::OutsideCoverage => "retained scroll offset is outside raster coverage",
            Self::OutsideExtent => "retained scroll offset is outside the document extent",
        })
    }
}

impl std::error::Error for RetainedScrollComposeError {}

/// An opaque, owned retained scene for pure scroll recomposition.
///
/// Hosts must call [`Self::matches_owner`] immediately before accepting an
/// asynchronously composed frame. A false result requires a canonical repaint.
/// The numeric tags are snapshot checks; live DOM/script revocation remains a
/// separate native-owner signal and must be connected before async acceptance.
pub struct RetainedScrollScene {
    inner: RetainedOverscanFrame,
    raster_scale: f64,
    document_generation: u64,
    paint_revision: u64,
    logical_scroll: (f64, f64),
    extent: (f64, f64),
}

impl RetainedScrollScene {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        inner: RetainedOverscanFrame,
        raster_scale: f64,
        document_generation: u64,
        paint_revision: u64,
        logical_scroll: (f64, f64),
        extent: (f64, f64),
    ) -> Result<Self, RetainedScrollComposeError> {
        inner
            .validate_compositor(raster_scale)
            .map_err(map_retained_compose_error)?;
        if !logical_scroll.0.is_finite()
            || !logical_scroll.1.is_finite()
            || !extent.0.is_finite()
            || !extent.1.is_finite()
            || extent.0 < 0.0
            || extent.1 < 0.0
        {
            return Err(RetainedScrollComposeError::InvalidScene);
        }
        if !scroll_in_extent(logical_scroll, extent) {
            return Err(RetainedScrollComposeError::OutsideExtent);
        }
        let coverage = inner.logical_coverage();
        if !scroll_in_coverage(logical_scroll, coverage) {
            return Err(RetainedScrollComposeError::OutsideCoverage);
        }
        Ok(Self {
            inner,
            raster_scale,
            document_generation,
            paint_revision,
            logical_scroll,
            extent,
        })
    }

    /// Returns the inclusive logical coverage as `(minimum, maximum)`.
    pub fn coverage(&self) -> ((f64, f64), (f64, f64)) {
        self.inner.logical_coverage()
    }

    /// Returns the logical scroll at which this scene was captured.
    pub fn current_scroll(&self) -> (f64, f64) {
        self.logical_scroll
    }

    /// Returns the maximum logical scroll; the minimum is `(0, 0)`.
    pub fn extent(&self) -> (f64, f64) {
        self.extent
    }

    /// Returns the immutable logical-to-raster scale for this scene.
    pub fn raster_scale(&self) -> f64 {
        self.raster_scale
    }

    /// Returns the document generation that owned the captured pixels.
    pub fn document_generation(&self) -> u64 {
        self.document_generation
    }

    /// Returns the paint revision that owned the captured pixels.
    pub fn paint_revision(&self) -> u64 {
        self.paint_revision
    }

    /// Compares numeric owner snapshots before async presentation.
    ///
    /// A mismatch requires a canonical repaint. This check does not replace a
    /// live revoke signal for mutations that occur while owner script is busy.
    pub fn matches_owner(&self, document_generation: u64, paint_revision: u64) -> bool {
        self.document_generation == document_generation && self.paint_revision == paint_revision
    }

    /// Composes retained pixels at an absolute logical CSS scroll offset.
    ///
    /// The operation reads only owned pixels. Any error requires the caller to
    /// keep its last valid frame and schedule a canonical owner-thread repaint.
    pub fn compose(&self, scroll: (f64, f64)) -> Result<BrowserFrame, RetainedScrollComposeError> {
        if !scroll.0.is_finite() || !scroll.1.is_finite() {
            return Err(RetainedScrollComposeError::NonFiniteScroll);
        }
        if !scroll_in_extent(scroll, self.extent) {
            return Err(RetainedScrollComposeError::OutsideExtent);
        }
        let canvas = self
            .inner
            .compose_logical(scroll, self.raster_scale)
            .map_err(map_retained_compose_error)?;
        Ok(BrowserFrame::from_canvas(canvas))
    }
}

fn scroll_in_extent(scroll: (f64, f64), extent: (f64, f64)) -> bool {
    scroll.0 >= 0.0 && scroll.1 >= 0.0 && scroll.0 <= extent.0 && scroll.1 <= extent.1
}

fn scroll_in_coverage(scroll: (f64, f64), coverage: ((f64, f64), (f64, f64))) -> bool {
    scroll.0 >= coverage.0.0
        && scroll.0 <= coverage.1.0
        && scroll.1 >= coverage.0.1
        && scroll.1 <= coverage.1.1
}

fn map_retained_compose_error(error: RetainedPaintError) -> RetainedScrollComposeError {
    match error {
        RetainedPaintError::InvalidOverscanRasterScale => {
            RetainedScrollComposeError::InvalidRasterScale
        }
        RetainedPaintError::InvalidOverscanScroll => RetainedScrollComposeError::NonFiniteScroll,
        RetainedPaintError::OutsideOverscanCoverage => RetainedScrollComposeError::OutsideCoverage,
        _ => RetainedScrollComposeError::InvalidScene,
    }
}

/// Failure while advancing or painting a browser frame.
#[derive(Debug)]
pub enum FrameError {
    EventLoop(JsonRpcError),
    Paint(PaintError),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EventLoop(error) => write!(f, "page event loop failed: {}", error.message),
            Self::Paint(error) => write!(f, "page paint failed: {error:?}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Drives rendering opportunities and snapshots the current DOM into raw RGBA.
///
/// `elapsed_ms` is the delta since the previous opportunity (or initialization).
/// The page accumulates this delta for timer deadlines and the absolute
/// timestamps supplied to `requestAnimationFrame` callbacks.
pub fn render_browser_frame(
    session: &mut CdpSession,
    width: u32,
    height: u32,
    elapsed_ms: u64,
) -> Result<BrowserFrame, FrameError> {
    advance_browser_frame(session, width, height, elapsed_ms)?;
    paint_browser_frame(session)
}

/// Advances jobs, timers and one animation frame without painting the page.
/// `elapsed_ms` is the delta since the previous page-clock advancement.
pub fn advance_browser_frame(
    session: &mut CdpSession,
    width: u32,
    height: u32,
    elapsed_ms: u64,
) -> Result<(), FrameError> {
    session.set_viewport(width, height);
    session
        .drive_event_loop(elapsed_ms)
        .map_err(FrameError::EventLoop)
}

/// Paints the current page without advancing its event loop or animation clock.
pub fn paint_browser_frame(session: &mut CdpSession) -> Result<BrowserFrame, FrameError> {
    let canvas = session
        .paint_current_document()
        .map_err(FrameError::Paint)?;
    Ok(BrowserFrame::from_canvas(canvas))
}

/// Owns the last painted page pixels and the native inputs that produced them.
/// Toolbar composition can borrow the cached frame without repainting the page.
#[derive(Debug, Default)]
pub struct BrowserFrameCache {
    frame: Option<BrowserFrame>,
    presented: Option<PaintStateKey>,
}

impl BrowserFrameCache {
    /// Returns the retained page frame, if it has been painted successfully.
    pub fn frame(&self) -> Option<&BrowserFrame> {
        self.frame.as_ref()
    }

    /// Queries page demand relative to this cache without changing either one.
    pub fn demand(&self, session: &CdpSession) -> RenderDemand {
        session.render_demand(self.presented.as_ref())
    }

    /// Advances page tasks and its animation clock without painting.
    ///
    /// Hosts that publish retained native state may use this boundary to sample
    /// a fresh policy epoch after author tasks and before any paint inputs are
    /// read. [`Self::update`] remains the combined convenience operation.
    pub fn advance(
        &mut self,
        session: &mut CdpSession,
        width: u32,
        height: u32,
        elapsed_ms: u64,
    ) -> Result<(), FrameError> {
        let started = std::env::var_os("OMOIKANE_TRACE_PAINT").map(|_| Instant::now());
        let advance_result = advance_browser_frame(session, width, height, elapsed_ms);
        if let Some(started) = started {
            eprintln!(
                "{}",
                crate::diagnostics::json_record(
                    "OMOIKANE_FRAME_EVENT_LOOP",
                    &serde_json::json!({
                        "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0,
                        "success": advance_result.is_ok(),
                    })
                )
            );
        }
        advance_result
    }

    /// Paints only when the current page differs from this cache.
    ///
    /// This method does not run page tasks or advance its clock. Callers that
    /// split task execution from painting must not expose the old cached frame
    /// between [`Self::advance`] and this operation.
    pub fn paint_if_needed(&mut self, session: &mut CdpSession) -> Result<bool, FrameError> {
        if !self.demand(session).needs_paint {
            return Ok(false);
        }
        self.frame = Some(paint_browser_frame(session)?);
        self.presented = Some(session.paint_state_key());
        Ok(true)
    }

    /// Advances the page and paints only if its native paint inputs changed.
    /// Returns whether new page pixels were produced. A false result lets hosts
    /// skip surface transfer unless native chrome or an OS exposure changed.
    pub fn update(
        &mut self,
        session: &mut CdpSession,
        width: u32,
        height: u32,
        elapsed_ms: u64,
    ) -> Result<bool, FrameError> {
        self.advance(session, width, height, elapsed_ms)?;
        self.paint_if_needed(session)
    }

    /// Paints current page state without running page tasks or advancing its clock.
    ///
    /// Native input can present a scroll offset before expensive networking or
    /// script tasks receive their next event-loop turn.
    pub fn paint_without_advance(&mut self, session: &mut CdpSession) -> Result<bool, FrameError> {
        self.paint_if_needed(session)
    }
}

#[cfg(test)]
mod demand_tests;

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::js::native_scroll::{
        Bounds, FallbackTicket, NativeScrollHandle, Offset, PublicationReconciliation,
        PublicationSequence, Scene, ScrollDecision, TargetKey,
    };
    use crate::paint::{Image, RetainedPaintSpace};
    use serde_json::json;

    use super::*;

    fn retained_test_frame(raster_scale: f64) -> RetainedOverscanFrame {
        let image = Image::new(1, 3, vec![10, 0, 0, 255, 20, 0, 0, 255, 30, 0, 0, 255]).unwrap();
        RetainedOverscanFrame::from_test_parts(
            1,
            1,
            raster_scale,
            ((0, 0), (0, 2)),
            (0, 0),
            vec![(RetainedPaintSpace::Document, vec![((0, 0), image)])],
        )
    }

    fn retained_test_scene(extent: (f64, f64)) -> RetainedScrollScene {
        RetainedScrollScene::new(retained_test_frame(1.0), 1.0, 7, 11, (0.0, 0.5), extent).unwrap()
    }

    fn retained_rebase_test_scene(canonical_y: i32, rows: Vec<[u8; 4]>) -> RetainedScrollScene {
        let height = u32::try_from(rows.len()).unwrap();
        let last_y = canonical_y + i32::try_from(height).unwrap() - 1;
        let pixels = rows.into_iter().flatten().collect();
        let inner = RetainedOverscanFrame::from_test_parts(
            1,
            1,
            1.0,
            ((0, canonical_y), (0, last_y)),
            (0, canonical_y),
            vec![(
                RetainedPaintSpace::Document,
                vec![((0, canonical_y), Image::new(1, height, pixels).unwrap())],
            )],
        );
        RetainedScrollScene::new(
            inner,
            1.0,
            7,
            11,
            (0.0, f64::from(canonical_y)),
            (0.0, 100.0),
        )
        .unwrap()
    }

    fn native_rebase_test_scene(
        key: TargetKey,
        publication: u64,
        policy_epoch: u64,
        offset_y: f64,
        coverage_y: (f64, f64),
    ) -> Scene {
        Scene {
            key,
            publication: PublicationSequence::new(publication),
            policy_epoch,
            acceptance_epoch: 0,
            acknowledged: 0,
            completed_fallback: FallbackTicket::default(),
            offset: Offset {
                x: 0.0,
                y: offset_y,
            },
            extent: Bounds {
                min: Offset { x: 0.0, y: 0.0 },
                max: Offset { x: 0.0, y: 100.0 },
            },
            coverage: Bounds {
                min: Offset {
                    x: 0.0,
                    y: coverage_y.0,
                },
                max: Offset {
                    x: 0.0,
                    y: coverage_y.1,
                },
            },
            eligible: true,
        }
    }

    #[test]
    fn atomic_scene_commit_recomposes_owned_rgba_at_the_replayed_live_offset() {
        let key = TargetKey {
            document: 7,
            owner_generation: 1,
            viewport_epoch: 1,
            scroll_node: 7,
        };
        let handle = NativeScrollHandle::new(key);
        let initial_gate = handle.publication_gate();
        let publication1 = PublicationSequence::new(1);
        assert_eq!(
            handle.commit_scene_publication(
                key,
                initial_gate.policy_epoch,
                initial_gate.acceptance_epoch,
                publication1,
                0,
                native_rebase_test_scene(key, 1, initial_gate.policy_epoch, 0.0, (0.0, 10.0)),
                true,
            ),
            PublicationReconciliation::Current
        );

        let accepted = match handle
            .scroll_for_publication(
                key,
                initial_gate.policy_epoch,
                initial_gate.acceptance_epoch,
                publication1,
                Offset { x: 0.0, y: 10.0 },
            )
            .expect("P1 remains the exact current publication")
        {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected decision {decision:?}"),
        };
        assert_eq!(accepted.input().visual_offset, Offset { x: 0.0, y: 10.0 });
        let red = retained_rebase_test_scene(0, vec![[255, 0, 0, 255]; 11]);
        assert_eq!(
            red.compose((0.0, accepted.visual_offset().y))
                .unwrap()
                .pixels(),
            [255, 0, 0, 255]
        );

        let publication2 = PublicationSequence::new(2);
        assert_eq!(
            handle.commit_scene_publication(
                key,
                initial_gate.policy_epoch,
                initial_gate.acceptance_epoch,
                publication2,
                0,
                native_rebase_test_scene(key, 2, initial_gate.policy_epoch, 40.0, (40.0, 50.0)),
                true,
            ),
            PublicationReconciliation::Current
        );
        let rebound = handle
            .rebind_presentation_to_publication(accepted, publication2)
            .expect("the unacknowledged P1 default replays over P2");
        assert_eq!(rebound.visual_offset(), Offset { x: 0.0, y: 50.0 });
        assert_eq!(rebound.input(), accepted.input());
        assert!(handle.validate_for_presentation(rebound));

        let mut blue_rows = vec![[0, 0, 255, 255]; 11];
        blue_rows[0] = [0, 255, 0, 255];
        let blue = retained_rebase_test_scene(40, blue_rows);
        assert_eq!(
            blue.compose((0.0, 40.0)).unwrap().pixels(),
            [0, 255, 0, 255]
        );
        assert_eq!(
            blue.compose((0.0, rebound.visual_offset().y))
                .unwrap()
                .pixels(),
            [0, 0, 255, 255]
        );

        let extent = Bounds {
            min: Offset { x: 0.0, y: 0.0 },
            max: Offset { x: 0.0, y: 100.0 },
        };
        let materialized = handle
            .materialize_pending(key, Offset { x: 0.0, y: 40.0 }, extent)
            .unwrap();
        assert_eq!(materialized.offset, Offset { x: 0.0, y: 50.0 });
        assert_eq!(materialized.default_applied_sequence, 1);
        assert_eq!(handle.owner_sequences(key), Ok((1, 0)));

        let notification = handle
            .claim_notification(key, materialized.offset, extent)
            .unwrap()
            .expect("the original P1 notification remains queued");
        assert_eq!(notification.input, accepted.input());
        assert_eq!(notification.input.visual_offset, Offset { x: 0.0, y: 10.0 });
        assert_eq!(notification.offset, Offset { x: 0.0, y: 50.0 });
        assert_eq!(
            handle.claim_notification(key, notification.offset, extent),
            Ok(None)
        );
        assert_eq!(handle.owner_sequences(key), Ok((1, 1)));
        assert_eq!(handle.notification_backlog(key), Ok(0));
    }

    #[test]
    fn platform_scheduler_coalesces_redraws_and_advances_after_presentation() {
        let origin = Instant::now();
        let mut scheduler = PlatformFrameScheduler::new(origin, Duration::from_millis(16));

        assert_eq!(scheduler.deadline(), origin);
        assert!(scheduler.queue_redraw_if_due(origin));
        assert!(!scheduler.queue_redraw_if_due(origin + Duration::from_millis(1)));

        assert_eq!(scheduler.begin_frame(origin + Duration::from_millis(5)), 5);
        assert_eq!(scheduler.deadline(), origin + Duration::from_millis(21));
        assert!(!scheduler.queue_redraw_if_due(origin + Duration::from_millis(20)));
        assert!(scheduler.queue_redraw_if_due(origin + Duration::from_millis(21)));
    }

    #[test]
    fn retained_scene_exposes_numeric_protocol_state_and_owner_gate() {
        let scene = retained_test_scene((0.0, 10.0));
        assert_eq!(scene.coverage(), ((0.0, 0.0), (0.0, 2.0)));
        assert_eq!(scene.current_scroll(), (0.0, 0.5));
        assert_eq!(scene.extent(), (0.0, 10.0));
        assert_eq!(scene.raster_scale(), 1.0);
        assert_eq!(scene.document_generation(), 7);
        assert_eq!(scene.paint_revision(), 11);
        assert!(scene.matches_owner(7, 11));
        assert!(!scene.matches_owner(8, 11));
        assert!(!scene.matches_owner(7, 12));
    }

    #[test]
    fn retained_scene_composes_exact_and_fractional_opaque_rgba() {
        let scene = retained_test_scene((0.0, 10.0));
        assert_eq!(scene.compose((0.0, 1.0)).unwrap().pixels(), [20, 0, 0, 255]);
        assert_eq!(scene.compose((0.0, 1.5)).unwrap().pixels(), [25, 0, 0, 255]);
    }

    #[test]
    fn retained_scene_rejects_invalid_scale_and_scroll_for_canonical_repaint() {
        let mismatch = RetainedScrollScene::new(
            retained_test_frame(1.0),
            2.0,
            7,
            11,
            (0.0, 0.5),
            (0.0, 10.0),
        );
        assert!(matches!(
            mismatch,
            Err(RetainedScrollComposeError::InvalidRasterScale)
        ));
        let nonfinite_scale = RetainedScrollScene::new(
            retained_test_frame(1.0),
            f64::NAN,
            7,
            11,
            (0.0, 0.5),
            (0.0, 10.0),
        );
        assert!(matches!(
            nonfinite_scale,
            Err(RetainedScrollComposeError::InvalidRasterScale)
        ));

        let scene = retained_test_scene((0.0, 10.0));
        assert_eq!(
            scene.compose((0.0, f64::INFINITY)),
            Err(RetainedScrollComposeError::NonFiniteScroll)
        );
        assert_eq!(
            scene.compose((0.0, 2.5)),
            Err(RetainedScrollComposeError::OutsideCoverage)
        );
        let bounded = retained_test_scene((0.0, 1.0));
        assert_eq!(
            bounded.compose((0.0, 1.5)),
            Err(RetainedScrollComposeError::OutsideExtent)
        );
    }

    #[test]
    fn platform_scheduler_wakes_early_for_invalidation_and_skips_missed_frames() {
        let origin = Instant::now();
        let mut scheduler = PlatformFrameScheduler::new(origin, Duration::from_millis(16));
        assert!(scheduler.queue_redraw_if_due(origin));
        scheduler.begin_frame(origin);

        let invalidated_at = origin + Duration::from_millis(4);
        scheduler.request_rendering_opportunity(invalidated_at);
        assert_eq!(scheduler.deadline(), invalidated_at);
        assert!(scheduler.queue_redraw_if_due(invalidated_at));

        let resumed_at = origin + Duration::from_millis(100);
        assert_eq!(scheduler.begin_frame(resumed_at), 100);
        assert_eq!(scheduler.deadline(), origin + Duration::from_millis(116));
        assert!(!scheduler.queue_redraw_if_due(resumed_at));
    }

    fn navigate(session: &mut CdpSession, html: &str) {
        let encoded = html
            .bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect::<String>();
        session
            .dispatch(
                "Page.navigate",
                json!({ "url": format!("data:text/html,{encoded}") }),
            )
            .unwrap();
    }

    fn evaluate(session: &mut CdpSession, expression: &str) -> serde_json::Value {
        session
            .dispatch("Runtime.evaluate", json!({ "expression": expression }))
            .unwrap()["result"]["value"]
            .clone()
    }

    #[test]
    fn platform_frames_advance_page_time_by_delivery_intervals() {
        let mut session = CdpSession::new().unwrap();
        navigate(
            &mut session,
            "<script>window.fired=false;window.stamps=[];setTimeout(()=>fired=true,100);function frame(t){stamps.push(t);requestAnimationFrame(frame)}requestAnimationFrame(frame)</script>",
        );
        let origin = Instant::now();
        let mut scheduler = PlatformFrameScheduler::new(origin, Duration::from_millis(16));
        for index in 1..=7 {
            let delta = scheduler.begin_frame(origin + Duration::from_millis(index * 16));
            render_browser_frame(&mut session, 4, 4, delta).unwrap();
            assert_eq!(evaluate(&mut session, "fired"), json!(index == 7));
        }
        assert_eq!(
            evaluate(&mut session, "stamps"),
            json!([16, 32, 48, 64, 80, 96, 112])
        );
    }

    #[test]
    fn returns_opaque_raw_rgba_at_requested_size() {
        let mut session = CdpSession::new().unwrap();
        navigate(
            &mut session,
            "<style>body{margin:0}div{width:3px;height:2px;background:#123456}</style><div></div>",
        );

        let frame = render_browser_frame(&mut session, 3, 2, 0).unwrap();

        assert_eq!((frame.width(), frame.height()), (3, 2));
        assert_eq!(frame.pixels().len(), 3 * 2 * 4);
        assert_eq!(&frame.pixels()[..4], &[0x12, 0x34, 0x56, 0xff]);
    }

    #[test]
    fn consecutive_frames_advance_raf_without_rerunning_scripts() {
        let mut session = CdpSession::new().unwrap();
        navigate(
            &mut session,
            "<style>body{margin:0}div{width:2px;height:2px}</style><div id='box'></div>\
             <script>globalThis.runs=(globalThis.runs||0)+1;\
             requestAnimationFrame(()=>{document.getElementById('box').style.background='rgb(255, 0, 0)'})</script>",
        );

        let first = render_browser_frame(&mut session, 2, 2, 16).unwrap();
        let second = render_browser_frame(&mut session, 2, 2, 16).unwrap();

        assert_eq!(evaluate(&mut session, "globalThis.runs"), json!(1));
        assert_eq!(&first.pixels()[..4], &[255, 0, 0, 255]);
        assert_eq!(first.pixels(), second.pixels());
    }

    #[test]
    fn resize_updates_window_metrics_and_frame_dimensions() {
        let mut session = CdpSession::new().unwrap();
        navigate(&mut session, "<body></body>");

        render_browser_frame(&mut session, 320, 200, 0).unwrap();
        assert_eq!(
            evaluate(&mut session, "innerWidth + ',' + innerHeight"),
            json!("320,200")
        );

        let resized = render_browser_frame(&mut session, 640, 360, 16).unwrap();
        assert_eq!((resized.width(), resized.height()), (640, 360));
        assert_eq!(
            evaluate(&mut session, "innerWidth + ',' + innerHeight"),
            json!("640,360")
        );
    }
}
