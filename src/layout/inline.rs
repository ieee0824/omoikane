//! Inline layout: text segments, line breaking, and inline image handling.

use std::sync::Arc;

use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};
use unicode_segmentation::UnicodeSegmentation;

use crate::css::{ComputedStyle, ComputedValue, PseudoElement, StyleResolver};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::font::{
    Font, FontFamilyKey, FontStyle, FontWeight, ShapingDirection, is_zero_advance_character,
    load_default_text_fonts, shape_text_with_fallback,
};
use crate::http::{HttpRequest, Url, url::resolve_url};
use crate::paint::{DataUri, Image, parse_data_uri};

use super::{
    FontMetrics, FragmentStyle, InlineFragment, InlineFragmentContent,
    LineBox, Rect, TextControlPaintState, VerticalAlign,
    border_box_adjust_length, edge_sizes, explicit_length, is_border_box, is_display_none,
    is_non_rendered_html_element,
    HTTP_CLIENT, IMAGE_ANIMATION_CACHE, IMAGE_ANIMATION_TIME_MS, IMAGE_BASE_URL, IMAGE_CACHE,
    LAYOUT_FONTS,
};

// ── Text align ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TextAlign {
    /// The logical inline-start edge.  It resolves to the physical left or
    /// right edge according to the containing block's `direction`.
    Start,
    /// The logical inline-end edge.  It resolves to the physical edge
    /// opposite [`TextAlign::Start`].
    End,
    Left,
    Right,
    Center,
}

pub(super) fn text_align(style: &ComputedStyle) -> TextAlign {
    match style.get("text-align") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("start") => {
            TextAlign::Start
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("end") => {
            TextAlign::End
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("right") => {
            TextAlign::Right
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("left") => {
            TextAlign::Left
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("center") => {
            TextAlign::Center
        }
        // `start` is the CSS initial value.  Keeping it logical means that a
        // block with `direction: rtl` naturally starts at its right edge even
        // when no explicit text-align declaration is present.
        _ => TextAlign::Start,
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
    direction_rtl: bool,
) -> Vec<LineBox> {
    let mut segments = Vec::new();
    for node in nodes {
        collect_inline_segments(node, resolver, &mut segments);
    }
    coalesce_adjacent_text_segments(&mut segments);

    layout_inline_segments(
        &segments,
        start_x,
        start_y,
        available_width,
        align,
        strut_line_height,
        direction_rtl,
    )
}

/// Lays out an inline formatting context whose inline axis is vertical.
///
/// The text shaping and line-breaking code is intentionally shared with the
/// horizontal path: it already measures glyph advances, applies white-space
/// and word-breaking rules, and produces deterministic DOM-order fragments.
/// We run that algorithm in a local horizontal coordinate system (where the
/// available width is the physical inline length), then transpose each line
/// into a vertical column.  This keeps wrapping decisions identical while
/// making column stacking and fragment geometry explicit for the block layout
/// and paint stages.
pub(super) fn layout_vertical_inline_nodes(
    nodes: &[NodeHandle],
    resolver: &mut StyleResolver,
    start_x: f32,
    start_y: f32,
    available_width: f32,
    available_height: f32,
    align: TextAlign,
    strut_line_height: f32,
    vertical_rl: bool,
    direction_rtl: bool,
) -> Vec<LineBox> {
    let mut segments = Vec::new();
    for node in nodes {
        collect_inline_segments(node, resolver, &mut segments);
    }
    coalesce_adjacent_text_segments(&mut segments);

    // Horizontal layout's x axis is the vertical inline axis in this local
    // coordinate system.  A local origin of zero makes the transpose below
    // independent of the containing block's absolute position.
    let horizontal_lines = layout_inline_segments(
        &segments,
        0.0,
        0.0,
        available_height.max(0.0),
        align,
        strut_line_height,
        false,
    );

    horizontal_lines
        .into_iter()
        .map(|line| {
            // Horizontal line stacking (local y) becomes vertical block-axis
            // column stacking.  A vertical-rl block starts at the right edge;
            // vertical-lr starts at the left edge.
            let column_width = line.rect.height.max(0.0);
            let column_offset = line.rect.y;
            let column_x = if vertical_rl {
                start_x + (available_width - column_offset - column_width).max(0.0)
            } else {
                start_x + column_offset
            };

            let mut fragments = line.fragments;
            for fragment in &mut fragments {
                // The local fragment's y offset is vertical-align within its
                // line.  Its x offset is the inline-axis position and maps to
                // physical y.
                let inline_offset = fragment.rect.x - line.rect.x;
                let cross_offset = fragment.rect.y - line.rect.y;
                fragment.rect = Rect {
                    x: column_x + cross_offset,
                    y: if direction_rtl {
                        start_y + (line.rect.width - inline_offset - fragment.rect.width).max(0.0)
                    } else {
                        start_y + inline_offset
                    },
                    width: fragment.rect.height,
                    height: fragment.rect.width,
                };
            }

            LineBox {
                rect: Rect {
                    x: column_x,
                    y: start_y + line.rect.x,
                    width: column_width,
                    height: line.rect.width.max(0.0),
                },
                // Baselines are currently consumed only by horizontal paint;
                // retain the inline-axis baseline in the transposed space so
                // callers inspecting line geometry still get a stable value.
                baseline: start_y + line.baseline,
                fragments,
            }
        })
        .collect()
}

/// Adjacent text nodes with identical formatting form one continuous inline
/// text run. Comments and DOM node boundaries do not introduce a soft wrap
/// opportunity (for example, `word<!-- -->.` must stay one word).
fn coalesce_adjacent_text_segments(segments: &mut Vec<InlineSegment>) {
    let mut coalesced: Vec<InlineSegment> = Vec::with_capacity(segments.len());
    for segment in segments.drain(..) {
        if let Some(previous) = coalesced.last_mut()
            && previous.node.parent_node() == segment.node.parent_node()
            && previous.metrics == segment.metrics
            && previous.line_height == segment.line_height
            && previous.vertical_align == segment.vertical_align
            && previous.style == segment.style
            && previous.word_break == segment.word_break
            && previous.overflow_wrap == segment.overflow_wrap
            && previous.white_space_mode == segment.white_space_mode
            && let (
                InlineSegmentContent::Text(previous_text),
                InlineSegmentContent::Text(text),
            ) = (&mut previous.content, &segment.content)
        {
            previous_text.push_str(text);
            continue;
        }
        coalesced.push(segment);
    }
    *segments = coalesced;
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
    FormControl(ComputedStyle, String, Option<TextControlPaintState>, f32, f32),
    IconFormControl(ComputedStyle, Image, f32, f32, f32, f32),
}

// ── Inline segment collection ───────────────────────────────────────────────

/// Creates a text `InlineSegment` from a node, text content, and resolved style.
/// Returns `None` when the normalized + transformed text is empty.
fn make_text_segment(
    node: NodeHandle,
    text: &str,
    style: &ComputedStyle,
) -> Option<InlineSegment> {
    let text = normalize_text(text, white_space(style));
    let text = apply_text_transform_layout(&text, style);
    if text.is_empty() {
        return None;
    }
    Some(InlineSegment {
        node,
        content: InlineSegmentContent::Text(text),
        metrics: font_metrics(style),
        line_height: line_height(style),
        vertical_align: vertical_align(style),
        style: FragmentStyle::from_computed(style),
        word_break: word_break(style),
        overflow_wrap: overflow_wrap(style),
        white_space_mode: white_space(style),
    })
}

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
                out.extend(make_text_segment(node.clone(), &text, &parent_style));
            }
        }
        NodeType::Element => {
            collect_element_inline_segments(node, resolver, out);
        }
        _ => {}
    }
}

