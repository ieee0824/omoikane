//! Inline layout: text segments, line breaking, and inline image handling.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use hypher::{Lang, hyphenate};
use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};
use unicode_segmentation::UnicodeSegmentation;

use crate::css::{
    ComputedStyle, ComputedValue, ComputedWritingMode, PseudoElement, StyleResolver, Value,
    evaluate_media_query, parse_media_query_list,
};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::font::{
    Font, FontFallbackCandidate, FontFamilyKey, FontStyle, FontVariantKey, FontWeight,
    ShapingDirection, WebFontCandidate, grapheme_spacing_boundaries, is_zero_advance_character,
    load_default_text_fonts_shared, select_text_font,
    shape_text_with_fallback_candidates_and_options,
};
use crate::http::cors::{self, CredentialsMode, PreflightCache, RedirectMode, RequestMode};
use crate::http::{Client, HttpRequest, HttpResponse, Url, url::resolve_url};
use crate::paint::{DataUri, Image, oriented_image_dimensions, parse_data_uri};

use super::{
    BoxDimensions, EdgeSizes, FontMetrics, FragmentStyle, HTTP_CLIENT, IMAGE_ANIMATION_CACHE,
    IMAGE_BASE_URL, IMAGE_CACHE, IMAGE_COOKIE_CONTEXT, InlineFragment, InlineFragmentContent,
    InlineTextSource, LAYOUT_FONTS, LayoutBox, LineBox, MeterRegion, Rect, TextControlPaintState,
    TextOverflowPaint, ValueIndicatorPaintState, VerticalAlign, border_box_adjust_length,
    edge_sizes, explicit_length, fragment_autospace_pair_applies, fragment_french_autospace_pair,
    is_border_box, is_non_rendered_html_element, text_is_combined_upright,
};

mod boxes;
#[cfg(test)]
mod data_image_tests;
mod text_source;

thread_local! {
    static PRINT_INLINE_SKIP: RefCell<Vec<(usize, usize)>> = const { RefCell::new(Vec::new()) };
    // Each active decode records only pending images it actually visits.
    static IMAGE_PENDING_DEPENDENCIES: RefCell<Vec<HashSet<String>>> = const { RefCell::new(Vec::new()) };
}

struct ImageDependencies;

impl ImageDependencies {
    fn begin() -> Self {
        IMAGE_PENDING_DEPENDENCIES.with(|stack| stack.borrow_mut().push(HashSet::new()));
        Self
    }

    fn has_pending(&self) -> bool {
        IMAGE_PENDING_DEPENDENCIES.with(|stack| !stack.borrow().last().unwrap().is_empty())
    }

    fn pending_references(&self) -> Vec<String> {
        IMAGE_PENDING_DEPENDENCIES
            .with(|stack| stack.borrow().last().unwrap().iter().cloned().collect())
    }
}

impl Drop for ImageDependencies {
    fn drop(&mut self) {
        IMAGE_PENDING_DEPENDENCIES.with(|stack| stack.borrow_mut().pop());
    }
}

/// Relayout inline formatting contexts after their already printed token prefixes.
pub(super) fn with_print_inline_skips<T>(
    owners: &[(NodeHandle, usize)],
    f: impl FnOnce() -> T,
) -> T {
    struct Restore(Vec<(usize, usize)>);

    impl Drop for Restore {
        fn drop(&mut self) {
            PRINT_INLINE_SKIP.with(|cell| *cell.borrow_mut() = std::mem::take(&mut self.0));
        }
    }

    let previous = PRINT_INLINE_SKIP.with(|cell| {
        std::mem::replace(
            &mut *cell.borrow_mut(),
            owners
                .iter()
                .map(|(owner, offset)| (owner.identity(), *offset))
                .collect(),
        )
    });
    let _restore = Restore(previous);
    f()
}

fn segment_token_units(segment: &InlineSegment) -> usize {
    match &segment.content {
        InlineSegmentContent::Text(text) => text.graphemes(true).count(),
        InlineSegmentContent::InlineEdge(_, _) => 0,
        _ => 1,
    }
}

fn skip_inline_prefix(segments: &mut Vec<InlineSegment>, token_offset: usize) {
    let mut remaining = token_offset;
    segments.retain_mut(|segment| {
        if remaining == 0 {
            return true;
        }
        let units = segment_token_units(segment);
        if units <= remaining {
            remaining -= units;
            return false;
        }
        if let InlineSegmentContent::Text(text) = &mut segment.content {
            let start = text
                .grapheme_indices(true)
                .nth(remaining)
                .map_or(text.len(), |(i, _)| i);
            segment.text_source = text_source::slice(&segment.text_source, text, start..text.len());
            *text = text[start..].to_owned();
            remaining = 0;
            return true;
        }
        false
    });
}

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
    JustifyInterWord,
    JustifyInterCharacter,
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
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("justify") => {
            match style.get("text-justify") {
                Some(ComputedValue::Keyword(method)) if method.eq_ignore_ascii_case("none") => {
                    TextAlign::Start
                }
                Some(ComputedValue::Keyword(method))
                    if method.eq_ignore_ascii_case("inter-character") =>
                {
                    TextAlign::JustifyInterCharacter
                }
                _ => TextAlign::JustifyInterWord,
            }
        }
        // `start` is the CSS initial value.  Keeping it logical means that a
        // block with `direction: rtl` naturally starts at its right edge even
        // when no explicit text-align declaration is present.
        _ => TextAlign::Start,
    }
}

fn text_align_last(
    style: Option<&ComputedStyle>,
    align: TextAlign,
    match_parent: Option<TextAlign>,
) -> TextAlign {
    let automatic = match align {
        TextAlign::JustifyInterWord | TextAlign::JustifyInterCharacter => TextAlign::Start,
        align => align,
    };
    let Some(ComputedValue::Keyword(keyword)) =
        style.and_then(|style| style.get("text-align-last"))
    else {
        return automatic;
    };
    match keyword.to_ascii_lowercase().as_str() {
        "start" => TextAlign::Start,
        "end" => TextAlign::End,
        "left" => TextAlign::Left,
        "right" => TextAlign::Right,
        "center" => TextAlign::Center,
        "justify" => match style.and_then(|style| style.get("text-justify")) {
            Some(ComputedValue::Keyword(method)) if method.eq_ignore_ascii_case("none") => {
                TextAlign::Start
            }
            Some(ComputedValue::Keyword(method))
                if method.eq_ignore_ascii_case("inter-character") =>
            {
                TextAlign::JustifyInterCharacter
            }
            _ => TextAlign::JustifyInterWord,
        },
        "match-parent" => match_parent.unwrap_or(TextAlign::Start),
        _ => automatic,
    }
}

fn resolved_text_align_last(
    owner: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    align: TextAlign,
) -> TextAlign {
    let parent_align = owner.parent_node().map(|parent| {
        let parent_style = resolver.computed_style(&parent);
        let align = text_align(&parent_style);
        resolved_text_align_last(&parent, resolver, &parent_style, align)
    });
    let resolved = text_align_last(Some(style), align, parent_align);
    match resolved {
        TextAlign::Start if super::direction_is_rtl(style) => TextAlign::Right,
        TextAlign::Start => TextAlign::Left,
        TextAlign::End if super::direction_is_rtl(style) => TextAlign::Left,
        TextAlign::End => TextAlign::Right,
        resolved => resolved,
    }
}

struct InitialLetterLayout {
    node: NodeHandle,
    text_source: Vec<InlineTextSource>,
    text: String,
    metrics: FontMetrics,
    style: FragmentStyle,
    width: f32,
    height: f32,
    sink: usize,
    size: f32,
    source_units: usize,
}

fn initial_letter_style(
    owner: &NodeHandle,
    resolver: &mut StyleResolver,
    container_style: &ComputedStyle,
) -> Option<(ComputedStyle, f32, usize)> {
    let pseudo = resolver.computed_pseudo_style(owner, PseudoElement::FirstLetter);
    let style = pseudo.as_ref().unwrap_or(container_style);
    let Some(ComputedValue::Keyword(value)) = style.get("initial-letter") else {
        return None;
    };
    if value.eq_ignore_ascii_case("normal") {
        return None;
    }
    let mut components = value.split_ascii_whitespace();
    let size = components.next()?.parse::<f32>().ok()?;
    let sink = match components.next() {
        None | Some("drop") => size.floor() as usize,
        Some("raise") => 1,
        Some(value) => value.parse::<usize>().ok()?,
    };
    (size >= 1.0 && sink >= 1).then(|| (style.clone(), size, sink))
}

fn take_initial_letter(
    segments: &mut Vec<InlineSegment>,
    style: &ComputedStyle,
    size: f32,
    sink: usize,
    strut_line_height: f32,
) -> Option<InitialLetterLayout> {
    let index = segments.iter().position(
        |segment| matches!(&segment.content, InlineSegmentContent::Text(text) if !text.trim().is_empty()),
    )?;
    let segment = &mut segments[index];
    let node = segment.node.clone();
    let InlineSegmentContent::Text(text) = &segment.content else {
        return None;
    };
    let mut start = None;
    let mut end = 0;
    let mut found_letter = false;
    for (offset, grapheme) in text.grapheme_indices(true) {
        let Some(ch) = grapheme.chars().next() else {
            continue;
        };
        if start.is_none() && ch.is_whitespace() {
            end = offset + grapheme.len();
            continue;
        }
        start.get_or_insert(offset);
        end = offset + grapheme.len();
        if !is_opening_hanging_punctuation(ch) {
            found_letter = true;
            break;
        }
    }
    let start = start?;
    if !found_letter {
        return None;
    }
    let initial_text = text[start..end].to_string();
    let source_units = text[..end].graphemes(true).count();
    let initial_source = text_source::slice(&segment.text_source, text, start..end);
    let remainder_source = text_source::slice(&segment.text_source, text, end..text.len());
    let remainder = text[end..].to_string();
    segment.text_source = remainder_source;
    segment.content = InlineSegmentContent::Text(remainder);
    if matches!(&segment.content, InlineSegmentContent::Text(text) if text.is_empty()) {
        segments.remove(index);
    }

    let mut metrics = font_metrics(style);
    let target_height = (size * strut_line_height).max(strut_line_height);
    let scale = target_height / (metrics.ascent + metrics.descent).max(0.01);
    metrics.font_size *= scale;
    metrics.ascent *= scale;
    metrics.descent *= scale;
    metrics.line_gap *= scale;
    metrics.average_advance *= scale;
    metrics.letter_spacing *= scale;
    metrics.word_spacing *= scale;
    let width = measure_text_width(&initial_text, &metrics).max(0.0);
    Some(InitialLetterLayout {
        node,
        text_source: initial_source,
        text: initial_text,
        metrics,
        style: FragmentStyle::from_computed(style),
        width,
        height: target_height,
        sink,
        size,
        source_units,
    })
}

fn attach_initial_letter(
    lines: &mut Vec<LineBox>,
    initial: InitialLetterLayout,
    start_x: f32,
    start_y: f32,
    available_width: f32,
    strut_line_height: f32,
    direction_rtl: bool,
) {
    if lines.is_empty() {
        lines.push(LineBox {
            rect: Rect {
                x: start_x,
                y: start_y,
                width: 0.0,
                height: strut_line_height,
            },
            baseline: start_y + strut_line_height * 0.8,
            fragments: Vec::new(),
            text_overflow: None,
        });
    }
    let line = &mut lines[0];
    let x = if direction_rtl {
        start_x + available_width - initial.width
    } else {
        start_x
    };
    let y = start_y + (initial.sink as f32 - initial.size) * strut_line_height;
    let mut style = initial.style;
    style.resolved_bidi_level = Some(if direction_rtl { 1 } else { 0 });
    line.fragments.insert(
        0,
        InlineFragment {
            node: initial.node,
            text_source: initial.text_source,
            content: InlineFragmentContent::Text(initial.text),
            rect: Rect {
                x,
                y,
                width: initial.width,
                height: initial.height,
            },
            autospace_after: 0.0,
            punctuation_trim: Default::default(),
            metrics: initial.metrics,
            vertical_align: VerticalAlign::Baseline,
            source_end_token: initial.source_units,
            style,
        },
    );
    let left = line.rect.x.min(x);
    let right = (line.rect.x + line.rect.width).max(x + initial.width);
    line.rect.x = left;
    line.rect.width = right - left;
}

// ── Inline layout entry point ───────────────────────────────────────────────

pub(super) struct InlineLayoutResult {
    pub(super) lines: Vec<LineBox>,
    pub(super) atomic_boxes: Vec<LayoutBox>,
}

#[derive(Clone, Copy)]
struct InlineLayoutContext {
    start_x: f32,
    start_y: f32,
    available_width: f32,
    containing_height: f32,
    viewport: super::LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    allow_atomic_boxes: bool,
}

pub(super) fn layout_inline_nodes(
    nodes: &[NodeHandle],
    resolver: &mut StyleResolver,
    start_x: f32,
    start_y: f32,
    available_width: f32,
    align: TextAlign,
    strut_line_height: f32,
    direction_rtl: bool,
    text_overflow_style: Option<&ComputedStyle>,
    containing_height: f32,
    viewport: super::LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    allow_atomic_boxes: bool,
) -> InlineLayoutResult {
    layout_inline_nodes_impl(
        nodes,
        resolver,
        start_x,
        start_y,
        available_width,
        align,
        strut_line_height,
        direction_rtl,
        text_overflow_style,
        containing_height,
        viewport,
        positioned_ancestor,
        allow_atomic_boxes,
        None,
        false,
        false,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn layout_inline_nodes_with_line_constraints(
    nodes: &[NodeHandle],
    resolver: &mut StyleResolver,
    start_x: f32,
    start_y: f32,
    available_width: f32,
    align: TextAlign,
    strut_line_height: f32,
    direction_rtl: bool,
    text_overflow_style: Option<&ComputedStyle>,
    containing_height: f32,
    viewport: super::LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    allow_atomic_boxes: bool,
    generated_owner: Option<&NodeHandle>,
    include_before: bool,
    include_after: bool,
    line_constraints: &dyn Fn(f32, f32) -> (f32, f32),
) -> InlineLayoutResult {
    layout_inline_nodes_impl(
        nodes,
        resolver,
        start_x,
        start_y,
        available_width,
        align,
        strut_line_height,
        direction_rtl,
        text_overflow_style,
        containing_height,
        viewport,
        positioned_ancestor,
        allow_atomic_boxes,
        generated_owner,
        include_before,
        include_after,
        Some(line_constraints),
    )
}

#[allow(clippy::too_many_arguments)]
fn layout_inline_nodes_impl(
    nodes: &[NodeHandle],
    resolver: &mut StyleResolver,
    start_x: f32,
    start_y: f32,
    available_width: f32,
    align: TextAlign,
    strut_line_height: f32,
    direction_rtl: bool,
    text_overflow_style: Option<&ComputedStyle>,
    containing_height: f32,
    viewport: super::LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    allow_atomic_boxes: bool,
    generated_owner: Option<&NodeHandle>,
    include_before: bool,
    include_after: bool,
    line_constraints: Option<&dyn Fn(f32, f32) -> (f32, f32)>,
) -> InlineLayoutResult {
    let context = InlineLayoutContext {
        start_x,
        start_y,
        available_width,
        containing_height,
        viewport,
        positioned_ancestor,
        allow_atomic_boxes,
    };
    let mut segments = Vec::new();
    let mut atomic_boxes = Vec::new();
    if include_before && let Some(owner) = generated_owner {
        segments.extend(generated_inline_segments(
            owner,
            resolver,
            PseudoElement::Before,
        ));
    }
    for node in nodes {
        let _phase = super::profile::Scope::begin(super::profile::Phase::InlineCollect);
        collect_inline_segments(node, resolver, &mut segments, context, &mut atomic_boxes);
    }
    if include_after && let Some(owner) = generated_owner {
        segments.extend(generated_inline_segments(
            owner,
            resolver,
            PseudoElement::After,
        ));
    }
    coalesce_adjacent_text_segments(&mut segments);
    let source_start_token = generated_owner
        .cloned()
        .or_else(|| nodes.first().and_then(NodeHandle::parent_node))
        .map(|owner| {
            PRINT_INLINE_SKIP.with(|cell| {
                cell.borrow()
                    .iter()
                    .find(|(id, _)| *id == owner.identity())
                    .map_or(0, |(_, offset)| *offset)
            })
        })
        .unwrap_or(0);
    if source_start_token > 0 {
        skip_inline_prefix(&mut segments, source_start_token);
    }
    let container_owner = generated_owner
        .cloned()
        .or_else(|| nodes.first().and_then(NodeHandle::parent_node));
    let container_style = container_owner
        .as_ref()
        .map(|container| resolver.computed_style(container));
    let mut initial_letter = (source_start_token == 0)
        .then(|| {
            container_owner
                .as_ref()
                .zip(container_style.as_ref())
                .and_then(|(owner, style)| initial_letter_style(owner, resolver, style))
                .and_then(|(style, size, sink)| {
                    take_initial_letter(&mut segments, &style, size, sink, strut_line_height)
                })
        })
        .flatten();
    let initial_source_units = initial_letter
        .as_ref()
        .map_or(0, |initial| initial.source_units);
    let strut_metrics = container_style.as_ref().map(font_metrics);
    let mut first_line_indent = container_style
        .as_ref()
        .map(|style| text_indent(style, available_width))
        .unwrap_or_default();
    first_line_indent.initial_affected = source_start_token == 0;
    let last_align = container_owner
        .as_ref()
        .zip(container_style.as_ref())
        .map_or(align, |(owner, style)| {
            resolved_text_align_last(owner, resolver, style, align)
        });

    let combined_constraints = |line_y: f32, line_height: f32| {
        let (mut x, mut width) = line_constraints
            .map(|constraints| constraints(line_y, line_height))
            .unwrap_or((start_x, available_width));
        if let Some(initial) = initial_letter.as_ref()
            && line_y < start_y + initial.sink as f32 * strut_line_height - 0.01
        {
            width = (width - initial.width).max(0.0);
            if !direction_rtl {
                x += initial.width;
            }
        }
        (x, width)
    };
    let effective_constraints = (line_constraints.is_some() || initial_letter.is_some())
        .then_some(&combined_constraints as &dyn Fn(f32, f32) -> (f32, f32));
    let (line_start_x, line_available_width) = effective_constraints
        .map(|constraints| constraints(start_y, strut_line_height))
        .unwrap_or((start_x, available_width));
    let mut lines = layout_inline_segments(
        &segments,
        source_start_token + initial_source_units,
        line_start_x,
        start_y,
        line_available_width,
        align,
        last_align,
        strut_line_height,
        strut_metrics.clone(),
        direction_rtl,
        first_line_indent,
        effective_constraints,
        None,
    );
    // Balancing against a changing float exclusion would need a two-dimensional
    // search. Keep those lines on the ordinary greedy path; the common no-float
    // constraint is constant and can safely be optimized below.
    let uniform_constraints = effective_constraints.is_none_or(|constraints| {
        lines.iter().all(|line| {
            let (constrained_x, constrained_width) = constraints(line.rect.y, strut_line_height);
            (constrained_x - line_start_x).abs() < 0.01
                && (constrained_width - line_available_width).abs() < 0.01
        })
    });
    if uniform_constraints {
        lines = optimize_text_wrap_style(
            &segments,
            source_start_token + initial_source_units,
            line_start_x,
            start_y,
            line_available_width,
            align,
            last_align,
            strut_line_height,
            strut_metrics,
            direction_rtl,
            first_line_indent,
            effective_constraints,
            lines,
        );
    }
    boxes::finish(&mut lines, nodes, resolver, strut_line_height);
    if let Some(initial) = initial_letter.take() {
        attach_initial_letter(
            &mut lines,
            initial,
            start_x,
            start_y,
            available_width,
            strut_line_height,
            direction_rtl,
        );
    }
    if let Some(style) = container_style.as_ref() {
        apply_hanging_punctuation(&mut lines, style, direction_rtl, available_width);
        apply_text_box_trim(&mut lines, style);
    }
    if let Some(style) = text_overflow_style {
        apply_text_overflow(
            &mut lines,
            nodes,
            start_x,
            available_width,
            direction_rtl,
            style,
        );
    }
    position_atomic_boxes(&lines, &mut atomic_boxes, resolver);
    InlineLayoutResult {
        lines,
        atomic_boxes,
    }
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
    text_overflow_style: Option<&ComputedStyle>,
    containing_width: f32,
    viewport: super::LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    generated_owner: Option<&NodeHandle>,
    include_before: bool,
    include_after: bool,
    column_constraints: Option<&dyn Fn(f32, f32) -> (f32, f32)>,
) -> InlineLayoutResult {
    let context = InlineLayoutContext {
        start_x,
        start_y,
        available_width: containing_width,
        containing_height: available_height,
        viewport,
        positioned_ancestor,
        allow_atomic_boxes: true,
    };
    let mut segments = Vec::new();
    let mut atomic_boxes = Vec::new();
    if include_before && let Some(owner) = generated_owner {
        segments.extend(generated_inline_segments(
            owner,
            resolver,
            PseudoElement::Before,
        ));
    }
    for node in nodes {
        let _phase = super::profile::Scope::begin(super::profile::Phase::InlineCollect);
        collect_inline_segments(node, resolver, &mut segments, context, &mut atomic_boxes);
    }
    if include_after && let Some(owner) = generated_owner {
        segments.extend(generated_inline_segments(
            owner,
            resolver,
            PseudoElement::After,
        ));
    }
    coalesce_adjacent_text_segments(&mut segments);

    // Horizontal layout's x axis is the vertical inline axis in this local
    // coordinate system.  A local origin of zero makes the transpose below
    // independent of the containing block's absolute position.
    let local_constraints = |column_offset: f32, column_width: f32| {
        let column_x = if vertical_rl {
            start_x + (available_width - column_offset - column_width).max(0.0)
        } else {
            start_x + column_offset
        };
        let (line_y, line_height) =
            column_constraints.expect("column constraints must be present")(column_x, column_width);
        ((line_y - start_y).max(0.0), line_height.max(0.0))
    };
    let local_constraints =
        column_constraints.map(|_| &local_constraints as &dyn Fn(f32, f32) -> (f32, f32));
    let layout_align = match (align, direction_rtl) {
        (TextAlign::Start, true) => TextAlign::Right,
        (TextAlign::End, true) => TextAlign::Left,
        (align, _) => align,
    };
    let container_owner = generated_owner
        .cloned()
        .or_else(|| nodes.first().and_then(NodeHandle::parent_node));
    let container_style = container_owner
        .as_ref()
        .map(|owner| resolver.computed_style(owner));
    let mut initial_letter = container_owner
        .as_ref()
        .zip(container_style.as_ref())
        .and_then(|(owner, style)| initial_letter_style(owner, resolver, style))
        .and_then(|(style, size, sink)| {
            take_initial_letter(&mut segments, &style, size, sink, strut_line_height)
        });
    let initial_source_units = initial_letter
        .as_ref()
        .map_or(0, |initial| initial.source_units);
    let combined_constraints = |line_y: f32, line_width: f32| {
        let (mut start, mut width) = local_constraints
            .map(|constraints| constraints(line_y, line_width))
            .unwrap_or((0.0, available_height.max(0.0)));
        if let Some(initial) = initial_letter.as_ref()
            && line_y < initial.sink as f32 * strut_line_height - 0.01
        {
            width = (width - initial.width).max(0.0);
            if !direction_rtl {
                start += initial.width;
            }
        }
        (start, width)
    };
    let effective_constraints = (local_constraints.is_some() || initial_letter.is_some())
        .then_some(&combined_constraints as &dyn Fn(f32, f32) -> (f32, f32));
    let (line_start, line_height) = effective_constraints
        .map(|constraints| constraints(0.0, strut_line_height))
        .unwrap_or((0.0, available_height.max(0.0)));
    let first_line_indent = container_style
        .as_ref()
        .map(|style| text_indent(style, available_height))
        .unwrap_or_default();
    let layout_last_align = container_owner
        .as_ref()
        .zip(container_style.as_ref())
        .map_or(layout_align, |(owner, style)| {
            resolved_text_align_last(owner, resolver, style, layout_align)
        });
    let mut horizontal_lines = layout_inline_segments(
        &segments,
        initial_source_units,
        line_start,
        0.0,
        line_height,
        layout_align,
        layout_last_align,
        strut_line_height,
        None,
        false,
        first_line_indent,
        effective_constraints,
        None,
    );

    boxes::finish(&mut horizontal_lines, nodes, resolver, strut_line_height);
    if let Some(initial) = initial_letter.take() {
        attach_initial_letter(
            &mut horizontal_lines,
            initial,
            0.0,
            0.0,
            available_height,
            strut_line_height,
            direction_rtl,
        );
    }
    if let Some(style) = container_style.as_ref() {
        apply_hanging_punctuation(
            &mut horizontal_lines,
            style,
            direction_rtl,
            available_height,
        );
        apply_text_box_trim(&mut horizontal_lines, style);
    }
    if let Some(style) = text_overflow_style {
        apply_text_overflow(
            &mut horizontal_lines,
            nodes,
            0.0,
            available_height.max(0.0),
            direction_rtl,
            style,
        );
    }
    let lines = horizontal_lines
        .into_iter()
        .map(|mut line| {
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

            let original_inline_extent = line.rect.width;
            let mut fragments = std::mem::take(&mut line.fragments);
            for fragment in &mut fragments {
                // The local fragment's y offset is vertical-align within its
                // line.  Its x offset is the inline-axis position and maps to
                // physical y.
                let inline_offset = fragment.rect.x - line.rect.x;
                let cross_offset = fragment.rect.y - line.rect.y;
                fragment.rect = Rect {
                    x: column_x + cross_offset,
                    y: if direction_rtl {
                        start_y
                            + line.rect.x
                            + (original_inline_extent - inline_offset - fragment.rect.width)
                                .max(0.0)
                    } else {
                        start_y + line.rect.x + inline_offset
                    },
                    width: fragment.rect.height,
                    height: fragment.rect.width,
                };
            }
            if let Some(overflow) = &mut line.text_overflow {
                for fragment in &mut overflow.fragments {
                    let inline_offset = fragment.rect.x - line.rect.x;
                    let cross_offset = fragment.rect.y - line.rect.y;
                    fragment.rect = Rect {
                        x: column_x + cross_offset,
                        y: if direction_rtl {
                            start_y
                                + line.rect.x
                                + (original_inline_extent - inline_offset - fragment.rect.width)
                                    .max(0.0)
                        } else {
                            start_y + line.rect.x + inline_offset
                        },
                        width: fragment.rect.height,
                        height: fragment.rect.width,
                    };
                }
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
                text_overflow: line.text_overflow,
            }
        })
        .collect::<Vec<_>>();
    position_atomic_boxes(&lines, &mut atomic_boxes, resolver);
    InlineLayoutResult {
        lines,
        atomic_boxes,
    }
}

fn apply_text_overflow(
    lines: &mut [LineBox],
    nodes: &[NodeHandle],
    start: f32,
    available_width: f32,
    direction_rtl: bool,
    style: &ComputedStyle,
) {
    let Some(marker_node) = nodes.first().cloned() else {
        return;
    };
    for line in lines {
        if !exceeds_available_inline_width(line.rect.width, available_width) {
            continue;
        }
        apply_overflow_marker(
            line,
            &marker_node,
            start,
            available_width,
            direction_rtl,
            style,
            "…",
            false,
        );
    }
}

fn apply_text_box_trim(lines: &mut [LineBox], style: &ComputedStyle) {
    let Some(ComputedValue::Keyword(trim)) = style.get("text-box-trim") else {
        return;
    };
    let (trim_start, trim_end) = match trim.to_ascii_lowercase().as_str() {
        "trim-start" => (true, false),
        "trim-end" => (false, true),
        "trim-both" => (true, true),
        _ => return,
    };
    let Some(first) = lines.first() else {
        return;
    };
    let metrics = font_metrics(style);
    let (over, under) = text_box_edges(style, metrics);
    let start_amount = if trim_start {
        (first.baseline - over - first.rect.y).clamp(0.0, first.rect.height)
    } else {
        0.0
    };
    if start_amount > 0.0 {
        for (index, line) in lines.iter_mut().enumerate() {
            if index == 0 {
                line.rect.height = (line.rect.height - start_amount).max(0.0);
            } else {
                line.rect.y -= start_amount;
            }
            line.baseline -= start_amount;
            for fragment in &mut line.fragments {
                fragment.rect.y -= start_amount;
            }
        }
    }
    if trim_end {
        let last = lines.last_mut().expect("non-empty line slice");
        let edge = last.baseline + under;
        let amount = (last.rect.y + last.rect.height - edge).clamp(0.0, last.rect.height);
        last.rect.height = (last.rect.height - amount).max(0.0);
    }
}

fn apply_hanging_punctuation(
    lines: &mut [LineBox],
    style: &ComputedStyle,
    direction_rtl: bool,
    available_width: f32,
) {
    let Some(ComputedValue::Keyword(value)) = style.get("hanging-punctuation") else {
        return;
    };
    let values = value
        .split_ascii_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<HashSet<_>>();
    if values.contains("first")
        && let Some(first) = lines.first_mut()
    {
        hang_punctuation(first, true, direction_rtl, is_opening_hanging_punctuation);
    }
    if values.contains("force-end") {
        for line in lines.iter_mut() {
            hang_punctuation(line, false, direction_rtl, is_closing_hanging_punctuation);
        }
    } else if values.contains("allow-end")
        && !matches!(style.get("text-align"), Some(ComputedValue::Keyword(align)) if align.eq_ignore_ascii_case("justify"))
    {
        for line in lines.iter_mut() {
            let forced_break = line
                .fragments
                .iter()
                .any(|fragment| fragment.rect.width == 0.0 && fragment.text() == Some(""));
            if forced_break || line.rect.width <= available_width + 0.01 {
                continue;
            }
            let original = line.clone();
            hang_punctuation(line, false, direction_rtl, is_closing_hanging_punctuation);
            if line.rect.width > available_width + 0.01
                || (line.rect.width - original.rect.width).abs() < 0.01
            {
                *line = original;
            }
        }
    } else if values.contains("last")
        && let Some(last) = lines.last_mut()
    {
        hang_punctuation(last, false, direction_rtl, is_closing_hanging_punctuation);
    }
}

fn hang_punctuation(
    line: &mut LineBox,
    inline_start: bool,
    direction_rtl: bool,
    eligible: fn(char) -> bool,
) {
    let Some((index, width)) = isolate_edge_grapheme(line, inline_start, direction_rtl, eligible)
    else {
        return;
    };
    if inline_start {
        if direction_rtl {
            for (candidate, fragment) in line.fragments.iter_mut().enumerate() {
                if candidate != index {
                    fragment.rect.x += width;
                }
            }
            line.fragments[index].rect.x += width;
            line.rect.x += width;
        } else {
            for (candidate, fragment) in line.fragments.iter_mut().enumerate() {
                if candidate != index {
                    fragment.rect.x -= width;
                }
            }
            line.fragments[index].rect.x -= width;
        }
    } else if direction_rtl {
        line.rect.x += width;
    }
    line.rect.width = (line.rect.width - width).max(0.0);
}

fn isolate_edge_grapheme(
    line: &mut LineBox,
    inline_start: bool,
    direction_rtl: bool,
    eligible: fn(char) -> bool,
) -> Option<(usize, f32)> {
    let index = line
        .fragments
        .iter()
        .enumerate()
        .filter(|(_, fragment)| {
            fragment.rect.width > 0.0 && fragment.text().is_some_and(|text| !text.is_empty())
        })
        .min_by(|(_, left), (_, right)| {
            let ordering = left.rect.x.total_cmp(&right.rect.x);
            if direction_rtl == inline_start {
                ordering.reverse()
            } else {
                ordering
            }
        })?
        .0;
    let fragment = line.fragments[index].clone();
    let text = fragment.text()?.to_string();
    let odd_level = fragment
        .style
        .resolved_bidi_level
        .is_some_and(|level| level % 2 == 1);
    let keep_prefix = if inline_start {
        direction_rtl == odd_level
    } else {
        direction_rtl != odd_level
    };
    let mut graphemes = text.grapheme_indices(true);
    let (start, grapheme) = if keep_prefix {
        graphemes.next()?
    } else {
        graphemes.next_back()?
    };
    if !grapheme.chars().next().is_some_and(eligible) {
        return None;
    }
    let width = measure_text_width(grapheme, &fragment.metrics).min(fragment.rect.width);
    if grapheme.len() == text.len() {
        return Some((index, width));
    }

    let end = start + grapheme.len();
    let mut punctuation = fragment.clone();
    punctuation.content = InlineFragmentContent::Text(grapheme.to_string());
    punctuation.text_source = text_source::slice(&fragment.text_source, &text, start..end);
    punctuation.rect.width = width;
    let punctuation_at_left = keep_prefix != odd_level;
    punctuation.rect.x = if punctuation_at_left {
        fragment.rect.x
    } else {
        fragment.rect.x + fragment.rect.width - width
    };

    let remainder_range = if keep_prefix {
        end..text.len()
    } else {
        0..start
    };
    let mut remainder = fragment;
    let remainder_text = text[remainder_range.clone()].to_string();
    remainder.text_source = text_source::slice(&remainder.text_source, &text, remainder_range);
    remainder.content = InlineFragmentContent::Text(remainder_text);
    remainder.rect.width = (remainder.rect.width - width).max(0.0);
    if punctuation_at_left {
        remainder.rect.x += width;
    }

    line.fragments.remove(index);
    if keep_prefix {
        line.fragments.insert(index, punctuation);
        line.fragments.insert(index + 1, remainder);
        Some((index, width))
    } else {
        line.fragments.insert(index, remainder);
        line.fragments.insert(index + 1, punctuation);
        Some((index + 1, width))
    }
}

fn is_opening_hanging_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '"' | '\'' | '(' | '[' | '{' | '\u{2018}' | '\u{201C}' | '\u{00AB}'
    ) || is_line_end_prohibited(ch, super::LineBreak::Normal)
}

