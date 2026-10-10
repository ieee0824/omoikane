use crate::cdp::{CdpSession, JsonRpcError};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::html::{TreeBuilder, decode_html_response};
use crate::http::url::UrlParseError;
use crate::http::url::resolve_url;
use crate::http::{Client, HttpParseError};
use crate::layout::Rect;
use crate::layout::frameset::parse_frameset_track_sizes;
use crate::paint::{
    Canvas, Color, Image, PaintError, RenderTimings, clear_render_timings, record_render_timings,
    render_document_with_url,
};
use std::time::Instant;

const MAX_FRAMESET_DEPTH: usize = 4;
/// Maximum width or height accepted by the screenshot path.
pub(crate) const MAX_SCREENSHOT_DIMENSION: u32 = 16_384;
/// Maximum pixel count accepted by the screenshot path.
pub(crate) const MAX_SCREENSHOT_PIXELS: u64 = 67_108_864;

/// Failures while preparing or rendering a screenshot.
#[derive(Debug)]
pub(crate) enum ScreenshotError {
    /// The page could not settle before painting.
    Settle(JsonRpcError),
    /// Painting the active document failed.
    Paint(PaintError),
    /// A frameset child has no source URL.
    MissingFrameSource,
    /// A frame source URL could not be resolved.
    Url(UrlParseError),
    /// Fetching a frame document failed.
    Http(HttpParseError),
    /// A rendered frame could not be materialized as an image.
    FrameImage(PaintError),
    /// Painting a frame document failed.
    FramePaint(PaintError),
    /// The requested viewport cannot be represented as a nonempty canvas.
    InvalidViewport,
    /// The canvas or its PNG representation exceeds the screenshot budget.
    CanvasBudgetExceeded,
}

impl std::fmt::Display for ScreenshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Settle(error) => write!(f, "{}", error.message),
            Self::Paint(error) => write!(f, "{error:?}"),
            Self::MissingFrameSource => write!(f, "frame src is missing"),
            Self::Url(error) => write!(f, "{error}"),
            Self::Http(error) => write!(f, "{error}"),
            Self::FrameImage(error) => write!(f, "failed to materialize frame image: {error:?}"),
            Self::FramePaint(error) => write!(f, "failed to render frame document: {error:?}"),
            Self::InvalidViewport => write!(f, "screenshot viewport must be finite and positive"),
            Self::CanvasBudgetExceeded => write!(f, "screenshot canvas exceeds resource budget"),
        }
    }
}

impl std::error::Error for ScreenshotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Settle(error) => Some(error),
            Self::Url(error) => Some(error),
            Self::Http(error) => Some(error),
            _ => None,
        }
    }
}

impl From<UrlParseError> for ScreenshotError {
    fn from(error: UrlParseError) -> Self {
        Self::Url(error)
    }
}

impl From<HttpParseError> for ScreenshotError {
    fn from(error: HttpParseError) -> Self {
        Self::Http(error)
    }
}

pub(crate) fn capture_session_screenshot_png(
    session: &mut CdpSession,
    viewport: Rect,
) -> Result<Vec<u8>, ScreenshotError> {
    validate_screenshot_viewport(viewport)?;
    clear_render_timings();
    session.set_viewport(viewport.width as u32, viewport.height as u32);
    let settle_timings = session
        .settle_for_render()
        .map_err(ScreenshotError::Settle)?;
    record_render_timings(&RenderTimings {
        timers: settle_timings.timers,
        animation_frames: settle_timings.animation_frames,
        ..RenderTimings::default()
    });
    let document = session.document();
    let base_url = session.current_url().parse::<crate::http::Url>().ok();

    match render_frameset_screenshot_png(
        &document,
        base_url.as_ref(),
        viewport,
        session.http_client_mut(),
    ) {
        Ok(Some(png)) => Ok(png),
        fallback => {
            if fallback.is_err() {
                session.report_paint_failure("SCREENSHOT_FRAMESET_FALLBACK");
            }
            let (render_document, render_base_url) =
                resolve_frameset_render_document(&document, base_url.as_ref())
                    .unwrap_or((document.clone(), base_url.clone()));
            let canvas = if render_document.identity() == document.identity() {
                session.paint_current_document()
            } else {
                render_document_with_url(&render_document, viewport, render_base_url.as_ref())
            }
            .map_err(|error| {
                session.report_paint_failure("SCREENSHOT_PAINT_FAILED");
                ScreenshotError::Paint(error)
            })?;
            encode_screenshot_canvas(canvas)
        }
    }
}