fn collect_element_inline_segments(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    out: &mut Vec<InlineSegment>,
) {
    if is_non_rendered_html_element(node) {
        return;
    }
    let style = resolver.computed_style(node);
    if is_display_none(&style) {
        return;
    }

    match node.tag_name().as_deref() {
        Some("input") => {
            collect_input_segment(node, &style, out);
            return;
        }
        Some("button") => {
            collect_button_segment(node, &style, resolver, out);
            return;
        }
        Some("textarea") => {
            collect_textarea_segment(node, &style, resolver, out);
            return;
        }
        Some("select") => {
            collect_select_segment(node, &style, resolver, out);
            return;
        }
        Some("progress" | "meter") => {
            collect_value_indicator_segment(node, &style, out);
            return;
        }
        Some("audio" | "canvas" | "video") if element_inline_image(node).is_none() => {
            collect_media_placeholder_segment(node, &style, out);
            return;
        }
        _ => {}
    }

    out.extend(generated_inline_segments(node, resolver, PseudoElement::Before));

    if let Some((image_node, image)) = element_inline_image_with_style(node, &style) {
        collect_image_segment(&image_node, &image, resolver, out);
        out.extend(generated_inline_segments(node, resolver, PseudoElement::After));
        return;
    }

    if node.tag_name().as_deref() == Some("img")
        && let Some(alt_text) = image_alt_fallback_text(node, &style) {
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
            out.extend(generated_inline_segments(node, resolver, PseudoElement::After));
            return;
        }

    for child in node.layout_child_nodes() {
        match child.node_type() {
            NodeType::Text => {
                if let Some(text) = child.data() {
                    out.extend(make_text_segment(child, &text, &style));
                }
            }
            NodeType::Element => {
                collect_inline_segments(&child, resolver, out);
            }
            _ => {}
        }
    }
    out.extend(generated_inline_segments(node, resolver, PseudoElement::After));
}

fn collect_input_segment(
    node: &NodeHandle,
    style: &ComputedStyle,
    out: &mut Vec<InlineSegment>,
) {
    let attributes = node.attributes().unwrap_or_default();
    let input_type = attributes
        .get("type")
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "text".to_string());
    if input_type == "hidden" {
        return;
    }

    let metrics = font_metrics(style);
    let button_like = matches!(input_type.as_str(), "submit" | "button" | "reset");
    let value = node
        .text_control_state()
        .map(|state| state.value)
        .or_else(|| attributes.get("value").cloned())
        .unwrap_or_else(|| match input_type.as_str() {
            "submit" => "Submit".to_string(),
            "reset" => "Reset".to_string(),
            _ => String::new(),
        });
    let content_width = explicit_length(style, "width").unwrap_or_else(|| {
        if button_like {
            measure_text_width(&value, metrics)
        } else {
            let columns = attributes
                .get("size")
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(20)
                .clamp(1, 1000);
            metrics.average_advance * columns as f32
        }
    });
    let content_height =
        explicit_length(style, "height").unwrap_or_else(|| metrics.font_size.max(13.0));

    push_form_control_segment(node, style, value, content_width, content_height, metrics, out);
}

/// Collects a `<button>` as a single inline `FormControl` fragment.
///
/// The label is the concatenation of the button's rendered descendant text
/// (non-rendered elements such as `<style>`/`<script>` and `display: none`
/// subtrees are excluded) with runs of whitespace collapsed to single spaces;
/// child elements are never laid out independently. An icon-only button keeps
/// its first descendant image or SVG and centers it in the control. The width
/// defaults to the label text width (box padding and border are added when the
/// fragment is split), and an explicit `width`/`height` takes precedence.
fn collect_button_segment(
    node: &NodeHandle,
    style: &ComputedStyle,
    resolver: &mut StyleResolver,
    out: &mut Vec<InlineSegment>,
) {
    let metrics = font_metrics(style);
    let label = normalize_inline_whitespace(&collect_rendered_text(node, resolver));
    let content_width = explicit_length(style, "width")
        .unwrap_or_else(|| measure_text_width(&label, metrics));
    let content_height =
        explicit_length(style, "height").unwrap_or_else(|| metrics.font_size.max(13.0));

    if label.is_empty()
        && let Some((image_node, image)) = find_descendant_inline_image(node, resolver)
    {
        let image_style = resolver.computed_style(&image_node);
        let (icon_width, icon_height) =
            resolve_image_rendered_size(&image_node, &image, &image_style);
        let padding = edge_sizes(style, "padding");
        let border = edge_sizes(style, "border");
        let total_height = content_height + padding.top + padding.bottom + border.top + border.bottom;
        out.push(InlineSegment {
            node: node.clone(),
            content: InlineSegmentContent::IconFormControl(
                style.clone(), image, content_width, content_height, icon_width, icon_height,
            ),
            metrics,
            line_height: line_height(style).max(total_height),
            vertical_align: vertical_align(style),
            style: FragmentStyle::from_computed(style),
            word_break: word_break(style),
            overflow_wrap: overflow_wrap(style),
            white_space_mode: white_space(style),
        });
        return;
    }

    push_form_control_segment(node, style, label, content_width, content_height, metrics, out);
}

fn find_descendant_inline_image(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
) -> Option<(NodeHandle, Image)> {
    for child in node.layout_child_nodes() {
        if is_non_rendered_html_element(&child) {
            continue;
        }
        let child_style = resolver.computed_style(&child);
        if is_display_none(&child_style) {
            continue;
        }
        if let Some(image) = element_inline_image_with_style(&child, &child_style) {
            return Some(image);
        }
        if let Some(image) = find_descendant_inline_image(&child, resolver) {
            return Some(image);
        }
    }
    None
}

