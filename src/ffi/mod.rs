//! C FFI surface for embedding the browser engine from other languages.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};

use base64::Engine;
use encoding_rs::Encoding;
use serde_json::json;

use crate::cdp::CdpSession;
use crate::dom::Node;
use crate::html::TreeBuilder;
use crate::http::Client;
use crate::http::url::resolve_url;
use crate::layout::Rect;
use crate::paint::{Canvas, Image, render_document_with_url};

const MAX_FRAMESET_DEPTH: usize = 4;

/// Opaque browser handle for the C ABI.
#[repr(C)]
pub struct OmoikaneBrowser {
    _private: [u8; 0],
}

struct OmoikaneBrowserHandle {
    session: RefCell<CdpSession>,
    last_error: RefCell<Option<String>>,
}

impl OmoikaneBrowserHandle {
    fn new() -> Result<Self, String> {
        Ok(Self {
            session: RefCell::new(CdpSession::new()?),
            last_error: RefCell::new(None),
        })
    }

    fn set_error(&self, message: impl Into<String>) {
        *self.last_error.borrow_mut() = Some(message.into());
    }

    fn clear_error(&self) {
        *self.last_error.borrow_mut() = None;
    }
}

/// Creates a new browser handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_init() -> *mut OmoikaneBrowser {
    match OmoikaneBrowserHandle::new() {
        Ok(browser) => Box::into_raw(Box::new(browser)) as *mut OmoikaneBrowser,
        Err(_) => std::ptr::null_mut(),
    }
}

/// Destroys a browser handle previously created by [`omoikane_init`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_free(browser: *mut OmoikaneBrowser) {
    if browser.is_null() {
        return;
    }

    // SAFETY: `browser` was created by `Box::into_raw` in `omoikane_init`.
    unsafe {
        drop(Box::from_raw(browser as *mut OmoikaneBrowserHandle));
    }
}

/// Navigates the active page to `url`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_navigate(
    browser: *mut OmoikaneBrowser,
    url: *const c_char,
) -> bool {
    let Some(browser) = browser_from_ptr(browser) else {
        return false;
    };
    let Some(url) = string_from_ptr(url) else {
        browser.set_error("url must be a valid UTF-8 string");
        return false;
    };

    let result = browser
        .session
        .borrow_mut()
        .dispatch("Page.navigate", json!({ "url": url }));

    match result {
        Ok(_) => {
            browser.clear_error();
            true
        }
        Err(error) => {
            browser.set_error(error.message);
            false
        }
    }
}

/// Sets the `User-Agent` used for subsequent navigations.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_set_user_agent(
    browser: *mut OmoikaneBrowser,
    user_agent: *const c_char,
) -> bool {
    let Some(browser) = browser_from_ptr(browser) else {
        return false;
    };
    let Some(user_agent) = string_from_ptr(user_agent) else {
        browser.set_error("user_agent must be a valid UTF-8 string");
        return false;
    };

    browser.session.borrow_mut().set_user_agent(user_agent);
    browser.clear_error();
    true
}

/// Evaluates JavaScript in the current page and returns a JSON payload string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_evaluate(
    browser: *mut OmoikaneBrowser,
    expression: *const c_char,
) -> *mut c_char {
    let Some(browser) = browser_from_ptr(browser) else {
        return std::ptr::null_mut();
    };
    let Some(expression) = string_from_ptr(expression) else {
        browser.set_error("expression must be a valid UTF-8 string");
        return std::ptr::null_mut();
    };

    let result = browser.session.borrow_mut().dispatch(
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
        }),
    );

    match result.and_then(|value| {
        serde_json::to_string(&value).map_err(|error| crate::cdp::JsonRpcError {
            code: -32000,
            message: error.to_string(),
        })
    }) {
        Ok(serialized) => {
            browser.clear_error();
            into_c_string(serialized)
        }
        Err(error) => {
            browser.set_error(error.message);
            std::ptr::null_mut()
        }
    }
}

