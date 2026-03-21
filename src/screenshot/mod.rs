use crate::cdp::CdpSession;
use crate::dom::{Node, NodeHandle, NodeType};
use crate::html::{TreeBuilder, decode_html_response};
use crate::http::Client;
use crate::http::url::resolve_url;
use crate::layout::Rect;
use crate::paint::{Canvas, Image, render_document_png_with_url, render_document_with_url};

const MAX_FRAMESET_DEPTH: usize = 4;

pub(crate) fn capture_session_screenshot_png(
    session: &mut CdpSession,
    viewport: Rect,
) -> Result<Vec<u8>, String> {
    let document = session.document();
    let base_url = session.current_url().parse::<crate::http::Url>().ok();

    match render_frameset_screenshot_png(
        &document,
        base_url.as_ref(),
        viewport,
        session.http_client_mut(),
    ) {
        Ok(Some(png)) => Ok(png),
        Ok(None) | Err(_) => {
            let (render_document, render_base_url) =
                resolve_frameset_render_document(&document, base_url.as_ref())
                    .unwrap_or((document.clone(), base_url.clone()));
            render_document_png_with_url(&render_document, viewport, render_base_url.as_ref())
                .map_err(|error| format!("{error:?}"))
        }
    }
}

fn render_frameset_screenshot_png(
    document: &NodeHandle,
    base_url: Option<&crate::http::Url>,
    viewport: Rect,
    client: &mut Client,
) -> Result<Option<Vec<u8>>, String> {
    let Some(canvas) = render_frameset_canvas(document, base_url, viewport, 0, client)? else {
        return Ok(None);
    };
    Ok(Some(canvas.encode_png()))
}

fn render_frameset_canvas(
    node: &NodeHandle,
    base_url: Option<&crate::http::Url>,
    viewport: Rect,
    depth: usize,
    client: &mut Client,
) -> Result<Option<Canvas>, String> {
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
                .ok_or_else(|| "frame src is missing".to_string())?;
            let resolved = match base_url {
                Some(base) => resolve_url(base, &src).map_err(|error| error.to_string())?,
                None => src
                    .parse::<crate::http::Url>()
                    .map_err(|error| error.to_string())?,
            };
            let response = client
                .get(&resolved.to_string())
                .map_err(|error| error.to_string())?;
            let html = decode_html_response(&response);
            let frame_document = TreeBuilder::parse(&html).document();
            render_document_or_frameset_canvas(
                &frame_document,
                Some(&resolved),
                child_viewport,
                depth + 1,
                client,
            )?
        };

        let frame_image = Image::new(
            child_canvas.width(),
            child_canvas.height(),
            child_canvas.pixels().to_vec(),
        )
        .map_err(|error| format!("failed to materialize frame image: {error:?}"))?;
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
) -> Result<Canvas, String> {
    if let Some(canvas) = render_frameset_canvas(document, base_url, viewport, depth, client)? {
        return Ok(canvas);
    }
    render_document_with_url(document, viewport, base_url)
        .map_err(|error| format!("failed to render frame document: {error:?}"))
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

fn parse_frameset_track_sizes(spec: Option<&str>, frame_count: usize, total_size: u32) -> Vec<u32> {
    if frame_count == 0 {
        return Vec::new();
    }

    let mut tokens: Vec<String> = spec
        .unwrap_or("")
        .split(',')
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        tokens.resize(frame_count, "*".to_string());
    }
    if tokens.len() < frame_count {
        tokens.resize(frame_count, "*".to_string());
    }
    if tokens.len() > frame_count {
        tokens.truncate(frame_count);
    }

    let all_plain_numeric = tokens
        .iter()
        .all(|token| !token.contains('*') && !token.ends_with('%') && token.parse::<f32>().is_ok());
    let numeric_sum = tokens
        .iter()
        .filter_map(|token| token.parse::<f32>().ok())
        .sum::<f32>();
    let treat_plain_as_percent = all_plain_numeric && (numeric_sum - 100.0).abs() <= 0.5;

    let mut widths = vec![0u32; frame_count];
    let mut star_weights = vec![0f32; frame_count];
    let mut assigned = 0u32;

    for (index, token) in tokens.iter().enumerate() {
        if let Some(percent) = token.strip_suffix('%').and_then(|v| v.parse::<f32>().ok()) {
            let width = ((total_size as f32) * (percent / 100.0)).round().max(0.0) as u32;
            widths[index] = width;
            assigned = assigned.saturating_add(width);
            continue;
        }
        if token.contains('*') {
            let weight = token.replace('*', "").trim().parse::<f32>().unwrap_or(1.0);
            star_weights[index] = weight.max(1.0);
            continue;
        }
        if let Ok(value) = token.parse::<f32>() {
            let width = if treat_plain_as_percent {
                ((total_size as f32) * (value / 100.0)).round().max(0.0) as u32
            } else {
                value.round().max(0.0) as u32
            };
            widths[index] = width;
            assigned = assigned.saturating_add(width);
            continue;
        }
        star_weights[index] = 1.0;
    }

    let remaining = total_size.saturating_sub(assigned);
    let total_star: f32 = star_weights.iter().sum();
    if total_star > 0.0 && remaining > 0 {
        for index in 0..frame_count {
            if star_weights[index] == 0.0 {
                continue;
            }
            let width = ((remaining as f32) * (star_weights[index] / total_star))
                .round()
                .max(0.0) as u32;
            widths[index] = widths[index].saturating_add(width);
        }
        let consumed: u32 = widths.iter().sum();
        if consumed < total_size {
            let delta = total_size - consumed;
            if let Some(last) = widths.last_mut() {
                *last = last.saturating_add(delta);
            }
        }
    } else if remaining > 0 {
        if let Some(last) = widths.last_mut() {
            *last = last.saturating_add(remaining);
        }
    }

    if widths.iter().all(|&w| w == 0) {
        let base = total_size / frame_count as u32;
        let mut out = vec![base; frame_count];
        let tail = total_size.saturating_sub(base * frame_count as u32);
        if let Some(last) = out.last_mut() {
            *last = last.saturating_add(tail);
        }
        return out;
    }

    widths
}

