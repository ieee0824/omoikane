//! Inline element regions shared by background paint and CSSOM geometry.
use super::*;
use std::collections::{HashMap, HashSet};

pub(super) fn edge(
    node: &NodeHandle,
    style: &ComputedStyle,
    start: bool,
    out: &mut Vec<InlineSegment>,
) {
    out.push(InlineSegment {
        node: node.clone(),
        content: InlineSegmentContent::InlineEdge(style.clone(), start),
        metrics: font_metrics(style),
        line_height: line_height(style),
        vertical_align: vertical_align(style),
        style: FragmentStyle::from_computed(style),
        word_break: word_break(style),
        overflow_wrap: overflow_wrap(style),
        white_space_mode: white_space(style),
    });
}

// Round the selected face's ascent and descent separately to the CSS pixel
// grid, as used for the content area of non-replaced inline boxes. Leading
// belongs to the line, not to the background or border of the inline element.
fn face_metrics(metrics: FontMetrics) -> (f32, f32) {
    LAYOUT_FONTS.with(|cell| {
        let mut fonts = cell.borrow_mut();
        let context = fonts.get_or_insert_with(|| super::super::LayoutFontContext {
            system_fonts: load_layout_fonts(),
            web_fonts: None,
            exact_metrics: false,
            metrics_cache: HashMap::new(),
        });
        let selected = select_text_font(
            "layout",
            metrics.font_family,
            crate::font::FontVariantKey::new(metrics.font_weight, metrics.font_style),
            context.web_fonts.as_deref(),
            &context.system_fonts,
        );
        selected
            .as_ref()
            .map(AsRef::as_ref)
            .or_else(|| context.system_fonts.first().map(AsRef::as_ref))
            .map_or((metrics.ascent, metrics.descent), |font| {
                let m = font.layout_metrics(metrics.font_size);
                (m.ascent.ceil(), m.descent.ceil())
            })
    })
}

struct Owner {
    node: NodeHandle,
    style: ComputedStyle,
    metrics: FontMetrics,
    ascent: f32,
    descent: f32,
    align: VerticalAlign,
}

