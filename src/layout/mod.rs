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

/// A laid out fragment of inline text.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineFragment {
    pub node: NodeHandle,
    pub text: String,
    pub rect: Rect,
    pub metrics: FontMetrics,
    pub vertical_align: VerticalAlign,
}

/// A single line box inside a block formatting context.
#[derive(Debug, Clone, PartialEq)]
pub struct LineBox {
    pub rect: Rect,
    pub baseline: f32,
    pub fragments: Vec<InlineFragment>,
}

/// Approximate font metrics used by inline layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    pub font_size: f32,
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub average_advance: f32,
}

impl FontMetrics {
    /// Creates approximate metrics from a CSS font size.
    pub fn from_font_size(font_size: f32) -> Self {
        Self {
            font_size,
            ascent: font_size * 0.8,
            descent: font_size * 0.2,
            line_gap: font_size * 0.2,
            average_advance: font_size * 0.6,
        }
    }
}

/// Minimal `vertical-align` values supported by the inline layout engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VerticalAlign {
    Baseline,
    Top,
    Middle,
    Bottom,
    Length(f32),
}

/// Supported flex directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexDirection {
    Row,
    Column,
}

/// Supported flex wrapping modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexWrap {
    NoWrap,
    Wrap,
}

/// Minimal justify-content values supported by the flex layout engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JustifyContent {
    FlexStart,
    Center,
    FlexEnd,
    SpaceBetween,
    SpaceAround,
}

/// Minimal align-items / align-self values supported by the flex layout engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignItems {
    Stretch,
    FlexStart,
    Center,
    FlexEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PositionScheme {
    Static,
    Absolute,
    Fixed,
}

/// A block layout box derived from a DOM node.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutBox {
    pub node: NodeHandle,
    pub dimensions: BoxDimensions,
    pub visibility: Visibility,
    pub overflow: Overflow,
    pub z_index: i32,
    pub lines: Vec<LineBox>,
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
    layout_node(node, resolver, containing_block, containing_block)
}

fn layout_node(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    viewport: Rect,
) -> Option<LayoutBox> {
    match node.node_type() {
        NodeType::Document => layout_document(node, resolver, containing_block, viewport),
        NodeType::Element => layout_element(node, resolver, containing_block, viewport),
        _ => None,
    }
}