fn render_frameset_screenshot_png(
    document: &NodeHandle,
    base_url: Option<&crate::http::Url>,
    viewport: Rect,
    client: &mut Client,
) -> Result<Option<Vec<u8>>, ScreenshotError> {
    let Some(canvas) = render_frameset_canvas(document, base_url, viewport, 0, client)? else {
        return Ok(None);
    };
    Ok(Some(encode_screenshot_canvas(canvas)?))
}

fn encode_screenshot_canvas(mut canvas: Canvas) -> Result<Vec<u8>, ScreenshotError> {
    let expected_rgba_bytes = validate_canvas_dimensions(canvas.width(), canvas.height())?;
    if canvas.pixels().len() != expected_rgba_bytes {
        return Err(ScreenshotError::Paint(PaintError::InvalidImageBuffer));
    }
    canvas.composite_over(Color::rgb(255, 255, 255));
    let encode_start = Instant::now();
    let png = canvas.encode_png();
    record_render_timings(&RenderTimings {
        png_encode: encode_start.elapsed(),
        ..RenderTimings::default()
    });
    Ok(png)
}

fn validate_screenshot_viewport(viewport: Rect) -> Result<(), ScreenshotError> {
    if !viewport.width.is_finite()
        || !viewport.height.is_finite()
        || viewport.width <= 0.0
        || viewport.height <= 0.0
    {
        return Err(ScreenshotError::InvalidViewport);
    }
    if viewport.width > MAX_SCREENSHOT_DIMENSION as f32
        || viewport.height > MAX_SCREENSHOT_DIMENSION as f32
    {
        return Err(ScreenshotError::CanvasBudgetExceeded);
    }
    validate_canvas_dimensions(viewport.width.ceil() as u32, viewport.height.ceil() as u32)?;
    Ok(())
}

fn validate_canvas_dimensions(width: u32, height: u32) -> Result<usize, ScreenshotError> {
    if width == 0 || height == 0 {
        return Err(ScreenshotError::InvalidViewport);
    }
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ScreenshotError::CanvasBudgetExceeded)?;
    if width > MAX_SCREENSHOT_DIMENSION
        || height > MAX_SCREENSHOT_DIMENSION
        || pixels > MAX_SCREENSHOT_PIXELS
    {
        return Err(ScreenshotError::CanvasBudgetExceeded);
    }
    let rgba_bytes = usize::try_from(pixels)
        .ok()
        .and_then(|count| count.checked_mul(4))
        .ok_or(ScreenshotError::CanvasBudgetExceeded)?;
    let raw_bytes = rgba_bytes
        .checked_add(height as usize)
        .ok_or(ScreenshotError::CanvasBudgetExceeded)?;
    let blocks = raw_bytes
        .checked_add(u16::MAX as usize - 1)
        .map(|bytes| bytes / u16::MAX as usize)
        .ok_or(ScreenshotError::CanvasBudgetExceeded)?;
    let compressed_bytes = blocks
        .checked_mul(5)
        .and_then(|overhead| raw_bytes.checked_add(overhead))
        .and_then(|bytes| bytes.checked_add(6))
        .ok_or(ScreenshotError::CanvasBudgetExceeded)?;
    u32::try_from(compressed_bytes).map_err(|_| ScreenshotError::CanvasBudgetExceeded)?;
    // PNG signature, IHDR chunk, IDAT chunk header/CRC, and IEND chunk.
    compressed_bytes
        .checked_add(8 + 25 + 12 + 12)
        .ok_or(ScreenshotError::CanvasBudgetExceeded)?;
    Ok(rgba_bytes)
}

