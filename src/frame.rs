//! Continuous browser-frame rendering for native and embedded frontends.

use crate::cdp::{CdpSession, JsonRpcError};
use crate::http::Url;
use crate::layout::Rect;
use crate::paint::{Color, PaintError, render_document_snapshot_with_url};

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
    use serde_json::json;

    use super::*;

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
