//! DOM provenance for normalized and case-transformed inline text.

use std::ops::Range;

use super::{InlineTextSource, WhiteSpaceMode, apply_text_transform_layout, white_space};
use crate::css::{ComputedStyle, ComputedValue};
use crate::dom::{Node, NodeHandle, NodeType};

/// Normalize text while retaining the original UTF-16 extent of each scalar.
pub(super) fn normalized_chars(text: &str, mode: WhiteSpaceMode) -> Vec<(char, Range<usize>)> {
    let mut result: Vec<(char, Range<usize>)> = Vec::new();
    let mut offset = 0;
    let mut previous_space = false;
    for ch in text.chars() {
        let source = offset..offset + ch.len_utf16();
        offset = source.end;
        if mode == WhiteSpaceMode::PreLine && ch == '\n' {
            if result.last().is_some_and(|(ch, _)| *ch == ' ') {
                result.pop();
            }
            result.push((ch, source));
            previous_space = true;
        } else if mode.collapses_whitespace()
            && if mode == WhiteSpaceMode::PreLine {
                ch.is_ascii_whitespace()
            } else {
                ch != '\u{00a0}' && ch.is_whitespace()
            }
        {
            if !previous_space {
                result.push((' ', source));
            } else if let Some((' ', range)) = result.last_mut() {
                range.end = source.end;
            }
            previous_space = true;
        } else {
            result.push((ch, source));
            previous_space = false;
        }
    }
    result
}

pub(super) fn prepare(
    node: &NodeHandle,
    original: &str,
    style: &ComputedStyle,
) -> (String, Vec<InlineTextSource>) {
    let chars = normalized_chars(original, white_space(style));
    let normalized = chars.iter().map(|(ch, _)| *ch).collect::<String>();
    // Whole-string lowercase retains context-sensitive final sigma behavior.
    let rendered = apply_text_transform_layout(&normalized, style);
    if node.node_type() != NodeType::Text {
        return (rendered, Vec::new());
    }
    let transform = match style.get("text-transform") {
        Some(ComputedValue::Keyword(keyword)) => keyword.to_ascii_lowercase(),
        _ => String::new(),
    };
    let mut output_chars = rendered.chars();
    let mut source = Vec::new();
    let mut byte = 0;
    let mut capitalize_next = true;
    for (ch, dom_range) in chars {
        let count = match transform.as_str() {
            "uppercase" => ch.to_uppercase().count(),
            "lowercase" => ch.to_lowercase().count(),
            "capitalize" if capitalize_next && !ch.is_whitespace() => ch.to_uppercase().count(),
            _ => 1,
        };
        capitalize_next = ch.is_whitespace();
        let start = byte;
        let mut utf16_len = 0;
        for transformed in output_chars.by_ref().take(count) {
            byte += transformed.len_utf8();
            utf16_len += transformed.len_utf16();
        }
        append(
            &mut source,
            [InlineTextSource {
                node: node.clone(),
                text_range: start..byte,
                direct: utf16_len == dom_range.len(),
                dom_range,
            }],
            0,
        );
    }
    debug_assert_eq!(byte, rendered.len());
    (rendered, source)
}

/// Append source spans, rebasing rendered bytes and coalescing direct mappings.
pub(super) fn append(
    target: &mut Vec<InlineTextSource>,
    spans: impl IntoIterator<Item = InlineTextSource>,
    byte_offset: usize,
) {
    for mut span in spans {
        span.text_range.start += byte_offset;
        span.text_range.end += byte_offset;
        if let Some(previous) = target.last_mut()
            && previous.direct
            && span.direct
            && previous.node == span.node
            && previous.text_range.end == span.text_range.start
            && previous.dom_range.end == span.dom_range.start
        {
            previous.text_range.end = span.text_range.end;
            previous.dom_range.end = span.dom_range.end;
        } else {
            target.push(span);
        }
    }
}

