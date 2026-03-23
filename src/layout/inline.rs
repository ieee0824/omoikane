//! Inline layout: text segments, line breaking, and inline image handling.

use crate::css::{ComputedStyle, ComputedValue, PseudoElement, StyleResolver};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::font::{Font, load_default_text_fonts};
use crate::http::url::resolve_url;
use crate::paint::{DataUri, Image, parse_data_uri};

use super::{
    FontMetrics, FragmentStyle, InlineFragment, InlineFragmentContent,
    LineBox, Rect, VerticalAlign,
    edge_sizes, explicit_length, is_display_none, is_inline_child, is_non_rendered_html_element,
    IMAGE_BASE_URL, IMAGE_CACHE, HTTP_CLIENT, LAYOUT_FONTS,
};

// ── Text align ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TextAlign {
    Left,
    Right,
    Center,
}

pub(super) fn text_align(style: &ComputedStyle) -> TextAlign {
    match style.get("text-align") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("right") => {
            TextAlign::Right
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("center") => {
            TextAlign::Center
        }
        _ => TextAlign::Left,
    }
}

// ── Inline layout entry point ───────────────────────────────────────────────

pub(super) fn layout_inline_nodes(
    nodes: &[NodeHandle],
    resolver: &mut StyleResolver,
    start_x: f32,
    start_y: f32,
    available_width: f32,
    align: TextAlign,
    strut_line_height: f32,
) -> Vec<LineBox> {
    let mut segments = Vec::new();
    for node in nodes {
        collect_inline_segments(node, resolver, &mut segments);
    }

    layout_inline_segments(
        &segments,
        start_x,
        start_y,
        available_width,
        align,
        strut_line_height,
    )
}

// ── Inline segment types ────────────────────────────────────────────────────

/// CSS `word-break` property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum WordBreak {
    #[default]
    Normal,
    BreakAll,
    KeepAll,
    BreakWord,
}

/// CSS `overflow-wrap` property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum OverflowWrap {
    #[default]
    Normal,
    BreakWord,
    Anywhere,
}

pub(super) fn word_break(style: &ComputedStyle) -> WordBreak {
    match style.get("word-break") {
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("break-all") => WordBreak::BreakAll,
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("keep-all") => WordBreak::KeepAll,
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("break-word") => WordBreak::BreakWord,
        _ => WordBreak::Normal,
    }
}

pub(super) fn overflow_wrap(style: &ComputedStyle) -> OverflowWrap {
    match style.get("overflow-wrap") {
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("break-word") => OverflowWrap::BreakWord,
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("anywhere") => OverflowWrap::Anywhere,
        _ => OverflowWrap::Normal,
    }
}

#[derive(Debug, Clone)]
pub(super) struct InlineSegment {
    pub(super) node: NodeHandle,
    pub(super) content: InlineSegmentContent,
    pub(super) metrics: FontMetrics,
    pub(super) line_height: f32,
    pub(super) vertical_align: VerticalAlign,
    /// Minimal style information extracted from the owning element's
    /// `ComputedStyle` — forwarded to `InlineFragment` so that paint can
    /// apply per-fragment text-transform / text-decoration / color.
    pub(super) style: FragmentStyle,
    /// CSS `word-break` value resolved from the owning element's computed style.
    pub(super) word_break: WordBreak,
    /// CSS `overflow-wrap` value resolved from the owning element's computed style.
    pub(super) overflow_wrap: OverflowWrap,
    /// CSS `white-space` mode for this segment.
    pub(super) white_space_mode: WhiteSpaceMode,
}

#[derive(Debug, Clone)]
pub(super) enum InlineSegmentContent {
    Text(String),
    Image(Image, ComputedStyle, f32, f32),
    GeneratedBox(ComputedStyle),
}

// ── Inline segment collection ───────────────────────────────────────────────

