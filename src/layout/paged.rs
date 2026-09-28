//! Page contexts and source fragments for print layout.

use std::collections::{BTreeMap, HashMap};

use crate::css::style::ContainerContext;
use crate::css::style::counter_pairs;
use crate::css::{
    ComputedStyle, ComputedValue, MediaType, PageBoxGeometry, PageMarginBox, PageMarginContent,
    PageSelectorContext, PageSide, ResolvedPageStyle, StyleResolver, Value,
};
use crate::dom::{Node, NodeHandle, NodeType};

use super::table::{TableDisplay, html_table_span_attribute, table_display_for_node};
use super::{FontMetrics, LayoutBox, PositionScheme, Rect, layout_tree, measure_text_width};

/// One page of a paged layout and its slice of the document flow.
#[derive(Debug, Clone)]
pub struct PagedPage {
    /// Selector facts used by `@page` rules.
    pub selector: PageSelectorContext,
    /// Winning page declarations.
    pub style: ResolvedPageStyle,
    /// Used sheet size and margins in CSS pixels.
    pub geometry: PageBoxGeometry,
    /// Sheet rectangle in page-local coordinates.
    pub sheet: Rect,
    /// Printable content rectangle in page-local coordinates.
    pub content: Rect,
    /// Index of the document layout at this page's printable width.
    layout_index: usize,
    /// Content fragments placed on this page, in document order.
    pub fragments: Vec<PageContentFragment>,
    /// Position where layout resumes on the next page, if any.
    pub continuation: Option<PageContinuation>,
    /// Compatibility slice for callers that inspect the page's continuous
    /// source flow. Painting uses `fragments` instead.
    pub source: Rect,
    /// Page-scoped counter values after this page is generated.
    pub counters: PageCounterValues,
    /// Generated page-margin box rectangles in page-local coordinates.
    pub margin_box_rects: BTreeMap<PageMarginBox, Rect>,
}

/// A position in the print flow from which pagination can resume.
///
/// The section identifies a named-page or forced-break region. Its flow
/// coordinate refers to the current continuous layout; later fragmentation
/// stages can extend this cursor with positions inside a box or line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageContinuation {
    section_index: usize,
    flow_y: f32,
    inline_token: Option<usize>,
}

impl PageContinuation {
    /// Returns the index of the flow section containing this position.
    pub fn section_index(self) -> usize {
        self.section_index
    }

    /// Returns the current vertical position in the source flow.
    pub fn flow_y(self) -> f32 {
        self.flow_y
    }

    /// Returns the normalized inline-token position when a line continues.
    pub fn inline_token(self) -> Option<usize> {
        self.inline_token
    }
}

/// A piece of document flow assigned to one printed page.
#[derive(Debug, Clone, PartialEq)]
pub struct PageContentFragment {
    /// Position at the start of this fragment.
    pub start: PageContinuation,
    /// Position immediately after this fragment.
    pub end: PageContinuation,
    /// Source-space rectangle represented by this fragment.
    pub source: Rect,
    /// Page-local rectangle into which this fragment is placed.
    pub destination: Rect,
}

/// Built-in counters available to printed page-margin boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageCounterValues {
    /// The current value of the mutable `page` counter.
    pub page: i32,
    /// The read-only total number of printed pages.
    pub pages: usize,
}

impl PagedPage {
    /// Returns the page-local rectangle of a generated page-margin box.
    pub fn margin_box_rect(&self, margin_box: PageMarginBox) -> Option<Rect> {
        self.margin_box_rects.get(&margin_box).copied()
    }

    /// Resolves static text and `counter(page)` / `counter(pages)` in one margin box.
    ///
    /// Other generated-content expressions remain available from `style` for
    /// later image and counter-style support.
    pub fn margin_box_text(&self, margin_box: PageMarginBox) -> Option<String> {
        let fragments = self.margin_box_fragments(margin_box)?;
        let mut text = String::new();
        for fragment in fragments {
            match fragment {
                PageMarginFragment::Text(part) => text.push_str(&part),
                PageMarginFragment::Image(_) => return None,
            }
        }
        Some(text)
    }

    /// Resolves the ordered text and image sources in one page-margin box.
    pub fn margin_box_fragments(
        &self,
        margin_box: PageMarginBox,
    ) -> Option<Vec<PageMarginFragment>> {
        let mut page = self.counters.page;
        if let Some(resets) = self
            .style
            .margin_box_property(margin_box, "counter-reset")
            .and_then(|value| counter_pairs(value, 0))
        {
            for (name, value) in resets {
                if name == "page" {
                    page = value;
                }
            }
        }
        if let Some(increments) = self
            .style
            .margin_box_property(margin_box, "counter-increment")
            .and_then(|value| counter_pairs(value, 1))
        {
            for (name, amount) in increments {
                if name == "page" {
                    page = page.saturating_add(amount);
                }
            }
        }
        let content = self.style.margin_box_content(margin_box)?;
        let mut fragments = Vec::new();
        match content {
            PageMarginContent::Text(text) => fragments.push(PageMarginFragment::Text(text)),
            PageMarginContent::Expression(value) => {
                page_content_fragments(&value, page, self.counters.pages, &mut fragments)?;
            }
        }
        let mut combined = Vec::with_capacity(fragments.len());
        for fragment in fragments {
            match fragment {
                PageMarginFragment::Text(text) => {
                    if let Some(PageMarginFragment::Text(previous)) = combined.last_mut() {
                        previous.push_str(&text);
                    } else {
                        combined.push(PageMarginFragment::Text(text));
                    }
                }
                image => combined.push(image),
            }
        }
        Some(combined)
    }
}

/// One resolved part of generated page-margin content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageMarginFragment {
    /// Text after string and counter evaluation.
    Text(String),
    /// URL to an image, before base-URL resolution and decoding.
    Image(String),
}

