//! CSS Scroll Snap geometry for one scroll container and one layout generation.

use crate::css::AffineTransform;
use crate::css::style::{ComputedStyle, ComputedValue, StyleResolver};
use crate::dom::{Node, NodeType};
use crate::layout::{LayoutBox, Rect, layout_box_style};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Selection {
    pub x: Option<usize>,
    pub y: Option<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct Geometry {
    port: Rect,
    max: (f32, f32),
    axis: (bool, bool),
    mandatory: bool,
    block_is_x: bool,
    x_start_is_min: bool,
    y_start_is_min: bool,
    areas: Vec<Area>,
}

#[derive(Clone, Debug)]
struct Area {
    node_id: usize,
    rect: Rect,
    block_align: Align,
    inline_align: Align,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Align {
    None,
    Start,
    Center,
    End,
}

impl Align {
    fn parse(value: &str) -> Self {
        match value {
            "start" => Self::Start,
            "center" => Self::Center,
            "end" => Self::End,
            _ => Self::None,
        }
    }
}

fn keyword<'a>(style: &'a ComputedStyle, property: &str) -> &'a str {
    match style.get(property) {
        Some(ComputedValue::Keyword(value)) => value,
        _ => "",
    }
}

fn length(style: &ComputedStyle, property: &str, basis: f32) -> f32 {
    style
        .get(property)
        .and_then(|value| value.resolve_length_percentage(basis))
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
}

fn physical_side_value(style: &ComputedStyle, prefix: &str, side: &str, basis: f32) -> f32 {
    let physical = length(style, &format!("{prefix}-{side}"), basis);
    let mode = keyword(style, "writing-mode");
    let vertical = mode.starts_with("vertical") || mode.starts_with("sideways");
    let rtl = keyword(style, "direction") == "rtl";
    let logical = match (vertical, rtl, mode, side) {
        (false, false, _, "left") | (false, true, _, "right") => "inline-start",
        (false, false, _, "right") | (false, true, _, "left") => "inline-end",
        (false, _, _, "top") => "block-start",
        (false, _, _, "bottom") => "block-end",
        (true, false, _, "top") | (true, true, _, "bottom") => "inline-start",
        (true, false, _, "bottom") | (true, true, _, "top") => "inline-end",
        (true, _, "vertical-rl", "right") | (true, _, "sideways-rl", "right") => "block-start",
        (true, _, "vertical-rl", "left") | (true, _, "sideways-rl", "left") => "block-end",
        (true, _, _, "left") => "block-start",
        (true, _, _, "right") => "block-end",
        _ => return physical,
    };
    let logical_value = length(style, &format!("{prefix}-{logical}"), basis);
    if logical_value != 0.0 {
        logical_value
    } else {
        physical
    }
}

