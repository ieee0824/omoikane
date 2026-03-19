//! Block layout primitives.
//!
//! The layout phase consumes DOM nodes together with computed styles and
//! produces a tree of rectangular block boxes.

use crate::css::{ComputedStyle, ComputedValue, PseudoElement, StyleResolver};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::paint::{DataUri, Image, parse_data_uri};

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
    pub content: InlineFragmentContent,
    pub rect: Rect,
    pub metrics: FontMetrics,
    pub vertical_align: VerticalAlign,
}

/// A laid out inline fragment payload.
#[derive(Debug, Clone, PartialEq)]
pub enum InlineFragmentContent {
    Text(String),
    Image(Image, ComputedStyle),
    GeneratedBox(ComputedStyle),
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
enum TableDisplay {
    Table,
    RowGroup,
    Row,
    Cell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PositionScheme {
    Static,
    Relative,
    Absolute,
    Fixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatSide {
    None,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy)]
struct FloatRegion {
    outer: Rect,
    side: FloatSide,
}

#[derive(Debug, Clone, Copy, Default)]
struct FloatOffsets {
    left: f32,
    right: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClearSide {
    None,
    Left,
    Right,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextAlign {
    Left,
    Right,
    Center,
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

impl InlineFragment {
    /// Returns the text payload when this fragment represents text.
    pub fn text(&self) -> Option<&str> {
        match &self.content {
            InlineFragmentContent::Text(text) => Some(text.as_str()),
            _ => None,
        }
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
    layout_node(node, resolver, containing_block, containing_block, None)
}

fn layout_node(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    viewport: Rect,
    positioned_ancestor: Option<BoxDimensions>,
) -> Option<LayoutBox> {
    match node.node_type() {
        NodeType::Document => {
            layout_document(node, resolver, containing_block, viewport, positioned_ancestor)
        }
        NodeType::Element => {
            layout_element(node, resolver, containing_block, viewport, positioned_ancestor)
        }
        _ => None,
    }
}

fn layout_document(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    viewport: Rect,
    positioned_ancestor: Option<BoxDimensions>,
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
        if let Some(style) = &child_style {
            if is_out_of_flow_positioned(style) {
                positioned_children.push((child, style.clone(), child_containing));
                continue;
            }
        }

        if let Some(layout_child) =
            layout_node(&child, resolver, child_containing, viewport, positioned_ancestor)
        {
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
    for (child, style, static_position) in positioned_children {
        if let Some(positioned) =
            layout_positioned_child(
                &child,
                resolver,
                &style,
                positioned_ancestor.unwrap_or(document_box),
                static_position,
                viewport,
            )
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
    positioned_ancestor: Option<BoxDimensions>,
) -> Option<LayoutBox> {
    if is_non_rendered_html_element(node) {
        return None;
    }

    let style = resolver.computed_style(node);
    if is_display_none(&style) {
        return None;
    }

    let padding = edge_sizes(&style, "padding");
    let border = edge_sizes(&style, "border");
    let mut margin = edge_sizes(&style, "margin");

    let mut width = compute_width(&style, containing_block.width, padding, border, &mut margin);
    if float_side(&style) != FloatSide::None
        && resolved_length(&style, "width", containing_block.width).is_none()
    {
        width = shrink_to_fit_width(node, resolver, containing_block.width);
    }
    let x = containing_block.x + margin.left + border.left + padding.left;
    let y = containing_block.y + margin.top + border.top + padding.top;

    if is_table_container_element(node, &style) {
        if resolved_length(&style, "width", containing_block.width).is_none() {
            width = shrink_to_fit_width(node, resolver, containing_block.width);
        }
        return layout_table_container(
            node, resolver, style, margin, padding, border, x, y, width, viewport,
        );
    }

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
    let mut float_regions = Vec::new();

    for child in node.child_nodes() {
        if is_inline_child(&child, resolver) {
            pending_inline_nodes.push(child);
            continue;
        }

        if !pending_inline_nodes.is_empty() {
            let all_whitespace = pending_inline_nodes.iter().all(|n| {
                n.node_type() == NodeType::Text
                    && n.data()
                        .map(|t| t.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0C')))
                        .unwrap_or(true)
            });
            if !all_whitespace {
                let offsets = active_float_offsets(&float_regions, cursor_y, x, width);
                let inline_lines = layout_inline_nodes(
                    &pending_inline_nodes,
                    resolver,
                    x + offsets.left,
                    cursor_y,
                    (width - offsets.left - offsets.right).max(0.0),
                    text_align(&style),
                );
                if let Some(last_line) = inline_lines.last() {
                    cursor_y = last_line.rect.y + last_line.rect.height;
                }
                lines.extend(inline_lines);
            }
            pending_inline_nodes.clear();
        }

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
        if let Some(style) = &child_style {
            match clear_side(style) {
                ClearSide::Left => {
                    cursor_y = clear_cursor_y_for_side(
                        cursor_y,
                        child_margin_top,
                        collapse_delta,
                        &float_regions,
                        FloatSide::Left,
                    );
                }
                ClearSide::Right => {
                    cursor_y = clear_cursor_y_for_side(
                        cursor_y,
                        child_margin_top,
                        collapse_delta,
                        &float_regions,
                        FloatSide::Right,
                    );
                }
                ClearSide::Both => {
                    cursor_y = clear_cursor_y_for_side(
                        cursor_y,
                        child_margin_top,
                        collapse_delta,
                        &float_regions,
                        FloatSide::Left,
                    );
                    cursor_y = clear_cursor_y_for_side(
                        cursor_y,
                        child_margin_top,
                        collapse_delta,
                        &float_regions,
                        FloatSide::Right,
                    );
                }
                ClearSide::None => {}
            }
        }
        if let Some(child_style) = &child_style {
            let parent_top_margin_collapse = previous_margin_bottom.is_none()
                && lines.is_empty()
                && pending_inline_nodes.is_empty()
                && border.top == 0.0
                && padding.top == 0.0
                && clear_side(child_style) == ClearSide::None
                && !is_out_of_flow_positioned(child_style)
                && float_side(child_style) == FloatSide::None;
            let effective_collapse_delta = if parent_top_margin_collapse {
                collapse_delta + child_margin_top
            } else {
                collapse_delta
            };
            let child_y = cursor_y - effective_collapse_delta;
            let offsets = active_float_offsets(&float_regions, child_y, x, width);
            let available_width = (width - offsets.left - offsets.right).max(0.0);
            let child_containing = Rect {
                x: if explicit_length(child_style, "width").is_some() {
                    x
                } else {
                    x + offsets.left
                },
                y: child_y,
                width: if explicit_length(child_style, "width").is_some() {
                    width
                } else {
                    available_width
                },
                height: 0.0,
            };
            if is_out_of_flow_positioned(child_style) {
                positioned_children.push((child, child_style.clone(), child_containing));
                continue;
            }
            let float_side = float_side(child_style);
            if float_side != FloatSide::None {
                let float_width = resolved_length(child_style, "width", available_width)
                    .unwrap_or_else(|| shrink_to_fit_width(&child, resolver, width));
                let mut float_y = child_y;
                loop {
                    let offsets = active_float_offsets(&float_regions, float_y, x, width);
                    let float_available_width = (width - offsets.left - offsets.right).max(0.0);
                    if float_width <= float_available_width + 0.5 {
                        let float_containing = Rect {
                            x: x + offsets.left,
                            y: float_y,
                            width: float_available_width.max(float_width),
                            height: 0.0,
                        };
                        if let Some(mut layout_child) = layout_node(
                            &child,
                            resolver,
                            float_containing,
                            viewport,
                            positioned_ancestor,
                        ) {
                            if resolved_length(child_style, "width", float_available_width).is_none()
                            {
                                layout_child.dimensions.content.width = float_width;
                            }
                            let outer_y = float_containing.y;
                            let outer_x = match float_side {
                                FloatSide::Left => x + offsets.left,
                                FloatSide::Right => {
                                    x + width - offsets.right - layout_child.total_width()
                                }
                                FloatSide::None => x + offsets.left,
                            };
                            translate_layout_box_to_outer(&mut layout_child, outer_x, outer_y);
                            float_regions.push(FloatRegion {
                                outer: Rect {
                                    x: outer_x,
                                    y: outer_y,
                                    width: layout_child.total_width(),
                                    height: layout_child.total_height(),
                                },
                                side: float_side,
                            });
                            children.push(layout_child);
                        }
                        break;
                    }
                    let Some(next_y) = next_float_boundary_after(&float_regions, float_y) else {
                        break;
                    };
                    if next_y <= float_y {
                        break;
                    }
                    float_y = next_y;
                }
                // Floats don't participate in margin collapsing;
                // preserve previous_margin_bottom so adjacent in-flow
                // siblings can still collapse through.
                continue;
            }
            let next_positioned_ancestor = if establishes_positioned_containing_block(&style) {
                Some(BoxDimensions {
                    content: Rect {
                        x,
                        y,
                        width,
                        height: 0.0,
                    },
                    padding,
                    border,
                    margin,
                })
            } else {
                positioned_ancestor
            };
            if let Some(layout_child) = layout_node(
                &child,
                resolver,
                child_containing,
                viewport,
                next_positioned_ancestor,
            ) {
                if is_empty_for_margin_collapse(&layout_child) {
                    let prev = previous_margin_bottom.unwrap_or(0.0);
                    let empty_collapsed = collapse_through_empty(&layout_child);
                    let combined = collapse_margins(prev, empty_collapsed);
                    // Advance cursor_y by the difference between the combined
                    // collapsed margin and the previous margin already tracked.
                    cursor_y += combined - prev - (effective_collapse_delta - collapse_delta);
                    previous_margin_bottom = Some(combined);
                    children.push(layout_child);
                } else {
                    cursor_y += layout_child.total_height() - effective_collapse_delta;
                    previous_margin_bottom = Some(layout_child.dimensions.margin.bottom);
                    children.push(layout_child);
                }
            }
            continue;
        }

        let child_containing = Rect {
            x,
            y: cursor_y - collapse_delta,
            width,
            height: 0.0,
        };
        if let Some(layout_child) =
            layout_node(&child, resolver, child_containing, viewport, positioned_ancestor)
        {
            if is_empty_for_margin_collapse(&layout_child) {
                let prev = previous_margin_bottom.unwrap_or(0.0);
                let empty_collapsed = collapse_through_empty(&layout_child);
                let combined = collapse_margins(prev, empty_collapsed);
                cursor_y += combined - prev;
                previous_margin_bottom = Some(combined);
                children.push(layout_child);
            } else {
                cursor_y += layout_child.total_height() - collapse_delta;
                previous_margin_bottom = Some(layout_child.dimensions.margin.bottom);
                children.push(layout_child);
            }
        }
    }

    if !pending_inline_nodes.is_empty() {
        let all_whitespace = pending_inline_nodes.iter().all(|n| {
            n.node_type() == NodeType::Text
                && n.data()
                    .map(|t| t.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0C')))
                    .unwrap_or(true)
        });
        if !all_whitespace {
            let offsets = active_float_offsets(&float_regions, cursor_y, x, width);
            let inline_lines = layout_inline_nodes(
                &pending_inline_nodes,
                resolver,
                x + offsets.left,
                cursor_y,
                (width - offsets.left - offsets.right).max(0.0),
                text_align(&style),
            );
            if let Some(last_line) = inline_lines.last() {
                cursor_y = last_line.rect.y + last_line.rect.height;
            }
            lines.extend(inline_lines);
        }
    }

    let float_bottom = float_regions
        .iter()
        .map(|region| region.outer.y + region.outer.height)
        .fold(y, f32::max);
    let auto_height = (cursor_y.max(float_bottom)) - y;
    let mut content_height = resolved_length(&style, "height", containing_block.height)
        .unwrap_or(auto_height);
    let (min_height, max_height) = normalized_min_max_lengths(
        &style,
        "min-height",
        "max-height",
        containing_block.height,
    );
    if let Some(min_height) = min_height {
        content_height = content_height.max(min_height);
    }
    if let Some(max_height) = max_height {
        content_height = content_height.min(max_height);
    }
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
    let next_positioned_ancestor = if establishes_positioned_containing_block(&style) {
        Some(dimensions)
    } else {
        positioned_ancestor
    };
    for (child, style, static_position) in positioned_children {
        if let Some(positioned) = layout_positioned_child(
            &child,
            resolver,
            &style,
            next_positioned_ancestor.unwrap_or(dimensions),
            static_position,
            viewport,
        ) {
            children.push(positioned);
        }
    }
    sort_children_by_z_index(&mut children);
    let mut layout = LayoutBox {
        node: node.clone(),
        dimensions,
        visibility: visibility(&style),
        overflow: overflow(&style),
        z_index: z_index(&style),
        lines,
        children,
    };
    apply_relative_offset(&mut layout, &style);

    Some(layout)
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
                layout_node(&item.node, resolver, child_containing, viewport, None)
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

    let auto_height = cross_cursor - y;
    let mut content_height = resolved_length(&style, "height", 0.0).unwrap_or(auto_height);
    let (min_height, max_height) =
        normalized_min_max_lengths(&style, "min-height", "max-height", 0.0);
    if let Some(min_height) = min_height {
        content_height = content_height.max(min_height);
    }
    if let Some(max_height) = max_height {
        content_height = content_height.min(max_height);
    }
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

fn layout_table_container(
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
    let spacing = table_border_spacing(&style);
    let collapse_spacing = spacing * 2.0;
    let mut entries = collect_table_entries(node, resolver);
    let column_count = entries
        .iter()
        .map(|entry| entry.cells.len())
        .max()
        .unwrap_or(0)
        .max(1);
    let column_width =
        ((width - spacing * (column_count as f32 + 1.0)).max(0.0)) / column_count as f32;
    let inner_width = (width - collapse_spacing).max(0.0);

    let mut children = Vec::new();
    let mut cursor_y = y + spacing;
    let mut pending_group: Option<(NodeHandle, Vec<LayoutBox>, f32, f32)> = None;

    for entry in entries.drain(..) {
        let row_y = cursor_y;
        let (row_box, row_height) =
            layout_table_row_entry(&entry, resolver, x + spacing, row_y, column_width, spacing, viewport)?;
        cursor_y += row_height + spacing;

        if let Some(group_node) = entry.row_group {
            match &mut pending_group {
                Some((current_group, rows, _, group_start_y)) if *current_group == group_node => {
                    rows.push(row_box);
                    let _ = group_start_y;
                }
                Some((current_group, rows, _, group_start_y)) => {
                    let group_box = build_row_group_box(
                        current_group.clone(),
                        std::mem::take(rows),
                        x + spacing,
                        *group_start_y,
                        inner_width,
                    );
                    children.push(group_box);
                    *current_group = group_node.clone();
                    *group_start_y = row_y;
                    rows.push(row_box);
                }
                None => {
                    pending_group = Some((group_node, vec![row_box], inner_width, row_y));
                }
            }
        } else {
            if let Some((group_node, rows, _, group_start_y)) = pending_group.take() {
                let group_box =
                    build_row_group_box(group_node, rows, x + spacing, group_start_y, inner_width);
                children.push(group_box);
            }
            children.push(row_box);
        }
    }

    if let Some((group_node, rows, _, group_start_y)) = pending_group.take() {
        let group_box = build_row_group_box(group_node, rows, x + spacing, group_start_y, inner_width);
        children.push(group_box);
    }

    let auto_height = (cursor_y - y).max(spacing);
    let mut content_height = resolved_length(&style, "height", 0.0).unwrap_or(auto_height);
    let (min_height, max_height) =
        normalized_min_max_lengths(&style, "min-height", "max-height", 0.0);
    if let Some(min_height) = min_height {
        content_height = content_height.max(min_height);
    }
    if let Some(max_height) = max_height {
        content_height = content_height.min(max_height);
    }
    Some(LayoutBox {
        node: node.clone(),
        dimensions: BoxDimensions {
            content: Rect {
                x,
                y,
                width,
                height: content_height,
            },
            padding,
            border,
            margin,
        },
        visibility: visibility(&style),
        overflow: overflow(&style),
        z_index: z_index(&style),
        lines: Vec::new(),
        children,
    })
}

#[derive(Debug, Clone)]
struct TableRowEntry {
    row_node: NodeHandle,
    row_group: Option<NodeHandle>,
    cells: Vec<NodeHandle>,
}

fn collect_table_entries(node: &NodeHandle, resolver: &mut StyleResolver) -> Vec<TableRowEntry> {
    let mut entries = Vec::new();
    let mut anonymous_cells = Vec::new();

    for child in node.child_nodes() {
        match table_display_for_node(&child, &resolver.computed_style(&child)) {
            Some(TableDisplay::RowGroup) => {
                flush_anonymous_row(&mut entries, &mut anonymous_cells);
                for row in child.child_nodes() {
                    match table_display_for_node(&row, &resolver.computed_style(&row)) {
                        Some(TableDisplay::Row) => entries.push(TableRowEntry {
                            row_node: row.clone(),
                            row_group: Some(child.clone()),
                            cells: collect_row_cells(&row, resolver),
                        }),
                        Some(TableDisplay::Cell) => anonymous_cells.push(row),
                        _ => {}
                    }
                }
            }
            Some(TableDisplay::Row) => {
                flush_anonymous_row(&mut entries, &mut anonymous_cells);
                entries.push(TableRowEntry {
                    row_node: child.clone(),
                    row_group: None,
                    cells: collect_row_cells(&child, resolver),
                });
            }
            Some(TableDisplay::Cell) => anonymous_cells.push(child),
            Some(TableDisplay::Table) => {
                // CSS 2.1 §17.2.1: wrap non-row/cell children in anonymous cell
                anonymous_cells.push(child);
            }
            _ => {
                if child.node_type() == NodeType::Element {
                    // Treat block-level children as anonymous cells
                    anonymous_cells.push(child);
                }
            }
        }
    }

    flush_anonymous_row(&mut entries, &mut anonymous_cells);
    entries
}

fn flush_anonymous_row(entries: &mut Vec<TableRowEntry>, anonymous_cells: &mut Vec<NodeHandle>) {
    if anonymous_cells.is_empty() {
        return;
    }
    entries.push(TableRowEntry {
        row_node: NodeHandle::element("tr"),
        row_group: None,
        cells: std::mem::take(anonymous_cells),
    });
}

fn collect_row_cells(row: &NodeHandle, resolver: &mut StyleResolver) -> Vec<NodeHandle> {
    row.child_nodes()
        .into_iter()
        .filter(|child| matches!(table_display_for_node(child, &resolver.computed_style(child)), Some(TableDisplay::Cell)))
        .collect()
}

fn layout_table_row_entry(
    entry: &TableRowEntry,
    resolver: &mut StyleResolver,
    x: f32,
    y: f32,
    column_width: f32,
    spacing: f32,
    viewport: Rect,
) -> Option<(LayoutBox, f32)> {
    let mut measured = Vec::new();
    let mut row_height = 0.0f32;

    for (index, cell) in entry.cells.iter().enumerate() {
        let cell_containing = Rect {
            x: 0.0,
            y: 0.0,
            width: column_width,
            height: 0.0,
        };
        let mut layout_cell = layout_node(cell, resolver, cell_containing, viewport, None)?;
        let cell_style = resolver.computed_style(cell);
        let cell_height = explicit_length(&cell_style, "height").unwrap_or(layout_cell.total_height());
        layout_cell.dimensions.content.width = column_width;
        layout_cell.dimensions.content.height = cell_height;
        row_height = row_height.max(layout_cell.total_height());
        measured.push((index, layout_cell, cell_style));
    }

    let mut children = Vec::new();
    for (index, mut cell, cell_style) in measured {
        let outer_x = x + index as f32 * (column_width + spacing);
        let original_total_height = cell.total_height();
        let extra_height = (row_height - original_total_height).max(0.0);
        if extra_height > 0.0 {
            cell.dimensions.content.height += extra_height;
            let content_offset = match vertical_align(&cell_style) {
                VerticalAlign::Bottom => extra_height,
                VerticalAlign::Middle => extra_height / 2.0,
                _ => 0.0,
            };
            if content_offset > 0.0 {
                translate_layout_contents(&mut cell, 0.0, content_offset);
            }
        }
        let outer_y = y;
        translate_layout_box_to_outer(&mut cell, outer_x, outer_y);
        children.push(cell);
    }

    let row_width = if entry.cells.is_empty() {
        0.0
    } else {
        entry.cells.len() as f32 * column_width + (entry.cells.len().saturating_sub(1)) as f32 * spacing
    };
    let row_box = LayoutBox {
        node: entry.row_node.clone(),
        dimensions: BoxDimensions {
            content: Rect {
                x,
                y,
                width: row_width,
                height: row_height,
            },
            ..BoxDimensions::default()
        },
        visibility: Visibility::Visible,
        overflow: Overflow::Visible,
        z_index: 0,
        lines: Vec::new(),
        children,
    };

    Some((row_box, row_height))
}

fn build_row_group_box(
    node: NodeHandle,
    rows: Vec<LayoutBox>,
    x: f32,
    y: f32,
    width: f32,
) -> LayoutBox {
    let height = rows
        .last()
        .map(|row| row.dimensions.content.y + row.dimensions.content.height - y)
        .unwrap_or(0.0);

    LayoutBox {
        node,
        dimensions: BoxDimensions {
            content: Rect {
                x,
                y,
                width,
                height,
            },
            ..BoxDimensions::default()
        },
        visibility: Visibility::Visible,
        overflow: Overflow::Visible,
        z_index: 0,
        lines: Vec::new(),
        children: rows,
    }
}

fn compute_width(
    style: &ComputedStyle,
    containing_width: f32,
    padding: EdgeSizes,
    border: EdgeSizes,
    margin: &mut EdgeSizes,
) -> f32 {
    let specified_width = resolved_length(style, "width", containing_width);
    let margin_left_auto = is_auto(style.get("margin-left"));
    let margin_right_auto = is_auto(style.get("margin-right"));

    let mut width = if let Some(width) = specified_width {
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
    };

    let (min_width, max_width) =
        normalized_min_max_lengths(style, "min-width", "max-width", containing_width);
    if let Some(min_width) = min_width {
        width = width.max(min_width);
    }
    if let Some(max_width) = max_width {
        width = width.min(max_width);
    }

    width
}

fn normalized_min_max_lengths(
    style: &ComputedStyle,
    min_name: &str,
    max_name: &str,
    containing_length: f32,
) -> (Option<f32>, Option<f32>) {
    let min = resolved_length(style, min_name, containing_length);
    let max = resolved_length(style, max_name, containing_length);
    match (min, max) {
        (Some(min), Some(max)) if min > max => (Some(min), Some(min)),
        pair => pair,
    }
}

fn active_float_offsets(regions: &[FloatRegion], y: f32, x: f32, width: f32) -> FloatOffsets {
    let mut offsets = FloatOffsets::default();
    for region in regions {
        if y < region.outer.y || y >= region.outer.y + region.outer.height {
            continue;
        }
        match region.side {
            FloatSide::Left => {
                offsets.left = offsets.left.max((region.outer.x + region.outer.width - x).max(0.0));
            }
            FloatSide::Right => {
                let right_edge = x + width;
                offsets.right = offsets.right.max((right_edge - region.outer.x).max(0.0));
            }
            FloatSide::None => {}
        }
    }
    offsets
}

fn next_float_boundary_after(regions: &[FloatRegion], y: f32) -> Option<f32> {
    regions
        .iter()
        .filter_map(|region| {
            let bottom = region.outer.y + region.outer.height;
            (bottom > y).then_some(bottom)
        })
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
}

fn clear_cursor_y_for_side(
    cursor_y: f32,
    child_margin_top: f32,
    collapse_delta: f32,
    regions: &[FloatRegion],
    side: FloatSide,
) -> f32 {
    let border_edge_top = cursor_y + child_margin_top - collapse_delta;
    let interfering_bottom = regions
        .iter()
        .filter(|region| match side {
            FloatSide::Left => region.side == FloatSide::Left,
            FloatSide::Right => region.side == FloatSide::Right,
            FloatSide::None => false,
        })
        .filter(|region| region.outer.y + region.outer.height > border_edge_top)
        .map(|region| region.outer.y + region.outer.height)
        .fold(border_edge_top, f32::max);
    cursor_y.max(interfering_bottom + collapse_delta - child_margin_top)
}



fn edge_sizes(style: &ComputedStyle, prefix: &str) -> EdgeSizes {
    let shorthand_property = match prefix {
        "border" => "border-width",
        _ => prefix,
    };
    let side_property = match prefix {
        "border" => "border-{}-width",
        _ => "{prefix}-{}",
    };
    let shorthand = explicit_length(style, shorthand_property).unwrap_or(0.0);
    EdgeSizes {
        top: explicit_length(style, &side_property.replace("{}", "top").replace("{prefix}", prefix))
            .or_else(|| explicit_length(style, &format!("{prefix}-top")))
            .unwrap_or(shorthand),
        right: explicit_length(style, &side_property.replace("{}", "right").replace("{prefix}", prefix))
            .or_else(|| explicit_length(style, &format!("{prefix}-right")))
            .unwrap_or(shorthand),
        bottom: explicit_length(style, &side_property.replace("{}", "bottom").replace("{prefix}", prefix))
            .or_else(|| explicit_length(style, &format!("{prefix}-bottom")))
            .unwrap_or(shorthand),
        left: explicit_length(style, &side_property.replace("{}", "left").replace("{prefix}", prefix))
            .or_else(|| explicit_length(style, &format!("{prefix}-left")))
            .unwrap_or(shorthand),
    }
}

fn explicit_length(style: &ComputedStyle, property: &str) -> Option<f32> {
    match style.get(property) {
        Some(ComputedValue::Px(value)) => Some(*value),
        // CSS 2.1: unitless numbers are only valid as lengths when the value is 0
        Some(ComputedValue::Number(value)) if *value == 0.0 => Some(0.0),
        _ => None,
    }
}

fn percentage_length(style: &ComputedStyle, property: &str) -> Option<f32> {
    match style.get(property) {
        Some(ComputedValue::Percentage(value)) => Some(*value),
        _ => None,
    }
}

fn resolved_length(style: &ComputedStyle, property: &str, basis: f32) -> Option<f32> {
    explicit_length(style, property).or_else(|| {
        percentage_length(style, property).and_then(|percent| {
            if basis > 0.0 {
                Some(basis * (percent / 100.0))
            } else {
                None
            }
        })
    })
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

/// CSS 2.1 §8.3.1: An element is "empty" for margin collapsing when it has
/// zero height, zero vertical border/padding, no line boxes, and all
/// children (if any) are themselves empty for margin collapsing.
fn is_empty_for_margin_collapse(layout: &LayoutBox) -> bool {
    layout.dimensions.content.height == 0.0
        && layout.dimensions.padding.top == 0.0
        && layout.dimensions.padding.bottom == 0.0
        && layout.dimensions.border.top == 0.0
        && layout.dimensions.border.bottom == 0.0
        && layout.lines.is_empty()
        && layout.children.iter().all(|c| is_empty_for_margin_collapse(c))
}

/// Collapse all margins through an empty element and its empty descendants.
/// Returns the single collapsed margin value that represents the entire chain.
fn collapse_through_empty(layout: &LayoutBox) -> f32 {
    let mut result = collapse_margins(
        layout.dimensions.margin.top,
        layout.dimensions.margin.bottom,
    );
    for child in &layout.children {
        result = collapse_margins(result, collapse_through_empty(child));
    }
    result
}

fn is_out_of_flow_positioned(style: &ComputedStyle) -> bool {
    matches!(
        position_scheme(style),
        PositionScheme::Absolute | PositionScheme::Fixed
    )
}

fn establishes_positioned_containing_block(style: &ComputedStyle) -> bool {
    matches!(
        position_scheme(style),
        PositionScheme::Relative | PositionScheme::Absolute | PositionScheme::Fixed
    )
}

fn position_scheme(style: &ComputedStyle) -> PositionScheme {
    match style.get("position") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("relative") => {
            PositionScheme::Relative
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("absolute") => {
            PositionScheme::Absolute
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("fixed") => {
            PositionScheme::Fixed
        }
        _ => PositionScheme::Static,
    }
}

fn float_side(style: &ComputedStyle) -> FloatSide {
    match style.get("float") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("left") => {
            FloatSide::Left
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("right") => {
            FloatSide::Right
        }
        _ => FloatSide::None,
    }
}

fn clear_side(style: &ComputedStyle) -> ClearSide {
    match style.get("clear") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("left") => {
            ClearSide::Left
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("right") => {
            ClearSide::Right
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("both") => {
            ClearSide::Both
        }
        _ => ClearSide::None,
    }
}

fn shrink_to_fit_width(node: &NodeHandle, resolver: &mut StyleResolver, available_width: f32) -> f32 {
    let outer = intrinsic_width(node, resolver);
    let style = resolver.computed_style(node);
    let padding = edge_sizes(&style, "padding");
    let border = edge_sizes(&style, "border");
    (outer - padding.horizontal() - border.horizontal())
        .max(0.0)
        .min(available_width)
}

fn shrink_to_fit_layout_width(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    available_width: f32,
) -> f32 {
    let style = resolver.computed_style(node);
    let padding = edge_sizes(&style, "padding");
    let border = edge_sizes(&style, "border");
    let mut margin = edge_sizes(&style, "margin");
    if is_auto(style.get("margin-left")) {
        margin.left = 0.0;
    }
    if is_auto(style.get("margin-right")) {
        margin.right = 0.0;
    }

    shrink_to_fit_width(node, resolver, available_width)
        + padding.horizontal()
        + border.horizontal()
        + margin.horizontal()
}

fn used_content_width(layout: &LayoutBox) -> f32 {
    let content_left = layout.dimensions.content.x;
    let line_width = layout
        .lines
        .iter()
        .map(|line| (line.rect.x + line.rect.width - content_left).max(0.0))
        .fold(0.0, f32::max);
    let child_width = layout
        .children
        .iter()
        .map(|child| {
            let outer_right = child.dimensions.content.x
                + child.dimensions.content.width
                + child.dimensions.padding.right
                + child.dimensions.border.right
                + child.dimensions.margin.right;
            (outer_right - content_left).max(0.0)
        })
        .fold(0.0, f32::max);

    line_width.max(child_width)
}

fn auto_width_from_layout(
    layout: &LayoutBox,
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    available_width: f32,
) -> f32 {
    used_content_width(layout)
        .max(shrink_to_fit_width(node, resolver, available_width))
        .min(available_width)
        .max(0.0)
}

/// Returns the outer width (content + padding + border) that `node` needs.
/// Used by parent elements to determine how wide their content area must be.
fn intrinsic_width(node: &NodeHandle, resolver: &mut StyleResolver) -> f32 {
    match node.node_type() {
        NodeType::Text => node
            .data()
            .map(|text| {
                let parent_style = node
                    .parent_node()
                    .map(|parent| resolver.computed_style(&parent))
                    .unwrap_or_default();
                measure_text_width(&normalize_text(&text, white_space(&parent_style)), font_metrics(&parent_style))
            })
            .unwrap_or(0.0),
        NodeType::Element => {
            let style = resolver.computed_style(node);
            let padding = edge_sizes(&style, "padding");
            let border = edge_sizes(&style, "border");
            if let Some(width) = explicit_length(&style, "width") {
                let margin = edge_sizes(&style, "margin");
                return width + padding.horizontal() + border.horizontal() + margin.horizontal();
            }
            if let Some((image_node, image)) = element_inline_image(node) {
                let image_style = resolver.computed_style(&image_node);
                let img_padding = edge_sizes(&image_style, "padding");
                let img_border = edge_sizes(&image_style, "border");
                return image.width() as f32
                    + img_padding.left
                    + img_padding.right
                    + img_border.left
                    + img_border.right;
            }
            // Content width = max of children's outer widths
            let mut content_width: f32 = 0.0;
            if is_table_container_element(node, &style) {
                let entries = collect_table_entries(node, resolver);
                let spacing = table_border_spacing(&style);
                for entry in &entries {
                    let row_width: f32 = entry
                        .cells
                        .iter()
                        .map(|cell| intrinsic_width(cell, resolver))
                        .sum::<f32>()
                        + spacing * (entry.cells.len().max(1) as f32 + 1.0);
                    content_width = content_width.max(row_width);
                }
            } else {
                for child in node.child_nodes() {
                    content_width = content_width.max(intrinsic_width(&child, resolver));
                }
            }
            let mut width = content_width;
            if width == 0.0 {
                width = generated_inline_segments(node, resolver, PseudoElement::Before)
                    .into_iter()
                    .chain(generated_inline_segments(node, resolver, PseudoElement::After))
                    .map(|segment| match segment.content {
                        InlineSegmentContent::Text(text) => measure_text_width(&text, segment.metrics),
                        InlineSegmentContent::Image(image, style) => {
                            let padding = edge_sizes(&style, "padding");
                            let border = edge_sizes(&style, "border");
                            image.width() as f32
                                + padding.left
                                + padding.right
                                + border.left
                                + border.right
                        }
                        InlineSegmentContent::GeneratedBox(style) => {
                            let padding = edge_sizes(&style, "padding");
                            let border = edge_sizes(&style, "border");
                            explicit_length(&style, "width").unwrap_or(0.0)
                                + padding.left
                                + padding.right
                                + border.left
                                + border.right
                        }
                    })
                    .fold(0.0, f32::max);
            }
            // Outer width = content + own padding + border
            width + padding.horizontal() + border.horizontal()
        }
        _ => 0.0,
    }
}

fn z_index(style: &ComputedStyle) -> i32 {
    match style.get("z-index") {
        Some(ComputedValue::Number(value)) => *value as i32,
        Some(ComputedValue::Px(value)) => *value as i32,
        _ => 0,
    }
}

fn apply_relative_offset(layout: &mut LayoutBox, style: &ComputedStyle) {
    if position_scheme(style) != PositionScheme::Relative {
        return;
    }

    let dx = explicit_length(style, "left").unwrap_or(0.0)
        - explicit_length(style, "right").unwrap_or(0.0);
    let dy = explicit_length(style, "top").unwrap_or(0.0)
        - explicit_length(style, "bottom").unwrap_or(0.0);

    if dx != 0.0 || dy != 0.0 {
        translate_layout_box(layout, dx, dy);
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
        PositionScheme::Relative => containing_block,
    };

    let left = explicit_length(style, "left");
    let right = explicit_length(style, "right");
    let top = explicit_length(style, "top");
    let bottom = explicit_length(style, "bottom");
    let static_outer = containing_block;
    let specified_width = resolved_length(style, "width", origin.width);
    let child_width = if specified_width.is_none() {
        shrink_to_fit_layout_width(child, resolver, origin.width)
    } else {
        origin.width
    };
    let child_containing = Rect {
        x: origin.x,
        y: origin.y,
        width: child_width,
        height: origin.height,
    };
    let mut layout_child = layout_node(child, resolver, child_containing, viewport, Some(parent_box))?;
    if specified_width.is_none() {
        let auto_width = auto_width_from_layout(&layout_child, child, resolver, origin.width);
        if (auto_width - layout_child.dimensions.content.width).abs() > 0.5 {
            let relayout_containing = Rect {
                width: auto_width,
                ..child_containing
            };
            layout_child =
                layout_node(child, resolver, relayout_containing, viewport, Some(parent_box))?;
        }
        layout_child.dimensions.content.width =
            auto_width_from_layout(&layout_child, child, resolver, origin.width);
    }
    let outer_width = layout_child.total_width();
    let outer_height = layout_child.total_height();
    let outer_x = if let Some(left) = left {
        origin.x + left
    } else if let Some(right) = right {
        origin.x + origin.width - outer_width - right
    } else {
        static_outer.x
    };
    let outer_y = if let Some(top) = top {
        origin.y + top
    } else if let Some(bottom) = bottom {
        origin.y + origin.height - outer_height - bottom
    } else {
        static_outer.y
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

fn is_table_container(style: &ComputedStyle) -> bool {
    matches!(table_display(style), Some(TableDisplay::Table))
}

fn is_table_container_element(node: &NodeHandle, style: &ComputedStyle) -> bool {
    if is_table_container(style) {
        return true;
    }
    // HTML default: <table> is display: table
    if matches!(style.get("display"), None) {
        if let Some(tag) = node.tag_name() {
            return tag.eq_ignore_ascii_case("table");
        }
    }
    false
}

fn table_display(style: &ComputedStyle) -> Option<TableDisplay> {
    match style.get("display") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("table") => {
            Some(TableDisplay::Table)
        }
        Some(ComputedValue::Keyword(keyword))
            if keyword.eq_ignore_ascii_case("table-row-group") =>
        {
            Some(TableDisplay::RowGroup)
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("table-row") => {
            Some(TableDisplay::Row)
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("table-cell") => {
            Some(TableDisplay::Cell)
        }
        _ => None,
    }
}

fn table_display_for_node(node: &NodeHandle, style: &ComputedStyle) -> Option<TableDisplay> {
    if let Some(display) = table_display(style) {
        return Some(display);
    }
    // HTML default display values for table elements
    if matches!(style.get("display"), None) {
        if let Some(tag) = node.tag_name() {
            return match tag.to_ascii_lowercase().as_str() {
                "table" => Some(TableDisplay::Table),
                "thead" | "tbody" | "tfoot" => Some(TableDisplay::RowGroup),
                "tr" => Some(TableDisplay::Row),
                "td" | "th" => Some(TableDisplay::Cell),
                _ => None,
            };
        }
    }
    None
}

fn table_border_spacing(style: &ComputedStyle) -> f32 {
    if matches!(
        style.get("border-collapse"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("collapse")
    ) {
        return 0.0;
    }

    explicit_length(style, "border-spacing").unwrap_or(0.0)
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
    translate_layout_contents(layout, dx, dy);
}

fn translate_layout_contents(layout: &mut LayoutBox, dx: f32, dy: f32) {
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
            if is_non_rendered_html_element(node) {
                return false;
            }
            let style = resolver.computed_style(node);
            if float_side(&style) != FloatSide::None || is_out_of_flow_positioned(&style) {
                return false;
            }
            if let Some(ComputedValue::Keyword(keyword)) = style.get("display") {
                return keyword.eq_ignore_ascii_case("inline")
                    || keyword.eq_ignore_ascii_case("inline-block");
            }
            node
                .tag_name()
                .map(|tag| {
                    matches!(
                        tag.as_str(),
                        "span" | "a" | "em" | "strong" | "b" | "i" | "img" | "object"
                    )
                })
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
    align: TextAlign,
) -> Vec<LineBox> {
    let mut segments = Vec::new();
    for node in nodes {
        collect_inline_segments(node, resolver, &mut segments);
    }

    layout_inline_segments(&segments, start_x, start_y, available_width, align)
}

#[derive(Debug, Clone)]
struct InlineSegment {
    node: NodeHandle,
    content: InlineSegmentContent,
    metrics: FontMetrics,
    line_height: f32,
    vertical_align: VerticalAlign,
}

#[derive(Debug, Clone)]
enum InlineSegmentContent {
    Text(String),
    Image(Image, ComputedStyle),
    GeneratedBox(ComputedStyle),
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
                        content: InlineSegmentContent::Text(text),
                        metrics: font_metrics(&parent_style),
                        line_height: line_height(&parent_style),
                        vertical_align: vertical_align(&parent_style),
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

            out.extend(generated_inline_segments(node, resolver, PseudoElement::Before));
            if let Some((image_node, image)) = element_inline_image(node) {
                let image_style = resolver.computed_style(&image_node);
                let padding = edge_sizes(&image_style, "padding");
                let border = edge_sizes(&image_style, "border");
                out.push(InlineSegment {
                    node: image_node,
                    content: InlineSegmentContent::Image(image.clone(), image_style.clone()),
                    metrics: font_metrics(&image_style),
                    line_height: line_height(&image_style).max(
                        image.height() as f32
                            + padding.top
                            + padding.bottom
                            + border.top
                            + border.bottom,
                    ),
                    vertical_align: vertical_align(&image_style),
                });
                out.extend(generated_inline_segments(node, resolver, PseudoElement::After));
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
                                    content: InlineSegmentContent::Text(text),
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
            out.extend(generated_inline_segments(node, resolver, PseudoElement::After));
        }
        _ => {}
    }
}

fn generated_inline_segments(
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

    match generated_content_value(content) {
        Some(GeneratedContent::Text(text)) => vec![InlineSegment {
            node: node.clone(),
            content: if text.is_empty() {
                InlineSegmentContent::GeneratedBox(style.clone())
            } else {
                InlineSegmentContent::Text(normalize_text(&text, white_space(&style)))
            },
            metrics,
            line_height,
            vertical_align,
        }],
        Some(GeneratedContent::Image(image)) => vec![InlineSegment {
            node: node.clone(),
            content: InlineSegmentContent::Image(image, style.clone()),
            metrics,
            line_height: line_height.max(metrics.font_size),
            vertical_align,
        }],
        None => Vec::new(),
    }
}

enum GeneratedContent {
    Text(String),
    Image(Image),
}

fn element_inline_image(node: &NodeHandle) -> Option<(NodeHandle, Image)> {
    let tag_name = node.tag_name()?;
    let attributes = node.attributes().unwrap_or_default();
    match tag_name.as_str() {
        "img" => {
            let src = attributes.get("src")?;
            let data_uri = parse_data_uri(src).ok()?;
            match data_uri {
                DataUri::Binary { mime_type, data } if mime_type.eq_ignore_ascii_case("image/png") => {
                    Image::decode_png(&data).ok().map(|image| (node.clone(), image))
                }
                _ => None,
            }
        }
        "object" => {
            if let Some(data) = attributes.get("data") {
                let data_uri = parse_data_uri(data).ok();
                if let Some(DataUri::Binary { mime_type, data }) = data_uri {
                    if mime_type.eq_ignore_ascii_case("image/png") {
                        if let Ok(image) = Image::decode_png(&data) {
                            return Some((node.clone(), image));
                        }
                    }
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

fn is_non_rendered_html_element(node: &NodeHandle) -> bool {
    matches!(
        node.tag_name().as_deref(),
        Some("head" | "title" | "meta" | "style" | "script" | "link")
    )
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
        WhiteSpaceMode::Normal => collapse_white_space(text),
        WhiteSpaceMode::Pre => text.to_string(),
    }
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

fn text_align(style: &ComputedStyle) -> TextAlign {
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

fn layout_inline_segments(
    segments: &[InlineSegment],
    start_x: f32,
    start_y: f32,
    available_width: f32,
    align: TextAlign,
) -> Vec<LineBox> {
    let mut lines = Vec::new();
    let mut current_fragments = Vec::new();
    let mut cursor_x = start_x;
    let mut cursor_y = start_y;
    let mut current_line_height: f32 = 0.0;

    for segment in segments {
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
                    current_line_height = 0.0;
                }
                InlinePiece::Fragment {
                    content,
                    width,
                    height,
                } => {
                    if cursor_x > start_x && cursor_x + width > start_x + available_width {
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
                        current_line_height = 0.0;
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
                    });
                    cursor_x += width;
                    current_line_height = current_line_height.max(segment.line_height.max(height));
                }
            }
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
        InlineSegmentContent::Text(text) => split_text_segment(text, segment.metrics, segment.line_height),
        InlineSegmentContent::Image(image, style) => {
            let padding = edge_sizes(style, "padding");
            let border = edge_sizes(style, "border");
            vec![InlinePiece::Fragment {
                content: InlineFragmentContent::Image(image.clone(), style.clone()),
                width: image.width() as f32 + padding.left + padding.right + border.left + border.right,
                height: image.height() as f32 + padding.top + padding.bottom + border.top + border.bottom,
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

fn split_text_segment(text: &str, metrics: FontMetrics, line_height: f32) -> Vec<InlinePiece> {
    if text.contains('\n') {
        let mut pieces = Vec::new();
        let line_count = text.split('\n').count();
        for (index, part) in text.split('\n').enumerate() {
            if !part.is_empty() {
                pieces.extend(
                    split_words_preserving_spaces(part)
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
        split_words_preserving_spaces(text)
            .into_iter()
            .map(|piece| InlinePiece::Fragment {
                width: measure_text_width(&piece, metrics),
                height: line_height,
                content: InlineFragmentContent::Text(piece),
            })
            .collect()
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

    fn find_layout_box_by_tag<'a>(layout: &'a LayoutBox, tag: &str) -> Option<&'a LayoutBox> {
        if layout.node.tag_name().as_deref() == Some(tag) {
            return Some(layout);
        }
        for child in &layout.children {
            if let Some(found) = find_layout_box_by_tag(child, tag) {
                return Some(found);
            }
        }
        None
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
    fn omits_non_rendered_head_elements() {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let head = NodeHandle::element("head");
        let title = NodeHandle::element("title");
        let meta = NodeHandle::element("meta");
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        let text = NodeHandle::text("visible");

        document.append_child(html.clone());
        html.append_child(head.clone());
        head.append_child(title.clone());
        head.append_child(meta);
        title.append_child(NodeHandle::text("hidden"));
        html.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(text);

        let mut resolver = StyleResolver::new();
        let layout = layout_tree(
            &document,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 0.0,
            },
        )
        .unwrap();

        let html_layout = layout.children.iter().find(|child| child.node == html).unwrap();
        assert_eq!(html_layout.children.len(), 1);
        assert_eq!(html_layout.children[0].node, body);
        assert!(find_layout_box_by_tag(&layout, "head").is_none());
        assert!(find_layout_box_by_tag(&layout, "title").is_none());
        assert!(find_layout_box_by_tag(&layout, "meta").is_none());
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
        assert_eq!(paragraph_box.lines[0].fragments[0].text(), Some("hello"));
        assert_eq!(paragraph_box.lines[1].fragments[0].text().map(str::trim), Some("world"));
        assert_eq!(paragraph_box.lines[2].fragments[0].text().map(str::trim), Some("again"));
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
            .filter_map(|fragment| fragment.text())
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
            .filter_map(|fragment| fragment.text())
            .collect::<String>();
        let second_line = paragraph_box.lines[1]
            .fragments
            .iter()
            .filter_map(|fragment| fragment.text())
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
        assert_eq!(paragraph_box.lines[0].fragments[0].text(), Some("inline"));
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
            .find(|fragment| fragment.text() == Some("lift"))
            .unwrap();

        assert!(raised_fragment.rect.y < base_fragment.rect.y);
    }

    #[test]
    fn inline_content_honors_text_align_right() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        paragraph.append_child(NodeHandle::text("hi"));
        document.append_child(body.clone());
        body.append_child(paragraph);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("p { text-align: right; font-size: 10px; }").unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 0.0,
            },
        )
        .unwrap();

        let line = &layout.children[0].lines[0];
        assert!(line.rect.x > 0.0);
        assert!(line.fragments[0].rect.x > 0.0);
    }

    #[test]
    fn generated_before_and_after_content_participate_in_inline_layout() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        let span = NodeHandle::element("span");
        span.append_child(NodeHandle::text("core"));
        document.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(span.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "span::before { content: \"pre \"; } \
                 span::after { content: \" post\"; }",
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
        let rendered = line
            .fragments
            .iter()
            .filter_map(|fragment| fragment.text())
            .collect::<String>();
        assert_eq!(rendered, "pre core post");
    }

    #[test]
    fn generated_empty_content_creates_a_zero_width_fragment() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        let span = NodeHandle::element("span");
        document.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(span.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("span::before { content: \"\"; }").unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 0.0,
            },
        )
        .unwrap();

        let line = &layout.children[0].lines[0];
        assert!(line
            .fragments
            .iter()
            .any(|fragment| matches!(fragment.content, InlineFragmentContent::GeneratedBox(_))));
    }

    #[test]
    fn generated_data_uri_png_content_creates_image_fragment() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        let span = NodeHandle::element("span");
        document.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(span.clone());

        let image_data = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADElEQVR4AQEFAPr/AP8AAP9zftimAAAAAElFTkSuQmCC";
        let stylesheet = format!(
            "span::before {{ content: url(\"data:image/png;base64,{image_data}\"); }}"
        );
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(&stylesheet).unwrap());

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 0.0,
            },
        )
        .unwrap();

        let line = &layout.children[0].lines[0];
        assert!(line.fragments.iter().any(|fragment| matches!(
            fragment.content,
            InlineFragmentContent::Image(_, _)
        )));
    }

    #[test]
    fn object_fallback_data_png_creates_image_fragment() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        let outer_object = NodeHandle::element("object");
        let inner_object = NodeHandle::element("object");
        document.append_child(body.clone());
        body.append_child(paragraph.clone());
        paragraph.append_child(outer_object.clone());
        outer_object.append_child(inner_object.clone());

        outer_object.set_attribute("data", "data:application/x-unknown,ERROR");
        inner_object.set_attribute(
            "data",
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADElEQVR4AQEFAPr/AP8AAP9zftimAAAAAElFTkSuQmCC",
        );

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet("object { display: inline; }").unwrap());

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 0.0,
            },
        )
        .unwrap();

        let line = &layout.children[0].lines[0];
        assert!(line.fragments.iter().any(|fragment| matches!(
            fragment.content,
            InlineFragmentContent::Image(_, _)
        )));
    }

    #[test]
    fn nested_object_fallback_with_vertical_align_bottom_stays_in_line_box() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let outer_object = NodeHandle::element("object");
        let middle_object = NodeHandle::element("object");
        let inner_object = NodeHandle::element("object");
        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(outer_object.clone());
        outer_object.append_child(middle_object.clone());
        middle_object.append_child(inner_object.clone());

        outer_object.set_attribute("data", "data:application/x-unknown,ERROR");
        middle_object.set_attribute("data", "data:application/x-unknown,ERROR");
        let image_data_uri = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAABnRSTlMAAAAAAABupgeRAAAABmJLR0QA%2FwD%2FAP%2BgvaeTAAAAEUlEQVR42mP4%2F58BCv7%2FZwAAHfAD%2FabwPj4AAAAASUVORK5CYII%3D";
        inner_object.set_attribute("data", image_data_uri);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { line-height: 16px; } object { display: inline; vertical-align: bottom; }",
            )
            .unwrap(),
        );

        let data_uri = parse_data_uri(image_data_uri).unwrap();
        let DataUri::Binary { data, .. } = data_uri else {
            panic!("expected binary data uri");
        };
        assert!(Image::decode_png(&data).is_ok(), "expected PNG payload to decode");
        assert!(
            element_inline_image(&outer_object).is_some(),
            "expected nested object fallback chain to resolve to a PNG image"
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

        let container_box = find_layout_box_by_tag(&layout, "div").unwrap();
        assert!(
            !container_box.lines.is_empty(),
            "expected nested object fallback to contribute an inline line box"
        );
        let line = &container_box.lines[0];
        let image_fragment = line
            .fragments
            .iter()
            .find(|fragment| matches!(fragment.content, InlineFragmentContent::Image(_, _)))
            .unwrap();

        assert!(image_fragment.rect.y >= line.rect.y);
        assert!(image_fragment.rect.y + image_fragment.rect.height <= line.rect.y + line.rect.height);
    }

    #[test]
    fn object_type_width_and_height_do_not_change_nested_inline_fallback_image_size() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let outer_object = NodeHandle::element("object");
        let middle_object = NodeHandle::element("object");
        let inner_object = NodeHandle::element("object");
        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(outer_object.clone());
        outer_object.append_child(middle_object.clone());
        middle_object.append_child(inner_object.clone());

        outer_object.set_attribute("data", "data:application/x-unknown,ERROR");
        middle_object.set_attribute("data", "data:application/x-unknown,ERROR");
        middle_object.set_attribute("type", "text/html");
        inner_object.set_attribute(
            "data",
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAABnRSTlMAAAAAAABupgeRAAAABmJLR0QA%2FwD%2FAP%2BgvaeTAAAAEUlEQVR42mP4%2F58BCv7%2FZwAAHfAD%2FabwPj4AAAAASUVORK5CYII%3D",
        );

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "object { display: inline; vertical-align: bottom; } \
                 object[type] { width: 90px; height: 30px; } \
                 object object object { padding-left: 11px; padding-right: 12px; border-right: 12px solid black; }",
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

        let container_box = find_layout_box_by_tag(&layout, "div").unwrap();
        let line = &container_box.lines[0];
        let image_fragment = line
            .fragments
            .iter()
            .find(|fragment| matches!(fragment.content, InlineFragmentContent::Image(_, _)))
            .unwrap();

        assert_eq!(image_fragment.rect.width, 37.0);
        assert_eq!(image_fragment.rect.height, 2.0);
    }

    #[test]
    fn lays_out_basic_table_rows_and_cells() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let table = NodeHandle::element("table");
        let tbody = NodeHandle::element("tbody");
        let row = NodeHandle::element("tr");
        let first = NodeHandle::element("td");
        let second = NodeHandle::element("td");

        first.append_child(NodeHandle::text("A"));
        second.append_child(NodeHandle::text("B"));
        document.append_child(body.clone());
        body.append_child(table.clone());
        table.append_child(tbody.clone());
        tbody.append_child(row.clone());
        row.append_child(first);
        row.append_child(second);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "table { display: table; width: 120px; border-spacing: 4px; } \
                 tbody { display: table-row-group; } \
                 tr { display: table-row; } \
                 td { display: table-cell; height: 10px; }",
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

        let table_box = &layout.children[0];
        let row_group_box = &table_box.children[0];
        let row_box = &row_group_box.children[0];
        assert_eq!(row_box.children.len(), 2);
        assert_eq!(row_box.children[0].dimensions.content.x, 4.0);
        assert_eq!(row_box.children[0].dimensions.content.width, 54.0);
        assert_eq!(row_box.children[1].dimensions.content.x, 62.0);
        assert_eq!(table_box.dimensions.content.width, 120.0);
    }

    #[test]
    fn aligns_table_cells_vertically_within_row_height() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let table = NodeHandle::element("table");
        let row = NodeHandle::element("tr");
        let tall = NodeHandle::element("td");
        let bottom = NodeHandle::element("td");

        tall.set_attribute("class", "tall");
        bottom.set_attribute("class", "bottom");
        bottom.append_child(NodeHandle::text("x"));
        document.append_child(body.clone());
        body.append_child(table.clone());
        table.append_child(row.clone());
        row.append_child(tall);
        row.append_child(bottom);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "table { display: table; width: 100px; } \
                 tr { display: table-row; } \
                 td { display: table-cell; height: 10px; vertical-align: top; font-size: 10px; line-height: 10px; } \
                 .tall { height: 30px; } \
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
                width: 120.0,
                height: 0.0,
            },
        )
        .unwrap();

        let row_box = &layout.children[0].children[0];
        let tall_box = &row_box.children[0];
        let bottom_box = &row_box.children[1];
        assert_eq!(row_box.dimensions.content.height, 30.0);
        assert_eq!(tall_box.dimensions.content.y, row_box.dimensions.content.y);
        assert_eq!(tall_box.dimensions.content.height, 30.0);
        assert_eq!(
            bottom_box.dimensions.content.y,
            row_box.dimensions.content.y
        );
        assert_eq!(bottom_box.dimensions.content.height, 30.0);
        assert_eq!(bottom_box.lines[0].rect.y, row_box.dimensions.content.y + 20.0);
    }

    #[test]
    fn creates_anonymous_rows_for_direct_table_cells() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let table = NodeHandle::element("table");
        let first = NodeHandle::element("td");
        let second = NodeHandle::element("td");

        document.append_child(body.clone());
        body.append_child(table.clone());
        table.append_child(first);
        table.append_child(second);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "table { display: table; width: 100px; } \
                 td { display: table-cell; height: 10px; }",
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

        let table_box = &layout.children[0];
        assert_eq!(table_box.children.len(), 1);
        let anonymous_row = &table_box.children[0];
        assert_eq!(anonymous_row.node.tag_name().as_deref(), Some("tr"));
        assert_eq!(anonymous_row.children.len(), 2);
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
    fn absolute_uses_nearest_positioned_ancestor_content_box() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let outer = NodeHandle::element("div");
        let middle = NodeHandle::element("section");
        let absolute = NodeHandle::element("aside");

        outer.set_attribute("class", "outer");
        middle.set_attribute("class", "middle");
        absolute.set_attribute("class", "absolute");
        document.append_child(body.clone());
        body.append_child(outer.clone());
        outer.append_child(middle.clone());
        middle.append_child(absolute);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".outer { position: relative; width: 200px; padding-left: 10px; padding-top: 5px; } \
                 .middle { width: 120px; padding-left: 7px; padding-top: 9px; } \
                 .absolute { position: absolute; left: 20px; top: 30px; width: 40px; height: 10px; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 240.0,
                height: 0.0,
            },
        )
        .unwrap();

        let absolute_box = find_layout_box_by_tag(&layout, "aside").unwrap();
        assert_eq!(absolute_box.dimensions.content.x, 30.0);
        assert_eq!(absolute_box.dimensions.content.y, 35.0);
    }

    #[test]
    fn absolute_auto_offsets_use_static_position() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let first = NodeHandle::element("section");
        let absolute = NodeHandle::element("aside");

        first.set_attribute("class", "first");
        absolute.set_attribute("class", "absolute");
        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(first);
        container.append_child(absolute.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { position: relative; width: 200px; } \
                 .first { height: 20px; } \
                 .absolute { position: absolute; width: 50px; height: 10px; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 240.0,
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
        assert_eq!(absolute_box.dimensions.content.x, 0.0);
        assert_eq!(absolute_box.dimensions.content.y, 20.0);
    }

    #[test]
    fn relative_position_offsets_visual_box_without_changing_flow_height() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let relative = NodeHandle::element("div");
        let sibling = NodeHandle::element("section");

        relative.set_attribute("class", "relative");
        sibling.set_attribute("class", "sibling");
        document.append_child(body.clone());
        body.append_child(relative.clone());
        body.append_child(sibling.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".relative { position: relative; top: 5px; left: 7px; width: 20px; height: 10px; } \
                 .sibling { width: 20px; height: 6px; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 80.0,
                height: 0.0,
            },
        )
        .unwrap();

        let relative_box = find_layout_box_by_tag(&layout, "div").unwrap();
        let sibling_box = find_layout_box_by_tag(&layout, "section").unwrap();
        assert_eq!(relative_box.dimensions.content.x, 7.0);
        assert_eq!(relative_box.dimensions.content.y, 5.0);
        assert_eq!(sibling_box.dimensions.content.y, 10.0);
    }

    #[test]
    fn absolute_auto_width_shrink_to_fit_text_content() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let absolute = NodeHandle::element("aside");
        absolute.set_attribute("class", "absolute");
        absolute.append_child(NodeHandle::text("hello"));
        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(absolute);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { position: relative; width: 200px; } \
                 .absolute { position: absolute; left: 0; top: 0; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 240.0,
                height: 0.0,
            },
        )
        .unwrap();