fn page_content_fragments(
    value: &Value,
    page: i32,
    pages: usize,
    fragments: &mut Vec<PageMarginFragment>,
) -> Option<()> {
    match value {
        Value::String(text) => fragments.push(PageMarginFragment::Text(text.clone())),
        Value::List(parts) => {
            for part in parts {
                page_content_fragments(part, page, pages, fragments)?;
            }
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("counter") => {
            let [Value::Keyword(counter)] = arguments.as_slice() else {
                return None;
            };
            let text = match counter.as_str() {
                "page" => page.to_string(),
                "pages" => pages.to_string(),
                _ => return None,
            };
            fragments.push(PageMarginFragment::Text(text));
        }
        Value::Keyword(keyword)
            if keyword
                .get(..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("url("))
                && keyword.ends_with(')') =>
        {
            let source = keyword[4..keyword.len() - 1]
                .trim()
                .trim_matches(['\'', '"']);
            if source.is_empty() {
                return None;
            }
            fragments.push(PageMarginFragment::Image(source.to_string()));
        }
        _ => return None,
    }
    Some(())
}

/// Print layout with page-specific contexts and a reusable document box tree.
#[derive(Debug, Clone)]
pub struct PagedLayout {
    /// Continuous flow at the first page's content width, used for pagination.
    pub layout: LayoutBox,
    /// Output pages in document order.
    pub pages: Vec<PagedPage>,
    /// Additional document layouts for pages with a different content width.
    alternate_layouts: Vec<LayoutBox>,
    /// Query-container dimensions captured alongside each document layout.
    container_contexts: Vec<HashMap<usize, ContainerContext>>,
}

impl PagedLayout {
    /// Returns the document layout calculated at this page's content width.
    pub(crate) fn layout_for_page(&self, page: &PagedPage) -> &LayoutBox {
        if page.layout_index == 0 {
            &self.layout
        } else {
            &self.alternate_layouts[page.layout_index - 1]
        }
    }

    /// Restores query-container styles for the selected page layout before paint.
    pub(crate) fn restore_style_context_for_page(
        &self,
        page: &PagedPage,
        resolver: &mut StyleResolver,
    ) {
        resolver.set_container_contexts(self.container_contexts[page.layout_index].clone());
    }
}

#[derive(Debug)]
struct FlowSection {
    name: Option<String>,
    start: f32,
    end: f32,
    requested_side: Option<PageSide>,
}

struct FlowBoundary {
    name: Option<String>,
    position: f32,
    forced_break: Option<ForcedPageBreak>,
    first_in_flow: bool,
}

#[derive(Clone, Copy)]
enum ForcedPageBreak {
    Page,
    Side(PageSide),
}

impl ForcedPageBreak {
    fn side(self) -> Option<PageSide> {
        match self {
            Self::Page => None,
            Self::Side(side) => Some(side),
        }
    }
}

fn forced_page_break(value: Option<&ComputedValue>) -> Option<ForcedPageBreak> {
    let Some(ComputedValue::Keyword(value)) = value else {
        return None;
    };
    if value.eq_ignore_ascii_case("page") {
        Some(ForcedPageBreak::Page)
    } else if value.eq_ignore_ascii_case("left") {
        Some(ForcedPageBreak::Side(PageSide::Left))
    } else if value.eq_ignore_ascii_case("right") {
        Some(ForcedPageBreak::Side(PageSide::Right))
    } else {
        None
    }
}

/// Lays out a document for print and builds page contexts from `@page` rules.
///
/// Pagination coordinates come from the first page's flow. Layouts at other
/// content widths recalculate block placement. For flow sections composed of
/// direct inline children, each continuation relayouts the unprinted text at
/// that page's width.
pub fn layout_paged_tree(
    document: &NodeHandle,
    resolver: &mut StyleResolver,
    default_sheet: Rect,
) -> Option<PagedLayout> {
    if !default_sheet.width.is_finite()
        || !default_sheet.height.is_finite()
        || default_sheet.width <= 0.0
        || default_sheet.height <= 0.0
    {
        return None;
    }
    resolver.set_viewport(default_sheet.width, default_sheet.height);
    resolver.set_media_type(MediaType::Print);
    let progression_first_side = document
        .child_nodes()
        .iter()
        .find(|child| child.tag_name().as_deref() == Some("html"))
        .and_then(|root| resolver.computed_style(root).get("direction").cloned())
        .and_then(|value| match value {
            ComputedValue::Keyword(value) if value.eq_ignore_ascii_case("rtl") => {
                Some(PageSide::Left)
            }
            _ => None,
        })
        .unwrap_or(PageSide::Right);
    // A side break propagated from the first in-flow body child changes the
    // side of the first printed page; no leading blank sheet is emitted.
    let first_side = first_body_break_side(document, resolver).unwrap_or(progression_first_side);
    let first_name = first_page_name(document, resolver);
    let first_selector = page_selector(0, first_name, first_side);
    let first_style = resolver.resolved_page_style(&first_selector);
    let first_geometry = first_style.geometry((default_sheet.width, default_sheet.height), 0.0);
    let first_content = content_rect(first_geometry);
    let layout = layout_tree(document, resolver, first_content)?;
    let sections = flow_sections(&layout, resolver, first_content.y);
    let body_layout = find_body_layout(&layout);
    let mut alternate_layouts = Vec::new();
    let mut alternate_sections = Vec::new();
    let mut container_contexts = vec![resolver.container_contexts_snapshot()];
    let mut page_breaks = vec![page_break_candidates(&layout, resolver, true)];
    let mut layout_by_flow = HashMap::new();

    let mut pages = Vec::new();
    for (section_index, section) in sections.iter().enumerate() {
        let inline_flows = body_layout.and_then(|body| {
            let children = body
                .children
                .iter()
                .filter(|child| {
                    child.pseudo.is_none()
                        && child.dimensions.border_box().y >= section.start
                        && child.dimensions.border_box().y < section.end
                })
                .collect::<Vec<_>>();
            if children.is_empty() || children.iter().any(|child| child.lines.is_empty()) {
                return None;
            }
            let flows = children
                .into_iter()
                .map(|child| {
                    let total_tokens = child
                        .lines
                        .iter()
                        .flat_map(|line| &line.fragments)
                        .map(|fragment| fragment.source_end_token)
                        .max()
                        .unwrap_or(0);
                    (child.node.clone(), total_tokens)
                })
                .collect::<Vec<_>>();
            flows.iter().all(|(_, total)| *total > 0).then_some(flows)
        });
        let total_inline_tokens = inline_flows
            .as_ref()
            .map(|flows| flows.iter().map(|(_, total)| total).sum::<usize>());
        let inline_owner_id = inline_flows
            .as_ref()
            .and_then(|flows| flows.first())
            .map_or(0, |(owner, _)| owner.identity());
        let mut inline_token: usize = 0;
        let mut cursor = PageContinuation {
            section_index,
            flow_y: section.start,
            inline_token: total_inline_tokens.map(|_| 0),
        };
        let mut section_started = false;
        if section.requested_side.is_some_and(|requested| {
            page_selector(pages.len(), section.name.clone(), first_side).side != requested
        }) {
            if pages.len() >= 1024 {
                return None;
            }
            let mut selector = page_selector(pages.len(), section.name.clone(), first_side);
            selector.blank = true;
            let style = resolver.resolved_page_style(&selector);
            let geometry = style.geometry((default_sheet.width, default_sheet.height), 0.0);
            let content = content_rect(geometry);
            let source = Rect {
                x: first_content.x,
                y: cursor.flow_y,
                width: first_content.width,
                height: 0.0,
            };
            let margin_box_rects = corner_margin_box_rects(&style, geometry);
            pages.push(PagedPage {
                selector,
                style,
                geometry,
                sheet: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: geometry.width,
                    height: geometry.height,
                },
                content,
                // The inserted page contains no document-flow fragments.
                layout_index: 0,
                fragments: Vec::new(),
                continuation: Some(cursor),
                source,
                counters: PageCounterValues { page: 0, pages: 0 },
                margin_box_rects,
            });
        }
        loop {
            if pages.len() >= 1024 {
                // Returning a truncated document would silently discard the
                // continuation and make a successful render look complete.
                return None;
            }
            let selector = page_selector(pages.len(), section.name.clone(), first_side);
            let style = resolver.resolved_page_style(&selector);
            let geometry = style.geometry((default_sheet.width, default_sheet.height), 0.0);
            let content = content_rect(geometry);
            let layout_key = (content.width.to_bits(), inline_owner_id, inline_token);
            let layout_index =
                if content.width.to_bits() == first_content.width.to_bits() && inline_token == 0 {
                    0
                } else if let Some(&index) = layout_by_flow.get(&layout_key) {
                    index
                } else {
                    let content_rect = Rect {
                        width: content.width,
                        ..first_content
                    };
                    let page_layout = if let Some(flows) = &inline_flows {
                        let mut prefix = 0;
                        let offsets = flows
                            .iter()
                            .map(|(owner, total)| {
                                let offset = inline_token.saturating_sub(prefix).min(*total);
                                prefix += total;
                                (owner.clone(), offset)
                            })
                            .collect::<Vec<_>>();
                        super::inline::with_print_inline_skips(&offsets, || {
                            layout_tree(document, resolver, content_rect)
                        })?
                    } else {
                        layout_tree(document, resolver, content_rect)?
                    };
                    let page_sections = flow_sections(&page_layout, resolver, first_content.y);
                    let page_break_candidates =
                        page_break_candidates(&page_layout, resolver, inline_flows.is_none());
                    alternate_layouts.push(page_layout);
                    alternate_sections.push(page_sections);
                    container_contexts.push(resolver.container_contexts_snapshot());
                    page_breaks.push(page_break_candidates);
                    let index = alternate_layouts.len();
                    layout_by_flow.insert(layout_key, index);
                    index
                };
            let page_section = if layout_index == 0 {
                section
            } else {
                alternate_sections[layout_index - 1].get(section_index)?
            };
            if !section_started || inline_flows.is_some() {
                cursor.flow_y = page_section.start;
                section_started = true;
            }
            let margin_box_rects = corner_margin_box_rects(&style, geometry);
            let remaining = page_section.end - cursor.flow_y;
            if !remaining.is_finite() || remaining < 0.0 {
                return None;
            }
            if remaining > 0.0 && (!content.height.is_finite() || content.height <= 0.0) {
                return None;
            }
            let mut end_y = (cursor.flow_y + content.height).min(page_section.end);
            if end_y < page_section.end {
                let candidates = &page_breaks[layout_index];
                let within_page = candidates
                    .points
                    .partition_point(|candidate| candidate.y <= end_y);
                let mut fallback = None;
                let mut preferred = None;
                for candidate in candidates.points[..within_page].iter().rev() {
                    if candidate.y <= cursor.flow_y {
                        break;
                    }
                    fallback.get_or_insert(candidate.y);
                    if candidates.is_preferred(candidate, cursor.flow_y) {
                        preferred = Some(candidate.y);
                        break;
                    }
                }
                if let Some(boundary) = preferred.or(fallback) {
                    end_y = boundary;
                }
                for range in &candidates.avoid_ranges {
                    if range.start > cursor.flow_y + 0.01
                        && range.start < end_y - 0.01
                        && range.end > end_y + 0.01
                    {
                        end_y = range.start;
                        break;
                    }
                }
            }
            if remaining > 0.0 && end_y <= cursor.flow_y {
                return None;
            }
            let end_inline_token = if let Some(flows) = &inline_flows {
                let page_layout = if layout_index == 0 {
                    &layout
                } else {
                    &alternate_layouts[layout_index - 1]
                };
                let mut prefix = 0;
                let mut end_token = inline_token;
                for (owner, total) in flows {
                    let owner_layout = find_layout_for_node(page_layout, owner)?;
                    let consumed = owner_layout
                        .lines
                        .iter()
                        .filter(|line| {
                            line.rect.y >= cursor.flow_y
                                && line.rect.y + line.rect.height <= end_y + 0.01
                        })
                        .flat_map(|line| &line.fragments)
                        .map(|fragment| fragment.source_end_token)
                        .max()
                        .unwrap_or(0);
                    if consumed > 0 {
                        end_token = end_token.max(prefix + consumed);
                    }
                    prefix += total;
                }
                if end_token <= inline_token && inline_token < prefix {
                    return None;
                }
                Some(end_token)
            } else {
                None
            };
            let continues_inline = end_inline_token
                .zip(total_inline_tokens)
                .is_some_and(|(end_token, total)| end_token < total);
            let end = PageContinuation {
                section_index,
                flow_y: end_y,
                inline_token: end_inline_token,
            };
            let source = Rect {
                x: first_content.x,
                y: cursor.flow_y,
                width: content.width,
                height: (end_y - cursor.flow_y).max(0.0),
            };
            let continuation = if continues_inline || end_y < page_section.end {
                Some(end)
            } else {
                sections
                    .get(section_index + 1)
                    .map(|next| PageContinuation {
                        section_index: section_index + 1,
                        flow_y: next.start,
                        inline_token: None,
                    })
            };
            pages.push(PagedPage {
                selector,
                style,
                geometry,
                sheet: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: geometry.width,
                    height: geometry.height,
                },
                content,
                layout_index,
                fragments: vec![PageContentFragment {
                    start: cursor,
                    end,
                    source,
                    destination: Rect {
                        x: content.x,
                        y: content.y,
                        width: content.width,
                        height: source.height,
                    },
                }],
                continuation,
                source,
                counters: PageCounterValues { page: 0, pages: 0 },
                margin_box_rects,
            });
            if continues_inline {
                inline_token = end_inline_token?;
                cursor = PageContinuation {
                    section_index,
                    flow_y: page_section.start,
                    inline_token: Some(inline_token),
                };
                continue;
            }
            if end_y >= page_section.end {
                break;
            }
            inline_token = end_inline_token.unwrap_or(0);
            cursor = end;
        }
    }
    let total_pages = pages.len();
    let mut page_counter = 0i32;
    for page in &mut pages {
        if let Some(resets) = page
            .style
            .get("counter-reset")
            .and_then(|value| counter_pairs(value, 0))
        {
            for (name, value) in resets {
                if name == "page" {
                    page_counter = value;
                }
            }
        }
        let mut explicit_page_increment = false;
        if let Some(increments) = page
            .style
            .get("counter-increment")
            .and_then(|value| counter_pairs(value, 1))
        {
            for (name, amount) in increments {
                if name == "page" {
                    page_counter = page_counter.saturating_add(amount);
                    explicit_page_increment = true;
                }
            }
        }
        if !explicit_page_increment {
            page_counter = page_counter.saturating_add(1);
        }
        page.counters = PageCounterValues {
            page: page_counter,
            pages: total_pages,
        };
        page.margin_box_rects.extend(edge_margin_box_rects(page));
    }
    Some(PagedLayout {
        layout,
        pages,
        alternate_layouts,
        container_contexts,
    })
}

fn page_selector(index: usize, name: Option<String>, first_side: PageSide) -> PageSelectorContext {
    let mut selector = PageSelectorContext::new(index, name);
    if first_side == PageSide::Left {
        selector.side = if index.is_multiple_of(2) {
            PageSide::Left
        } else {
            PageSide::Right
        };
    }
    selector
}

fn content_rect(geometry: PageBoxGeometry) -> Rect {
    Rect {
        x: geometry.margin_left,
        y: geometry.margin_top,
        width: (geometry.width - geometry.margin_left - geometry.margin_right).max(0.0),
        height: (geometry.height - geometry.margin_top - geometry.margin_bottom).max(0.0),
    }
}