fn layout_document(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    viewport: Rect,
) -> Option<LayoutBox> {
    let mut children = Vec::new();
    let mut positioned_children = Vec::new();
    let mut cursor_y = containing_block.y;
    let mut previous_margin_bottom: Option<f32> = None;

    for child in node.child_nodes() {
        let child_style = match child.node_type() {
            NodeType::Element => Some(resolver.computed_style(&child)),
            _ => None,
        };
        if let Some(style) = &child_style {
            if is_out_of_flow_positioned(style) {
                positioned_children.push((child, style.clone()));
                continue;
            }
        }
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

        if let Some(layout_child) = layout_node(&child, resolver, child_containing, viewport) {
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

    let document_box = BoxDimensions {
        content: dimensions.content,
        ..BoxDimensions::default()
    };
    for (child, style) in positioned_children {
        if let Some(positioned) =
            layout_positioned_child(&child, resolver, &style, document_box, viewport, viewport)
        {
            children.push(positioned);
        }
    }
    sort_children_by_z_index(&mut children);

    Some(LayoutBox {
        node: node.clone(),
        dimensions,
        visibility: Visibility::Visible,
        overflow: Overflow::Visible,
        z_index: 0,
        lines: Vec::new(),
        children,
    })
}

fn layout_element(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    viewport: Rect,
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

    if is_flex_container(&style) {
        return layout_flex_container(
            node, resolver, style, margin, padding, border, x, y, width, viewport,
        );
    }

    let mut children = Vec::new();
    let mut positioned_children = Vec::new();
    let mut lines = Vec::new();
    let mut cursor_y = y;
    let mut previous_margin_bottom: Option<f32> = None;
    let mut pending_inline_nodes = Vec::new();

    for child in node.child_nodes() {
        if is_inline_child(&child, resolver) {
            pending_inline_nodes.push(child);
            continue;
        }

        if !pending_inline_nodes.is_empty() {
            let inline_lines =
                layout_inline_nodes(&pending_inline_nodes, resolver, x, cursor_y, width);
            if let Some(last_line) = inline_lines.last() {
                cursor_y = last_line.rect.y + last_line.rect.height;
            }
            lines.extend(inline_lines);
            pending_inline_nodes.clear();
        }

        let child_style = match child.node_type() {
            NodeType::Element => Some(resolver.computed_style(&child)),
            _ => None,
        };
        if let Some(style) = &child_style {
            if is_out_of_flow_positioned(style) {
                positioned_children.push((child, style.clone()));
                continue;
            }
        }
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

        if let Some(layout_child) = layout_node(&child, resolver, child_containing, viewport) {
            cursor_y += layout_child.total_height();
            previous_margin_bottom = Some(layout_child.dimensions.margin.bottom);
            children.push(layout_child);
        }
    }

    if !pending_inline_nodes.is_empty() {
        let inline_lines = layout_inline_nodes(&pending_inline_nodes, resolver, x, cursor_y, width);
        if let Some(last_line) = inline_lines.last() {
            cursor_y = last_line.rect.y + last_line.rect.height;
        }
        lines.extend(inline_lines);
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
    for (child, style) in positioned_children {
        if let Some(positioned) = layout_positioned_child(
            &child,
            resolver,
            &style,
            dimensions,
            containing_block,
            viewport,
        ) {
            children.push(positioned);
        }
    }
    sort_children_by_z_index(&mut children);

    Some(LayoutBox {
        node: node.clone(),
        dimensions,
        visibility: visibility(&style),
        overflow: overflow(&style),
        z_index: z_index(&style),
        lines,
        children,
    })
}

fn layout_flex_container(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: ComputedStyle,
    margin: EdgeSizes,
    padding: EdgeSizes,
    border: EdgeSizes,
    x: f32,
    y: f32,
    width: f32,
    viewport: Rect,
) -> Option<LayoutBox> {
    let direction = flex_direction(&style);
    let wrap = flex_wrap(&style);
    let justify = justify_content(&style);
    let align = align_items(&style);

    let mut items = Vec::new();
    let mut positioned_children = Vec::new();
    for child in node.child_nodes() {
        if child.node_type() != NodeType::Element {
            continue;
        }
        let child_style = resolver.computed_style(&child);
        if is_display_none(&child_style) {
            continue;
        }
        if is_out_of_flow_positioned(&child_style) {
            positioned_children.push((child, child_style));
            continue;
        }
        items.push(FlexItemSpec {
            node: child,
            base_main_size: flex_basis(&child_style, direction)
                .or_else(|| explicit_main_size(&child_style, direction))
                .unwrap_or(0.0),
            explicit_cross_size: explicit_cross_size(&child_style, direction),
            flex_grow: flex_grow(&child_style),
            flex_shrink: flex_shrink(&child_style),
            align_self: align_self(&child_style),
        });
    }

    let lines = build_flex_lines(&items, width, wrap);
    let mut children = Vec::new();
    let mut cross_cursor = y;

    for line in lines {
        let resolved_main_sizes = resolve_flex_main_sizes(&line.items, width);
        let mut laid_out = Vec::new();
        let mut line_cross_size = 0.0f32;

        for (item, main_size) in line.items.iter().zip(resolved_main_sizes.iter()) {
            let child_containing = match direction {
                FlexDirection::Row => Rect {
                    x: 0.0,
                    y: 0.0,
                    width: *main_size,
                    height: item.explicit_cross_size.unwrap_or(0.0),
                },
                FlexDirection::Column => Rect {
                    x: 0.0,
                    y: 0.0,
                    width: item.explicit_cross_size.unwrap_or(width),
                    height: *main_size,
                },
            };

            if let Some(layout_child) =
                layout_node(&item.node, resolver, child_containing, viewport)
            {
                let cross_size = match direction {
                    FlexDirection::Row => layout_child.total_height(),
                    FlexDirection::Column => layout_child.total_width(),
                };
                line_cross_size = line_cross_size.max(cross_size);
                laid_out.push((item, layout_child));
            }
        }

        let total_main_size: f32 = laid_out
            .iter()
            .map(|(_, child)| match direction {
                FlexDirection::Row => child.total_width(),
                FlexDirection::Column => child.total_height(),
            })
            .sum();
        let (line_start, gap) = justify_offsets(justify, width, total_main_size, laid_out.len());

        let mut main_cursor = match direction {
            FlexDirection::Row => x + line_start,
            FlexDirection::Column => y + line_start,
        };

        for (item, mut child) in laid_out {
            let child_main_size = match direction {
                FlexDirection::Row => child.total_width(),
                FlexDirection::Column => child.total_height(),
            };
            let child_cross_size = match direction {
                FlexDirection::Row => child.total_height(),
                FlexDirection::Column => child.total_width(),
            };
            let align_value = item.align_self.unwrap_or(align);
            let cross_offset = align_offset(align_value, line_cross_size, child_cross_size);

            let (outer_x, outer_y) = match direction {
                FlexDirection::Row => (main_cursor, cross_cursor + cross_offset),
                FlexDirection::Column => (x + cross_offset, main_cursor),
            };
            translate_layout_box_to_outer(&mut child, outer_x, outer_y);
            children.push(child);

            main_cursor += child_main_size + gap;
        }

        cross_cursor += line_cross_size;
    }

    let content_height = explicit_length(&style, "height").unwrap_or(cross_cursor - y);
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
    for (child, style) in positioned_children {
        if let Some(positioned) = layout_positioned_child(
            &child,
            resolver,
            &style,
            dimensions,
            dimensions.content,
            viewport,
        ) {
            children.push(positioned);
        }
    }
    sort_children_by_z_index(&mut children);

    Some(LayoutBox {
        node: node.clone(),
        dimensions,
        visibility: visibility(&style),
        overflow: overflow(&style),
        z_index: z_index(&style),
        lines: Vec::new(),
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
    let shorthand_property = match prefix {
        "border" => "border-width",
        _ => prefix,
    };
    let shorthand = explicit_length(style, shorthand_property).unwrap_or(0.0);
    EdgeSizes {
        top: explicit_length(style, &format!("{prefix}-top")).unwrap_or(shorthand),
        right: explicit_length(style, &format!("{prefix}-right")).unwrap_or(shorthand),
        bottom: explicit_length(style, &format!("{prefix}-bottom")).unwrap_or(shorthand),
        left: explicit_length(style, &format!("{prefix}-left")).unwrap_or(shorthand),
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

fn is_out_of_flow_positioned(style: &ComputedStyle) -> bool {
    matches!(
        position_scheme(style),
        PositionScheme::Absolute | PositionScheme::Fixed
    )
}

fn position_scheme(style: &ComputedStyle) -> PositionScheme {
    match style.get("position") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("absolute") => {
            PositionScheme::Absolute
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("fixed") => {
            PositionScheme::Fixed
        }
        _ => PositionScheme::Static,
    }
}

fn z_index(style: &ComputedStyle) -> i32 {
    match style.get("z-index") {
        Some(ComputedValue::Number(value)) => *value as i32,
        Some(ComputedValue::Px(value)) => *value as i32,
        _ => 0,
    }
}

fn layout_positioned_child(
    child: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    parent_box: BoxDimensions,
    containing_block: Rect,
    viewport: Rect,
) -> Option<LayoutBox> {
    let position = position_scheme(style);
    let origin = match position {
        PositionScheme::Fixed => viewport,
        PositionScheme::Absolute => parent_box.content,
        PositionScheme::Static => containing_block,
    };

    let child_containing = Rect {
        x: origin.x,
        y: origin.y,
        width: origin.width,
        height: origin.height,
    };
    let mut layout_child = layout_node(child, resolver, child_containing, viewport)?;
    let outer_width = layout_child.total_width();
    let outer_height = layout_child.total_height();
    let left = explicit_length(style, "left");
    let right = explicit_length(style, "right");
    let top = explicit_length(style, "top");
    let bottom = explicit_length(style, "bottom");
    let outer_x = if let Some(left) = left {
        origin.x + left
    } else if let Some(right) = right {
        origin.x + origin.width - outer_width - right
    } else {
        origin.x
    };
    let outer_y = if let Some(top) = top {
        origin.y + top
    } else if let Some(bottom) = bottom {
        origin.y + origin.height - outer_height - bottom
    } else {
        origin.y
    };
    translate_layout_box_to_outer(&mut layout_child, outer_x, outer_y);
    layout_child.z_index = z_index(style);
    Some(layout_child)
}

fn sort_children_by_z_index(children: &mut [LayoutBox]) {
    children.sort_by_key(|child| child.z_index);
}

#[derive(Debug, Clone)]
struct FlexItemSpec {
    node: NodeHandle,
    base_main_size: f32,
    explicit_cross_size: Option<f32>,
    flex_grow: f32,
    flex_shrink: f32,
    align_self: Option<AlignItems>,
}

#[derive(Debug, Clone)]
struct FlexLine<'a> {
    items: Vec<&'a FlexItemSpec>,
}

fn is_flex_container(style: &ComputedStyle) -> bool {
    matches!(
        style.get("display"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("flex")
    )
}

fn flex_direction(style: &ComputedStyle) -> FlexDirection {
    match style.get("flex-direction") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("column") => {
            FlexDirection::Column
        }
        _ => FlexDirection::Row,
    }
}

fn flex_wrap(style: &ComputedStyle) -> FlexWrap {
    match style.get("flex-wrap") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("wrap") => {
            FlexWrap::Wrap
        }
        _ => FlexWrap::NoWrap,
    }
}

fn justify_content(style: &ComputedStyle) -> JustifyContent {
    match style.get("justify-content") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("center") => {
            JustifyContent::Center
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("flex-end") => {
            JustifyContent::FlexEnd
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("space-between") => {
            JustifyContent::SpaceBetween
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("space-around") => {
            JustifyContent::SpaceAround
        }
        _ => JustifyContent::FlexStart,
    }
}

fn align_items(style: &ComputedStyle) -> AlignItems {
    match style.get("align-items") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("flex-start") => {
            AlignItems::FlexStart
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("center") => {
            AlignItems::Center
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("flex-end") => {
            AlignItems::FlexEnd
        }
        _ => AlignItems::Stretch,
    }
}

fn align_self(style: &ComputedStyle) -> Option<AlignItems> {
    match style.get("align-self") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("flex-start") => {
            Some(AlignItems::FlexStart)
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("center") => {
            Some(AlignItems::Center)
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("flex-end") => {
            Some(AlignItems::FlexEnd)
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("stretch") => {
            Some(AlignItems::Stretch)
        }
        _ => None,
    }
}

fn explicit_main_size(style: &ComputedStyle, direction: FlexDirection) -> Option<f32> {
    match direction {
        FlexDirection::Row => explicit_length(style, "width"),
        FlexDirection::Column => explicit_length(style, "height"),
    }
}

fn explicit_cross_size(style: &ComputedStyle, direction: FlexDirection) -> Option<f32> {
    match direction {
        FlexDirection::Row => explicit_length(style, "height"),
        FlexDirection::Column => explicit_length(style, "width"),
    }
}

fn flex_basis(style: &ComputedStyle, direction: FlexDirection) -> Option<f32> {
    explicit_length(style, "flex-basis").or_else(|| explicit_main_size(style, direction))
}

fn flex_grow(style: &ComputedStyle) -> f32 {
    match style.get("flex-grow") {
        Some(ComputedValue::Number(value)) => *value,
        Some(ComputedValue::Px(value)) => *value,
        _ => 0.0,
    }
}

fn flex_shrink(style: &ComputedStyle) -> f32 {
    match style.get("flex-shrink") {
        Some(ComputedValue::Number(value)) => *value,
        Some(ComputedValue::Px(value)) => *value,
        _ => 1.0,
    }
}

fn build_flex_lines<'a>(
    items: &'a [FlexItemSpec],
    available_main_size: f32,
    wrap: FlexWrap,
) -> Vec<FlexLine<'a>> {
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut occupied = 0.0f32;

    for item in items {
        let item_size = item.base_main_size;
        let would_wrap = wrap == FlexWrap::Wrap
            && !current.is_empty()
            && occupied + item_size > available_main_size;
        if would_wrap {
            lines.push(FlexLine { items: current });
            current = Vec::new();
            occupied = 0.0;
        }
        occupied += item_size;
        current.push(item);
    }

    if !current.is_empty() {
        lines.push(FlexLine { items: current });
    }

    lines
}

fn resolve_flex_main_sizes(items: &[&FlexItemSpec], available_main_size: f32) -> Vec<f32> {
    let total_base: f32 = items.iter().map(|item| item.base_main_size).sum();
    let total_grow: f32 = items.iter().map(|item| item.flex_grow).sum();
    let total_shrink_factor: f32 = items
        .iter()
        .map(|item| item.flex_shrink * item.base_main_size)
        .sum();

    items
        .iter()
        .map(|item| {
            if total_base < available_main_size && total_grow > 0.0 {
                let extra = available_main_size - total_base;
                item.base_main_size + extra * (item.flex_grow / total_grow)
            } else if total_base > available_main_size && total_shrink_factor > 0.0 {
                let overflow = total_base - available_main_size;
                let shrink =
                    overflow * ((item.flex_shrink * item.base_main_size) / total_shrink_factor);
                (item.base_main_size - shrink).max(0.0)
            } else {
                item.base_main_size
            }
        })
        .collect()
}

fn justify_offsets(
    justify: JustifyContent,
    available_main_size: f32,
    used_main_size: f32,
    item_count: usize,
) -> (f32, f32) {
    let free_space = (available_main_size - used_main_size).max(0.0);
    match justify {
        JustifyContent::FlexStart => (0.0, 0.0),
        JustifyContent::Center => (free_space / 2.0, 0.0),
        JustifyContent::FlexEnd => (free_space, 0.0),
        JustifyContent::SpaceBetween if item_count > 1 => {
            (0.0, free_space / (item_count - 1) as f32)
        }
        JustifyContent::SpaceAround if item_count > 0 => {
            let gap = free_space / item_count as f32;
            (gap / 2.0, gap)
        }
        _ => (0.0, 0.0),
    }
}

fn align_offset(align: AlignItems, line_cross_size: f32, child_cross_size: f32) -> f32 {
    match align {
        AlignItems::FlexStart | AlignItems::Stretch => 0.0,
        AlignItems::Center => (line_cross_size - child_cross_size).max(0.0) / 2.0,
        AlignItems::FlexEnd => (line_cross_size - child_cross_size).max(0.0),
    }
}

fn translate_layout_box_to_outer(layout: &mut LayoutBox, outer_x: f32, outer_y: f32) {
    let current_outer_x = layout.dimensions.content.x
        - layout.dimensions.padding.left
        - layout.dimensions.border.left
        - layout.dimensions.margin.left;
    let current_outer_y = layout.dimensions.content.y
        - layout.dimensions.padding.top
        - layout.dimensions.border.top
        - layout.dimensions.margin.top;
    translate_layout_box(layout, outer_x - current_outer_x, outer_y - current_outer_y);
}

fn translate_layout_box(layout: &mut LayoutBox, dx: f32, dy: f32) {
    layout.dimensions.content.x += dx;
    layout.dimensions.content.y += dy;
    for line in &mut layout.lines {
        line.rect.x += dx;
        line.rect.y += dy;
        line.baseline += dy;
        for fragment in &mut line.fragments {
            fragment.rect.x += dx;
            fragment.rect.y += dy;
        }
    }
    for child in &mut layout.children {
        translate_layout_box(child, dx, dy);
    }
}

fn is_inline_child(node: &NodeHandle, resolver: &mut StyleResolver) -> bool {
    match node.node_type() {
        NodeType::Text => true,
        NodeType::Element => {
            let style = resolver.computed_style(node);
            matches!(
                style.get("display"),
                Some(ComputedValue::Keyword(keyword))
                    if keyword.eq_ignore_ascii_case("inline")
                        || keyword.eq_ignore_ascii_case("inline-block")
            ) || node
                .tag_name()
                .map(|tag| matches!(tag.as_str(), "span" | "a" | "em" | "strong" | "b" | "i"))
                .unwrap_or(false)
        }
        _ => false,
    }
}

fn layout_inline_nodes(
    nodes: &[NodeHandle],
    resolver: &mut StyleResolver,
    start_x: f32,
    start_y: f32,
    available_width: f32,
) -> Vec<LineBox> {
    let mut segments = Vec::new();
    for node in nodes {
        collect_inline_segments(node, resolver, &mut segments);
    }

    layout_inline_segments(&segments, start_x, start_y, available_width)
}

#[derive(Debug, Clone)]
struct InlineSegment {
    node: NodeHandle,
    text: String,
    metrics: FontMetrics,
    line_height: f32,
    vertical_align: VerticalAlign,
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
                let text = normalize_text(&text, white_space(&parent_style));
                if !text.is_empty() {
                    out.push(InlineSegment {
                        node: node.clone(),
                        text,
                        metrics: font_metrics(&parent_style),
                        line_height: line_height(&parent_style),
                        vertical_align: vertical_align(&parent_style),
                    });
                }
            }
        }
        NodeType::Element => {
            let style = resolver.computed_style(node);
            if is_display_none(&style) {
                return;
            }

            for child in node.child_nodes() {
                match child.node_type() {
                    NodeType::Text => {
                        if let Some(text) = child.data() {
                            let text = normalize_text(&text, white_space(&style));
                            if !text.is_empty() {
                                out.push(InlineSegment {
                                    node: child,
                                    text,
                                    metrics: font_metrics(&style),
                                    line_height: line_height(&style),
                                    vertical_align: vertical_align(&style),
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
        }
        _ => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WhiteSpaceMode {
    Normal,
    Pre,
}

fn white_space(style: &ComputedStyle) -> WhiteSpaceMode {
    match style.get("white-space") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("pre") => {
            WhiteSpaceMode::Pre
        }
        _ => WhiteSpaceMode::Normal,
    }
}

fn normalize_text(text: &str, mode: WhiteSpaceMode) -> String {
    match mode {
        WhiteSpaceMode::Normal => {
            let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
            collapsed
        }
        WhiteSpaceMode::Pre => text.to_string(),
    }
}

fn font_size(style: &ComputedStyle) -> f32 {
    explicit_length(style, "font-size").unwrap_or(16.0)
}

fn font_metrics(style: &ComputedStyle) -> FontMetrics {
    FontMetrics::from_font_size(font_size(style))
}

fn line_height(style: &ComputedStyle) -> f32 {
    match style.get("line-height") {
        Some(ComputedValue::Px(value)) => *value,
        Some(ComputedValue::Number(value)) => *value * font_size(style),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("normal") => {
            font_size(style) * 1.2
        }
        _ => font_size(style) * 1.2,
    }
}

fn vertical_align(style: &ComputedStyle) -> VerticalAlign {
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
        Some(ComputedValue::Px(value)) => VerticalAlign::Length(*value),
        Some(ComputedValue::Number(value)) => VerticalAlign::Length(*value),
        _ => VerticalAlign::Baseline,
    }
}

fn layout_inline_segments(
    segments: &[InlineSegment],
    start_x: f32,
    start_y: f32,
    available_width: f32,
) -> Vec<LineBox> {
    let mut lines = Vec::new();
    let mut current_fragments = Vec::new();
    let mut cursor_x = start_x;
    let mut cursor_y = start_y;
    let mut current_line_height: f32 = 0.0;

    for segment in segments {
        for piece in split_segment(segment) {
            if piece == "\n" {
                push_line(
                    &mut lines,
                    &mut current_fragments,
                    start_x,
                    cursor_y,
                    cursor_x - start_x,
                    current_line_height.max(segment.line_height),
                );
                cursor_y += current_line_height.max(segment.line_height);
                cursor_x = start_x;
                current_line_height = 0.0;
                continue;
            }

            let piece_width = measure_text_width(&piece, segment.metrics);
            if cursor_x > start_x && cursor_x + piece_width > start_x + available_width {
                push_line(
                    &mut lines,
                    &mut current_fragments,
                    start_x,
                    cursor_y,
                    cursor_x - start_x,
                    current_line_height.max(segment.line_height),
                );
                cursor_y += current_line_height.max(segment.line_height);
                cursor_x = start_x;
                current_line_height = 0.0;
            }

            current_fragments.push(InlineFragment {
                node: segment.node.clone(),
                text: piece.clone(),
                rect: Rect {
                    x: cursor_x,
                    y: cursor_y,
                    width: piece_width,
                    height: segment.line_height,
                },
                metrics: segment.metrics,
                vertical_align: segment.vertical_align,
            });
            cursor_x += piece_width;
            current_line_height = current_line_height.max(segment.line_height);
        }
    }

    if !current_fragments.is_empty() {
        push_line(
            &mut lines,
            &mut current_fragments,
            start_x,
            cursor_y,
            cursor_x - start_x,
            current_line_height.max(0.0),
        );
    }

    lines
}

fn split_segment(segment: &InlineSegment) -> Vec<String> {
    if segment.text.contains('\n') {
        let mut pieces = Vec::new();
        for (index, part) in segment.text.split('\n').enumerate() {
            if !part.is_empty() {
                pieces.extend(split_words_preserving_spaces(part));
            }
            if index + 1 < segment.text.split('\n').count() {
                pieces.push("\n".to_string());
            }
        }
        pieces
    } else {
        split_words_preserving_spaces(&segment.text)
    }
}

fn split_words_preserving_spaces(text: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut was_space = None;

    for ch in text.chars() {
        let is_space = ch == ' ';
        match was_space {
            Some(previous) if previous != is_space => {
                if !current.is_empty() {
                    pieces.push(std::mem::take(&mut current));
                }
            }
            _ => {}
        }
        current.push(ch);
        was_space = Some(is_space);
    }

    if !current.is_empty() {
        pieces.push(current);
    }

    pieces
}

fn push_line(
    lines: &mut Vec<LineBox>,
    fragments: &mut Vec<InlineFragment>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
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
            x,
            y,
            width,
            height,
        },
        baseline: y + baseline,
        fragments: std::mem::take(fragments),
    });
}

fn measure_text_width(text: &str, metrics: FontMetrics) -> f32 {
    text.chars().count() as f32 * metrics.average_advance
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

    #[test]
    fn wraps_inline_text_into_multiple_lines() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        let text = NodeHandle::text("hello world again");

        document.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(text);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("p { line-height: 20px; }").unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 70.0,
                height: 0.0,
            },
        )
        .unwrap();

        let paragraph_box = &layout.children[0];
        assert_eq!(paragraph_box.lines.len(), 3);
        assert_eq!(paragraph_box.lines[0].fragments[0].text, "hello");
        assert_eq!(paragraph_box.lines[1].fragments[0].text.trim(), "world");
        assert_eq!(paragraph_box.lines[2].fragments[0].text.trim(), "again");
        assert_eq!(paragraph_box.dimensions.content.height, 60.0);
    }

    #[test]
    fn normal_white_space_collapses_runs() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        let text = NodeHandle::text("hello   world");

        document.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(text);

        let mut resolver = StyleResolver::new();
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

        let paragraph_box = &layout.children[0];
        let rendered = paragraph_box.lines[0]
            .fragments
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect::<String>();
        assert_eq!(rendered, "hello world");
    }

    #[test]
    fn pre_white_space_preserves_spaces_and_newlines() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        let text = NodeHandle::text("hello   world\nnext");

        document.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(text);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("p { white-space: pre; line-height: 18px; }").unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 500.0,
                height: 0.0,
            },
        )
        .unwrap();

        let paragraph_box = &layout.children[0];
        assert_eq!(paragraph_box.lines.len(), 2);
        let first_line = paragraph_box.lines[0]
            .fragments
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect::<String>();
        let second_line = paragraph_box.lines[1]
            .fragments
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect::<String>();
        assert_eq!(first_line, "hello   world");
        assert_eq!(second_line, "next");
        assert_eq!(paragraph_box.lines[0].rect.height, 18.0);
    }

    #[test]
    fn inline_elements_contribute_text_fragments() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        let span = NodeHandle::element("span");
        let text = NodeHandle::text("inline");

        span.append_child(text);
        document.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(span);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("span { display: inline; }").unwrap(),
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

        let paragraph_box = &layout.children[0];
        assert_eq!(paragraph_box.lines.len(), 1);
        assert_eq!(paragraph_box.lines[0].fragments[0].text, "inline");
    }

    #[test]
    fn approximates_font_metrics_from_font_size() {
        let metrics = FontMetrics::from_font_size(20.0);

        assert_eq!(metrics.font_size, 20.0);
        assert_eq!(metrics.ascent, 16.0);
        assert_eq!(metrics.descent, 4.0);
        assert_eq!(metrics.line_gap, 4.0);
        assert_eq!(metrics.average_advance, 12.0);
    }

    #[test]
    fn vertical_align_top_and_bottom_adjust_fragment_positions() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        let top = NodeHandle::element("span");
        let bottom = NodeHandle::element("span");

        top.set_attribute("class", "top");
        bottom.set_attribute("class", "bottom");
        top.append_child(NodeHandle::text("A"));
        bottom.append_child(NodeHandle::text("B"));
        document.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(top);
        paragraph.append_child(bottom);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "p { line-height: 30px; } \
                 span { display: inline; } \
                 .top { vertical-align: top; } \
                 .bottom { vertical-align: bottom; }",
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

        let line = &layout.children[0].lines[0];
        assert_eq!(line.rect.height, 30.0);
        assert_eq!(line.fragments[0].rect.y, line.rect.y);
        assert_eq!(
            line.fragments[1].rect.y,
            line.rect.y + line.rect.height - line.fragments[1].rect.height
        );
    }

    #[test]
    fn vertical_align_length_raises_fragment_above_baseline() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        let raised = NodeHandle::element("span");

        raised.append_child(NodeHandle::text("lift"));
        document.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(NodeHandle::text("base"));
        paragraph.append_child(raised);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "p { line-height: 20px; } \
                 span { display: inline; vertical-align: 4px; }",
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

        let line = &layout.children[0].lines[0];
        let base_fragment = &line.fragments[0];
        let raised_fragment = line
            .fragments
            .iter()
            .find(|fragment| fragment.text == "lift")
            .unwrap();

        assert!(raised_fragment.rect.y < base_fragment.rect.y);
    }

    #[test]
    fn lays_out_flex_row_with_center_justification() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let first = NodeHandle::element("article");
        let second = NodeHandle::element("article");

        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(first);
        container.append_child(second);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { display: flex; width: 300px; justify-content: center; } \
                 article { width: 100px; height: 40px; }",
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

        let container_box = &layout.children[0];
        assert_eq!(container_box.children.len(), 2);
        assert_eq!(container_box.children[0].dimensions.content.width, 100.0);
        assert_eq!(container_box.children[1].dimensions.content.width, 100.0);
        assert_eq!(container_box.children[0].dimensions.content.x, 50.0);
        assert_eq!(container_box.children[1].dimensions.content.x, 150.0);
    }

    #[test]
    fn grows_last_flex_item_to_fill_remaining_space() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let first = NodeHandle::element("article");
        let second = NodeHandle::element("article");

        second.set_attribute("class", "grow");
        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(first);
        container.append_child(second);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { display: flex; width: 300px; } \
                 article { flex-basis: 100px; height: 40px; } \
                 .grow { flex-grow: 1; }",
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

        let container_box = &layout.children[0];
        assert_eq!(container_box.children[0].dimensions.content.width, 100.0);
        assert_eq!(container_box.children[1].dimensions.content.width, 200.0);
    }

    #[test]
    fn lays_out_flex_column() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let first = NodeHandle::element("section");
        let second = NodeHandle::element("section");

        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(first);
        container.append_child(second);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { display: flex; flex-direction: column; width: 120px; } \
                 section { height: 30px; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 120.0,
                height: 0.0,
            },
        )
        .unwrap();

        let container_box = &layout.children[0];
        assert_eq!(container_box.children[0].dimensions.content.y, 0.0);
        assert_eq!(container_box.children[1].dimensions.content.y, 30.0);
    }

    #[test]
    fn wraps_flex_items_across_multiple_lines() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");

        for _ in 0..3 {
            container.append_child(NodeHandle::element("article"));
        }

        document.append_child(body.clone());
        body.append_child(container);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { display: flex; width: 200px; flex-wrap: wrap; } \
                 article { width: 100px; height: 20px; }",
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

        let container_box = &layout.children[0];
        assert_eq!(container_box.children[0].dimensions.content.y, 0.0);
        assert_eq!(container_box.children[1].dimensions.content.y, 0.0);
        assert_eq!(container_box.children[2].dimensions.content.y, 20.0);
    }

    #[test]
    fn aligns_flex_items_with_align_items_and_align_self() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let first = NodeHandle::element("article");
        let second = NodeHandle::element("article");

        first.set_attribute("class", "tall");
        second.set_attribute("class", "self-end");
        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(first);
        container.append_child(second);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { display: flex; width: 200px; height: 80px; align-items: center; } \
                 article { width: 60px; height: 10px; } \
                 .tall { height: 20px; } \
                 .self-end { align-self: flex-end; }",
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

        let container_box = &layout.children[0];
        assert_eq!(container_box.children[0].dimensions.content.y, 0.0);
        assert_eq!(container_box.children[1].dimensions.content.y, 10.0);
    }

    #[test]
    fn absolutely_positions_child_relative_to_parent_content_box() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let absolute = NodeHandle::element("aside");
        let flow = NodeHandle::element("section");

        absolute.set_attribute("class", "absolute");
        flow.set_attribute("class", "flow");
        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(flow);
        container.append_child(absolute);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { width: 200px; padding-left: 10px; padding-top: 5px; } \
                 .flow { height: 20px; } \
                 .absolute { position: absolute; left: 30px; top: 12px; width: 50px; height: 15px; }",
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

        let container_box = &layout.children[0];
        let absolute_box = container_box
            .children
            .iter()
            .find(|child| child.node.tag_name().as_deref() == Some("aside"))
            .unwrap();
        assert_eq!(absolute_box.dimensions.content.x, 40.0);
        assert_eq!(absolute_box.dimensions.content.y, 17.0);
        assert_eq!(container_box.dimensions.content.height, 20.0);
    }

    #[test]
    fn fixed_position_uses_viewport_as_containing_block() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let fixed = NodeHandle::element("div");

        fixed.set_attribute("class", "fixed");
        document.append_child(body.clone());
        body.append_child(fixed);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".fixed { position: fixed; right: 10px; bottom: 20px; width: 50px; height: 30px; }",
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
                height: 200.0,
            },
        )
        .unwrap();

        let fixed_box = &layout.children[0];
        assert_eq!(fixed_box.dimensions.content.x, 240.0);
        assert_eq!(fixed_box.dimensions.content.y, 150.0);
    }

    #[test]
    fn sorts_siblings_by_z_index() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let low = NodeHandle::element("div");
        let high = NodeHandle::element("div");

        low.set_attribute("class", "low");
        high.set_attribute("class", "high");
        document.append_child(body.clone());
        body.append_child(low);
        body.append_child(high);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".low { position: absolute; z-index: 1; width: 10px; height: 10px; } \
                 .high { position: absolute; z-index: 10; width: 10px; height: 10px; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
        )
        .unwrap();

        assert_eq!(layout.children[0].z_index, 1);
        assert_eq!(layout.children[1].z_index, 10);
        assert_eq!(
            layout.children[0]
                .node
                .attributes()
                .unwrap()
                .get("class")
                .unwrap(),
            "low"
        );
        assert_eq!(
            layout.children[1]
                .node
                .attributes()
                .unwrap()
                .get("class")
                .unwrap(),
            "high"
        );
    }
}
