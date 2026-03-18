//! Block layout primitives.
//!
//! The layout phase consumes DOM nodes together with computed styles and
//! produces a tree of rectangular block boxes.

use crate::css::{ComputedStyle, ComputedValue, StyleResolver};
use crate::dom::{Node, NodeHandle, NodeType};

/// A rectangle in layout space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Edge sizes for the CSS box model.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EdgeSizes {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl EdgeSizes {
    fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

/// CSS box dimensions for a single layout box.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoxDimensions {
    pub content: Rect,
    pub padding: EdgeSizes,
    pub border: EdgeSizes,
    pub margin: EdgeSizes,
}

impl BoxDimensions {
    /// Returns the total width including padding, border, and margin.
    pub fn total_width(&self) -> f32 {
        self.content.width
            + self.padding.horizontal()
            + self.border.horizontal()
            + self.margin.horizontal()
    }

    /// Returns the total height including padding, border, and margin.
    pub fn total_height(&self) -> f32 {
        self.content.height
            + self.padding.vertical()
            + self.border.vertical()
            + self.margin.vertical()
    }
}

/// Visibility state for a laid out box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Visible,
    Hidden,
}

/// Overflow behavior tracked by the layout tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overflow {
    #[default]
    Visible,
    Hidden,
}

/// A block layout box derived from a DOM node.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutBox {
    pub node: NodeHandle,
    pub dimensions: BoxDimensions,
    pub visibility: Visibility,
    pub overflow: Overflow,
    pub children: Vec<LayoutBox>,
}

impl LayoutBox {
    /// Returns the box width including padding, border, and margins.
    pub fn total_width(&self) -> f32 {
        self.dimensions.total_width()
    }

    /// Returns the box height including padding, border, and margins.
    pub fn total_height(&self) -> f32 {
        self.dimensions.total_height()
    }
}

/// Lays out a DOM subtree as block boxes inside `containing_block`.
///
/// Nodes with `display: none` are omitted from the result. Non-element nodes do
/// not currently produce layout boxes.
pub fn layout_tree(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
) -> Option<LayoutBox> {
    layout_node(node, resolver, containing_block)
}

fn layout_node(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
) -> Option<LayoutBox> {
    match node.node_type() {
        NodeType::Document => layout_document(node, resolver, containing_block),
        NodeType::Element => layout_element(node, resolver, containing_block),
        _ => None,
    }
}

fn layout_document(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
) -> Option<LayoutBox> {
    let mut children = Vec::new();
    let mut cursor_y = containing_block.y;
    let mut previous_margin_bottom: Option<f32> = None;

    for child in node.child_nodes() {
        let child_style = match child.node_type() {
            NodeType::Element => Some(resolver.computed_style(&child)),
            _ => None,
        };
        let child_margin_top = child_style
            .as_ref()
            .map(|style| edge_sizes(style, "margin").top)
            .unwrap_or(0.0);
        let collapse_delta = previous_margin_bottom
            .map(|margin_bottom| {
                margin_bottom + child_margin_top - collapse_margins(margin_bottom, child_margin_top)
            })
            .unwrap_or(0.0);
        let child_containing = Rect {
            x: containing_block.x,
            y: cursor_y - collapse_delta,
            width: containing_block.width,
            height: 0.0,
        };

        if let Some(layout_child) = layout_node(&child, resolver, child_containing) {
            cursor_y += layout_child.total_height();
            previous_margin_bottom = Some(layout_child.dimensions.margin.bottom);
            children.push(layout_child);
        }
    }

    let mut dimensions = BoxDimensions::default();
    dimensions.content = Rect {
        x: containing_block.x,
        y: containing_block.y,
        width: containing_block.width,
        height: cursor_y - containing_block.y,
    };

    Some(LayoutBox {
        node: node.clone(),
        dimensions,
        visibility: Visibility::Visible,
        overflow: Overflow::Visible,
        children,
    })
}