fn corner_margin_box_rects(
    style: &ResolvedPageStyle,
    geometry: PageBoxGeometry,
) -> BTreeMap<PageMarginBox, Rect> {
    let width = geometry.width.max(0.0);
    let height = geometry.height.max(0.0);
    let left = geometry.margin_left.clamp(0.0, width);
    let right = geometry.margin_right.clamp(0.0, width);
    let top = geometry.margin_top.clamp(0.0, height);
    let bottom = geometry.margin_bottom.clamp(0.0, height);
    let mut rectangles = BTreeMap::new();
    for (margin_box, rect) in [
        (
            PageMarginBox::TopLeftCorner,
            Rect {
                x: 0.0,
                y: 0.0,
                width: left,
                height: top,
            },
        ),
        (
            PageMarginBox::TopRightCorner,
            Rect {
                x: width - right,
                y: 0.0,
                width: right,
                height: top,
            },
        ),
        (
            PageMarginBox::BottomLeftCorner,
            Rect {
                x: 0.0,
                y: height - bottom,
                width: left,
                height: bottom,
            },
        ),
        (
            PageMarginBox::BottomRightCorner,
            Rect {
                x: width - right,
                y: height - bottom,
                width: right,
                height: bottom,
            },
        ),
    ] {
        if style.margin_box_content(margin_box).is_some() {
            rectangles.insert(margin_box, rect);
        }
    }
    rectangles
}

#[derive(Clone, Copy)]
enum PageEdge {
    Top,
    Right,
    Bottom,
    Left,
}

impl PageEdge {
    fn boxes(self) -> [PageMarginBox; 3] {
        match self {
            Self::Top => [
                PageMarginBox::TopLeft,
                PageMarginBox::TopCenter,
                PageMarginBox::TopRight,
            ],
            Self::Right => [
                PageMarginBox::RightTop,
                PageMarginBox::RightMiddle,
                PageMarginBox::RightBottom,
            ],
            Self::Bottom => [
                PageMarginBox::BottomLeft,
                PageMarginBox::BottomCenter,
                PageMarginBox::BottomRight,
            ],
            Self::Left => [
                PageMarginBox::LeftTop,
                PageMarginBox::LeftMiddle,
                PageMarginBox::LeftBottom,
            ],
        }
    }

    fn is_horizontal(self) -> bool {
        matches!(self, Self::Top | Self::Bottom)
    }
}

#[derive(Clone, Copy, Default)]
struct EdgeBoxMeasure {
    generated: bool,
    specified: Option<f32>,
    min_content: f32,
    max_content: f32,
    min_limit: Option<f32>,
    max_limit: Option<f32>,
}

fn edge_margin_box_rects(page: &PagedPage) -> BTreeMap<PageMarginBox, Rect> {
    let geometry = page.geometry;
    let width = geometry.width.max(0.0);
    let height = geometry.height.max(0.0);
    let left = geometry.margin_left.clamp(0.0, width);
    let right = geometry.margin_right.clamp(0.0, width);
    let top = geometry.margin_top.clamp(0.0, height);
    let bottom = geometry.margin_bottom.clamp(0.0, height);
    let mut rectangles = BTreeMap::new();
    for edge in [
        PageEdge::Top,
        PageEdge::Right,
        PageEdge::Bottom,
        PageEdge::Left,
    ] {
        let horizontal = edge.is_horizontal();
        let axis_length = if horizontal { width } else { height };
        let leading = if horizontal { left } else { top };
        let trailing = if horizontal { right } else { bottom };
        let start = leading.min(axis_length - trailing);
        let available = (axis_length - leading - trailing).max(0.0);
        let (normal_start, thickness) = match edge {
            PageEdge::Top => (0.0, top),
            PageEdge::Right => (width - right, right),
            PageEdge::Bottom => (height - bottom, bottom),
            PageEdge::Left => (0.0, left),
        };
        let boxes = edge.boxes();
        let measures = boxes.map(|margin_box| {
            if page.style.margin_box_content(margin_box).is_none() {
                return None;
            }
            let (leading_margin, trailing_margin, border_padding) =
                margin_box_axis_edges(&page.style, margin_box, geometry, horizontal, available);
            let outer_extra = leading_margin + trailing_margin + border_padding;
            let metrics = FontMetrics::from_font_size(page.style.margin_box_font_size(margin_box));
            let fragments = page.margin_box_fragments(margin_box).unwrap_or_default();
            let (min_content, max_content) = margin_content_sizes(&fragments, metrics, horizontal);
            let axis_property = if horizontal { "width" } else { "height" };
            let min_property = if horizontal {
                "min-width"
            } else {
                "min-height"
            };
            let max_property = if horizontal {
                "max-width"
            } else {
                "max-height"
            };
            let specified = match page.style.margin_box_property(margin_box, axis_property) {
                Some(Value::Keyword(keyword)) if keyword.eq_ignore_ascii_case("min-content") => {
                    Some(min_content)
                }
                Some(Value::Keyword(keyword)) if keyword.eq_ignore_ascii_case("max-content") => {
                    Some(max_content)
                }
                _ => page
                    .style
                    .margin_box_length(margin_box, axis_property, geometry, available),
            };
            Some(EdgeBoxMeasure {
                generated: true,
                specified: specified.map(|value| value.max(0.0) + outer_extra),
                min_content: min_content + outer_extra,
                max_content: max_content + outer_extra,
                min_limit: page
                    .style
                    .margin_box_length(margin_box, min_property, geometry, available)
                    .map(|value| value.max(0.0) + outer_extra),
                max_limit: page
                    .style
                    .margin_box_length(margin_box, max_property, geometry, available)
                    .map(|value| value.max(0.0) + outer_extra),
            })
        });
        let lengths = edge_box_lengths(measures, available);
        for (index, (margin_box, length)) in boxes.into_iter().zip(lengths).enumerate() {
            if measures[index].is_none() {
                continue;
            }
            let (fixed_offset, fixed_length) =
                fixed_edge_dimension(&page.style, geometry, edge, margin_box, thickness);
            let (leading_margin, trailing_margin, _) =
                margin_box_axis_edges(&page.style, margin_box, geometry, horizontal, available);
            let axis_start = match index {
                0 => start,
                1 => start + (available - length) / 2.0,
                _ => start + available - length,
            };
            let rect = if horizontal {
                Rect {
                    x: axis_start + leading_margin,
                    y: normal_start + fixed_offset,
                    width: (length - leading_margin - trailing_margin).max(0.0),
                    height: fixed_length,
                }
            } else {
                Rect {
                    x: normal_start + fixed_offset,
                    y: axis_start + leading_margin,
                    width: fixed_length,
                    height: (length - leading_margin - trailing_margin).max(0.0),
                }
            };
            rectangles.insert(margin_box, rect);
        }
    }
    rectangles
}

fn margin_box_axis_edges(
    style: &ResolvedPageStyle,
    margin_box: PageMarginBox,
    geometry: PageBoxGeometry,
    horizontal: bool,
    percentage_basis: f32,
) -> (f32, f32, f32) {
    let (leading, trailing) = if horizontal {
        ("left", "right")
    } else {
        ("top", "bottom")
    };
    let margin = |side: &str| {
        style
            .margin_box_length(
                margin_box,
                &format!("margin-{side}"),
                geometry,
                percentage_basis,
            )
            .unwrap_or(0.0)
    };
    let border_padding = [leading, trailing]
        .into_iter()
        .map(|side| {
            let border =
                style.margin_box_border_width(margin_box, side, geometry, percentage_basis);
            let padding = style
                .margin_box_length(
                    margin_box,
                    &format!("padding-{side}"),
                    geometry,
                    percentage_basis,
                )
                .unwrap_or(0.0)
                .max(0.0);
            border + padding
        })
        .sum();
    (margin(leading), margin(trailing), border_padding)
}

fn margin_content_sizes(
    fragments: &[PageMarginFragment],
    metrics: FontMetrics,
    horizontal: bool,
) -> (f32, f32) {
    let mut minimum = 0.0f32;
    let mut maximum = 0.0f32;
    let mut text_present = false;
    for fragment in fragments {
        match fragment {
            PageMarginFragment::Text(text) => {
                text_present = true;
                if horizontal {
                    maximum += measure_text_width(text, metrics);
                    minimum = minimum.max(
                        super::inline::split_words_preserving_spaces_cjk(text)
                            .iter()
                            .map(|word| measure_text_width(word, metrics))
                            .fold(0.0f32, f32::max),
                    );
                } else {
                    let line_height = metrics.ascent + metrics.descent + metrics.line_gap;
                    minimum = minimum.max(line_height);
                    maximum = maximum.max(text.lines().count().max(1) as f32 * line_height);
                }
            }
            PageMarginFragment::Image(source) => {
                if let Some(image) = super::inline::decode_or_fetch_image_asset(source) {
                    let size = if horizontal {
                        image.width() as f32
                    } else {
                        image.height() as f32
                    };
                    minimum = minimum.max(size);
                    if horizontal {
                        maximum += size;
                    } else {
                        maximum = maximum.max(size);
                    }
                }
            }
        }
    }
    if !horizontal && !text_present && fragments.is_empty() {
        let line_height = metrics.ascent + metrics.descent + metrics.line_gap;
        return (line_height, line_height);
    }
    (minimum, maximum)
}

fn fixed_edge_dimension(
    style: &ResolvedPageStyle,
    geometry: PageBoxGeometry,
    edge: PageEdge,
    margin_box: PageMarginBox,
    thickness: f32,
) -> (f32, f32) {
    let horizontal = edge.is_horizontal();
    let dimension = if horizontal { "height" } else { "width" };
    let before = if horizontal {
        "margin-top"
    } else {
        "margin-left"
    };
    let after = if horizontal {
        "margin-bottom"
    } else {
        "margin-right"
    };
    let (_, _, border_padding) =
        margin_box_axis_edges(style, margin_box, geometry, !horizontal, thickness);
    let inset = |name| {
        if matches!(style.margin_box_property(margin_box, name), Some(Value::Keyword(value)) if value.eq_ignore_ascii_case("auto"))
        {
            None
        } else {
            Some(
                style
                    .margin_box_length(margin_box, name, geometry, thickness)
                    .unwrap_or(0.0),
            )
        }
    };
    let mut before_value = inset(before);
    let mut after_value = inset(after);
    let used = style
        .margin_box_length(margin_box, dimension, geometry, thickness)
        .map(|length| length.max(0.0) + border_padding);
    let length = used.unwrap_or_else(|| {
        let before = before_value.unwrap_or(0.0);
        let after = after_value.unwrap_or(0.0);
        (thickness - before - after).max(border_padding)
    });
    let minimum = style
        .margin_box_length(
            margin_box,
            if horizontal {
                "min-height"
            } else {
                "min-width"
            },
            geometry,
            thickness,
        )
        .unwrap_or(0.0)
        .max(0.0)
        + border_padding;
    let maximum = style
        .margin_box_length(
            margin_box,
            if horizontal {
                "max-height"
            } else {
                "max-width"
            },
            geometry,
            thickness,
        )
        .map(|length| length.max(0.0) + border_padding)
        .unwrap_or(f32::INFINITY)
        .max(minimum);
    let length = length.clamp(minimum, maximum);
    if used.is_none() {
        before_value = Some(before_value.unwrap_or(0.0));
        after_value = Some(after_value.unwrap_or(0.0));
    } else if before_value.is_some() && after_value.is_some() {
        // An over-constrained top/left box ignores the leading margin;
        // bottom/right boxes ignore the trailing one.
        if matches!(edge, PageEdge::Top | PageEdge::Left) {
            before_value = None;
        } else {
            after_value = None;
        }
    }
    let remaining = thickness - length - before_value.unwrap_or(0.0) - after_value.unwrap_or(0.0);
    let offset = match (before_value, after_value) {
        (None, None) => remaining / 2.0,
        (None, Some(_)) => remaining,
        (Some(before), _) => before,
    };
    (offset, length)
}