fn is_closing_hanging_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '"' | '\''
            | ')'
            | ']'
            | '}'
            | ','
            | '.'
            | ':'
            | ';'
            | '!'
            | '?'
            | '\u{2019}'
            | '\u{201D}'
            | '\u{00BB}'
    ) || is_line_start_prohibited(ch, super::LineBreak::Normal)
}

fn text_box_edges(style: &ComputedStyle, metrics: FontMetrics) -> (f32, f32) {
    let edge = match style.get("text-box-edge") {
        Some(ComputedValue::Keyword(edge)) => edge.as_str(),
        _ => "auto",
    };
    let mut components = edge.split_ascii_whitespace();
    let over = components.next().unwrap_or("auto");
    let under = components.next().unwrap_or(over);
    let over = match over.to_ascii_lowercase().as_str() {
        "cap" => metrics.font_size * 0.7,
        "ex" => metrics.font_size * 0.5,
        _ => metrics.ascent,
    };
    let under = match under.to_ascii_lowercase().as_str() {
        "alphabetic" => 0.0,
        _ => metrics.descent,
    };
    (over, under)
}

pub(super) fn apply_block_ellipsis(
    line: &mut LineBox,
    marker_node: &NodeHandle,
    start: f32,
    available_width: f32,
    direction_rtl: bool,
    style: &ComputedStyle,
    marker: &str,
) {
    apply_overflow_marker(
        line,
        marker_node,
        start,
        available_width,
        direction_rtl,
        style,
        marker,
        matches!(
            style.writing_mode(),
            Some(
                ComputedWritingMode::VerticalRl
                    | ComputedWritingMode::VerticalLr
                    | ComputedWritingMode::SidewaysRl
                    | ComputedWritingMode::SidewaysLr
            )
        ),
    );
}

fn apply_overflow_marker(
    line: &mut LineBox,
    marker_node: &NodeHandle,
    start: f32,
    available_width: f32,
    direction_rtl: bool,
    style: &ComputedStyle,
    marker_text: &str,
    vertical: bool,
) {
    let available_width = available_width.max(0.0);
    let marker_metrics = font_metrics(style);
    let marker_width = measure_text_width(marker_text, &marker_metrics).max(0.0);
    let content_budget = (available_width - marker_width).max(0.0);
    let content_start = if direction_rtl {
        start + marker_width
    } else {
        start
    };
    let content_end = if direction_rtl {
        start + available_width
    } else {
        start + content_budget
    };
    let mut decorations = line
        .fragments
        .iter()
        .filter(|fragment| matches!(fragment.content, InlineFragmentContent::InlineBox(_)))
        .filter_map(|fragment| {
            let fragment_start = if vertical {
                fragment.rect.y
            } else {
                fragment.rect.x
            };
            let fragment_size = if vertical {
                fragment.rect.height
            } else {
                fragment.rect.width
            };
            let clipped_start = fragment_start.max(content_start);
            let clipped_end = (fragment_start + fragment_size).min(content_end);
            (clipped_end > clipped_start).then(|| {
                let mut clipped = fragment.clone();
                if vertical {
                    clipped.rect.y = clipped_start;
                    clipped.rect.height = clipped_end - clipped_start;
                } else {
                    clipped.rect.x = clipped_start;
                    clipped.rect.width = clipped_end - clipped_start;
                }
                clipped
            })
        })
        .collect::<Vec<_>>();
    let mut candidates = line
        .fragments
        .iter()
        .filter(|fragment| !matches!(fragment.content, InlineFragmentContent::InlineBox(_)))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        let left = if vertical { left.rect.y } else { left.rect.x };
        let right = if vertical { right.rect.y } else { right.rect.x };
        left.total_cmp(&right)
    });
    if direction_rtl {
        candidates.reverse();
    }
    let first_unit_width = candidates
        .iter()
        .find_map(|fragment| match &fragment.content {
            InlineFragmentContent::Text(text) => {
                let odd_level = fragment
                    .style
                    .resolved_bidi_level
                    .map_or(direction_rtl, |level| level % 2 == 1);
                let keep_prefix = direction_rtl == odd_level;
                let mut graphemes = text.graphemes(true);
                let grapheme = if keep_prefix {
                    graphemes.next()
                } else {
                    graphemes.next_back()
                }?;
                Some(measure_text_width(grapheme, &fragment.metrics))
            }
            _ => Some(
                if vertical {
                    fragment.rect.height
                } else {
                    fragment.rect.width
                }
                .max(0.0),
            ),
        });
    // The inline-start unit must be clipped rather than removed to make
    // room for an ellipsis. Leaving the original fragments in place lets
    // the container's overflow clip expose the fitting part of that unit.
    if first_unit_width.is_some_and(|width| exceeds_available_inline_width(width, content_budget)) {
        return;
    }

    let mut remaining = content_budget;
    let mut cursor = if direction_rtl {
        start + available_width
    } else {
        start
    };
    let mut visible = Vec::new();
    for fragment in candidates {
        let mut painted = fragment.clone();
        let width = if vertical {
            fragment.rect.height
        } else {
            fragment.rect.width
        }
        .max(0.0);
        if width > remaining + 0.01 {
            let InlineFragmentContent::Text(text) = &fragment.content else {
                break;
            };
            let odd_level = fragment
                .style
                .resolved_bidi_level
                .map_or(direction_rtl, |level| level % 2 == 1);
            let keep_prefix = direction_rtl == odd_level;
            let Some((text, width)) =
                text_fitting_graphemes(text, fragment.metrics.clone(), remaining, keep_prefix)
            else {
                break;
            };
            let original = match &fragment.content {
                InlineFragmentContent::Text(text) => text,
                _ => unreachable!(),
            };
            let start = if keep_prefix {
                0
            } else {
                original.len() - text.len()
            };
            painted.text_source =
                text_source::slice(&fragment.text_source, original, start..start + text.len());
            painted.content = InlineFragmentContent::Text(text);
            if vertical {
                painted.rect.height = width;
            } else {
                painted.rect.width = width;
            }
        }

        let painted_width = if vertical {
            painted.rect.height
        } else {
            painted.rect.width
        }
        .max(0.0);
        if direction_rtl {
            cursor -= painted_width;
            if vertical {
                painted.rect.y = cursor;
            } else {
                painted.rect.x = cursor;
            }
        } else {
            if vertical {
                painted.rect.y = cursor;
            } else {
                painted.rect.x = cursor;
            }
            cursor += painted_width;
        }
        remaining = (remaining - painted_width).max(0.0);
        visible.push(painted);
        if width > remaining + painted_width + 0.01 {
            break;
        }
    }

    let mut marker_style = FragmentStyle::from_computed(style);
    marker_style.resolved_bidi_level = Some(if direction_rtl { 1 } else { 0 });
    let marker = InlineFragment {
        node: marker_node.clone(),
        text_source: Vec::new(),
        content: InlineFragmentContent::Text(marker_text.to_string()),
        rect: if vertical {
            Rect {
                x: line
                    .fragments
                    .first()
                    .map_or(line.rect.x, |fragment| fragment.rect.x),
                y: if direction_rtl {
                    start
                } else {
                    start + (available_width - marker_width).max(0.0)
                },
                width: marker_metrics.ascent + marker_metrics.descent,
                height: marker_width,
            }
        } else {
            Rect {
                x: if direction_rtl {
                    start
                } else {
                    start + (available_width - marker_width).max(0.0)
                },
                y: line.baseline - marker_metrics.ascent,
                width: marker_width,
                height: marker_metrics.ascent + marker_metrics.descent,
            }
        },
        autospace_after: 0.0,
        punctuation_trim: Default::default(),
        metrics: marker_metrics,
        vertical_align: VerticalAlign::Baseline,
        source_end_token: 0,
        style: marker_style,
    };
    visible.sort_by(|left, right| {
        let left = if vertical { left.rect.y } else { left.rect.x };
        let right = if vertical { right.rect.y } else { right.rect.x };
        left.total_cmp(&right)
    });
    decorations.extend(visible);
    decorations.push(marker);
    line.text_overflow = Some(TextOverflowPaint {
        fragments: decorations,
    });
}

fn text_fitting_graphemes(
    text: &str,
    metrics: FontMetrics,
    max_width: f32,
    keep_prefix: bool,
) -> Option<(String, f32)> {
    if max_width <= 0.0 {
        return None;
    }
    let mut boundaries = text
        .grapheme_indices(true)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    boundaries.push(text.len());
    let grapheme_count = boundaries.len().saturating_sub(1);
    let mut low = 0;
    let mut high = grapheme_count + 1;
    while low + 1 < high {
        let count = (low + high) / 2;
        let candidate = if keep_prefix {
            &text[..boundaries[count]]
        } else {
            &text[boundaries[grapheme_count - count]..]
        };
        if measure_text_width(candidate, &metrics) <= max_width + 0.01 {
            low = count;
        } else {
            high = count;
        }
    }
    if low == 0 {
        return None;
    }
    let fitted = if keep_prefix {
        &text[..boundaries[low]]
    } else {
        &text[boundaries[grapheme_count - low]..]
    };
    Some((fitted.to_string(), measure_text_width(fitted, &metrics)))
}

fn position_atomic_boxes(
    lines: &[LineBox],
    atomic_boxes: &mut [LayoutBox],
    resolver: &mut StyleResolver,
) {
    for fragment in lines.iter().flat_map(|line| &line.fragments) {
        if matches!(fragment.content, InlineFragmentContent::AtomicInline(_))
            && let Some(layout) = atomic_boxes
                .iter_mut()
                .find(|layout| layout.node == fragment.node)
        {
            super::translate_layout_box_to_outer(
                layout,
                fragment.rect.x,
                fragment.rect.y,
                resolver,
            );
        }
    }
}

fn atomic_inline_baseline(layout: &LayoutBox) -> f32 {
    fn last_line_baseline(layout: &LayoutBox) -> Option<f32> {
        layout
            .lines
            .iter()
            .map(|line| line.baseline)
            .chain(layout.children.iter().filter_map(last_line_baseline))
            .max_by(f32::total_cmp)
    }

    let fallback = layout.total_height();
    if layout.overflow != super::Overflow::Visible {
        return fallback;
    }
    let outer_y = layout.dimensions.content.y
        - layout.dimensions.padding.top
        - layout.dimensions.border.top
        - layout.dimensions.margin.top;
    last_line_baseline(layout)
        .map(|baseline| (baseline - outer_y).clamp(0.0, fallback))
        .unwrap_or(fallback)
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
            && let (InlineSegmentContent::Text(previous_text), InlineSegmentContent::Text(text)) =
                (&mut previous.content, &segment.content)
        {
            text_source::append(
                &mut previous.text_source,
                segment.text_source,
                previous_text.len(),
            );
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
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("break-all") => {
            WordBreak::BreakAll
        }
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("keep-all") => {
            WordBreak::KeepAll
        }
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("break-word") => {
            WordBreak::BreakWord
        }
        _ => WordBreak::Normal,
    }
}

pub(super) fn overflow_wrap(style: &ComputedStyle) -> OverflowWrap {
    match style.get("overflow-wrap") {
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("break-word") => {
            OverflowWrap::BreakWord
        }
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("anywhere") => {
            OverflowWrap::Anywhere
        }
        _ => OverflowWrap::Normal,
    }
}

fn inherited_hyphenation_language(node: &NodeHandle) -> Option<Lang> {
    let mut current = Some(node.clone());
    while let Some(node) = current {
        if let Some(language) = node.get_attribute("lang") {
            let primary = language.split('-').next().unwrap_or_default();
            let bytes = primary.as_bytes();
            if bytes.len() == 2 && bytes.iter().all(u8::is_ascii_alphabetic) {
                return Lang::from_iso([
                    bytes[0].to_ascii_lowercase(),
                    bytes[1].to_ascii_lowercase(),
                ]);
            }
            return None;
        }
        current = node.parent_node();
    }
    None
}

fn inherited_content_language_is_french(node: &NodeHandle) -> bool {
    let mut current = Some(node.clone());
    while let Some(node) = current {
        if let Some(language) = node
            .get_attribute("lang")
            .or_else(|| node.get_attribute("xml:lang"))
        {
            return language
                .split(['-', '_'])
                .next()
                .is_some_and(|primary| primary.eq_ignore_ascii_case("fr"));
        }
        current = node.parent_node();
    }
    false
}

fn hyphenation_opportunities(
    text: &str,
    mode: super::Hyphens,
    language: Option<Lang>,
) -> Vec<usize> {
    if mode == super::Hyphens::None {
        return Vec::new();
    }
    let mut opportunities = text
        .char_indices()
        .filter_map(|(offset, ch)| (ch == '\u{00ad}').then_some(offset + ch.len_utf8()))
        .collect::<Vec<_>>();
    if mode == super::Hyphens::Auto
        && let Some(language) = language
        && !text.contains('\u{00ad}')
    {
        let mut offset = 0;
        let mut syllables = hyphenate(text, language).peekable();
        while let Some(syllable) = syllables.next() {
            offset += syllable.len();
            if syllables.peek().is_some() {
                opportunities.push(offset);
            }
        }
    }
    opportunities.sort_unstable();
    opportunities.dedup();
    opportunities
}

#[derive(Debug, Clone)]
pub(super) struct InlineSegment {
    pub(super) node: NodeHandle,
    pub(super) text_source: Vec<InlineTextSource>,
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
    InlineEdge(ComputedStyle, bool),
    AtomicInline(f32, f32, f32),
    Image(Image, ComputedStyle, f32, f32),
    GeneratedBox(ComputedStyle),
    FormControl(
        ComputedStyle,
        String,
        Option<TextControlPaintState>,
        f32,
        f32,
    ),
    ValueIndicator(ComputedStyle, ValueIndicatorPaintState, f32, f32),
    IconFormControl(ComputedStyle, Image, f32, f32, f32, f32),
}

// ── Inline segment collection ───────────────────────────────────────────────

/// Creates a text `InlineSegment` from a node, text content, and resolved style.
/// Returns `None` when the normalized + transformed text is empty.
fn make_text_segment(node: NodeHandle, text: &str, style: &ComputedStyle) -> Option<InlineSegment> {
    let (text, text_source) = text_source::prepare(&node, text, style);
    if text.is_empty() {
        return None;
    }
    let mut fragment_style = FragmentStyle::from_computed(style);
    fragment_style.french_content_language = inherited_content_language_is_french(&node);
    Some(InlineSegment {
        node,
        text_source,
        content: InlineSegmentContent::Text(text),
        metrics: font_metrics_with_fragment_style(style, &fragment_style),
        line_height: line_height(style),
        vertical_align: vertical_align(style),
        style: fragment_style,
        word_break: word_break(style),
        overflow_wrap: overflow_wrap(style),
        white_space_mode: white_space(style),
    })
}