fn collect_inline_segments(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    out: &mut Vec<InlineSegment>,
) {
    match node.node_type() {
        NodeType::Text => {
            if let Some(text) = node.data() {
                let parent_style = node
                    .parent_node()
                    .map(|parent| resolver.computed_style(&parent))
                    .unwrap_or_default();
                let text = normalize_text(&text, white_space(&parent_style));
                let text = apply_text_transform_layout(&text, &parent_style);
                if !text.is_empty() {
                    out.push(InlineSegment {
                        node: node.clone(),
                        content: InlineSegmentContent::Text(text),
                        metrics: font_metrics(&parent_style),
                        line_height: line_height(&parent_style),
                        vertical_align: vertical_align(&parent_style),
                        style: FragmentStyle::from_computed(&parent_style),
                        word_break: word_break(&parent_style),
                        overflow_wrap: overflow_wrap(&parent_style),
                        white_space_mode: white_space(&parent_style),
                    });
                }
            }
        }
        NodeType::Element => {
            if is_non_rendered_html_element(node) {
                return;
            }
            let style = resolver.computed_style(node);
            if is_display_none(&style) {
                return;
            }

            out.extend(generated_inline_segments(
                node,
                resolver,
                PseudoElement::Before,
            ));
            if let Some((image_node, image)) = element_inline_image(node) {
                let image_style = resolver.computed_style(&image_node);
                let padding = edge_sizes(&image_style, "padding");
                let border = edge_sizes(&image_style, "border");
                let (rendered_width, rendered_height) =
                    resolve_image_rendered_size(&image_node, &image, &image_style);
                out.push(InlineSegment {
                    node: image_node,
                    content: InlineSegmentContent::Image(
                        image.clone(),
                        image_style.clone(),
                        rendered_width,
                        rendered_height,
                    ),
                    metrics: font_metrics(&image_style),
                    word_break: word_break(&image_style),
                    overflow_wrap: overflow_wrap(&image_style),
                    white_space_mode: white_space(&image_style),
                    line_height: line_height(&image_style).max(
                        rendered_height + padding.top + padding.bottom + border.top + border.bottom,
                    ),
                    vertical_align: vertical_align(&image_style),
                    style: FragmentStyle::from_computed(&image_style),
                });
                out.extend(generated_inline_segments(
                    node,
                    resolver,
                    PseudoElement::After,
                ));
                return;
            }

            if node.tag_name().as_deref() == Some("img") {
                if let Some(alt_text) = image_alt_fallback_text(node, &style) {
                    out.push(InlineSegment {
                        node: node.clone(),
                        content: InlineSegmentContent::Text(alt_text),
                        metrics: font_metrics(&style),
                        line_height: line_height(&style),
                        vertical_align: vertical_align(&style),
                        style: FragmentStyle::from_computed(&style),
                        word_break: word_break(&style),
                        overflow_wrap: overflow_wrap(&style),
                        white_space_mode: white_space(&style),
                    });
                    out.extend(generated_inline_segments(
                        node,
                        resolver,
                        PseudoElement::After,
                    ));
                    return;
                }
            }
            for child in node.child_nodes() {
                match child.node_type() {
                    NodeType::Text => {
                        if let Some(text) = child.data() {
                            let text = normalize_text(&text, white_space(&style));
                            let text = apply_text_transform_layout(&text, &style);
                            if !text.is_empty() {
                                out.push(InlineSegment {
                                    node: child,
                                    content: InlineSegmentContent::Text(text),
                                    metrics: font_metrics(&style),
                                    line_height: line_height(&style),
                                    vertical_align: vertical_align(&style),
                                    style: FragmentStyle::from_computed(&style),
                                    word_break: word_break(&style),
                                    overflow_wrap: overflow_wrap(&style),
                                    white_space_mode: white_space(&style),
                                });
                            }
                        }
                    }
                    NodeType::Element if is_inline_child(&child, resolver) => {
                        collect_inline_segments(&child, resolver, out);
                    }
                    _ => {}
                }
            }
            out.extend(generated_inline_segments(
                node,
                resolver,
                PseudoElement::After,
            ));
        }
        _ => {}
    }
}

pub(super) fn generated_inline_segments(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    pseudo: PseudoElement,
) -> Vec<InlineSegment> {
    let Some(style) = resolver.computed_pseudo_style(node, pseudo) else {
        return Vec::new();
    };
    if is_display_none(&style) {
        return Vec::new();
    }

    let Some(content) = style.get("content") else {
        return Vec::new();
    };
    let metrics = font_metrics(&style);
    let line_height = line_height(&style);
    let vertical_align = vertical_align(&style);
    let wb = word_break(&style);
    let ow = overflow_wrap(&style);

    match generated_content_value(content) {
        Some(GeneratedContent::Text(text)) => vec![InlineSegment {
            node: node.clone(),
            content: if text.is_empty() {
                InlineSegmentContent::GeneratedBox(style.clone())
            } else {
                let normalized = normalize_text(&text, white_space(&style));
                // Apply text-transform during layout (same as regular text nodes)
                // so that layout width measurements use the transformed form.
                let transformed = apply_text_transform_layout(&normalized, &style);
                InlineSegmentContent::Text(transformed)
            },
            metrics,
            line_height,
            vertical_align,
            style: FragmentStyle::from_computed(&style),
            word_break: wb,
            overflow_wrap: ow,
            white_space_mode: white_space(&style),
        }],
        Some(GeneratedContent::Image(image)) => vec![InlineSegment {
            node: node.clone(),
            content: InlineSegmentContent::Image(
                image.clone(),
                style.clone(),
                image.width() as f32,
                image.height() as f32,
            ),
            metrics,
            line_height: line_height.max(metrics.font_size),
            vertical_align,
            style: FragmentStyle::from_computed(&style),
            word_break: wb,
            overflow_wrap: ow,
            white_space_mode: white_space(&style),
        }],
        None => Vec::new(),
    }
}

// ── Generated content ───────────────────────────────────────────────────────

enum GeneratedContent {
    Text(String),
    Image(Image),
}

fn generated_content_value(value: &ComputedValue) -> Option<GeneratedContent> {
    match value {
        ComputedValue::String(text) => Some(GeneratedContent::Text(text.clone())),
        ComputedValue::Keyword(keyword)
            if keyword.eq_ignore_ascii_case("none") || keyword.eq_ignore_ascii_case("normal") =>
        {
            None
        }
        ComputedValue::Keyword(keyword) => parse_generated_content_keyword(keyword),
        _ => None,
    }
}

fn parse_generated_content_keyword(keyword: &str) -> Option<GeneratedContent> {
    let url = keyword
        .strip_prefix("url(")
        .and_then(|value| value.strip_suffix(')'))?
        .trim()
        .trim_matches('"')
        .trim_matches('\'');
    let data_uri = parse_data_uri(url).ok()?;
    match data_uri {
        DataUri::Text { data, .. } => Some(GeneratedContent::Text(data)),
        DataUri::Binary { mime_type, data } if mime_type.eq_ignore_ascii_case("image/png") => {
            Image::decode_png(&data).ok().map(GeneratedContent::Image)
        }
        DataUri::Binary { .. } => None,
    }
}

