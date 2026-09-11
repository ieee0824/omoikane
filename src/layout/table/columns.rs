use super::*;
use crate::css::ComputedValue;

pub(super) fn column_hints(
    table: &NodeHandle,
    resolver: &mut StyleResolver,
    basis: f32,
) -> Vec<Option<f32>> {
    fn append(
        node: &NodeHandle,
        resolver: &mut StyleResolver,
        basis: f32,
        inherited: Option<f32>,
        hints: &mut Vec<Option<f32>>,
    ) {
        let style = resolver.computed_style(node);
        let width = resolved_length(&style, "width", basis).or(inherited);
        let columns: Vec<_> = node
            .layout_child_nodes()
            .into_iter()
            .filter(|child| is_column(child, &resolver.computed_style(child)))
            .collect();
        if columns.is_empty() {
            // HTML limits column spans to 1000.
            let span = html_table_span_attribute(node, "span")
                .unwrap_or(1)
                .min(1000);
            hints.extend(std::iter::repeat_n(width, span));
        } else {
            for column in columns {
                append(&column, resolver, basis, width, hints);
            }
        }
    }
    let mut hints = Vec::new();
    for child in table.layout_child_nodes() {
        if is_column(&child, &resolver.computed_style(&child)) {
            append(&child, resolver, basis, None, &mut hints);
        }
    }
    hints
}

pub(super) fn is_column(node: &NodeHandle, style: &ComputedStyle) -> bool {
    matches!(style.get("display"), Some(ComputedValue::Keyword(value))
        if value.eq_ignore_ascii_case("table-column") || value.eq_ignore_ascii_case("table-column-group"))
        || (style.get("display").is_none()
            && matches!(node.tag_name().as_deref(), Some("col" | "colgroup")))
}

pub(super) fn fixed_column_widths(
    first_row: Option<&TableRowEntry>,
    resolver: &mut StyleResolver,
    mut hints: Vec<Option<f32>>,
    count: usize,
    width: f32,
    spacing: f32,
    collapsed: bool,
) -> Vec<f32> {
    hints.resize(count, None);
    if let Some(row) = first_row {
        let mut column = 0;
        for cell in &row.cells {
            if column >= count {
                break;
            }
            let span = html_table_span_attribute(cell, "colspan")
                .unwrap_or(1)
                .min(count - column);
            let style = resolver.computed_style(cell);
            if let Some(specified) = resolved_length(&style, "width", width) {
                let padding = super::super::edge_sizes(&style, "padding");
                let border = cell_border(&style, collapsed);
                let decorations = padding.horizontal() + border.horizontal();
                let border_box =
                    super::super::border_box_adjust_length(&style, specified, decorations, 0.0)
                        + decorations;
                let share = ((border_box - spacing * (span - 1) as f32) / span as f32).max(0.0);
                for hint in &mut hints[column..column + span] {
                    if hint.is_none() {
                        *hint = Some(share);
                    }
                }
            }
            column += span;
        }
    }
    let specified: f32 = hints.iter().flatten().sum();
    let missing = hints.iter().filter(|hint| hint.is_none()).count();
    let extra = (width - specified).max(0.0);
    if missing > 0 {
        hints
            .into_iter()
            .map(|hint| hint.unwrap_or(extra / missing as f32))
            .collect()
    } else if specified > 0.0 {
        // A fully specified grid shares surplus in proportion to its columns.
        hints
            .into_iter()
            .map(|hint| {
                let value = hint.unwrap_or(0.0);
                value + extra * value / specified
            })
            .collect()
    } else {
        vec![width / count as f32; count]
    }
}