fn render_frameset_canvas(
    node: &NodeHandle,
    base_url: Option<&crate::http::Url>,
    viewport: Rect,
    depth: usize,
    client: &mut Client,
) -> Result<Option<Canvas>, ScreenshotError> {
    if depth > MAX_FRAMESET_DEPTH {
        return Ok(None);
    }
    let Some(frameset) = node.query_selector("frameset") else {
        return Ok(None);
    };

    let layout_children = collect_frameset_layout_children(&frameset);
    if layout_children.is_empty() {
        return Ok(None);
    }

    let total_width = viewport.width.max(1.0).round() as u32;
    let total_height = viewport.height.max(1.0).round() as u32;
    validate_canvas_dimensions(total_width, total_height)?;
    let attrs = frameset.attributes().unwrap_or_default();
    let cols_attr = attrs.get("cols").cloned();
    let rows_attr = attrs.get("rows").cloned();
    let use_rows = rows_attr
        .as_deref()
        .map(|rows| !rows.trim().is_empty())
        .unwrap_or(false)
        && cols_attr
            .as_deref()
            .map(|cols| cols.trim().is_empty())
            .unwrap_or(true);

    let tracks = if use_rows {
        parse_frameset_track_sizes(rows_attr.as_deref(), layout_children.len(), total_height)
    } else {
        parse_frameset_track_sizes(cols_attr.as_deref(), layout_children.len(), total_width)
    };
    let mut composed = Canvas::new(total_width, total_height);
    let mut offset = 0u32;

    for (index, child) in layout_children.iter().enumerate() {
        let track = tracks.get(index).copied().unwrap_or(0);
        if track == 0 {
            continue;
        }

        let child_viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: if use_rows {
                total_width as f32
            } else {
                track as f32
            },
            height: if use_rows {
                track as f32
            } else {
                total_height as f32
            },
        };

        let child_canvas = if child.tag_name().as_deref() == Some("frameset") {
            match render_frameset_canvas(child, base_url, child_viewport, depth + 1, client)? {
                Some(canvas) => canvas,
                None => continue,
            }
        } else {
            let src = child
                .attributes()
                .and_then(|attrs| attrs.get("src").cloned())
                .map(|src| src.trim().to_string())
                .filter(|src| !src.is_empty())
                .ok_or(ScreenshotError::MissingFrameSource)?;
            let resolved = match base_url {
                Some(base) => resolve_url(base, &src)?,
                None => src.parse::<crate::http::Url>()?,
            };
            let response = client.get(&resolved.to_string())?;
            let html = decode_html_response(&response);
            let frame_document = TreeBuilder::parse_decoded(&html).document();
            render_document_or_frameset_canvas(
                &frame_document,
                Some(&resolved),
                child_viewport,
                depth + 1,
                client,
            )?
        };

        let child_width = child_canvas.width();
        let child_height = child_canvas.height();
        let frame_image = Image::new(child_width, child_height, child_canvas.into_pixels())
            .map_err(ScreenshotError::FrameImage)?;
        if use_rows {
            composed.draw_image(&frame_image, 0.0, offset as f32);
        } else {
            composed.draw_image(&frame_image, offset as f32, 0.0);
        }
        offset = offset.saturating_add(track);
    }

    Ok(Some(composed))
}

fn render_document_or_frameset_canvas(
    document: &NodeHandle,
    base_url: Option<&crate::http::Url>,
    viewport: Rect,
    depth: usize,
    client: &mut Client,
) -> Result<Canvas, ScreenshotError> {
    if let Some(canvas) = render_frameset_canvas(document, base_url, viewport, depth, client)? {
        return Ok(canvas);
    }
    render_document_with_url(document, viewport, base_url).map_err(ScreenshotError::FramePaint)
}

fn collect_frameset_layout_children(frameset: &NodeHandle) -> Vec<NodeHandle> {
    let mut out = Vec::new();
    for child in frameset.child_nodes() {
        collect_frameset_layout_children_from_node(&child, &mut out);
    }
    out
}

fn collect_frameset_layout_children_from_node(node: &NodeHandle, out: &mut Vec<NodeHandle>) {
    if node.node_type() != NodeType::Element {
        return;
    }

    match node.tag_name().as_deref() {
        Some("frame") => {
            let has_src = node
                .attributes()
                .and_then(|attrs| attrs.get("src").cloned())
                .map(|src| !src.trim().is_empty())
                .unwrap_or(false);
            if has_src {
                out.push(node.clone());
            }
            for child in node.child_nodes() {
                collect_frameset_layout_children_from_node(&child, out);
            }
        }
        Some("frameset") => {
            out.push(node.clone());
        }
        _ => {
            for child in node.child_nodes() {
                collect_frameset_layout_children_from_node(&child, out);
            }
        }
    }
}