fn edge_box_lengths(measures: [Option<EdgeBoxMeasure>; 3], available: f32) -> [f32; 3] {
    let mut sizes = measures.map(|measure| {
        measure.unwrap_or(EdgeBoxMeasure {
            specified: Some(0.0),
            ..EdgeBoxMeasure::default()
        })
    });
    for pass in 0..3 {
        let lengths = resolve_edge_lengths(sizes, available);
        let mut changed = false;
        for (index, measure) in measures.iter().enumerate() {
            let Some(measure) = measure else {
                continue;
            };
            let constraint = match pass {
                0 => measure.max_limit.filter(|max| lengths[index] > *max),
                _ => measure.min_limit.filter(|min| lengths[index] < *min),
            };
            if let Some(length) = constraint {
                sizes[index].specified = Some(length);
                changed = true;
            }
        }
        if !changed {
            if pass == 0 {
                continue;
            }
            return lengths;
        }
    }
    resolve_edge_lengths(sizes, available)
}

fn resolve_edge_lengths(measures: [EdgeBoxMeasure; 3], available: f32) -> [f32; 3] {
    let [first, middle, last] = measures;
    if !middle.generated {
        let (first_size, last_size) = distribute_pair(first, last, available);
        return [first_size, 0.0, last_size];
    }
    let imaginary_sides = EdgeBoxMeasure {
        specified: match (first.specified, last.specified) {
            (Some(first), Some(last)) => Some(2.0 * first.max(last)),
            _ => None,
        },
        min_content: 2.0
            * first
                .specified
                .unwrap_or(first.min_content)
                .max(last.specified.unwrap_or(last.min_content)),
        max_content: 2.0
            * first
                .specified
                .unwrap_or(first.max_content)
                .max(last.specified.unwrap_or(last.max_content)),
        ..EdgeBoxMeasure::default()
    };
    let (middle_size, _) = distribute_pair(middle, imaginary_sides, available);
    let side_space = ((available - middle_size) / 2.0).max(0.0);
    [
        first.specified.unwrap_or(side_space),
        middle_size,
        last.specified.unwrap_or(side_space),
    ]
}

fn distribute_pair(first: EdgeBoxMeasure, last: EdgeBoxMeasure, available: f32) -> (f32, f32) {
    match (first.specified, last.specified) {
        (Some(first), Some(last)) => (first, last),
        (Some(first), None) => (first, (available - first).max(0.0)),
        (None, Some(last)) => ((available - last).max(0.0), last),
        (None, None) => {
            let max_sum = first.max_content + last.max_content;
            let min_sum = first.min_content + last.min_content;
            let (base_first, base_last, weight_first, weight_last) = if max_sum < available {
                (
                    first.max_content,
                    last.max_content,
                    first.max_content,
                    last.max_content,
                )
            } else if min_sum < available {
                (
                    first.min_content,
                    last.min_content,
                    (first.max_content - first.min_content).max(0.0),
                    (last.max_content - last.min_content).max(0.0),
                )
            } else {
                (0.0, 0.0, first.min_content, last.min_content)
            };
            let weight_sum = weight_first + weight_last;
            let first_weight = if weight_sum > 0.0 {
                weight_first / weight_sum
            } else {
                0.5
            };
            let extra = (available - base_first - base_last).max(0.0);
            let first_size = base_first + extra * first_weight;
            (first_size, (available - first_size).max(0.0))
        }
    }
}

fn first_page_name(document: &NodeHandle, resolver: &mut StyleResolver) -> Option<String> {
    let body = find_body_node(document)?;
    body.child_nodes()
        .iter()
        .find(|child| child.node_type() == NodeType::Element)
        .and_then(|child| page_name(child, resolver))
        .or_else(|| page_name(&body, resolver))
}

fn first_body_break_side(document: &NodeHandle, resolver: &mut StyleResolver) -> Option<PageSide> {
    fn first_side(node: &NodeHandle, resolver: &mut StyleResolver) -> Option<PageSide> {
        for child in node.child_nodes() {
            match child.node_type() {
                NodeType::Text if child.data().is_some_and(|text| !text.trim().is_empty()) => {
                    // Text before this element forms an earlier in-flow box.
                    return None;
                }
                NodeType::Element => {}
                _ => continue,
            }
            let style = resolver.computed_style(&child);
            if matches!(
                style.get("display"),
                Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("none")
            ) || matches!(
                style.get("position"),
                Some(ComputedValue::Keyword(value))
                    if value.eq_ignore_ascii_case("absolute") || value.eq_ignore_ascii_case("fixed")
            ) {
                continue;
            }
            if let Some(side) =
                forced_page_break(style.get("break-before")).and_then(ForcedPageBreak::side)
            {
                return Some(side);
            }
            return can_fragment_style_descendants(&style)
                .then(|| first_side(&child, resolver))
                .flatten();
        }
        None
    }

    first_side(&find_body_node(document)?, resolver)
}

fn find_body_node(node: &NodeHandle) -> Option<NodeHandle> {
    if node.tag_name().as_deref() == Some("body") {
        return Some(node.clone());
    }
    node.child_nodes().iter().find_map(find_body_node)
}

fn page_name(node: &NodeHandle, resolver: &mut StyleResolver) -> Option<String> {
    let mut current = Some(node.clone());
    while let Some(node) = current {
        if node.node_type() == NodeType::Element
            && let Some(ComputedValue::Keyword(name)) = resolver.computed_style(&node).get("page")
            && !name.eq_ignore_ascii_case("auto")
        {
            return Some(name.clone());
        }
        current = node.parent_node();
    }
    None
}

fn find_body_layout(layout: &LayoutBox) -> Option<&LayoutBox> {
    if layout.node.tag_name().as_deref() == Some("body") {
        return Some(layout);
    }
    layout.children.iter().find_map(find_body_layout)
}

fn find_layout_for_node<'a>(layout: &'a LayoutBox, node: &NodeHandle) -> Option<&'a LayoutBox> {
    if layout.pseudo.is_none() && &layout.node == node {
        return Some(layout);
    }
    layout
        .children
        .iter()
        .find_map(|child| find_layout_for_node(child, node))
}

fn has_printable_flow(layout: &LayoutBox) -> bool {
    layout.dimensions.border_box().height > 0.0
        || !layout.lines.is_empty()
        || layout.children.iter().any(has_printable_flow)
}

fn is_in_flow_principal(layout: &LayoutBox) -> bool {
    layout.pseudo.is_none()
        && !matches!(
            layout.position_scheme,
            PositionScheme::Absolute | PositionScheme::Fixed
        )
}

fn can_fragment_descendants(layout: &LayoutBox, resolver: &mut StyleResolver) -> bool {
    if layout.node.tag_name().is_none() {
        return true;
    }
    let style = resolver.computed_style(&layout.node);
    can_fragment_style_descendants(&style)
}

fn can_fragment_style_descendants(style: &ComputedStyle) -> bool {
    let unsupported_display = matches!(
        style.get("display"),
        Some(ComputedValue::Keyword(value))
            if matches!(
                value.to_ascii_lowercase().as_str(),
                "inline-table" | "table-row" | "table-cell"
                    | "flex" | "inline-flex" | "grid" | "inline-grid"
            )
    );
    let floating = matches!(
        style.get("float"),
        Some(ComputedValue::Keyword(value)) if !value.eq_ignore_ascii_case("none")
    );
    !unsupported_display && !floating
}

#[derive(Clone, Copy)]
struct PageBreakCandidate {
    y: f32,
    line_group: Option<usize>,
    line_index: usize,
}

struct PageLineGroup {
    line_ends: Vec<f32>,
    orphans: usize,
    widows: usize,
}

struct PageAvoidRange {
    start: f32,
    end: f32,
}

#[derive(Default)]
struct PageBreakCandidates {
    points: Vec<PageBreakCandidate>,
    line_groups: Vec<PageLineGroup>,
    avoid_ranges: Vec<PageAvoidRange>,
}

fn collect_page_avoid_range(
    layout: &LayoutBox,
    style: &ComputedStyle,
    breaks: &mut PageBreakCandidates,
) {
    if !matches!(
        style.get("break-inside"),
        Some(ComputedValue::Keyword(value))
            if value.eq_ignore_ascii_case("avoid") || value.eq_ignore_ascii_case("avoid-page")
    ) {
        return;
    }
    let bounds = layout.dimensions.border_box();
    let end = bounds.y + bounds.height;
    if bounds.y.is_finite() && end.is_finite() && end > bounds.y {
        breaks.avoid_ranges.push(PageAvoidRange {
            start: bounds.y,
            end,
        });
    }
}

impl PageBreakCandidates {
    fn is_preferred(&self, candidate: &PageBreakCandidate, cursor_y: f32) -> bool {
        let Some(group_index) = candidate.line_group else {
            return true;
        };
        let group = &self.line_groups[group_index];
        let lines_after = group.line_ends.len() - candidate.line_index - 1;
        if lines_after == 0 {
            return true;
        }
        let consumed = group
            .line_ends
            .partition_point(|&end| end <= cursor_y + 0.01);
        let lines_here = (candidate.line_index + 1).saturating_sub(consumed);
        lines_here >= group.orphans && lines_after >= group.widows
    }
}

fn page_line_minima(resolver: &mut StyleResolver, layout: &LayoutBox) -> (usize, usize) {
    let Some(_) = layout.node.tag_name() else {
        return (2, 2);
    };
    let style = resolver.computed_style(&layout.node);
    let minimum = |name| match style.get(name) {
        Some(ComputedValue::Number(value)) if value.is_finite() && *value >= 1.0 => *value as usize,
        _ => 2,
    };
    (minimum("orphans"), minimum("widows"))
}