// ── Image handling ──────────────────────────────────────────────────────────

pub(crate) fn element_inline_image(node: &NodeHandle) -> Option<(NodeHandle, Image)> {
    let tag_name = node.tag_name()?;
    let attributes = node.attributes().unwrap_or_default();
    match tag_name.as_str() {
        "img" => {
            let src = attributes.get("src")?;
            decode_or_fetch_image(src).map(|image| (node.clone(), image))
        }
        "svg" => {
            let image = crate::svg::render_svg_to_image(node)?;
            return Some((node.clone(), image));
        }
        "object" => {
            if let Some(data) = attributes.get("data") {
                if let Some(image) = decode_or_fetch_image(data) {
                    return Some((node.clone(), image));
                }
            }

            for child in node.child_nodes() {
                if let Some(image) = element_inline_image(&child) {
                    return Some(image);
                }
            }

            None
        }
        _ => None,
    }
}

/// Decode an image from a data: URI (PNG, JPEG, or SVG).
fn decode_data_uri_image(uri: &str) -> Option<Image> {
    let data_uri = parse_data_uri(uri).ok()?;
    match data_uri {
        DataUri::Binary { mime_type, data } => {
            if mime_type.eq_ignore_ascii_case("image/png") {
                Image::decode_png(&data).ok()
            } else if mime_type.eq_ignore_ascii_case("image/jpeg")
                || mime_type.eq_ignore_ascii_case("image/jpg")
            {
                Image::decode_jpeg(&data).ok()
            } else if mime_type.eq_ignore_ascii_case("image/svg+xml") {
                decode_svg_bytes(&data)
            } else {
                None
            }
        }
        DataUri::Text { mime_type, data } => {
            if mime_type.eq_ignore_ascii_case("image/svg+xml") {
                decode_svg_text(&data)
            } else {
                None
            }
        }
    }
}

/// Decode SVG from raw bytes (UTF-8 text).
fn decode_svg_bytes(bytes: &[u8]) -> Option<Image> {
    let text = std::str::from_utf8(bytes).ok()?;
    decode_svg_text(text)
}

/// Decode SVG from text, parse and rasterize.
fn decode_svg_text(text: &str) -> Option<Image> {
    use crate::dom::Node;
    use crate::html::TreeBuilder;
    let doc = TreeBuilder::parse(text).document();
    // Find the <svg> element in the parsed document
    fn find_svg(node: &NodeHandle) -> Option<NodeHandle> {
        if node.tag_name().as_deref() == Some("svg") {
            return Some(node.clone());
        }
        for child in node.child_nodes() {
            if let Some(found) = find_svg(&child) {
                return Some(found);
            }
        }
        None
    }
    let svg_node = find_svg(&doc)?;
    crate::svg::render_svg_to_image(&svg_node)
}

/// Maximum image size to fetch (10 MiB).
const MAX_IMAGE_SIZE: usize = 10 * 1024 * 1024;

/// Fetch an image from an HTTP/HTTPS URL with caching.
fn fetch_image(url: &str) -> Option<Image> {
    // Check cache first
    let cached = IMAGE_CACHE.with(|cache| cache.borrow().get(url).cloned());
    if let Some(result) = cached {
        return result;
    }

    // Fetch and decode the image
    let result = fetch_image_uncached(url);

    // Cache the result (even if None, to avoid re-fetching failed URLs)
    IMAGE_CACHE.with(|cache| {
        cache.borrow_mut().insert(url.to_string(), result.clone());
    });

    result
}

/// Fetch an image without caching (internal helper).
fn fetch_image_uncached(url: &str) -> Option<Image> {
    // Use shared HTTP client for connection reuse and cookie sharing
    let response = HTTP_CLIENT.with(|client| client.borrow_mut().get(url).ok())?;

    if response.status_code() != 200 {
        return None;
    }

    let body = response.body();

    // Enforce size limit to prevent DoS
    if body.len() > MAX_IMAGE_SIZE {
        return None;
    }

    let content_type: String = response
        .header("content-type")
        .map(str::to_lowercase)
        .unwrap_or_default();

    // Determine image type from Content-Type header or try both decoders
    if content_type.contains("image/svg+xml") || url.ends_with(".svg") {
        return decode_svg_bytes(body);
    }
    if content_type.contains("image/png") {
        Image::decode_png(body).ok()
    } else if content_type.contains("image/jpeg") || content_type.contains("image/jpg") {
        Image::decode_jpeg(body).ok()
    } else {
        // Try PNG first, then JPEG, then SVG
        Image::decode_png(body)
            .ok()
            .or_else(|| Image::decode_jpeg(body).ok())
            .or_else(|| decode_svg_bytes(body))
    }
}

fn decode_or_fetch_image(url_like: &str) -> Option<Image> {
    let url_like = url_like.trim();
    if url_like.is_empty() {
        return None;
    }
    if url_like.starts_with("data:") {
        return decode_data_uri_image(url_like);
    }
    let resolved = resolve_image_url(url_like)?;
    fetch_image(&resolved)
}

pub(crate) fn decode_or_fetch_image_asset(url_like: &str) -> Option<Image> {
    decode_or_fetch_image(url_like)
}

