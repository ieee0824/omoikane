//! Page contexts and source fragments for print layout.

use crate::css::{
    ComputedValue, MediaType, PageBoxGeometry, PageSelectorContext, PageSide, ResolvedPageStyle,
    StyleResolver,
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
            });
            if end >= section.end {
                break;
            }
            start = end;
        }
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
}