fn page_break_candidates(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    include_nested_children: bool,
) -> PageBreakCandidates {
    fn collect(
        parent: &LayoutBox,
        resolver: &mut StyleResolver,
        breaks: &mut PageBreakCandidates,
        include_nested_children: bool,
    ) {
        if parent.node.tag_name().is_some() {
            let style = resolver.computed_style(&parent.node);
            collect_page_avoid_range(parent, &style, breaks);
            if table_display_for_node(&parent.node, &style) == Some(TableDisplay::Table) {
                collect_table_row_breaks(parent, resolver, breaks);
                return;
            }
        }
        let mut line_ends = parent
            .lines
            .iter()
            .map(|line| line.rect.y + line.rect.height)
            .filter(|boundary| boundary.is_finite())
            .collect::<Vec<_>>();
        if !line_ends.is_empty() {
            line_ends.sort_by(f32::total_cmp);
            let (orphans, widows) = page_line_minima(resolver, parent);
            let group_index = breaks.line_groups.len();
            for (line_index, &y) in line_ends.iter().enumerate() {
                breaks.points.push(PageBreakCandidate {
                    y,
                    line_group: Some(group_index),
                    line_index,
                });
            }
            breaks.line_groups.push(PageLineGroup {
                line_ends,
                orphans,
                widows,
            });
        }
        let mut seen_block = false;
        for child in parent
            .children
            .iter()
            .filter(|child| is_in_flow_principal(child))
        {
            if !can_fragment_descendants(child, resolver) {
                continue;
            }
            let block = child.node.tag_name().is_some_and(|_| {
                let style = resolver.computed_style(&child.node);
                let display = match style.get("display") {
                    Some(ComputedValue::Keyword(value)) => value.to_ascii_lowercase(),
                    _ => String::new(),
                };
                matches!(display.as_str(), "block" | "flow-root" | "list-item")
            });
            if include_nested_children && block && seen_block {
                let boundary = child.dimensions.border_box().y - child.dimensions.margin.top;
                if boundary.is_finite() {
                    breaks.points.push(PageBreakCandidate {
                        y: boundary,
                        line_group: None,
                        line_index: 0,
                    });
                }
            }
            seen_block |= block;
            collect(child, resolver, breaks, include_nested_children);
        }
    }

    let mut breaks = PageBreakCandidates::default();
    if let Some(body) = find_body_layout(layout) {
        for child in body
            .children
            .iter()
            .filter(|child| is_in_flow_principal(child))
        {
            if can_fragment_descendants(child, resolver) {
                collect(child, resolver, &mut breaks, include_nested_children);
            }
        }
    }
    breaks
        .points
        .sort_by(|left, right| left.y.total_cmp(&right.y));
    breaks.points.dedup_by(|left, right| {
        (left.y - right.y).abs() < 0.01 && left.line_group == right.line_group
    });
    breaks
        .avoid_ranges
        .sort_by(|left, right| left.start.total_cmp(&right.start));
    breaks
}

fn collect_table_row_breaks(
    table: &LayoutBox,
    resolver: &mut StyleResolver,
    breaks: &mut PageBreakCandidates,
) {
    let mut rows = Vec::new();
    for child in table
        .children
        .iter()
        .filter(|child| is_in_flow_principal(child))
    {
        let style = resolver.computed_style(&child.node);
        match table_display_for_node(&child.node, &style) {
            Some(TableDisplay::Row) => rows.push(child),
            Some(TableDisplay::RowGroup) => {
                rows.extend(child.children.iter().filter(|row| {
                    if !is_in_flow_principal(row) {
                        return false;
                    }
                    let style = resolver.computed_style(&row.node);
                    table_display_for_node(&row.node, &style) == Some(TableDisplay::Row)
                }));
            }
            _ => {}
        }
    }

    let mut span_end = 0;
    for (index, row) in rows.iter().enumerate() {
        let style = resolver.computed_style(&row.node);
        collect_page_avoid_range(row, &style, breaks);
        for cell in &row.children {
            let span = html_table_span_attribute(&cell.node, "rowspan").unwrap_or(1);
            span_end = span_end.max(index.saturating_add(span).min(rows.len()));
        }
        if span_end <= index + 1 {
            let boundary = row.dimensions.border_box().y + row.dimensions.border_box().height;
            if boundary.is_finite() {
                breaks.points.push(PageBreakCandidate {
                    y: boundary,
                    line_group: None,
                    line_index: 0,
                });
            }
        }
    }
}