fn transformed_rect(layout: &LayoutBox, transform: AffineTransform) -> Option<Rect> {
    let box_rect = layout.dimensions.border_box();
    let corners = [
        transform.transform_point(box_rect.x, box_rect.y),
        transform.transform_point(box_rect.x + box_rect.width, box_rect.y),
        transform.transform_point(box_rect.x, box_rect.y + box_rect.height),
        transform.transform_point(box_rect.x + box_rect.width, box_rect.y + box_rect.height),
    ];
    if corners
        .iter()
        .any(|(x, y)| !x.is_finite() || !y.is_finite())
    {
        return None;
    }
    let left = corners
        .iter()
        .map(|point| point.0)
        .fold(f32::INFINITY, f32::min);
    let right = corners
        .iter()
        .map(|point| point.0)
        .fold(f32::NEG_INFINITY, f32::max);
    let top = corners
        .iter()
        .map(|point| point.1)
        .fold(f32::INFINITY, f32::min);
    let bottom = corners
        .iter()
        .map(|point| point.1)
        .fold(f32::NEG_INFINITY, f32::max);
    Some(Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

fn collect_areas(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    areas: &mut Vec<Area>,
    viewport_root: bool,
    ancestor_transform: AffineTransform,
) {
    for child in &layout.children {
        let transform = ancestor_transform.multiply(child.transform);
        if child.node.node_type() == NodeType::Element && child.pseudo.is_none() {
            let style = layout_box_style(child, resolver);
            let alignment = keyword(&style, "scroll-snap-align");
            let mut parts = alignment.split_ascii_whitespace();
            let block_align = Align::parse(parts.next().unwrap_or("none"));
            let inline_align = Align::parse(parts.next().unwrap_or(alignment));
            if (block_align != Align::None || inline_align != Align::None)
                && let Some(mut rect) = transformed_rect(child, transform)
            {
                let top = physical_side_value(&style, "scroll-margin", "top", rect.height);
                let right = physical_side_value(&style, "scroll-margin", "right", rect.width);
                let bottom = physical_side_value(&style, "scroll-margin", "bottom", rect.height);
                let left = physical_side_value(&style, "scroll-margin", "left", rect.width);
                rect.x -= left;
                rect.y -= top;
                rect.width += left + right;
                rect.height += top + bottom;
                areas.push(Area {
                    node_id: child.node.identity(),
                    rect,
                    block_align,
                    inline_align,
                });
            }
        }
        // The root HTML element is the viewport's scrolling element, rather
        // than a nested scroll container. Other scrollers own their own areas.
        if child.is_scroll_container()
            && !(viewport_root && layout.node.node_type() == NodeType::Document)
        {
            continue;
        }
        collect_areas(child, resolver, areas, false, transform);
    }
}

impl Geometry {
    pub(super) fn for_element(layout: &LayoutBox, resolver: &mut StyleResolver) -> Option<Self> {
        if !layout.is_scroll_container() {
            return None;
        }
        let style = layout_box_style(layout, resolver);
        let content = layout.dimensions.content;
        let padding = layout.dimensions.padding;
        let port = Rect {
            x: content.x - padding.left,
            y: content.y - padding.top,
            width: content.width + padding.left + padding.right,
            height: content.height + padding.top + padding.bottom,
        };
        Self::build(
            layout,
            resolver,
            style,
            port,
            layout.max_scroll_offset(),
            false,
        )
    }

    pub(super) fn for_viewport(
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        viewport: Rect,
    ) -> Option<Self> {
        let root = layout
            .children
            .iter()
            .find(|child| child.node.node_type() == NodeType::Element)?;
        let style = layout_box_style(root, resolver);
        let metrics = super::compute_layout_metrics(layout);
        let max = (
            (metrics.scroll_width - viewport.width).max(0.0),
            (metrics.scroll_height - viewport.height).max(0.0),
        );
        Self::build(layout, resolver, style, viewport, max, true)
    }

    fn build(
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        style: ComputedStyle,
        mut port: Rect,
        max: (f32, f32),
        viewport_root: bool,
    ) -> Option<Self> {
        let snap_type = keyword(&style, "scroll-snap-type");
        let mut parts = snap_type.split_ascii_whitespace();
        let axis = parts.next().unwrap_or("none");
        if axis == "none" {
            return None;
        }
        let mandatory = parts.next() == Some("mandatory");
        let writing_mode = keyword(&style, "writing-mode");
        let block_is_x =
            writing_mode.starts_with("vertical") || writing_mode.starts_with("sideways");
        let rtl = keyword(&style, "direction") == "rtl";
        let enabled = match axis {
            "both" => (true, true),
            "x" => (true, false),
            "y" => (false, true),
            "block" => (block_is_x, !block_is_x),
            "inline" => (!block_is_x, block_is_x),
            _ => return None,
        };
        let left = physical_side_value(&style, "scroll-padding", "left", port.width).max(0.0);
        let right = physical_side_value(&style, "scroll-padding", "right", port.width).max(0.0);
        let top = physical_side_value(&style, "scroll-padding", "top", port.height).max(0.0);
        let bottom = physical_side_value(&style, "scroll-padding", "bottom", port.height).max(0.0);
        port.x += left;
        port.y += top;
        port.width = (port.width - left - right).max(0.0);
        port.height = (port.height - top - bottom).max(0.0);
        let mut areas = Vec::new();
        collect_areas(
            layout,
            resolver,
            &mut areas,
            viewport_root,
            AffineTransform::identity(),
        );
        Some(Self {
            port,
            max,
            axis: enabled,
            mandatory,
            block_is_x,
            x_start_is_min: if block_is_x {
                writing_mode != "vertical-rl" && writing_mode != "sideways-rl"
            } else {
                !rtl
            },
            y_start_is_min: if block_is_x { !rtl } else { true },
            areas,
        })
    }

    pub(super) fn choose(
        &self,
        destination: (f32, f32),
        preserve: Selection,
    ) -> ((f32, f32), Selection) {
        let (x, selected_x) = self.choose_axis(true, destination.0, preserve.x);
        let (y, selected_y) = self.choose_axis(false, destination.1, preserve.y);
        (
            (x, y),
            Selection {
                x: selected_x,
                y: selected_y,
            },
        )
    }

    fn choose_axis(
        &self,
        x_axis: bool,
        destination: f32,
        preserve: Option<usize>,
    ) -> (f32, Option<usize>) {
        let (enabled, max, port_start, port_size, start_is_min) = if x_axis {
            (
                self.axis.0,
                self.max.0,
                self.port.x,
                self.port.width,
                self.x_start_is_min,
            )
        } else {
            (
                self.axis.1,
                self.max.1,
                self.port.y,
                self.port.height,
                self.y_start_is_min,
            )
        };
        let destination = destination.clamp(0.0, max);
        if !enabled {
            return (destination, None);
        }
        let mut best: Option<(f32, usize, f32)> = None;
        for area in &self.areas {
            let align = if x_axis == self.block_is_x {
                area.block_align
            } else {
                area.inline_align
            };
            if align == Align::None {
                continue;
            }
            let (area_start, area_size) = if x_axis {
                (area.rect.x, area.rect.width)
            } else {
                (area.rect.y, area.rect.height)
            };
            let candidate = if area_size > port_size {
                destination.clamp(
                    area_start - port_start,
                    area_start + area_size - port_start - port_size,
                )
            } else {
                match align {
                    Align::Start if start_is_min => area_start - port_start,
                    Align::Start => area_start + area_size - port_start - port_size,
                    Align::End if start_is_min => area_start + area_size - port_start - port_size,
                    Align::End => area_start - port_start,
                    Align::Center => area_start + area_size / 2.0 - port_start - port_size / 2.0,
                    Align::None => continue,
                }
            }
            .clamp(0.0, max);
            let distance = (candidate - destination).abs();
            if preserve == Some(area.node_id) {
                return (candidate, Some(area.node_id));
            }
            if best.is_none_or(|(best_distance, _, _)| distance < best_distance) {
                best = Some((distance, area.node_id, candidate));
            }
        }
        let Some((distance, node_id, position)) = best else {
            return (destination, None);
        };
        // Proximity is intentionally UA-defined. Thirty percent of the
        // snapport gives nearby alignments a useful capture range while
        // leaving a large free-scroll region between distant targets.
        if !self.mandatory && distance > port_size * 0.3 {
            return (destination, None);
        }
        (position, Some(node_id))
    }
}
