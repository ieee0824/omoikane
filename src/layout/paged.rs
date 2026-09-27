//! Page contexts and source fragments for print layout.

use std::collections::BTreeMap;

use crate::css::style::counter_pairs;
use crate::css::{
    ComputedValue, MediaType, PageBoxGeometry, PageMarginBox, PageMarginContent,
    PageSelectorContext, PageSide, ResolvedPageStyle, StyleResolver, Value,
};
use crate::dom::{Node, NodeHandle, NodeType};

use super::{FontMetrics, LayoutBox, Rect, layout_tree, measure_text_width};

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
    /// Slice of the continuous source flow placed on this page.
    pub source: Rect,
    /// Page-scoped counter values after this page is generated.
    pub counters: PageCounterValues,
    /// Generated page-margin box rectangles in page-local coordinates.
    pub margin_box_rects: BTreeMap<PageMarginBox, Rect>,
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
        match self.style.margin_box_content(margin_box)? {
            PageMarginContent::Text(text) => Some(text),
            PageMarginContent::Expression(value) => {
                page_content_text(&value, page, self.counters.pages)
            }
        }
    }
}

fn page_content_text(value: &Value, page: i32, pages: usize) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::List(parts) => {
            let mut text = String::new();
            for part in parts {
                text.push_str(&page_content_text(part, page, pages)?);
            }
            Some(text)
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("counter") => {
            let [Value::Keyword(counter)] = arguments.as_slice() else {
                return None;
            };
            match counter.as_str() {
                "page" => Some(page.to_string()),
                "pages" => Some(pages.to_string()),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Print layout with page-specific contexts and a reusable document box tree.
#[derive(Debug, Clone)]
pub struct PagedLayout {
    /// Laid-out document flow.
    pub layout: LayoutBox,
    /// Output pages in document order.
    pub pages: Vec<PagedPage>,
}

#[derive(Debug)]
struct FlowSection {
    name: Option<String>,
    start: f32,
    end: f32,
}

/// Lays out a document for print and builds page contexts from `@page` rules.
///
/// The current flow is laid out at the first page's content width. Subsequent
/// page contexts can vary the sheet and margins; their source slices are
/// positioned in those page areas. Page-local line reflow is handled by the
/// fragmentation work tracked separately from this entry point.
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
    let first_side = document
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
    let first_name = first_page_name(document, resolver);
    let first_selector = page_selector(0, first_name, first_side);
    let first_style = resolver.resolved_page_style(&first_selector);
    let first_geometry = first_style.geometry((default_sheet.width, default_sheet.height), 0.0);
    let first_content = content_rect(first_geometry);
    let layout = layout_tree(document, resolver, first_content)?;
    let sections = flow_sections(&layout, resolver, first_content.y);

    let mut pages = Vec::new();
    for section in sections {
        let mut start = section.start;
        loop {
            if pages.len() >= 1024 {
                break;
            }
            let selector = page_selector(pages.len(), section.name.clone(), first_side);
            let style = resolver.resolved_page_style(&selector);
            let geometry = style.geometry((default_sheet.width, default_sheet.height), 0.0);
            let content = content_rect(geometry);
            let margin_box_rects = corner_margin_box_rects(&style, geometry);
            let capacity = content.height.max(1.0);
            let end = (start + capacity).min(section.end);
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
                source: Rect {
                    x: first_content.x,
                    y: start,
                    width: first_content.width,
                    height: (end - start).max(0.0),
                },
                counters: PageCounterValues { page: 0, pages: 0 },
                margin_box_rects,
            });
            if end >= section.end {
                break;
            }
            start = end;
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
    Some(PagedLayout { layout, pages })
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
            let text = page.margin_box_text(margin_box).unwrap_or_default();
            let metrics = FontMetrics::from_font_size(page.style.margin_box_font_size(margin_box));
            let (min_content, max_content) = if horizontal {
                let max_content = measure_text_width(&text, metrics);
                let min_content = super::inline::split_words_preserving_spaces_cjk(&text)
                    .iter()
                    .map(|word| measure_text_width(word, metrics))
                    .fold(0.0f32, f32::max);
                (min_content, max_content)
            } else {
                let lines = text.lines().count().max(1) as f32;
                let line_height = metrics.ascent + metrics.descent + metrics.line_gap;
                (line_height, lines * line_height)
            };
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
                specified: specified.map(|value| value.max(0.0)),
                min_content,
                max_content,
                min_limit: page
                    .style
                    .margin_box_length(margin_box, min_property, geometry, available)
                    .map(|value| value.max(0.0)),
                max_limit: page
                    .style
                    .margin_box_length(margin_box, max_property, geometry, available)
                    .map(|value| value.max(0.0)),
            })
        });
        let lengths = edge_box_lengths(measures, available);
        for (index, (margin_box, length)) in boxes.into_iter().zip(lengths).enumerate() {
            if measures[index].is_none() {
                continue;
            }
            let (fixed_offset, fixed_length) =
                fixed_edge_dimension(&page.style, geometry, edge, margin_box, thickness);
            let axis_start = match index {
                0 => start,
                1 => start + (available - length) / 2.0,
                _ => start + available - length,
            };
            let rect = if horizontal {
                Rect {
                    x: axis_start,
                    y: normal_start + fixed_offset,
                    width: length,
                    height: fixed_length,
                }
            } else {
                Rect {
                    x: normal_start + fixed_offset,
                    y: axis_start,
                    width: fixed_length,
                    height: length,
                }
            };
            rectangles.insert(margin_box, rect);
        }
    }
    rectangles
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
        .map(|length| length.max(0.0));
    let length = used.unwrap_or_else(|| {
        let before = before_value.unwrap_or(0.0);
        let after = after_value.unwrap_or(0.0);
        (thickness - before - after).max(0.0)
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
        .max(0.0);
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

fn flow_sections(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    content_start: f32,
) -> Vec<FlowSection> {
    let flow_end = layout
        .scrollable_overflow()
        .1
        .max(layout.dimensions.content.height)
        + layout.dimensions.content.y;
    let Some(body) = find_body_layout(layout) else {
        return vec![FlowSection {
            name: None,
            start: content_start,
            end: flow_end.max(content_start),
        }];
    };
    let mut sections = Vec::new();
    let mut current_name = body
        .children
        .iter()
        .find(|child| child.pseudo.is_none())
        .and_then(|child| page_name(&child.node, resolver));
    let mut start = content_start;
    for child in body.children.iter().filter(|child| child.pseudo.is_none()) {
        let name = page_name(&child.node, resolver);
        let child_style = resolver.computed_style(&child.node);
        let break_before = matches!(
            child_style.get("break-before"),
            Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("page")
        );
        if name != current_name || break_before {
            let boundary = (child.dimensions.border_box().y - child.dimensions.margin.top)
                .max(start)
                .min(flow_end);
            if boundary > start {
                sections.push(FlowSection {
                    name: current_name,
                    start,
                    end: boundary,
                });
            }
            current_name = name;
            start = boundary;
        }
        let break_after = matches!(
            child_style.get("break-after"),
            Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("page")
        );
        if break_after {
            let border = child.dimensions.border_box();
            let boundary = (border.y + border.height + child.dimensions.margin.bottom)
                .max(start)
                .min(flow_end);
            if boundary > start {
                sections.push(FlowSection {
                    name: current_name.clone(),
                    start,
                    end: boundary,
                });
            }
            start = boundary;
        }
    }
    if flow_end > start || sections.is_empty() {
        sections.push(FlowSection {
            name: current_name,
            start,
            end: flow_end.max(start),
        });
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::{Origin, parse_stylesheet};

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