fn flow_sections(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    content_start: f32,
) -> Vec<FlowSection> {
    let mut flow_end = layout
        .scrollable_overflow()
        .1
        .max(layout.dimensions.content.height)
        + layout.dimensions.content.y;
    let Some(body) = find_body_layout(layout) else {
        return vec![FlowSection {
            name: None,
            start: content_start,
            end: flow_end.max(content_start),
            requested_side: None,
        }];
    };
    // The root's default margins can extend scroll geometry even when there
    // is no body content. Preserve a margin-only printed page in that case.
    if !has_printable_flow(body) {
        flow_end = content_start;
    }
    fn collect_boundaries(
        box_: &LayoutBox,
        resolver: &mut StyleResolver,
        boundaries: &mut Vec<FlowBoundary>,
        first_in_flow: bool,
        inherited_name: &Option<String>,
    ) {
        let style = resolver.computed_style(&box_.node);
        let name = match style.get("page") {
            Some(ComputedValue::Keyword(value)) if !value.eq_ignore_ascii_case("auto") => {
                Some(value.clone())
            }
            _ => inherited_name.clone(),
        };
        let before = forced_page_break(style.get("break-before"));
        let after = forced_page_break(style.get("break-after"));
        let border = box_.dimensions.border_box();
        boundaries.push(FlowBoundary {
            name: name.clone(),
            position: border.y - box_.dimensions.margin.top,
            forced_break: before,
            first_in_flow,
        });
        if can_fragment_descendants(box_, resolver) {
            for (index, child) in box_
                .children
                .iter()
                .filter(|child| is_in_flow_principal(child))
                .enumerate()
            {
                collect_boundaries(
                    child,
                    resolver,
                    boundaries,
                    first_in_flow && index == 0,
                    &name,
                );
            }
        }
        if after.is_some() {
            boundaries.push(FlowBoundary {
                name,
                position: border.y + border.height + box_.dimensions.margin.bottom,
                forced_break: after,
                first_in_flow: false,
            });
        }
    }

    let mut boundaries = Vec::new();
    let body_name = page_name(&body.node, resolver);
    for (index, child) in body
        .children
        .iter()
        .filter(|child| is_in_flow_principal(child))
        .enumerate()
    {
        collect_boundaries(child, resolver, &mut boundaries, index == 0, &body_name);
    }
    let mut sections = Vec::new();
    let mut current_name = boundaries
        .first()
        .and_then(|boundary| boundary.name.clone());
    let mut start = content_start;
    let mut requested_side = None;
    for boundary in boundaries {
        let name = boundary.name;
        let forced_break = boundary.forced_break;
        if name != current_name || forced_break.is_some() {
            let position = if boundary.first_in_flow && forced_break.is_some() {
                // A break before the first in-flow child propagates to the
                // root; do not manufacture a page for the body top margin.
                start
            } else {
                boundary.position.max(start).min(flow_end)
            };
            if position > start {
                sections.push(FlowSection {
                    name: current_name,
                    start,
                    end: position,
                    requested_side,
                });
                requested_side = None;
            }
            current_name = name;
            start = position;
            if let Some(side) = forced_break.and_then(ForcedPageBreak::side) {
                requested_side = Some(side);
            }
        }
    }
    if flow_end > start || sections.is_empty() {
        sections.push(FlowSection {
            name: current_name,
            start,
            end: flow_end.max(start),
            requested_side,
        });
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::{Origin, parse_stylesheet};

    #[test]
    fn paginated_paragraph_stops_at_a_complete_line() {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(NodeHandle::text(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz".repeat(4),
        ));
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "@page { size: 80px 45px; margin: 0 } body, p { margin: 0 } \
                 p { font-size: 10px; line-height: 20px; word-break: break-all }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 80.0,
                height: 45.0,
                ..Rect::default()
            },
        )
        .unwrap();

        assert!(paged.pages.len() >= 2);
        assert_eq!(paged.pages[0].fragments[0].end.flow_y(), 40.0);
        assert!(
            paged.pages[1].fragments[0]
                .start
                .inline_token()
                .is_some_and(|token| token > 0)
        );
    }

    #[test]
    fn nested_block_children_produce_a_continuation_at_the_child_boundary() {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let parent = NodeHandle::element("section");
        let first = NodeHandle::element("div");
        let second = NodeHandle::element("div");
        let after = NodeHandle::element("aside");
        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(parent.clone());
        parent.append_child(first);
        parent.append_child(second);
        body.append_child(after);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "@page { size: 100px 50px; margin: 0 } body { margin: 0 } \
                 section { border: 2px solid black } section > div { height: 30px } \
                 aside { height: 15px }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 100.0,
                height: 50.0,
                ..Rect::default()
            },
        )
        .unwrap();

        assert_eq!(paged.pages.len(), 2);
        assert_eq!(paged.pages[0].fragments[0].end.flow_y(), 32.0);
        assert_eq!(paged.pages[1].fragments[0].start.flow_y(), 32.0);
        assert_eq!(paged.pages[1].fragments[0].source.height, 47.0);
    }

    fn assert_width_change_preserves_inline_text(
        first_width: u32,
        later_width: u32,
        repetitions: usize,
    ) {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        let text = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz".repeat(repetitions);
        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(NodeHandle::text(&text));
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(&format!(
                "body, p {{ margin: 0 }} p {{ font-size: 10px; line-height: 20px; word-break: break-all }} \
                 @page {{ size: {later_width}px 40px; margin: 0 }} \
                 @page :first {{ size: {first_width}px 40px; margin: 0 }}",
            ))
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: first_width as f32,
                height: 40.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert!(paged.pages.len() >= 2);
        let mut printed = String::new();
        for page in &paged.pages {
            let body = find_body_layout(paged.layout_for_page(page)).unwrap();
            let paragraph = body
                .children
                .iter()
                .find(|child| child.node == paragraph)
                .unwrap();
            for line in &paragraph.lines {
                if line.rect.y >= page.source.y && line.rect.y < page.source.y + page.source.height
                {
                    for fragment in &line.fragments {
                        if let Some(text) = fragment.text() {
                            printed.push_str(text);
                        }
                    }
                }
            }
        }
        assert_eq!(printed, text);
    }

    #[test]
    fn widening_page_does_not_drop_or_repeat_inline_text() {
        assert_width_change_preserves_inline_text(80, 200, 1);
    }

    #[test]
    fn narrowing_page_does_not_drop_or_repeat_inline_text() {
        assert_width_change_preserves_inline_text(200, 80, 2);
    }

    #[test]
    fn continuation_tracks_text_across_adjacent_nodes_and_inline_elements() {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        let span = NodeHandle::element("span");
        let expected = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(NodeHandle::text("ABCDEFGHIJKLMN"));
        paragraph.append_child(NodeHandle::text("OPQRSTUV"));
        span.append_child(NodeHandle::text("WXYZabcdefghijklmnopqrstuvwxyz"));
        paragraph.append_child(span);
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body, p { margin: 0 } p { font-size: 10px; line-height: 20px; word-break: break-all } \
                 @page { size: 200px 40px; margin: 0 } \
                 @page :first { size: 80px 40px; margin: 0 }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 80.0,
                height: 40.0,
                ..Rect::default()
            },
        )
        .unwrap();
        let mut printed = String::new();
        for page in &paged.pages {
            let body = find_body_layout(paged.layout_for_page(page)).unwrap();
            let paragraph = body
                .children
                .iter()
                .find(|child| child.node == paragraph)
                .unwrap();
            for line in &paragraph.lines {
                if line.rect.y >= page.source.y && line.rect.y < page.source.y + page.source.height
                {
                    for fragment in &line.fragments {
                        if let Some(text) = fragment.text() {
                            printed.push_str(text);
                        }
                    }
                }
            }
        }
        assert_eq!(printed, expected);
    }

    #[test]
    fn continuation_preserves_text_across_multiple_paragraphs() {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let first = NodeHandle::element("p");
        let second = NodeHandle::element("p");
        let first_text = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let second_text = "0123456789";
        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(first.clone());
        body.append_child(second.clone());
        first.append_child(NodeHandle::text(first_text));
        second.append_child(NodeHandle::text(second_text));
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body, p { margin: 0 } p { font-size: 10px; line-height: 20px; word-break: break-all } \
                 @page { size: 200px 40px; margin: 0 } \
                 @page :first { size: 80px 40px; margin: 0 }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 80.0,
                height: 40.0,
                ..Rect::default()
            },
        )
        .unwrap();
        let mut first_printed = String::new();
        let mut second_printed = String::new();
        for page in &paged.pages {
            let body = find_body_layout(paged.layout_for_page(page)).unwrap();
            for (node, printed) in [(&first, &mut first_printed), (&second, &mut second_printed)] {
                let paragraph = body
                    .children
                    .iter()
                    .find(|child| &child.node == node)
                    .unwrap();
                for line in &paragraph.lines {
                    if line.rect.y >= page.source.y
                        && line.rect.y < page.source.y + page.source.height
                    {
                        for fragment in &line.fragments {
                            if let Some(text) = fragment.text() {
                                printed.push_str(text);
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(first_printed, first_text);
        assert_eq!(second_printed, second_text);
    }

    #[test]
    fn named_page_text_uses_its_own_line_count_and_page_extent() {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let first = NodeHandle::element("p");
        let second = NodeHandle::element("p");
        first.set_attribute("style", "page: narrow; height: 40px");
        second.set_attribute("style", "page: wide");
        first.append_child(NodeHandle::text("FIRST"));
        second.append_child(NodeHandle::text("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmn"));
        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(first);
        body.append_child(second.clone());
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body, p { margin: 0 } p { font-size: 10px; line-height: 20px; word-break: break-all } \
                 @page narrow { size: 80px 40px; margin: 0 } \
                 @page wide { size: 200px 40px; margin: 0 }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 80.0,
                height: 40.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(paged.pages.len(), 2);
        let body = find_body_layout(paged.layout_for_page(&paged.pages[1])).unwrap();
        let second_box = body
            .children
            .iter()
            .find(|child| child.node == second)
            .unwrap();
        assert_eq!(second_box.lines.len(), 2);
    }

    #[test]
    fn named_page_continuation_keeps_text_when_later_pages_have_a_new_width() {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let first = NodeHandle::element("p");
        let second = NodeHandle::element("p");
        let text = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz".repeat(2);
        first.set_attribute("style", "page: narrow; height: 40px");
        second.set_attribute("style", "page: wide");
        first.append_child(NodeHandle::text("FIRST"));
        second.append_child(NodeHandle::text(&text));
        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(first);
        body.append_child(second.clone());
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body, p { margin: 0 } p { font-size: 10px; line-height: 20px; word-break: break-all } \
                 @page narrow { size: 80px 40px; margin: 0 } \
                 @page wide:left { size: 80px 40px; margin: 0 } \
                 @page wide:right { size: 200px 40px; margin: 0 }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 80.0,
                height: 40.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert!(paged.pages.len() >= 3);
        let mut printed = String::new();
        for page in &paged.pages {
            let body = find_body_layout(paged.layout_for_page(page)).unwrap();
            let paragraph = body
                .children
                .iter()
                .find(|child| child.node == second)
                .unwrap();
            for line in &paragraph.lines {
                if line.rect.y >= page.source.y && line.rect.y < page.source.y + page.source.height
                {
                    for fragment in &line.fragments {
                        if let Some(text) = fragment.text() {
                            printed.push_str(text);
                        }
                    }
                }
            }
        }
        assert_eq!(printed, text);
    }

    fn document_with_two_boxes() -> (NodeHandle, NodeHandle, NodeHandle) {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let first = NodeHandle::element("div");
        let second = NodeHandle::element("div");
        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(first.clone());
        body.append_child(second.clone());
        (document, first, second)
    }

    #[test]
    fn named_pages_use_their_own_page_contexts() {
        let (document, first, second) = document_with_two_boxes();
        first.set_attribute("style", "page: a");
        second.set_attribute("style", "page: b");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            crate::css::Origin::Author,
            parse_stylesheet(
                "body { margin: 0 } div { width: 20px; height: 20px } \
                 @page a { margin: 1cm } @page b { margin: 2cm }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 400.0,
                height: 400.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(paged.pages.len(), 2);
        assert_eq!(paged.pages[0].selector.name.as_deref(), Some("a"));
        assert_eq!(paged.pages[1].selector.name.as_deref(), Some("b"));
        assert!((paged.pages[0].content.x - 96.0 / 2.54).abs() < 0.01);
        assert!((paged.pages[1].content.x - 2.0 * 96.0 / 2.54).abs() < 0.01);
        assert!(paged.pages[1].source.y >= paged.pages[0].source.y);
    }

    #[test]
    fn print_media_and_overflow_create_multiple_pages() {
        let (document, first, _) = document_with_two_boxes();
        first.set_attribute("style", "height: 250px");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body { margin: 0 } @media screen { div { width: 10px } } \
                 @media print { div { width: 50px } } @page { margin: 0 }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 100.0,
                height: 100.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(resolver.media_type(), MediaType::Print);
        assert!(paged.pages.len() >= 3);
        let body = find_body_layout(&paged.layout).unwrap();
        assert!((body.children[0].dimensions.content.width - 50.0).abs() < 0.01);
    }

    #[test]
    fn page_fragments_advance_through_oversized_flow_without_gaps() {
        let (document, first, _) = document_with_two_boxes();
        first.set_attribute("style", "height: 250px");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("body { margin: 0 } @page { size: 100px 100px; margin: 0 }").unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 100.0,
                height: 100.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(paged.pages.len(), 3);
        let first_y = paged.pages[0].fragments[0].start.flow_y();
        let last_y = paged.pages.last().unwrap().fragments[0].end.flow_y();
        let mut covered = 0.0;
        for (index, page) in paged.pages.iter().enumerate() {
            assert_eq!(page.selector.index, index);
            assert_eq!(page.fragments.len(), 1);
            let fragment = &page.fragments[0];
            assert_eq!(fragment.start.section_index(), 0);
            assert_eq!(fragment.end.section_index(), 0);
            assert_eq!(fragment.source, page.source);
            assert_eq!(fragment.destination.x, page.content.x);
            assert_eq!(fragment.destination.y, page.content.y);
            assert_eq!(fragment.destination.height, fragment.source.height);
            assert!(fragment.source.height > 0.0);
            assert!(fragment.source.height <= page.content.height);
            covered += fragment.source.height;
            assert_eq!(
                page.continuation,
                paged
                    .pages
                    .get(index + 1)
                    .map(|next| next.fragments[0].start)
            );
        }
        assert_eq!(covered, last_y - first_y);
    }

    #[test]
    fn exact_page_boundary_does_not_create_an_extra_fragment() {
        let (document, first, _) = document_with_two_boxes();
        first.set_attribute("style", "height: 200px");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("body { margin: 0 } @page { size: 100px 100px; margin: 0 }").unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 100.0,
                height: 100.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(paged.pages.len(), 2);
        assert_eq!(paged.pages[0].fragments[0].source.height, 100.0);
        assert_eq!(paged.pages[1].fragments[0].source.height, 100.0);
        assert_eq!(
            paged.pages[0].continuation,
            Some(paged.pages[1].fragments[0].start)
        );
        assert_eq!(paged.pages[1].continuation, None);
    }

    #[test]
    fn continuation_moves_to_the_next_named_page_section() {
        let (document, first, second) = document_with_two_boxes();
        first.set_attribute("style", "page: a; height: 20px");
        second.set_attribute("style", "page: b; height: 20px");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body { margin: 0 } @page a { size: 100px 100px; margin: 0 } \
                 @page b { size: 100px 100px; margin: 0 }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 100.0,
                height: 100.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(paged.pages.len(), 2);
        let first = &paged.pages[0];
        let second = &paged.pages[1];
        assert_eq!(first.fragments[0].start.section_index(), 0);
        assert_eq!(second.fragments[0].start.section_index(), 1);
        assert_eq!(
            (first.selector.index, first.selector.name.as_deref()),
            (0, Some("a"))
        );
        assert_eq!(
            (second.selector.index, second.selector.name.as_deref()),
            (1, Some("b"))
        );
        assert_eq!(first.continuation, Some(second.fragments[0].start));
        assert_eq!(
            first.fragments[0].end.flow_y(),
            second.fragments[0].start.flow_y()
        );
        assert_eq!(second.continuation, None);
    }

    #[test]
    fn page_with_no_printable_height_does_not_discard_flow() {
        let (document, first, _) = document_with_two_boxes();
        first.set_attribute("style", "height: 20px");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("body { margin: 0 } @page { size: 100px 100px; margin: 60px }")
                .unwrap(),
        );
        assert!(
            layout_paged_tree(
                &document,
                &mut resolver,
                Rect {
                    width: 100.0,
                    height: 100.0,
                    ..Rect::default()
                }
            )
            .is_none()
        );
    }

    #[test]
    fn page_limit_returns_failure_instead_of_truncated_output() {
        let (document, first, _) = document_with_two_boxes();
        first.set_attribute("style", "height: 1025px");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("body { margin: 0 } @page { size: 100px 1px; margin: 0 }").unwrap(),
        );
        assert!(
            layout_paged_tree(
                &document,
                &mut resolver,
                Rect {
                    width: 100.0,
                    height: 1.0,
                    ..Rect::default()
                }
            )
            .is_none()
        );
    }

    #[test]
    fn rtl_page_progression_starts_on_left() {
        let (document, _, _) = document_with_two_boxes();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "html { direction: rtl } @page :left { margin-left: 11px } \
                 @page :right { margin-left: 22px }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 200.0,
                height: 200.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(paged.pages[0].selector.side, PageSide::Left);
        assert_eq!(paged.pages[0].content.x, 11.0);
    }

    #[test]
    fn side_breaks_before_and_after_body_children_insert_only_needed_blank_pages() {
        for (direction, property, requested, expect_blank) in [
            ("ltr", "break-before: left", PageSide::Left, false),
            ("ltr", "break-before: right", PageSide::Right, true),
            ("rtl", "break-before: right", PageSide::Right, false),
            ("rtl", "break-before: left", PageSide::Left, true),
            ("ltr", "break-after: left", PageSide::Left, false),
            ("ltr", "break-after: right", PageSide::Right, true),
            ("rtl", "break-after: right", PageSide::Right, false),
            ("rtl", "break-after: left", PageSide::Left, true),
        ] {
            let (document, first, second) = document_with_two_boxes();
            first.set_attribute("style", "height: 20px");
            second.set_attribute("style", "height: 20px");
            if property.starts_with("break-before") {
                second.set_attribute("style", format!("height: 20px; {property}"));
            } else {
                first.set_attribute("style", format!("height: 20px; {property}"));
            }
            let mut resolver = StyleResolver::new();
            resolver.add_stylesheet(
                Origin::Author,
                parse_stylesheet(&format!(
                    "html {{ direction: {direction} }} body {{ margin: 0 }} \
                     @page {{ size: 100px 100px; margin: 0 }}"
                ))
                .unwrap(),
            );
            let paged = layout_paged_tree(
                &document,
                &mut resolver,
                Rect {
                    width: 100.0,
                    height: 100.0,
                    ..Rect::default()
                },
            )
            .unwrap();
            assert_eq!(
                paged.pages.len(),
                if expect_blank { 3 } else { 2 },
                "{direction} {property}"
            );
            let first_page = &paged.pages[0];
            let last_page = paged.pages.last().unwrap();
            assert!(!first_page.selector.blank);
            assert!(!last_page.selector.blank);
            assert_eq!(last_page.selector.side, requested, "{direction} {property}");
            assert_eq!(first_page.fragments.len(), 1);
            assert_eq!(last_page.fragments.len(), 1);
            assert_eq!(
                first_page.fragments[0].end.flow_y(),
                last_page.fragments[0].start.flow_y(),
                "{direction} {property}"
            );
            if expect_blank {
                let blank = &paged.pages[1];
                assert!(blank.selector.blank, "{direction} {property}");
                assert!(blank.fragments.is_empty());
                assert_eq!(blank.source.height, 0.0);
                assert_eq!(blank.continuation, Some(last_page.fragments[0].start));
                assert_eq!(blank.counters.page, 2);
                assert_eq!(blank.counters.pages, 3);
            }
        }
    }

    #[test]
    fn first_child_side_break_selects_the_first_printed_page_without_a_blank() {
        for (direction, property, expected) in [
            ("ltr", "break-before: left", PageSide::Left),
            ("rtl", "break-before: right", PageSide::Right),
        ] {
            let (document, first, _) = document_with_two_boxes();
            first.set_attribute("style", format!("height: 20px; {property}"));
            let mut resolver = StyleResolver::new();
            resolver.add_stylesheet(
                Origin::Author,
                parse_stylesheet(&format!(
                    "html {{ direction: {direction} }} body {{ margin: 0 }} \
                     @page {{ size: 100px 100px; margin: 0 }}"
                ))
                .unwrap(),
            );
            let paged = layout_paged_tree(
                &document,
                &mut resolver,
                Rect {
                    width: 100.0,
                    height: 100.0,
                    ..Rect::default()
                },
            )
            .unwrap();
            assert_eq!(paged.pages.len(), 1, "{direction} {property}");
            assert_eq!(paged.pages[0].selector.side, expected);
            assert!(!paged.pages[0].selector.blank);
            assert_eq!(paged.pages[0].selector.index, 0);
        }
    }

    #[test]
    fn later_before_side_wins_over_earlier_after_side() {
        let (document, first, second) = document_with_two_boxes();
        first.set_attribute("style", "height: 20px; break-after: left");
        second.set_attribute("style", "height: 20px; break-before: right");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("body { margin: 0 } @page { size: 100px 100px; margin: 0 }").unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 100.0,
                height: 100.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(paged.pages.len(), 3);
        assert!(paged.pages[1].selector.blank);
        assert_eq!(paged.pages[2].selector.side, PageSide::Right);
    }

    #[test]
    fn side_break_uses_page_number_after_an_oversized_previous_box() {
        let (document, first, second) = document_with_two_boxes();
        first.set_attribute("style", "height: 180px");
        second.set_attribute("style", "height: 20px; break-before: left");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("body { margin: 0 } @page { size: 100px 100px; margin: 0 }").unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 100.0,
                height: 100.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(paged.pages.len(), 4);
        assert_eq!(paged.pages[0].selector.side, PageSide::Right);
        assert_eq!(paged.pages[1].selector.side, PageSide::Left);
        assert!(paged.pages[2].selector.blank);
        assert_eq!(paged.pages[2].selector.side, PageSide::Right);
        assert_eq!(paged.pages[3].selector.side, PageSide::Left);
        assert_eq!(paged.pages[3].fragments[0].start.flow_y(), 180.0);
    }

    #[test]
    fn ordinary_page_break_does_not_insert_a_blank_page() {
        let (document, first, second) = document_with_two_boxes();
        first.set_attribute("style", "height: 20px");
        second.set_attribute("style", "height: 20px; break-before: page");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("body { margin: 0 } @page { size: 100px 100px; margin: 0 }").unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 100.0,
                height: 100.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(paged.pages.len(), 2);
        assert!(paged.pages.iter().all(|page| !page.selector.blank));
        assert_eq!(paged.pages[1].selector.side, PageSide::Left);
    }

    #[test]
    fn page_margin_counters_use_current_and_final_page_count() {
        let (document, first, _) = document_with_two_boxes();
        first.set_attribute("style", "height: 250px");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body { margin: 0 } @page { margin: 0; \
                 @top-center { content: 'Page ' counter(page) ' / ' counter(pages) } }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 100.0,
                height: 100.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert!(paged.pages.len() >= 3);
        for (index, page) in paged.pages.iter().enumerate() {
            assert_eq!(page.counters.page, (index + 1) as i32);
            assert_eq!(page.counters.pages, paged.pages.len());
            assert_eq!(
                page.margin_box_text(PageMarginBox::TopCenter),
                Some(format!("Page {} / {}", index + 1, paged.pages.len()))
            );
        }
    }

    #[test]
    fn page_counter_reset_and_increment_do_not_change_total_pages() {
        let (document, first, _) = document_with_two_boxes();
        first.set_attribute("style", "height: 250px");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body { margin: 0 } \
                 @page { margin: 0; counter-reset: pages 99; \
                         counter-increment: page 2 pages 8; \
                         @top-center { content: counter(page) '/' counter(pages) } } \
                 @page :first { counter-reset: page 4 pages 500 }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 100.0,
                height: 100.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert!(paged.pages.len() >= 3);
        for (index, page) in paged.pages.iter().enumerate() {
            assert_eq!(page.counters.page, 6 + index as i32 * 2);
            assert_eq!(page.counters.pages, paged.pages.len());
            assert_eq!(
                page.margin_box_text(PageMarginBox::TopCenter),
                Some(format!("{}/{}", 6 + index as i32 * 2, paged.pages.len()))
            );
        }
    }

    #[test]
    fn named_and_left_pages_keep_margin_counter_scope_local() {
        let (document, first, second) = document_with_two_boxes();
        first.set_attribute("style", "page: a");
        second.set_attribute("style", "page: b");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body { margin: 0 } div { width: 20px; height: 20px } \
                 @page a:right { @top-center { content: 'A' counter(page) '/' counter(pages) } } \
                 @page b:left { counter-reset: page 20; \
                   @top-center { counter-increment: page 2 pages 77; \
                                 content: 'B' counter(page) '/' counter(pages) } \
                   @top-left { counter-reset: page 7; content: counter(page) } }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 400.0,
                height: 400.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(paged.pages.len(), 2);
        assert_eq!(
            paged.pages[0].margin_box_text(PageMarginBox::TopCenter),
            Some("A1/2".into())
        );
        assert_eq!(paged.pages[1].counters.page, 21);
        assert_eq!(
            paged.pages[1].margin_box_text(PageMarginBox::TopCenter),
            Some("B23/2".into())
        );
        assert_eq!(
            paged.pages[1].margin_box_text(PageMarginBox::TopLeft),
            Some("7".into())
        );
        assert_eq!(paged.pages[1].counters.page, 21);
    }

    #[test]
    fn corner_margin_boxes_follow_asymmetric_page_margins() {
        let (document, _, _) = document_with_two_boxes();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "@page { size: 200px 300px; margin: 10px 20px 30px 40px; \
                   @top-left-corner { content: 'tl' } \
                   @top-right-corner { content: 'tr' } \
                   @bottom-left-corner { content: 'bl' } \
                   @bottom-right-corner { content: 'br' } }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 200.0,
                height: 300.0,
                ..Rect::default()
            },
        )
        .unwrap();
        let page = &paged.pages[0];
        assert_eq!(
            page.margin_box_rect(PageMarginBox::TopLeftCorner),
            Some(Rect {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 10.0
            })
        );
        assert_eq!(
            page.margin_box_rect(PageMarginBox::TopRightCorner),
            Some(Rect {
                x: 180.0,
                y: 0.0,
                width: 20.0,
                height: 10.0
            })
        );
        assert_eq!(
            page.margin_box_rect(PageMarginBox::BottomLeftCorner),
            Some(Rect {
                x: 0.0,
                y: 270.0,
                width: 40.0,
                height: 30.0
            })
        );
        assert_eq!(
            page.margin_box_rect(PageMarginBox::BottomRightCorner),
            Some(Rect {
                x: 180.0,
                y: 270.0,
                width: 20.0,
                height: 30.0
            })
        );
        assert_eq!(page.margin_box_rect(PageMarginBox::TopCenter), None);
        for rect in page.margin_box_rects.values() {
            assert!(
                rect.x + rect.width <= page.content.x
                    || rect.x >= page.content.x + page.content.width
                    || rect.y + rect.height <= page.content.y
                    || rect.y >= page.content.y + page.content.height
            );
        }
    }

    #[test]
    fn named_pages_use_their_own_corner_rectangles_and_landscape_sheet() {
        let (document, first, second) = document_with_two_boxes();
        first.set_attribute("style", "page: portrait");
        second.set_attribute("style", "page: landscape");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body { margin: 0 } div { width: 20px; height: 20px } \
                 @page portrait { size: 200px 300px; margin: 10px 20px 30px 40px; \
                     @top-left-corner { content: 'p' } } \
                 @page landscape { size: 300px 200px; margin: 5px 6px 7px 8px; \
                     @bottom-right-corner { content: 'l' } }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 200.0,
                height: 300.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(paged.pages.len(), 2);
        assert_eq!(
            paged.pages[0].margin_box_rect(PageMarginBox::TopLeftCorner),
            Some(Rect {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 10.0
            })
        );
        assert_eq!(
            paged.pages[0].margin_box_rect(PageMarginBox::BottomRightCorner),
            None
        );
        assert_eq!(paged.pages[1].sheet.width, 300.0);
        assert_eq!(paged.pages[1].sheet.height, 200.0);
        assert_eq!(
            paged.pages[1].margin_box_rect(PageMarginBox::BottomRightCorner),
            Some(Rect {
                x: 294.0,
                y: 193.0,
                width: 6.0,
                height: 7.0
            })
        );
        assert_eq!(
            paged.pages[1].margin_box_rect(PageMarginBox::TopLeftCorner),
            None
        );
    }

    #[test]
    fn zero_and_overfull_margins_keep_corner_rectangles_finite() {
        for (css, sheet, expected_width) in [
            (
                "@page { size: 20px 10px; margin: 0; @top-left-corner { content: '' } @bottom-right-corner { content: 'x' } }",
                (20.0, 10.0),
                0.0,
            ),
            (
                "@page { size: 20px 10px; margin: 15px; @top-left-corner { content: '' } @bottom-right-corner { content: 'x' } }",
                (20.0, 10.0),
                15.0,
            ),
        ] {
            let mut resolver = StyleResolver::new();
            resolver.set_media_type(MediaType::Print);
            resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
            let style = resolver.resolved_page_style(&PageSelectorContext::new(0, None));
            let geometry = style.geometry(sheet, 0.0);
            let rectangles = corner_margin_box_rects(&style, geometry);
            let top_left = rectangles.get(&PageMarginBox::TopLeftCorner).unwrap();
            assert_eq!(top_left.width, expected_width);
            for rect in rectangles.values() {
                assert!(
                    rect.x.is_finite()
                        && rect.y.is_finite()
                        && rect.width.is_finite()
                        && rect.height.is_finite()
                );
                assert!(rect.width >= 0.0 && rect.height >= 0.0);
                assert!(rect.x >= 0.0 && rect.y >= 0.0);
                assert!(rect.x + rect.width <= geometry.width);
                assert!(rect.y + rect.height <= geometry.height);
            }
        }
    }

    #[test]
    fn all_twelve_edge_boxes_use_the_available_side_between_corners() {
        let (document, _, _) = document_with_two_boxes();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "@page { size: 200px 300px; margin: 10px 20px 30px 40px; \
                 @top-left { content: 'a'; width: 20px } \
                 @top-center { content: 'b'; width: 30px } \
                 @top-right { content: 'c'; width: 40px } \
                 @bottom-left { content: 'a'; width: 20px } \
                 @bottom-center { content: 'b'; width: 30px } \
                 @bottom-right { content: 'c'; width: 40px } \
                 @left-top { content: 'a'; height: 40px } \
                 @left-middle { content: 'b'; height: 50px } \
                 @left-bottom { content: 'c'; height: 60px } \
                 @right-top { content: 'a'; height: 40px } \
                 @right-middle { content: 'b'; height: 50px } \
                 @right-bottom { content: 'c'; height: 60px } }",
            )
            .unwrap(),
        );
        let paged = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 200.0,
                height: 300.0,
                ..Rect::default()
            },
        )
        .unwrap();
        let page = &paged.pages[0];
        for (margin_box, rect) in [
            (
                PageMarginBox::TopLeft,
                Rect {
                    x: 40.0,
                    y: 0.0,
                    width: 20.0,
                    height: 10.0,
                },
            ),
            (
                PageMarginBox::TopCenter,
                Rect {
                    x: 95.0,
                    y: 0.0,
                    width: 30.0,
                    height: 10.0,
                },
            ),
            (
                PageMarginBox::TopRight,
                Rect {
                    x: 140.0,
                    y: 0.0,
                    width: 40.0,
                    height: 10.0,
                },
            ),
            (
                PageMarginBox::BottomLeft,
                Rect {
                    x: 40.0,
                    y: 270.0,
                    width: 20.0,
                    height: 30.0,
                },
            ),
            (
                PageMarginBox::BottomCenter,
                Rect {
                    x: 95.0,
                    y: 270.0,
                    width: 30.0,
                    height: 30.0,
                },
            ),
            (
                PageMarginBox::BottomRight,
                Rect {
                    x: 140.0,
                    y: 270.0,
                    width: 40.0,
                    height: 30.0,
                },
            ),
            (
                PageMarginBox::LeftTop,
                Rect {
                    x: 0.0,
                    y: 10.0,
                    width: 40.0,
                    height: 40.0,
                },
            ),
            (
                PageMarginBox::LeftMiddle,
                Rect {
                    x: 0.0,
                    y: 115.0,
                    width: 40.0,
                    height: 50.0,
                },
            ),
            (
                PageMarginBox::LeftBottom,
                Rect {
                    x: 0.0,
                    y: 210.0,
                    width: 40.0,
                    height: 60.0,
                },
            ),
            (
                PageMarginBox::RightTop,
                Rect {
                    x: 180.0,
                    y: 10.0,
                    width: 20.0,
                    height: 40.0,
                },
            ),
            (
                PageMarginBox::RightMiddle,
                Rect {
                    x: 180.0,
                    y: 115.0,
                    width: 20.0,
                    height: 50.0,
                },
            ),
            (
                PageMarginBox::RightBottom,
                Rect {
                    x: 180.0,
                    y: 210.0,
                    width: 20.0,
                    height: 60.0,
                },
            ),
        ] {
            assert_eq!(
                page.margin_box_rect(margin_box),
                Some(rect),
                "{margin_box:?}"
            );
        }
        assert_eq!(page.margin_box_rects.len(), 12);
    }

    #[test]
    fn one_two_and_three_generated_boxes_resolve_without_phantom_siblings() {
        let available = 120.0;
        let auto = EdgeBoxMeasure {
            generated: true,
            min_content: 5.0,
            max_content: 10.0,
            ..EdgeBoxMeasure::default()
        };
        let fixed = EdgeBoxMeasure {
            generated: true,
            specified: Some(20.0),
            ..EdgeBoxMeasure::default()
        };
        assert_eq!(
            edge_box_lengths([Some(auto), None, None], available),
            [120.0, 0.0, 0.0]
        );
        assert_eq!(
            edge_box_lengths([Some(auto), None, Some(auto)], available),
            [60.0, 0.0, 60.0]
        );
        assert_eq!(
            edge_box_lengths([Some(auto), Some(fixed), Some(auto)], available),
            [50.0, 20.0, 50.0]
        );
        assert_eq!(
            edge_box_lengths([None, Some(auto), None], available),
            [0.0, 120.0, 0.0]
        );
    }

    #[test]
    fn intrinsic_content_and_constraints_control_auto_edge_lengths() {
        let short = EdgeBoxMeasure {
            generated: true,
            min_content: 5.0,
            max_content: 10.0,
            ..EdgeBoxMeasure::default()
        };
        let long = EdgeBoxMeasure {
            generated: true,
            min_content: 15.0,
            max_content: 30.0,
            ..EdgeBoxMeasure::default()
        };
        assert_eq!(distribute_pair(short, long, 100.0), (25.0, 75.0));
        assert_eq!(distribute_pair(short, long, 30.0), (7.5, 22.5));
        assert_eq!(distribute_pair(short, long, 10.0), (2.5, 7.5));
        let limited = EdgeBoxMeasure {
            max_limit: Some(20.0),
            ..short
        };
        assert_eq!(
            edge_box_lengths([Some(limited), None, Some(long)], 100.0),
            [20.0, 0.0, 80.0]
        );
        let minimum = EdgeBoxMeasure {
            min_limit: Some(40.0),
            ..short
        };
        assert_eq!(
            edge_box_lengths([Some(minimum), None, Some(long)], 30.0),
            [40.0, 0.0, 0.0]
        );
        let fixed_side = EdgeBoxMeasure {
            specified: Some(40.0),
            ..short
        };
        let [_, centered, _] =
            edge_box_lengths([Some(fixed_side), Some(short), Some(short)], 100.0);
        assert!((centered - 100.0 / 9.0).abs() < 0.001);
    }

    #[test]
    fn narrow_edge_regions_keep_auto_rectangles_finite() {
        let (document, _, _) = document_with_two_boxes();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "@page { size: 20px 10px; margin: 15px; \
                 @top-left { content: 'long text' } \
                 @top-center { content: 'center' } \
                 @top-right { content: 'right' } \
                 @left-middle { content: 'side' } }",
            )
            .unwrap(),
        );
        let page = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 20.0,
                height: 10.0,
                ..Rect::default()
            },
        )
        .unwrap()
        .pages
        .remove(0);
        for margin_box in [
            PageMarginBox::TopLeft,
            PageMarginBox::TopCenter,
            PageMarginBox::TopRight,
            PageMarginBox::LeftMiddle,
        ] {
            let rect = page.margin_box_rect(margin_box).unwrap();
            assert!(
                rect.x.is_finite()
                    && rect.y.is_finite()
                    && rect.width.is_finite()
                    && rect.height.is_finite()
            );
            assert!(rect.width >= 0.0 && rect.height >= 0.0);
        }
    }

    #[test]
    fn fixed_edge_dimensions_honor_specified_size_and_auto_margins() {
        let (document, _, _) = document_with_two_boxes();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "@page { size: 200px 300px; margin: 10px 20px 30px 40px; \
                 @top-left { content: 't'; width: 20px; height: 5px } \
                 @top-center { content: 'c'; width: 20px; height: 4px; margin-top: auto; margin-bottom: auto } \
                 @top-right { content: 'm'; width: 20px; min-height: 15px } \
                 @bottom-left { content: 'b'; width: 20px; height: 5px } \
                 @left-top { content: 'l'; height: 20px; width: 10px } \
                 @right-top { content: 'r'; height: 20px; width: 10px } }",
            )
            .unwrap(),
        );
        let page = layout_paged_tree(
            &document,
            &mut resolver,
            Rect {
                width: 200.0,
                height: 300.0,
                ..Rect::default()
            },
        )
        .unwrap()
        .pages
        .remove(0);
        assert_eq!(page.margin_box_rect(PageMarginBox::TopLeft).unwrap().y, 5.0);
        assert_eq!(
            page.margin_box_rect(PageMarginBox::TopLeft).unwrap().height,
            5.0
        );
        assert_eq!(
            page.margin_box_rect(PageMarginBox::TopCenter).unwrap().y,
            3.0
        );
        assert_eq!(
            page.margin_box_rect(PageMarginBox::TopRight)
                .unwrap()
                .height,
            15.0
        );
        assert_eq!(
            page.margin_box_rect(PageMarginBox::BottomLeft).unwrap().y,
            270.0
        );
        assert_eq!(
            page.margin_box_rect(PageMarginBox::LeftTop).unwrap().x,
            30.0
        );
        assert_eq!(
            page.margin_box_rect(PageMarginBox::RightTop).unwrap().x,
            180.0
        );
    }
}
