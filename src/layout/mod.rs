//! Block layout primitives.
//!
//! The layout phase consumes DOM nodes together with computed styles and
//! produces a tree of rectangular block boxes.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::css::{ComputedStyle, ComputedValue, PseudoElement, StyleResolver};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::font::{Font, load_default_text_fonts};
use crate::http::url::resolve_url;
use crate::http::{Client, Url};
use crate::paint::{DataUri, Image, parse_data_uri};

// Thread-local cache for fetched images and fonts to avoid redundant loads
thread_local! {
    static IMAGE_CACHE: RefCell<HashMap<String, Option<Image>>> = RefCell::new(HashMap::new());
    static HTTP_CLIENT: RefCell<Client> = RefCell::new(Client::new());
    static LAYOUT_FONTS: RefCell<Option<Vec<Font>>> = RefCell::new(None);
    static IMAGE_BASE_URL: RefCell<Option<Url>> = const { RefCell::new(None) };
}

/// Runs layout/image resolution with a temporary base URL used for relative image sources.
pub fn with_image_base_url<T>(base_url: Option<Url>, f: impl FnOnce() -> T) -> T {
    struct ImageBaseUrlGuard(Option<Url>);

    impl Drop for ImageBaseUrlGuard {
        fn drop(&mut self) {
            IMAGE_BASE_URL.with(|cell| {
                cell.replace(self.0.take());
            });
        }
    }

    IMAGE_BASE_URL.with(|cell| {
        let previous = cell.replace(base_url);
        let _guard = ImageBaseUrlGuard(previous);
        f()
    })
}

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
        NodeType::Document => layout_document(
            node,
            resolver,
            containing_block,
            viewport,
            positioned_ancestor,
        ),
        NodeType::Element => layout_element(
            node,
            resolver,
            containing_block,
            viewport,
            positioned_ancestor,
        ),
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

        if let Some(layout_child) = layout_node(
            &child,
            resolver,
            child_containing,
            viewport,
            positioned_ancestor,
        ) {
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
        if let Some(positioned) = layout_positioned_child(
            &child,
            resolver,
            &style,
            positioned_ancestor.unwrap_or(document_box),
            static_position,
            viewport,
        ) {
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
                        .map(|t| {
                            t.bytes()
                                .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0C'))
                        })
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
                    line_height(&style),
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
                            if resolved_length(child_style, "width", float_available_width)
                                .is_none()
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
        if let Some(layout_child) = layout_node(
            &child,
            resolver,
            child_containing,
            viewport,
            positioned_ancestor,
        ) {
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
                    .map(|t| {
                        t.bytes()
                            .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0C'))
                    })
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
                line_height(&style),
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
    let mut content_height =
        resolved_length(&style, "height", containing_block.height).unwrap_or(auto_height);
    let (min_height, max_height) =
        normalized_min_max_lengths(&style, "min-height", "max-height", containing_block.height);
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
    let main_gap = flex_main_axis_gap(&style, direction);
    let line_gap = flex_cross_axis_gap(&style, direction);

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
        let base_main_size = flex_basis(&child_style, direction)
            .or_else(|| explicit_main_size(&child_style, direction))
            .unwrap_or_else(|| auto_flex_base_main_size(&child, resolver, direction));
        items.push(FlexItemSpec {
            node: child,
            base_main_size,
            explicit_cross_size: explicit_cross_size(&child_style, direction),
            flex_grow: flex_grow(&child_style),
            flex_shrink: flex_shrink(&child_style),
            align_self: align_self(&child_style),
        });
    }

    let available_main_size = flex_available_main_size(&style, direction, width, &items, main_gap);
    let lines = build_flex_lines(&items, available_main_size, wrap, main_gap);
    let mut children = Vec::new();
    let mut cross_cursor = y;
    let line_count = lines.len();

    for (line_index, line) in lines.into_iter().enumerate() {
        let line_item_count = line.items.len();
        let fixed_main_gap = if line_item_count > 1 {
            main_gap * (line_item_count.saturating_sub(1)) as f32
        } else {
            0.0
        };
        let available_main_for_items = (available_main_size - fixed_main_gap).max(0.0);
        let resolved_main_sizes = resolve_flex_main_sizes(&line.items, available_main_for_items);
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
        let used_main_size = total_main_size + fixed_main_gap;
        let (line_start, justify_gap) =
            justify_offsets(justify, available_main_size, used_main_size, laid_out.len());

        let mut main_cursor = match direction {
            FlexDirection::Row => x + line_start,
            FlexDirection::Column => y + line_start,
        };

        let laid_out_count = laid_out.len();
        for (index, (item, mut child)) in laid_out.into_iter().enumerate() {
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

            if index + 1 < laid_out_count {
                main_cursor += child_main_size + main_gap + justify_gap;
            } else {
                main_cursor += child_main_size;
            }
        }

        cross_cursor += line_cross_size;
        if line_index + 1 < line_count {
            cross_cursor += line_gap;
        }
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
    let column_count = table_column_count(&entries);
    let column_width =
        ((width - spacing * (column_count as f32 + 1.0)).max(0.0)) / column_count as f32;
    let inner_width = (width - collapse_spacing).max(0.0);

    let mut children = Vec::new();
    let mut cursor_y = y + spacing;
    let mut pending_group: Option<(NodeHandle, Vec<LayoutBox>, f32, f32)> = None;
    let mut occupied_columns = vec![0usize; column_count];

    for entry in entries.drain(..) {
        for occupied in &mut occupied_columns {
            if *occupied > 0 {
                *occupied -= 1;
            }
        }

        let row_y = cursor_y;
        let (row_box, row_height) = layout_table_row_entry(
            &entry,
            resolver,
            x + spacing,
            row_y,
            column_count,
            &mut occupied_columns,
            column_width,
            spacing,
            viewport,
        )?;
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
        let group_box =
            build_row_group_box(group_node, rows, x + spacing, group_start_y, inner_width);
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
        .filter(|child| {
            matches!(
                table_display_for_node(child, &resolver.computed_style(child)),
                Some(TableDisplay::Cell)
            )
        })
        .collect()
}

fn layout_table_row_entry(
    entry: &TableRowEntry,
    resolver: &mut StyleResolver,
    x: f32,
    y: f32,
    column_count: usize,
    occupied_columns: &mut [usize],
    column_width: f32,
    spacing: f32,
    viewport: Rect,
) -> Option<(LayoutBox, f32)> {
    let mut measured = Vec::new();
    let mut row_height = 0.0f32;
    let mut column_cursor = 0usize;

    for cell in &entry.cells {
        while column_cursor < column_count && occupied_columns[column_cursor] > 0 {
            column_cursor += 1;
        }
        if column_cursor >= column_count {
            break;
        }

        let max_span = column_count.saturating_sub(column_cursor).max(1);
        let span = html_table_span_attribute(cell, "colspan")
            .unwrap_or(1)
            .max(1)
            .min(max_span);
        let rowspan = html_table_span_attribute(cell, "rowspan")
            .unwrap_or(1)
            .max(1);
        let cell_containing = Rect {
            x: 0.0,
            y: 0.0,
            width: column_width * span as f32 + spacing * span.saturating_sub(1) as f32,
            height: 0.0,
        };
        let mut layout_cell = layout_node(cell, resolver, cell_containing, viewport, None)?;
        let cell_style = resolver.computed_style(cell);
        let cell_height =
            explicit_length(&cell_style, "height").unwrap_or(layout_cell.total_height());
        layout_cell.dimensions.content.width =
            column_width * span as f32 + spacing * span.saturating_sub(1) as f32;
        layout_cell.dimensions.content.height = cell_height;
        row_height = row_height.max(layout_cell.total_height());
        measured.push((column_cursor, span, layout_cell, cell_style));
        if rowspan > 1 {
            for column in column_cursor..column_cursor.saturating_add(span) {
                if let Some(occupied) = occupied_columns.get_mut(column) {
                    *occupied = (*occupied).max(rowspan);
                }
            }
        }
        column_cursor = column_cursor.saturating_add(span);
    }

    let mut children = Vec::new();
    for (column_start, _span, mut cell, cell_style) in measured {
        let outer_x = x + column_start as f32 * (column_width + spacing);
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

    let used_columns = column_count;
    let row_width =
        used_columns as f32 * column_width + (used_columns.saturating_sub(1)) as f32 * spacing;
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

fn table_column_count(entries: &[TableRowEntry]) -> usize {
    let mut occupied_columns = Vec::<usize>::new();
    let mut max_columns = 0usize;

    for entry in entries {
        for occupied in &mut occupied_columns {
            if *occupied > 0 {
                *occupied -= 1;
            }
        }

        let mut column_cursor = 0usize;
        for cell in &entry.cells {
            while column_cursor < occupied_columns.len() && occupied_columns[column_cursor] > 0 {
                column_cursor += 1;
            }

            let span = html_table_span_attribute(cell, "colspan")
                .unwrap_or(1)
                .max(1);
            let rowspan = html_table_span_attribute(cell, "rowspan")
                .unwrap_or(1)
                .max(1);
            let end = column_cursor.saturating_add(span);
            if end > occupied_columns.len() {
                occupied_columns.resize(end, 0);
            }
            if rowspan > 1 {
                for occupied in &mut occupied_columns[column_cursor..end] {
                    *occupied = (*occupied).max(rowspan);
                }
            }
            column_cursor = end;
        }

        max_columns = max_columns.max(column_cursor.max(occupied_columns.len()));
    }

    max_columns.max(1)
}

fn html_table_span_attribute(node: &NodeHandle, name: &str) -> Option<usize> {
    node.attributes()
        .and_then(|attrs| attrs.get(name).cloned())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
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
    let margin_left_auto = margin_start_is_auto(style);
    let margin_right_auto = margin_end_is_auto(style);

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

    if margin_left_auto || margin_right_auto {
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
                offsets.left = offsets
                    .left
                    .max((region.outer.x + region.outer.width - x).max(0.0));
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
        top: explicit_length(
            style,
            &side_property
                .replace("{}", "top")
                .replace("{prefix}", prefix),
        )
        .or_else(|| explicit_length(style, &format!("{prefix}-top")))
        .unwrap_or(shorthand),
        right: explicit_length(
            style,
            &side_property
                .replace("{}", "right")
                .replace("{prefix}", prefix),
        )
        .or_else(|| explicit_length(style, &format!("{prefix}-right")))
        .or_else(|| logical_inline_end_length(style, prefix))
        .unwrap_or(shorthand),
        bottom: explicit_length(
            style,
            &side_property
                .replace("{}", "bottom")
                .replace("{prefix}", prefix),
        )
        .or_else(|| explicit_length(style, &format!("{prefix}-bottom")))
        .unwrap_or(shorthand),
        left: explicit_length(
            style,
            &side_property
                .replace("{}", "left")
                .replace("{prefix}", prefix),
        )
        .or_else(|| explicit_length(style, &format!("{prefix}-left")))
        .or_else(|| logical_inline_start_length(style, prefix))
        .unwrap_or(shorthand),
    }
}

fn logical_inline_start_length(style: &ComputedStyle, prefix: &str) -> Option<f32> {
    match prefix {
        "margin" => explicit_length(style, "margin-inline-start"),
        "padding" => explicit_length(style, "padding-inline-start"),
        _ => None,
    }
}

fn logical_inline_end_length(style: &ComputedStyle, prefix: &str) -> Option<f32> {
    match prefix {
        "margin" => explicit_length(style, "margin-inline-end"),
        "padding" => explicit_length(style, "padding-inline-end"),
        _ => None,
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

fn margin_start_is_auto(style: &ComputedStyle) -> bool {
    is_auto(style.get("margin-left")) || is_auto(style.get("margin-inline-start"))
}

fn margin_end_is_auto(style: &ComputedStyle) -> bool {
    is_auto(style.get("margin-right")) || is_auto(style.get("margin-inline-end"))
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
        && layout
            .children
            .iter()
            .all(|c| is_empty_for_margin_collapse(c))
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

fn shrink_to_fit_width(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    available_width: f32,
) -> f32 {
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
    if margin_start_is_auto(&style) {
        margin.left = 0.0;
    }
    if margin_end_is_auto(&style) {
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
                measure_text_width(
                    &normalize_text(&text, white_space(&parent_style)),
                    font_metrics(&parent_style),
                )
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
                let (rendered_width, _) =
                    resolve_image_rendered_size(&image_node, &image, &image_style);
                return rendered_width
                    + img_padding.left
                    + img_padding.right
                    + img_border.left
                    + img_border.right;
            }
            if is_flex_container(&style) {
                let direction = flex_direction(&style);
                let mut content_width = 0.0f32;
                for child in node.child_nodes() {
                    if child.node_type() != NodeType::Element {
                        continue;
                    }
                    let child_style = resolver.computed_style(&child);
                    if is_display_none(&child_style) {
                        continue;
                    }
                    let child_width = intrinsic_width(&child, resolver);
                    match direction {
                        FlexDirection::Row => content_width += child_width,
                        FlexDirection::Column => content_width = content_width.max(child_width),
                    }
                }
                return content_width + padding.horizontal() + border.horizontal();
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
                    .chain(generated_inline_segments(
                        node,
                        resolver,
                        PseudoElement::After,
                    ))
                    .map(|segment| match segment.content {
                        InlineSegmentContent::Text(text) => {
                            measure_text_width(&text, segment.metrics)
                        }
                        InlineSegmentContent::Image(_, style, rendered_width, _) => {
                            let padding = edge_sizes(&style, "padding");
                            let border = edge_sizes(&style, "border");
                            rendered_width
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
        apply_transform_offset(layout, style);
        return;
    }

    let dx = explicit_length(style, "left").unwrap_or(0.0)
        - explicit_length(style, "right").unwrap_or(0.0);
    let dy = explicit_length(style, "top").unwrap_or(0.0)
        - explicit_length(style, "bottom").unwrap_or(0.0);

    if dx != 0.0 || dy != 0.0 {
        translate_layout_box(layout, dx, dy);
    }
    apply_transform_offset(layout, style);
}

fn apply_transform_offset(layout: &mut LayoutBox, style: &ComputedStyle) {
    let (dx, dy) = transform_translate_offset(style);
    if dx != 0.0 || dy != 0.0 {
        translate_layout_box(layout, dx, dy);
    }
}

fn transform_translate_offset(style: &ComputedStyle) -> (f32, f32) {
    let value = match style.get("transform") {
        Some(ComputedValue::Keyword(keyword)) => keyword.as_str(),
        Some(ComputedValue::String(value)) => value.as_str(),
        _ => return (0.0, 0.0),
    };
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return (0.0, 0.0);
    }

    let mut dx = 0.0;
    let mut dy = 0.0;
    let mut cursor = 0usize;
    while cursor < value.len() {
        let tail = &value[cursor..];
        let Some(open_rel) = tail.find('(') else {
            break;
        };
        let name = tail[..open_rel].trim();
        let args_start = open_rel + 1;
        let Some(close_rel) = tail[args_start..].find(')') else {
            break;
        };
        let args = &tail[args_start..args_start + close_rel];
        let (x, y) = parse_transform_translate_function(name, args);
        dx += x;
        dy += y;
        cursor += args_start + close_rel + 1;
    }

    (dx, dy)
}

fn parse_transform_translate_function(name: &str, args: &str) -> (f32, f32) {
    let name = name.trim();
    let args = split_transform_args(args);
    if name.eq_ignore_ascii_case("translatex") {
        let dx = args
            .first()
            .and_then(|value| parse_transform_length(value))
            .unwrap_or(0.0);
        return (dx, 0.0);
    }
    if name.eq_ignore_ascii_case("translatey") {
        let dy = args
            .first()
            .and_then(|value| parse_transform_length(value))
            .unwrap_or(0.0);
        return (0.0, dy);
    }
    if name.eq_ignore_ascii_case("translate") || name.eq_ignore_ascii_case("translate3d") {
        let dx = args
            .first()
            .and_then(|value| parse_transform_length(value))
            .unwrap_or(0.0);
        let dy = args
            .get(1)
            .and_then(|value| parse_transform_length(value))
            .unwrap_or(0.0);
        return (dx, dy);
    }
    if name.eq_ignore_ascii_case("matrix") {
        let tx = args
            .get(4)
            .and_then(|value| parse_transform_length(value))
            .unwrap_or(0.0);
        let ty = args
            .get(5)
            .and_then(|value| parse_transform_length(value))
            .unwrap_or(0.0);
        return (tx, ty);
    }
    (0.0, 0.0)
}

fn split_transform_args(args: &str) -> Vec<&str> {
    let comma_separated = args
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if comma_separated.len() > 1 {
        return comma_separated;
    }
    args.split_whitespace()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect()
}

fn parse_transform_length(token: &str) -> Option<f32> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    if let Some(px) = token.strip_suffix("px") {
        return px.trim().parse::<f32>().ok();
    }
    token.parse::<f32>().ok()
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
    let mut layout_child = layout_node(
        child,
        resolver,
        child_containing,
        viewport,
        Some(parent_box),
    )?;
    if specified_width.is_none() {
        let auto_width = auto_width_from_layout(&layout_child, child, resolver, origin.width);
        if (auto_width - layout_child.dimensions.content.width).abs() > 0.5 {
            let relayout_containing = Rect {
                width: auto_width,
                ..child_containing
            };
            layout_child = layout_node(
                child,
                resolver,
                relayout_containing,
                viewport,
                Some(parent_box),
            )?;
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

fn flex_gap(style: &ComputedStyle) -> Option<f32> {
    explicit_length(style, "gap")
}

fn flex_main_axis_gap(style: &ComputedStyle, direction: FlexDirection) -> f32 {
    match direction {
        FlexDirection::Row => explicit_length(style, "column-gap")
            .or_else(|| flex_gap(style))
            .unwrap_or(0.0),
        FlexDirection::Column => explicit_length(style, "row-gap")
            .or_else(|| flex_gap(style))
            .unwrap_or(0.0),
    }
}

fn flex_cross_axis_gap(style: &ComputedStyle, direction: FlexDirection) -> f32 {
    match direction {
        FlexDirection::Row => explicit_length(style, "row-gap")
            .or_else(|| flex_gap(style))
            .unwrap_or(0.0),
        FlexDirection::Column => explicit_length(style, "column-gap")
            .or_else(|| flex_gap(style))
            .unwrap_or(0.0),
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

fn flex_available_main_size(
    style: &ComputedStyle,
    direction: FlexDirection,
    width: f32,
    items: &[FlexItemSpec],
    main_gap: f32,
) -> f32 {
    match direction {
        FlexDirection::Row => width,
        FlexDirection::Column => explicit_main_size(style, direction).unwrap_or_else(|| {
            let item_count = items.len();
            let gap_total = if item_count > 1 {
                main_gap * (item_count.saturating_sub(1)) as f32
            } else {
                0.0
            };
            items.iter().map(|item| item.base_main_size).sum::<f32>() + gap_total
        }),
    }
}

fn auto_flex_base_main_size(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    direction: FlexDirection,
) -> f32 {
    match direction {
        FlexDirection::Row => intrinsic_width(node, resolver),
        FlexDirection::Column => 0.0,
    }
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
    main_gap: f32,
) -> Vec<FlexLine<'a>> {
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut occupied = 0.0f32;

    for item in items {
        let item_size = item.base_main_size;
        let gap = if current.is_empty() { 0.0 } else { main_gap };
        let would_wrap = wrap == FlexWrap::Wrap
            && !current.is_empty()
            && occupied + gap + item_size > available_main_size;
        if would_wrap {
            lines.push(FlexLine { items: current });
            current = Vec::new();
            occupied = 0.0;
        }
        let leading_gap = if current.is_empty() { 0.0 } else { main_gap };
        occupied += leading_gap + item_size;
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
            node.tag_name()
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
    Image(Image, ComputedStyle, f32, f32),
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
                    line_height: line_height(&image_style).max(
                        rendered_height + padding.top + padding.bottom + border.top + border.bottom,
                    ),
                    vertical_align: vertical_align(&image_style),
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
            out.extend(generated_inline_segments(
                node,
                resolver,
                PseudoElement::After,
            ));
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
            content: InlineSegmentContent::Image(
                image.clone(),
                style.clone(),
                image.width() as f32,
                image.height() as f32,
            ),
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
            decode_or_fetch_image(src).map(|image| (node.clone(), image))
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

/// Decode an image from a data: URI (PNG or JPEG).
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
            } else {
                None
            }
        }
        _ => None,
    }
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
    if content_type.contains("image/png") {
        Image::decode_png(body).ok()
    } else if content_type.contains("image/jpeg") || content_type.contains("image/jpg") {
        Image::decode_jpeg(body).ok()
    } else {
        // Try PNG first, then JPEG
        Image::decode_png(body)
            .ok()
            .or_else(|| Image::decode_jpeg(body).ok())
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

fn image_alt_fallback_text(node: &NodeHandle, style: &ComputedStyle) -> Option<String> {
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

fn resolve_image_rendered_size(
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
    strut_line_height: f32,
) -> Vec<LineBox> {
    let mut lines = Vec::new();
    let mut current_fragments = Vec::new();
    let mut cursor_x = start_x;
    let mut cursor_y = start_y;
    let mut current_line_height: f32 = strut_line_height;

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
                    current_line_height = strut_line_height;
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
                        current_line_height = strut_line_height;
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
        InlineSegmentContent::Text(text) => {
            split_text_segment(text, segment.metrics, segment.line_height)
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

fn split_text_segment(text: &str, metrics: FontMetrics, line_height: f32) -> Vec<InlinePiece> {
    if text.contains('\n') {
        let mut pieces = Vec::new();
        let line_count = text.split('\n').count();
        for (index, part) in text.split('\n').enumerate() {
            if !part.is_empty() {
                pieces.extend(
                    split_words_preserving_spaces_cjk(part)
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
        split_words_preserving_spaces_cjk(text)
            .into_iter()
            .map(|piece| InlinePiece::Fragment {
                width: measure_text_width(&piece, metrics),
                height: line_height,
                content: InlineFragmentContent::Text(piece),
            })
            .collect()
    }
}

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
        '。' | '、' | '，' | '．' | '・' | '：' | '；' | '！' | '？' |
        // Closing brackets
        '）' | '」' | '』' | '】' | '〕' | '｝' | '］' |
        // Other
        'ー' | '～' | '…' | '‥' |
        // Small kana
        'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ' | 'っ' | 'ゃ' | 'ゅ' | 'ょ' | 'ゎ' |
        'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ' | 'ッ' | 'ャ' | 'ュ' | 'ョ' | 'ヮ'
    )
}

/// Characters that must not appear at the end of a line (line-end prohibited).
/// These are typically opening punctuation.
fn is_line_end_prohibited(ch: char) -> bool {
    matches!(
        ch,
        // Opening brackets
        '（' | '「' | '『' | '【' | '〔' | '｛' | '［'
    )
}

/// Split text into pieces that can be laid out, with CJK-aware breaking.
/// This allows line breaks between CJK characters while respecting kinsoku rules.
fn split_words_preserving_spaces_cjk(text: &str) -> Vec<String> {
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
    LAYOUT_FONTS.with(|cell| {
        let mut fonts_ref = cell.borrow_mut();
        if fonts_ref.is_none() {
            *fonts_ref = Some(load_layout_fonts());
        }

        if let Some(ref fonts) = *fonts_ref {
            if !fonts.is_empty() {
                return measure_text_width_with_fallback(text, metrics.font_size, fonts);
            }
        }

        // Fallback to approximation when no font is available
        text.chars().count() as f32 * metrics.average_advance
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
    if prefer_cjk && fonts.len() > 1 {
        for index in 1..fonts.len() {
            if !ch.is_whitespace() && !fonts[index].has_glyph(ch) {
                continue;
            }
            return index;
        }
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

fn is_cjk_preferred_character(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3000..=0x30FF // CJK Symbols/Punctuation, Hiragana, Katakana
            | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
            | 0xFF66..=0xFF9F // Half-width Katakana
    )
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
    for property in ["overflow", "overflow-x", "overflow-y"] {
        if matches!(
            style.get(property),
            Some(ComputedValue::Keyword(keyword)) if overflow_keyword_sets_hidden(keyword)
        ) {
            return Overflow::Hidden;
        }
    }
    Overflow::Visible
}

fn overflow_keyword_sets_hidden(keyword: &str) -> bool {
    keyword
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',')
        .any(|token| token.eq_ignore_ascii_case("hidden"))
}

#[cfg(test)]
mod tests;