fn resolve_frameset_render_document(
    document: &NodeHandle,
    base_url: Option<&crate::http::Url>,
) -> Result<(NodeHandle, Option<crate::http::Url>), String> {
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
        Some(base) => resolve_url(base, &src).map_err(|error| error.to_string())?,
        None => src
            .parse::<crate::http::Url>()
            .map_err(|error| error.to_string())?,
    };
    let response = Client::new()
        .get(&resolved.to_string())
        .map_err(|error| error.to_string())?;
    let html = decode_html_response(&response);
    let frame_document = TreeBuilder::parse(&html).document();
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
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

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
    fn session_screenshot_fetches_all_referenced_frames_for_columns_frameset() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requested_paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let requested_paths_for_thread = Arc::clone(&requested_paths);
        thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(&stream);
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .to_string();
                requested_paths_for_thread
                    .lock()
                    .unwrap()
                    .push(path.clone());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                }

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
        let paths = requested_paths.lock().unwrap().clone();
        assert!(paths.contains(&"/index.html".to_string()));
        assert!(paths.contains(&"/left.htm".to_string()));
        assert!(paths.contains(&"/right.htm".to_string()));
    }

    #[test]
    fn session_screenshot_fetches_all_referenced_frames_for_rows_frameset() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requested_paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let requested_paths_for_thread = Arc::clone(&requested_paths);
        thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(&stream);
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .to_string();
                requested_paths_for_thread
                    .lock()
                    .unwrap()
                    .push(path.clone());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                }

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
        let paths = requested_paths.lock().unwrap().clone();
        assert!(paths.contains(&"/index.html".to_string()));
        assert!(paths.contains(&"/top.htm".to_string()));
        assert!(paths.contains(&"/bottom.htm".to_string()));
    }
}