        let absolute_box = find_layout_box_by_tag(&layout, "aside").unwrap();
        assert!(absolute_box.dimensions.content.width < 200.0);
        assert!(absolute_box.dimensions.content.width > 0.0);
    }

    #[test]
    fn absolute_auto_width_includes_inline_image_padding_and_border() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let absolute = NodeHandle::element("aside");
        let object = NodeHandle::element("object");
        absolute.set_attribute("class", "absolute");
        object.set_attribute(
            "data",
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAABnRSTlMAAAAAAABupgeRAAAABmJLR0QA/wD/AP+gvaeTAAAAEUlEQVR42mP4/58BCv7/ZwAAHfAD/abwPj4AAAAASUVORK5CYII=",
        );
        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(absolute.clone());
        absolute.append_child(object);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { position: relative; width: 20px; } \
                 .absolute { position: absolute; left: 0; top: 0; } \
                 object { display: inline; padding: 1px 2px 1px 3px; border-right: 4px solid black; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 0.0,
            },
        )
        .unwrap();

        let absolute_box = find_layout_box_by_tag(&layout, "aside").unwrap();
        assert_eq!(absolute_box.dimensions.content.width, 11.0);
    }

    #[test]
    fn absolute_auto_width_relayouts_right_aligned_inline_content_after_expanding() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let absolute = NodeHandle::element("aside");
        let object = NodeHandle::element("object");
        let sibling = NodeHandle::element("section");

        absolute.set_attribute("class", "absolute");
        sibling.set_attribute("class", "sibling");
        object.set_attribute(
            "data",
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAABnRSTlMAAAAAAABupgeRAAAABmJLR0QA/wD/AP+gvaeTAAAAEUlEQVR42mP4/58BCv7/ZwAAHfAD/abwPj4AAAAASUVORK5CYII=",
        );
        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(absolute.clone());
        container.append_child(sibling);
        absolute.append_child(object);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "div { position: relative; width: 200px; } \
                 .absolute { position: absolute; left: 0; top: 0; text-align: right; } \
                 object { display: inline; padding-left: 3px; border-right: 4px solid black; } \
                 .sibling { width: 40px; height: 1px; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 240.0,
                height: 0.0,
            },
        )
        .unwrap();

        let absolute_box = find_layout_box_by_tag(&layout, "aside").unwrap();
        let line = absolute_box.lines.first().unwrap();
        let image = line
            .fragments
            .iter()
            .find(|fragment| matches!(fragment.content, InlineFragmentContent::Image(_, _)))
            .unwrap();

        assert_eq!(absolute_box.dimensions.content.width, 9.0);
        assert_eq!(line.rect.width, 9.0);
        assert_eq!(image.rect.x + image.rect.width, absolute_box.dimensions.content.x + 9.0);
    }

    #[test]
    fn percentage_height_in_auto_sized_container_becomes_auto() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let parent = NodeHandle::element("div");
        let child = NodeHandle::element("section");
        let grandchild = NodeHandle::element("p");

        child.set_attribute("class", "percent");
        grandchild.set_attribute("class", "content");
        document.append_child(body.clone());
        body.append_child(parent.clone());
        parent.append_child(child.clone());
        child.append_child(grandchild);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".percent { height: 50%; max-height: 18px; } \
                 .content { height: 40px; }",
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

        let child_box = find_layout_box_by_tag(&layout, "section").unwrap();
        assert_eq!(child_box.dimensions.content.height, 18.0);
    }

    #[test]
    fn percentage_width_resolves_for_positioned_elements() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let parent = NodeHandle::element("div");
        let child = NodeHandle::element("section");

        parent.set_attribute("class", "parent");
        child.set_attribute("class", "child");
        document.append_child(body.clone());
        body.append_child(parent.clone());
        parent.append_child(child.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".parent { position: relative; width: 200px; } \
                 .child { position: absolute; width: 50%; max-width: 80px; height: 10px; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 240.0,
                height: 0.0,
            },
        )
        .unwrap();

        let child_box = find_layout_box_by_tag(&layout, "section").unwrap();
        assert_eq!(child_box.dimensions.content.width, 80.0);
    }

    #[test]
    fn min_height_overrides_smaller_max_height() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let child = NodeHandle::element("div");
        child.set_attribute("class", "clamped");
        document.append_child(body.clone());
        body.append_child(child.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".clamped { height: 8px; min-height: 12px; max-height: 7px; width: 20px; }",
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
                height: 0.0,
            },
        )
        .unwrap();

        let child_box = find_layout_box_by_tag(&layout, "div").unwrap();
        assert_eq!(child_box.dimensions.content.height, 12.0);
    }

    #[test]
    fn min_width_overrides_smaller_max_width() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let child = NodeHandle::element("div");
        child.set_attribute("class", "clamped");
        document.append_child(body.clone());
        body.append_child(child.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".clamped { width: 20px; min-width: 32px; max-width: 24px; height: 10px; }",
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
                height: 0.0,
            },
        )
        .unwrap();

        let child_box = find_layout_box_by_tag(&layout, "div").unwrap();
        assert_eq!(child_box.dimensions.content.width, 32.0);
    }

    #[test]
    fn floated_inline_element_is_taken_out_of_inline_line_layout() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let floated = NodeHandle::element("span");
        floated.set_attribute("class", "floated");
        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(floated.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".floated { display: inline; float: right; width: 20px; height: 10px; }",
            )
            .unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 80.0,
                height: 0.0,
            },
        )
        .unwrap();

        let container_box = find_layout_box_by_tag(&layout, "div").unwrap();
        assert!(container_box.lines.is_empty());
        assert_eq!(container_box.children.len(), 1);
        assert_eq!(
            container_box.children[0].node.tag_name().as_deref(),
            Some("span")
        );
    }

    #[test]
    fn explicit_block_display_overrides_inline_tag_default() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let strong = NodeHandle::element("strong");
        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(strong.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("strong { display: block; width: 20px; height: 10px; }").unwrap(),
        );

        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 0.0,
            },
        )
        .unwrap();

        let container_box = find_layout_box_by_tag(&layout, "div").unwrap();
        assert!(container_box.lines.is_empty());
        assert_eq!(container_box.children.len(), 1);
        assert_eq!(container_box.children[0].node.tag_name().as_deref(), Some("strong"));
        assert_eq!(container_box.children[0].dimensions.content.width, 20.0);
    }

    #[test]
    fn float_left_and_right_reduce_available_block_width() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let left = NodeHandle::element("div");
        let right = NodeHandle::element("div");
        let block = NodeHandle::element("section");

        left.set_attribute("class", "left");
        right.set_attribute("class", "right");
        block.set_attribute("class", "block");
        document.append_child(body.clone());
        body.append_child(left);
        body.append_child(right);
        body.append_child(block.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".left { float: left; width: 20px; height: 10px; } \
                 .right { float: right; width: 30px; height: 10px; } \
                 .block { height: 5px; }",
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

        let left_box = find_layout_box_by_tag(&layout, "div").unwrap();
        let block_box = find_layout_box_by_tag(&layout, "section").unwrap();
        let right_box = layout
            .children
            .iter()
            .find(|child| child.node.attributes().and_then(|attrs| attrs.get("class").cloned()) == Some("right".to_string()))
            .unwrap();

        assert_eq!(left_box.dimensions.content.x, 0.0);
        assert_eq!(right_box.dimensions.content.x, 70.0);
        assert_eq!(block_box.dimensions.content.x, 20.0);
        assert_eq!(block_box.dimensions.content.width, 50.0);
    }

    #[test]
    fn clear_both_moves_block_below_floats() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let float = NodeHandle::element("div");
        let cleared = NodeHandle::element("section");

        float.set_attribute("class", "float");
        cleared.set_attribute("class", "cleared");
        document.append_child(body.clone());
        body.append_child(float);
        body.append_child(cleared.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".float { float: left; width: 20px; height: 10px; } \
                 .cleared { clear: both; height: 5px; }",
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

        let cleared_box = find_layout_box_by_tag(&layout, "section").unwrap();
        assert_eq!(cleared_box.dimensions.content.y, 10.0);
        assert_eq!(cleared_box.dimensions.content.x, 0.0);
    }

    #[test]
    fn clear_both_positions_border_edge_below_float_not_margin_edge() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let float = NodeHandle::element("div");
        let cleared = NodeHandle::element("section");

        float.set_attribute("class", "float");
        cleared.set_attribute("class", "cleared");
        document.append_child(body.clone());
        body.append_child(float);
        body.append_child(cleared.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".float { float: left; width: 20px; height: 10px; } \
                 .cleared { clear: both; margin-top: 5px; height: 5px; }",
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

        let cleared_box = find_layout_box_by_tag(&layout, "section").unwrap();
        assert_eq!(cleared_box.dimensions.content.y, 10.0);
    }

    #[test]
    fn float_preserves_negative_top_margin_offset() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let before = NodeHandle::element("div");
        let floated = NodeHandle::element("section");

        before.set_attribute("class", "before");
        floated.set_attribute("class", "floated");
        document.append_child(body.clone());
        body.append_child(before);
        body.append_child(floated.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".before { height: 40px; } \
                 .floated { float: left; width: 20px; height: 10px; margin-top: -12px; }",
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

        let floated_box = find_layout_box_by_tag(&layout, "section").unwrap();
        assert_eq!(floated_box.dimensions.content.y, 28.0);
    }

    #[test]
    fn empty_element_collapses_own_margins_through() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let before = NodeHandle::element("div");
        let empty = NodeHandle::element("div");
        let after = NodeHandle::element("section");

        before.set_attribute("class", "before");
        empty.set_attribute("class", "empty");
        after.set_attribute("class", "after");
        document.append_child(body.clone());
        body.append_child(before);
        body.append_child(empty);
        body.append_child(after.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".before { height: 10px; margin-bottom: 0; } \
                 .empty { margin-top: 20px; margin-bottom: 30px; } \
                 .after { height: 10px; margin-top: 0; }",
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
                height: 0.0,
            },
        )
        .unwrap();

        let after_box = find_layout_box_by_tag(&layout, "section").unwrap();
        // empty element's top (20) and bottom (30) collapse → max = 30
        // then 30 collapses with .before's mb (0) → 30
        // and with .after's mt (0) → 30
        assert_eq!(after_box.dimensions.content.y, 40.0); // 10 (before height) + 30 (collapsed margin)
    }

    #[test]
    fn empty_element_with_negative_child_margin_collapses_through() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let before = NodeHandle::element("div");
        let empty = NodeHandle::element("div");
        let inner = NodeHandle::element("div");
        let after = NodeHandle::element("section");

        before.set_attribute("class", "before");
        empty.set_attribute("class", "empty");
        inner.set_attribute("class", "inner");
        after.set_attribute("class", "after");
        document.append_child(body.clone());
        body.append_child(before);
        body.append_child(empty.clone());
        empty.append_child(inner);
        body.append_child(after.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".before { height: 10px; margin-bottom: 0; } \
                 .empty { margin: 20px 0; } \
                 .inner { margin-bottom: -15px; } \
                 .after { height: 10px; margin-top: 5px; }",
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
                height: 0.0,
            },
        )
        .unwrap();

        let after_box = find_layout_box_by_tag(&layout, "section").unwrap();
        // Collapse chain: .empty mt=20, .inner mt=0, .inner mb=-15, .empty mb=20, .after mt=5
        // Positive max: max(20, 0, 20, 5) = 20
        // Negative min: min(-15) = -15
        // Result: 20 + (-15) = 5
        assert_eq!(after_box.dimensions.content.y, 15.0); // 10 (before height) + 5 (collapsed)
    }

    #[test]
    fn first_in_flow_child_top_margin_collapses_with_parent_top() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let parent = NodeHandle::element("div");
        let child = NodeHandle::element("section");

        parent.set_attribute("class", "parent");
        child.set_attribute("class", "child");
        document.append_child(body.clone());
        body.append_child(parent.clone());
        parent.append_child(child.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".parent { width: 100px; } \
                 .child { margin-top: 12px; height: 10px; }",
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
                height: 0.0,
            },
        )
        .unwrap();

        let parent_box = find_layout_box_by_tag(&layout, "div").unwrap();
        let child_box = find_layout_box_by_tag(&layout, "section").unwrap();
        assert_eq!(child_box.dimensions.content.y, parent_box.dimensions.content.y);
        assert_eq!(parent_box.dimensions.content.height, 10.0);
    }

    #[test]
    fn whitespace_between_blocks_does_not_create_line_box() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let first = NodeHandle::element("div");
        let whitespace = NodeHandle::text("\n   ");
        let second = NodeHandle::element("section");

        first.set_attribute("class", "first");
        second.set_attribute("class", "second");
        document.append_child(body.clone());
        body.append_child(first);
        body.append_child(whitespace);
        body.append_child(second.clone());

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".first { height: 10px; margin-bottom: 5px; } \
                 .second { height: 10px; margin-top: 3px; }",
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
                height: 0.0,
            },
        )
        .unwrap();

        let second_box = find_layout_box_by_tag(&layout, "section").unwrap();
        // margin collapse: max(5, 3) = 5
        assert_eq!(second_box.dimensions.content.y, 15.0); // 10 + 5
        assert!(layout.lines.is_empty(), "whitespace should not create line boxes");
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