/// Collects a `<textarea>` as a single inline `FormControl` fragment.
///
/// The displayed value is the element's `textContent` (its initial value) with
/// a single leading newline (`\n` or `\r\n`) removed, per the HTML spec's
/// textarea value rules. The width defaults to `cols` (default 20, clamped to
/// `1..=1000`) multiplied by the average character advance, and the height
/// defaults to `rows` (default 2, clamped to `1..=1000`) multiplied by the line
/// height. Explicit `width`/`height` take precedence.
fn collect_textarea_segment(
    node: &NodeHandle,
    style: &ComputedStyle,
    resolver: &mut StyleResolver,
    out: &mut Vec<InlineSegment>,
) {
    let attributes = node.attributes().unwrap_or_default();
    let metrics = font_metrics(style);
    let value = node.text_control_state().map(|state| state.value).unwrap_or_else(|| {
        strip_textarea_leading_newline(&collect_rendered_text(node, resolver)).to_string()
    });
    let content_width = explicit_length(style, "width").unwrap_or_else(|| {
        let cols = attributes
            .get("cols")
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(20)
            .clamp(1, 1000);
        metrics.average_advance * cols as f32
    });
    let content_height = explicit_length(style, "height").unwrap_or_else(|| {
        let rows = attributes
            .get("rows")
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(2)
            .clamp(1, 1000);
        line_height(style) * rows as f32
    });

    push_form_control_segment(node, style, value, content_width, content_height, metrics, out);
}

/// Collects a `<select>` as a single inline `FormControl` fragment.
///
/// The label is the text of the last `<option>` carrying a `selected` attribute
/// (matching real non-`multiple` browser behavior), or the first `<option>`
/// otherwise, or empty when there are no options. Options with `display: none`
/// are excluded from both the label candidates and the width computation. The
/// width defaults to the widest option text plus 20px for the dropdown arrow (box
/// padding and border are added when the fragment is split). Explicit
/// `width`/`height` take precedence. `<option>` elements are not rendered on their
/// own.
fn collect_select_segment(
    node: &NodeHandle,
    style: &ComputedStyle,
    resolver: &mut StyleResolver,
    out: &mut Vec<InlineSegment>,
) {
    let metrics = font_metrics(style);
    let mut options = Vec::new();
    collect_option_entries(node, resolver, &mut options);

    let label = options
        .iter()
        .rev()
        .find(|(_, selected)| *selected)
        .or_else(|| options.first())
        .map(|(text, _)| text.clone())
        .unwrap_or_default();

    let content_width = explicit_length(style, "width").unwrap_or_else(|| {
        let widest = options
            .iter()
            .map(|(text, _)| measure_text_width(text, metrics))
            .fold(0.0f32, f32::max);
        widest + 20.0
    });
    let content_height =
        explicit_length(style, "height").unwrap_or_else(|| metrics.font_size.max(13.0));

    push_form_control_segment(node, style, label, content_width, content_height, metrics, out);
}

fn collect_media_placeholder_segment(
    node: &NodeHandle,
    style: &ComputedStyle,
    out: &mut Vec<InlineSegment>,
) {
    let metrics = font_metrics(style);
    let default_size = match node.tag_name().as_deref() {
        Some("audio") => (300.0, 54.0),
        Some("canvas") => (300.0, 150.0),
        _ => (300.0, 150.0),
    };
    let width = html_image_dimension_attribute(node, "width")
        .or_else(|| explicit_length(style, "width"))
        .unwrap_or(default_size.0);
    let height = html_image_dimension_attribute(node, "height")
        .or_else(|| explicit_length(style, "height"))
        .unwrap_or(default_size.1);
    push_form_control_segment(node, style, String::new(), width, height, metrics, out);
}

fn collect_value_indicator_segment(
    node: &NodeHandle,
    style: &ComputedStyle,
    out: &mut Vec<InlineSegment>,
) {
    let metrics = font_metrics(style);
    let content_width = explicit_length(style, "width").unwrap_or(160.0);
    let content_height = explicit_length(style, "height").unwrap_or(16.0);
    push_form_control_segment(
        node,
        style,
        String::new(),
        content_width,
        content_height,
        metrics,
        out,
    );
}

/// Pushes a `FormControl` inline segment shared by all form control collectors.
fn push_form_control_segment(
    node: &NodeHandle,
    style: &ComputedStyle,
    value: String,
    content_width: f32,
    content_height: f32,
    metrics: FontMetrics,
    out: &mut Vec<InlineSegment>,
) {
    let padding = edge_sizes(style, "padding");
    let border = edge_sizes(style, "border");
    let total_height =
        content_height + padding.top + padding.bottom + border.top + border.bottom;
    let editing = node.text_control_state().map(|state| TextControlPaintState {
        selection_start: state.selection_start,
        selection_end: state.selection_end,
        focused: state.focused,
    });

    out.push(InlineSegment {
        node: node.clone(),
        content: InlineSegmentContent::FormControl(
            style.clone(),
            value,
            editing,
            content_width,
            content_height,
        ),
        metrics,
        line_height: line_height(style).max(total_height),
        vertical_align: vertical_align(style),
        style: FragmentStyle::from_computed(style),
        word_break: word_break(style),
        overflow_wrap: overflow_wrap(style),
        white_space_mode: white_space(style),
    });
}

/// Concatenates the text of all rendered descendant text nodes.
///
/// Unlike a plain `textContent` walk, non-rendered elements (`<style>`,
/// `<script>`, etc.) and subtrees whose computed style is `display: none` are
/// skipped so that hidden text never leaks into form control labels.
fn collect_rendered_text(node: &NodeHandle, resolver: &mut StyleResolver) -> String {
    let mut text = String::new();
    for child in node.layout_child_nodes() {
        match child.node_type() {
            NodeType::Text => {
                if let Some(data) = child.data() {
                    text.push_str(&data);
                }
            }
            NodeType::Element => {
                if is_non_rendered_html_element(&child) {
                    continue;
                }
                let style = resolver.computed_style(&child);
                if is_display_none(&style) {
                    continue;
                }
                text.push_str(&collect_rendered_text(&child, resolver));
            }
            _ => {}
        }
    }
    text
}

/// Collapses runs of Unicode whitespace to a single space and trims the ends.
fn normalize_inline_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strips a single leading newline (`\n` or `\r\n`) from a textarea's initial
/// value, per the HTML spec. Later newlines are preserved.
fn strip_textarea_leading_newline(text: &str) -> &str {
    if let Some(rest) = text.strip_prefix("\r\n") {
        rest
    } else if let Some(rest) = text.strip_prefix('\n') {
        rest
    } else {
        text
    }
}

/// Recursively collects `(label, selected)` for each rendered `<option>`
/// descendant. Options (and container subtrees such as `<optgroup>`) that are
/// non-rendered or `display: none` are excluded entirely, so they contribute
/// neither a label candidate nor width.
fn collect_option_entries(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    out: &mut Vec<(String, bool)>,
) {
    for child in node.layout_child_nodes() {
        if child.node_type() != NodeType::Element {
            continue;
        }
        if is_non_rendered_html_element(&child) {
            continue;
        }
        let style = resolver.computed_style(&child);
        if is_display_none(&style) {
            continue;
        }
        if child.tag_name().as_deref() == Some("option") {
            let label = normalize_inline_whitespace(&collect_rendered_text(&child, resolver));
            let selected = child.get_attribute("selected").is_some();
            out.push((label, selected));
        } else {
            collect_option_entries(&child, resolver, out);
        }
    }
}