/// Slice rendered text without treating rendered byte indices as DOM offsets.
pub(super) fn slice(
    spans: &[InlineTextSource],
    text: &str,
    range: Range<usize>,
) -> Vec<InlineTextSource> {
    let first = spans.partition_point(|span| span.text_range.end <= range.start);
    spans[first..]
        .iter()
        .take_while(|span| span.text_range.start < range.end)
        .filter_map(|span| {
            let start = range.start.max(span.text_range.start);
            let end = range.end.min(span.text_range.end);
            if start >= end {
                return None;
            }
            let mut result = span.clone();
            if span.direct {
                result.dom_range.start += text[span.text_range.start..start].encode_utf16().count();
                result.dom_range.end =
                    result.dom_range.start + text[start..end].encode_utf16().count();
            }
            result.text_range = start - range.start..end - range.start;
            Some(result)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::{Origin, StyleResolver, parse_stylesheet};
    use crate::layout::{InlineFragment, LayoutBox, Rect, layout_tree};

    fn style(css: &str) -> ComputedStyle {
        let node = NodeHandle::element("p");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(&format!("p {{{css}}}")).unwrap(),
        );
        resolver.computed_style(&node)
    }

    fn texts(root: &LayoutBox) -> Vec<&InlineFragment> {
        root.lines
            .iter()
            .flat_map(|line| &line.fragments)
            .filter(|fragment| fragment.text().is_some_and(|text| !text.is_empty()))
            .chain(root.children.iter().flat_map(texts))
            .collect()
    }

    #[test]
    fn text_source_maps_unicode_and_case_expansion_to_original_utf16() {
        let node = NodeHandle::text("😀ßİΟΣ");
        let (rendered, source) = prepare(
            &node,
            &node.data().unwrap(),
            &style("text-transform: uppercase"),
        );
        assert_eq!(rendered, "😀SSİΟΣ");
        assert_eq!(source.len(), 3);
        assert_eq!(source[0].text_range, 0..4);
        assert_eq!(source[0].dom_range, 0..2);
        assert!(source[0].direct);
        assert_eq!(source[1].text_range, 4..6);
        assert_eq!(source[1].dom_range, 2..3);
        assert!(!source[1].direct);
        let (lower, source) = prepare(
            &node,
            &node.data().unwrap(),
            &style("text-transform: lowercase"),
        );
        assert_eq!(lower, "😀ßi\u{307}ος");
        let i = source.iter().find(|span| span.dom_range == (3..4)).unwrap();
        assert_eq!(&lower[i.text_range.clone()], "i\u{307}");
        assert!(!i.direct);
        let sigma = slice(&source, &lower, lower.len() - 'ς'.len_utf8()..lower.len());
        assert_eq!(sigma[0].dom_range, 5..6);
    }

    #[test]
    fn text_source_keeps_collapsed_and_discarded_whitespace_offsets() {
        let node = NodeHandle::text(" \t😀\t  x\n   y");
        let (rendered, source) =
            prepare(&node, &node.data().unwrap(), &style("white-space: normal"));
        assert_eq!(rendered, " 😀 x y");
        assert_eq!(source[0].dom_range, 0..2);
        assert!(!source[0].direct);
        let x = rendered.find('x').unwrap();
        assert_eq!(slice(&source, &rendered, x..x + 1)[0].dom_range, 7..8);
        let (rendered, source) = prepare(
            &node,
            &node.data().unwrap(),
            &style("white-space: pre-line"),
        );
        assert_eq!(rendered, " 😀 x\ny");
        let y = rendered.find('y').unwrap();
        assert_eq!(slice(&source, &rendered, y..y + 1)[0].dom_range, 12..13);
    }

    #[test]
    fn text_source_retains_adjacent_nodes_inside_one_wrapped_word() {
        let document = NodeHandle::document();
        let p = NodeHandle::element("p");
        let first = NodeHandle::text("word");
        let second = NodeHandle::text(".😀");
        document.append_child(p.clone());
        p.append_child(first.clone());
        p.append_child(NodeHandle::comment("no wrap opportunity"));
        p.append_child(second.clone());
        let mut resolver = StyleResolver::new();
        let layout = layout_tree(
            &document,
            &mut resolver,
            Rect {
                width: 200.,
                height: 200.,
                ..Rect::default()
            },
        )
        .unwrap();
        let fragments = texts(&layout);
        let word = fragments
            .iter()
            .find(|fragment| fragment.text() == Some("word.😀"))
            .unwrap();
        assert_eq!(word.text_source.len(), 2);
        assert_eq!(word.text_source[0].node, first);
        assert_eq!(word.text_source[0].dom_range, 0..4);
        assert_eq!(word.text_source[1].node, second);
        assert_eq!(word.text_source[1].text_range, 4..9);
        assert_eq!(word.text_source[1].dom_range, 0..3);
        let emoji = slice(&word.text_source, word.text().unwrap(), 5..9);
        assert_eq!(emoji[0].node, second);
        assert_eq!(emoji[0].dom_range, 1..3);
    }

    #[test]
    fn text_source_survives_emergency_breaks_and_print_prefix_skips() {
        use crate::layout::inline::{
            TextAlign, coalesce_adjacent_text_segments, layout_inline_segments, make_text_segment,
            skip_inline_prefix,
        };
        let first = NodeHandle::text("😀ab");
        let second = NodeHandle::text("c\u{301}d");
        let p = NodeHandle::element("p");
        p.append_child(first.clone());
        p.append_child(second.clone());
        let mut segments = vec![
            make_text_segment(first, "😀ab", &style("overflow-wrap: anywhere")).unwrap(),
            make_text_segment(
                second.clone(),
                "c\u{301}d",
                &style("overflow-wrap: anywhere"),
            )
            .unwrap(),
        ];
        coalesce_adjacent_text_segments(&mut segments);
        assert_eq!(segments.len(), 1);
        skip_inline_prefix(&mut segments, 3);
        let lines = layout_inline_segments(
            &segments,
            3,
            0.,
            0.,
            1.,
            TextAlign::Left,
            16.,
            None,
            false,
            None,
        );
        assert_eq!(lines.len(), 2);
        let first = &lines[0].fragments[0];
        assert_eq!(first.text(), Some("c\u{301}"));
        assert_eq!(first.text_source[0].node, second);
        assert_eq!(first.text_source[0].dom_range, 0..2);
        assert_eq!(first.source_end_token, 4);
        let last = &lines[1].fragments[0];
        assert_eq!(last.text_source[0].dom_range, 2..3);
        assert_eq!(last.source_end_token, 5);
    }

    #[test]
    fn text_source_excludes_generated_content_and_control_labels() {
        let element = NodeHandle::element("input");
        let (rendered, source) = prepare(&element, "label", &style("text-transform: uppercase"));
        assert_eq!(rendered, "LABEL");
        assert!(source.is_empty());
    }

    #[test]
    fn text_source_after_preserved_newline_keeps_original_offsets() {
        use crate::layout::inline::{TextAlign, layout_inline_segments, make_text_segment};
        let node = NodeHandle::text("😀\n  abc");
        let segment = make_text_segment(
            node.clone(),
            &node.data().unwrap(),
            &style("white-space: pre-line"),
        )
        .unwrap();
        let lines = layout_inline_segments(
            &[segment],
            0,
            0.,
            0.,
            500.,
            TextAlign::Left,
            16.,
            None,
            false,
            None,
        );
        assert_eq!(lines.len(), 2);
        let abc = lines[1]
            .fragments
            .iter()
            .find(|fragment| fragment.text() == Some("abc"))
            .unwrap();
        assert_eq!(abc.text_source[0].node, node);
        assert_eq!(abc.text_source[0].dom_range, 5..8);
    }

    #[test]
    fn text_source_ellipsis_keeps_only_the_visible_dom_extent() {
        use crate::html::TreeBuilder;
        for direction in ["ltr", "rtl"] {
            let document =
                TreeBuilder::parse("<html><body><p>ABCDEFGHIJKLMNOPQRSTUVWXYZ</p></body></html>")
                    .document();
            let node = document.query_selector("p").unwrap().child_nodes()[0].clone();
            let mut resolver = StyleResolver::new();
            resolver.add_stylesheet(Origin::Author, parse_stylesheet(&format!(
                "p {{ width: 90px; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; direction: {direction}; }}"
            )).unwrap());
            let layout = layout_tree(
                &document,
                &mut resolver,
                Rect {
                    width: 300.,
                    height: 200.,
                    ..Rect::default()
                },
            )
            .unwrap();
            let mut pending = vec![&layout];
            let mut checked = false;
            while let Some(root) = pending.pop() {
                pending.extend(&root.children);
                for paint in root
                    .lines
                    .iter()
                    .filter_map(|line| line.text_overflow.as_ref())
                {
                    for fragment in &paint.fragments {
                        let text = fragment.text().unwrap();
                        if text == "…" {
                            assert!(fragment.text_source.is_empty());
                        } else {
                            assert!(!text.is_empty());
                            let source = &fragment.text_source[0];
                            assert_eq!(source.node, node);
                            assert_eq!(&node.data().unwrap()[source.dom_range.clone()], text);
                            assert_eq!(source.text_range, 0..text.len());
                            assert!(text.len() < 26);
                            checked = true;
                        }
                    }
                }
            }
            assert!(checked, "expected cropped text in {direction} layout");
        }
    }
}