fn resolve_frameset_render_document(
    document: &NodeHandle,
    base_url: Option<&crate::http::Url>,
) -> Result<(NodeHandle, Option<crate::http::Url>), ScreenshotError> {
    if document.query_selector("frameset").is_none() {
        return Ok((document.clone(), base_url.cloned()));
    }

    let frame = document
        .query_selector(r#"frame[name="right"]"#)
        .or_else(|| find_first_frame_with_src(document));
    let Some(frame) = frame else {
        return Ok((document.clone(), base_url.cloned()));
    };
    let Some(src) = frame
        .attributes()
        .and_then(|attrs| attrs.get("src").cloned())
        .map(|src| src.trim().to_string())
        .filter(|src| !src.is_empty())
    else {
        return Ok((document.clone(), base_url.cloned()));
    };

    let resolved = match base_url {
        Some(base) => resolve_url(base, &src)?,
        None => src.parse::<crate::http::Url>()?,
    };
    let response = Client::new().get(&resolved.to_string())?;
    let html = decode_html_response(&response);
    let frame_document = TreeBuilder::parse_decoded(&html).document();
    Ok((frame_document, Some(resolved)))
}

fn find_first_frame_with_src(node: &NodeHandle) -> Option<NodeHandle> {
    if node.node_type() == NodeType::Element
        && node.tag_name().as_deref() == Some("frame")
        && node
            .attributes()
            .and_then(|attrs| attrs.get("src").cloned())
            .map(|src| !src.trim().is_empty())
            .unwrap_or(false)
    {
        return Some(node.clone());
    }

    for child in node.child_nodes() {
        if let Some(found) = find_first_frame_with_src(&child) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error_reporting::{
        ErrorCategory, ErrorCode, ErrorReporter, ErrorSeverity, EventStore, ExecutionSurface,
        RawEvent, ReporterConfig, RetentionPolicy,
    };
    use crate::test_support::http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    use std::sync::Arc;

    #[test]
    fn typed_screenshot_errors_keep_visible_messages_and_sources() {
        let missing = ScreenshotError::MissingFrameSource;
        assert_eq!(missing.to_string(), "frame src is missing");

        let url = ScreenshotError::Url(UrlParseError::EmptyHost);
        assert_eq!(url.to_string(), "empty host in URL");
        assert!(std::error::Error::source(&url).is_some());

        let frame = ScreenshotError::FramePaint(PaintError::InvalidImageBuffer);
        assert_eq!(
            frame.to_string(),
            "failed to render frame document: InvalidImageBuffer"
        );

        let settle = ScreenshotError::Settle(JsonRpcError {
            code: -32000,
            message: "page did not settle".to_string(),
        });
        assert_eq!(settle.to_string(), "page did not settle");
        assert!(std::error::Error::source(&settle).is_some());
    }

    #[test]
    fn screenshot_records_recoverable_layout_image_decode_failure() {
        let directory = std::env::temp_dir().join(format!(
            "omoikane-layout-screenshot-report-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("events.sqlite");
        let config = ReporterConfig::from_values(Some("record-only"), None).unwrap();
        let reporter = Arc::new(
            ErrorReporter::new(&config, database.clone(), RetentionPolicy::default()).unwrap(),
        );
        let mut session = CdpSession::new().unwrap();
        session.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
        session
            .dispatch(
                "Page.navigate",
                serde_json::json!({"url": "data:text/html,<html><body><img src='data:image/png;base64,PRIVATE_IMAGE_939' alt='PRIVATE_ALT_939'></body></html>"}),
            )
            .unwrap();
        let png = capture_session_screenshot_png(
            &mut session,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 32.0,
                height: 32.0,
            },
        )
        .unwrap();
        let image = Image::decode_png(&png).unwrap();
        assert_eq!((image.width(), image.height()), (32, 32));
        reporter.flush().unwrap();
        let expected = RawEvent::new(
            ErrorCategory::Layout,
            ErrorSeverity::Warning,
            ErrorCode::new("LAYOUT_INLINE_IMAGE_DECODE_FAILED").unwrap(),
            ExecutionSurface::Headless,
            "Layout failed",
            &[("operation", "decode"), ("resource", "image")],
        )
        .sanitize();
        let store = EventStore::open(&database, RetentionPolicy::default()).unwrap();
        assert!(store.get(expected.fingerprint()).unwrap().is_some());
        drop(store);
        drop(session);
        drop(reporter);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn frameset_screenshot_fallback_records_safe_error_and_preserves_png() {
        let directory =
            std::env::temp_dir().join(format!("omoikane-screenshot-report-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("events.sqlite");
        let config = ReporterConfig::from_values(Some("record-only"), None).unwrap();
        let reporter = Arc::new(
            ErrorReporter::new(&config, database.clone(), RetentionPolicy::default()).unwrap(),
        );
        let page = "data:text/html,<html><frameset cols='100'><frame src='http://127.0.0.1:0/?token=QUERY_SECRET_940'></frameset><!-- BODY_SECRET_940 PATH_SECRET_940 --></html>";
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 32.0,
            height: 32.0,
        };
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch("Page.navigate", serde_json::json!({ "url": page }))
            .unwrap();
        assert!(session.document().query_selector("frameset").is_some());
        assert!(session.document().query_selector("frame").is_some());
        session.set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Headless);
        let png = capture_session_screenshot_png(&mut session, viewport).unwrap();
        let image = Image::decode_png(&png).unwrap();
        assert_eq!((image.width(), image.height()), (32, 32));

        let mut control = CdpSession::new().unwrap();
        control
            .dispatch("Page.navigate", serde_json::json!({ "url": page }))
            .unwrap();
        let expected_png = capture_session_screenshot_png(&mut control, viewport).unwrap();
        assert_eq!(png, expected_png);
        reporter.flush().unwrap();
        let event = RawEvent::new(
            ErrorCategory::Paint,
            ErrorSeverity::Error,
            ErrorCode::new("SCREENSHOT_FRAMESET_FALLBACK").unwrap(),
            ExecutionSurface::Headless,
            "Paint failed",
            &[("operation", "render"), ("resource", "other")],
        )
        .sanitize();
        let store = EventStore::open(&database, RetentionPolicy::default()).unwrap();
        assert!(store.len().unwrap() >= 1);
        assert!(store.get(event.fingerprint()).unwrap().is_some());
        drop(store);
        drop(session);
        drop(reporter);
        for suffix in ["", "-wal", "-shm"] {
            let mut path = database.as_os_str().to_os_string();
            path.push(suffix);
            if let Ok(bytes) = std::fs::read(std::path::PathBuf::from(path)) {
                for secret in ["QUERY_SECRET_940", "BODY_SECRET_940", "PATH_SECRET_940"] {
                    assert!(
                        !bytes
                            .windows(secret.len())
                            .any(|part| part == secret.as_bytes())
                    );
                }
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn parses_frameset_columns_as_percentage_when_sum_is_100() {
        let widths = parse_frameset_track_sizes(Some("18,82"), 2, 1000);
        assert_eq!(widths, vec![180, 820]);
    }

    #[test]
    fn parses_frameset_rows_as_percentage_when_sum_is_100() {
        let heights = parse_frameset_track_sizes(Some("30,70"), 2, 1000);
        assert_eq!(heights, vec![300, 700]);
    }

    #[test]
    fn frameset_tracks_never_exceed_the_visible_viewport() {
        assert_eq!(
            parse_frameset_track_sizes(Some("10000,20"), 2, 800),
            [799, 1]
        );
        assert_eq!(parse_frameset_track_sizes(Some("200%,*"), 2, 800), [800, 0]);
        assert_eq!(
            parse_frameset_track_sizes(Some("80%,80%,*"), 3, 800),
            [400, 400, 0]
        );
        assert_eq!(parse_frameset_track_sizes(Some("*,*,*"), 3, 2), [0, 0, 2]);
    }

    #[test]
    fn screenshot_budget_rejects_invalid_or_oversized_viewports_before_paint() {
        for width in [0.0, f32::NAN, f32::INFINITY] {
            let viewport = Rect {
                x: 0.0,
                y: 0.0,
                width,
                height: 32.0,
            };
            assert!(matches!(
                validate_screenshot_viewport(viewport),
                Err(ScreenshotError::InvalidViewport)
            ));
        }
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 16_384.0,
            height: 16_384.0,
        };
        assert!(matches!(
            validate_screenshot_viewport(viewport),
            Err(ScreenshotError::CanvasBudgetExceeded)
        ));
        assert_eq!(validate_canvas_dimensions(32, 24).unwrap(), 32 * 24 * 4);
        assert!(matches!(
            validate_canvas_dimensions(u32::MAX, u32::MAX),
            Err(ScreenshotError::CanvasBudgetExceeded)
        ));
    }

    #[test]
    fn nested_frameset_with_oversized_tracks_renders_within_viewport() {
        let listener = bind_loopback().unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = FixtureWorker::spawn(move || {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let body = "<html><body bgcolor='ff0000'></body></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        let html = format!(
            "<html><frameset cols='10000'><frameset rows='10000'><frame src='http://127.0.0.1:{port}/leaf'></frameset></frameset></html>"
        );
        let document = TreeBuilder::parse(&html).document();
        let canvas = render_frameset_canvas(
            &document,
            None,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 32.0,
                height: 24.0,
            },
            0,
            &mut Client::new(),
        )
        .unwrap()
        .unwrap();
        server.join();
        assert_eq!((canvas.width(), canvas.height()), (32, 24));
        assert_eq!(canvas.pixels().len(), 32 * 24 * 4);
    }

    #[test]
    fn session_screenshot_does_not_execute_document_scripts_twice() {
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch(
                "Page.navigate",
                serde_json::json!({
                    "url": "data:text/html,<html><body><script>document.body.setAttribute('data-runs', String(Number(document.body.getAttribute('data-runs') || 0) %2B 1))</script></body></html>"
                }),
            )
            .unwrap();

        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 32.0,
            height: 32.0,
        };
        let png = capture_session_screenshot_png(&mut session, viewport).unwrap();
        let image = Image::decode_png(&png).unwrap();
        assert_eq!((image.width(), image.height()), (32, 32));
        let metrics = session
            .dispatch(
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": "innerWidth + 'x' + innerHeight", "returnByValue": true,
                }),
            )
            .unwrap();
        assert_eq!(metrics["result"]["value"], "32x32");
        let runs = session
            .dispatch(
                "Runtime.evaluate",
                serde_json::json!({ "expression": "document.body.getAttribute('data-runs')" }),
            )
            .unwrap();

        assert_eq!(runs["result"]["value"], "1");
    }

    #[test]
    fn session_screenshot_fetches_all_referenced_frames_for_columns_frameset() {
        let listener = bind_loopback().unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = FixtureWorker::spawn(move || {
            let mut requested_paths = Vec::new();
            for _ in 0..3 {
                let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
                let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                requested_paths.push(path.clone());

                let body = if path == "/index.html" {
                    r#"<html><frameset cols="18,82"><frame src="/left.htm" name="left"><frame src="/right.htm" name="right"></frameset></html>"#.to_string()
                } else if path == "/right.htm" {
                    r#"<html><body bgcolor="ff0000"></body></html>"#.to_string()
                } else {
                    r#"<html><body bgcolor="00ff00"></body></html>"#.to_string()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
            requested_paths
        });

        let mut session = CdpSession::new().unwrap();
        session
            .dispatch(
                "Page.navigate",
                serde_json::json!({ "url": format!("http://127.0.0.1:{port}/index.html") }),
            )
            .unwrap();

        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 720.0,
        };
        let _png = capture_session_screenshot_png(&mut session, viewport).unwrap();
        let paths = server.join();
        assert!(paths.contains(&"/index.html".to_string()));
        assert!(paths.contains(&"/left.htm".to_string()));
        assert!(paths.contains(&"/right.htm".to_string()));
    }

    #[test]
    fn session_screenshot_fetches_all_referenced_frames_for_rows_frameset() {
        let listener = bind_loopback().unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = FixtureWorker::spawn(move || {
            let mut requested_paths = Vec::new();
            for _ in 0..3 {
                let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
                let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                requested_paths.push(path.clone());

                let body = if path == "/index.html" {
                    r#"<html><frameset rows="30,70"><frame src="/top.htm" name="top"><frame src="/bottom.htm" name="bottom"></frameset></html>"#.to_string()
                } else if path == "/top.htm" {
                    r#"<html><body bgcolor="ff0000"></body></html>"#.to_string()
                } else {
                    r#"<html><body bgcolor="00ff00"></body></html>"#.to_string()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
            requested_paths
        });

        let mut session = CdpSession::new().unwrap();
        session
            .dispatch(
                "Page.navigate",
                serde_json::json!({ "url": format!("http://127.0.0.1:{port}/index.html") }),
            )
            .unwrap();

        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 720.0,
        };
        let _png = capture_session_screenshot_png(&mut session, viewport).unwrap();
        let paths = server.join();
        assert!(paths.contains(&"/index.html".to_string()));
        assert!(paths.contains(&"/top.htm".to_string()));
        assert!(paths.contains(&"/bottom.htm".to_string()));
    }
}