fn collect_image_segment(
    image_node: &NodeHandle,
    image: &Image,
    resolver: &mut StyleResolver,
    out: &mut Vec<InlineSegment>,
) {
    let image_style = resolver.computed_style(image_node);
    let padding = edge_sizes(&image_style, "padding");
    let border = edge_sizes(&image_style, "border");
    let (rendered_width, rendered_height) =
        resolve_image_rendered_size(image_node, image, &image_style);
    out.push(InlineSegment {
        node: image_node.clone(),
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
    element_inline_image_with_current_color(node, None)
}

fn element_inline_image_with_style(
    node: &NodeHandle,
    style: &ComputedStyle,
) -> Option<(NodeHandle, Image)> {
    let current_color = style.get("color").and_then(|value| match value {
        ComputedValue::Color(value) | ComputedValue::Keyword(value) => {
            crate::paint::color::parse_color(value)
        }
        _ => None,
    });
    element_inline_image_with_current_color(node, current_color)
}

fn element_inline_image_with_current_color(
    node: &NodeHandle,
    current_color: Option<crate::paint::color::Color>,
) -> Option<(NodeHandle, Image)> {
    let tag_name = node.tag_name()?;
    let attributes = node.attributes().unwrap_or_default();
    match tag_name.as_str() {
        "canvas" => crate::canvas::image(node.identity()).map(|image| (node.clone(), image)),
        "img" => {
            let src = attributes.get("src")?;
            decode_or_fetch_image(src).map(|image| (node.clone(), image))
        }
        "video" => {
            let poster = attributes.get("poster")?;
            decode_or_fetch_image(poster).map(|image| (node.clone(), image))
        }
        "picture" => node
            .layout_child_nodes()
            .into_iter()
            .find_map(|child| element_inline_image_with_current_color(&child, current_color)),
        "svg" => {
            let image = crate::svg::render_svg_to_image_with_current_color(
                node,
                current_color.unwrap_or(crate::paint::color::Color::rgb(0, 0, 0)),
            )?;
            Some((node.clone(), image))
        }
        "object" => {
            if let Some(data) = attributes.get("data")
                && let Some(image) = decode_or_fetch_image(data) {
                    return Some((node.clone(), image));
                }

            for child in node.layout_child_nodes() {
                if let Some(image) = element_inline_image_with_current_color(&child, current_color) {
                    return Some(image);
                }
            }

            None
        }
        _ => None,
    }
}

/// Decode an image from a data: URI (PNG, JPEG, GIF, WebP, or SVG).
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
            } else if mime_type.eq_ignore_ascii_case("image/gif") {
                let animation = Image::decode_gif_animation(&data).ok()?;
                let time = IMAGE_ANIMATION_TIME_MS.with(|cell| cell.get());
                Some(animation.frame_at(time).image().clone())
            } else if mime_type.eq_ignore_ascii_case("image/webp") {
                Image::decode_webp(&data).ok()
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
    let time = IMAGE_ANIMATION_TIME_MS.with(|cell| cell.get());
    if let Some(image) = IMAGE_ANIMATION_CACHE.with(|cache| {
        cache
            .borrow()
            .get(url)
            .map(|animation| animation.frame_at(time).image().clone())
    }) {
        return Some(image);
    }
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
    let mut request = HttpRequest::get(url).ok()?;
    request.set_header(
        "Accept",
        "image/webp,image/png,image/jpeg,image/gif,image/svg+xml;q=0.9,*/*;q=0.1",
    );
    let response = HTTP_CLIENT.with(|client| client.borrow_mut().send(request).ok())?;

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

    if content_type.contains("image/gif") || url.ends_with(".gif") || body.starts_with(b"GIF8") {
        let animation = Image::decode_gif_animation(body).ok()?;
        let time = IMAGE_ANIMATION_TIME_MS.with(|cell| cell.get());
        let image = animation.frame_at(time).image().clone();
        IMAGE_ANIMATION_CACHE.with(|cache| {
            cache.borrow_mut().insert(url.to_string(), animation);
        });
        return Some(image);
    }
    decode_image_bytes(body, &content_type, url)
}

/// Decodes image bytes, picking the decoder from the media type and falling back
/// to sniffing when it is absent or unrecognized.
///
/// `url` only contributes its file extension, which some servers make the only
/// hint available for SVG and GIF.
fn decode_image_bytes(bytes: &[u8], content_type: &str, url: &str) -> Option<Image> {
    if content_type.contains("image/svg+xml") || url.ends_with(".svg") {
        return decode_svg_bytes(bytes);
    }
    if content_type.contains("image/webp") || url.ends_with(".webp") {
        Image::decode_webp(bytes).ok()
    } else if content_type.contains("image/gif") || url.ends_with(".gif") {
        Image::decode_gif(bytes).ok()
    } else if content_type.contains("image/png") {
        Image::decode_png(bytes).ok()
    } else if content_type.contains("image/jpeg") || content_type.contains("image/jpg") {
        Image::decode_jpeg(bytes).ok()
    } else {
        // Sniff supported raster formats by their decoder before SVG text.
        Image::decode_webp(bytes)
            .ok()
            .or_else(|| Image::decode_png(bytes).ok())
            .or_else(|| Image::decode_jpeg(bytes).ok())
            .or_else(|| Image::decode_gif(bytes).ok())
            .or_else(|| decode_svg_bytes(bytes))
    }
}

/// Resolves a `blob:` URL minted by `URL.createObjectURL()` into an image.
///
/// The bytes come from the host blob URL store rather than the network, because
/// layout runs after the JavaScript runtime that created the URL is gone (see
/// [`crate::data`]).
///
/// Only a successful decode is cached. Nothing here is fetched, so a failure is
/// cheap to retry, and caching one would key a negative result on a URL whose
/// store entry can be replaced. Not caching failures also means revoking an
/// object URL cannot leave an entry that outlives a later registration.
///
/// A decoded image does stay cached after its URL is revoked, so an `<img>` that
/// already painted keeps painting — the same thing browsers do.
fn decode_blob_url_image(url: &str) -> Option<Image> {
    if let Some(cached) = IMAGE_CACHE.with(|cache| cache.borrow().get(url).cloned()) {
        return cached;
    }
    let entry = crate::data::lookup_blob_url(url)?;
    if entry.bytes.len() > MAX_IMAGE_SIZE {
        return None;
    }
    let image = decode_image_bytes(&entry.bytes, &entry.media_type.to_lowercase(), url)?;
    IMAGE_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .insert(url.to_string(), Some(image.clone()));
    });
    Some(image)
}