/// Returns the current document serialized as HTML.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_get_content(browser: *mut OmoikaneBrowser) -> *mut c_char {
    let Some(browser) = browser_from_ptr(browser) else {
        return std::ptr::null_mut();
    };

    let result = browser
        .session
        .borrow_mut()
        .dispatch("DOM.getDocument", json!({}));
    let html = match result.and_then(|document| {
        let node_id =
            document["root"]["nodeId"]
                .as_u64()
                .ok_or_else(|| crate::cdp::JsonRpcError {
                    code: -32000,
                    message: "DOM.getDocument did not return a root nodeId".to_string(),
                })?;
        browser
            .session
            .borrow_mut()
            .dispatch("DOM.getOuterHTML", json!({ "nodeId": node_id }))
    }) {
        Ok(value) => value["outerHTML"].as_str().map(ToString::to_string),
        Err(error) => {
            browser.set_error(error.message);
            return std::ptr::null_mut();
        }
    };

    match html {
        Some(html) => {
            browser.clear_error();
            into_c_string(html)
        }
        None => {
            browser.set_error("DOM.getOuterHTML did not return outerHTML");
            std::ptr::null_mut()
        }
    }
}

/// Captures the current page rendering and returns a base64-encoded PNG string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_screenshot_png(browser: *mut OmoikaneBrowser) -> *mut c_char {
    let Some(browser) = browser_from_ptr(browser) else {
        return std::ptr::null_mut();
    };

    let (document, base_url) = {
        let session = browser.session.borrow();
        let document = session.document();
        let base_url = session.current_url().parse::<crate::http::Url>().ok();
        (document, base_url)
    };
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 1280.0,
        height: 720.0,
    };
    let rendered = match render_frameset_screenshot_png(&document, base_url.as_ref(), viewport) {
        Ok(Some(png)) => Ok(png),
        Ok(None) | Err(_) => {
            let (render_document, render_base_url) =
                resolve_frameset_render_document(&document, base_url.as_ref()).unwrap_or((
                    document.clone(),
                    base_url.clone(),
                ));
            crate::paint::render_document_png_with_url(&render_document, viewport, render_base_url.as_ref())
        }
    };

    match rendered {
        Ok(png) => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(png);
            browser.clear_error();
            into_c_string(encoded)
        }
        Err(error) => {
            browser.set_error(format!("failed to render screenshot: {error:?}"));
            std::ptr::null_mut()
        }
    }
}

fn render_frameset_screenshot_png(
    document: &crate::dom::NodeHandle,
    base_url: Option<&crate::http::Url>,
    viewport: Rect,
) -> Result<Option<Vec<u8>>, String> {
    let Some(canvas) = render_frameset_canvas(document, base_url, viewport, 0)? else {
        return Ok(None);
    };
    Ok(Some(canvas.encode_png()))
}

fn render_frameset_canvas(
    node: &crate::dom::NodeHandle,
    base_url: Option<&crate::http::Url>,
    viewport: Rect,
    depth: usize,
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
            match render_frameset_canvas(child, base_url, child_viewport, depth + 1)? {
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
                None => src.parse::<crate::http::Url>().map_err(|error| error.to_string())?,
            };
            let response = Client::new()
                .get(&resolved.to_string())
                .map_err(|error| error.to_string())?;
            let html = decode_html_response(&response);
            let frame_document = TreeBuilder::parse(&html).document();
            render_document_or_frameset_canvas(
                &frame_document,
                Some(&resolved),
                child_viewport,
                depth + 1,
            )?
        };

        let frame_image = Image::decode_png(&child_canvas.encode_png())
            .map_err(|error| format!("failed to decode rendered frame png: {error:?}"))?;
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
    document: &crate::dom::NodeHandle,
    base_url: Option<&crate::http::Url>,
    viewport: Rect,
    depth: usize,
) -> Result<Canvas, String> {
    if let Some(canvas) = render_frameset_canvas(document, base_url, viewport, depth)? {
        return Ok(canvas);
    }
    render_document_with_url(document, viewport, base_url)
        .map_err(|error| format!("failed to render frame document: {error:?}"))
}

fn collect_frameset_layout_children(frameset: &crate::dom::NodeHandle) -> Vec<crate::dom::NodeHandle> {
    let mut out = Vec::new();
    for child in frameset.child_nodes() {
        collect_frameset_layout_children_from_node(&child, &mut out);
    }
    out
}