fn collect_inline_segments(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    out: &mut Vec<InlineSegment>,
    context: InlineLayoutContext,
    atomic_boxes: &mut Vec<LayoutBox>,
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
            collect_element_inline_segments(node, resolver, out, context, atomic_boxes);
        }
        _ => {}
    }
}

fn collect_element_inline_segments(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    out: &mut Vec<InlineSegment>,
    context: InlineLayoutContext,
    atomic_boxes: &mut Vec<LayoutBox>,
) {
    if is_non_rendered_html_element(node) {
        return;
    }
    let style = resolver.computed_style(node);
    if style.is_display_none() {
        return;
    }

    match node.tag_name().as_deref() {
        Some("br") => {
            // An explicit HTML line break is independent of white-space's
            // handling of literal newlines. Reuse the forced-break path.
            out.push(InlineSegment {
                node: node.clone(),
                text_source: Vec::new(),
                content: InlineSegmentContent::Text("\n".to_string()),
                metrics: font_metrics(&style),
                line_height: line_height(&style),
                vertical_align: vertical_align(&style),
                style: FragmentStyle::from_computed(&style),
                word_break: word_break(&style),
                overflow_wrap: overflow_wrap(&style),
                white_space_mode: WhiteSpaceMode::pre_line(),
            });
            return;
        }
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
        Some("ruby") if context.allow_atomic_boxes => {
            if let Some(layout) = layout_ruby_inline(node, resolver, &style, context) {
                let width = layout.total_width();
                let height = layout.total_height();
                let baseline = atomic_inline_baseline(&layout);
                atomic_boxes.push(layout);
                out.push(InlineSegment {
                    node: node.clone(),
                    text_source: Vec::new(),
                    content: InlineSegmentContent::AtomicInline(width, height, baseline),
                    metrics: font_metrics(&style),
                    line_height: height,
                    vertical_align: vertical_align(&style),
                    style: FragmentStyle::from_computed(&style),
                    word_break: word_break(&style),
                    overflow_wrap: overflow_wrap(&style),
                    white_space_mode: white_space(&style),
                });
                return;
            }
        }
        Some("audio" | "canvas" | "video") if element_inline_image(node).is_none() => {
            collect_media_placeholder_segment(node, &style, out);
            return;
        }
        _ => {}
    }

    out.extend(generated_inline_segments(
        node,
        resolver,
        PseudoElement::Before,
    ));

    if let Some((image_node, image)) = element_inline_image_with_style(node, &style) {
        collect_image_segment(&image_node, &image, resolver, out);
        out.extend(generated_inline_segments(
            node,
            resolver,
            PseudoElement::After,
        ));
        return;
    }

    if node.has_tag_name("img")
        && let Some(alt_text) = image_alt_fallback_text(node, &style)
    {
        out.push(InlineSegment {
            node: node.clone(),
            text_source: Vec::new(),
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

    let inline_frame = node.has_tag_name("iframe")
        && matches!(style.get("display"), Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("inline"));
    if context.allow_atomic_boxes
        && (inline_frame
            || matches!(style.get("display"), Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("inline-block")))
    {
        let available_width = context.available_width.max(0.0);
        // An inline iframe still needs a layout box for its child viewport.
        let containing_width =
            if inline_frame || super::resolved_length(&style, "width", available_width).is_some() {
                available_width
            } else {
                super::shrink_to_fit_layout_width(node, resolver, available_width)
            };
        let containing = Rect {
            x: context.start_x,
            y: context.start_y,
            width: containing_width,
            height: context.containing_height,
        };
        if let Some(mut layout) = super::layout_element(
            node,
            resolver,
            containing,
            context.viewport,
            context.positioned_ancestor,
            None,
            None,
        ) {
            if super::margin_start_is_auto(&style) {
                let auto_margin = layout.dimensions.margin.left;
                layout.dimensions.margin.left = 0.0;
                super::translate_layout_box(&mut layout, -auto_margin, 0.0, resolver);
            }
            if super::margin_end_is_auto(&style) {
                layout.dimensions.margin.right = 0.0;
            }
            let width = layout.total_width();
            let height = layout.total_height();
            let baseline = atomic_inline_baseline(&layout);
            atomic_boxes.push(layout);
            out.push(InlineSegment {
                node: node.clone(),
                text_source: Vec::new(),
                content: InlineSegmentContent::AtomicInline(width, height, baseline),
                metrics: font_metrics(&style),
                line_height: height,
                vertical_align: vertical_align(&style),
                style: FragmentStyle::from_computed(&style),
                word_break: word_break(&style),
                overflow_wrap: overflow_wrap(&style),
                white_space_mode: white_space(&style),
            });
        }
        return;
    }

    let inline_box = super::is_inline_child(node, resolver);
    if inline_box {
        boxes::edge(node, &style, true, out);
    }

    for child in node.layout_child_nodes() {
        match child.node_type() {
            NodeType::Text => {
                if let Some(text) = child.data() {
                    out.extend(make_text_segment(child, &text, &style));
                }
            }
            NodeType::Element => {
                collect_inline_segments(&child, resolver, out, context, atomic_boxes);
            }
            _ => {}
        }
    }
    out.extend(generated_inline_segments(
        node,
        resolver,
        PseudoElement::After,
    ));
    if inline_box {
        boxes::edge(node, &style, false, out);
    }
}

struct RubyAnnotationLevel {
    nodes: Vec<NodeHandle>,
    position_owner: Option<NodeHandle>,
}

struct RubyLaidOutLevel {
    layout: InlineLayoutResult,
    cross: f32,
    under: bool,
}

#[derive(Clone, Copy)]
enum RubyAlign {
    Start,
    Center,
    SpaceBetween,
    SpaceAround,
}

impl RubyAlign {
    fn from_style(style: &ComputedStyle) -> Self {
        match style.get("ruby-align") {
            Some(ComputedValue::Keyword(value)) if value == "start" => Self::Start,
            Some(ComputedValue::Keyword(value)) if value == "center" => Self::Center,
            Some(ComputedValue::Keyword(value)) if value == "space-between" => Self::SpaceBetween,
            _ => Self::SpaceAround,
        }
    }

    fn text_align(self) -> TextAlign {
        match self {
            Self::Start | Self::SpaceBetween | Self::SpaceAround => TextAlign::Left,
            Self::Center => TextAlign::Center,
        }
    }

    fn finish(self, lines: &mut [LineBox], inline_extent: f32) {
        if !matches!(self, Self::SpaceBetween | Self::SpaceAround) {
            return;
        }
        for line in lines {
            let graphemes = line
                .fragments
                .iter()
                .filter_map(InlineFragment::text)
                .map(|text| text.graphemes(true).count())
                .sum::<usize>();
            let used = line.rect.width;
            let extra = (inline_extent - used).max(0.0);
            if graphemes > 1 {
                let internal = if matches!(self, Self::SpaceBetween) {
                    extra
                } else {
                    extra * (graphemes - 1) as f32 / graphemes as f32
                };
                justify_inter_character(&mut line.fragments, used, used + internal);
            }
            let edge = if matches!(self, Self::SpaceBetween) {
                0.0
            } else if graphemes == 0 {
                extra * 0.5
            } else {
                extra / (2 * graphemes) as f32
            };
            for fragment in &mut line.fragments {
                fragment.rect.x += edge;
            }
            line.rect.width = inline_extent;
        }
    }
}

fn ruby_annotation_levels(children: &[NodeHandle]) -> Vec<RubyAnnotationLevel> {
    let mut levels = Vec::new();
    let mut direct_rt = Vec::new();
    for child in children {
        match child.with_tag_name(|tag| tag.map(str::to_owned)) {
            Some(tag) if tag == "rt" => direct_rt.push(child.clone()),
            Some(tag) if tag == "rtc" => {
                if !direct_rt.is_empty() {
                    levels.push(RubyAnnotationLevel {
                        nodes: std::mem::take(&mut direct_rt),
                        position_owner: None,
                    });
                }
                levels.push(RubyAnnotationLevel {
                    nodes: child.layout_child_nodes(),
                    position_owner: Some(child.clone()),
                });
            }
            _ => {}
        }
    }
    if !direct_rt.is_empty() {
        levels.push(RubyAnnotationLevel {
            nodes: direct_rt,
            position_owner: None,
        });
    }
    levels.retain(|level| !level.nodes.is_empty());
    levels
}

fn ruby_annotation_strut(
    level: &RubyAnnotationLevel,
    resolver: &mut StyleResolver,
    fallback: f32,
) -> f32 {
    level
        .nodes
        .first()
        .map(|annotation| resolver.computed_style(annotation))
        .map(|style| font_metrics(&style).font_size * 1.2)
        .unwrap_or(fallback * 0.5)
}

fn ruby_level_cross_extent(level: &InlineLayoutResult) -> f32 {
    level
        .lines
        .iter()
        .map(|line| line.rect.height)
        .fold(0.0_f32, f32::max)
}

fn layout_ruby_level(
    nodes: &[NodeHandle],
    resolver: &mut StyleResolver,
    width: f32,
    align: TextAlign,
    strut: f32,
    direction_rtl: bool,
    context: InlineLayoutContext,
) -> InlineLayoutResult {
    layout_inline_nodes_impl(
        nodes,
        resolver,
        0.0,
        0.0,
        width,
        align,
        strut,
        direction_rtl,
        None,
        context.containing_height,
        context.viewport,
        context.positioned_ancestor,
        true,
        None,
        false,
        false,
        None,
    )
}

fn translate_ruby_lines(lines: &mut [LineBox], dx: f32, dy: f32) {
    for line in lines {
        line.rect.x += dx;
        line.rect.y += dy;
        line.baseline += dy;
        for fragment in &mut line.fragments {
            fragment.rect.x += dx;
            fragment.rect.y += dy;
        }
        if let Some(overflow) = &mut line.text_overflow {
            for fragment in &mut overflow.fragments {
                fragment.rect.x += dx;
                fragment.rect.y += dy;
            }
        }
    }
}

fn transpose_ruby_lines(lines: &mut [LineBox], cross: f32) {
    for line in lines {
        let old = line.rect;
        let block_origin = line
            .fragments
            .iter()
            .filter(|fragment| fragment.text().is_some())
            .map(|fragment| fragment.rect.y - old.y)
            .reduce(f32::min)
            .unwrap_or(0.0);
        for fragment in &mut line.fragments {
            let inline = fragment.rect.x - old.x;
            let block = fragment.rect.y - old.y - block_origin;
            fragment.rect = Rect {
                x: cross + block,
                y: inline,
                width: fragment.rect.height,
                height: fragment.rect.width,
            };
        }
        line.rect = Rect {
            x: cross,
            y: 0.0,
            width: old.height,
            height: old.width,
        };
    }
}

fn position_ruby_levels(
    style: &ComputedStyle,
    base: &mut InlineLayoutResult,
    annotations: &mut [RubyLaidOutLevel],
    inline_extent: f32,
    base_cross: f32,
) -> (f32, f32) {
    let over_cross = annotations
        .iter()
        .filter(|level| !level.under)
        .map(|level| level.cross)
        .sum::<f32>();
    let under_cross = annotations
        .iter()
        .filter(|level| level.under)
        .map(|level| level.cross)
        .sum::<f32>();
    if !super::is_vertical_writing(style) {
        translate_ruby_lines(&mut base.lines, 0.0, over_cross);
        let mut over_cursor = over_cross;
        let mut under_cursor = over_cross + base_cross;
        for level in annotations {
            let y = if level.under {
                let y = under_cursor;
                under_cursor += level.cross;
                y
            } else {
                over_cursor -= level.cross;
                over_cursor
            };
            translate_ruby_lines(&mut level.layout.lines, 0.0, y);
        }
        return (inline_extent, over_cross + base_cross + under_cross);
    }

    let vertical_rl = matches!(
        style.get("writing-mode"),
        Some(ComputedValue::Keyword(mode)) if mode == "vertical-rl" || mode == "sideways-rl"
    );
    let base_position = if vertical_rl { under_cross } else { over_cross };
    transpose_ruby_lines(&mut base.lines, base_position);
    let mut over_cursor = over_cross;
    let mut under_cursor = over_cross + base_cross;
    for level in annotations {
        let logical_cross = if level.under {
            let cross = under_cursor;
            under_cursor += level.cross;
            cross
        } else {
            over_cursor -= level.cross;
            over_cursor
        };
        let physical_cross = if vertical_rl {
            over_cross + base_cross + under_cross - logical_cross - level.cross
        } else {
            logical_cross
        };
        transpose_ruby_lines(&mut level.layout.lines, physical_cross);
    }
    (over_cross + base_cross + under_cross, inline_extent)
}

fn layout_ruby_inline(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    context: InlineLayoutContext,
) -> Option<LayoutBox> {
    let children = node.layout_child_nodes();
    let base_nodes = children
        .iter()
        .filter(|child| {
            !child.with_tag_name(|tag| matches!(tag, Some("rt" | "rtc" | "rp")))
                && !(child.node_type() == NodeType::Text
                    && child.data().is_none_or(|text| text.trim().is_empty()))
        })
        .cloned()
        .collect::<Vec<_>>();
    let annotation_levels = ruby_annotation_levels(&children);
    if base_nodes.is_empty() || annotation_levels.is_empty() {
        return None;
    }

    let direction_rtl = super::direction_is_rtl(style);
    let base_strut = line_height(style);
    let natural_width = context.available_width.max(100_000.0);
    let natural_base = layout_ruby_level(
        &base_nodes,
        resolver,
        natural_width,
        TextAlign::Left,
        base_strut,
        direction_rtl,
        context,
    );
    let natural_annotations = annotation_levels
        .iter()
        .map(|level| {
            let strut = ruby_annotation_strut(level, resolver, base_strut);
            layout_ruby_level(
                &level.nodes,
                resolver,
                natural_width,
                TextAlign::Left,
                strut,
                direction_rtl,
                context,
            )
        })
        .collect::<Vec<_>>();
    let inline_extent = std::iter::once(&natural_base)
        .chain(natural_annotations.iter())
        .flat_map(|level| &level.lines)
        .map(|line| line.rect.width)
        .fold(0.0_f32, f32::max);
    if inline_extent <= 0.0 {
        return None;
    }
    let ruby_align = RubyAlign::from_style(style);
    let mut base = layout_ruby_level(
        &base_nodes,
        resolver,
        inline_extent,
        ruby_align.text_align(),
        base_strut,
        direction_rtl,
        context,
    );
    ruby_align.finish(&mut base.lines, inline_extent);
    let base_cross = ruby_level_cross_extent(&base);
    let mut annotations = annotation_levels
        .iter()
        .map(|level| {
            let strut = ruby_annotation_strut(level, resolver, base_strut);
            let mut layout = layout_ruby_level(
                &level.nodes,
                resolver,
                inline_extent,
                ruby_align.text_align(),
                strut,
                direction_rtl,
                context,
            );
            ruby_align.finish(&mut layout.lines, inline_extent);
            RubyLaidOutLevel {
                under: false,
                cross: ruby_level_cross_extent(&layout),
                layout,
            }
        })
        .collect::<Vec<_>>();
    let position = match style.get("ruby-position") {
        Some(ComputedValue::Keyword(value)) => value.as_str(),
        _ => "alternate",
    };
    let starts_under = position
        .split_ascii_whitespace()
        .any(|part| part == "under");
    let mut previous_under = !starts_under;
    for (source, laid_out) in annotation_levels.iter().zip(&mut annotations) {
        let level_style = source
            .position_owner
            .as_ref()
            .map(|owner| resolver.computed_style(owner));
        let explicit = level_style
            .as_ref()
            .and_then(|style| style.get("ruby-position"))
            .and_then(|value| match value {
                ComputedValue::Keyword(value)
                    if value != position && value.split_ascii_whitespace().any(|v| v == "over") =>
                {
                    Some(false)
                }
                ComputedValue::Keyword(value)
                    if value != position
                        && value.split_ascii_whitespace().any(|v| v == "under") =>
                {
                    Some(true)
                }
                _ => None,
            });
        laid_out.under = explicit.unwrap_or(!previous_under);
        previous_under = laid_out.under;
    }

    let (width, height) = position_ruby_levels(
        style,
        &mut base,
        &mut annotations,
        inline_extent,
        base_cross,
    );
    let mut lines = base.lines;
    let mut atomic_boxes = base.atomic_boxes;
    for level in annotations {
        lines.extend(level.layout.lines);
        atomic_boxes.extend(level.layout.atomic_boxes);
    }
    position_atomic_boxes(&lines, &mut atomic_boxes, resolver);

    Some(LayoutBox {
        node: node.clone(),
        pseudo: None,
        dimensions: BoxDimensions {
            content: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            ..BoxDimensions::default()
        },
        visibility: super::visibility(style),
        overflow: super::Overflow::Visible,
        position_scheme: super::position_scheme(style),
        fixed_containing_block: super::establishes_fixed_containing_block(style),
        z_index: super::z_index(style),
        transform: crate::css::AffineTransform::identity(),
        needs_scroll_translation: false,
        has_out_of_flow_descendants: None,
        content_visibility_contents_skipped: false,
        paint_scroll: None,
        block_fragments: Vec::new(),
        multicol: None,
        lines,
        children: atomic_boxes,
        marker: None,
    })
}

fn collect_input_segment(node: &NodeHandle, style: &ComputedStyle, out: &mut Vec<InlineSegment>) {
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
    let native_appearance = !matches!(
        style.get("appearance"),
        Some(ComputedValue::Keyword(value)) if value == "none"
    );
    let sizes_to_content = matches!(
        style.get("field-sizing"),
        Some(ComputedValue::Keyword(value)) if value == "content"
    );
    let content_width = explicit_length(style, "width").unwrap_or_else(|| {
        if native_appearance && matches!(input_type.as_str(), "checkbox" | "radio") {
            13.0
        } else if native_appearance && input_type == "range" {
            129.0
        } else if button_like {
            measure_text_width(&value, &metrics)
        } else if sizes_to_content {
            let displayed = if value.is_empty() {
                attributes
                    .get("placeholder")
                    .map(String::as_str)
                    .unwrap_or("")
            } else {
                &value
            };
            measure_text_width(displayed, &metrics).max(metrics.average_advance)
        } else {
            let columns = attributes
                .get("size")
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(20)
                .clamp(1, 1000);
            metrics.average_advance * columns as f32
        }
    });
    let content_height = explicit_length(style, "height").unwrap_or_else(|| {
        if native_appearance && matches!(input_type.as_str(), "checkbox" | "radio" | "range") {
            13.0
        } else {
            metrics.font_size.max(13.0)
        }
    });

    push_form_control_segment(
        node,
        style,
        value,
        content_width,
        content_height,
        metrics,
        out,
    );
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
    let content_width =
        explicit_length(style, "width").unwrap_or_else(|| measure_text_width(&label, &metrics));
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
        let total_height =
            content_height + padding.top + padding.bottom + border.top + border.bottom;
        out.push(InlineSegment {
            node: node.clone(),
            text_source: Vec::new(),
            content: InlineSegmentContent::IconFormControl(
                style.clone(),
                image,
                content_width,
                content_height,
                icon_width,
                icon_height,
            ),
            metrics: metrics.clone(),
            line_height: line_height(style).max(total_height),
            vertical_align: vertical_align(style),
            style: FragmentStyle::from_computed(style),
            word_break: word_break(style),
            overflow_wrap: overflow_wrap(style),
            white_space_mode: white_space(style),
        });
        return;
    }

    push_form_control_segment(
        node,
        style,
        label,
        content_width,
        content_height,
        metrics,
        out,
    );
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
        if child_style.is_display_none() {
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
    let metrics = font_metrics(style);
    let value = node
        .text_control_state()
        .map(|state| state.value)
        .unwrap_or_else(|| {
            strip_textarea_leading_newline(&collect_rendered_text(node, resolver)).to_string()
        });
    let content_width = explicit_length(style, "width").unwrap_or_else(|| {
        if matches!(
            style.get("field-sizing"),
            Some(ComputedValue::Keyword(value)) if value == "content"
        ) {
            return value
                .lines()
                .map(|line| measure_text_width(line, &metrics))
                .fold(metrics.average_advance, f32::max);
        }
        let cols = node
            .with_attribute("cols", |value| {
                value.and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(20)
            .clamp(1, 1000);
        metrics.average_advance * cols as f32
    });
    let content_height = explicit_length(style, "height").unwrap_or_else(|| {
        if matches!(
            style.get("field-sizing"),
            Some(ComputedValue::Keyword(value)) if value == "content"
        ) {
            return line_height(style) * value.lines().count().max(1) as f32;
        }
        let rows = node
            .with_attribute("rows", |value| {
                value.and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(2)
            .clamp(1, 1000);
        line_height(style) * rows as f32
    });

    push_form_control_segment(
        node,
        style,
        value,
        content_width,
        content_height,
        metrics,
        out,
    );
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
    if super::select_picker::renders_base_drop_down(node, style) {
        collect_base_select_segment(node, style, resolver, metrics, out);
        return;
    }
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
        if matches!(
            style.get("field-sizing"),
            Some(ComputedValue::Keyword(value)) if value == "content"
        ) {
            return measure_text_width(&label, &metrics) + 20.0;
        }
        let widest = options
            .iter()
            .map(|(text, _)| measure_text_width(text, &metrics))
            .fold(0.0f32, f32::max);
        widest + 20.0
    });
    let content_height =
        explicit_length(style, "height").unwrap_or_else(|| metrics.font_size.max(13.0));

    push_form_control_segment(
        node,
        style,
        label,
        content_width,
        content_height,
        metrics,
        out,
    );
}

/// A base appearance drop-down sizes to its content: the leading `button`'s
/// rendered text, or the selected option's label. It is at least 24px tall.
fn collect_base_select_segment(
    node: &NodeHandle,
    style: &ComputedStyle,
    resolver: &mut StyleResolver,
    metrics: FontMetrics,
    out: &mut Vec<InlineSegment>,
) {
    let label = match crate::dom::select::select_button(node) {
        Some(button) => collect_rendered_text(&button, resolver),
        None => {
            let mut options = Vec::new();
            collect_option_entries(node, resolver, &mut options);
            options
                .into_iter()
                .find(|(_, selected)| *selected)
                .map(|(text, _)| text)
                .unwrap_or_default()
        }
    };
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
    let padding = edge_sizes(style, "padding");
    let border = edge_sizes(style, "border");
    let chrome_height = padding.top + padding.bottom + border.top + border.bottom;
    let content_width = explicit_length(style, "width")
        .unwrap_or_else(|| measure_text_width(&label, &metrics).max(24.0));
    let content_height = explicit_length(style, "height")
        .unwrap_or_else(|| line_height(style).max(24.0 - chrome_height));
    push_form_control_segment(
        node,
        style,
        label,
        content_width,
        content_height,
        metrics,
        out,
    );
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
    let padding = edge_sizes(style, "padding");
    let border = edge_sizes(style, "border");
    let total_height = content_height + padding.top + padding.bottom + border.top + border.bottom;
    out.push(InlineSegment {
        node: node.clone(),
        text_source: Vec::new(),
        content: InlineSegmentContent::ValueIndicator(
            style.clone(),
            value_indicator_paint_state(node),
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

fn finite_attribute(node: &NodeHandle, name: &str, fallback: f32) -> f32 {
    node.get_attribute(name)
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
}

fn value_indicator_paint_state(node: &NodeHandle) -> ValueIndicatorPaintState {
    if node.has_tag_name("progress") {
        if node.get_attribute("value").is_none() {
            return ValueIndicatorPaintState::Progress(None);
        }
        let max = finite_attribute(node, "max", 1.0);
        let max = if max > 0.0 { max } else { 1.0 };
        let value = finite_attribute(node, "value", 0.0).clamp(0.0, max);
        return ValueIndicatorPaintState::Progress(Some(value / max));
    }

    let min = finite_attribute(node, "min", 0.0);
    let max = finite_attribute(node, "max", 1.0).max(min);
    let value = finite_attribute(node, "value", 0.0).clamp(min, max);
    let low = finite_attribute(node, "low", min).clamp(min, max);
    let high = finite_attribute(node, "high", max).clamp(low, max);
    let optimum = finite_attribute(node, "optimum", min + (max - min) / 2.0).clamp(min, max);
    let region = if optimum < low {
        if value <= low {
            MeterRegion::Optimum
        } else if value <= high {
            MeterRegion::Suboptimal
        } else {
            MeterRegion::EvenLessGood
        }
    } else if optimum > high {
        if value >= high {
            MeterRegion::Optimum
        } else if value >= low {
            MeterRegion::Suboptimal
        } else {
            MeterRegion::EvenLessGood
        }
    } else if (low..=high).contains(&value) {
        MeterRegion::Optimum
    } else {
        MeterRegion::Suboptimal
    };
    let fraction = if max > min {
        (value - min) / (max - min)
    } else {
        0.0
    };
    ValueIndicatorPaintState::Meter { fraction, region }
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
    let total_height = content_height + padding.top + padding.bottom + border.top + border.bottom;
    let editing = node
        .text_control_state()
        .map(|state| TextControlPaintState {
            selection_start: state.selection_start,
            selection_end: state.selection_end,
            focused: state.focused,
        });

    out.push(InlineSegment {
        node: node.clone(),
        text_source: Vec::new(),
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
                if style.is_display_none() {
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
        if style.is_display_none() {
            continue;
        }
        if child.has_tag_name("option") {
            let label = normalize_inline_whitespace(&collect_rendered_text(&child, resolver));
            let selected = child.has_attribute("selected");
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
        text_source: Vec::new(),
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
        line_height: line_height(&image_style)
            .max(rendered_height + padding.top + padding.bottom + border.top + border.bottom),
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
    if style.is_display_none() {
        return Vec::new();
    }

    let Some(content) = style.get("content") else {
        return Vec::new();
    };
    let line_height = line_height(&style);
    let vertical_align = vertical_align(&style);
    let wb = word_break(&style);
    let ow = overflow_wrap(&style);
    let mut fragment_style = FragmentStyle::from_computed(&style);
    fragment_style.pseudo = Some(pseudo);
    fragment_style.french_content_language = inherited_content_language_is_french(node);
    let metrics = font_metrics_with_fragment_style(&style, &fragment_style);

    match generated_content_value(
        content,
        style.component_value("content"),
        resolver,
        node,
        pseudo,
    ) {
        Some(GeneratedContent::Text(text)) => vec![InlineSegment {
            node: node.clone(),
            text_source: Vec::new(),
            content: if text.is_empty() {
                InlineSegmentContent::GeneratedBox(style.clone())
            } else {
                let normalized = normalize_text(&text, white_space(&style));
                // Apply text-transform during layout (same as regular text nodes)
                // so that layout width measurements use the transformed form.
                let transformed = apply_text_transform_layout(&normalized, &style);
                InlineSegmentContent::Text(transformed)
            },
            metrics: metrics.clone(),
            line_height,
            vertical_align,
            style: fragment_style.clone(),
            word_break: wb,
            overflow_wrap: ow,
            white_space_mode: white_space(&style),
        }],
        Some(GeneratedContent::Image(image)) => {
            let (width, height) = oriented_image_dimensions(&image, &style);
            vec![InlineSegment {
                node: node.clone(),
                text_source: Vec::new(),
                content: InlineSegmentContent::Image(image.clone(), style.clone(), width, height),
                metrics: metrics.clone(),
                line_height: line_height.max(metrics.font_size),
                vertical_align,
                style: fragment_style,
                word_break: wb,
                overflow_wrap: ow,
                white_space_mode: white_space(&style),
            }]
        }
        None => Vec::new(),
    }
}

/// Lays out the generated payload of a block-level pseudo-element inside its
/// own content box. An empty string creates the block box but no nested inline
/// fragment; its explicit height, padding, and border are handled by block
/// layout.
pub(super) fn layout_block_pseudo_content(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    pseudo: PseudoElement,
    style: &ComputedStyle,
    start_x: f32,
    start_y: f32,
    available_width: f32,
) -> Vec<LineBox> {
    let mut segments = generated_inline_segments(node, resolver, pseudo);
    segments.retain(|segment| !matches!(segment.content, InlineSegmentContent::GeneratedBox(_)));
    if segments.is_empty() {
        return Vec::new();
    }
    coalesce_adjacent_text_segments(&mut segments);
    layout_inline_segments(
        &segments,
        0,
        start_x,
        start_y,
        available_width,
        text_align(style),
        resolved_text_align_last(node, resolver, style, text_align(style)),
        line_height(style),
        Some(font_metrics(style)),
        super::direction_is_rtl(style),
        text_indent(style, available_width),
        None,
        None,
    )
}

// ── Generated content ───────────────────────────────────────────────────────

enum GeneratedContent {
    Text(String),
    Image(Image),
}

fn generated_content_value(
    value: &ComputedValue,
    components: Option<&Value>,
    resolver: &StyleResolver,
    node: &NodeHandle,
    pseudo: PseudoElement,
) -> Option<GeneratedContent> {
    if let Some(components) = components
        && let Some(text) = generated_counter_text(components, resolver, node, pseudo)
    {
        return Some(GeneratedContent::Text(text));
    }
    if components.is_none()
        && matches!(value, ComputedValue::Keyword(keyword) if matches!(keyword.to_ascii_lowercase().as_str(), "open-quote" | "close-quote" | "no-open-quote" | "no-close-quote"))
    {
        let text = resolver
            .quote_values(node, pseudo)
            .and_then(|values| values.first())
            .cloned()
            .unwrap_or_default();
        return Some(GeneratedContent::Text(text));
    }
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

fn generated_counter_text(
    value: &Value,
    resolver: &StyleResolver,
    node: &NodeHandle,
    pseudo: PseudoElement,
) -> Option<String> {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let mut output = String::new();
    let mut quote_values = resolver
        .quote_values(node, pseudo)
        .unwrap_or_default()
        .iter();
    for value in values {
        match value {
            Value::String(text) => output.push_str(text),
            Value::Function { name, arguments } if name.eq_ignore_ascii_case("counter") => {
                let counter_name = match arguments.first() {
                    Some(Value::Keyword(name)) => name,
                    _ => return None,
                };
                let style = match arguments.get(1) {
                    Some(Value::Keyword(style)) => style.as_str(),
                    None => "decimal",
                    _ => return None,
                };
                let value = resolver
                    .counter_values(node, pseudo, counter_name)
                    .and_then(|values| values.last().copied())
                    .unwrap_or(0);
                output.push_str(
                    &resolver
                        .format_counter_value(value, style)
                        .unwrap_or_else(|| format_counter(value, style)),
                );
            }
            Value::Function { name, arguments } if name.eq_ignore_ascii_case("counters") => {
                let counter_name = match arguments.first() {
                    Some(Value::Keyword(name)) => name,
                    _ => return None,
                };
                let separator = match arguments.get(1) {
                    Some(Value::String(separator)) => separator,
                    _ => return None,
                };
                let style = match arguments.get(2) {
                    Some(Value::Keyword(style)) => style.as_str(),
                    None => "decimal",
                    _ => return None,
                };
                let formatted = resolver
                    .counter_values(node, pseudo, counter_name)
                    .unwrap_or(&[0])
                    .iter()
                    .map(|value| {
                        resolver
                            .format_counter_value(*value, style)
                            .unwrap_or_else(|| format_counter(*value, style))
                    })
                    .collect::<Vec<_>>()
                    .join(separator);
                output.push_str(&formatted);
            }
            Value::Keyword(keyword)
                if matches!(
                    keyword.to_ascii_lowercase().as_str(),
                    "open-quote" | "close-quote"
                ) =>
            {
                output.push_str(quote_values.next().map(String::as_str).unwrap_or(""));
            }
            Value::Keyword(keyword)
                if matches!(
                    keyword.to_ascii_lowercase().as_str(),
                    "no-open-quote" | "no-close-quote"
                ) => {}
            _ => return None,
        }
    }
    Some(output)
}

fn format_counter(value: i32, style: &str) -> String {
    match style.to_ascii_lowercase().as_str() {
        "none" => String::new(),
        "decimal-leading-zero" => {
            if (-9..=9).contains(&value) {
                if value < 0 {
                    format!("-{:02}", value.unsigned_abs())
                } else {
                    format!("{value:02}")
                }
            } else {
                value.to_string()
            }
        }
        "lower-roman" if (1..=3999).contains(&value) => super::to_roman_lower(value as usize),
        "upper-roman" if (1..=3999).contains(&value) => super::to_roman_upper(value as usize),
        "lower-alpha" | "lower-latin" if value > 0 => super::to_alpha_lower(value as usize),
        "upper-alpha" | "upper-latin" if value > 0 => super::to_alpha_upper(value as usize),
        _ => value.to_string(),
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
    match tag_name.as_str() {
        "canvas" => crate::canvas::image(node.identity()).map(|image| (node.clone(), image)),
        "img" => selected_image_url(node, None)
            .and_then(|src| decode_or_fetch_image(&src))
            .map(|image| (node.clone(), image)),
        "video" => {
            let poster = node.get_attribute("poster")?;
            decode_or_fetch_image(&poster).map(|image| (node.clone(), image))
        }
        "picture" => picture_image(node).and_then(|(image_node, src)| {
            decode_or_fetch_image(&src)
                .or_else(|| {
                    selected_image_url(&image_node, None)
                        .and_then(|fallback| decode_or_fetch_image(&fallback))
                })
                .map(|image| (image_node, image))
        }),
        "svg" => {
            let image = crate::svg::render_svg_to_image_with_current_color(
                node,
                current_color.unwrap_or(crate::paint::color::Color::rgb(0, 0, 0)),
            )?;
            Some((node.clone(), image))
        }
        "object" => {
            if let Some(data) = node.get_attribute("data")
                && let Some(image) = decode_or_fetch_image(&data)
            {
                return Some((node.clone(), image));
            }

            for child in node.layout_child_nodes() {
                if let Some(image) = element_inline_image_with_current_color(&child, current_color)
                {
                    return Some(image);
                }
            }

            None
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum SourceDescriptor {
    Density(f32),
    Width(f32),
}

fn picture_image(picture: &NodeHandle) -> Option<(NodeHandle, String)> {
    let children = picture.layout_child_nodes();
    let image = children
        .iter()
        .find(|child| child.tag_name().as_deref() == Some("img"))?;
    for source in children
        .iter()
        .take_while(|child| child.identity() != image.identity())
    {
        if source.tag_name().as_deref() != Some("source") || !source_type_supported(source) {
            continue;
        }
        let media = source.get_attribute("media").unwrap_or_default();
        if !source_media_matches(&media) {
            continue;
        }
        if let Some(url) = selected_image_url(source, Some(image)) {
            return Some((image.clone(), url));
        }
    }
    selected_image_url(image, None).map(|url| (image.clone(), url))
}

fn source_type_supported(source: &NodeHandle) -> bool {
    let Some(kind) = source.get_attribute("type") else {
        return true;
    };
    matches!(
        kind.split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "" | "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/gif"
            | "image/webp"
            | "image/svg+xml"
    )
}

fn source_media_matches(media: &str) -> bool {
    if media.trim().is_empty() {
        return true;
    }
    let (width, height, _) = super::current_image_selection_environment();
    parse_media_query_list(media).is_some_and(|queries| {
        queries
            .iter()
            .any(|query| evaluate_media_query(query, width, height, false))
    })
}

fn selected_image_url(node: &NodeHandle, fallback_sizes: Option<&NodeHandle>) -> Option<String> {
    let srcset = node.get_attribute("srcset").unwrap_or_default();
    let candidates = parse_srcset(&srcset);
    if candidates.is_empty() {
        return node
            .get_attribute("src")
            .filter(|src| !src.trim().is_empty());
    }
    let sizes = node
        .get_attribute("sizes")
        .or_else(|| fallback_sizes.and_then(|image| image.get_attribute("sizes")))
        .unwrap_or_default();
    select_source_candidate(&candidates, &sizes).map(|(url, _)| url.clone())
}

fn parse_srcset(input: &str) -> Vec<(String, SourceDescriptor)> {
    let mut rest = input.trim();
    let mut candidates = Vec::new();
    while !rest.is_empty() {
        rest = rest.trim_start_matches(|ch: char| ch.is_ascii_whitespace() || ch == ',');
        if rest.is_empty() {
            break;
        }
        let url_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let mut url = &rest[..url_end];
        rest = &rest[url_end..];
        let trailing_separator = url.ends_with(',');
        url = url.trim_end_matches(',');
        let descriptor_end = rest.find(',').unwrap_or(rest.len());
        let descriptor = if trailing_separator {
            ""
        } else {
            rest[..descriptor_end].trim()
        };
        rest = if trailing_separator {
            rest
        } else {
            &rest[descriptor_end..]
        };
        let parsed = if descriptor.is_empty() {
            Some(SourceDescriptor::Density(1.0))
        } else if let Some(value) = descriptor.strip_suffix('x') {
            value
                .parse()
                .ok()
                .filter(|value: &f32| value.is_finite() && *value > 0.0)
                .map(SourceDescriptor::Density)
        } else if let Some(value) = descriptor.strip_suffix('w') {
            value
                .parse()
                .ok()
                .filter(|value: &f32| value.is_finite() && *value > 0.0)
                .map(SourceDescriptor::Width)
        } else {
            None
        };
        if !url.is_empty()
            && let Some(descriptor) = parsed
        {
            candidates.push((url.to_string(), descriptor));
        }
    }
    candidates
}

fn select_source_candidate<'a>(
    candidates: &'a [(String, SourceDescriptor)],
    sizes: &str,
) -> Option<&'a (String, SourceDescriptor)> {
    let (viewport_width, viewport_height, dpr) = super::current_image_selection_environment();
    let width_descriptors = candidates
        .iter()
        .all(|(_, descriptor)| matches!(descriptor, SourceDescriptor::Width(_)));
    let density_descriptors = candidates
        .iter()
        .all(|(_, descriptor)| matches!(descriptor, SourceDescriptor::Density(_)));
    if !width_descriptors && !density_descriptors {
        return None;
    }
    let target = if width_descriptors {
        source_size(sizes, viewport_width, viewport_height) * dpr
    } else {
        dpr
    };
    candidates
        .iter()
        .filter_map(|candidate| {
            let value = match candidate.1 {
                SourceDescriptor::Density(value) | SourceDescriptor::Width(value) => value,
            };
            (value >= target).then_some((value, candidate))
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, candidate)| candidate)
        .or_else(|| {
            candidates.iter().max_by(|left, right| {
                let value = |descriptor| match descriptor {
                    SourceDescriptor::Density(value) | SourceDescriptor::Width(value) => value,
                };
                value(left.1).total_cmp(&value(right.1))
            })
        })
}

fn source_size(sizes: &str, viewport_width: f32, viewport_height: f32) -> f32 {
    for component in sizes.split(',') {
        let component = component.trim();
        let Some((condition, length)) = component.rsplit_once(char::is_whitespace) else {
            if let Some(length) = source_size_length(component, viewport_width) {
                return length;
            }
            continue;
        };
        if source_media_matches_with_viewport(condition, viewport_width, viewport_height)
            && let Some(length) = source_size_length(length, viewport_width)
        {
            return length;
        }
    }
    viewport_width
}

fn source_media_matches_with_viewport(media: &str, width: f32, height: f32) -> bool {
    parse_media_query_list(media).is_some_and(|queries| {
        queries
            .iter()
            .any(|query| evaluate_media_query(query, width, height, false))
    })
}

fn source_size_length(length: &str, viewport_width: f32) -> Option<f32> {
    let length = length.trim().to_ascii_lowercase();
    if let Some(value) = length.strip_suffix("px") {
        return value
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|value| *value >= 0.0);
    }
    if let Some(value) = length.strip_suffix("vw") {
        return value
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|value| *value >= 0.0)
            .map(|value| value * viewport_width / 100.0);
    }
    None
}

/// Decode an image from a data: URI (PNG, JPEG, GIF, WebP, or SVG).
fn decode_data_uri_image(uri: &str) -> Option<Image> {
    super::ensure_image_cache_context();
    if let Some(image) = IMAGE_CACHE.with(|cache| cache.borrow().get(uri).cloned()) {
        return image;
    }
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
                let animation = std::sync::Arc::new(Image::decode_gif_animation(&data).ok()?);
                Some(super::image_animation::sample(&animation, uri))
            } else if mime_type.eq_ignore_ascii_case("image/webp") {
                Image::decode_webp(&data).ok()
            } else if mime_type.eq_ignore_ascii_case("image/svg+xml") {
                decode_data_svg(std::str::from_utf8(&data).ok()?, uri)
            } else {
                None
            }
        }
        DataUri::Text { mime_type, data } => {
            if mime_type.eq_ignore_ascii_case("image/svg+xml") {
                decode_data_svg(&data, uri)
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
    crate::svg::render_svg_to_image(&parse_svg_node(text)?)
}

const MAX_CACHED_DATA_SVGS: usize = 64;
const MAX_CACHED_DATA_SVG_PIXEL_BYTES: usize = 256 * 1024;
const MAX_CACHED_DATA_SVG_URI_BYTES: usize = 64 * 1024;

/// Reuses immutable, self-contained SVG data images across layout and paint.
/// SVGs containing image elements remain live: their nested resources can be
/// pending, relative to a changing base URL, revoked, or animated.
fn decode_data_svg(text: &str, uri: &str) -> Option<Image> {
    fn contains_image(node: &NodeHandle) -> bool {
        node.tag_name()
            .is_some_and(|tag| tag.eq_ignore_ascii_case("image"))
            || node.child_nodes().iter().any(contains_image)
    }
    let svg = parse_svg_node(text)?;
    let cacheable = !contains_image(&svg);
    let image = crate::svg::render_svg_to_image(&svg)?;
    // Bound the added retention even when a page continuously changes its URIs.
    if cacheable
        && image.pixels().len() <= MAX_CACHED_DATA_SVG_PIXEL_BYTES
        && uri.len() <= MAX_CACHED_DATA_SVG_URI_BYTES
    {
        IMAGE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let count = cache
                .keys()
                .filter(|key| {
                    key.get(..5)
                        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
                })
                .take(MAX_CACHED_DATA_SVGS)
                .count();
            if count < MAX_CACHED_DATA_SVGS {
                cache.insert(uri.to_owned(), Some(image.clone()));
            }
        });
    }
    Some(image)
}

fn parse_svg_node(text: &str) -> Option<NodeHandle> {
    use crate::dom::Node;
    use crate::html::TreeBuilder;
    let doc = TreeBuilder::parse(text).document();
    // Find the <svg> element in the parsed document
    fn find_svg(node: &NodeHandle) -> Option<NodeHandle> {
        if node.has_tag_name("svg") {
            return Some(node.clone());
        }
        for child in node.child_nodes() {
            if let Some(found) = find_svg(&child) {
                return Some(found);
            }
        }
        None
    }
    find_svg(&doc)
}

/// Maximum image size to fetch (10 MiB).
const MAX_IMAGE_SIZE: usize = 10 * 1024 * 1024;

/// Fetch an image from an HTTP/HTTPS URL with caching.
fn fetch_image(url: &str) -> Option<Image> {
    super::ensure_image_cache_context();
    super::image_security::record_layout_image_dependency(url);
    if let Some(image) = IMAGE_ANIMATION_CACHE.with(|cache| {
        cache
            .borrow()
            .get(url)
            .map(|animation| super::image_animation::sample(animation, url))
    }) {
        return Some(image);
    }
    // Check cache first
    let cached = IMAGE_CACHE.with(|cache| cache.borrow().get(url).cloned());
    if let Some(result) = cached {
        return result;
    }

    if super::image_security::start_async_image(url) {
        if super::image_security::image_asset_pending(url) {
            IMAGE_PENDING_DEPENDENCIES.with(|stack| {
                for dependencies in stack.borrow_mut().iter_mut() {
                    dependencies.insert(url.to_owned());
                }
            });
        }
        return None;
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
    let context = IMAGE_COOKIE_CONTEXT.with(|cell| cell.borrow().clone());
    if let Some(context) = context.as_ref() {
        context.cookie_context.apply(&mut request);
    } else if let Some(site) = IMAGE_BASE_URL.with(|cell| cell.borrow().clone()) {
        request.set_cookie_context(site, false);
    }
    request.set_header(
        "Accept",
        "image/webp,image/png,image/jpeg,image/gif,image/svg+xml;q=0.9,*/*;q=0.1",
    );
    let response = HTTP_CLIENT.with(|client| {
        let mut client = client.borrow_mut();
        match context {
            Some(context) => {
                client.set_shared_cookie_store(Arc::clone(&context.store));
                let mut violations = Vec::new();
                let mut check_redirect = |target: &Url, redirects| {
                    super::image_security::allows_image_request(&target.to_string(), redirects, url)
                };
                let fetched = cors::fetch_with_embedder_policy_reporting_checked(
                    &mut client,
                    request,
                    &context.origin,
                    RequestMode::NoCors,
                    CredentialsMode::Include,
                    RedirectMode::Follow,
                    &mut PreflightCache::default(),
                    context.embedder_policy,
                    context.report_only_embedder_policy,
                    &mut violations,
                    None,
                    &mut check_redirect,
                );
                context.coep_reports.borrow_mut().extend(violations);
                let response = fetched.ok().map(|fetched| fetched.response);
                client.clear_shared_cookie_store();
                response
            }
            None => fetch_image_without_document(&mut client, request, url),
        }
    })?;

    if !super::image_security::image_request_is_live() {
        return None;
    }
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
        let animation = std::sync::Arc::new(Image::decode_gif_animation(body).ok()?);
        let image = super::image_animation::sample(&animation, url);
        IMAGE_ANIMATION_CACHE.with(|cache| {
            cache.borrow_mut().insert(url.to_string(), animation);
        });
        return Some(image);
    }
    decode_image_bytes(body, &content_type, url)
}

/// Retains the standalone client path while checking redirect targets before I/O.
fn fetch_image_without_document(
    client: &mut Client,
    request: HttpRequest,
    initial: &str,
) -> Option<HttpResponse> {
    let mut redirects = 0;
    let mut observe = |request: &HttpRequest, response: &HttpResponse| {
        if !matches!(response.status_code(), 301 | 302 | 303 | 307 | 308) {
            return true;
        }
        let Some(location) = response.header("location") else {
            return true;
        };
        let Ok(target) = resolve_url(request.url(), location) else {
            return true;
        };
        redirects += 1;
        super::image_security::allows_image_request(&target.to_string(), redirects, initial)
    };
    client
        .send_observing_navigation(request, &mut observe)
        .ok()
        .flatten()
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
    super::ensure_image_cache_context();
    if let Some(cached) = IMAGE_CACHE.with(|cache| cache.borrow().get(url).cloned()) {
        return cached;
    }
    let entry = crate::data::lookup_blob_url(url)?;
    if entry.bytes.len() > MAX_IMAGE_SIZE {
        return None;
    }
    let dependencies = ImageDependencies::begin();
    let image = decode_image_bytes(&entry.bytes, &entry.media_type.to_lowercase(), url)?;
    if !dependencies.has_pending() {
        IMAGE_CACHE.with(|cache| {
            cache
                .borrow_mut()
                .insert(url.to_string(), Some(image.clone()));
        });
    }
    Some(image)
}

fn decode_or_fetch_image(url_like: &str) -> Option<Image> {
    let base_url = super::current_image_base_url();
    decode_or_fetch_image_with_base(url_like, base_url.as_ref())
}

fn decode_or_fetch_image_with_base(url_like: &str, base_url: Option<&Url>) -> Option<Image> {
    let url_like = url_like.trim();
    if url_like.is_empty() {
        return None;
    }
    let reference = canonical_image_asset_reference(url_like, base_url)?;
    if !super::image_security::allows_image_request(&reference, 0, &reference) {
        return None;
    }
    if url_like
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
    {
        let image = decode_data_uri_image(url_like);
        if image.is_none() {
            super::report_inline_image_decode_failure();
        }
        return image;
    }
    // `get` rather than a range index: a source such as "日本語です" would make
    // byte 5 land inside a character and panic.
    if url_like
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("blob:"))
    {
        return decode_blob_url_image(url_like);
    }
    let resolved = resolve_image_url(url_like, base_url)?;
    fetch_image(&resolved)
}

/// Decodes an image reference using the caller's explicit base URL.
pub(crate) fn decode_or_fetch_image_asset(url_like: &str, base_url: Option<&Url>) -> Option<Image> {
    decode_or_fetch_image_with_base(url_like, base_url)
}

/// Resolves an image and reports the pending network dependencies encountered
/// during this decode, including those inside composite blob or data SVG images.
pub(crate) fn decode_or_fetch_image_asset_with_pending_dependencies(
    url_like: &str,
    base_url: Option<&Url>,
) -> (Option<Image>, Vec<String>) {
    let dependencies = ImageDependencies::begin();
    let image = decode_or_fetch_image_asset(url_like, base_url);
    (image, dependencies.pending_references())
}

/// Produces a stable identity for a referenced image without reading ambient URL state.
pub(crate) fn canonical_image_asset_reference(
    url_like: &str,
    base_url: Option<&Url>,
) -> Option<String> {
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
    resolve_image_url(url_like, base_url)
}

fn resolve_image_url(url_like: &str, base_url: Option<&Url>) -> Option<String> {
    let is_http = url_like.split_once("://").is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    });
    if is_http {
        return url_like.parse::<Url>().ok().and_then(normalize_image_url);
    }
    if url_like.contains("://") || url_like.starts_with("//") {
        return None;
    }
    resolve_url(base_url?, url_like)
        .ok()
        .and_then(normalize_image_url)
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
    let normalized = node.with_attribute("alt", |alt| {
        alt.map(|alt| normalize_text(alt, white_space(style)))
    })?;
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn html_image_dimension_attribute(node: &NodeHandle, name: &str) -> Option<f32> {
    node.with_attribute(name, |raw| {
        let parsed = raw?.trim().parse::<f32>().ok()?;
        (parsed.is_finite() && parsed > 0.0).then_some(parsed)
    })
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
    let (intrinsic_w, intrinsic_h) = oriented_image_dimensions(image, style);
    let intrinsic_ratio =
        (intrinsic_w > 0.0 && intrinsic_h > 0.0).then(|| intrinsic_w / intrinsic_h);
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
        border_box_adjust_length(
            style,
            value,
            padding.left + border.left,
            padding.right + border.right,
        )
    };
    let to_content_height = |value| {
        border_box_adjust_length(
            style,
            value,
            padding.top + border.top,
            padding.bottom + border.bottom,
        )
    };
    let specified_width = explicit_length(style, "width")
        .or_else(|| html_image_dimension_attribute(node, "width"))
        .map(to_content_width);
    let specified_height = explicit_length(style, "height")
        .or_else(|| html_image_dimension_attribute(node, "height"))
        .map(to_content_height);

    let from_height = |height: f32| {
        ratio.map_or(intrinsic_w, |ratio| {
            if ratio.uses_box_sizing {
                to_content_width((height + vertical_decoration) * ratio.value)
            } else {
                (height * ratio.value).max(0.0)
            }
        })
    };
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
        && width > max_width
    {
        if height_is_derived {
            height = from_width(max_width);
        }
        width = max_width;
    }
    if let Some(max_height) = explicit_length(style, "max-height").map(to_content_height)
        && height > max_height
    {
        if width_is_derived {
            width = from_height(max_height);
        }
        height = max_height;
    }
    if let Some(min_width) = explicit_length(style, "min-width").map(to_content_width)
        && width < min_width
    {
        if height_is_derived {
            height = from_width(min_width);
        }
        width = min_width;
    }
    if let Some(min_height) = explicit_length(style, "min-height").map(to_content_height)
        && height < min_height
    {
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
pub(super) enum WhiteSpaceCollapse {
    Collapse,
    Discard,
    Preserve,
    PreserveBreaks,
    PreserveSpaces,
    BreakSpaces,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TabSize {
    Spaces(f32),
    Length(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WhiteSpaceMode {
    collapse: WhiteSpaceCollapse,
    wraps: bool,
    tab_size: TabSize,
}

impl WhiteSpaceMode {
    fn pre_line() -> Self {
        Self {
            collapse: WhiteSpaceCollapse::PreserveBreaks,
            wraps: true,
            tab_size: TabSize::Spaces(8.0),
        }
    }

    /// Whether the mode collapses whitespace sequences into a single space.
    pub(super) fn collapses_whitespace(self) -> bool {
        matches!(
            self.collapse,
            WhiteSpaceCollapse::Collapse | WhiteSpaceCollapse::PreserveBreaks
        )
    }

    /// Whether the mode preserves newline characters as line breaks.
    pub(super) fn preserves_newlines(self) -> bool {
        matches!(
            self.collapse,
            WhiteSpaceCollapse::Preserve
                | WhiteSpaceCollapse::PreserveBreaks
                | WhiteSpaceCollapse::BreakSpaces
        )
    }

    /// Whether the mode allows automatic line wrapping.
    pub(super) fn allows_wrapping(self) -> bool {
        self.wraps
    }

    pub(super) fn collapse(self) -> WhiteSpaceCollapse {
        self.collapse
    }

    fn tab_advance(self, used_inline_size: f32, metrics: &FontMetrics) -> f32 {
        let interval = match self.tab_size {
            TabSize::Spaces(count) => {
                let mut space_metrics = metrics.clone();
                space_metrics.word_spacing = 0.0;
                measure_text_width(" ", &space_metrics) * count
            }
            TabSize::Length(length) => length,
        };
        if interval <= 0.0 || !interval.is_finite() {
            return 0.0;
        }
        let remainder = used_inline_size.max(0.0) % interval;
        if remainder <= f32::EPSILON {
            interval
        } else {
            interval - remainder
        }
    }
}

pub(super) fn white_space(style: &ComputedStyle) -> WhiteSpaceMode {
    let collapse = match style.get("white-space-collapse") {
        Some(ComputedValue::Keyword(keyword)) => match keyword.to_ascii_lowercase().as_str() {
            "discard" => WhiteSpaceCollapse::Discard,
            "preserve" => WhiteSpaceCollapse::Preserve,
            "preserve-breaks" => WhiteSpaceCollapse::PreserveBreaks,
            "preserve-spaces" => WhiteSpaceCollapse::PreserveSpaces,
            "break-spaces" => WhiteSpaceCollapse::BreakSpaces,
            _ => WhiteSpaceCollapse::Collapse,
        },
        _ => WhiteSpaceCollapse::Collapse,
    };
    let wraps = !matches!(
        style.get("text-wrap-mode"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("nowrap")
    );
    let tab_size = match style.get("tab-size") {
        Some(ComputedValue::Px(length)) => TabSize::Length(length.max(0.0)),
        Some(ComputedValue::Number(count)) => TabSize::Spaces(count.max(0.0)),
        _ => TabSize::Spaces(8.0),
    };
    WhiteSpaceMode {
        collapse,
        wraps,
        tab_size,
    }
}

pub(super) fn normalize_text(text: &str, mode: WhiteSpaceMode) -> String {
    text_source::normalized_chars(text, mode)
        .into_iter()
        .map(|(ch, _)| ch)
        .collect()
}

pub(super) fn font_size(style: &ComputedStyle) -> f32 {
    explicit_length(style, "font-size").unwrap_or(16.0)
}

#[derive(Clone, Copy)]
enum FontSizeAdjustMetric {
    ExHeight,
    CapHeight,
    ChWidth,
    IcWidth,
    IcHeight,
}

#[derive(Clone, Copy)]
enum FontSizeAdjustRatio {
    Number(f32),
    FromFont,
}

fn font_size_adjust(style: &ComputedStyle) -> Option<(FontSizeAdjustMetric, FontSizeAdjustRatio)> {
    let value = match style.get("font-size-adjust")? {
        ComputedValue::Number(value) => {
            return Some((
                FontSizeAdjustMetric::ExHeight,
                FontSizeAdjustRatio::Number(*value),
            ));
        }
        ComputedValue::Keyword(value) | ComputedValue::String(value) => value.as_str(),
        _ => return None,
    };
    let mut components = value.split_ascii_whitespace();
    let first = components.next()?;
    let (metric, ratio) = match first.to_ascii_lowercase().as_str() {
        "none" => return None,
        "from-font" => (
            FontSizeAdjustMetric::ExHeight,
            FontSizeAdjustRatio::FromFont,
        ),
        "ex-height" => (
            FontSizeAdjustMetric::ExHeight,
            parse_font_size_adjust_ratio(components.next()?)?,
        ),
        "cap-height" => (
            FontSizeAdjustMetric::CapHeight,
            parse_font_size_adjust_ratio(components.next()?)?,
        ),
        "ch-width" => (
            FontSizeAdjustMetric::ChWidth,
            parse_font_size_adjust_ratio(components.next()?)?,
        ),
        "ic-width" => (
            FontSizeAdjustMetric::IcWidth,
            parse_font_size_adjust_ratio(components.next()?)?,
        ),
        "ic-height" => (
            FontSizeAdjustMetric::IcHeight,
            parse_font_size_adjust_ratio(components.next()?)?,
        ),
        number => (
            FontSizeAdjustMetric::ExHeight,
            FontSizeAdjustRatio::Number(number.parse().ok()?),
        ),
    };
    components.next().is_none().then_some((metric, ratio))
}

fn parse_font_size_adjust_ratio(value: &str) -> Option<FontSizeAdjustRatio> {
    if value.eq_ignore_ascii_case("from-font") {
        Some(FontSizeAdjustRatio::FromFont)
    } else {
        value.parse().ok().map(FontSizeAdjustRatio::Number)
    }
}

fn adjusted_font_size(
    font: &crate::font::Font,
    specified_size: f32,
    adjustment: Option<(FontSizeAdjustMetric, FontSizeAdjustRatio)>,
) -> f32 {
    let Some((metric, ratio)) = adjustment else {
        return specified_size;
    };
    let metrics = font.css_relative_metrics(1.0, false, false);
    let actual_ratio = match metric {
        FontSizeAdjustMetric::ExHeight => metrics.ex,
        FontSizeAdjustMetric::CapHeight => metrics.cap,
        FontSizeAdjustMetric::ChWidth => metrics.ch,
        FontSizeAdjustMetric::IcWidth => metrics.ic,
        // CSS fallback for a missing ideographic full-height metric is 1em.
        FontSizeAdjustMetric::IcHeight => 1.0,
    };
    let desired_ratio = match ratio {
        FontSizeAdjustRatio::Number(value) => value,
        FontSizeAdjustRatio::FromFont => actual_ratio,
    };
    if actual_ratio > 0.0 && actual_ratio.is_finite() && desired_ratio.is_finite() {
        specified_size * desired_ratio / actual_ratio
    } else {
        specified_size
    }
}

pub(super) fn font_metrics(style: &ComputedStyle) -> FontMetrics {
    let fragment_style = FragmentStyle::from_computed(style);
    font_metrics_with_fragment_style(style, &fragment_style)
}

fn font_metrics_with_fragment_style(
    style: &ComputedStyle,
    fragment_style: &FragmentStyle,
) -> FontMetrics {
    let mut metrics = FontMetrics::from_font_size(font_size(style));
    metrics.letter_spacing = letter_spacing(style);
    metrics.word_spacing = word_spacing(style);
    metrics.text_autospace = fragment_style.text_autospace;
    metrics.text_spacing_trim = fragment_style.text_spacing_trim;
    metrics.french_content_language = fragment_style.french_content_language;
    metrics.text_orientation = fragment_style.text_orientation;
    metrics.text_combine_upright = fragment_style.text_combine_upright;
    metrics.vertical_writing = fragment_style.writing_mode.as_deref().is_some_and(|value| {
        matches!(
            value,
            "vertical-rl" | "vertical-lr" | "sideways-rl" | "sideways-lr"
        )
    });
    metrics.sideways_writing = fragment_style
        .writing_mode
        .as_deref()
        .is_some_and(|value| value.starts_with("sideways-"));
    metrics.font_family = style.get("font-family").and_then(computed_font_family_key);
    metrics.font_scope_root = style.font_family_scope_root();
    metrics.shaping_options = Arc::clone(style.shaping_options());
    metrics.font_weight = computed_font_weight(style);
    let font_style = computed_font_property(style, "font-style")
        .map(crate::font::FontStyleRange::parse)
        .unwrap_or_else(|| crate::font::FontStyleRange::exact(FontStyle::default()));
    metrics.font_style = font_style.style;
    metrics.font_style_angle = font_style.requested_angle();
    metrics.font_stretch = match style.get("font-stretch") {
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => {
            crate::font::FontStretch::parse(value)
        }
        Some(ComputedValue::Percentage(value)) => {
            crate::font::FontStretch::parse(&format!("{value}%"))
        }
        _ => crate::font::FontStretch::default(),
    };
    LAYOUT_FONTS.with(|cell| {
        let mut fonts = cell.borrow_mut();
        let Some(context) = fonts.as_mut() else {
            return;
        };
        if !context.exact_metrics {
            return;
        }
        let variant = FontVariantKey {
            weight: metrics.font_weight,
            style: metrics.font_style,
            style_angle: metrics.font_style_angle,
            stretch: metrics.font_stretch,
        };
        let font_phase = super::profile::Scope::begin(super::profile::Phase::FontSelect);
        let selected = select_text_font(
            "layout",
            metrics.font_family,
            metrics.font_scope_root,
            variant,
            context.web_fonts.as_deref(),
            &context.system_fonts,
        );
        drop(font_phase);
        let selected = selected
            .as_ref()
            .map(AsRef::as_ref)
            .or_else(|| context.system_fonts.first().map(AsRef::as_ref));
        let Some(selected) = selected else {
            return;
        };
        metrics.font_size =
            adjusted_font_size(selected, metrics.font_size, font_size_adjust(style));
        let key = (
            metrics.font_family,
            metrics.font_scope_root,
            variant,
            metrics.font_size.to_bits(),
        );
        let actual = context.metrics_cache.get(&key).copied().or_else(|| {
            let actual = selected.layout_metrics(metrics.font_size);
            context.metrics_cache.insert(key, actual);
            Some(actual)
        });
        if let Some(actual) = actual {
            metrics.ascent = actual.ascent;
            metrics.descent = actual.descent;
            metrics.line_gap = actual.line_gap;
            metrics.average_advance = actual.average_advance;
        }
    });
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
    let family = value.trim();
    let family = if family.contains(',') {
        family
    } else {
        family.trim_matches(['"', '\''])
    };
    (!family.is_empty()).then(|| FontFamilyKey::new(family))
}

fn letter_spacing(style: &ComputedStyle) -> f32 {
    match style.get("letter-spacing") {
        Some(ComputedValue::Px(value)) => *value,
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("normal") => 0.0,
        _ => 0.0,
    }
}

fn word_spacing(style: &ComputedStyle) -> f32 {
    match style.get("word-spacing") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("normal") => 0.0,
        Some(value) => value
            .resolve_length_percentage(font_size(style))
            .unwrap_or(0.0),
        None => 0.0,
    }
}

#[derive(Clone, Copy, Debug)]
struct TextIndent {
    amount: f32,
    hanging: bool,
    each_line: bool,
    initial_affected: bool,
}

impl Default for TextIndent {
    fn default() -> Self {
        Self {
            amount: 0.0,
            hanging: false,
            each_line: false,
            initial_affected: true,
        }
    }
}

fn text_indent(style: &ComputedStyle, containing_inline_size: f32) -> TextIndent {
    let Some(value) = style.get("text-indent") else {
        return TextIndent::default();
    };
    match value {
        ComputedValue::TextIndent {
            value,
            hanging,
            each_line,
        } => TextIndent {
            amount: value
                .resolve_length_percentage(containing_inline_size)
                .unwrap_or(0.0),
            hanging: *hanging,
            each_line: *each_line,
            initial_affected: true,
        },
        value => TextIndent {
            amount: value
                .resolve_length_percentage(containing_inline_size)
                .unwrap_or(0.0),
            ..TextIndent::default()
        },
    }
}

pub(super) fn text_indent_amount(style: &ComputedStyle, containing_inline_size: f32) -> f32 {
    text_indent(style, containing_inline_size).amount
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
    available_width: f32,
    line_box_width: f32,
    unindented_start_x: f32,
    unindented_available_width: f32,
    unindented_line_box_width: f32,
    maximum_wrap_width: Option<f32>,
    strut_line_height: f32,
    strut_metrics: Option<FontMetrics>,
    direction_rtl: bool,
    indent: TextIndent,
    last_align: TextAlign,
    line_after_forced_break: bool,
}

#[derive(Clone, Copy)]
enum LineBreakKind {
    Soft,
    Forced,
}

impl InlineCursor {
    fn new(
        start_x: f32,
        start_y: f32,
        available_width: f32,
        wrap_width: Option<f32>,
        strut_line_height: f32,
        strut_metrics: Option<FontMetrics>,
        direction_rtl: bool,
        indent: TextIndent,
        last_align: TextAlign,
    ) -> Self {
        let unindented_start_x = start_x;
        let maximum_wrap_width = wrap_width.map(|width| width.min(available_width));
        let wrap_width = maximum_wrap_width.unwrap_or(available_width);
        let mut cursor = Self {
            x: start_x,
            y: start_y,
            line_height: strut_line_height,
            start_x,
            available_width: wrap_width,
            line_box_width: available_width,
            unindented_start_x,
            unindented_available_width: wrap_width,
            unindented_line_box_width: available_width,
            maximum_wrap_width,
            strut_line_height,
            strut_metrics,
            direction_rtl,
            indent,
            last_align,
            line_after_forced_break: false,
        };
        cursor.apply_line_indent(indent.initial_affected);
        cursor
    }

    fn apply_line_indent(&mut self, normally_affected: bool) {
        let amount = if normally_affected != self.indent.hanging {
            self.indent.amount
        } else {
            0.0
        };
        self.start_x = if self.direction_rtl {
            self.unindented_start_x
        } else {
            self.unindented_start_x + amount
        };
        self.available_width = (self.unindented_available_width - amount).max(0.0);
        self.line_box_width = (self.unindented_line_box_width - amount).max(0.0);
        self.x = self.start_x;
    }

    fn wrap_line(
        &mut self,
        lines: &mut Vec<LineBox>,
        fragments: &mut Vec<InlineFragment>,
        segment_line_height: f32,
        align: TextAlign,
        line_constraints: Option<&dyn Fn(f32, f32) -> (f32, f32)>,
        break_kind: LineBreakKind,
    ) {
        let effective_height = self.line_height.max(segment_line_height);
        let line_align = if matches!(break_kind, LineBreakKind::Forced) {
            self.last_align
        } else {
            align
        };
        let used_height = push_line(
            lines,
            fragments,
            self.start_x,
            self.y,
            self.x - self.start_x,
            effective_height,
            self.line_box_width,
            line_align,
            self.direction_rtl,
            self.strut_line_height,
            self.strut_metrics.clone(),
            lines.is_empty() || self.line_after_forced_break,
        );
        self.y += used_height;
        if let Some(constraints) = line_constraints {
            let (start_x, line_box_width) = constraints(self.y, self.strut_line_height);
            self.unindented_start_x = start_x;
            self.unindented_line_box_width = line_box_width;
            self.unindented_available_width = self
                .maximum_wrap_width
                .map_or(line_box_width, |maximum| line_box_width.min(maximum));
        }
        self.apply_line_indent(
            matches!(break_kind, LineBreakKind::Forced) && self.indent.each_line,
        );
        self.line_after_forced_break = matches!(break_kind, LineBreakKind::Forced);
        self.line_height = self.strut_line_height;
    }
    fn finish_line(
        &self,
        lines: &mut Vec<LineBox>,
        fragments: &mut Vec<InlineFragment>,
        _align: TextAlign,
    ) {
        if !fragments.is_empty() {
            let final_height = if fragments.iter().all(|fragment| {
                matches!(
                    fragment.content,
                    InlineFragmentContent::InlineEdge(_, _)
                        | InlineFragmentContent::InlineSpacing(_)
                ) && fragment.rect.width == 0.0
            }) {
                0.0
            } else {
                self.line_height.max(0.0)
            };
            push_line(
                lines,
                fragments,
                self.start_x,
                self.y,
                self.x - self.start_x,
                final_height,
                self.line_box_width,
                self.last_align,
                self.direction_rtl,
                self.strut_line_height,
                self.strut_metrics.clone(),
                lines.is_empty() || self.line_after_forced_break,
            );
        }
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
        && (matches!(
            overflow_wrap,
            OverflowWrap::BreakWord | OverflowWrap::Anywhere
        ) || word_break == WordBreak::BreakWord)
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
    source_start_token: usize,
    text_source: &[InlineTextSource],
    height: f32,
    cursor: &mut InlineCursor,
    lines: &mut Vec<LineBox>,
    fragments: &mut Vec<InlineFragment>,
    align: TextAlign,
    line_constraints: Option<&dyn Fn(f32, f32) -> (f32, f32)>,
) {
    let mut text_byte = 0;
    for (index, ch_str) in split_chars(text).into_iter().enumerate() {
        let source = text_source::slice(text_source, text, text_byte..text_byte + ch_str.len());
        text_byte += ch_str.len();
        let ch_width = measure_text_width(&ch_str, &segment.metrics);
        if cursor.x > cursor.start_x
            && exceeds_available_inline_width(
                cursor.x + ch_width - cursor.start_x,
                cursor.available_width,
            )
        {
            cursor.wrap_line(
                lines,
                fragments,
                0.0,
                align,
                line_constraints,
                LineBreakKind::Soft,
            );
        }
        fragments.push(InlineFragment {
            node: segment.node.clone(),
            text_source: source,
            content: InlineFragmentContent::Text(ch_str),
            rect: Rect {
                x: cursor.x,
                y: cursor.y,
                width: ch_width,
                height,
            },
            autospace_after: 0.0,
            punctuation_trim: Default::default(),
            metrics: segment.metrics.clone(),
            vertical_align: segment.vertical_align,
            source_end_token: source_start_token + index + 1,
            style: segment.style.clone(),
        });
        cursor.x += ch_width;
        cursor.line_height = cursor.line_height.max(segment.line_height.max(height));
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_hyphenated_text(
    text: &str,
    segment: &InlineSegment,
    source_start_token: usize,
    text_source: &[InlineTextSource],
    height: f32,
    cursor: &mut InlineCursor,
    lines: &mut Vec<LineBox>,
    fragments: &mut Vec<InlineFragment>,
    align: TextAlign,
    line_constraints: Option<&dyn Fn(f32, f32) -> (f32, f32)>,
) -> bool {
    let language = inherited_hyphenation_language(&segment.node);
    if hyphenation_opportunities(text, segment.style.hyphens, language).is_empty() {
        return false;
    }
    let hyphen = match &segment.style.hyphenate_character {
        super::HyphenateCharacter::String(value) => value.clone(),
        super::HyphenateCharacter::Auto => {
            if language.is_some_and(|language| language.hyphenation_character().is_none()) {
                String::new()
            } else {
                "‐".to_string()
            }
        }
    };
    if hyphen.is_empty() {
        return false;
    }
    let hyphen_width = measure_text_width(&hyphen, &segment.metrics);
    let mut byte_start = 0;
    let mut broke = false;

    while byte_start < text.len() {
        let remaining = &text[byte_start..];
        let remaining_width = measure_text_width(remaining, &segment.metrics);
        let used = cursor.x - cursor.start_x;
        if !exceeds_available_inline_width(used + remaining_width, cursor.available_width) {
            if broke {
                push_hyphenated_fragment(
                    segment,
                    remaining.to_string(),
                    text_source::slice(text_source, text, byte_start..text.len()),
                    remaining_width,
                    height,
                    source_start_token + text[..byte_start].graphemes(true).count(),
                    cursor,
                    fragments,
                );
            }
            return broke;
        }

        let opportunities = hyphenation_opportunities(remaining, segment.style.hyphens, language);
        let fitting = opportunities.into_iter().rev().find(|offset| {
            *offset < remaining.len()
                && !exceeds_available_inline_width(
                    used + measure_text_width(&remaining[..*offset], &segment.metrics)
                        + hyphen_width,
                    cursor.available_width,
                )
        });
        if let Some(offset) = fitting {
            let byte_end = byte_start + offset;
            let prefix = &text[byte_start..byte_end];
            let token_end = source_start_token + text[..byte_end].graphemes(true).count();
            push_hyphenated_fragment(
                segment,
                prefix.to_string(),
                text_source::slice(text_source, text, byte_start..byte_end),
                measure_text_width(prefix, &segment.metrics),
                height,
                token_end,
                cursor,
                fragments,
            );
            push_hyphenated_fragment(
                segment,
                hyphen.clone(),
                Vec::new(),
                hyphen_width,
                height,
                token_end,
                cursor,
                fragments,
            );
            cursor.wrap_line(
                lines,
                fragments,
                height,
                align,
                line_constraints,
                LineBreakKind::Soft,
            );
            byte_start = byte_end;
            broke = true;
            continue;
        }

        if cursor.x > cursor.start_x {
            cursor.wrap_line(
                lines,
                fragments,
                height,
                align,
                line_constraints,
                LineBreakKind::Soft,
            );
            continue;
        }
        if broke {
            push_hyphenated_fragment(
                segment,
                remaining.to_string(),
                text_source::slice(text_source, text, byte_start..text.len()),
                remaining_width,
                height,
                source_start_token + text.graphemes(true).count(),
                cursor,
                fragments,
            );
        }
        return broke;
    }
    broke
}

#[allow(clippy::too_many_arguments)]
fn push_hyphenated_fragment(
    segment: &InlineSegment,
    text: String,
    text_source: Vec<InlineTextSource>,
    width: f32,
    height: f32,
    source_end_token: usize,
    cursor: &mut InlineCursor,
    fragments: &mut Vec<InlineFragment>,
) {
    fragments.push(InlineFragment {
        node: segment.node.clone(),
        text_source,
        content: InlineFragmentContent::Text(text),
        rect: Rect {
            x: cursor.x,
            y: cursor.y,
            width,
            height,
        },
        autospace_after: 0.0,
        punctuation_trim: Default::default(),
        metrics: segment.metrics.clone(),
        vertical_align: segment.vertical_align,
        source_end_token,
        style: segment.style.clone(),
    });
    cursor.x += width;
    cursor.line_height = cursor.line_height.max(segment.line_height.max(height));
}

/// A prepared inline piece with independent DOM and pagination positions.
struct InlineFragmentInput {
    content: InlineFragmentContent,
    width: f32,
    height: f32,
    autospace_after: f32,
    token_range: std::ops::Range<usize>,
    text_source: Vec<InlineTextSource>,
}

impl InlineFragmentInput {
    fn new(
        segment: &InlineSegment,
        piece: InlinePiece,
        text_byte: &mut usize,
        source_token: &mut usize,
    ) -> Self {
        let (content, width, height, autospace_after) = match piece {
            InlinePiece::Fragment {
                content,
                width,
                height,
            } => (content, width, height, 0.0),
            InlinePiece::AutospaceReplacement {
                text,
                width,
                height,
            } => (InlineFragmentContent::Text(text), width, height, width),
            InlinePiece::Newline => unreachable!(),
        };
        let start_byte = *text_byte;
        let text_source = match (&segment.content, &content) {
            (InlineSegmentContent::Text(original), InlineFragmentContent::Text(text)) => {
                *text_byte += text.len();
                text_source::slice(&segment.text_source, original, start_byte..*text_byte)
            }
            _ => Vec::new(),
        };
        let start_token = *source_token;
        *source_token += match &content {
            InlineFragmentContent::Text(text) => text.graphemes(true).count(),
            InlineFragmentContent::InlineEdge(_, _) | InlineFragmentContent::InlineSpacing(_) => 0,
            _ => 1,
        };
        Self {
            content,
            width,
            height,
            autospace_after,
            token_range: start_token..*source_token,
            text_source,
        }
    }
}

fn layout_inline_fragment(
    input: InlineFragmentInput,
    segment: &InlineSegment,
    prev_segment_allows_wrapping: bool,
    is_first_piece_in_segment: &mut bool,
    cursor: &mut InlineCursor,
    lines: &mut Vec<LineBox>,
    fragments: &mut Vec<InlineFragment>,
    align: TextAlign,
    line_constraints: Option<&dyn Fn(f32, f32) -> (f32, f32)>,
) {
    let InlineFragmentInput {
        content,
        width,
        height,
        autospace_after,
        token_range,
        text_source: piece_source,
    } = input;
    let piece_start_token = token_range.start;
    let source_token = token_range.end;
    let overflow_wrap = segment.overflow_wrap;
    let allows_wrapping = segment.white_space_mode.allows_wrapping();
    let collapsible_whitespace = segment.white_space_mode.collapses_whitespace()
        && matches!(&content, InlineFragmentContent::Text(text) if text
            .chars()
            .all(|ch| ch != '\u{00A0}' && ch.is_whitespace()));
    if cursor.x == cursor.start_x && collapsible_whitespace {
        return;
    }
    let can_wrap = if *is_first_piece_in_segment {
        prev_segment_allows_wrapping
    } else {
        allows_wrapping
    };
    *is_first_piece_in_segment = false;

    cursor.x += apply_pending_autospace_replacement(fragments, &content, segment);

    let candidate_used_width =
        cursor.x + autospace_before_fragment(fragments, &content, segment) + width - cursor.start_x;
    let allow_end_delta = allow_end_trim_delta(&content, segment);
    let candidate_fits_with_allow_end = allow_end_delta < 0.0
        && !exceeds_available_inline_width(
            candidate_used_width + allow_end_delta,
            cursor.available_width,
        );
    if can_wrap
        && let InlineFragmentContent::Text(text) = &content
        && layout_hyphenated_text(
            text,
            segment,
            piece_start_token,
            &piece_source,
            height,
            cursor,
            lines,
            fragments,
            align,
            line_constraints,
        )
    {
        return;
    }

    if can_wrap
        && !matches!(
            content,
            InlineFragmentContent::InlineEdge(_, _) | InlineFragmentContent::InlineSpacing(_)
        )
        && cursor.x > cursor.start_x
        && french_autospace_before_fragment(fragments, &content, segment).is_none()
        && exceeds_available_inline_width(candidate_used_width, cursor.available_width)
        && !candidate_fits_with_allow_end
    {
        // Collapsible whitespace at the end of a line is
        // discarded. Do not let it create an otherwise empty
        // continuation line before an inline closing edge.
        if collapsible_whitespace {
            return;
        }
        let pending_starts = take_trailing_inline_starts(fragments, &mut cursor.x);
        cursor.wrap_line(
            lines,
            fragments,
            0.0,
            align,
            line_constraints,
            LineBreakKind::Soft,
        );
        for mut fragment in pending_starts {
            fragment.rect.x = cursor.x;
            fragment.rect.y = cursor.y;
            cursor.x += fragment.rect.width;
            fragments.push(fragment);
        }
    }

    if needs_character_break(
        overflow_wrap,
        segment.word_break,
        allows_wrapping,
        cursor.x,
        cursor.start_x,
        width,
        cursor.available_width,
    ) && let InlineFragmentContent::Text(text) = content
    {
        break_text_by_characters(
            &text,
            segment,
            piece_start_token,
            &piece_source,
            height,
            cursor,
            lines,
            fragments,
            align,
            line_constraints,
        );
        return;
    }

    let autospace = autospace_before_fragment(fragments, &content, segment);
    if autospace > 0.0
        && let Some(previous) = previous_autospace_text_fragment_mut(fragments)
    {
        previous.rect.width += autospace;
        previous.autospace_after += autospace;
        cursor.x += autospace;
    }

    fragments.push(InlineFragment {
        node: segment.node.clone(),
        text_source: piece_source,
        content,
        rect: Rect {
            x: cursor.x,
            y: cursor.y,
            width,
            height,
        },
        autospace_after,
        punctuation_trim: Default::default(),
        metrics: segment.metrics.clone(),
        vertical_align: segment.vertical_align,
        source_end_token: source_token,
        style: segment.style.clone(),
    });
    cursor.x += width;
    cursor.line_height = cursor.line_height.max(segment.line_height.max(height));
}

fn allow_end_trim_delta(content: &InlineFragmentContent, segment: &InlineSegment) -> f32 {
    let InlineFragmentContent::Text(text) = content else {
        return 0.0;
    };
    let Some(last) = text.chars().next_back() else {
        return 0.0;
    };
    if segment.metrics.text_spacing_trim.end != super::TextSpacingTrimEnd::Allow
        || super::fullwidth_punctuation_class(last, segment.metrics.vertical_writing)
            != Some(super::FullwidthPunctuationClass::Closing)
    {
        return 0.0;
    }
    measure_text_width_with_trim_edges(
        text,
        &segment.metrics,
        super::PunctuationTrim {
            first: false,
            last: true,
        },
    ) - measure_text_width(text, &segment.metrics)
}

fn autospace_before_fragment(
    fragments: &[InlineFragment],
    content: &InlineFragmentContent,
    segment: &InlineSegment,
) -> f32 {
    if let Some(spacing) = french_autospace_before_fragment(fragments, content, segment) {
        return spacing;
    }
    let InlineFragmentContent::Text(text) = content else {
        return 0.0;
    };
    if text_is_combined_upright(text, &segment.style) {
        return 0.0;
    }
    let Some(current) = text.chars().next() else {
        return 0.0;
    };
    let Some(previous) = previous_autospace_text_fragment(fragments).and_then(|fragment| {
        let text = fragment.text()?;
        (!text_is_combined_upright(text, &fragment.style))
            .then(|| text.chars().next_back())
            .flatten()
    }) else {
        return 0.0;
    };
    fragment_autospace_pair_applies(previous, current, &segment.style)
        .then_some(segment.metrics.font_size * 0.125)
        .unwrap_or(0.0)
}

fn french_autospace_before_fragment(
    fragments: &[InlineFragment],
    content: &InlineFragmentContent,
    segment: &InlineSegment,
) -> Option<f32> {
    let current = match content {
        InlineFragmentContent::Text(text) => text.chars().next()?,
        _ => return None,
    };
    let previous = previous_autospace_text_fragment(fragments)?
        .text()?
        .chars()
        .next_back()?;
    let spacing = fragment_french_autospace_pair(previous, current, &segment.style)?;
    Some(measure_text_width(
        &spacing.character().to_string(),
        &segment.metrics,
    ))
}

fn previous_autospace_text_fragment(fragments: &[InlineFragment]) -> Option<&InlineFragment> {
    for fragment in fragments.iter().rev() {
        if fragment.text().is_some() {
            return Some(fragment);
        }
        if !matches!(
            &fragment.content,
            InlineFragmentContent::InlineEdge(_, _) | InlineFragmentContent::InlineSpacing(_)
        ) || fragment.rect.width != 0.0
        {
            return None;
        }
    }
    None
}

fn previous_autospace_text_fragment_mut(
    fragments: &mut [InlineFragment],
) -> Option<&mut InlineFragment> {
    let index = fragments.iter().rposition(|fragment| {
        fragment.text().is_some()
            || !matches!(
                &fragment.content,
                InlineFragmentContent::InlineEdge(_, _) | InlineFragmentContent::InlineSpacing(_)
            )
            || fragment.rect.width != 0.0
    })?;
    if fragments[index].text().is_some() {
        Some(&mut fragments[index])
    } else {
        None
    }
}

fn apply_pending_autospace_replacement(
    fragments: &mut [InlineFragment],
    content: &InlineFragmentContent,
    segment: &InlineSegment,
) -> f32 {
    if !segment.style.text_autospace.replace {
        return 0.0;
    }
    let Some(current) = (match content {
        InlineFragmentContent::Text(text) => text.chars().next(),
        _ => None,
    }) else {
        return 0.0;
    };
    let mut index = fragments.len();
    while index > 0 && is_zero_width_inline_boundary(&fragments[index - 1]) {
        index -= 1;
    }
    let Some(space_index) = index.checked_sub(1) else {
        return 0.0;
    };
    let Some(space_text) = fragments[space_index].text() else {
        return 0.0;
    };
    if space_text.is_empty()
        || !space_text.chars().all(|character| character == ' ')
        || !fragments[space_index].style.text_autospace.replace
    {
        return 0.0;
    }
    index = space_index;
    while index > 0 && is_zero_width_inline_boundary(&fragments[index - 1]) {
        index -= 1;
    }
    let Some(previous) = index
        .checked_sub(1)
        .and_then(|index| fragments[index].text())
        .and_then(|text| text.chars().next_back())
    else {
        return 0.0;
    };
    let Some(width) = fragment_french_autospace_pair(previous, current, &segment.style)
        .map(|spacing| measure_text_width(&spacing.character().to_string(), &segment.metrics))
        .or_else(|| {
            fragment_autospace_pair_applies(previous, current, &segment.style)
                .then_some(segment.metrics.font_size * 0.125)
        })
    else {
        return 0.0;
    };
    let delta = width - fragments[space_index].rect.width;
    fragments[space_index].rect.width = width;
    fragments[space_index].autospace_after = width;
    for fragment in &mut fragments[space_index + 1..] {
        fragment.rect.x += delta;
    }
    delta
}

fn is_zero_width_inline_boundary(fragment: &InlineFragment) -> bool {
    matches!(
        &fragment.content,
        InlineFragmentContent::InlineEdge(_, _) | InlineFragmentContent::InlineSpacing(_)
    ) && fragment.rect.width == 0.0
}

fn layout_inline_newline(
    segment: &InlineSegment,
    source_token: usize,
    cursor: &mut InlineCursor,
    lines: &mut Vec<LineBox>,
    fragments: &mut Vec<InlineFragment>,
    align: TextAlign,
    line_constraints: Option<&dyn Fn(f32, f32) -> (f32, f32)>,
) {
    // An explicit line break occupies a line even when no
    // glyph precedes it. Retain its owner for empty pre lines.
    fragments.push(InlineFragment {
        node: segment.node.clone(),
        text_source: Vec::new(),
        content: InlineFragmentContent::Text(String::new()),
        rect: Rect {
            x: cursor.x,
            y: cursor.y,
            width: 0.0,
            height: segment.line_height,
        },
        autospace_after: 0.0,
        punctuation_trim: Default::default(),
        metrics: segment.metrics.clone(),
        vertical_align: segment.vertical_align,
        source_end_token: source_token,
        style: segment.style.clone(),
    });
    cursor.wrap_line(
        lines,
        fragments,
        segment.line_height,
        align,
        line_constraints,
        LineBreakKind::Forced,
    );
}

fn layout_inline_segments(
    segments: &[InlineSegment],
    source_start_token: usize,
    start_x: f32,
    start_y: f32,
    available_width: f32,
    align: TextAlign,
    last_align: TextAlign,
    strut_line_height: f32,
    strut_metrics: Option<FontMetrics>,
    direction_rtl: bool,
    first_line_indent: TextIndent,
    line_constraints: Option<&dyn Fn(f32, f32) -> (f32, f32)>,
    wrap_width: Option<f32>,
) -> Vec<LineBox> {
    let mut lines = Vec::new();
    let mut current_fragments = Vec::new();
    let mut cursor = InlineCursor::new(
        start_x,
        start_y,
        available_width,
        wrap_width,
        strut_line_height,
        strut_metrics.clone(),
        direction_rtl,
        first_line_indent,
        last_align,
    );

    let mut prev_segment_allows_wrapping = true;
    let mut source_token = source_start_token;
    for segment in segments {
        let allows_wrapping = segment.white_space_mode.allows_wrapping();
        let mut is_first_piece_in_segment = true;
        let mut text_byte = 0;
        for piece in split_segment(segment) {
            match piece {
                InlinePiece::Newline => {
                    text_byte += 1;
                    source_token += 1;
                    layout_inline_newline(
                        segment,
                        source_token,
                        &mut cursor,
                        &mut lines,
                        &mut current_fragments,
                        align,
                        line_constraints,
                    );
                }
                mut piece @ (InlinePiece::Fragment { .. }
                | InlinePiece::AutospaceReplacement { .. }) => {
                    if let InlinePiece::Fragment {
                        content: InlineFragmentContent::Text(text),
                        width,
                        ..
                    } = &mut piece
                        && text == "\t"
                    {
                        *width = segment
                            .white_space_mode
                            .tab_advance(cursor.x - cursor.start_x, &segment.metrics);
                    }
                    let input =
                        InlineFragmentInput::new(segment, piece, &mut text_byte, &mut source_token);
                    layout_inline_fragment(
                        input,
                        segment,
                        prev_segment_allows_wrapping,
                        &mut is_first_piece_in_segment,
                        &mut cursor,
                        &mut lines,
                        &mut current_fragments,
                        align,
                        line_constraints,
                    );
                }
            }
        }
        prev_segment_allows_wrapping = allows_wrapping;
    }

    cursor.finish_line(&mut lines, &mut current_fragments, align);

    lines
}

#[allow(clippy::too_many_arguments)]
fn optimize_text_wrap_style(
    segments: &[InlineSegment],
    source_start_token: usize,
    start_x: f32,
    start_y: f32,
    available_width: f32,
    align: TextAlign,
    last_align: TextAlign,
    strut_line_height: f32,
    strut_metrics: Option<FontMetrics>,
    direction_rtl: bool,
    first_line_indent: TextIndent,
    line_constraints: Option<&dyn Fn(f32, f32) -> (f32, f32)>,
    lines: Vec<LineBox>,
) -> Vec<LineBox> {
    let style = segments
        .first()
        .map(|segment| segment.style.text_wrap_style)
        .unwrap_or_default();
    let has_forced_break = segments.iter().any(|segment| {
        matches!(&segment.content, InlineSegmentContent::Text(text) if text.contains('\n') && segment.white_space_mode.preserves_newlines())
    });
    let should_optimize = match style {
        // CSS Text permits a UA line-count limit for balancing. Keep this
        // implementation bounded to the short headings the value targets.
        super::TextWrapStyle::Balance => (2..=6).contains(&lines.len()),
        super::TextWrapStyle::Pretty => {
            lines.len() >= 2
                && lines.last().is_some_and(|last| {
                    let previous = &lines[lines.len() - 2];
                    last.rect.width * 2.0 < previous.rect.width
                })
        }
        // `stable` only changes line selection while a line is actively being
        // edited. Omoikane currently has no editing-line layout context, so its
        // non-editing behavior is the ordinary greedy algorithm.
        super::TextWrapStyle::Auto | super::TextWrapStyle::Stable => false,
    };
    if !should_optimize || has_forced_break || available_width <= 0.0 {
        return lines;
    }

    let target_line_count = lines.len();
    let mut lower = lines
        .iter()
        .map(|line| line.rect.width)
        .fold(0.0_f32, f32::max)
        / target_line_count as f32;
    let mut upper = available_width;
    let mut best = lines;
    // Find the narrowest soft-wrap measure that preserves the original line
    // count. Alignment still uses `available_width`, so centered/right-aligned
    // text remains positioned in the original containing block.
    for _ in 0..12 {
        let candidate_width = (lower + upper) / 2.0;
        let candidate = layout_inline_segments(
            segments,
            source_start_token,
            start_x,
            start_y,
            available_width,
            align,
            last_align,
            strut_line_height,
            strut_metrics.clone(),
            direction_rtl,
            first_line_indent,
            line_constraints,
            Some(candidate_width),
        );
        if candidate.len() > target_line_count {
            lower = candidate_width;
        } else {
            upper = candidate_width;
            best = candidate;
        }
    }
    best
}

enum InlinePiece {
    Newline,
    AutospaceReplacement {
        text: String,
        width: f32,
        height: f32,
    },
    Fragment {
        content: InlineFragmentContent,
        width: f32,
        height: f32,
    },
}

fn inline_edge_size(style: &ComputedStyle, edges: EdgeSizes, start: bool) -> f32 {
    if super::is_vertical_writing(style) {
        match (super::direction_is_rtl(style), start) {
            (false, true) | (true, false) => edges.top,
            (false, false) | (true, true) => edges.bottom,
        }
    } else {
        match (super::direction_is_rtl(style), start) {
            (false, true) | (true, false) => edges.left,
            (false, false) | (true, true) => edges.right,
        }
    }
}

fn take_trailing_inline_starts(
    fragments: &mut Vec<InlineFragment>,
    cursor_x: &mut f32,
) -> Vec<InlineFragment> {
    let mut split = fragments.len();
    while split > 0
        && matches!(
            fragments[split - 1].content,
            InlineFragmentContent::InlineEdge(_, true) | InlineFragmentContent::InlineSpacing(true)
        )
    {
        split -= 1;
    }
    let pending = fragments.split_off(split);
    *cursor_x -= pending
        .iter()
        .map(|fragment| fragment.rect.width)
        .sum::<f32>();
    pending
}

fn split_segment(segment: &InlineSegment) -> Vec<InlinePiece> {
    match &segment.content {
        InlineSegmentContent::Text(text) => {
            let mut pieces = split_text_segment(
                text,
                &segment.metrics,
                segment.line_height,
                segment.word_break,
                segment.style.line_break,
                segment.white_space_mode,
                &segment.style,
            );
            apply_autospace_replacements(&mut pieces, segment);
            pieces
        }
        InlineSegmentContent::InlineEdge(style, start) => {
            let margin = inline_edge_size(style, edge_sizes(style, "margin"), *start);
            let border_box_edge = InlinePiece::Fragment {
                content: InlineFragmentContent::InlineEdge(style.clone(), *start),
                width: inline_edge_size(style, edge_sizes(style, "padding"), *start)
                    + inline_edge_size(style, edge_sizes(style, "border"), *start),
                height: 0.0,
            };
            if margin == 0.0 {
                vec![border_box_edge]
            } else {
                let margin = InlinePiece::Fragment {
                    content: InlineFragmentContent::InlineSpacing(*start),
                    width: margin,
                    height: 0.0,
                };
                if *start {
                    vec![margin, border_box_edge]
                } else {
                    vec![border_box_edge, margin]
                }
            }
        }
        InlineSegmentContent::AtomicInline(width, height, baseline) => {
            vec![InlinePiece::Fragment {
                content: InlineFragmentContent::AtomicInline(*baseline),
                width: *width,
                height: *height,
            }]
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
        InlineSegmentContent::ValueIndicator(style, state, content_width, content_height) => {
            let padding = edge_sizes(style, "padding");
            let border = edge_sizes(style, "border");
            vec![InlinePiece::Fragment {
                content: InlineFragmentContent::ValueIndicator(style.clone(), *state),
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
                    style.clone(),
                    image.clone(),
                    *icon_width,
                    *icon_height,
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

fn apply_autospace_replacements(pieces: &mut [InlinePiece], segment: &InlineSegment) {
    if !segment.style.text_autospace.replace || pieces.len() < 3 {
        return;
    }
    let replacements = (1..pieces.len() - 1)
        .filter_map(|index| {
            let text = match &pieces[index] {
                InlinePiece::Fragment {
                    content: InlineFragmentContent::Text(text),
                    ..
                } => text,
                _ => return None,
            };
            if text.is_empty() || !text.chars().all(|character| character == ' ') {
                return None;
            }
            let left = match &pieces[index - 1] {
                InlinePiece::Fragment {
                    content: InlineFragmentContent::Text(text),
                    ..
                } => text.chars().next_back(),
                _ => None,
            };
            let right = match &pieces[index + 1] {
                InlinePiece::Fragment {
                    content: InlineFragmentContent::Text(text),
                    ..
                } => text.chars().next(),
                _ => None,
            };
            let (left, right) = left.zip(right)?;
            let width = fragment_french_autospace_pair(left, right, &segment.style)
                .map(|spacing| {
                    measure_text_width(&spacing.character().to_string(), &segment.metrics)
                })
                .or_else(|| {
                    fragment_autospace_pair_applies(left, right, &segment.style)
                        .then_some(segment.metrics.font_size * 0.125)
                })?;
            Some((index, width))
        })
        .collect::<Vec<_>>();
    for (index, width) in replacements {
        let old = std::mem::replace(&mut pieces[index], InlinePiece::Newline);
        let InlinePiece::Fragment {
            content: InlineFragmentContent::Text(text),
            height,
            ..
        } = old
        else {
            unreachable!("replacement candidates are text fragments")
        };
        pieces[index] = InlinePiece::AutospaceReplacement {
            text,
            width,
            height,
        };
    }
}

fn split_text_segment(
    text: &str,
    metrics: &FontMetrics,
    line_height: f32,
    wb: WordBreak,
    line_break: super::LineBreak,
    ws_mode: WhiteSpaceMode,
    style: &FragmentStyle,
) -> Vec<InlinePiece> {
    if vertical_text_combine_applies(style) {
        match style.text_combine_upright {
            super::TextCombineUpright::All => {
                return split_all_combined_text(text, metrics, line_height, ws_mode);
            }
            super::TextCombineUpright::Digits(maximum) => {
                return split_digit_combined_text(
                    text,
                    metrics,
                    line_height,
                    wb,
                    line_break,
                    ws_mode,
                    style,
                    maximum,
                );
            }
            super::TextCombineUpright::None => {}
        }
    }
    split_uncombined_text_segment(text, metrics, line_height, wb, line_break, ws_mode, style)
}

fn vertical_text_combine_applies(style: &FragmentStyle) -> bool {
    matches!(
        style.writing_mode.as_deref(),
        Some("vertical-rl" | "vertical-lr" | "sideways-rl" | "sideways-lr")
    )
}

fn combined_text_piece(text: &str, metrics: &FontMetrics, line_height: f32) -> InlinePiece {
    InlinePiece::Fragment {
        content: InlineFragmentContent::Text(text.to_string()),
        width: metrics.font_size,
        height: line_height,
    }
}

fn split_all_combined_text(
    text: &str,
    metrics: &FontMetrics,
    line_height: f32,
    ws_mode: WhiteSpaceMode,
) -> Vec<InlinePiece> {
    if !ws_mode.preserves_newlines() || !text.contains('\n') {
        return (!text.is_empty())
            .then(|| combined_text_piece(text, metrics, line_height))
            .into_iter()
            .collect();
    }
    let mut pieces = Vec::new();
    let line_count = text.split('\n').count();
    for (index, part) in text.split('\n').enumerate() {
        if !part.is_empty() {
            pieces.push(combined_text_piece(part, metrics, line_height));
        }
        if index + 1 < line_count {
            pieces.push(InlinePiece::Newline);
        }
    }
    pieces
}

#[allow(clippy::too_many_arguments)]
fn split_digit_combined_text(
    text: &str,
    metrics: &FontMetrics,
    line_height: f32,
    wb: WordBreak,
    line_break: super::LineBreak,
    ws_mode: WhiteSpaceMode,
    style: &FragmentStyle,
    maximum: u8,
) -> Vec<InlinePiece> {
    let mut pieces = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut chars = text[start..].char_indices();
        let (_, first) = chars.next().expect("start is a character boundary");
        let digits = first.is_ascii_digit();
        let end = chars
            .find_map(|(offset, ch)| (ch.is_ascii_digit() != digits).then_some(start + offset))
            .unwrap_or(text.len());
        let run = &text[start..end];
        if digits && (2..=maximum as usize).contains(&run.len()) {
            pieces.push(combined_text_piece(run, metrics, line_height));
        } else {
            pieces.extend(split_uncombined_text_segment(
                run,
                metrics,
                line_height,
                wb,
                line_break,
                ws_mode,
                style,
            ));
        }
        start = end;
    }
    pieces
}

fn split_uncombined_text_segment(
    text: &str,
    metrics: &FontMetrics,
    line_height: f32,
    wb: WordBreak,
    line_break: super::LineBreak,
    ws_mode: WhiteSpaceMode,
    style: &FragmentStyle,
) -> Vec<InlinePiece> {
    // `word-break: break-word` is a non-standard value treated identically to
    // `word-break: normal` for segment-level splitting; the actual emergency
    // break behaviour is handled in the line-building loop (same as
    // `overflow-wrap: break-word`).
    let split = |text| split_for_break_style(text, wb, line_break);

    // A tab advances to a cursor-relative stop, so it cannot be measured with
    // the surrounding shaped text. Keep it as an independent piece whose used
    // width is filled in by the line builder once the current inline position
    // is known.
    if text.contains('\t') {
        let parts = text.split('\t').collect::<Vec<_>>();
        let mut pieces = Vec::new();
        for (index, part) in parts.iter().enumerate() {
            if !part.is_empty() {
                pieces.extend(split_text_segment(
                    part,
                    metrics,
                    line_height,
                    wb,
                    line_break,
                    ws_mode,
                    style,
                ));
            }
            if index + 1 < parts.len() {
                pieces.push(InlinePiece::Fragment {
                    content: InlineFragmentContent::Text("\t".to_string()),
                    width: 0.0,
                    height: line_height,
                });
            }
        }
        return pieces;
    }

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
        return split(text)
            .into_iter()
            .map(|piece| InlinePiece::Fragment {
                width: measure_text_width(&piece, metrics),
                height: line_height,
                content: InlineFragmentContent::Text(piece),
            })
            .collect();
    }

    if ws_mode.preserves_newlines() && text.contains('\n') {
        let mut pieces = Vec::new();
        let line_count = text.split('\n').count();
        for (index, part) in text.split('\n').enumerate() {
            if !part.is_empty() {
                pieces.extend(split(part).into_iter().map(|piece| InlinePiece::Fragment {
                    width: measure_text_width(&piece, metrics),
                    height: line_height,
                    content: InlineFragmentContent::Text(piece),
                }));
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
                pieces.extend(split(part).into_iter().map(|piece| InlinePiece::Fragment {
                    width: measure_text_width(&piece, metrics),
                    height: line_height,
                    content: InlineFragmentContent::Text(piece),
                }));
            }
            if index + 1 < line_count {
                pieces.push(InlinePiece::Newline);
            }
        }
        pieces
    } else {
        split(text)
            .into_iter()
            .map(|piece| InlinePiece::Fragment {
                width: measure_text_width(&piece, metrics),
                height: line_height,
                content: InlineFragmentContent::Text(piece),
            })
            .collect()
    }
}

fn split_for_break_style(
    text: &str,
    word_break: WordBreak,
    line_break: super::LineBreak,
) -> Vec<String> {
    if line_break == super::LineBreak::Anywhere || word_break == WordBreak::BreakAll {
        return split_chars(text);
    }
    if word_break == WordBreak::KeepAll {
        return split_words_no_cjk_break(text);
    }
    split_words_preserving_spaces_cjk_with_mode(text, line_break)
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
fn is_line_start_prohibited(ch: char, line_break: super::LineBreak) -> bool {
    let common = matches!(
        ch,
        // Japanese punctuation
        '\u{3002}' | '\u{3001}' | '\u{FF0C}' | '\u{FF0E}' | '\u{30FB}' | '\u{FF1A}' | '\u{FF1B}' | '\u{FF01}' | '\u{FF1F}' |
        // Closing brackets
        '\u{FF09}' | '\u{300D}' | '\u{300F}' | '\u{3011}' | '\u{3015}' | '\u{FF5D}' | '\u{FF3D}' |
        // Other punctuation
        '\u{FF5E}' | '\u{2026}' | '\u{2025}'
    );
    let normal = matches!(
        ch,
        // Prolonged sound and iteration marks
        '\u{30FC}' | '\u{3005}' | '\u{303B}' | '\u{309D}' | '\u{309E}' | '\u{30FD}' | '\u{30FE}' |
        // Small kana
        '\u{3041}' | '\u{3043}' | '\u{3045}' | '\u{3047}' | '\u{3049}' | '\u{3063}' | '\u{3083}' | '\u{3085}' | '\u{3087}' | '\u{308E}' |
        '\u{30A1}' | '\u{30A3}' | '\u{30A5}' | '\u{30A7}' | '\u{30A9}' | '\u{30C3}' | '\u{30E3}' | '\u{30E5}' | '\u{30E7}' | '\u{30EE}'
    );
    let strict = matches!(ch, '\u{2010}' | '\u{2013}' | '\u{301C}' | '\u{30A0}');
    common
        || (!matches!(line_break, super::LineBreak::Loose) && normal)
        || (matches!(line_break, super::LineBreak::Strict) && strict)
}

/// Characters that must not appear at the end of a line (line-end prohibited).
/// These are typically opening punctuation.
fn is_line_end_prohibited(ch: char, _line_break: super::LineBreak) -> bool {
    matches!(
        ch,
        // Opening brackets
        '\u{FF08}' | '\u{300C}' | '\u{300E}' | '\u{3010}' | '\u{3014}' | '\u{FF5B}' | '\u{FF3B}'
    )
}

/// Split text into pieces that can be laid out, with CJK-aware breaking.
/// This allows line breaks between CJK characters while respecting kinsoku rules.
pub(crate) fn split_words_preserving_spaces_cjk(text: &str) -> Vec<String> {
    split_words_preserving_spaces_cjk_with_mode(text, super::LineBreak::Normal)
}

pub(super) fn split_words_preserving_spaces_cjk_with_mode(
    text: &str,
    line_break: super::LineBreak,
) -> Vec<String> {
    let mut pieces = Vec::new();
    for_each_word_preserving_spaces_cjk_with_mode(text, line_break, |piece| {
        pieces.push(piece.to_string())
    });
    pieces
}

/// Visits the same CJK-aware pieces as [`split_words_preserving_spaces_cjk`]
/// without allocating owned strings.
pub(super) fn for_each_word_preserving_spaces_cjk<'a>(text: &'a str, visit: impl FnMut(&'a str)) {
    for_each_word_preserving_spaces_cjk_with_mode(text, super::LineBreak::Normal, visit);
}

fn for_each_word_preserving_spaces_cjk_with_mode<'a>(
    text: &'a str,
    line_break: super::LineBreak,
    mut visit: impl FnMut(&'a str),
) {
    let mut piece_start = 0;
    let mut previous = None;

    for (offset, ch) in text.char_indices() {
        let is_space = ch == ' ';
        let is_cjk = is_cjk_char(ch);

        // Check if we should break before this character
        let should_break = if previous.is_none() {
            false
        } else if is_space {
            // Break before space if previous was not space
            previous != Some(' ')
        } else if is_cjk {
            let prev_char = previous.expect("non-first character has a predecessor");
            let prev_is_cjk = is_cjk_char(prev_char);
            let prev_is_space = prev_char == ' ';

            if prev_is_space {
                // Always break after space
                true
            } else if prev_is_cjk {
                // Between two CJK characters: check kinsoku rules
                // Don't break if current char is line-start prohibited
                // Don't break if previous char is line-end prohibited
                !is_line_start_prohibited(ch, line_break)
                    && !is_line_end_prohibited(prev_char, line_break)
            } else {
                // Transition from non-CJK to CJK
                true
            }
        } else {
            // Non-CJK, non-space character
            let prev_char = previous.expect("non-first character has a predecessor");
            let prev_is_space = prev_char == ' ';
            let prev_is_cjk = is_cjk_char(prev_char);

            if prev_is_space {
                true
            } else if prev_is_cjk {
                // Transition from CJK to non-CJK: allow break unless kinsoku
                !is_line_start_prohibited(ch, line_break)
            } else {
                false
            }
        };

        if should_break {
            visit(&text[piece_start..offset]);
            piece_start = offset;
        }
        previous = Some(ch);
    }

    if previous.is_some() {
        visit(&text[piece_start..]);
    }
}

// ── Line building ───────────────────────────────────────────────────────────

fn push_line(
    lines: &mut Vec<LineBox>,
    fragments: &mut Vec<InlineFragment>,
    x: f32,
    y: f32,
    mut width: f32,
    mut height: f32,
    available_width: f32,
    align: TextAlign,
    direction_rtl: bool,
    strut_line_height: f32,
    strut_metrics: Option<FontMetrics>,
    space_first_should_space: bool,
) -> f32 {
    let allow_end_should_trim = exceeds_available_inline_width(width, available_width);
    resolve_line_bidi_geometry(fragments, x, direction_rtl);
    width += apply_line_text_spacing_trim(
        fragments,
        direction_rtl,
        space_first_should_space,
        allow_end_should_trim,
    );
    let physical_align = match align {
        TextAlign::Start if direction_rtl => TextAlign::Right,
        TextAlign::Start => TextAlign::Left,
        TextAlign::End if direction_rtl => TextAlign::Left,
        TextAlign::End => TextAlign::Right,
        other => other,
    };
    if physical_align == TextAlign::JustifyInterWord {
        width = justify_inter_word(fragments, width, available_width);
    } else if physical_align == TextAlign::JustifyInterCharacter {
        width = justify_inter_character(fragments, width, available_width);
    }
    let offset_x = match physical_align {
        TextAlign::Left => 0.0,
        TextAlign::Right => (available_width - width).max(0.0),
        TextAlign::Center => (available_width - width).max(0.0) / 2.0,
        TextAlign::JustifyInterWord | TextAlign::JustifyInterCharacter => 0.0,
        TextAlign::Start | TextAlign::End => unreachable!("logical alignment must be resolved"),
    };
    for fragment in fragments.iter_mut() {
        fragment.rect.x += offset_x;
    }

    let has_atomic_inline = fragments
        .iter()
        .any(|fragment| matches!(fragment.content, InlineFragmentContent::AtomicInline(_)));
    let initial_height = height;
    let fragment_ascent = |fragment: &InlineFragment| match &fragment.content {
        InlineFragmentContent::AtomicInline(baseline) => *baseline,
        InlineFragmentContent::Image(_, _)
        | InlineFragmentContent::FormControl(_, _, _)
        | InlineFragmentContent::ValueIndicator(_, _)
            if fragment.rect.height >= initial_height =>
        {
            fragment.rect.height
        }
        _ => fragment.metrics.ascent,
    };
    let fragment_extents = |fragment: &InlineFragment| {
        let ascent = fragment_ascent(fragment);
        match &fragment.content {
            InlineFragmentContent::Text(_) => {
                let leading = (fragment.rect.height - ascent - fragment.metrics.descent) / 2.0;
                Some((leading + ascent, leading + fragment.metrics.descent))
            }
            InlineFragmentContent::AtomicInline(_)
            | InlineFragmentContent::Image(_, _)
            | InlineFragmentContent::FormControl(_, _, _)
            | InlineFragmentContent::ValueIndicator(_, _) => {
                Some((ascent, fragment.rect.height - ascent))
            }
            InlineFragmentContent::InlineEdge(_, _)
            | InlineFragmentContent::InlineSpacing(_)
            | InlineFragmentContent::InlineBox(_)
            | InlineFragmentContent::GeneratedBox(_)
            | InlineFragmentContent::IconFormControl(_, _, _, _) => None,
        }
    };
    let baseline = if let Some(metrics) = strut_metrics.filter(|_| height > 0.0) {
        let half_leading = (strut_line_height - metrics.ascent - metrics.descent) / 2.0;
        let mut above = half_leading + metrics.ascent;
        let mut below = half_leading + metrics.descent;
        let rounded_ascent = metrics.ascent.ceil();
        let rounded_descent = metrics.descent.ceil();
        let rounded_half_leading = (strut_line_height - rounded_ascent - rounded_descent) / 2.0;
        let mut extent_above = rounded_half_leading + rounded_ascent;
        let mut extent_below = rounded_half_leading + rounded_descent;
        for fragment in fragments.iter() {
            let Some((fragment_above, fragment_below)) = fragment_extents(fragment) else {
                continue;
            };
            let (fragment_extent_above, fragment_extent_below) =
                if matches!(fragment.content, InlineFragmentContent::Text(_)) {
                    let ascent = fragment.metrics.ascent.ceil();
                    let descent = fragment.metrics.descent.ceil();
                    let leading = (fragment.rect.height - ascent - descent) / 2.0;
                    (leading + ascent, leading + descent)
                } else {
                    (fragment_above, fragment_below)
                };
            match fragment.vertical_align {
                VerticalAlign::Baseline => {
                    above = above.max(fragment_above);
                    below = below.max(fragment_below);
                    extent_above = extent_above.max(fragment_extent_above);
                    extent_below = extent_below.max(fragment_extent_below);
                }
                VerticalAlign::Length(shift) => {
                    above = above.max(fragment_above + shift);
                    below = below.max(fragment_below - shift);
                    extent_above = extent_above.max(fragment_extent_above + shift);
                    extent_below = extent_below.max(fragment_extent_below - shift);
                }
                VerticalAlign::Top | VerticalAlign::Middle | VerticalAlign::Bottom => {}
            }
        }
        height = height.max(extent_above + extent_below);
        above
    } else {
        fragments
            .iter()
            .filter_map(|fragment| match fragment.vertical_align {
                VerticalAlign::Baseline | VerticalAlign::Length(_) => {
                    fragment_extents(fragment).map(|(above, _)| above)
                }
                _ => None,
            })
            .reduce(f32::max)
            .unwrap_or(if has_atomic_inline { 0.0 } else { height * 0.8 })
    };

    for fragment in fragments.iter_mut() {
        fragment.rect.y = match fragment.vertical_align {
            VerticalAlign::Baseline => {
                let ascent = fragment_ascent(fragment);
                y + baseline - ascent
            }
            VerticalAlign::Length(shift) => {
                let ascent = fragment_ascent(fragment);
                y + baseline - ascent - shift
            }
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
        text_overflow: None,
    });
    height
}

fn justify_inter_word(
    fragments: &mut [InlineFragment],
    used_width: f32,
    available_width: f32,
) -> f32 {
    let extra = available_width - used_width;
    if extra <= 0.0 {
        return used_width;
    }
    let opportunities = fragments
        .iter()
        .filter(|fragment| {
            matches!(&fragment.content, InlineFragmentContent::Text(text)
                if !text.is_empty() && text.chars().all(char::is_whitespace))
        })
        .count();
    if opportunities == 0 {
        return used_width;
    }
    let expansion = extra / opportunities as f32;
    let mut shift = 0.0;
    for fragment in fragments {
        fragment.rect.x += shift;
        if matches!(&fragment.content, InlineFragmentContent::Text(text)
            if !text.is_empty() && text.chars().all(char::is_whitespace))
        {
            fragment.rect.width += expansion;
            shift += expansion;
        }
    }
    available_width
}

fn justify_inter_character(
    fragments: &mut Vec<InlineFragment>,
    used_width: f32,
    available_width: f32,
) -> f32 {
    let extra = available_width - used_width;
    if extra <= 0.0 {
        return used_width;
    }

    let grapheme_count = fragments
        .iter()
        .filter_map(InlineFragment::text)
        .map(|text| text.graphemes(true).count())
        .sum::<usize>();
    let opportunities = grapheme_count.saturating_sub(1);
    if opportunities == 0 {
        return used_width;
    }

    let mut split = Vec::with_capacity(fragments.len() + opportunities);
    for fragment in std::mem::take(fragments) {
        let Some(text) = fragment.text() else {
            split.push(fragment);
            continue;
        };
        let measured = text
            .grapheme_indices(true)
            .map(|(_, grapheme)| measure_text_width(grapheme, &fragment.metrics))
            .collect::<Vec<_>>();
        let measured_total = measured.iter().sum::<f32>();
        let scale = if measured_total > 0.0 {
            fragment.rect.width / measured_total
        } else {
            1.0
        };
        let mut x = fragment.rect.x;
        for ((start, grapheme), measured_width) in
            text.grapheme_indices(true).zip(measured.into_iter())
        {
            let mut unit = fragment.clone();
            let end = start + grapheme.len();
            unit.content = InlineFragmentContent::Text(grapheme.to_string());
            unit.text_source = text_source::slice(&fragment.text_source, text, start..end);
            unit.punctuation_trim = super::PunctuationTrim {
                first: fragment.punctuation_trim.first && start == 0,
                last: fragment.punctuation_trim.last && end == text.len(),
            };
            unit.rect.x = x;
            unit.rect.width = measured_width * scale;
            x += unit.rect.width;
            split.push(unit);
        }
    }

    let expansion = extra / opportunities as f32;
    let mut remaining_text_units = grapheme_count;
    let mut shift = 0.0;
    for fragment in &mut split {
        fragment.rect.x += shift;
        if fragment.text().is_some() {
            remaining_text_units -= 1;
            if remaining_text_units > 0 {
                fragment.rect.width += expansion;
                shift += expansion;
            }
        }
    }
    *fragments = split;
    available_width
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
    let needs_bidi = direction_rtl
        || fragments.iter().any(|fragment| {
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
        ordering_offsets.push(if prefix.is_empty() {
            start
        } else {
            fragment_start
        });
    }

    let paragraph_level = if direction_rtl {
        Level::rtl()
    } else {
        Level::ltr()
    };
    let bidi = BidiInfo::new(&bidi_source, Some(paragraph_level));
    let Some(paragraph) = bidi.paragraphs.first() else {
        return;
    };
    let (resolved_levels, _) = bidi.visual_runs(paragraph, paragraph.range.clone());
    let fragment_levels: Vec<Level> = ordering_offsets
        .iter()
        .map(|offset| {
            resolved_levels
                .get(*offset)
                .copied()
                .unwrap_or(paragraph_level)
        })
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

    // Line construction reserves autospace in logical order so it participates
    // in wrapping. CSS Text requires the actual spacing boundaries to be
    // selected after UAX#9 reordering, so discard those reservations and attach
    // the same kind of spacing to visually adjacent fragments instead.
    let replacements = fragments
        .iter()
        .map(|fragment| {
            fragment.autospace_after > 0.0
                && fragment
                    .text()
                    .is_some_and(|text| !text.is_empty() && text.chars().all(|ch| ch == ' '))
                && fragment.style.text_autospace.replace
        })
        .collect::<Vec<_>>();
    for fragment in fragments.iter_mut() {
        fragment.rect.width -= fragment.autospace_after;
        fragment.autospace_after = 0.0;
    }
    let mut visual_text_groups = vec![Vec::new()];
    for index in visual_order.iter().copied() {
        let fragment = &fragments[index];
        if fragment.text().is_some() {
            visual_text_groups.last_mut().unwrap().push(index);
        } else if !matches!(
            &fragment.content,
            InlineFragmentContent::InlineEdge(_, _) | InlineFragmentContent::InlineSpacing(_)
        ) || fragment.rect.width != 0.0
        {
            visual_text_groups.push(Vec::new());
        }
    }
    let visual_autospace = visual_text_groups
        .iter()
        .flat_map(|group| group.windows(2))
        .filter_map(|pair| {
            let left_index = pair[0];
            let right_index = pair[1];
            let left = &fragments[left_index];
            let right = &fragments[right_index];
            let left_text = left.text()?;
            let right_text = right.text()?;
            if text_is_combined_upright(left_text, &left.style)
                || text_is_combined_upright(right_text, &right.style)
            {
                return None;
            }
            let left_reversed = homogeneous_levels[left_index].is_some_and(|level| level.is_rtl());
            let right_reversed =
                homogeneous_levels[right_index].is_some_and(|level| level.is_rtl());
            let left_character = if left_reversed {
                left_text.chars().next()
            } else {
                left_text.chars().next_back()
            }?;
            let right_character = if right_reversed {
                right_text.chars().next_back()
            } else {
                right_text.chars().next()
            }?;
            let spacing =
                fragment_french_autospace_pair(left_character, right_character, &right.style)
                    .map(|spacing| {
                        measure_text_width(&spacing.character().to_string(), &right.metrics)
                    })
                    .or_else(|| {
                        fragment_autospace_pair_applies(
                            left_character,
                            right_character,
                            &right.style,
                        )
                        .then_some(right.metrics.font_size * 0.125)
                    })?;
            Some((left_index, spacing))
        })
        .collect::<Vec<_>>();
    for (left_index, spacing) in visual_autospace {
        fragments[left_index].rect.width += spacing;
        fragments[left_index].autospace_after += spacing;
    }
    let visual_replacements = visual_text_groups
        .iter()
        .flat_map(|group| group.windows(3))
        .filter_map(|window| {
            let left_index = window[0];
            let space_index = window[1];
            let right_index = window[2];
            if !replacements[space_index] {
                return None;
            }
            let left = &fragments[left_index];
            let right = &fragments[right_index];
            let left_text = left.text()?;
            let right_text = right.text()?;
            let left_character =
                if homogeneous_levels[left_index].is_some_and(|level| level.is_rtl()) {
                    left_text.chars().next()
                } else {
                    left_text.chars().next_back()
                }?;
            let right_character =
                if homogeneous_levels[right_index].is_some_and(|level| level.is_rtl()) {
                    right_text.chars().next_back()
                } else {
                    right_text.chars().next()
                }?;
            let space = &fragments[space_index];
            let spacing =
                fragment_french_autospace_pair(left_character, right_character, &space.style)
                    .map(|spacing| {
                        measure_text_width(&spacing.character().to_string(), &space.metrics)
                    })
                    .or_else(|| {
                        fragment_autospace_pair_applies(
                            left_character,
                            right_character,
                            &space.style,
                        )
                        .then_some(space.metrics.font_size * 0.125)
                    })?;
            Some((space_index, spacing))
        })
        .collect::<Vec<_>>();
    for (space_index, spacing) in visual_replacements {
        fragments[space_index].rect.width = spacing;
        fragments[space_index].autospace_after = spacing;
    }

    let mut cursor = line_start;
    for logical_index in visual_order {
        let fragment = &mut fragments[logical_index];
        fragment.rect.x = cursor;
        fragment.style.resolved_bidi_level =
            homogeneous_levels[logical_index].map(|level| level.number());
        cursor += fragment.rect.width;
    }
}

fn apply_line_text_spacing_trim(
    fragments: &mut [InlineFragment],
    direction_rtl: bool,
    space_first_should_space: bool,
    allow_end_should_trim: bool,
) -> f32 {
    fn source_edge(fragment: &InlineFragment, physical_left: bool) -> Option<(char, bool)> {
        let text = fragment.text()?;
        let reversed = fragment
            .style
            .resolved_bidi_level
            .is_some_and(|level| level % 2 == 1);
        let source_first = physical_left != reversed;
        let character = if source_first {
            text.chars().next()
        } else {
            text.chars().next_back()
        }?;
        Some((character, source_first))
    }

    let mut visual_order = (0..fragments.len()).collect::<Vec<_>>();
    visual_order.sort_by(|left, right| {
        let left_start = fragments[*left]
            .rect
            .x
            .min(fragments[*left].rect.x + fragments[*left].rect.width);
        let right_start = fragments[*right]
            .rect
            .x
            .min(fragments[*right].rect.x + fragments[*right].rect.width);
        left_start
            .total_cmp(&right_start)
            .then_with(|| left.cmp(right))
    });
    let mut text_groups = vec![Vec::new()];
    for index in visual_order.iter().copied() {
        if fragments[index].text().is_some_and(|text| !text.is_empty()) {
            text_groups.last_mut().unwrap().push(index);
        } else if !matches!(
            fragments[index].content,
            InlineFragmentContent::InlineEdge(_, _) | InlineFragmentContent::InlineSpacing(_)
        ) || fragments[index].rect.width != 0.0
        {
            text_groups.push(Vec::new());
        }
    }

    let mut trim_source_edges = Vec::<(usize, bool)>::new();
    for group in &text_groups {
        for pair in group.windows(2) {
            let left_index = pair[0];
            let right_index = pair[1];
            let Some((left_character, left_source_first)) =
                source_edge(&fragments[left_index], false)
            else {
                continue;
            };
            let Some((right_character, right_source_first)) =
                source_edge(&fragments[right_index], true)
            else {
                continue;
            };
            let vertical = fragments[right_index].metrics.vertical_writing;
            let left_class = super::fullwidth_punctuation_class(left_character, vertical);
            let right_class = super::fullwidth_punctuation_class(right_character, vertical);
            if right_class == Some(super::FullwidthPunctuationClass::Opening)
                && fragments[right_index].metrics.text_spacing_trim.adjacent
                    == super::TextSpacingTrimAdjacent::Trim
                && super::opening_punctuation_trims_after(
                    left_character,
                    fragments[left_index].metrics.font_size,
                    fragments[right_index].metrics.font_size,
                    vertical,
                )
            {
                trim_source_edges.push((right_index, right_source_first));
            }
            if left_class == Some(super::FullwidthPunctuationClass::Closing)
                && fragments[left_index].metrics.text_spacing_trim.adjacent
                    == super::TextSpacingTrimAdjacent::Trim
                && super::closing_punctuation_trims_before(
                    right_character,
                    fragments[left_index].metrics.font_size,
                    fragments[right_index].metrics.font_size,
                    vertical,
                )
            {
                trim_source_edges.push((left_index, left_source_first));
            }
        }
    }

    let first_meaningful = visual_order.iter().copied().find(|index| {
        fragments[*index]
            .text()
            .is_some_and(|text| !text.is_empty())
            || fragments[*index].rect.width != 0.0
    });
    let last_meaningful = visual_order.iter().copied().rev().find(|index| {
        fragments[*index]
            .text()
            .is_some_and(|text| !text.is_empty())
            || fragments[*index].rect.width != 0.0
    });
    let inline_start = if direction_rtl {
        last_meaningful
    } else {
        first_meaningful
    };
    let inline_end = if direction_rtl {
        first_meaningful
    } else {
        last_meaningful
    };
    if let Some(index) = inline_start
        && let Some((character, source_first)) = source_edge(&fragments[index], !direction_rtl)
        && super::fullwidth_punctuation_class(character, fragments[index].metrics.vertical_writing)
            == Some(super::FullwidthPunctuationClass::Opening)
        && (fragments[index].metrics.text_spacing_trim.start == super::TextSpacingTrimStart::Trim
            || fragments[index].metrics.text_spacing_trim.start
                == super::TextSpacingTrimStart::SpaceFirst
                && !space_first_should_space)
    {
        trim_source_edges.push((index, source_first));
    }
    if let Some(index) = inline_end
        && let Some((character, source_first)) = source_edge(&fragments[index], direction_rtl)
        && super::fullwidth_punctuation_class(character, fragments[index].metrics.vertical_writing)
            == Some(super::FullwidthPunctuationClass::Closing)
        && (fragments[index].metrics.text_spacing_trim.end == super::TextSpacingTrimEnd::Trim
            || fragments[index].metrics.text_spacing_trim.end == super::TextSpacingTrimEnd::Allow
                && allow_end_should_trim)
    {
        trim_source_edges.push((index, source_first));
    }

    trim_source_edges.sort_unstable();
    trim_source_edges.dedup();
    for (index, source_first) in trim_source_edges {
        if source_first {
            fragments[index].punctuation_trim.first = true;
        } else {
            fragments[index].punctuation_trim.last = true;
        }
    }

    let old_width = fragments
        .iter()
        .map(|fragment| fragment.rect.width)
        .sum::<f32>();
    for fragment in fragments.iter_mut() {
        let Some(text) = fragment.text() else {
            continue;
        };
        if fragment.punctuation_trim == Default::default() {
            continue;
        }
        let untrimmed = measure_text_width(text, &fragment.metrics);
        let trimmed =
            measure_text_width_with_trim_edges(text, &fragment.metrics, fragment.punctuation_trim);
        fragment.rect.width += trimmed - untrimmed;
    }
    let new_width = fragments
        .iter()
        .map(|fragment| fragment.rect.width)
        .sum::<f32>();
    let line_start = visual_order
        .iter()
        .map(|index| fragments[*index].rect.x)
        .reduce(f32::min)
        .unwrap_or(0.0);
    let mut cursor = line_start;
    for index in visual_order {
        fragments[index].rect.x = cursor;
        cursor += fragments[index].rect.width;
    }
    new_width - old_width
}

fn bidi_controls(style: &FragmentStyle) -> (&'static str, &'static str) {
    let rtl = style.direction.as_deref() == Some("rtl");
    match style.unicode_bidi.as_deref() {
        Some("embed") => (if rtl { "\u{202b}" } else { "\u{202a}" }, "\u{202c}"),
        Some("bidi-override") => (if rtl { "\u{202e}" } else { "\u{202d}" }, "\u{202c}"),
        Some("isolate") => (if rtl { "\u{2067}" } else { "\u{2066}" }, "\u{2069}"),
        Some("isolate-override") => (
            if rtl {
                "\u{2067}\u{202e}"
            } else {
                "\u{2066}\u{202d}"
            },
            "\u{202c}\u{2069}",
        ),
        Some("plaintext") => ("\u{2068}", "\u{2069}"),
        _ => ("", ""),
    }
}

// ── Text measurement ────────────────────────────────────────────────────────

pub(super) fn measure_text_width(text: &str, metrics: &FontMetrics) -> f32 {
    measure_text_width_with_trim_edges(text, metrics, Default::default())
}

fn measure_text_width_with_trim_edges(
    text: &str,
    metrics: &FontMetrics,
    edges: super::PunctuationTrim,
) -> f32 {
    let _measure_phase = super::profile::Scope::begin(super::profile::Phase::TextMeasure);
    let shaping_options = super::text_spacing_trim_shaping_options_with_edges(
        text,
        metrics.text_spacing_trim,
        metrics.vertical_writing,
        edges,
        &metrics.shaping_options,
    );
    LAYOUT_FONTS.with(|cell| {
        let mut fonts_ref = cell.borrow_mut();
        if fonts_ref.is_none() {
            *fonts_ref = Some(super::LayoutFontContext {
                system_fonts: load_layout_fonts(),
                web_fonts: None,
                exact_metrics: false,
                metrics_cache: HashMap::new(),
            });
        }

        if let Some(ref context) = *fonts_ref {
            let variant = crate::font::FontVariantKey {
                weight: metrics.font_weight,
                style: metrics.font_style,
                style_angle: metrics.font_style_angle,
                stretch: metrics.font_stretch,
            };
            let web_candidates = metrics
                .font_family
                .and_then(|family| {
                    context.web_fonts.as_deref().map(|registry| {
                        registry.select_candidates_for_family_list(
                            metrics.font_scope_root,
                            family,
                            variant,
                        )
                    })
                })
                .unwrap_or_default();
            let font_phase = super::profile::Scope::begin(super::profile::Phase::FontSelect);
            let selected = select_text_font(
                "layout",
                metrics.font_family,
                metrics.font_scope_root,
                variant,
                context.web_fonts.as_deref(),
                &context.system_fonts,
            );
            drop(font_phase);
            let primary = selected.as_ref().map(AsRef::as_ref);
            if primary.is_some() || !context.system_fonts.is_empty() {
                let horizontal_options = if metrics.vertical_writing {
                    metrics.shaping_options.as_ref()
                } else {
                    shaping_options.as_ref()
                };
                let base = measure_text_width_with_fallback(
                    text,
                    metrics.font_size,
                    horizontal_options,
                    primary,
                    &web_candidates,
                    &context.system_fonts,
                );
                let vertical_trim_delta = if metrics.vertical_writing
                    && matches!(shaping_options, std::borrow::Cow::Owned(_))
                {
                    let untrimmed = measure_text_advance_with_fallback(
                        text,
                        metrics.font_size,
                        &metrics.shaping_options,
                        primary,
                        &web_candidates,
                        &context.system_fonts,
                        ShapingDirection::TopToBottom,
                        true,
                    );
                    let trimmed = measure_text_advance_with_fallback(
                        text,
                        metrics.font_size,
                        &shaping_options,
                        primary,
                        &web_candidates,
                        &context.system_fonts,
                        ShapingDirection::TopToBottom,
                        true,
                    );
                    trimmed - untrimmed
                } else {
                    0.0
                };
                // Shaped paint inserts spacing between extended grapheme
                // clusters, not between the scalars that form one cluster.
                let spacing = metrics.letter_spacing * grapheme_spacing_boundaries(text) as f32;
                let word_spacing = metrics.word_spacing * text.matches(' ').count() as f32;
                let french_spacing = super::font_metrics_french_autospace_boundaries(text, metrics)
                    .into_iter()
                    .map(|(_, spacing)| {
                        measure_text_width_with_fallback(
                            &spacing.character().to_string(),
                            metrics.font_size,
                            &shaping_options,
                            primary,
                            &web_candidates,
                            &context.system_fonts,
                        )
                    })
                    .sum::<f32>();
                return base
                    + vertical_trim_delta
                    + spacing
                    + word_spacing
                    + french_spacing
                    + super::font_metrics_autospace_advance(text, metrics);
            }
        }

        // Fallback to approximation when no font is available
        let char_count = text
            .chars()
            .filter(|ch| !is_zero_advance_character(*ch))
            .count();
        let base = char_count as f32 * metrics.average_advance;
        let spacing = metrics.letter_spacing * grapheme_spacing_boundaries(text) as f32;
        let word_spacing = metrics.word_spacing * text.matches(' ').count() as f32;
        let french_spacing = super::font_metrics_french_autospace_boundaries(text, metrics)
            .into_iter()
            .map(|(_, spacing)| match spacing {
                super::FrenchAutospace::NarrowNoBreak => metrics.font_size * 0.2,
                super::FrenchAutospace::NoBreak => metrics.average_advance,
            })
            .sum::<f32>();
        base + spacing
            + word_spacing
            + french_spacing
            + super::font_metrics_autospace_advance(text, metrics)
    })
}

fn load_layout_fonts() -> Vec<Arc<Font>> {
    load_default_text_fonts_shared()
}

fn measure_text_width_with_fallback(
    text: &str,
    font_size: f32,
    options: &crate::font::ShapingOptions,
    primary: Option<&Font>,
    web_fonts: &[WebFontCandidate<'_>],
    fonts: &[Arc<Font>],
) -> f32 {
    let direction = if text
        .chars()
        .any(|ch| matches!(bidi_class(ch), BidiClass::R | BidiClass::AL | BidiClass::AN))
    {
        ShapingDirection::RightToLeft
    } else {
        ShapingDirection::LeftToRight
    };
    measure_text_advance_with_fallback(
        text, font_size, options, primary, web_fonts, fonts, direction, false,
    )
}

#[allow(clippy::too_many_arguments)]
fn measure_text_advance_with_fallback(
    text: &str,
    font_size: f32,
    options: &crate::font::ShapingOptions,
    primary: Option<&Font>,
    web_fonts: &[WebFontCandidate<'_>],
    fonts: &[Arc<Font>],
    direction: ShapingDirection,
    vertical_axis: bool,
) -> f32 {
    let mut run_fonts =
        Vec::with_capacity(web_fonts.len() + fonts.len() + usize::from(primary.is_some()));
    run_fonts.extend(web_fonts.iter().map(|candidate| FontFallbackCandidate {
        font: candidate.font,
        unicode_range: Some(candidate.unicode_range),
        weight: candidate.weight,
        style: candidate.style,
    }));
    if web_fonts.is_empty()
        && let Some(primary) = primary
    {
        run_fonts.push(FontFallbackCandidate::unrestricted(primary));
    }
    run_fonts.extend(
        fonts
            .iter()
            .map(Arc::as_ref)
            .map(FontFallbackCandidate::unrestricted),
    );
    if !run_fonts.is_empty()
        && let Ok(runs) = shape_text_with_fallback_candidates_and_options(
            &run_fonts, text, font_size, direction, options,
        )
    {
        return runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .map(|glyph| {
                if vertical_axis {
                    glyph.y_advance.abs()
                } else {
                    glyph.x_advance.abs()
                }
            })
            .sum();
    }

    if let Some(font) = run_fonts
        .iter()
        .find(|candidate| {
            candidate
                .unicode_range
                .is_none_or(|range| text.chars().all(|ch| range.contains(ch)))
                && text.chars().all(|ch| {
                    ch.is_whitespace()
                        || is_zero_advance_character(ch)
                        || candidate.font.has_glyph(ch)
                })
        })
        .map(|candidate| candidate.font)
    {
        if let Ok(glyphs) = font.shape_text_with_options(text, font_size, direction, options) {
            return glyphs
                .iter()
                .map(|glyph| {
                    if vertical_axis {
                        glyph.y_advance.abs()
                    } else {
                        glyph.x_advance.abs()
                    }
                })
                .sum();
        }
    }

    let mut width = 0.0;
    let mut previous: Option<(char, *const Font)> = None;

    for ch in text.chars() {
        let Some(font) = run_fonts
            .iter()
            .find(|candidate| {
                candidate
                    .unicode_range
                    .is_none_or(|range| range.contains(ch))
                    && (ch.is_whitespace() || candidate.font.has_glyph(ch))
            })
            .map(|candidate| candidate.font)
        else {
            continue;
        };
        let font_id = std::ptr::from_ref(font);

        if !vertical_axis
            && !is_zero_advance_character(ch)
            && let Some((prev_char, prev_font)) = previous
            && prev_font == font_id
        {
            width += font.glyph_kerning(prev_char, ch, font_size);
        }

        if !is_zero_advance_character(ch) {
            let advance = if vertical_axis {
                font_size
            } else {
                font.glyph_advance(ch, font_size)
            };
            width += if advance > 0.0 { advance } else { 0.0 };
            previous = Some((ch, font_id));
        }
    }

    width
}

#[cfg(test)]
mod image_url_tests {
    use super::*;
    use crate::http::corp::EmbedderPolicyValue;
    use crate::http::cors::Origin as CorsOrigin;
    use crate::http::{
        Client, CookieJar, CookieRequestContext, HttpParseError, HttpResponse, HttpTransport,
    };
    use crate::test_support::http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    use std::sync::Mutex;
    use std::time::Duration;

    struct ImageTransport {
        body: Vec<u8>,
        requests: Arc<Mutex<Vec<(String, Option<String>)>>>,
    }

    impl HttpTransport for ImageTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            self.requests.lock().unwrap().push((
                request.url().to_string(),
                request.header("cookie").map(str::to_owned),
            ));
            let headers = if request.url().path() == "/opted-in.png" {
                vec![("Cross-Origin-Resource-Policy".into(), "cross-origin".into())]
            } else {
                Vec::new()
            };
            Ok(HttpResponse::new(200, "OK", headers, self.body.clone()))
        }
    }

    struct ImageClientGuard(Client);

    impl Drop for ImageClientGuard {
        fn drop(&mut self) {
            HTTP_CLIENT.with(|client| {
                client.replace(std::mem::replace(&mut self.0, Client::new()));
            });
        }
    }

    #[test]
    fn document_image_fetch_enforces_coep_and_credentialless_cookies() {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[255, 0, 0, 255])
                .unwrap();
        }
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mut client = Client::new();
        client.set_transport(Some(Arc::new(ImageTransport {
            body: png,
            requests: Arc::clone(&requests),
        })));
        let _guard = ImageClientGuard(HTTP_CLIENT.with(|cell| cell.replace(client)));
        let store = Arc::new(Mutex::new(CookieJar::new()));
        store
            .lock()
            .unwrap()
            .add_from_header("session=value", "resource.example.test");
        let document: Url = "http://source.example.test/page".parse().unwrap();
        let origin = CorsOrigin::from_url(&document);
        let blocked = "http://resource.example.test/blocked.png";
        let allowed = "http://resource.example.test/opted-in.png";

        super::super::with_image_cookie_store_reporting(
            Arc::clone(&store),
            CookieRequestContext::new(Some(document.clone()), None, false),
            1,
            origin.clone(),
            EmbedderPolicyValue::RequireCorp,
            EmbedderPolicyValue::UnsafeNone,
            || {
                assert!(fetch_image_uncached(blocked).is_none());
                assert!(fetch_image_uncached(allowed).is_some());
            },
        );
        super::super::with_image_cookie_store_reporting(
            Arc::clone(&store),
            CookieRequestContext::new(Some(document.clone()), None, false),
            1,
            origin.clone(),
            EmbedderPolicyValue::Credentialless,
            EmbedderPolicyValue::UnsafeNone,
            || assert!(fetch_image_uncached(blocked).is_some()),
        );
        let (loaded, reports) = super::super::with_image_cookie_store_reporting(
            Arc::clone(&store),
            CookieRequestContext::new(Some(document), None, false),
            1,
            origin,
            EmbedderPolicyValue::UnsafeNone,
            EmbedderPolicyValue::RequireCorp,
            || fetch_image_uncached(blocked).is_some(),
        );
        assert!(loaded);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].blocked_url, blocked);
        assert!(reports[0].report_only);
        assert!(!reports[0].enforced);
        assert_eq!(
            requests.lock().unwrap().as_slice(),
            [
                (blocked.to_string(), Some("session=value".to_string())),
                (allowed.to_string(), Some("session=value".to_string())),
                (blocked.to_string(), None),
                (blocked.to_string(), Some("session=value".to_string())),
            ]
        );
    }

    #[test]
    fn canonical_reference_uses_only_the_explicit_base() {
        let first: Url = "https://example.test/assets/document.svg".parse().unwrap();
        let second: Url = "https://other.test/another/document.svg".parse().unwrap();
        let first_result = Some("https://example.test/assets/self.svg".to_string());
        let second_result = Some("https://other.test/another/self.svg".to_string());

        assert_eq!(canonical_image_asset_reference("self.svg", None), None);
        assert_eq!(
            canonical_image_asset_reference("self.svg", Some(&first)),
            first_result
        );
        assert_eq!(
            canonical_image_asset_reference("self.svg", Some(&second)),
            second_result
        );

        super::super::with_image_base_url(Some(second.clone()), || {
            assert_eq!(canonical_image_asset_reference("self.svg", None), None);
            assert_eq!(
                canonical_image_asset_reference("self.svg", Some(&first)),
                first_result
            );
            super::super::with_image_base_url(None, || {
                assert_eq!(canonical_image_asset_reference("self.svg", None), None);
                assert_eq!(
                    canonical_image_asset_reference("self.svg", Some(&first)),
                    first_result
                );
            });
            assert_eq!(
                canonical_image_asset_reference("self.svg", Some(&first)),
                first_result
            );
        });
    }

    #[test]
    fn absolute_data_and_blob_references_ignore_the_base() {
        let base: Url = "https://example.test/assets/document.svg".parse().unwrap();
        for source in [
            "HTTPS://EXAMPLE.TEST:443/assets/./self.svg",
            "DATA:image/png,bytes",
            "BLOB:https://example.test/image-id",
        ] {
            assert_eq!(
                canonical_image_asset_reference(source, Some(&base)),
                canonical_image_asset_reference(source, None),
                "source: {source}"
            );
        }
        assert_eq!(
            canonical_image_asset_reference("DATA:image/png,bytes", None),
            Some("data:image/png,bytes".to_string())
        );
        assert_eq!(
            canonical_image_asset_reference("BLOB:https://example.test/image-id", None),
            Some("blob:https://example.test/image-id".to_string())
        );
    }

    #[test]
    fn third_party_image_sends_and_stores_cookies_only_with_document_access() {
        let listener = bind_loopback().unwrap();
        let address = listener.local_addr().unwrap();
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[255, 0, 0, 255])
                .unwrap();
        }
        let server = FixtureWorker::spawn(move || {
            let mut cookies = Vec::new();
            for phase in ["pre", "post"] {
                let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
                let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                assert!(request.starts_with("GET /image.png HTTP/1.1"));
                cookies.push(
                    request
                        .lines()
                        .find_map(|line| line.strip_prefix("Cookie: "))
                        .map(str::trim)
                        .map(str::to_owned),
                );
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nSet-Cookie: {phase}=1; Path=/; Secure; SameSite=None\r\nConnection: close\r\n\r\n",
                    png.len(),
                )
                .unwrap();
                stream.write_all(&png).unwrap();
            }
            cookies
        });
        let target: Url = format!("http://{address}/image.png").parse().unwrap();
        let top: Url = format!("http://localhost:{}/", address.port())
            .parse()
            .unwrap();
        let store = Arc::new(Mutex::new(CookieJar::new()));
        store
            .lock()
            .unwrap()
            .add_from_header_for_url("session=pre; Path=/; Secure; SameSite=None", &target);
        for phase in ["pre", "post"] {
            let context = CookieRequestContext::new(
                Some(top.clone()),
                (phase == "post").then(|| target.clone()),
                false,
            );
            let url = format!("http://{address}/image.png");
            super::super::with_image_cookie_store(Arc::clone(&store), context, 867, || {
                assert!(fetch_image(&url).is_some());
            });
        }
        assert_eq!(server.join(), [None, Some("session=pre".to_string())]);
        let cookies = store.lock().unwrap().document_cookie(&target, &target);
        assert!(!cookies.contains("pre=1"));
        assert!(cookies.contains("post=1"));
    }
}