pub(super) fn finish(
    lines: &mut [LineBox],
    nodes: &[NodeHandle],
    resolver: &mut StyleResolver,
    strut_height: f32,
) {
    // Plain text/replaced-only lines need no owner regions or face lookup.
    if !lines.iter().any(|line| {
        line.fragments
            .iter()
            .any(|fragment| matches!(fragment.content, InlineFragmentContent::InlineBox(_)))
    }) {
        return;
    }
    let mut face_cache = HashMap::new();
    let mut metrics_for = |m: FontMetrics| {
        *face_cache
            .entry((
                m.font_family,
                m.font_weight,
                m.font_style,
                m.font_size.to_bits(),
            ))
            .or_insert_with(|| face_metrics(m))
    };
    let parent_style = nodes
        .first()
        .and_then(NodeHandle::parent_node)
        .map(|node| resolver.computed_style(&node))
        .unwrap_or_default();
    let (strut_ascent, strut_descent) = metrics_for(font_metrics(&parent_style));
    let mut owners: HashMap<usize, Owner> = HashMap::new();
    let mut chains: HashMap<(usize, bool), Vec<usize>> = HashMap::new();
    let mut emitted_owners = HashSet::new();
    for line in lines {
        let empty = line.rect.height == 0.0;
        let mut baseline = (strut_height - strut_ascent - strut_descent) / 2.0 + strut_ascent;
        for fragment in &line.fragments {
            if matches!(fragment.content, InlineFragmentContent::InlineBox(_)) {
                continue;
            }
            if let InlineFragmentContent::Text(_) = &fragment.content {
                let (a, d) = metrics_for(fragment.metrics);
                baseline = baseline.max((fragment.rect.height - a - d) / 2.0 + a);
            }
        }
        baseline += line.rect.y;
        let last_ink = line.fragments.iter().rposition(|fragment| {
            !matches!(fragment.content, InlineFragmentContent::InlineBox(_))
                && !collapsible_space(fragment, resolver)
        });
        let mut regions: Vec<(usize, Rect)> = Vec::new();
        let mut indices = HashMap::new();
        for (index, fragment) in line.fragments.iter().enumerate() {
            if collapsible_space(fragment, resolver) && last_ink.is_none_or(|last| index > last) {
                continue;
            }
            let chain = chains
                .entry((
                    fragment.node.identity(),
                    matches!(fragment.content, InlineFragmentContent::InlineBox(_)),
                ))
                .or_insert_with(|| {
                    let mut chain = Vec::new();
                    let mut current =
                        if matches!(fragment.content, InlineFragmentContent::InlineBox(_)) {
                            Some(fragment.node.clone())
                        } else {
                            fragment.node.parent_node()
                        };
                    while let Some(node) = current {
                        if node.node_type() != NodeType::Element
                            || !super::super::is_inline_child(&node, resolver)
                        {
                            break;
                        }
                        let id = node.identity();
                        owners.entry(id).or_insert_with(|| {
                            let style = resolver.computed_style(&node);
                            let metrics = font_metrics(&style);
                            let (ascent, descent) = metrics_for(metrics);
                            Owner {
                                node: node.clone(),
                                align: vertical_align(&style),
                                style,
                                metrics,
                                ascent,
                                descent,
                            }
                        });
                        chain.push(id);
                        current = node.parent_node();
                    }
                    chain.reverse();
                    chain
                });
            for id in chain {
                let owner = &owners[id];
                let padding = edge_sizes(&owner.style, "padding");
                let border = edge_sizes(&owner.style, "border");
                let height = owner.ascent + owner.descent;
                let top = match owner.align {
                    VerticalAlign::Length(shift) => baseline - owner.ascent - shift,
                    VerticalAlign::Top => line.rect.y + (line_height(&owner.style) - height) / 2.0,
                    VerticalAlign::Bottom => {
                        line.rect.y + line.rect.height - (line_height(&owner.style) + height) / 2.0
                    }
                    VerticalAlign::Middle => line.rect.y + (line.rect.height - height) / 2.0,
                    VerticalAlign::Baseline => baseline - owner.ascent,
                };
                let rect = Rect {
                    x: fragment.rect.x,
                    y: if empty {
                        line.rect.y
                    } else {
                        top - padding.top - border.top
                    },
                    width: fragment.rect.width,
                    height: if empty {
                        0.0
                    } else {
                        height + padding.vertical() + border.vertical()
                    },
                };
                if let Some(&i) = indices.get(id) {
                    let (_, r): &mut (usize, Rect) = &mut regions[i];
                    let right = (r.x + r.width).max(rect.x + rect.width);
                    r.x = r.x.min(rect.x);
                    r.width = right - r.x;
                } else {
                    indices.insert(*id, regions.len());
                    regions.push((*id, rect));
                }
            }
        }
        // Parent backgrounds precede descendants, and every background precedes
        // text. Keep the text fragments themselves and their original DOM owners.
        let mut fragments = Vec::with_capacity(regions.len() + line.fragments.len());
        for (id, rect) in regions {
            // A hard break at the end of preserved text leaves only the
            // closing marker on a zero-height continuation. CSSOM does not
            // expose that as another client rect once this inline already had
            // a real fragment. Keep the first 0x0 region for a standalone
            // empty inline element.
            if rect.width == 0.0 && rect.height == 0.0 && emitted_owners.contains(&id) {
                continue;
            }
            let owner = &owners[&id];
            fragments.push(InlineFragment {
                node: owner.node.clone(),
                content: InlineFragmentContent::InlineBox(owner.style.clone()),
                rect,
                metrics: owner.metrics,
                vertical_align: owner.align,
                style: FragmentStyle::from_computed(&owner.style),
            });
            emitted_owners.insert(id);
        }
        fragments.extend(
            line.fragments
                .drain(..)
                .filter(|f| !matches!(f.content, InlineFragmentContent::InlineBox(_))),
        );
        line.fragments = fragments;
    }
}

fn collapsible_space(fragment: &InlineFragment, resolver: &mut StyleResolver) -> bool {
    let InlineFragmentContent::Text(text) = &fragment.content else {
        return false;
    };
    if !text
        .chars()
        .all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'))
    {
        return false;
    }
    let node = fragment
        .node
        .parent_node()
        .unwrap_or_else(|| fragment.node.clone());
    white_space(&resolver.computed_style(&node)).collapses_whitespace()
}