fn collect_frameset_layout_children_from_node(
    node: &crate::dom::NodeHandle,
    out: &mut Vec<crate::dom::NodeHandle>,
) {
    if node.node_type() != crate::dom::NodeType::Element {
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
            // Old frameset markup can be mis-nested by permissive parsing.
            // Keep walking through <frame> descendants to recover sibling frames.
            for child in node.child_nodes() {
                collect_frameset_layout_children_from_node(&child, out);
            }
        }
        Some("frameset") => {
            // Keep nested framesets as a single layout item; inner frames are handled recursively.
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

    let all_plain_numeric = tokens.iter().all(|token| {
        !token.contains('*') && !token.ends_with('%') && token.parse::<f32>().is_ok()
    });
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

fn decode_html_response(response: &crate::http::HttpResponse) -> String {
    let body = response.body();
    let charset = response
        .header("content-type")
        .and_then(parse_charset_from_content_type)
        .or_else(|| detect_charset_from_html_meta(body));

    if let Some(label) = charset.as_deref() {
        if let Some(encoding) = Encoding::for_label(label.as_bytes()) {
            let (decoded, _, _) = encoding.decode(body);
            return decoded.into_owned();
        }
    }

    String::from_utf8_lossy(body).to_string()
}

fn parse_charset_from_content_type(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|part| {
        let (name, value) = part.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if value.is_empty() {
            None
        } else {
            Some(value.to_ascii_lowercase())
        }
    })
}

fn detect_charset_from_html_meta(body: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&body[..body.len().min(8192)]).to_string();
    let lower = head.to_ascii_lowercase();
    let mut cursor = 0usize;
    while let Some(relative) = lower[cursor..].find("<meta") {
        let start = cursor + relative;
        let Some(end_rel) = lower[start..].find('>') else {
            break;
        };
        let end = start + end_rel + 1;
        let tag = &head[start..end];
        if let Some(attributes) = parse_html_attributes(tag) {
            if let Some(charset) = attributes
                .get("charset")
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty())
            {
                return Some(charset);
            }
            let has_content_type_equiv = attributes
                .get("http-equiv")
                .map(|value| value.trim().eq_ignore_ascii_case("content-type"))
                .unwrap_or(false);
            if has_content_type_equiv {
                if let Some(content) = attributes.get("content") {
                    if let Some(charset) = parse_charset_from_content_type(content) {
                        return Some(charset);
                    }
                }
            }
        }
        cursor = end;
    }
    None
}

fn parse_html_attributes(tag: &str) -> Option<std::collections::BTreeMap<String, String>> {
    let mut attributes = std::collections::BTreeMap::new();
    let open = tag.find('<')?;
    let close = tag.rfind('>')?;
    if close <= open {
        return None;
    }
    let mut chars = tag[open + 1..close].chars().peekable();

    while let Some(ch) = chars.peek() {
        if ch.is_ascii_whitespace() {
            chars.next();
        } else {
            break;
        }
    }

    while let Some(ch) = chars.peek() {
        if ch.is_ascii_whitespace() {
            break;
        }
        chars.next();
    }

    loop {
        while let Some(ch) = chars.peek() {
            if ch.is_ascii_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        let mut name = String::new();
        while let Some(ch) = chars.peek() {
            if ch.is_ascii_whitespace() || *ch == '=' || *ch == '/' {
                break;
            }
            name.push(*ch);
            chars.next();
        }
        if name.is_empty() {
            break;
        }
        while let Some(ch) = chars.peek() {
            if ch.is_ascii_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        let mut value = String::new();
        if chars.peek() == Some(&'=') {
            chars.next();
            while let Some(ch) = chars.peek() {
                if ch.is_ascii_whitespace() {
                    chars.next();
                } else {
                    break;
                }
            }
            if let Some(quote) = chars.peek().copied().filter(|c| *c == '"' || *c == '\'') {
                chars.next();
                while let Some(ch) = chars.peek() {
                    if *ch == quote {
                        chars.next();
                        break;
                    }
                    value.push(*ch);
                    chars.next();
                }
            } else {
                while let Some(ch) = chars.peek() {
                    if ch.is_ascii_whitespace() || *ch == '/' {
                        break;
                    }
                    value.push(*ch);
                    chars.next();
                }
            }
        }
        attributes.insert(name.to_ascii_lowercase(), value);
    }

    Some(attributes)
}

fn resolve_frameset_render_document(
    document: &crate::dom::NodeHandle,
    base_url: Option<&crate::http::Url>,
) -> Result<(crate::dom::NodeHandle, Option<crate::http::Url>), String> {
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
        None => src.parse::<crate::http::Url>().map_err(|error| error.to_string())?,
    };
    let response = Client::new()
        .get(&resolved.to_string())
        .map_err(|error| error.to_string())?;
    let html = decode_html_response(&response);
    let frame_document = TreeBuilder::parse(&html).document();
    Ok((frame_document, Some(resolved)))
}

fn find_first_frame_with_src(node: &crate::dom::NodeHandle) -> Option<crate::dom::NodeHandle> {
    if node.node_type() == crate::dom::NodeType::Element
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

/// Returns the last error message for the browser handle, if any.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_last_error(browser: *const OmoikaneBrowser) -> *mut c_char {
    let Some(browser) = browser_from_const_ptr(browser) else {
        return std::ptr::null_mut();
    };

    match browser.last_error.borrow().clone() {
        Some(error) => into_c_string(error),
        None => std::ptr::null_mut(),
    }
}

/// Frees a string allocated by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_string_free(value: *mut c_char) {
    if value.is_null() {
        return;
    }

    // SAFETY: `value` must have come from `CString::into_raw` in this module.
    unsafe {
        drop(CString::from_raw(value));
    }
}

