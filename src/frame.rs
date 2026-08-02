//! Continuous browser-frame rendering for native and embedded frontends.

use std::time::{Duration, Instant};

use crate::cdp::{CdpSession, JsonRpcError};
use crate::http::Url;
use crate::layout::Rect;
use crate::paint::{Color, PaintError, render_document_snapshot_with_url};

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
    interval: Duration,
    next_deadline: Instant,
    redraw_pending: bool,
}

impl PlatformFrameScheduler {
    /// Creates a scheduler whose first rendering opportunity is immediately due.
    ///
    /// A zero interval is clamped to one nanosecond so every presented frame has
    /// a future deadline.
    pub fn new(origin: Instant, interval: Duration) -> Self {
        Self {
            origin,
            interval: interval.max(Duration::from_nanos(1)),
            next_deadline: origin,
            redraw_pending: false,
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

    /// Begins delivery of a frame and returns its animation timestamp.
    ///
    /// The next deadline is based on the actual delivery time. This avoids
    /// catch-up bursts after the window was blocked, occluded, or suspended.
    pub fn begin_frame(&mut self, now: Instant) -> u64 {
        self.redraw_pending = false;
        self.next_deadline = now + self.interval;
        now.saturating_duration_since(self.origin)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }
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
/// `elapsed_ms` is an absolute monotonic timestamp, matching the timestamp
/// supplied to `requestAnimationFrame` callbacks.
pub fn render_browser_frame(
    session: &mut CdpSession,
    width: u32,
    height: u32,
    elapsed_ms: u64,
) -> Result<BrowserFrame, FrameError> {
    session.set_viewport(width, height);
    session
        .drive_event_loop(elapsed_ms)
        .map_err(FrameError::EventLoop)?;

    let base_url = session.current_url().parse::<Url>().ok();
    let mut canvas = render_document_snapshot_with_url(
        &session.document(),
        Rect {
            x: 0.0,
            y: 0.0,
            width: width as f32,
            height: height as f32,
        },
        base_url.as_ref(),
        session.window_scroll_offset(),
    )
    .map_err(FrameError::Paint)?;
    canvas.composite_over(Color::rgb(255, 255, 255));

    Ok(BrowserFrame {
        width: canvas.width(),
        height: canvas.height(),
        pixels: canvas.into_pixels(),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

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
        let second = render_browser_frame(&mut session, 2, 2, 32).unwrap();

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