fn resolve_image_url(url_like: &str) -> Option<String> {
    if url_like.starts_with("http://") || url_like.starts_with("https://") {
        return Some(url_like.to_string());
    }
    if url_like.contains("://") || url_like.starts_with("//") {
        return None;
    }
    IMAGE_BASE_URL.with(|cell| {
        let base = cell.borrow().clone()?;
        resolve_url(&base, url_like).ok().map(|url| url.to_string())
    })
}

pub(super) fn image_alt_fallback_text(node: &NodeHandle, style: &ComputedStyle) -> Option<String> {
    let attributes = node.attributes().unwrap_or_default();
    let alt = attributes.get("alt")?;
    let normalized = normalize_text(alt, white_space(style));
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn html_image_dimension_attribute(node: &NodeHandle, name: &str) -> Option<f32> {
    let attributes = node.attributes().unwrap_or_default();
    let raw = attributes.get(name)?.trim();
    if raw.is_empty() {
        return None;
    }
    let parsed = raw.parse::<f32>().ok()?;
    if parsed.is_finite() && parsed > 0.0 {
        Some(parsed)
    } else {
        None
    }
}

pub(super) fn resolve_image_rendered_size(
    node: &NodeHandle,
    image: &Image,
    style: &ComputedStyle,
) -> (f32, f32) {
    let intrinsic_w = image.width() as f32;
    let intrinsic_h = image.height() as f32;
    let css_w = explicit_length(style, "width");
    let css_h = explicit_length(style, "height");

    match (css_w, css_h) {
        (Some(w), Some(h)) => return (w, h),
        (Some(w), None) => return (w, scale_with_aspect(intrinsic_h, intrinsic_w, w)),
        (None, Some(h)) => return (scale_with_aspect(intrinsic_w, intrinsic_h, h), h),
        (None, None) => {}
    }

    let attr_w = html_image_dimension_attribute(node, "width");
    let attr_h = html_image_dimension_attribute(node, "height");
    match (attr_w, attr_h) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, scale_with_aspect(intrinsic_h, intrinsic_w, w)),
        (None, Some(h)) => (scale_with_aspect(intrinsic_w, intrinsic_h, h), h),
        (None, None) => (intrinsic_w, intrinsic_h),
    }
}

fn scale_with_aspect(numerator: f32, denominator: f32, target: f32) -> f32 {
    if denominator > 0.0 {
        (numerator * target / denominator).max(0.0)
    } else {
        0.0
    }
}

// ── Text processing ─────────────────────────────────────────────────────────