fn browser_from_ptr<'a>(browser: *mut OmoikaneBrowser) -> Option<&'a OmoikaneBrowserHandle> {
    if browser.is_null() {
        None
    } else {
        // SAFETY: Caller promises a valid pointer for the duration of the call.
        Some(unsafe { &*(browser as *mut OmoikaneBrowserHandle) })
    }
}

fn browser_from_const_ptr<'a>(
    browser: *const OmoikaneBrowser,
) -> Option<&'a OmoikaneBrowserHandle> {
    if browser.is_null() {
        None
    } else {
        // SAFETY: Caller promises a valid pointer for the duration of the call.
        Some(unsafe { &*(browser as *const OmoikaneBrowserHandle) })
    }
}

fn string_from_ptr(value: *const c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }

    // SAFETY: Caller promises a valid NUL-terminated C string.
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .ok()
        .map(ToString::to_string)
}

fn into_c_string(value: String) -> *mut c_char {
    CString::new(value)
        .expect("FFI strings must not contain interior NUL bytes")
        .into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use encoding_rs::SHIFT_JIS;
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    fn to_c_string(value: &str) -> CString {
        CString::new(value).unwrap()
    }

    unsafe fn take_string(value: *mut c_char) -> String {
        assert!(!value.is_null());
        let owned = unsafe { CString::from_raw(value) };
        owned.into_string().unwrap()
    }

    #[test]
    fn ffi_can_navigate_evaluate_and_read_content() {
        let browser = unsafe { omoikane_init() };
        assert!(!browser.is_null());

        let url =
            to_c_string("data:text/html,<html><body><main id=\"app\">ffi</main></body></html>");
        let ok = unsafe { omoikane_navigate(browser, url.as_ptr()) };
        assert!(ok);

        let expression = to_c_string("document.getElementById('app').nodeName");
        let evaluated = unsafe { omoikane_evaluate(browser, expression.as_ptr()) };
        let payload = unsafe { take_string(evaluated) };
        assert!(payload.contains("\"MAIN\""));

        let content = unsafe { omoikane_get_content(browser) };
        let html = unsafe { take_string(content) };
        assert!(html.contains("<main id=\"app\">ffi</main>"));

        unsafe { omoikane_free(browser) };
    }

    #[test]
    fn ffi_can_override_user_agent_for_navigation() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();

            let mut user_agent = None;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    break;
                }
                if let Some((name, value)) = trimmed.split_once(':') {
                    if name.trim().eq_ignore_ascii_case("user-agent") {
                        user_agent = Some(value.trim().to_string());
                    }
                }
            }

            assert_eq!(user_agent.as_deref(), Some("FFIAgent/1.0"));

            let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        });

        let browser = unsafe { omoikane_init() };
        assert!(!browser.is_null());

        let user_agent = to_c_string("FFIAgent/1.0");
        let ok = unsafe { omoikane_set_user_agent(browser, user_agent.as_ptr()) };
        assert!(ok);

        let url = to_c_string(&format!("http://127.0.0.1:{port}/"));
        let ok = unsafe { omoikane_navigate(browser, url.as_ptr()) };
        assert!(ok);

        unsafe { omoikane_free(browser) };
    }

    #[test]
    fn ffi_exposes_last_error_for_invalid_javascript() {
        let browser = unsafe { omoikane_init() };
        assert!(!browser.is_null());

        let expression = to_c_string("(()");
        let evaluated = unsafe { omoikane_evaluate(browser, expression.as_ptr()) };
        assert!(evaluated.is_null());

        let error = unsafe { omoikane_last_error(browser) };
        let message = unsafe { take_string(error) };
        assert!(message.contains("SyntaxError"));

        unsafe { omoikane_free(browser) };
    }

    #[test]
    fn ffi_can_capture_base64_png_screenshot() {
        let browser = unsafe { omoikane_init() };
        assert!(!browser.is_null());

        let url = to_c_string(
            "data:text/html,<html><head><style>html,body{margin:0;background:#123456;}</style></head><body></body></html>",
        );
        let ok = unsafe { omoikane_navigate(browser, url.as_ptr()) };
        assert!(ok);

        let screenshot = unsafe { omoikane_screenshot_png(browser) };
        let payload = unsafe { take_string(screenshot) };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap();

        assert!(bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]));
        assert!(bytes.len() > 24);
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        assert_eq!(width, 1280);
        assert_eq!(height, 720);

        unsafe { omoikane_free(browser) };
    }

    #[test]
    fn generated_header_includes_core_exports() {
        let header = fs::read_to_string("include/omoikane.h").unwrap();

        assert!(header.contains("omoikane_init"));
        assert!(header.contains("omoikane_navigate"));
        assert!(header.contains("omoikane_evaluate"));
        assert!(header.contains("omoikane_string_free"));
    }

    #[test]
    fn ffi_screenshot_renders_right_frame_for_frameset_documents() {
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

        let browser = unsafe { omoikane_init() };
        assert!(!browser.is_null());
        let url = to_c_string(&format!("http://127.0.0.1:{port}/index.html"));
        assert!(unsafe { omoikane_navigate(browser, url.as_ptr()) });

        let screenshot = unsafe { omoikane_screenshot_png(browser) };
        let _payload = unsafe { take_string(screenshot) };
        let paths = requested_paths.lock().unwrap().clone();
        assert!(
            paths.contains(&"/index.html".to_string()),
            "requested paths: {:?}",
            paths
        );
        assert!(
            paths.contains(&"/left.htm".to_string()),
            "requested paths: {:?}",
            paths
        );
        assert!(
            paths.contains(&"/right.htm".to_string()),
            "requested paths: {:?}",
            paths
        );

        unsafe { omoikane_free(browser) };
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
    fn ffi_screenshot_renders_rows_frameset_documents() {
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

        let browser = unsafe { omoikane_init() };
        assert!(!browser.is_null());
        let url = to_c_string(&format!("http://127.0.0.1:{port}/index.html"));
        assert!(unsafe { omoikane_navigate(browser, url.as_ptr()) });

        let screenshot = unsafe { omoikane_screenshot_png(browser) };
        let _payload = unsafe { take_string(screenshot) };
        let paths = requested_paths.lock().unwrap().clone();
        assert!(
            paths.contains(&"/index.html".to_string()),
            "requested paths: {:?}",
            paths
        );
        assert!(
            paths.contains(&"/top.htm".to_string()),
            "requested paths: {:?}",
            paths
        );
        assert!(
            paths.contains(&"/bottom.htm".to_string()),
            "requested paths: {:?}",
            paths
        );

        unsafe { omoikane_free(browser) };
    }

    #[test]
    fn decodes_html_response_using_content_type_charset() {
        let (encoded, _, _) = SHIFT_JIS.encode("阿部寛");
        let response = crate::http::HttpResponse::new(
            200,
            "OK",
            vec![(
                "Content-Type".to_string(),
                "text/html; charset=Shift_JIS".to_string(),
            )],
            encoded.into_owned(),
        );
        let decoded = decode_html_response(&response);
        assert_eq!(decoded, "阿部寛");
    }

    #[test]
    fn detects_charset_from_meta_http_equiv_content_type() {
        let html = br#"<html><head><meta http-equiv="Content-Type" content="text/html; charset=Shift_JIS"></head></html>"#;
        let detected = detect_charset_from_html_meta(html);
        assert_eq!(detected.as_deref(), Some("shift_jis"));
    }

    #[test]
    fn detects_charset_from_meta_charset_attribute() {
        let html = br#"<html><head><meta charset="EUC-JP"></head></html>"#;
        let detected = detect_charset_from_html_meta(html);
        assert_eq!(detected.as_deref(), Some("euc-jp"));
    }
}
