//! Anonymous flex items retain the original text nodes; layout never inserts
//! wrappers into the DOM or applies element selectors to synthetic elements.

use super::*;
use crate::layout::inline::{TextAlign, layout_inline_nodes, line_height, text_align};

fn lines(
    nodes: &[NodeHandle],
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    width: f32,
    align: TextAlign,
) -> Vec<super::super::LineBox> {
    layout_inline_nodes(
        nodes,
        resolver,
        0.0,
        0.0,
        width,
        align,
        line_height(style),
        super::super::direction_is_rtl(style),
        None,
        0.0,
        Rect {
            x: 0.0,
            y: 0.0,
            width,
            height: 0.0,
        },
        None,
        false,
    )
    .lines
}

pub(super) fn append(
    items: &mut Vec<FlexItemSpec>,
    nodes: &mut Vec<NodeHandle>,
    resolver: &mut StyleResolver,
    direction: FlexDirection,
    align: AlignItems,
    width: f32,
) {
    let Some(nodes) = take_text(nodes) else {
        return;
    };
    let style = resolver.computed_style(&nodes[0]);
    let maximum = max_width(&nodes, resolver);
    let minimum = lines(&nodes, resolver, &style, 0.0, TextAlign::Left)
        .iter()
        .map(|line| line.rect.width)
        .fold(0.0, f32::max);
    let cross_size = (direction == FlexDirection::Column && align != AlignItems::Stretch)
        .then_some(maximum.min(width));
    let natural_height = if direction == FlexDirection::Column {
        lines(
            &nodes,
            resolver,
            &style,
            cross_size.unwrap_or(width),
            TextAlign::Left,
        )
        .last()
        .map_or(0.0, |line| line.rect.y + line.rect.height)
    } else {
        0.0
    };
    items.push(FlexItemSpec {
        node: nodes[0].clone(),
        text_nodes: nodes,
        base_main_size: if direction == FlexDirection::Row {
            maximum
        } else {
            natural_height
        },
        min_main_size: if direction == FlexDirection::Row {
            minimum
        } else {
            natural_height
        },
        explicit_cross_size: cross_size,
        flex_grow: 0.0,
        flex_shrink: 1.0,
        align_self: None,
    });
}

pub(super) fn layout(
    item: &FlexItemSpec,
    resolver: &mut StyleResolver,
    containing: Rect,
    used_height: Option<super::super::UsedHeight>,
) -> LayoutBox {
    let style = resolver.computed_style(&item.node);
    let lines = lines(
        &item.text_nodes,
        resolver,
        &style,
        containing.width,
        text_align(&style),
    );
    let height = used_height.map_or_else(
        || {
            lines
                .last()
                .map_or(0.0, |line| line.rect.y + line.rect.height)
        },
        |height| height.value,
    );
    LayoutBox {
        node: item.node.clone(),
        dimensions: BoxDimensions {
            content: Rect {
                x: 0.0,
                y: 0.0,
                width: containing.width,
                height,
            },
            ..BoxDimensions::default()
        },
        visibility: visibility(&style),
        overflow: overflow(&style),
        z_index: z_index(&style),
        transform: AffineTransform::identity(),
        needs_scroll_translation: false,
        paint_scroll: None,
        lines,
        children: Vec::new(),
        marker: None,
    }
}

// Whitespace-only runs do not generate anonymous items, including in intrinsic
// sizing. Comments and display:none nodes do not split a contiguous text run.
pub(super) fn take_text(nodes: &mut Vec<NodeHandle>) -> Option<Vec<NodeHandle>> {
    let nodes = std::mem::take(nodes);
    (!nodes.is_empty()
        && !nodes.iter().all(|node| {
            node.data().is_none_or(|text| {
                text.chars()
                    .all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'))
            })
        }))
    .then_some(nodes)
}

pub(super) fn max_width(nodes: &[NodeHandle], resolver: &mut StyleResolver) -> f32 {
    let style = resolver.computed_style(&nodes[0]);
    lines(nodes, resolver, &style, f32::MAX, TextAlign::Left)
        .iter()
        .map(|line| line.rect.width)
        .fold(0.0, f32::max)
}