fn decode_or_fetch_image(url_like: &str) -> Option<Image> {
    let url_like = url_like.trim();
    if url_like.is_empty() {
        return None;
    }
    if url_like
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
    {
        return decode_data_uri_image(url_like);
    }
    // `get` rather than a range index: a source such as "日本語です" would make
    // byte 5 land inside a character and panic.
    if url_like
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("blob:"))
    {
        return decode_blob_url_image(url_like);
    }
    let resolved = resolve_image_url(url_like)?;
    fetch_image(&resolved)
}

pub(crate) fn decode_or_fetch_image_asset(url_like: &str) -> Option<Image> {
    decode_or_fetch_image(url_like)
}

pub(crate) fn canonical_image_asset_reference(url_like: &str) -> Option<String> {
    let url_like = url_like.trim();
    if url_like.is_empty() {
        return None;
    }
    if url_like
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
    {
        return Some(format!("data:{}", &url_like[5..]));
    }
    if url_like
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("blob:"))
    {
        return Some(format!("blob:{}", &url_like[5..]));
    }
    resolve_image_url(url_like)
}

fn resolve_image_url(url_like: &str) -> Option<String> {
    let is_http = url_like
        .split_once("://")
        .is_some_and(|(scheme, _)| {
            scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
        });
    if is_http {
        return url_like.parse::<Url>().ok().and_then(normalize_image_url);
    }
    if url_like.contains("://") || url_like.starts_with("//") {
        return None;
    }
    IMAGE_BASE_URL.with(|cell| {
        let base = cell.borrow().clone()?;
        resolve_url(&base, url_like).ok().and_then(normalize_image_url)
    })
}

fn normalize_image_url(url: Url) -> Option<String> {
    let normalized = resolve_url(&url, &url.request_target()).ok()?;
    Some(format!(
        "{}://{}{}",
        normalized.scheme(),
        normalized.authority().to_ascii_lowercase(),
        normalized.request_target(),
    ))
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

/// Returns the preferred aspect ratio (width / height) a replaced element should
/// size with, given its computed `aspect-ratio` and its intrinsic dimensions.
///
/// `aspect-ratio: auto` and the `auto <ratio>` form defer to the intrinsic ratio
/// whenever there is one; a bare `<ratio>` overrides it. A degenerate ratio (a
/// zero on either side) can size nothing, so it falls back to the intrinsic ratio
/// as well.
#[derive(Clone, Copy)]
struct PreferredAspectRatio {
    value: f32,
    uses_box_sizing: bool,
}

fn preferred_aspect_ratio(
    style: &ComputedStyle,
    intrinsic: Option<f32>,
) -> Option<PreferredAspectRatio> {
    let Some(ComputedValue::Keyword(value)) = style.get("aspect-ratio") else {
        return intrinsic.map(|value| PreferredAspectRatio {
            value,
            uses_box_sizing: false,
        });
    };
    let value = value.trim();
    if value.eq_ignore_ascii_case("auto") {
        return intrinsic.map(|value| PreferredAspectRatio {
            value,
            uses_box_sizing: false,
        });
    }
    let (prefers_intrinsic, ratio_text) = match value.strip_prefix("auto ") {
        Some(rest) => (true, rest),
        None => (false, value),
    };
    if prefers_intrinsic && intrinsic.is_some() {
        return intrinsic.map(|value| PreferredAspectRatio {
            value,
            uses_box_sizing: false,
        });
    }
    let (width, height) = ratio_text.split_once('/')?;
    let width = width.trim().parse::<f32>().ok()?;
    let height = height.trim().parse::<f32>().ok()?;
    if width > 0.0 && height > 0.0 {
        Some(PreferredAspectRatio {
            value: width / height,
            uses_box_sizing: true,
        })
    } else {
        intrinsic.map(|value| PreferredAspectRatio {
            value,
            uses_box_sizing: false,
        })
    }
}

pub(super) fn resolve_image_rendered_size(
    node: &NodeHandle,
    image: &Image,
    style: &ComputedStyle,
) -> (f32, f32) {
    let intrinsic_w = image.width() as f32;
    let intrinsic_h = image.height() as f32;
    let intrinsic_ratio = (intrinsic_w > 0.0 && intrinsic_h > 0.0)
        .then(|| intrinsic_w / intrinsic_h);
    let ratio = preferred_aspect_ratio(style, intrinsic_ratio);
    let padding = edge_sizes(style, "padding");
    let border = edge_sizes(style, "border");
    let horizontal_decoration = if is_border_box(style) {
        padding.horizontal() + border.horizontal()
    } else {
        0.0
    };
    let vertical_decoration = if is_border_box(style) {
        padding.vertical() + border.vertical()
    } else {
        0.0
    };
    let to_content_width = |value| {
        border_box_adjust_length(style, value, padding.left + border.left, padding.right + border.right)
    };
    let to_content_height = |value| {
        border_box_adjust_length(style, value, padding.top + border.top, padding.bottom + border.bottom)
    };
    let specified_width = explicit_length(style, "width")
        .or_else(|| html_image_dimension_attribute(node, "width"))
        .map(to_content_width);
    let specified_height = explicit_length(style, "height")
        .or_else(|| html_image_dimension_attribute(node, "height"))
        .map(to_content_height);

    let from_height = |height: f32| ratio.map_or(intrinsic_w, |ratio| {
        if ratio.uses_box_sizing {
            to_content_width((height + vertical_decoration) * ratio.value)
        } else {
            (height * ratio.value).max(0.0)
        }
    });
    let from_width = |width: f32| {
        ratio
            .filter(|ratio| ratio.value > 0.0)
            .map_or(intrinsic_h, |ratio| {
                if ratio.uses_box_sizing {
                    to_content_height((width + horizontal_decoration) / ratio.value)
                } else {
                    (width / ratio.value).max(0.0)
                }
            })
    };
    let (mut width, mut height) = match (specified_width, specified_height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, from_width(w)),
        (None, Some(h)) => (from_height(h), h),
        // Neither axis is specified: the intrinsic width anchors the box and the
        // preferred ratio gives the height, so an author ratio still applies.
        (None, None) => (intrinsic_w, from_width(intrinsic_w)),
    };

    // Constraint violations (CSS 2.1 §10.4) re-derive the other axis only when
    // that axis was not specified: a specified width keeps its value even when
    // max-height clamps the height it produced.
    let width_is_derived = specified_width.is_none();
    let height_is_derived = specified_height.is_none();
    if let Some(max_width) = explicit_length(style, "max-width").map(to_content_width)
        && width > max_width {
            if height_is_derived {
                height = from_width(max_width);
            }
            width = max_width;
        }
    if let Some(max_height) = explicit_length(style, "max-height").map(to_content_height)
        && height > max_height {
            if width_is_derived {
                width = from_height(max_height);
            }
            height = max_height;
        }
    if let Some(min_width) = explicit_length(style, "min-width").map(to_content_width)
        && width < min_width {
            if height_is_derived {
                height = from_width(min_width);
            }
            width = min_width;
        }
    if let Some(min_height) = explicit_length(style, "min-height").map(to_content_height)
        && height < min_height {
            if width_is_derived {
                width = from_height(min_height);
            }
            height = min_height;
        }

    (width, height)
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
    metrics.font_family = style.get("font-family").and_then(computed_font_family_key);
    metrics.font_weight = computed_font_weight(style);
    metrics.font_style = computed_font_property(style, "font-style")
        .map(FontStyle::parse)
        .unwrap_or_default();
    metrics
}

