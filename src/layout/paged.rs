//! Page contexts and source fragments for print layout.

use crate::css::style::counter_pairs;
use crate::css::{
    ComputedValue, MediaType, PageBoxGeometry, PageMarginBox, PageMarginContent,
    PageSelectorContext, PageSide, ResolvedPageStyle, StyleResolver, Value,
};
use crate::dom::{Node, NodeHandle, NodeType};

use super::{LayoutBox, Rect, layout_tree};

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
}