fn layout_element(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
) -> Option<LayoutBox> {
    let style = resolver.computed_style(node);
    if is_display_none(&style) {
        return None;
    }

    let padding = edge_sizes(&style, "padding");
    let border = edge_sizes(&style, "border");
    let mut margin = edge_sizes(&style, "margin");

    let width = compute_width(&style, containing_block.width, padding, border, &mut margin);
    let x = containing_block.x + margin.left + border.left + padding.left;
    let y = containing_block.y + margin.top + border.top + padding.top;

    let mut children = Vec::new();
    let mut cursor_y = y;
    let mut previous_margin_bottom: Option<f32> = None;

    for child in node.child_nodes() {
        let child_style = match child.node_type() {
            NodeType::Element => Some(resolver.computed_style(&child)),
            _ => None,
        };
        let child_margin_top = child_style
            .as_ref()
            .map(|style| edge_sizes(style, "margin").top)
            .unwrap_or(0.0);
        let collapse_delta = previous_margin_bottom
            .map(|margin_bottom| {
                margin_bottom + child_margin_top - collapse_margins(margin_bottom, child_margin_top)
            })
            .unwrap_or(0.0);
        let child_containing = Rect {
            x,
            y: cursor_y - collapse_delta,
            width,
            height: 0.0,
        };

        if let Some(layout_child) = layout_node(&child, resolver, child_containing) {
            cursor_y += layout_child.total_height();
            previous_margin_bottom = Some(layout_child.dimensions.margin.bottom);
            children.push(layout_child);
        }
    }

    let content_height = explicit_length(&style, "height").unwrap_or(cursor_y - y);
    let dimensions = BoxDimensions {
        content: Rect {
            x,
            y,
            width,
            height: content_height,
        },
        padding,
        border,
        margin,
    };

    Some(LayoutBox {
        node: node.clone(),
        dimensions,
        visibility: visibility(&style),
        overflow: overflow(&style),
        children,
    })
}

fn compute_width(
    style: &ComputedStyle,
    containing_width: f32,
    padding: EdgeSizes,
    border: EdgeSizes,
    margin: &mut EdgeSizes,
) -> f32 {
    let specified_width = explicit_length(style, "width");
    let margin_left_auto = is_auto(style.get("margin-left"));
    let margin_right_auto = is_auto(style.get("margin-right"));

    if let Some(width) = specified_width {
        let remaining =
            (containing_width - width - padding.horizontal() - border.horizontal()).max(0.0);

        match (margin_left_auto, margin_right_auto) {
            (true, true) => {
                margin.left = remaining / 2.0;
                margin.right = remaining / 2.0;
            }
            (true, false) => {
                margin.left = (remaining - margin.right).max(0.0);
            }
            (false, true) => {
                margin.right = (remaining - margin.left).max(0.0);
            }
            (false, false) => {}
        }

        width
    } else {
        if margin_left_auto {
            margin.left = 0.0;
        }
        if margin_right_auto {
            margin.right = 0.0;
        }

        (containing_width - padding.horizontal() - border.horizontal() - margin.horizontal())
            .max(0.0)
    }
}

fn edge_sizes(style: &ComputedStyle, prefix: &str) -> EdgeSizes {
    EdgeSizes {
        top: explicit_length(style, &format!("{prefix}-top")).unwrap_or(0.0),
        right: explicit_length(style, &format!("{prefix}-right")).unwrap_or(0.0),
        bottom: explicit_length(style, &format!("{prefix}-bottom")).unwrap_or(0.0),
        left: explicit_length(style, &format!("{prefix}-left")).unwrap_or(0.0),
    }
}

fn explicit_length(style: &ComputedStyle, property: &str) -> Option<f32> {
    match style.get(property) {
        Some(ComputedValue::Px(value)) => Some(*value),
        Some(ComputedValue::Number(value)) => Some(*value),
        _ => None,
    }
}

fn is_auto(value: Option<&ComputedValue>) -> bool {
    matches!(value, Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("auto"))
}

fn collapse_margins(first: f32, second: f32) -> f32 {
    if first >= 0.0 && second >= 0.0 {
        first.max(second)
    } else if first <= 0.0 && second <= 0.0 {
        first.min(second)
    } else {
        first + second
    }
}

