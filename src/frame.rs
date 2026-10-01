//! Browser frame advancement, painting and native rendering scheduling.

use std::time::{Duration, Instant};

use crate::cdp::{CdpSession, JsonRpcError, NextRendering, PaintStateKey, RenderDemand};
use crate::paint::{Color, PaintError};

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
    /// OS wakeups cannot move the same timer's deadline into the future.
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
    let mut canvas = session
        .paint_current_document()
        .map_err(FrameError::Paint)?;
    canvas.composite_over(Color::rgb(255, 255, 255));

    Ok(BrowserFrame {
        width: canvas.width(),
        height: canvas.height(),
        pixels: canvas.into_pixels(),
    })
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
        advance_browser_frame(session, width, height, elapsed_ms)?;
        if !self.demand(session).needs_paint {
            return Ok(false);
        }
        self.frame = Some(paint_browser_frame(session)?);
        self.presented = Some(session.paint_state_key());
        Ok(true)
    }
}

#[cfg(test)]
mod demand_tests;

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::json;

    use super::*;

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