fn computed_font_weight(style: &ComputedStyle) -> FontWeight {
    match style.get("font-weight") {
        Some(ComputedValue::Number(value)) => FontWeight((*value as u16).clamp(1, 1000)),
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => {
            FontWeight::parse(value)
        }
        _ => FontWeight::default(),
    }
}

fn computed_font_property<'a>(style: &'a ComputedStyle, property: &str) -> Option<&'a str> {
    match style.get(property) {
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => Some(value),
        _ => None,
    }
}

fn computed_font_family_key(value: &ComputedValue) -> Option<FontFamilyKey> {
    let value = match value {
        ComputedValue::Keyword(value) | ComputedValue::String(value) => value,
        _ => return None,
    };
    let first = value.split(',').next()?.trim().trim_matches(['"', '\'']);
    (!first.is_empty()).then(|| FontFamilyKey::new(first))
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

/// Mutable cursor state threaded through inline layout.
struct InlineCursor {
    x: f32,
    y: f32,
    line_height: f32,
    start_x: f32,
    strut_line_height: f32,
    direction_rtl: bool,
}

impl InlineCursor {
    fn new(start_x: f32, start_y: f32, strut_line_height: f32, direction_rtl: bool) -> Self {
        Self {
            x: start_x,
            y: start_y,
            line_height: strut_line_height,
            start_x,
            strut_line_height,
            direction_rtl,
        }
    }

    fn wrap_line(
        &mut self,
        lines: &mut Vec<LineBox>,
        fragments: &mut Vec<InlineFragment>,
        segment_line_height: f32,
        available_width: f32,
        align: TextAlign,
    ) {
        let effective_height = self.line_height.max(segment_line_height);
        push_line(
            lines,
            fragments,
            self.start_x,
            self.y,
            self.x - self.start_x,
            effective_height,
            available_width,
            align,
            self.direction_rtl,
        );
        self.y += effective_height;
        self.x = self.start_x;
        self.line_height = self.strut_line_height;
    }
}

/// Determines whether a text fragment needs emergency character-by-character
/// breaking (overflow-wrap: break-word / anywhere).
fn needs_character_break(
    overflow_wrap: OverflowWrap,
    word_break: WordBreak,
    allows_wrapping: bool,
    cursor_x: f32,
    start_x: f32,
    fragment_width: f32,
    available_width: f32,
) -> bool {
    allows_wrapping
        && (matches!(overflow_wrap, OverflowWrap::BreakWord | OverflowWrap::Anywhere)
            || word_break == WordBreak::BreakWord)
        && cursor_x == start_x
        && exceeds_available_inline_width(fragment_width, available_width)
        && available_width > 0.0
}

/// Text measured as a whole can differ by a tiny fraction of a pixel from the
/// sum of separately shaped word fragments (notably around kerning pairs).
/// Do not create an extra line for that floating-point-only overflow.
pub(super) fn exceeds_available_inline_width(used: f32, available: f32) -> bool {
    const INLINE_WRAP_EPSILON_PX: f32 = 0.01;
    used > available + INLINE_WRAP_EPSILON_PX
}

/// Breaks a text fragment character by character, emitting fragments and
/// wrapping lines as needed. Returns the fragments produced.
fn break_text_by_characters(
    text: &str,
    segment: &InlineSegment,
    height: f32,
    cursor: &mut InlineCursor,
    lines: &mut Vec<LineBox>,
    fragments: &mut Vec<InlineFragment>,
    available_width: f32,
    align: TextAlign,
) {
    for ch_str in split_chars(text) {
        let ch_width = measure_text_width(&ch_str, segment.metrics);
        if cursor.x > cursor.start_x
            && exceeds_available_inline_width(
                cursor.x + ch_width - cursor.start_x,
                available_width,
            )
        {
            cursor.wrap_line(lines, fragments, 0.0, available_width, align);
        }
        fragments.push(InlineFragment {
            node: segment.node.clone(),
            content: InlineFragmentContent::Text(ch_str),
            rect: Rect {
                x: cursor.x,
                y: cursor.y,
                width: ch_width,
                height,
            },
            metrics: segment.metrics,
            vertical_align: segment.vertical_align,
            style: segment.style.clone(),
        });
        cursor.x += ch_width;
        cursor.line_height = cursor.line_height.max(segment.line_height.max(height));
    }
}

fn layout_inline_segments(
    segments: &[InlineSegment],
    start_x: f32,
    start_y: f32,
    available_width: f32,
    align: TextAlign,
    strut_line_height: f32,
    direction_rtl: bool,
) -> Vec<LineBox> {
    let mut lines = Vec::new();
    let mut current_fragments = Vec::new();
    let mut cursor = InlineCursor::new(start_x, start_y, strut_line_height, direction_rtl);

    let mut prev_segment_allows_wrapping = true;
    for segment in segments {
        let overflow_wrap = segment.overflow_wrap;
        let allows_wrapping = segment.white_space_mode.allows_wrapping();
        let mut is_first_piece_in_segment = true;
        for piece in split_segment(segment) {
            match piece {
                InlinePiece::Newline => {
                    cursor.wrap_line(
                        &mut lines,
                        &mut current_fragments,
                        segment.line_height,
                        available_width,
                        align,
                    );
                }
                InlinePiece::Fragment { content, width, height } => {
                    let collapsible_whitespace = segment.white_space_mode.collapses_whitespace()
                        && matches!(&content, InlineFragmentContent::Text(text) if text
                            .chars()
                            .all(|ch| ch != '\u{00A0}' && ch.is_whitespace()));
                    if cursor.x == start_x && collapsible_whitespace {
                        continue;
                    }
                    let can_wrap = if is_first_piece_in_segment {
                        prev_segment_allows_wrapping
                    } else {
                        allows_wrapping
                    };
                    is_first_piece_in_segment = false;

                    if can_wrap
                        && cursor.x > start_x
                        && exceeds_available_inline_width(
                            cursor.x + width - start_x,
                            available_width,
                        )
                    {
                        cursor.wrap_line(
                            &mut lines,
                            &mut current_fragments,
                            0.0,
                            available_width,
                            align,
                        );
                        if collapsible_whitespace {
                            continue;
                        }
                    }

                    if needs_character_break(
                        overflow_wrap,
                        segment.word_break,
                        allows_wrapping,
                        cursor.x,
                        start_x,
                        width,
                        available_width,
                    )
                        && let InlineFragmentContent::Text(text) = content {
                            break_text_by_characters(
                                &text,
                                segment,
                                height,
                                &mut cursor,
                                &mut lines,
                                &mut current_fragments,
                                available_width,
                                align,
                            );
                            continue;
                        }

                    current_fragments.push(InlineFragment {
                        node: segment.node.clone(),
                        content,
                        rect: Rect {
                            x: cursor.x,
                            y: cursor.y,
                            width,
                            height,
                        },
                        metrics: segment.metrics,
                        vertical_align: segment.vertical_align,
                        style: segment.style.clone(),
                    });
                    cursor.x += width;
                    cursor.line_height =
                        cursor.line_height.max(segment.line_height.max(height));
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
            cursor.y,
            cursor.x - start_x,
            cursor.line_height.max(0.0),
            available_width,
            align,
            direction_rtl,
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
        InlineSegmentContent::FormControl(style, value, editing, content_width, content_height) => {
            let padding = edge_sizes(style, "padding");
            let border = edge_sizes(style, "border");
            vec![InlinePiece::Fragment {
                content: InlineFragmentContent::FormControl(style.clone(), value.clone(), *editing),
                width: *content_width + padding.left + padding.right + border.left + border.right,
                height: *content_height + padding.top + padding.bottom + border.top + border.bottom,
            }]
        }
        InlineSegmentContent::IconFormControl(
            style,
            image,
            content_width,
            content_height,
            icon_width,
            icon_height,
        ) => {
            let padding = edge_sizes(style, "padding");
            let border = edge_sizes(style, "border");
            vec![InlinePiece::Fragment {
                content: InlineFragmentContent::IconFormControl(
                    style.clone(), image.clone(), *icon_width, *icon_height,
                ),
                width: *content_width + padding.left + padding.right + border.left + border.right,
                height: *content_height + padding.top + padding.bottom + border.top + border.bottom,
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

/// Split text into extended grapheme clusters for `word-break: break-all`.
/// Exported for tests.
/// Spaces remain as their own piece so that trailing-space collapsing still works.
pub(crate) fn split_chars(text: &str) -> Vec<String> {
    text.graphemes(true).map(str::to_string).collect()
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
    direction_rtl: bool,
) {
    let physical_align = match align {
        TextAlign::Start if direction_rtl => TextAlign::Right,
        TextAlign::Start => TextAlign::Left,
        TextAlign::End if direction_rtl => TextAlign::Left,
        TextAlign::End => TextAlign::Right,
        other => other,
    };
    let offset_x = match physical_align {
        TextAlign::Left => 0.0,
        TextAlign::Right => (available_width - width).max(0.0),
        TextAlign::Center => (available_width - width).max(0.0) / 2.0,
        TextAlign::Start | TextAlign::End => unreachable!("logical alignment must be resolved"),
    };
    for fragment in fragments.iter_mut() {
        fragment.rect.x += offset_x;
    }

    resolve_line_bidi_geometry(fragments, x + offset_x, direction_rtl);

    let baseline = fragments
        .iter()
        .filter_map(|fragment| match fragment.vertical_align {
            VerticalAlign::Baseline | VerticalAlign::Length(_) => Some(
                if matches!(
                    &fragment.content,
                    InlineFragmentContent::Image(_, _)
                        | InlineFragmentContent::FormControl(_, _, _)
                ) && fragment.rect.height >= height {
                    fragment.rect.height
                } else {
                    fragment.metrics.ascent
                },
            ),
            _ => None,
        })
        .fold(0.0f32, f32::max)
        .max(height * 0.8);

    for fragment in fragments.iter_mut() {
        fragment.rect.y = match fragment.vertical_align {
            VerticalAlign::Baseline => {
                let ascent = if matches!(
                    &fragment.content,
                    InlineFragmentContent::Image(_, _)
                        | InlineFragmentContent::FormControl(_, _, _)
                ) && fragment.rect.height >= height {
                    fragment.rect.height
                } else {
                    fragment.metrics.ascent
                };
                y + baseline - ascent
            },
            VerticalAlign::Length(shift) => {
                let ascent = if matches!(
                    &fragment.content,
                    InlineFragmentContent::Image(_, _)
                        | InlineFragmentContent::FormControl(_, _, _)
                ) && fragment.rect.height >= height {
                    fragment.rect.height
                } else {
                    fragment.metrics.ascent
                };
                y + baseline - ascent - shift
            },
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

/// Resolves adjacent inline fragments as one UAX#9 line while retaining the
/// fragment vector's logical DOM order.  Only physical origins are reordered;
/// paint consumes the resolved level to order scalars inside each fragment.
fn resolve_line_bidi_geometry(
    fragments: &mut [InlineFragment],
    line_start: f32,
    direction_rtl: bool,
) {
    if fragments.is_empty() {
        return;
    }
    let needs_bidi = direction_rtl || fragments.iter().any(|fragment| {
        let explicit_mode = !matches!(
            fragment.style.unicode_bidi.as_deref(),
            None | Some("normal")
        );
        explicit_mode
            || fragment.text().is_some_and(|text| {
                text.chars().any(|ch| {
                    matches!(
                        bidi_class(ch),
                        BidiClass::R
                            | BidiClass::AL
                            | BidiClass::AN
                            | BidiClass::RLE
                            | BidiClass::RLI
                            | BidiClass::RLO
                            | BidiClass::FSI
                    )
                })
            })
    });
    if !needs_bidi {
        return;
    }

    let mut bidi_source = String::new();
    let mut content_offsets = Vec::with_capacity(fragments.len());
    let mut ordering_offsets = Vec::with_capacity(fragments.len());
    for fragment in fragments.iter() {
        let (prefix, suffix) = bidi_controls(&fragment.style);
        let fragment_start = bidi_source.len();
        bidi_source.push_str(prefix);
        let start = bidi_source.len();
        match &fragment.content {
            InlineFragmentContent::Text(text) if !text.is_empty() => bidi_source.push_str(text),
            _ => bidi_source.push('\u{fffc}'),
        }
        let end = bidi_source.len();
        bidi_source.push_str(suffix);
        content_offsets.push(start..end);
        ordering_offsets.push(if prefix.is_empty() { start } else { fragment_start });
    }

    let paragraph_level = if direction_rtl { Level::rtl() } else { Level::ltr() };
    let bidi = BidiInfo::new(&bidi_source, Some(paragraph_level));
    let Some(paragraph) = bidi.paragraphs.first() else {
        return;
    };
    let (resolved_levels, _) = bidi.visual_runs(paragraph, paragraph.range.clone());
    let fragment_levels: Vec<Level> = ordering_offsets
        .iter()
        .map(|offset| resolved_levels.get(*offset).copied().unwrap_or(paragraph_level))
        .collect();
    let homogeneous_levels: Vec<Option<Level>> = content_offsets
        .iter()
        .map(|range| {
            let first = resolved_levels.get(range.start).copied()?;
            resolved_levels[range.clone()]
                .iter()
                .all(|level| *level == first)
                .then_some(first)
        })
        .collect();
    let visual_order = BidiInfo::reorder_visual(&fragment_levels);

    let mut cursor = line_start;
    for logical_index in visual_order {
        let fragment = &mut fragments[logical_index];
        fragment.rect.x = cursor;
        fragment.style.resolved_bidi_level =
            homogeneous_levels[logical_index].map(|level| level.number());
        cursor += fragment.rect.width;
    }
}

fn bidi_controls(style: &FragmentStyle) -> (&'static str, &'static str) {
    let rtl = style.direction.as_deref() == Some("rtl");
    match style.unicode_bidi.as_deref() {
        Some("embed") => (if rtl { "\u{202b}" } else { "\u{202a}" }, "\u{202c}"),
        Some("bidi-override") => (if rtl { "\u{202e}" } else { "\u{202d}" }, "\u{202c}"),
        Some("isolate") => (if rtl { "\u{2067}" } else { "\u{2066}" }, "\u{2069}"),
        Some("isolate-override") => {
            (if rtl { "\u{2067}\u{202e}" } else { "\u{2066}\u{202d}" }, "\u{202c}\u{2069}")
        },
        Some("plaintext") => ("\u{2068}", "\u{2069}"),
        _ => ("", ""),
    }
}

// ── Text measurement ────────────────────────────────────────────────────────

pub(super) fn measure_text_width(text: &str, metrics: FontMetrics) -> f32 {
    LAYOUT_FONTS.with(|cell| {
        let mut fonts_ref = cell.borrow_mut();
        if fonts_ref.is_none() {
            *fonts_ref = Some(super::LayoutFontContext {
                system_fonts: load_layout_fonts(),
                web_fonts: None,
            });
        }

        if let Some(ref context) = *fonts_ref {
            let primary = metrics.font_family.and_then(|family| {
                context.web_fonts.as_ref()?.select_best_by_key(
                    family,
                    metrics.font_weight,
                    metrics.font_style,
                )
            });
            if primary.is_some() || !context.system_fonts.is_empty() {
                let base = measure_text_width_with_fallback(
                    text,
                    metrics.font_size,
                    primary,
                    &context.system_fonts,
                );
                // Shaped paint inserts spacing between extended grapheme
                // clusters, not between the scalars that form one cluster.
                let cluster_count = text
                    .graphemes(true)
                    .filter(|cluster| {
                        cluster.chars().any(|ch| !is_zero_advance_character(ch))
                    })
                    .count();
                let spacing = if cluster_count > 1 {
                    metrics.letter_spacing * (cluster_count - 1) as f32
                } else {
                    0.0
                };
                return base + spacing;
            }
        }

        // Fallback to approximation when no font is available
        let char_count = text
            .chars()
            .filter(|ch| !is_zero_advance_character(*ch))
            .count();
        let base = char_count as f32 * metrics.average_advance;
        let spacing = if char_count > 1 {
            metrics.letter_spacing * (char_count - 1) as f32
        } else {
            0.0
        };
        base + spacing
    })
}

fn load_layout_fonts() -> Vec<Arc<Font>> {
    load_default_text_fonts().into_iter().map(Arc::new).collect()
}

fn measure_text_width_with_fallback(
    text: &str,
    font_size: f32,
    primary: Option<&Font>,
    fonts: &[Arc<Font>],
) -> f32 {
    let direction = if text.chars().any(|ch| {
        matches!(bidi_class(ch), BidiClass::R | BidiClass::AL | BidiClass::AN)
    }) {
        ShapingDirection::RightToLeft
    } else {
        ShapingDirection::LeftToRight
    };
    let mut run_fonts = Vec::with_capacity(fonts.len() + usize::from(primary.is_some()));
    if let Some(primary) = primary {
        run_fonts.push(primary);
    }
    run_fonts.extend(fonts.iter().map(Arc::as_ref));
    if !run_fonts.is_empty()
        && let Ok(runs) = shape_text_with_fallback(&run_fonts, text, font_size, direction)
    {
        return runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .map(|glyph| glyph.x_advance.abs())
            .sum();
    }

    if let Some(font) = select_layout_run_font(primary, fonts, text) {
        if let Ok(glyphs) = font.shape_text(text, font_size, direction) {
            return glyphs.iter().map(|glyph| glyph.x_advance.abs()).sum();
        }
    }

    let mut width = 0.0;
    let mut previous: Option<(char, *const Font)> = None;

    for ch in text.chars() {
        let Some(font) = select_layout_font(primary, fonts, ch) else {
            continue;
        };
        let font_id = std::ptr::from_ref(font);

        if !is_zero_advance_character(ch)
            && let Some((prev_char, prev_font)) = previous
            && prev_font == font_id
        {
            width += font.glyph_kerning(prev_char, ch, font_size);
        }

        if !is_zero_advance_character(ch) {
            let advance = font.glyph_advance(ch, font_size);
            width += if advance > 0.0 { advance } else { 0.0 };
            previous = Some((ch, font_id));
        }
    }

    width
}

fn select_layout_run_font<'a>(
    primary: Option<&'a Font>,
    fonts: &'a [Arc<Font>],
    text: &str,
) -> Option<&'a Font> {
    let supports_run = |font: &Font| {
        text.chars().all(|ch| {
            ch.is_whitespace() || is_zero_advance_character(ch) || font.has_glyph(ch)
        })
    };
    primary
        .filter(|font| supports_run(font))
        .or_else(|| fonts.iter().map(Arc::as_ref).find(|font| supports_run(font)))
}

fn select_layout_font<'a>(
    primary: Option<&'a Font>,
    fonts: &'a [Arc<Font>],
    ch: char,
) -> Option<&'a Font> {
    let prefer_cjk = is_cjk_preferred_character(ch);

    if prefer_cjk && fonts.len() > 1 {
        // Try CJK-capable fallback fonts first
        for font in fonts.iter().skip(1) {
            if font.has_glyph(ch) {
                return Some(font);
            }
        }
        return primary.or_else(|| fonts.first().map(AsRef::as_ref));
    }

    if let Some(font) = primary
        && (ch.is_whitespace() || font.has_glyph(ch))
    {
        return Some(font);
    }

    for font in fonts {
        if !ch.is_whitespace() && !font.has_glyph(ch) {
            continue;
        }
        return Some(font);
    }

    primary.or_else(|| fonts.first().map(AsRef::as_ref))
}

use crate::font::is_cjk_preferred_character;