fn is_display_none(style: &ComputedStyle) -> bool {
    matches!(
        style.get("display"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("none")
    )
}

fn visibility(style: &ComputedStyle) -> Visibility {
    match style.get("visibility") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("hidden") => {
            Visibility::Hidden
        }
        _ => Visibility::Visible,
    }
}

fn overflow(style: &ComputedStyle) -> Overflow {
    match style.get("overflow") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("hidden") => {
            Overflow::Hidden
        }
        _ => Overflow::Visible,
    }
}

#[cfg(test)]
mod tests {
    use crate::css::{Origin, parse_stylesheet};

    use super::*;

    fn sample_tree() -> (NodeHandle, NodeHandle, NodeHandle, NodeHandle) {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let card = NodeHandle::element("div");

        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(card.clone());

        (document, html, body, card)
    }

    #[test]
    fn computes_block_box_dimensions_from_style() {
        let (_document, _html, body, _card) = sample_tree();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body { width: 300px; } \
                 div { width: 120px; height: 40px; margin-top: 10px; margin-bottom: 14px; padding-left: 8px; padding-right: 12px; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 300.0,
                height: 0.0,
            },
        )
        .unwrap();

        let child = &layout.children[0];
        assert_eq!(child.dimensions.content.width, 120.0);
        assert_eq!(child.dimensions.content.height, 40.0);
        assert_eq!(child.dimensions.padding.left, 8.0);
        assert_eq!(child.dimensions.padding.right, 12.0);
        assert_eq!(child.dimensions.margin.top, 10.0);
        assert_eq!(child.dimensions.margin.bottom, 14.0);
        assert_eq!(child.total_height(), 64.0);
    }

    #[test]
    fn auto_width_fills_remaining_space() {
        let (_document, _html, body, _card) = sample_tree();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { margin-left: 10px; margin-right: 20px; padding-left: 5px; padding-right: 5px; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 0.0,
            },
        )
        .unwrap();

        let child = &layout.children[0];
        assert_eq!(child.dimensions.content.width, 160.0);
        assert_eq!(child.total_width(), 200.0);
    }

    #[test]
    fn auto_margins_center_fixed_width_blocks() {
        let (_document, _html, body, _card) = sample_tree();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("div { width: 80px; margin-left: auto; margin-right: auto; }")
                .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 0.0,
            },
        )
        .unwrap();

        let child = &layout.children[0];
        assert_eq!(child.dimensions.margin.left, 60.0);
        assert_eq!(child.dimensions.margin.right, 60.0);
    }

    #[test]
    fn omits_display_none_nodes() {
        let (_document, _html, body, _card) = sample_tree();
        let hidden = NodeHandle::element("aside");
        body.append_child(hidden);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("aside { display: none; }").unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 0.0,
            },
        )
        .unwrap();

        assert_eq!(layout.children.len(), 1);
        assert_eq!(layout.children[0].node.tag_name().as_deref(), Some("div"));
    }

    #[test]
    fn keeps_visibility_hidden_boxes_in_layout() {
        let (_document, _html, body, _card) = sample_tree();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { visibility: hidden; overflow: hidden; width: 50px; height: 20px; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 0.0,
            },
        )
        .unwrap();

        let child = &layout.children[0];
        assert_eq!(child.visibility, Visibility::Hidden);
        assert_eq!(child.overflow, Overflow::Hidden);
        assert_eq!(child.dimensions.content.width, 50.0);
        assert_eq!(child.dimensions.content.height, 20.0);
    }

    #[test]
    fn collapses_vertical_margins_between_siblings() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let first = NodeHandle::element("div");
        let second = NodeHandle::element("section");

        document.append_child(body.clone());
        body.append_child(first.clone());
        body.append_child(second.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { height: 30px; margin-bottom: 20px; } \
                 section { height: 10px; margin-top: 12px; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 0.0,
            },
        )
        .unwrap();

        let first_box = &layout.children[0];
        let second_box = &layout.children[1];

        let first_border_bottom =
            first_box.dimensions.content.y + first_box.dimensions.content.height;
        let second_border_top = second_box.dimensions.content.y;

        assert_eq!(first_border_bottom, 30.0);
        assert_eq!(second_border_top, 50.0);
        assert_eq!(second_border_top - first_border_bottom, 20.0);
    }
}