pub(super) fn apply_text_transform_layout(text: &str, style: &ComputedStyle) -> String {
    match style.get("text-transform") {
        Some(ComputedValue::Keyword(kw)) => match kw.to_ascii_lowercase().as_str() {
            "uppercase" => text.to_uppercase(),
            "lowercase" => text.to_lowercase(),
            "capitalize" => {
                let mut result = String::with_capacity(text.len());
                let mut cap_next = true;
                for ch in text.chars() {
                    if ch.is_whitespace() {
                        cap_next = true;
                        result.push(ch);
                    } else if cap_next {
                        for c in ch.to_uppercase() {
                            result.push(c);
                        }
                        cap_next = false;
                    } else {
                        result.push(ch);
                    }
                }
                result
            }
            _ => text.to_string(),
        },
        _ => text.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WhiteSpaceMode {
    /// Collapse whitespace, no preserved newlines, allow wrapping.
    Normal,
    /// Preserve all whitespace and newlines, no wrapping.
    Pre,
    /// Collapse whitespace, no preserved newlines, no wrapping.
    Nowrap,
    /// Preserve whitespace and newlines, allow wrapping.
    PreWrap,
    /// Collapse whitespace sequences, preserve newlines, allow wrapping.
    PreLine,
}

impl WhiteSpaceMode {
    /// Whether the mode collapses whitespace sequences into a single space.
    pub(super) fn collapses_whitespace(self) -> bool {
        matches!(self, Self::Normal | Self::Nowrap | Self::PreLine)
    }

    /// Whether the mode preserves newline characters as line breaks.
    pub(super) fn preserves_newlines(self) -> bool {
        matches!(self, Self::Pre | Self::PreWrap | Self::PreLine)
    }

    /// Whether the mode allows automatic line wrapping.
    pub(super) fn allows_wrapping(self) -> bool {
        matches!(self, Self::Normal | Self::PreWrap | Self::PreLine)
    }
}

pub(super) fn white_space(style: &ComputedStyle) -> WhiteSpaceMode {
    match style.get("white-space") {
        Some(ComputedValue::Keyword(keyword)) => {
            match keyword.to_ascii_lowercase().as_str() {
                "pre" => WhiteSpaceMode::Pre,
                "nowrap" => WhiteSpaceMode::Nowrap,
                "pre-wrap" => WhiteSpaceMode::PreWrap,
                "pre-line" => WhiteSpaceMode::PreLine,
                _ => WhiteSpaceMode::Normal,
            }
        }
        _ => WhiteSpaceMode::Normal,
    }
}

pub(super) fn normalize_text(text: &str, mode: WhiteSpaceMode) -> String {
    if mode.collapses_whitespace() {
        if mode.preserves_newlines() {
            // pre-line: collapse whitespace but keep newlines
            collapse_white_space_preserve_newlines(text)
        } else {
            collapse_white_space(text)
        }
    } else {
        // pre, pre-wrap: preserve all whitespace
        text.to_string()
    }
}

fn collapse_white_space_preserve_newlines(text: &str) -> String {
    // First pass: collapse whitespace within lines, preserving newlines.
    let mut out = String::new();
    let mut previous_was_space = false;
    for ch in text.chars() {
        if ch == '\n' {
            // Drop trailing space before newline
            if out.ends_with(' ') {
                out.pop();
            }
            out.push('\n');
            previous_was_space = true; // suppress leading space after newline
        } else if ch.is_ascii_whitespace() {
            if !previous_was_space {
                out.push(' ');
            }
            previous_was_space = true;
        } else {
            out.push(ch);
            previous_was_space = false;
        }
    }
    out
}

fn collapse_white_space(text: &str) -> String {
    let mut out = String::new();
    let mut previous_was_space = false;

    for ch in text.chars() {
        // CSS 2.1 §16.6.1: only ASCII whitespace (space, tab, newline, etc.)
        // is collapsible. Non-breaking space (U+00A0) is NOT collapsible.
        if ch != '\u{00A0}' && ch.is_whitespace() {
            if !previous_was_space {
                out.push(' ');
                previous_was_space = true;
            }
        } else {
            out.push(ch);
            previous_was_space = false;
        }
    }

    out
}

pub(super) fn font_size(style: &ComputedStyle) -> f32 {
    explicit_length(style, "font-size").unwrap_or(16.0)
}

pub(super) fn font_metrics(style: &ComputedStyle) -> FontMetrics {
    let mut metrics = FontMetrics::from_font_size(font_size(style));
    metrics.letter_spacing = letter_spacing(style);
    metrics
}

fn letter_spacing(style: &ComputedStyle) -> f32 {
    match style.get("letter-spacing") {
        Some(ComputedValue::Px(value)) => *value,
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("normal") => 0.0,
        _ => 0.0,
    }
}

pub(super) fn line_height(style: &ComputedStyle) -> f32 {
    match style.get("line-height") {
        Some(ComputedValue::Px(value)) => *value,
        Some(ComputedValue::Number(value)) => *value * font_size(style),
        Some(ComputedValue::Percentage(value)) => font_size(style) * value / 100.0,
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("normal") => {
            font_size(style) * 1.2
        }
        _ => font_size(style) * 1.2,
    }
}

pub(super) fn vertical_align(style: &ComputedStyle) -> VerticalAlign {
    match style.get("vertical-align") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("top") => {
            VerticalAlign::Top
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("middle") => {
            VerticalAlign::Middle
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("bottom") => {
            VerticalAlign::Bottom
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("sub") => {
            // sub: lower by ~0.4em
            VerticalAlign::Length(-font_size(style) * 0.4)
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("super") => {
            // super: raise by ~0.6em
            VerticalAlign::Length(font_size(style) * 0.6)
        }
        Some(ComputedValue::Px(value)) => VerticalAlign::Length(*value),
        Some(ComputedValue::Number(value)) => VerticalAlign::Length(*value),
        _ => VerticalAlign::Baseline,
    }
}

// ── Inline layout engine ────────────────────────────────────────────────────

fn layout_inline_segments(
    segments: &[InlineSegment],
    start_x: f32,
    start_y: f32,
    available_width: f32,
    align: TextAlign,
    strut_line_height: f32,
) -> Vec<LineBox> {
    let mut lines = Vec::new();
    let mut current_fragments = Vec::new();
    let mut cursor_x = start_x;
    let mut cursor_y = start_y;
    let mut current_line_height: f32 = strut_line_height;

    let mut prev_segment_allows_wrapping = true;
    for segment in segments {
        let overflow_wrap = segment.overflow_wrap;
        let allows_wrapping = segment.white_space_mode.allows_wrapping();
        let mut is_first_piece_in_segment = true;
        for piece in split_segment(segment) {
            match piece {
                InlinePiece::Newline => {
                    push_line(
                        &mut lines,
                        &mut current_fragments,
                        start_x,
                        cursor_y,
                        cursor_x - start_x,
                        current_line_height.max(segment.line_height),
                        available_width,
                        align,
                    );
                    cursor_y += current_line_height.max(segment.line_height);
                    cursor_x = start_x;
                    current_line_height = strut_line_height;
                }
                InlinePiece::Fragment {
                    content,
                    width,
                    height,
                } => {
                    // Allow wrapping at segment boundary only if the previous
                    // segment allowed wrapping (i.e., not inside a nowrap run).
                    // Inside a segment, use the segment's own allows_wrapping.
                    let can_wrap = if is_first_piece_in_segment {
                        prev_segment_allows_wrapping
                    } else {
                        allows_wrapping
                    };
                    is_first_piece_in_segment = false;
                    if can_wrap && cursor_x > start_x && cursor_x + width > start_x + available_width {
                        push_line(
                            &mut lines,
                            &mut current_fragments,
                            start_x,
                            cursor_y,
                            cursor_x - start_x,
                            current_line_height.max(segment.line_height),
                            available_width,
                            align,
                        );
                        cursor_y += current_line_height.max(segment.line_height);
                        cursor_x = start_x;
                        current_line_height = strut_line_height;
                    }

                    // overflow-wrap: break-word / anywhere, and the non-standard
                    // word-break: break-word — if the fragment still doesn't fit
                    // even at the start of a fresh line, break it character by
                    // character.
                    let needs_char_break = (matches!(
                        overflow_wrap,
                        OverflowWrap::BreakWord | OverflowWrap::Anywhere
                    ) || segment.word_break == WordBreak::BreakWord)
                        && cursor_x == start_x
                        && width > available_width
                        && available_width > 0.0;

                    if needs_char_break {
                        if let InlineFragmentContent::Text(text) = content {
                            for ch_str in split_chars(&text) {
                                let ch_width = measure_text_width(&ch_str, segment.metrics);
                                if cursor_x > start_x
                                    && cursor_x + ch_width > start_x + available_width
                                {
                                    push_line(
                                        &mut lines,
                                        &mut current_fragments,
                                        start_x,
                                        cursor_y,
                                        cursor_x - start_x,
                                        current_line_height.max(segment.line_height),
                                        available_width,
                                        align,
                                    );
                                    cursor_y += current_line_height.max(segment.line_height);
                                    cursor_x = start_x;
                                    current_line_height = strut_line_height;
                                }
                                current_fragments.push(InlineFragment {
                                    node: segment.node.clone(),
                                    content: InlineFragmentContent::Text(ch_str),
                                    rect: Rect {
                                        x: cursor_x,
                                        y: cursor_y,
                                        width: ch_width,
                                        height,
                                    },
                                    metrics: segment.metrics,
                                    vertical_align: segment.vertical_align,
                                    style: segment.style.clone(),
                                });
                                cursor_x += ch_width;
                                current_line_height =
                                    current_line_height.max(segment.line_height.max(height));
                            }
                            continue;
                        }
                    }

                    current_fragments.push(InlineFragment {
                        node: segment.node.clone(),
                        content,
                        rect: Rect {
                            x: cursor_x,
                            y: cursor_y,
                            width,
                            height,
                        },
                        metrics: segment.metrics,
                        vertical_align: segment.vertical_align,
                        style: segment.style.clone(),
                    });
                    cursor_x += width;
                    current_line_height = current_line_height.max(segment.line_height.max(height));
                }
            }
        }
        prev_segment_allows_wrapping = allows_wrapping;
    }

    if !current_fragments.is_empty() {
        push_line(
            &mut lines,
            &mut current_fragments,
            start_x,
            cursor_y,
            cursor_x - start_x,
            current_line_height.max(0.0),
            available_width,
            align,
        );
    }

    lines
}

enum InlinePiece {
    Newline,
    Fragment {
        content: InlineFragmentContent,
        width: f32,
        height: f32,
    },
}

fn split_segment(segment: &InlineSegment) -> Vec<InlinePiece> {
    match &segment.content {
        InlineSegmentContent::Text(text) => {
            split_text_segment(
                text,
                segment.metrics,
                segment.line_height,
                segment.word_break,
                segment.white_space_mode,
            )
        }
        InlineSegmentContent::Image(image, style, rendered_width, rendered_height) => {
            let padding = edge_sizes(style, "padding");
            let border = edge_sizes(style, "border");
            vec![InlinePiece::Fragment {
                content: InlineFragmentContent::Image(image.clone(), style.clone()),
                width: *rendered_width + padding.left + padding.right + border.left + border.right,
                height: *rendered_height
                    + padding.top
                    + padding.bottom
                    + border.top
                    + border.bottom,
            }]
        }
        InlineSegmentContent::GeneratedBox(style) => {
            let padding = edge_sizes(style, "padding");
            let border = edge_sizes(style, "border");
            let width = explicit_length(style, "width").unwrap_or(0.0)
                + padding.left
                + padding.right
                + border.left
                + border.right;
            let height = explicit_length(style, "height").unwrap_or(0.0)
                + padding.top
                + padding.bottom
                + border.top
                + border.bottom;

            vec![InlinePiece::Fragment {
                content: InlineFragmentContent::GeneratedBox(style.clone()),
                width,
                height,
            }]
        }
    }
}

fn split_text_segment(
    text: &str,
    metrics: FontMetrics,
    line_height: f32,
    wb: WordBreak,
    ws_mode: WhiteSpaceMode,
) -> Vec<InlinePiece> {
    // `word-break: break-word` is a non-standard value treated identically to
    // `word-break: normal` for segment-level splitting; the actual emergency
    // break behaviour is handled in the line-building loop (same as
    // `overflow-wrap: break-word`).
    let split_fn: fn(&str) -> Vec<String> = match wb {
        WordBreak::BreakAll => split_chars,
        WordBreak::KeepAll => split_words_no_cjk_break,
        WordBreak::Normal | WordBreak::BreakWord => split_words_preserving_spaces_cjk,
    };

    // For nowrap: treat the entire text as one fragment (no splitting into words).
    if !ws_mode.allows_wrapping() {
        if ws_mode.preserves_newlines() && text.contains('\n') {
            let mut pieces = Vec::new();
            let line_count = text.split('\n').count();
            for (index, part) in text.split('\n').enumerate() {
                if !part.is_empty() {
                    pieces.push(InlinePiece::Fragment {
                        width: measure_text_width(part, metrics),
                        height: line_height,
                        content: InlineFragmentContent::Text(part.to_string()),
                    });
                }
                if index + 1 < line_count {
                    pieces.push(InlinePiece::Newline);
                }
            }
            return pieces;
        }
        return vec![InlinePiece::Fragment {
            width: measure_text_width(text, metrics),
            height: line_height,
            content: InlineFragmentContent::Text(text.to_string()),
        }];
    }

    if ws_mode.preserves_newlines() && text.contains('\n') {
        let mut pieces = Vec::new();
        let line_count = text.split('\n').count();
        for (index, part) in text.split('\n').enumerate() {
            if !part.is_empty() {
                pieces.extend(
                    split_fn(part)
                        .into_iter()
                        .map(|piece| InlinePiece::Fragment {
                            width: measure_text_width(&piece, metrics),
                            height: line_height,
                            content: InlineFragmentContent::Text(piece),
                        }),
                );
            }
            if index + 1 < line_count {
                pieces.push(InlinePiece::Newline);
            }
        }
        pieces
    } else if text.contains('\n') && !ws_mode.collapses_whitespace() {
        // pre/pre-wrap with newlines
        let mut pieces = Vec::new();
        let line_count = text.split('\n').count();
        for (index, part) in text.split('\n').enumerate() {
            if !part.is_empty() {
                pieces.extend(
                    split_fn(part)
                        .into_iter()
                        .map(|piece| InlinePiece::Fragment {
                            width: measure_text_width(&piece, metrics),
                            height: line_height,
                            content: InlineFragmentContent::Text(piece),
                        }),
                );
            }
            if index + 1 < line_count {
                pieces.push(InlinePiece::Newline);
            }
        }
        pieces
    } else {
        split_fn(text)
            .into_iter()
            .map(|piece| InlinePiece::Fragment {
                width: measure_text_width(&piece, metrics),
                height: line_height,
                content: InlineFragmentContent::Text(piece),
            })
            .collect()
    }
}

/// Split text into individual characters for `word-break: break-all`.
/// Exported for tests.
/// Spaces remain as their own piece so that trailing-space collapsing still works.
pub(crate) fn split_chars(text: &str) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch == ' ' {
            if !current.is_empty() {
                pieces.push(std::mem::take(&mut current));
            }
            // keep space as its own piece
            pieces.push(" ".to_string());
        } else {
            if !current.is_empty() {
                // Each non-space character becomes its own breakable unit
                pieces.push(std::mem::take(&mut current));
            }
            current.push(ch);
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

/// Split text without allowing breaks between CJK characters (`word-break: keep-all`).
/// CJK characters are treated like non-CJK word characters; breaks only occur at spaces.
pub(crate) fn split_words_no_cjk_break(text: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch == ' ' {
            // Break before space
            if !current.is_empty() {
                pieces.push(std::mem::take(&mut current));
            }
            pieces.push(" ".to_string());
        } else {
            // All non-space characters (including CJK) stay together
            current.push(ch);
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

// ── CJK text breaking ───────────────────────────────────────────────────────

/// Check if a character is a CJK ideograph or related character.
/// This includes CJK Unified Ideographs, Hiragana, Katakana, and common symbols.
fn is_cjk_char(ch: char) -> bool {
    matches!(ch,
        // CJK Unified Ideographs
        '\u{4E00}'..='\u{9FFF}' |
        // CJK Unified Ideographs Extension A
        '\u{3400}'..='\u{4DBF}' |
        // Hiragana
        '\u{3040}'..='\u{309F}' |
        // Katakana
        '\u{30A0}'..='\u{30FF}' |
        // Katakana Phonetic Extensions
        '\u{31F0}'..='\u{31FF}' |
        // Halfwidth and Fullwidth Forms (Katakana)
        '\u{FF65}'..='\u{FF9F}' |
        // CJK Symbols and Punctuation
        '\u{3000}'..='\u{303F}' |
        // Fullwidth ASCII variants
        '\u{FF01}'..='\u{FF60}' |
        // Hangul Syllables
        '\u{AC00}'..='\u{D7AF}'
    )
}

/// Characters that must not appear at the start of a line (line-start prohibited).
/// These are typically closing punctuation and certain symbols.
fn is_line_start_prohibited(ch: char) -> bool {
    matches!(
        ch,
        // Japanese punctuation
        '\u{3002}' | '\u{3001}' | '\u{FF0C}' | '\u{FF0E}' | '\u{30FB}' | '\u{FF1A}' | '\u{FF1B}' | '\u{FF01}' | '\u{FF1F}' |
        // Closing brackets
        '\u{FF09}' | '\u{300D}' | '\u{300F}' | '\u{3011}' | '\u{3015}' | '\u{FF5D}' | '\u{FF3D}' |
        // Other
        '\u{30FC}' | '\u{FF5E}' | '\u{2026}' | '\u{2025}' |
        // Small kana
        '\u{3041}' | '\u{3043}' | '\u{3045}' | '\u{3047}' | '\u{3049}' | '\u{3063}' | '\u{3083}' | '\u{3085}' | '\u{3087}' | '\u{308E}' |
        '\u{30A1}' | '\u{30A3}' | '\u{30A5}' | '\u{30A7}' | '\u{30A9}' | '\u{30C3}' | '\u{30E3}' | '\u{30E5}' | '\u{30E7}' | '\u{30EE}'
    )
}

/// Characters that must not appear at the end of a line (line-end prohibited).
/// These are typically opening punctuation.
fn is_line_end_prohibited(ch: char) -> bool {
    matches!(
        ch,
        // Opening brackets
        '\u{FF08}' | '\u{300C}' | '\u{300E}' | '\u{3010}' | '\u{3014}' | '\u{FF5B}' | '\u{FF3B}'
    )
}

/// Split text into pieces that can be laid out, with CJK-aware breaking.
/// This allows line breaks between CJK characters while respecting kinsoku rules.
pub(crate) fn split_words_preserving_spaces_cjk(text: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        let is_space = ch == ' ';
        let is_cjk = is_cjk_char(ch);

        // Check if we should break before this character
        let should_break = if current.is_empty() {
            false
        } else if is_space {
            // Break before space if previous was not space
            !current.ends_with(' ')
        } else if is_cjk {
            let prev_char = current.chars().last().unwrap();
            let prev_is_cjk = is_cjk_char(prev_char);
            let prev_is_space = prev_char == ' ';

            if prev_is_space {
                // Always break after space
                true
            } else if prev_is_cjk {
                // Between two CJK characters: check kinsoku rules
                // Don't break if current char is line-start prohibited
                // Don't break if previous char is line-end prohibited
                !is_line_start_prohibited(ch) && !is_line_end_prohibited(prev_char)
            } else {
                // Transition from non-CJK to CJK
                true
            }
        } else {
            // Non-CJK, non-space character
            let prev_char = current.chars().last().unwrap();
            let prev_is_space = prev_char == ' ';
            let prev_is_cjk = is_cjk_char(prev_char);

            if prev_is_space {
                true
            } else if prev_is_cjk {
                // Transition from CJK to non-CJK: allow break unless kinsoku
                !is_line_start_prohibited(ch)
            } else {
                false
            }
        };

        if should_break && !current.is_empty() {
            pieces.push(std::mem::take(&mut current));
        }

        current.push(ch);
    }

    if !current.is_empty() {
        pieces.push(current);
    }

    pieces
}

// ── Line building ───────────────────────────────────────────────────────────

fn push_line(
    lines: &mut Vec<LineBox>,
    fragments: &mut Vec<InlineFragment>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    available_width: f32,
    align: TextAlign,
) {
    let offset_x = match align {
        TextAlign::Left => 0.0,
        TextAlign::Right => (available_width - width).max(0.0),
        TextAlign::Center => (available_width - width).max(0.0) / 2.0,
    };
    for fragment in fragments.iter_mut() {
        fragment.rect.x += offset_x;
    }

    let baseline = fragments
        .iter()
        .filter_map(|fragment| match fragment.vertical_align {
            VerticalAlign::Baseline | VerticalAlign::Length(_) => Some(fragment.metrics.ascent),
            _ => None,
        })
        .fold(0.0f32, f32::max)
        .max(height * 0.8);

    for fragment in fragments.iter_mut() {
        fragment.rect.y = match fragment.vertical_align {
            VerticalAlign::Baseline => y + baseline - fragment.metrics.ascent,
            VerticalAlign::Length(shift) => y + baseline - fragment.metrics.ascent - shift,
            VerticalAlign::Top => y,
            VerticalAlign::Middle => y + (height - fragment.rect.height) / 2.0,
            VerticalAlign::Bottom => y + height - fragment.rect.height,
        };
    }

    lines.push(LineBox {
        rect: Rect {
            x: x + offset_x,
            y,
            width,
            height,
        },
        baseline: y + baseline,
        fragments: std::mem::take(fragments),
    });
}

// ── Text measurement ────────────────────────────────────────────────────────

pub(super) fn measure_text_width(text: &str, metrics: FontMetrics) -> f32 {
    LAYOUT_FONTS.with(|cell| {
        let mut fonts_ref = cell.borrow_mut();
        if fonts_ref.is_none() {
            *fonts_ref = Some(load_layout_fonts());
        }

        if let Some(ref fonts) = *fonts_ref {
            if !fonts.is_empty() {
                let base = measure_text_width_with_fallback(text, metrics.font_size, fonts);
                let char_count = text.chars().count();
                let spacing = if char_count > 1 {
                    metrics.letter_spacing * (char_count - 1) as f32
                } else {
                    0.0
                };
                return base + spacing;
            }
        }

        // Fallback to approximation when no font is available
        let char_count = text.chars().count();
        let base = char_count as f32 * metrics.average_advance;
        let spacing = if char_count > 1 {
            metrics.letter_spacing * (char_count - 1) as f32
        } else {
            0.0
        };
        base + spacing
    })
}

fn load_layout_fonts() -> Vec<Font> {
    load_default_text_fonts()
}

fn measure_text_width_with_fallback(text: &str, font_size: f32, fonts: &[Font]) -> f32 {
    let mut width = 0.0;
    let mut previous: Option<(char, usize)> = None;

    for ch in text.chars() {
        let font_index = select_layout_font_index(fonts, ch);

        if let Some((prev_char, prev_index)) = previous
            && prev_index == font_index
        {
            width += fonts[font_index].glyph_kerning(prev_char, ch, font_size);
        }

        let advance = fonts[font_index].glyph_advance(ch, font_size);
        width += if advance > 0.0 { advance } else { 0.0 };
        previous = Some((ch, font_index));
    }

    width
}

fn select_layout_font_index(fonts: &[Font], ch: char) -> usize {
    let prefer_cjk = is_cjk_preferred_character(ch);

    // Always try the primary font (index 0) first, even for CJK characters.
    // The primary font is allowed to render .notdef (missing glyph) — this
    // matches paint-side rasterize_with_fallback which also accepts index 0
    // unconditionally. Only fallback fonts (index > 0) require has_glyph.
    if prefer_cjk && fonts.len() > 1 {
        // Try CJK-capable fallback fonts first
        for index in 1..fonts.len() {
            if fonts[index].has_glyph(ch) {
                return index;
            }
        }
        // Fall back to primary (accepts .notdef like paint side)
        return 0;
    }

    for index in 0..fonts.len() {
        if index != 0 && !ch.is_whitespace() && !fonts[index].has_glyph(ch) {
            continue;
        }
        return index;
    }

    0
}

use crate::font::is_cjk_preferred_character;
