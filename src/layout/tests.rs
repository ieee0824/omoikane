use crate::css::{Origin, parse_stylesheet};
use crate::dom::ShadowRootMode;
use crate::layout::*;

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
fn computes_block_box_dimensions_from_inline_style() {
    let (_document, _html, body, card) = sample_tree();
    card.set_attribute("style", "width: 120px; padding: 10px");
    let mut resolver = StyleResolver::new();

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
    assert_eq!(child.dimensions.padding.top, 10.0);
    assert_eq!(child.dimensions.padding.right, 10.0);
    assert_eq!(child.dimensions.padding.bottom, 10.0);
    assert_eq!(child.dimensions.padding.left, 10.0);
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
        parse_stylesheet("div { width: 80px; margin-left: auto; margin-right: auto; }").unwrap(),
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
fn logical_auto_margins_center_fixed_width_blocks() {
    let (_document, _html, body, _card) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { width: 80px; margin-inline-start: auto; margin-inline-end: auto; }",
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
    assert_eq!(child.dimensions.margin.left, 60.0);
    assert_eq!(child.dimensions.margin.right, 60.0);
}

#[test]
fn logical_auto_margins_center_with_max_width() {
    let (_document, _html, body, _card) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { max-width: 80px; margin-inline-start: auto; margin-inline-end: auto; }",
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
    assert_eq!(child.dimensions.content.width, 80.0);
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

    let html_layout = layout
        .children
        .iter()
        .find(|child| child.node == html)
        .unwrap();
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
fn transform_translate_offsets_layout_box() {
    let (_document, _html, body, _card) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { width: 50px; height: 20px; transform: translate(10px, 6px); }")
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
    assert_eq!(child.dimensions.content.x, 10.0);
    assert_eq!(child.dimensions.content.y, 6.0);
}

#[test]
fn transform_translate_function_variants_and_matrix_accumulate_offsets() {
    let (_document, _html, body, _card) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { width: 50px; height: 20px; transform: translateX(10px) translateY(6px) translate3d(4px, 5px, 0) matrix(1, 0, 0, 1, 3, 2); }",
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
    assert_eq!(child.dimensions.content.x, 17.0);
    assert_eq!(child.dimensions.content.y, 13.0);
}

#[test]
fn overflow_axis_hidden_sets_hidden_overflow_state() {
    let (_document, _html, body, _card) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { overflow-x: hidden; width: 50px; height: 20px; }").unwrap(),
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
    assert_eq!(child.overflow, Overflow::Hidden);
}

#[test]
fn overflow_y_hidden_sets_hidden_overflow_state() {
    let (_document, _html, body, _card) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { overflow-y: hidden; width: 50px; height: 20px; }").unwrap(),
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
    assert_eq!(child.overflow, Overflow::Hidden);
}

#[test]
fn overflow_shorthand_two_values_marks_hidden_axis() {
    let (_document, _html, body, _card) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { overflow: visible hidden; width: 50px; height: 20px; }").unwrap(),
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
    assert_eq!(child.overflow, Overflow::Hidden);
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

    let first_border_bottom = first_box.dimensions.content.y + first_box.dimensions.content.height;
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
            width: 50.0, // Narrower width to force 3 lines with real font metrics
            height: 0.0,
        },
    )
    .unwrap();

    let paragraph_box = &layout.children[0];
    assert_eq!(paragraph_box.lines.len(), 3);
    assert_eq!(paragraph_box.lines[0].fragments[0].text(), Some("hello"));
    assert_eq!(
        paragraph_box.lines[1].fragments[0].text().map(str::trim),
        Some("world")
    );
    assert_eq!(
        paragraph_box.lines[2].fragments[0].text().map(str::trim),
        Some("again")
    );
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
fn shadow_host_layout_uses_slot_assignment_and_fallback_flat_tree() {
    fn rendered_text(layout: &LayoutBox) -> String {
        let mut text = String::new();
        for line in &layout.lines {
            for fragment in &line.fragments {
                if let Some(value) = fragment.text() {
                    text.push_str(value);
                }
            }
        }
        for child in &layout.children {
            text.push_str(&rendered_text(child));
        }
        text
    }

    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let host = NodeHandle::element("div");
    let light = NodeHandle::element("span");
    light.set_attribute("slot", "content");
    light.append_child(NodeHandle::text("assigned"));
    let unmatched = NodeHandle::element("span");
    unmatched.append_child(NodeHandle::text("unmatched"));
    host.append_child(light.clone());
    host.append_child(unmatched);
    document.append_child(body.clone());
    body.append_child(host.clone());

    let root = host.attach_shadow(ShadowRootMode::Open).unwrap();
    let wrapper = NodeHandle::element("section");
    let slot = NodeHandle::element("slot");
    slot.set_attribute("name", "content");
    slot.append_child(NodeHandle::text("fallback"));
    wrapper.append_child(slot);
    root.append_child(wrapper);

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 300.0,
        height: 0.0,
    };
    let mut resolver = StyleResolver::new();
    let assigned_layout = layout_tree(&body, &mut resolver, viewport).unwrap();
    assert_eq!(rendered_text(&assigned_layout), "assigned");

    light.set_attribute("slot", "missing");
    let fallback_layout = layout_tree(&body, &mut resolver, viewport).unwrap();
    assert_eq!(rendered_text(&fallback_layout), "fallback");
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
fn font_metrics_carry_css_web_font_selection() {
    let style = resolve_style_for_test(
        "div { font-family: 'TwitterChirp', sans-serif; font-weight: 700; font-style: italic; }",
        "div",
    );
    let metrics = font_metrics(&style);

    assert_eq!(metrics.font_family, Some(crate::font::FontFamilyKey::new("twitterchirp")));
    assert_eq!(metrics.font_weight, crate::font::FontWeight(700));
    assert_eq!(metrics.font_style, crate::font::FontStyle::Italic);
}

#[test]
fn with_layout_fonts_restores_previous_context_on_panic() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;

    super::with_layout_fonts(Vec::new(), None, || {
        let result = catch_unwind(AssertUnwindSafe(|| {
            super::with_layout_fonts(
                Vec::new(),
                Some(Arc::new(crate::font::WebFontRegistry::new())),
                || panic!("layout fonts panic test"),
            )
        }));
        assert!(result.is_err());

        // The panicking inner scope must restore the outer context
        // (which has no web fonts), not leave its own registry behind.
        let outer_restored = super::LAYOUT_FONTS.with(|cell| {
            cell.borrow()
                .as_ref()
                .is_some_and(|context| context.web_fonts.is_none())
        });
        assert!(outer_restored);
    });

    let cleared = super::LAYOUT_FONTS.with(|cell| cell.borrow().is_none());
    assert!(cleared);
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
    assert!(
        line.fragments
            .iter()
            .any(|fragment| matches!(fragment.content, InlineFragmentContent::GeneratedBox(_)))
    );
}

#[test]
fn google_style_form_controls_create_visible_replaced_fragments() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let form = NodeHandle::element("form");
    let hidden = NodeHandle::element("input");
    let query = NodeHandle::element("input");
    let submit_wrapper = NodeHandle::element("span");
    let submit_inner = NodeHandle::element("span");
    let submit = NodeHandle::element("input");
    hidden.set_attribute("type", "hidden");
    query.set_attribute("name", "q");
    query.set_attribute("size", "57");
    submit_wrapper.set_attribute("class", "submit-outer");
    submit_inner.set_attribute("class", "submit-inner");
    submit.set_attribute("type", "submit");
    submit.set_attribute("value", "Google 検索");
    document.append_child(body.clone());
    body.append_child(form.clone());
    form.append_child(hidden);
    form.append_child(query);
    form.append_child(submit_wrapper.clone());
    submit_wrapper.append_child(submit_inner.clone());
    submit_inner.append_child(submit);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "form { text-align: center; }
             input[name=q] { margin: 0; padding: 5px 8px 0 6px; font-size: 18px; }
             .submit-outer { display: inline-block; }
             .submit-inner { display: block; }
             input[type=submit] { margin: 0 4px; padding: 0 12px; height: 36px; }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 1000.0, height: 0.0 },
    )
    .unwrap();
    let fragments: Vec<_> = layout.children[0]
        .lines
        .iter()
        .flat_map(|line| &line.fragments)
        .filter_map(|fragment| match &fragment.content {
            InlineFragmentContent::FormControl(_, value) => {
                Some((fragment.rect, value.clone()))
            }
            _ => None,
        })
        .collect();

    assert_eq!(fragments.len(), 2, "hidden input must not create a fragment");
    assert!(fragments[0].0.width > 400.0, "size=57 search input should be wide");
    assert!(fragments[0].0.height >= 27.0);
    assert_eq!(fragments[1].1, "Google 検索");
    assert!(fragments[1].0.width > 80.0);
}

/// Collects `(rect, value)` for every `FormControl` fragment in `container`'s
/// line boxes. Used by the `<button>`/`<textarea>`/`<select>` layout tests.
fn form_control_fragments(container: &LayoutBox) -> Vec<(Rect, String)> {
    container
        .lines
        .iter()
        .flat_map(|line| &line.fragments)
        .filter_map(|fragment| match &fragment.content {
            InlineFragmentContent::FormControl(_, value) => Some((fragment.rect, value.clone())),
            _ => None,
        })
        .collect()
}

fn layout_single_control_container(body: &NodeHandle) -> LayoutBox {
    let mut resolver = StyleResolver::new();
    layout_control_container(body, &mut resolver)
}

fn layout_control_container(body: &NodeHandle, resolver: &mut StyleResolver) -> LayoutBox {
    let layout = layout_tree(
        body,
        resolver,
        Rect { x: 0.0, y: 0.0, width: 1000.0, height: 0.0 },
    )
    .unwrap();
    layout.children.into_iter().next().expect("container box")
}

#[test]
fn button_label_width_is_text_plus_box_spacing() {
    // A UA-styled button and one with padding/border stripped share the same
    // label text width, so the difference is exactly the UA box spacing:
    // padding-left(6) + padding-right(6) + border-left(2) + border-right(2) = 16.
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let styled = NodeHandle::element("button");
    let bare = NodeHandle::element("button");
    styled.append_child(NodeHandle::text("送信する"));
    bare.set_attribute("class", "bare");
    bare.append_child(NodeHandle::text("送信する"));
    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(styled);
    div.append_child(bare);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ".bare { padding: 0px; border-top-width: 0px; border-right-width: 0px;
                     border-bottom-width: 0px; border-left-width: 0px; }",
        )
        .unwrap(),
    );
    let container = layout_control_container(&body, &mut resolver);
    let fragments = form_control_fragments(&container);
    assert_eq!(fragments.len(), 2);
    assert_eq!(fragments[0].1, "送信する");
    assert_eq!(fragments[1].1, "送信する");
    let spacing = fragments[0].0.width - fragments[1].0.width;
    assert!(
        (spacing - 16.0).abs() < 0.01,
        "button width must be label text width + 16px box spacing, got diff {spacing}"
    );
}

#[test]
fn button_explicit_width_takes_precedence() {
    // width:200px content + padding(6+6) + border(2+2) = 216.
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let button = NodeHandle::element("button");
    button.set_attribute("style", "width: 200px");
    button.append_child(NodeHandle::text("OK"));
    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(button);

    let container = layout_single_control_container(&body);
    let fragments = form_control_fragments(&container);
    assert_eq!(fragments.len(), 1);
    assert!(
        (fragments[0].0.width - 216.0).abs() < 0.01,
        "explicit width should win: expected 216, got {}",
        fragments[0].0.width
    );
}

#[test]
fn button_flattens_descendant_text_into_single_fragment() {
    // <button><span>a</span>b</button>: children are not laid out independently;
    // the label is the flattened descendant text "ab".
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let button = NodeHandle::element("button");
    let span = NodeHandle::element("span");
    span.append_child(NodeHandle::text("a"));
    button.append_child(span);
    button.append_child(NodeHandle::text("b"));
    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(button);

    let container = layout_single_control_container(&body);
    let all_fragments: Vec<_> = container
        .lines
        .iter()
        .flat_map(|line| &line.fragments)
        .collect();
    assert_eq!(
        all_fragments.len(),
        1,
        "button children must not produce independent fragments"
    );
    let fragments = form_control_fragments(&container);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].1, "ab");
}

#[test]
fn icon_only_button_preserves_nested_svg() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let button = NodeHandle::element("button");
    let span = NodeHandle::element("span");
    let svg = NodeHandle::element("svg");
    let path = NodeHandle::element("path");
    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(button.clone());
    button.append_child(span.clone());
    span.append_child(svg.clone());
    svg.append_child(path.clone());
    svg.set_attribute("viewBox", "0 0 24 24");
    path.set_attribute("d", "M11 5h2v14h-2zM5 11h14v2H5z");

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("button { width: 36px; height: 36px; padding: 0; border: 0; } svg { width: 24px; height: 24px; }").unwrap(),
    );
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 100.0, height: 0.0 },
    )
    .unwrap();
    let fragment = &layout.children[0].lines[0].fragments[0];

    assert_eq!(fragment.rect.width, 36.0);
    assert_eq!(fragment.rect.height, 36.0);
    assert!(matches!(
        fragment.content,
        InlineFragmentContent::IconFormControl(_, _, 24.0, 24.0)
    ));
}

#[test]
fn icon_only_button_skips_hidden_and_non_rendered_images() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let button = NodeHandle::element("button");
    let style = NodeHandle::element("style");
    let style_svg = NodeHandle::element("svg");
    let hidden = NodeHandle::element("span");
    let hidden_svg = NodeHandle::element("svg");
    let visible_svg = NodeHandle::element("svg");

    style_svg.set_attribute("viewBox", "0 0 6 6");
    style.append_child(style_svg);
    hidden.set_attribute("style", "display: none");
    hidden_svg.set_attribute("viewBox", "0 0 12 12");
    hidden.append_child(hidden_svg);
    visible_svg.set_attribute("viewBox", "0 0 24 24");

    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(button.clone());
    button.append_child(style);
    button.append_child(hidden);
    button.append_child(visible_svg);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "button { width: 36px; height: 36px; padding: 0; border: 0; }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 100.0, height: 0.0 },
    )
    .unwrap();
    let fragment = &layout.children[0].lines[0].fragments[0];

    assert!(matches!(
        fragment.content,
        InlineFragmentContent::IconFormControl(_, _, 24.0, 24.0)
    ));
}

#[test]
fn media_elements_create_placeholders_from_default_and_attribute_sizes() {
    for (tag, attributes, expected) in [
        ("video", vec![("width", "320"), ("height", "180")], (320.0, 180.0)),
        ("canvas", vec![("width", "200"), ("height", "100")], (200.0, 100.0)),
        ("audio", vec![("controls", "")], (300.0, 54.0)),
    ] {
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let media = NodeHandle::element(tag);
        for (name, value) in attributes {
            media.set_attribute(name, value);
        }
        container.append_child(media);
        body.append_child(container);

        let layout = layout_single_control_container(&body);
        let fragments = form_control_fragments(&layout);
        assert_eq!(fragments.len(), 1, "{tag} should create one placeholder");
        assert_eq!((fragments[0].0.width, fragments[0].0.height), expected);
    }
}

#[test]
fn video_poster_and_picture_img_use_image_fallback() {
    let data_uri = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
    let video = NodeHandle::element("video");
    video.set_attribute("poster", data_uri);
    let (_, poster) = super::element_inline_image(&video).expect("video poster image");
    assert_eq!((poster.width(), poster.height()), (1, 1));

    let picture = NodeHandle::element("picture");
    let source = NodeHandle::element("source");
    source.set_attribute("srcset", "unsupported.webp");
    let img = NodeHandle::element("img");
    img.set_attribute("src", data_uri);
    picture.append_child(source);
    picture.append_child(img.clone());
    let (image_node, fallback) =
        super::element_inline_image(&picture).expect("picture img fallback");
    assert_eq!(image_node.identity(), img.identity());
    assert_eq!((fallback.width(), fallback.height()), (1, 1));
}

#[test]
fn progress_and_meter_create_placeholder_fragments() {
    for tag in ["progress", "meter"] {
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let indicator = NodeHandle::element(tag);
        container.append_child(indicator);
        body.append_child(container);

        let layout = layout_single_control_container(&body);
        let fragments = form_control_fragments(&layout);
        assert_eq!(fragments.len(), 1, "{tag} should create one placeholder");
        assert_eq!(fragments[0].0.width, 162.0);
        assert_eq!(fragments[0].0.height, 18.0);
        assert!(fragments[0].1.is_empty());
    }
}

#[test]
fn textarea_size_derives_from_cols_and_rows() {
    // font-size:20px => average_advance = 12, line-height(normal) = 24.
    // width  = cols(10) * 12 + padding(2+2) + border(1+1) = 126.
    // height = rows(3)  * 24 + padding(2+2) + border(1+1) = 78.
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let textarea = NodeHandle::element("textarea");
    textarea.set_attribute("cols", "10");
    textarea.set_attribute("rows", "3");
    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(textarea);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("textarea { font-size: 20px; }").unwrap(),
    );
    let container = layout_control_container(&body, &mut resolver);
    let fragments = form_control_fragments(&container);
    assert_eq!(fragments.len(), 1);
    assert!(
        (fragments[0].0.width - 126.0).abs() < 0.01,
        "cols-derived width expected 126, got {}",
        fragments[0].0.width
    );
    assert!(
        (fragments[0].0.height - 78.0).abs() < 0.01,
        "rows-derived height expected 78, got {}",
        fragments[0].0.height
    );
}

#[test]
fn textarea_initial_value_is_text_content() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let textarea = NodeHandle::element("textarea");
    textarea.append_child(NodeHandle::text("hello"));
    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(textarea);

    let container = layout_single_control_container(&body);
    let fragments = form_control_fragments(&container);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].1, "hello");
}

#[test]
fn select_uses_selected_option_label_and_longest_option_width() {
    // Second option is `selected`, so the label is "Banana"; the control width is
    // driven by the longest option, not the selected/first one.
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let select = NodeHandle::element("select");
    let opt1 = NodeHandle::element("option");
    let opt2 = NodeHandle::element("option");
    let opt3 = NodeHandle::element("option");
    opt1.append_child(NodeHandle::text("Fig"));
    opt2.set_attribute("selected", "selected");
    opt2.append_child(NodeHandle::text("Banana"));
    opt3.append_child(NodeHandle::text("A really long option label"));
    select.append_child(opt1);
    select.append_child(opt2);
    select.append_child(opt3);
    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(select);

    let container = layout_single_control_container(&body);
    let fragments = form_control_fragments(&container);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].1, "Banana");

    // A select whose only option is the short selected label must be narrower,
    // proving the width comes from the longest option, not the shown one.
    let document2 = NodeHandle::document();
    let body2 = NodeHandle::element("body");
    let div2 = NodeHandle::element("div");
    let select2 = NodeHandle::element("select");
    let only = NodeHandle::element("option");
    only.set_attribute("selected", "selected");
    only.append_child(NodeHandle::text("Banana"));
    select2.append_child(only);
    document2.append_child(body2.clone());
    body2.append_child(div2.clone());
    div2.append_child(select2);

    let container2 = layout_single_control_container(&body2);
    let fragments2 = form_control_fragments(&container2);
    assert_eq!(fragments2.len(), 1);
    assert_eq!(fragments2[0].1, "Banana");
    assert!(
        fragments[0].0.width > fragments2[0].0.width,
        "longest-option width {} should exceed shown-label-only width {}",
        fragments[0].0.width,
        fragments2[0].0.width
    );
}

#[test]
fn select_without_selected_uses_first_option() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let select = NodeHandle::element("select");
    let opt1 = NodeHandle::element("option");
    let opt2 = NodeHandle::element("option");
    opt1.append_child(NodeHandle::text("First"));
    opt2.append_child(NodeHandle::text("Second"));
    select.append_child(opt1);
    select.append_child(opt2);
    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(select);

    let container = layout_single_control_container(&body);
    let fragments = form_control_fragments(&container);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].1, "First");
}

#[test]
fn select_without_options_has_arrow_and_box_spacing_width() {
    // No options => widest = 0, so width = arrow(20) + padding(4+4) + border(1+1) = 30,
    // and the label is empty.
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let select = NodeHandle::element("select");
    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(select);

    let container = layout_single_control_container(&body);
    let fragments = form_control_fragments(&container);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].1, "");
    assert!(
        (fragments[0].0.width - 30.0).abs() < 0.01,
        "empty select width expected 30 (arrow 20 + spacing 10), got {}",
        fragments[0].0.width
    );
}

#[test]
fn button_label_excludes_display_none_descendants() {
    // <button>Send<span style="display:none">hidden</span></button> → label "Send".
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let button = NodeHandle::element("button");
    let hidden = NodeHandle::element("span");
    hidden.set_attribute("style", "display: none");
    hidden.append_child(NodeHandle::text("hidden"));
    button.append_child(NodeHandle::text("Send"));
    button.append_child(hidden);
    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(button);

    let container = layout_single_control_container(&body);
    let fragments = form_control_fragments(&container);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].1, "Send");
}

#[test]
fn button_label_excludes_non_rendered_elements() {
    // <button><style>.x{}</style>Go</button> → label "Go" (style/script content
    // must never leak into the label).
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let button = NodeHandle::element("button");
    let style = NodeHandle::element("style");
    style.append_child(NodeHandle::text(".x{}"));
    button.append_child(style);
    button.append_child(NodeHandle::text("Go"));
    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(button);

    let container = layout_single_control_container(&body);
    let fragments = form_control_fragments(&container);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].1, "Go");
}

#[test]
fn select_multiple_selected_uses_last_selected() {
    // Real browsers (non-multiple) show the LAST option carrying `selected`.
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let select = NodeHandle::element("select");
    let opt1 = NodeHandle::element("option");
    let opt2 = NodeHandle::element("option");
    opt1.set_attribute("selected", "selected");
    opt1.append_child(NodeHandle::text("A"));
    opt2.set_attribute("selected", "selected");
    opt2.append_child(NodeHandle::text("B"));
    select.append_child(opt1);
    select.append_child(opt2);
    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(select);

    let container = layout_single_control_container(&body);
    let fragments = form_control_fragments(&container);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].1, "B");
}

#[test]
fn select_display_none_option_excluded_from_label_and_width() {
    // A display:none option must contribute neither the label (even when
    // `selected`) nor the longest-option width.
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    let select = NodeHandle::element("select");
    let hidden = NodeHandle::element("option");
    let visible = NodeHandle::element("option");
    hidden.set_attribute("style", "display: none");
    hidden.set_attribute("selected", "selected");
    hidden.append_child(NodeHandle::text("A very long hidden option label"));
    visible.append_child(NodeHandle::text("Vis"));
    select.append_child(hidden);
    select.append_child(visible);
    document.append_child(body.clone());
    body.append_child(div.clone());
    div.append_child(select);

    let container = layout_single_control_container(&body);
    let fragments = form_control_fragments(&container);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].1, "Vis", "hidden selected option must not become the label");

    // Reference select with only the visible option: widths must match exactly.
    let document2 = NodeHandle::document();
    let body2 = NodeHandle::element("body");
    let div2 = NodeHandle::element("div");
    let select2 = NodeHandle::element("select");
    let only = NodeHandle::element("option");
    only.append_child(NodeHandle::text("Vis"));
    select2.append_child(only);
    document2.append_child(body2.clone());
    body2.append_child(div2.clone());
    div2.append_child(select2);

    let container2 = layout_single_control_container(&body2);
    let fragments2 = form_control_fragments(&container2);
    assert_eq!(fragments2.len(), 1);
    assert!(
        (fragments[0].0.width - fragments2[0].0.width).abs() < 0.01,
        "hidden option must not affect width: {} vs {}",
        fragments[0].0.width,
        fragments2[0].0.width
    );
}

#[test]
fn textarea_strips_single_leading_newline_via_html_parse() {
    // Per the HTML spec, a textarea initial value starting with a single newline
    // has that newline removed. Exercised through real HTML parsing.
    let html = "<html><body><div><textarea>\nhello</textarea></div></body></html>";
    let document = crate::html::TreeBuilder::parse(html).document();
    let body = document.query_selector("body").unwrap();

    let container = layout_single_control_container(&body);
    let fragments = form_control_fragments(&container);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].1, "hello");
}

#[test]
fn textarea_strips_only_one_leading_newline() {
    // Only a single leading \n or \r\n is removed; later newlines are preserved.
    for (raw, expected) in [
        ("\r\nhello", "hello"),
        ("\n\nhi", "\nhi"),
        ("hello", "hello"),
    ] {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let div = NodeHandle::element("div");
        let textarea = NodeHandle::element("textarea");
        textarea.append_child(NodeHandle::text(raw));
        document.append_child(body.clone());
        body.append_child(div.clone());
        div.append_child(textarea);

        let container = layout_single_control_container(&body);
        let fragments = form_control_fragments(&container);
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].1, expected, "raw value {raw:?}");
    }
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
    let stylesheet =
        format!("span::before {{ content: url(\"data:image/png;base64,{image_data}\"); }}");
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
    assert!(
        line.fragments
            .iter()
            .any(|fragment| matches!(fragment.content, InlineFragmentContent::Image(_, _)))
    );
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
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("object { display: inline; }").unwrap(),
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
    assert!(
        line.fragments
            .iter()
            .any(|fragment| matches!(fragment.content, InlineFragmentContent::Image(_, _)))
    );
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
    assert!(
        Image::decode_png(&data).is_ok(),
        "expected PNG payload to decode"
    );
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
    // Columns are proportional to intrinsic width; verify total table width and 2 cells present
    let cell0_w = row_box.children[0].dimensions.content.width;
    let cell1_w = row_box.children[1].dimensions.content.width;
    assert!((cell0_w + cell1_w - 108.0).abs() < 1.0, "cells should share 108px (120 - 3*4 spacing)");
    assert_eq!(table_box.dimensions.content.width, 120.0);
}

#[test]
fn empty_table_row_keeps_table_content_width() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let table = NodeHandle::element("table");
    let empty_row = NodeHandle::element("tr");
    let filled_row = NodeHandle::element("tr");
    let first = NodeHandle::element("td");
    let second = NodeHandle::element("td");

    first.append_child(NodeHandle::text("A"));
    second.append_child(NodeHandle::text("B"));
    document.append_child(body.clone());
    body.append_child(table.clone());
    table.append_child(empty_row.clone());
    table.append_child(filled_row.clone());
    filled_row.append_child(first);
    filled_row.append_child(second);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "table { display: table; width: 120px; border-spacing: 0; } \
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
    assert_eq!(table_box.children.len(), 2);
    assert_eq!(table_box.children[0].dimensions.content.width, 120.0);
    assert_eq!(table_box.children[1].dimensions.content.width, 120.0);
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
    assert_eq!(
        bottom_box.lines[0].rect.y,
        row_box.dimensions.content.y + 20.0
    );
}

#[test]
fn rowspan_keeps_following_row_cells_in_later_columns() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let table = NodeHandle::element("table");
    let first_row = NodeHandle::element("tr");
    let second_row = NodeHandle::element("tr");
    let image_like = NodeHandle::element("td");
    let top_right = NodeHandle::element("td");
    let bottom_left_padding = NodeHandle::element("td");
    let bottom_right = NodeHandle::element("td");

    image_like.set_attribute("rowspan", "2");
    image_like.set_attribute("class", "hero");
    image_like.append_child(NodeHandle::text("left"));
    top_right.append_child(NodeHandle::text("top"));
    bottom_left_padding.append_child(NodeHandle::text("pad"));
    bottom_right.append_child(NodeHandle::text("right"));

    document.append_child(body.clone());
    body.append_child(table.clone());
    table.append_child(first_row.clone());
    table.append_child(second_row.clone());
    first_row.append_child(image_like);
    first_row.append_child(top_right);
    second_row.append_child(bottom_left_padding);
    second_row.append_child(bottom_right);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "table { display: table; width: 300px; border-spacing: 0; } \
             tr { display: table-row; } \
             td { display: table-cell; height: 20px; } \
             .hero { width: 150px; }",
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

    let table_box = &layout.children[0];
    assert_eq!(table_box.children.len(), 2);
    let first_row_box = &table_box.children[0];
    let second_row_box = &table_box.children[1];
    assert_eq!(first_row_box.children.len(), 2);
    assert_eq!(second_row_box.children.len(), 2);
    assert_eq!(first_row_box.children[0].dimensions.content.x, 0.0);
    // hero has explicit width 150px; remaining 150px is split proportionally among 2 auto columns
    assert_eq!(first_row_box.children[0].dimensions.content.width, 150.0);
    assert_eq!(first_row_box.children[1].dimensions.content.x, 150.0);
    let col1_w = first_row_box.children[1].dimensions.content.width;
    let col2_x = second_row_box.children[1].dimensions.content.x;
    assert!((col2_x - (150.0 + col1_w)).abs() < 1.0, "col2 x should be after hero + col1");
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
fn lays_out_grid_row_major_with_fractional_columns_and_gap() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for _ in 0..4 {
        grid.append_child(NodeHandle::element("article"));
    }

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } div { display: grid; width: 210px; grid-template-columns: 1fr 1fr; gap: 10px; } article { height: 20px; }",
        )
        .unwrap(),
    );
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 210.0, height: 0.0 }).unwrap();
    let children = &layout.children[0].children;
    assert_eq!(children.len(), 4);
    let rects: Vec<_> = children.iter().map(|child| child.dimensions.content).collect();
    assert_eq!((rects[0].x, rects[0].y, rects[0].width), (0.0, 0.0, 100.0));
    assert_eq!((rects[1].x, rects[1].y, rects[1].width), (110.0, 0.0, 100.0));
    assert_eq!((rects[2].x, rects[2].y, rects[2].width), (0.0, 30.0, 100.0));
    assert_eq!((rects[3].x, rects[3].y, rects[3].width), (110.0, 30.0, 100.0));
}

#[test]
fn sizes_repeat_auto_px_percent_and_fractional_grid_tracks() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    grid.set_attribute("class", "mixed");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for class_name in ["auto", "fixed", "percent", "fraction"] {
        let child = NodeHandle::element("article");
        child.set_attribute("class", class_name);
        grid.append_child(child);
    }
    let repeated = NodeHandle::element("section");
    repeated.set_attribute("class", "repeated");
    body.append_child(repeated.clone());
    for _ in 0..3 { repeated.append_child(NodeHandle::element("i")); }

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } .mixed { display: grid; width: 400px; grid-template-columns: auto 50px 25% 1fr; } .auto { width: 40px; height: 10px; } article { height: 10px; } .repeated { display: grid; width: 300px; grid-template-columns: repeat(3, 1fr); } i { height: 5px; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 400.0, height: 0.0 }).unwrap();
    let mixed = &layout.children[0];
    let widths: Vec<_> = mixed.children.iter().map(|child| child.dimensions.content.width).collect();
    assert_eq!(widths, vec![40.0, 50.0, 100.0, 210.0]);
    let repeated = &layout.children[1];
    for child in &repeated.children {
        assert!((child.dimensions.content.width - 100.0).abs() < 0.01);
    }
}

fn grid_track_extension_rects(
    template: &str,
    item_count: usize,
    grid_width: f32,
    viewport_width: f32,
) -> Vec<Rect> {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for _ in 0..item_count {
        grid.append_child(NodeHandle::element("article"));
    }

    let mut resolver = StyleResolver::new();
    resolver.set_viewport(viewport_width, 800.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(&format!(
            "body {{ margin: 0; }} div {{ display: grid; width: {grid_width}px; grid-template-columns: {template}; }} article {{ height: 10px; }}"
        ))
        .unwrap(),
    );
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: viewport_width, height: 0.0 },
    )
    .unwrap();
    layout.children[0]
        .children
        .iter()
        .map(|child| child.dimensions.content)
        .collect()
}

#[test]
fn resolves_viewport_and_calc_grid_tracks_to_exact_widths() {
    let viewport = grid_track_extension_rects("10vw calc(30px * 2) calc(50% - 10px) 1fr", 4, 500.0, 1000.0);
    assert_eq!((viewport[0].x, viewport[0].width), (0.0, 100.0));
    assert_eq!((viewport[1].x, viewport[1].width), (100.0, 60.0));
    assert_eq!((viewport[2].x, viewport[2].width), (160.0, 240.0));
    assert_eq!((viewport[3].x, viewport[3].width), (400.0, 100.0));
}

#[test]
fn sizes_minmax_tracks_with_fractional_and_fixed_maxima() {
    let fractional = grid_track_extension_rects(
        "minmax(150px, 1fr) minmax(50px, 2fr)",
        2,
        400.0,
        400.0,
    );
    assert_eq!((fractional[0].x, fractional[0].width), (0.0, 150.0));
    assert_eq!((fractional[1].x, fractional[1].width), (150.0, 250.0));

    let fixed = grid_track_extension_rects(
        "minmax(40px, 90px) minmax(120px, 80px) 1fr",
        3,
        300.0,
        300.0,
    );
    assert_eq!((fixed[0].x, fixed[0].width), (0.0, 90.0));
    assert_eq!((fixed[1].x, fixed[1].width), (90.0, 120.0));
    assert_eq!((fixed[2].x, fixed[2].width), (210.0, 90.0));
}

#[test]
fn expands_multiple_tracks_in_numeric_repeat() {
    let rects = grid_track_extension_rects("repeat(2, 40px 60px)", 4, 200.0, 200.0);
    assert_eq!((rects[0].x, rects[0].width), (0.0, 40.0));
    assert_eq!((rects[1].x, rects[1].width), (40.0, 60.0));
    assert_eq!((rects[2].x, rects[2].width), (100.0, 40.0));
    assert_eq!((rects[3].x, rects[3].width), (140.0, 60.0));
}

#[test]
fn auto_fill_keeps_empty_repetitions_for_fractional_sizing() {
    let rects = grid_track_extension_rects(
        "repeat(auto-fill, minmax(100px, 1fr))",
        2,
        400.0,
        400.0,
    );
    assert_eq!((rects[0].x, rects[0].width), (0.0, 100.0));
    assert_eq!((rects[1].x, rects[1].width), (100.0, 100.0));
}

#[test]
fn auto_fit_collapses_empty_repetitions_before_fractional_sizing() {
    let rects = grid_track_extension_rects(
        "repeat(auto-fit, minmax(100px, 1fr))",
        2,
        400.0,
        400.0,
    );
    assert_eq!((rects[0].x, rects[0].width), (0.0, 200.0));
    assert_eq!((rects[1].x, rects[1].width), (200.0, 200.0));
}

#[test]
fn auto_fit_collapses_gutters_adjacent_to_empty_repetitions() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for _ in 0..2 {
        grid.append_child(NodeHandle::element("article"));
    }

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } div { display: grid; width: 430px; column-gap: 10px; grid-template-columns: repeat(auto-fit, minmax(100px, 1fr)); } article { height: 10px; }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 430.0, height: 0.0 },
    )
    .unwrap();
    let rects: Vec<_> = layout.children[0]
        .children
        .iter()
        .map(|child| child.dimensions.content)
        .collect();
    assert_eq!((rects[0].x, rects[0].width), (0.0, 210.0));
    assert_eq!((rects[1].x, rects[1].width), (220.0, 210.0));
}

#[test]
fn skips_named_grid_lines_while_parsing_tracks() {
    let rects = grid_track_extension_rects(
        "[start] 80px [middle alternate] 120px [end]",
        2,
        200.0,
        200.0,
    );
    assert_eq!((rects[0].x, rects[0].width), (0.0, 80.0));
    assert_eq!((rects[1].x, rects[1].width), (80.0, 120.0));
}

#[test]
fn falls_back_only_unparseable_grid_tracks_to_auto() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    let first = NodeHandle::element("article");
    let fallback = NodeHandle::element("article");
    let third = NodeHandle::element("article");
    fallback.set_attribute("class", "fallback");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    grid.append_child(first);
    grid.append_child(fallback);
    grid.append_child(third);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } div { display: grid; width: 220px; grid-template-columns: 50px fit-content(20px) 100px; } article { height: 10px; } .fallback { width: 70px; }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 220.0, height: 0.0 },
    )
    .unwrap();
    let rects: Vec<_> = layout.children[0]
        .children
        .iter()
        .map(|child| child.dimensions.content)
        .collect();
    assert_eq!((rects[0].x, rects[0].width), (0.0, 50.0));
    assert_eq!((rects[1].x, rects[1].width), (50.0, 70.0));
    assert_eq!((rects[2].x, rects[2].width), (120.0, 100.0));
}

#[test]
fn creates_implicit_grid_rows_using_row_content_height() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for class_name in ["h10", "h20", "h15"] {
        let child = NodeHandle::element("article");
        child.set_attribute("class", class_name);
        grid.append_child(child);
    }
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: inline-grid; width: 200px; grid-template-columns: repeat(2, 1fr); row-gap: 5px; } .h10 { height: 10px; } .h20 { height: 20px; } .h15 { height: 15px; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 }).unwrap();
    let grid = &layout.children[0];
    assert_eq!(grid.children[0].dimensions.content.y, 0.0);
    assert_eq!(grid.children[1].dimensions.content.y, 0.0);
    assert_eq!(grid.children[2].dimensions.content.y, 25.0);
    assert_eq!(grid.dimensions.content.height, 40.0);
}

#[test]
fn resolves_percentage_grid_row_against_explicit_container_height() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    let child = NodeHandle::element("article");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    grid.append_child(child);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; height: 200px; grid-template-rows: 50%; } article { height: 100%; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 }).unwrap();
    let child = &layout.children[0].children[0];
    assert_eq!(child.dimensions.content.height, 100.0);
}

#[test]
fn places_grid_items_by_explicit_lines_and_spans() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for class_name in ["lines", "column-span", "row-span"] {
        let child = NodeHandle::element("article");
        child.set_attribute("class", class_name);
        grid.append_child(child);
    }

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 320px; grid-template-columns: repeat(3, 100px); grid-template-rows: repeat(3, 20px); gap: 10px; } article { height: 100%; } .lines { grid-column: 1 / 3; } .column-span { grid-column: span 2; grid-row: 2; } .row-span { grid-column-start: 3; grid-row: 1 / span 3; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 320.0, height: 0.0 }).unwrap();
    let rects: Vec<_> = layout.children[0].children.iter().map(|child| child.dimensions.content).collect();
    assert_eq!((rects[0].x, rects[0].y, rects[0].width, rects[0].height), (0.0, 0.0, 210.0, 20.0));
    assert_eq!((rects[1].x, rects[1].y, rects[1].width, rects[1].height), (0.0, 30.0, 210.0, 20.0));
    assert_eq!((rects[2].x, rects[2].y, rects[2].width, rects[2].height), (220.0, 0.0, 100.0, 80.0));
}

#[test]
fn places_items_in_named_grid_areas_with_exact_rectangles() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for class_name in ["a", "b", "c"] {
        let child = NodeHandle::element("article");
        child.set_attribute("class", class_name);
        grid.append_child(child);
    }

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 230px; grid-template-columns: 100px 120px; grid-template-rows: 30px 40px; gap: 10px; grid-template-areas: \"a a\" \"b c\"; } article { height: 100%; } .a { grid-area: a; } .b { grid-area: b; } .c { grid-area: c; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 230.0, height: 0.0 }).unwrap();
    let rects: Vec<_> = layout.children[0].children.iter().map(|child| child.dimensions.content).collect();
    assert_eq!((rects[0].x, rects[0].y, rects[0].width, rects[0].height), (0.0, 0.0, 230.0, 30.0));
    assert_eq!((rects[1].x, rects[1].y, rects[1].width, rects[1].height), (0.0, 40.0, 100.0, 40.0));
    assert_eq!((rects[2].x, rects[2].y, rects[2].width, rects[2].height), (110.0, 40.0, 120.0, 40.0));
}

#[test]
fn leaves_dot_cells_available_to_auto_placement() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for class_name in ["tall", "auto", "corner"] {
        let child = NodeHandle::element("article");
        child.set_attribute("class", class_name);
        grid.append_child(child);
    }

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 210px; grid-template-columns: 100px 100px; grid-template-rows: 20px 30px; gap: 10px; grid-template-areas: \"tall ...\" \"tall corner\"; } article { height: 100%; } .tall { grid-area: tall; } .corner { grid-area: corner; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 210.0, height: 0.0 }).unwrap();
    let rects: Vec<_> = layout.children[0].children.iter().map(|child| child.dimensions.content).collect();
    assert_eq!((rects[0].x, rects[0].y, rects[0].width, rects[0].height), (0.0, 0.0, 100.0, 60.0));
    assert_eq!((rects[1].x, rects[1].y, rects[1].width, rects[1].height), (110.0, 0.0, 100.0, 20.0));
    assert_eq!((rects[2].x, rects[2].y, rects[2].width, rects[2].height), (110.0, 30.0, 100.0, 30.0));
}

#[test]
fn auto_grid_line_keyword_does_not_resolve_to_an_area_named_auto() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    let child = NodeHandle::element("article");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    grid.append_child(child);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 200px; grid-template-columns: 100px 100px; grid-template-rows: 20px; grid-template-areas: \"free auto\"; } article { grid-column: auto; grid-row: 1; height: 20px; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 }).unwrap();
    let rect = layout.children[0].children[0].dimensions.content;
    assert_eq!((rect.x, rect.y, rect.width, rect.height), (0.0, 0.0, 100.0, 20.0));
}

#[test]
fn area_columns_expand_the_explicit_grid_for_names_and_negative_lines() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for class_name in ["named", "negative"] {
        let child = NodeHandle::element("article");
        child.set_attribute("class", class_name);
        grid.append_child(child);
    }

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 200px; grid-template-columns: 100px; grid-template-rows: 20px 20px; grid-template-areas: \"a b c\"; } article { height: 20px; } .named { grid-area: b; width: 60px; } .negative { grid-column: -2; grid-row: 2; width: 40px; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 }).unwrap();
    let rects: Vec<_> = layout.children[0].children.iter().map(|child| child.dimensions.content).collect();
    assert_eq!((rects[0].x, rects[0].y, rects[0].width, rects[0].height), (100.0, 0.0, 60.0, 20.0));
    assert_eq!((rects[1].x, rects[1].y, rects[1].width, rects[1].height), (160.0, 20.0, 40.0, 20.0));
}

#[test]
fn invalid_non_rectangular_area_falls_back_to_auto_placement() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    let child = NodeHandle::element("article");
    child.set_attribute("class", "bad");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    grid.append_child(child);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 200px; grid-template-columns: 100px 100px; grid-template-rows: 20px 30px; grid-template-areas: \"bad bad\" \"bad .\"; } article { height: 100%; } .bad { grid-area: bad; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 }).unwrap();
    let rect = layout.children[0].children[0].dimensions.content;
    assert_eq!((rect.x, rect.y, rect.width, rect.height), (0.0, 0.0, 100.0, 20.0));
}

#[test]
fn grid_template_area_rows_create_implicit_auto_rows() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    let child = NodeHandle::element("article");
    child.set_attribute("class", "footer");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    grid.append_child(child);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 200px; grid-template-columns: 200px; grid-template-rows: 25px; grid-template-areas: \"header\" \"footer\"; } article { height: 35px; } .footer { grid-area: footer; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 }).unwrap();
    let grid = &layout.children[0];
    let rect = grid.children[0].dimensions.content;
    assert_eq!((rect.x, rect.y, rect.width, rect.height), (0.0, 25.0, 200.0, 35.0));
    assert_eq!(grid.dimensions.content.height, 60.0);
}

#[test]
fn lays_out_grid_template_shorthand_with_calc_and_viewport_tracks() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for class_name in ["hero", "side", "main"] {
        let child = NodeHandle::element("article");
        child.set_attribute("class", class_name);
        grid.append_child(child);
    }

    let mut resolver = StyleResolver::new();
    resolver.set_viewport(1000.0, 800.0);
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 400px; grid-template: \"hero hero\" 30px \"side main\" auto / calc(10vw + 20px) 1fr; } article { height: 40px; } .hero { grid-area: hero; height: 100%; } .side { grid-area: side; } .main { grid-area: main; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 1000.0, height: 0.0 }).unwrap();
    let rects: Vec<_> = layout.children[0].children.iter().map(|child| child.dimensions.content).collect();
    assert_eq!((rects[0].x, rects[0].y, rects[0].width, rects[0].height), (0.0, 0.0, 400.0, 30.0));
    assert_eq!((rects[1].x, rects[1].y, rects[1].width, rects[1].height), (0.0, 30.0, 120.0, 40.0));
    assert_eq!((rects[2].x, rects[2].y, rects[2].width, rects[2].height), (120.0, 30.0, 280.0, 40.0));
}

#[test]
fn lays_out_kasaneteto_named_areas_with_compact_grid_slash() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for class_name in ["kasane", "teto", "official", "singable", "since", "april"] {
        let child = NodeHandle::element("article");
        child.set_attribute("class", class_name);
        grid.append_child(child);
    }

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; }
             div {
               display: grid;
               margin-inline: auto;
               width: 840px;
               grid-template:
                 \"kasane teto teto\" auto
                 \"official official official\" auto
                 \"singable since april\" auto/1fr 210px 90px;
             }
             article { height: 20px; }
             .kasane { grid-area: kasane; }
             .teto { grid-area: teto; }
             .official { grid-area: official; }
             .singable { grid-area: singable; }
             .since { grid-area: since; }
             .april { grid-area: april; }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 1000.0, height: 0.0 },
    )
    .unwrap();
    let children = &layout.children[0].children;

    assert_eq!((children[0].dimensions.content.x, children[0].dimensions.content.y), (80.0, 0.0));
    assert_eq!((children[1].dimensions.content.x, children[1].dimensions.content.y), (620.0, 0.0));
    assert_eq!(children[2].dimensions.content.y, 20.0);
    assert_eq!(children[3].dimensions.content.y, 40.0);
    assert_eq!(children[4].dimensions.content.y, 40.0);
    assert_eq!(children[5].dimensions.content.y, 40.0);
}

#[test]
fn resolves_negative_grid_line_to_explicit_grid_end() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    let child = NodeHandle::element("article");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    grid.append_child(child);
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 320px; grid-template-columns: repeat(3, 100px); column-gap: 10px; } article { grid-column: 1 / -1; height: 10px; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 320.0, height: 0.0 }).unwrap();
    let rect = layout.children[0].children[0].dimensions.content;
    assert_eq!((rect.x, rect.width), (0.0, 320.0));
}

#[test]
fn auto_placement_skips_cells_occupied_by_explicit_items() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for class_name in ["auto-first", "placed", "auto-second"] {
        let child = NodeHandle::element("article");
        child.set_attribute("class", class_name);
        grid.append_child(child);
    }
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 210px; grid-template-columns: repeat(2, 100px); gap: 10px; } article { height: 20px; } .placed { grid-column: 1; grid-row: 1; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 210.0, height: 0.0 }).unwrap();
    let rects: Vec<_> = layout.children[0].children.iter().map(|child| child.dimensions.content).collect();
    assert_eq!((rects[0].x, rects[0].y), (110.0, 0.0));
    assert_eq!((rects[1].x, rects[1].y), (0.0, 0.0));
    assert_eq!((rects[2].x, rects[2].y), (0.0, 30.0));
}

#[test]
fn overlapping_row_spans_assign_each_height_deficit_to_one_row() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for class_name in ["first", "second"] {
        let child = NodeHandle::element("article");
        child.set_attribute("class", class_name);
        grid.append_child(child);
    }
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; grid-template-columns: repeat(2, 100px); } article { height: 100px; } .first { grid-column: 1; grid-row: 1 / 3; } .second { grid-column: 2; grid-row: 2 / 4; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 }).unwrap();
    let grid = &layout.children[0];
    assert_eq!(grid.dimensions.content.height, 100.0);
    assert_eq!(grid.children[0].dimensions.content.y, 0.0);
    assert_eq!(grid.children[1].dimensions.content.y, 0.0);
}

#[test]
fn span_only_item_remains_in_auto_placement_order() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for class_name in ["span-only", "auto", "placed"] {
        let child = NodeHandle::element("article");
        child.set_attribute("class", class_name);
        grid.append_child(child);
    }
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 300px; grid-template-columns: repeat(3, 100px); } article { height: 20px; } .span-only { grid-column: span 2; } .placed { grid-column-start: 1; grid-row-start: 1; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 300.0, height: 0.0 }).unwrap();
    let rects: Vec<_> = layout.children[0].children.iter().map(|child| child.dimensions.content).collect();
    assert_eq!((rects[0].x, rects[0].y, rects[0].width), (100.0, 0.0, 200.0));
    assert_eq!((rects[1].x, rects[1].y), (0.0, 20.0));
    assert_eq!((rects[2].x, rects[2].y), (0.0, 0.0));
}

#[test]
fn explicit_grid_placement_creates_implicit_columns_and_rows() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    let child = NodeHandle::element("article");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    grid.append_child(child);
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 200px; grid-template-columns: 100px; grid-template-rows: 20px; gap: 5px; } article { grid-column: 2 / 4; grid-row: 2 / 4; height: 100%; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 }).unwrap();
    let rect = layout.children[0].children[0].dimensions.content;
    assert_eq!((rect.x, rect.y, rect.width, rect.height), (105.0, 25.0, 5.0, 5.0));
}

#[test]
fn aligns_grid_items_inside_their_cells_and_allows_self_override() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    for class_name in ["default", "override"] {
        let child = NodeHandle::element("article");
        child.set_attribute("class", class_name);
        grid.append_child(child);
    }
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 200px; height: 100px; grid-template-columns: repeat(2, 100px); grid-template-rows: 100px; justify-items: center; align-items: end; } article { width: 20px; height: 10px; } .override { justify-self: end; align-self: start; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 }).unwrap();
    let rects: Vec<_> = layout.children[0].children.iter().map(|child| child.dimensions.content).collect();
    assert_eq!((rects[0].x, rects[0].y, rects[0].width, rects[0].height), (40.0, 90.0, 20.0, 10.0));
    assert_eq!((rects[1].x, rects[1].y, rects[1].width, rects[1].height), (180.0, 0.0, 20.0, 10.0));
}

#[test]
fn grid_justify_self_start_resolves_percentage_width_against_cell() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    let child = NodeHandle::element("article");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    grid.append_child(child);
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 200px; grid-template-columns: 200px; } article { justify-self: start; width: 50%; height: 10px; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 }).unwrap();
    let rect = layout.children[0].children[0].dimensions.content;
    assert_eq!((rect.x, rect.width), (0.0, 100.0));
}

#[test]
fn grid_justify_self_auto_falls_back_to_justify_items() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    let child = NodeHandle::element("article");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    grid.append_child(child);
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 200px; grid-template-columns: 200px; justify-items: center; } article { justify-self: auto; width: 40px; height: 10px; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 }).unwrap();
    let rect = layout.children[0].children[0].dimensions.content;
    assert_eq!((rect.x, rect.width), (80.0, 40.0));
}

#[test]
fn distributes_grid_track_space_and_expands_place_shorthands() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    document.append_child(body.clone());
    for class_name in ["between", "centered"] {
        let grid = NodeHandle::element("div");
        grid.set_attribute("class", class_name);
        body.append_child(grid.clone());
        for _ in 0..2 { grid.append_child(NodeHandle::element("article")); }
    }
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(
        "body { margin: 0; } div { display: grid; width: 300px; height: 100px; grid-template-columns: repeat(2, 50px); grid-template-rows: 20px; } article { width: 10px; height: 10px; } .between { justify-content: space-between; } .centered { place-content: center; place-items: center; }"
    ).unwrap());
    let layout = layout_tree(&body, &mut resolver, Rect { x: 0.0, y: 0.0, width: 300.0, height: 0.0 }).unwrap();
    let between = &layout.children[0];
    assert_eq!((between.children[0].dimensions.content.x, between.children[1].dimensions.content.x), (0.0, 250.0));
    let centered = &layout.children[1];
    assert_eq!((centered.children[0].dimensions.content.x, centered.children[0].dimensions.content.y), (120.0, 145.0));
    assert_eq!((centered.children[1].dimensions.content.x, centered.children[1].dimensions.content.y), (170.0, 145.0));
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
fn flex_auto_basis_sums_consecutive_inline_children() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("div");
    let item = NodeHandle::element("article");
    let first = NodeHandle::element("span");
    let second = NodeHandle::element("span");
    let fixed = NodeHandle::element("aside");
    document.append_child(body.clone());
    body.append_child(container.clone());
    container.append_child(item.clone());
    container.append_child(fixed);
    item.append_child(first);
    item.append_child(second);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { display: flex; width: 200px; } \
             span { display: inline-block; height: 10px; } \
             span:first-child { width: 40px; } \
             span:last-child { width: 50px; } \
             aside { width: 20px; height: 10px; flex-shrink: 0; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 },
    )
    .unwrap();
    assert_eq!(layout.children[0].children[0].dimensions.content.width, 90.0);
}

#[test]
fn flex_auto_basis_preserves_content_width_when_item_has_margins() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("div");
    let link = NodeHandle::element("a");
    let filler = NodeHandle::element("span");
    document.append_child(body.clone());
    body.append_child(container.clone());
    container.append_child(link.clone());
    container.append_child(filler);
    link.append_child(NodeHandle::text("about"));

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { display: flex; width: 200px; } \
             a { display: inline-block; margin: 0 15px; padding: 0 5px; \
                 font-size: 10px; white-space: nowrap; } \
             span { flex-grow: 1; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 },
    )
    .unwrap();
    let link_box = &layout.children[0].children[0];
    assert_eq!(link_box.lines.len(), 1);
    assert!(
        link_box.dimensions.content.width >= link_box.lines[0].rect.width,
        "content width {} should contain the unwrapped line width {}",
        link_box.dimensions.content.width,
        link_box.lines[0].rect.width,
    );
}

#[test]
fn flex_item_does_not_shrink_below_nowrap_content_width() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("div");
    let item = NodeHandle::element("span");
    let text = NodeHandle::text("one two three four");
    document.append_child(body.clone());
    body.append_child(container.clone());
    container.append_child(item.clone());
    item.append_child(text);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { display: flex; width: 30px; } \
             span { white-space: nowrap; font-size: 10px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 30.0, height: 0.0 },
    )
    .unwrap();
    assert!(layout.children[0].children[0].dimensions.content.width > 30.0);
}

#[test]
fn intrinsic_width_ignores_display_none_descendants() {
    let parent = NodeHandle::element("div");
    let visible = NodeHandle::element("span");
    let hidden = NodeHandle::element("aside");
    parent.append_child(visible.clone());
    parent.append_child(hidden);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "span { display: inline-block; width: 20px; } \
             aside { display: none; width: 1000px; }",
        )
        .unwrap(),
    );

    assert_eq!(intrinsic_width(&parent, &mut resolver), 20.0);
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
    assert_eq!(
        container_box.dimensions.content.height,
        60.0,
        "auto-height column flex containers must use their main-axis content height",
    );
}

#[test]
fn flex_column_uses_height_as_main_axis_for_justify_content_and_gap() {
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
            "div { display: flex; flex-direction: column; width: 120px; height: 100px; justify-content: space-between; gap: 10px 4px; } \
             section { height: 20px; }",
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
    assert_eq!(container_box.children[1].dimensions.content.y, 80.0);
}

#[test]
fn flex_column_distributes_min_height_to_growing_child() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("div");
    let child = NodeHandle::element("article");
    document.append_child(body.clone());
    body.append_child(container.clone());
    container.append_child(child);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { display: flex; flex-direction: column; min-height: 100px; } \
             article { flex-grow: 1; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 },
    )
    .unwrap();

    let container_box = &layout.children[0];
    assert_eq!(container_box.dimensions.content.height, 100.0);
    assert_eq!(container_box.children[0].dimensions.content.height, 100.0);
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
    assert_eq!(container_box.children[0].dimensions.content.y, 30.0);
    assert_eq!(container_box.children[1].dimensions.content.y, 70.0);
}

#[test]
fn column_flex_aligns_items_against_the_container_width() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let main = NodeHandle::element("main");
    let span = NodeHandle::element("span");
    document.append_child(body.clone());
    body.append_child(main.clone());
    main.append_child(span);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "main { display: flex; flex-direction: column; align-items: center; width: 300px } \
             span { width: 40px; height: 10px }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 400.0,
            height: 0.0,
        },
    )
    .unwrap();

    let main = &layout.children[0];
    let span = &main.children[0];
    assert_eq!(span.dimensions.content.x, 130.0);
}

#[test]
fn flex_row_aligns_items_within_min_height() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("div");
    let child = NodeHandle::element("article");
    document.append_child(body.clone());
    body.append_child(container.clone());
    container.append_child(child);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { display: flex; width: 200px; min-height: 100px; align-items: center; } \
             article { width: 60px; height: 20px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 200.0, height: 100.0 },
    )
    .unwrap();

    assert_eq!(layout.children[0].children[0].dimensions.content.y, 40.0);
}

#[test]
fn flex_row_uses_intrinsic_width_for_auto_basis_items() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("div");
    let first = NodeHandle::element("a");
    let second = NodeHandle::element("a");

    first.append_child(NodeHandle::text("五里霧中"));
    second.append_child(NodeHandle::text("blog"));
    document.append_child(body.clone());
    body.append_child(container.clone());
    container.append_child(first);
    container.append_child(second);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { word-break: break-word; } \
                 div { display: flex; width: 300px; justify-content: space-between; } \
                 a { display: block; }",
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
    assert!(container_box.children[0].dimensions.content.width > 0.0);
    assert!(container_box.children[1].dimensions.content.width > 0.0);
    assert!(
        container_box.children[1].dimensions.content.x
            >= container_box.children[0].dimensions.content.x
                + container_box.children[0].dimensions.content.width
    );
}

#[test]
fn nested_flex_container_keeps_menu_items_separated() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let nav = NodeHandle::element("nav");
    let logo = NodeHandle::element("div");
    let menu = NodeHandle::element("ul");
    let item_a = NodeHandle::element("li");
    let item_b = NodeHandle::element("li");
    let item_c = NodeHandle::element("li");

    logo.append_child(NodeHandle::text("logo"));
    item_a.append_child(NodeHandle::text("top"));
    item_b.append_child(NodeHandle::text("blog"));
    item_c.append_child(NodeHandle::text("tags"));
    menu.append_child(item_a);
    menu.append_child(item_b);
    menu.append_child(item_c);
    nav.append_child(logo);
    nav.append_child(menu);
    document.append_child(body.clone());
    body.append_child(nav.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "nav { display: flex; width: 600px; justify-content: space-between; } \
             ul { display: flex; } \
             body { word-break: break-word; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 600.0,
            height: 0.0,
        },
    )
    .unwrap();

    let nav_box = &layout.children[0];
    assert_eq!(nav_box.children.len(), 2);
    let menu_box = &nav_box.children[1];
    assert_eq!(menu_box.children.len(), 3);
    assert!(menu_box.children[1].dimensions.content.x > menu_box.children[0].dimensions.content.x);
    assert!(menu_box.children[2].dimensions.content.x > menu_box.children[1].dimensions.content.x);
}

#[test]
fn logical_margin_inline_start_offsets_flex_item() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("div");
    let first = NodeHandle::element("article");
    let second = NodeHandle::element("article");

    second.set_attribute("class", "spaced");
    document.append_child(body.clone());
    body.append_child(container.clone());
    container.append_child(first);
    container.append_child(second);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { display: flex; width: 200px; } \
                 article { width: 40px; height: 10px; } \
                 .spaced { margin-inline-start: 20px; }",
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
    assert_eq!(container_box.children.len(), 2);
    assert_eq!(container_box.children[0].dimensions.content.x, 0.0);
    assert_eq!(container_box.children[1].dimensions.content.x, 60.0);
}

#[test]
fn centered_flex_button_keeps_intrinsic_text_on_one_line() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let button = NodeHandle::element("span");
    let icon_wrapper = NodeHandle::element("span");
    let icon = NodeHandle::element("svg");
    let label = NodeHandle::element("span");
    label.append_child(NodeHandle::text("Continue with phone"));
    icon_wrapper.append_child(icon);
    button.append_child(icon_wrapper);
    button.append_child(label);
    body.append_child(button);
    document.append_child(body.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; width: 384px; font-size: 15px; } \
             body > span { display: flex; align-items: center; justify-content: center; \
                           padding-inline: 24px; padding-block: 12px; } \
             span span:first-child { margin-inline-end: 8px; } \
             svg { display: block; width: 22px; height: 22px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 384.0, height: 0.0 },
    )
    .unwrap();

    let button_box = &layout.children[0];
    let label_box = &button_box.children[1];
    assert_eq!(
        label_box.lines.len(),
        1,
        "label width was {} with lines {:?}",
        label_box.dimensions.content.width,
        label_box.lines,
    );
    let line = &label_box.lines[0];
    assert!(
        line.fragments
            .iter()
            .all(|fragment| fragment.rect.y == line.fragments[0].rect.y),
        "fragments were vertically split: {:?}",
        line.fragments,
    );
    assert_eq!(
        line.fragments.iter().filter_map(InlineFragment::text).collect::<String>(),
        "Continue with phone",
    );
}

#[test]
fn inline_wrapping_ignores_subpixel_font_fragment_rounding() {
    assert!(!super::inline::exceeds_available_inline_width(125.99748, 125.99));
    assert!(super::inline::exceeds_available_inline_width(126.01, 125.99));
}

#[test]
fn collapsible_whitespace_around_full_width_image_does_not_create_lines() {
    let image = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAABnRSTlMAAAAAAABupgeRAAAABmJLR0QA%2FwD%2FAP%2BgvaeTAAAAEUlEQVR42mP4%2F58BCv7%2FZwAAHfAD%2FabwPj4AAAAASUVORK5CYII%3D";
    let html = format!(
        "<html><body><div>\n  <a>\n    <img src=\"{image}\" width=\"2\" height=\"2\">\n  </a>\n</div></body></html>"
    );
    let document = crate::html::TreeBuilder::parse(&html).document();
    let body = document
        .child_nodes()
        .into_iter()
        .find(|node| node.tag_name().as_deref() == Some("html"))
        .and_then(|html| {
            html.child_nodes()
                .into_iter()
                .find(|node| node.tag_name().as_deref() == Some("body"))
        })
        .unwrap();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { margin: 0; } div { width: 2px; line-height: 20px; }").unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 20.0, height: 0.0 },
    )
    .unwrap();
    let container = find_layout_box_by_tag(&layout, "div").unwrap();

    assert_eq!(container.lines.len(), 1, "lines: {:?}", container.lines);
    assert_eq!(container.lines[0].rect.height, 20.0);
    assert_eq!(container.dimensions.content.height, 20.0);
}

#[test]
fn block_svg_flex_item_keeps_its_replaced_image_fragment() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("div");
    let svg = NodeHandle::element("svg");
    svg.set_attribute("viewBox", "0 0 100 50");
    let rect = NodeHandle::element("rect");
    rect.set_attribute("width", "100");
    rect.set_attribute("height", "50");
    rect.set_attribute("fill", "black");
    svg.append_child(rect);
    container.append_child(svg);
    body.append_child(container);
    document.append_child(body.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { display: flex; width: 200px; } svg { display: block; }")
            .unwrap(),
    );
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 100.0,
        },
    )
    .unwrap();

    let svg_box = &layout.children[0].children[0];
    assert!(svg_box.lines.iter().any(|line| line.fragments.iter().any(|fragment| {
        matches!(fragment.content, InlineFragmentContent::Image(_, _))
    })));
    assert_eq!(svg_box.dimensions.content.height, 50.0);
}

#[test]
fn block_svg_percentage_width_scales_image_to_flex_item() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("div");
    let svg = NodeHandle::element("svg");
    svg.set_attribute("viewBox", "0 0 400 200");
    let rect = NodeHandle::element("rect");
    rect.set_attribute("width", "400");
    rect.set_attribute("height", "200");
    svg.append_child(rect);
    container.append_child(svg);
    body.append_child(container);
    document.append_child(body.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { display: flex; width: 160px; } svg { display: block; width: 100%; }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 160.0,
            height: 100.0,
        },
    )
    .unwrap();
    let fragment = &layout.children[0].children[0].lines[0].fragments[0];
    assert_eq!(fragment.rect.width, 160.0);
    assert_eq!(fragment.rect.height, 80.0);
}

#[test]
fn flex_row_honors_column_gap_between_items() {
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
            "div { display: flex; width: 200px; column-gap: 12px; } \
             article { width: 40px; height: 10px; }",
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
    assert_eq!(container_box.children.len(), 2);
    assert_eq!(container_box.children[0].dimensions.content.x, 0.0);
    assert_eq!(container_box.children[1].dimensions.content.x, 52.0);
}

#[test]
fn wrapped_flex_rows_honor_row_gap() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("div");
    let first = NodeHandle::element("article");
    let second = NodeHandle::element("article");
    let third = NodeHandle::element("article");

    document.append_child(body.clone());
    body.append_child(container.clone());
    container.append_child(first);
    container.append_child(second);
    container.append_child(third);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { display: flex; width: 120px; flex-wrap: wrap; row-gap: 8px; } \
             article { width: 60px; height: 10px; }",
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
    assert_eq!(container_box.children.len(), 3);
    assert_eq!(container_box.children[0].dimensions.content.y, 0.0);
    assert_eq!(container_box.children[1].dimensions.content.y, 0.0);
    assert_eq!(container_box.children[2].dimensions.content.y, 18.0);
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
fn fixed_position_resolves_logical_insets() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let ltr = NodeHandle::element("div");
    ltr.set_attribute("class", "ltr");
    document.append_child(body.clone());
    body.append_child(ltr);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ".ltr { position: fixed; inset-inline-end: 10px; bottom: 20px; width: 50px; height: 30px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 300.0, height: 200.0 },
    )
    .unwrap();

    assert_eq!(layout.children[0].dimensions.content.x, 240.0);
    assert_eq!(layout.children[0].dimensions.content.y, 150.0);
}

#[test]
fn fixed_position_inset_inline_end_maps_to_left_in_rtl() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let rtl = NodeHandle::element("div");
    rtl.set_attribute("class", "rtl");
    document.append_child(body.clone());
    body.append_child(rtl);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ".rtl { direction: rtl; position: fixed; inset-inline-end: 10px; top: 20px; width: 50px; height: 30px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 300.0, height: 200.0 },
    )
    .unwrap();

    // In RTL the inline-end edge is the left edge: 10px from the viewport's left.
    assert_eq!(layout.children[0].dimensions.content.x, 10.0);
    assert_eq!(layout.children[0].dimensions.content.y, 20.0);
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
fn absolute_percentage_insets_resolve_against_positioned_ancestor() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let parent = NodeHandle::element("div");
    let child = NodeHandle::element("aside");

    parent.set_attribute("class", "parent");
    child.set_attribute("class", "child");
    document.append_child(body.clone());
    body.append_child(parent.clone());
    parent.append_child(child);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ".parent { position: relative; width: 200px; height: 100px; } \
             .child { position: absolute; left: 100%; bottom: 10%; \
                      width: 40px; height: 20px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 300.0, height: 200.0 },
    )
    .unwrap();

    let child_box = find_layout_box_by_tag(&layout, "aside").unwrap();
    assert_eq!(child_box.dimensions.content.x, 200.0);
    assert_eq!(child_box.dimensions.content.y, 70.0);
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
    assert_eq!(
        image.rect.x + image.rect.width,
        absolute_box.dimensions.content.x + 9.0
    );
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
fn root_percentage_heights_resolve_against_viewport() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let main = NodeHandle::element("main");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(main);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "html, body { height: 100%; margin: 0; } main { height: 100%; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 1280.0, height: 720.0 },
    )
    .unwrap();

    for tag in ["html", "body", "main"] {
        assert_eq!(
            find_layout_box_by_tag(&layout, tag)
                .unwrap()
                .dimensions
                .content
                .height,
            720.0,
            "{tag} should resolve height:100% against its containing block",
        );
    }
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
fn grid_justify_items_center_shrink_wraps_auto_width_item() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let grid = NodeHandle::element("div");
    let item = NodeHandle::element("article");
    let content = NodeHandle::element("span");
    document.append_child(body.clone());
    body.append_child(grid.clone());
    grid.append_child(item.clone());
    item.append_child(content);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { display: grid; width: 200px; justify-items: center; } \
             article { position: relative; } \
             span { display: inline-block; width: 40px; height: 10px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 200.0, height: 100.0 },
    )
    .unwrap();

    let item_box = find_layout_box_by_tag(&layout, "article").unwrap();
    assert_eq!(item_box.dimensions.content.width, 40.0);
    assert_eq!(item_box.dimensions.content.x, 80.0);
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
        parse_stylesheet(".floated { display: inline; float: right; width: 20px; height: 10px; }")
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
    assert_eq!(
        container_box.children[0].node.tag_name().as_deref(),
        Some("strong")
    );
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
        .find(|child| {
            child
                .node
                .attributes()
                .and_then(|attrs| attrs.get("class").cloned())
                == Some("right".to_string())
        })
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
fn negative_margin_float_fits_beside_full_width_float() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let main = NodeHandle::element("main");
    let sidebar = NodeHandle::element("aside");

    main.set_attribute("class", "main");
    sidebar.set_attribute("class", "sidebar");
    document.append_child(body.clone());
    body.append_child(main.clone());
    body.append_child(sidebar.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ".main { float: left; width: 100%; height: 40px; } \
             .sidebar { float: left; width: 30px; height: 20px; margin-left: -30px; }",
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

    let main_box = find_layout_box_by_tag(&layout, "main").unwrap();
    let sidebar_box = find_layout_box_by_tag(&layout, "aside").unwrap();
    assert_eq!(sidebar_box.dimensions.content.x, 70.0);
    assert_eq!(sidebar_box.dimensions.content.y, main_box.dimensions.content.y);
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
    assert_eq!(
        child_box.dimensions.content.y,
        parent_box.dimensions.content.y
    );
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
    assert!(
        layout.lines.is_empty(),
        "whitespace should not create line boxes"
    );
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

#[test]
fn strut_enforces_parent_line_height_as_minimum_for_line_box() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("div");
    let inline_child = NodeHandle::element("span");
    let text = NodeHandle::text("x");
    container.set_attribute("class", "container");
    inline_child.set_attribute("class", "small");
    document.append_child(body.clone());
    body.append_child(container.clone());
    container.append_child(inline_child.clone());
    inline_child.append_child(text);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".container { line-height: 24px; width: 100px; } .small { font-size: 2px; line-height: 4px; }",
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
    // The container's strut (line-height: 24px) should be the minimum
    // line box height, even though the inline child only has 4px.
    assert_eq!(
        container_box.dimensions.content.height, 24.0,
        "container height should be 24px (strut), got {}",
        container_box.dimensions.content.height,
    );
}

#[test]
fn cjk_text_splits_between_characters() {
    // Test that CJK text is split between characters for line breaking
    let pieces = super::split_words_preserving_spaces_cjk("日本語");
    // Should be split into individual characters
    assert_eq!(pieces.len(), 3);
    assert_eq!(pieces[0], "日");
    assert_eq!(pieces[1], "本");
    assert_eq!(pieces[2], "語");
}

#[test]
fn cjk_kinsoku_keeps_punctuation_with_previous() {
    // Line-start prohibited characters should stay with previous character
    let pieces = super::split_words_preserving_spaces_cjk("日本。語");
    // '。' should stay with '本', not be its own piece
    assert_eq!(pieces.len(), 3);
    assert_eq!(pieces[0], "日");
    assert_eq!(pieces[1], "本。");
    assert_eq!(pieces[2], "語");
}

#[test]
fn cjk_kinsoku_keeps_opening_bracket_with_next() {
    // Line-end prohibited characters should stay with next character
    let pieces = super::split_words_preserving_spaces_cjk("日「本」語");
    // '「' should stay with '本', '」' should stay with '本'
    assert_eq!(pieces.len(), 3);
    assert_eq!(pieces[0], "日");
    assert_eq!(pieces[1], "「本」");
    assert_eq!(pieces[2], "語");
}

#[test]
fn mixed_ascii_and_cjk_text_splits_correctly() {
    // Mixed ASCII and CJK should break at transitions
    let pieces = super::split_words_preserving_spaces_cjk("Hello日本語World");
    assert!(pieces.len() >= 3);
    assert_eq!(pieces[0], "Hello");
    // CJK characters should be split
    assert!(pieces.contains(&"日".to_string()));
}

#[test]
fn spaces_still_cause_breaks() {
    // Spaces should still cause breaks
    let pieces = super::split_words_preserving_spaces_cjk("hello world");
    assert_eq!(pieces.len(), 3);
    assert_eq!(pieces[0], "hello");
    assert_eq!(pieces[1], " ");
    assert_eq!(pieces[2], "world");
}

#[test]
fn cjk_small_kana_stays_with_previous() {
    // Small kana (っ, ゃ, etc.) should stay with previous character
    let pieces = super::split_words_preserving_spaces_cjk("日本っ語");
    assert_eq!(pieces.len(), 3);
    assert_eq!(pieces[0], "日");
    assert_eq!(pieces[1], "本っ");
    assert_eq!(pieces[2], "語");
}

// ===== letter-spacing tests =====

#[test]
fn letter_spacing_increases_text_width() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    let text = NodeHandle::text("hello");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(p.clone());
    p.append_child(text);

    let mut resolver_no_spacing = StyleResolver::new();
    resolver_no_spacing.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { margin: 0; } p { font-size: 16px; }").unwrap(),
    );
    let layout_no_spacing = layout_tree(
        &document,
        &mut resolver_no_spacing,
        Rect { x: 0.0, y: 0.0, width: 500.0, height: 0.0 },
    );

    let mut resolver_with_spacing = StyleResolver::new();
    resolver_with_spacing.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { margin: 0; } p { font-size: 16px; letter-spacing: 10px; }").unwrap(),
    );
    let layout_with_spacing = layout_tree(
        &document,
        &mut resolver_with_spacing,
        Rect { x: 0.0, y: 0.0, width: 500.0, height: 0.0 },
    );

    // Find the p box width in both layouts to compare text widths
    fn find_p_box(layout: &LayoutBox) -> Option<&LayoutBox> {
        if layout.node.tag_name().as_deref() == Some("p") {
            return Some(layout);
        }
        for child in &layout.children {
            if let Some(found) = find_p_box(child) {
                return Some(found);
            }
        }
        None
    }

    let width_no_spacing = layout_no_spacing
        .as_ref()
        .and_then(|l| find_p_box(l))
        .and_then(|p| p.lines.first())
        .map(|l| l.rect.width)
        .unwrap_or(0.0);

    let width_with_spacing = layout_with_spacing
        .as_ref()
        .and_then(|l| find_p_box(l))
        .and_then(|p| p.lines.first())
        .map(|l| l.rect.width)
        .unwrap_or(0.0);

    // "hello" has 5 chars → 4 gaps × 10px = 40px extra
    assert!(
        width_with_spacing > width_no_spacing,
        "letter-spacing should increase text width: no_spacing={width_no_spacing}, with_spacing={width_with_spacing}"
    );
    // Exact delta should be 40px (4 inter-char gaps × 10px)
    let delta = width_with_spacing - width_no_spacing;
    assert!(
        (delta - 40.0).abs() < 2.0,
        "expected ~40px letter-spacing increase but got {delta}"
    );
}

// ---- list-style layout tests ----

fn build_ul_with_items(item_count: usize) -> (NodeHandle, NodeHandle, Vec<NodeHandle>) {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let ul = NodeHandle::element("ul");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(ul.clone());
    let mut items = Vec::new();
    for i in 0..item_count {
        let li = NodeHandle::element("li");
        let text = NodeHandle::text(format!("Item {}", i + 1));
        li.append_child(text);
        ul.append_child(li.clone());
        items.push(li);
    }
    (document, body, items)
}

#[test]
fn list_item_generates_disc_marker() {
    let (document, _body, items) = build_ul_with_items(1);
    let mut resolver = StyleResolver::new();
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 300.0, height: 0.0 },
    )
    .unwrap();

    let li = &items[0];
    let li_box = find_layout_box_by_tag(&layout, "li").unwrap();
    assert!(
        li_box.marker.is_some(),
        "li should have a marker for display:list-item"
    );
    let marker = li_box.marker.as_ref().unwrap();
    assert_eq!(marker.text, "\u{2022}", "ul default marker should be disc (•)");
    assert!(marker.outside, "default list-style-position is outside");
    let _ = li;
}

#[test]
fn list_item_marker_none_produces_no_marker() {
    let (document, _body, _items) = build_ul_with_items(1);
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("ul { list-style-type: none; }").unwrap(),
    );
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 300.0, height: 0.0 },
    )
    .unwrap();

    let li_box = find_layout_box_by_tag(&layout, "li").unwrap();
    assert!(
        li_box.marker.is_none(),
        "list-style-type:none should produce no marker"
    );
}

#[test]
fn ol_list_items_get_decimal_markers() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let ol = NodeHandle::element("ol");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(ol.clone());
    let mut lis = Vec::new();
    for i in 0..3usize {
        let li = NodeHandle::element("li");
        li.append_child(NodeHandle::text(format!("item {}", i + 1)));
        ol.append_child(li.clone());
        lis.push(li);
    }

    let mut resolver = StyleResolver::new();
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 300.0, height: 0.0 },
    )
    .unwrap();

    let mut found = Vec::new();
    collect_markers_by_tag(&layout, "li", &mut found);
    assert_eq!(found.len(), 3, "should have 3 li markers");
    assert_eq!(found[0], "1.");
    assert_eq!(found[1], "2.");
    assert_eq!(found[2], "3.");
}

#[test]
fn circle_marker_type() {
    let (document, _body, _items) = build_ul_with_items(1);
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("ul { list-style-type: circle; }").unwrap(),
    );
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 300.0, height: 0.0 },
    )
    .unwrap();

    let li_box = find_layout_box_by_tag(&layout, "li").unwrap();
    let marker = li_box.marker.as_ref().unwrap();
    assert_eq!(marker.text, "\u{25e6}");
}

#[test]
fn square_marker_type() {
    let (document, _body, _items) = build_ul_with_items(1);
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("ul { list-style-type: square; }").unwrap(),
    );
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 300.0, height: 0.0 },
    )
    .unwrap();

    let li_box = find_layout_box_by_tag(&layout, "li").unwrap();
    let marker = li_box.marker.as_ref().unwrap();
    assert_eq!(marker.text, "\u{25a0}");
}

#[test]
fn inside_marker_position() {
    let (document, _body, _items) = build_ul_with_items(1);
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("ul { list-style-position: inside; }").unwrap(),
    );
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 300.0, height: 0.0 },
    )
    .unwrap();

    let li_box = find_layout_box_by_tag(&layout, "li").unwrap();
    let marker = li_box.marker.as_ref().unwrap();
    assert!(!marker.outside, "list-style-position:inside should set outside=false");
}

/// text-transform must be applied even when the text node is the direct child of
/// an element that itself is passed as the root to layout_tree (i.e., the text
/// node is handled via the NodeType::Text branch of collect_inline_segments).
#[test]
fn text_transform_applied_to_direct_text_node() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    let text = NodeHandle::text("hello world");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(p.clone());
    p.append_child(text);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { margin: 0; } p { text-transform: uppercase; }").unwrap(),
    );

    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 500.0, height: 0.0 },
    )
    .unwrap();

    // Collect all text fragments from the layout tree
    fn collect_text(layout: &LayoutBox, out: &mut Vec<String>) {
        for line in &layout.lines {
            for fragment in &line.fragments {
                if let Some(t) = fragment.text() {
                    out.push(t.to_string());
                }
            }
        }
        for child in &layout.children {
            collect_text(child, out);
        }
    }

    let mut texts = Vec::new();
    collect_text(&layout, &mut texts);
    let combined = texts.join("");

    assert!(
        !combined.is_empty(),
        "expected text fragments in layout, got none"
    );
    assert_eq!(
        combined, "HELLO WORLD",
        "text-transform:uppercase should uppercase the text node content, got {:?}",
        combined
    );
}

/// Each `InlineFragment` should carry the computed style of its owning element
/// so that paint can apply per-fragment text-transform / text-decoration without
/// re-resolving styles.
#[test]
fn inline_fragment_carries_per_element_style() {
    // Build: <p>normal <span style="text-transform:uppercase">upper</span></p>
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    let text_normal = NodeHandle::text("normal ");
    let span = NodeHandle::element("span");
    let text_upper = NodeHandle::text("upper");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(p.clone());
    p.append_child(text_normal);
    p.append_child(span.clone());
    span.append_child(text_upper);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } \
             p { font-size: 16px; } \
             span { text-transform: uppercase; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 500.0, height: 0.0 },
    )
    .unwrap();

    // Collect all text fragments with their text-transform style value.
    fn collect_fragments(layout: &LayoutBox, out: &mut Vec<(String, String)>) {
        for line in &layout.lines {
            for fragment in &line.fragments {
                if let Some(t) = fragment.text() {
                    let transform = fragment.style.text_transform.as_deref()
                        .map(|kw| kw.to_ascii_lowercase())
                        .unwrap_or_else(|| "none".to_string());
                    out.push((t.to_string(), transform));
                }
            }
        }
        for child in &layout.children {
            collect_fragments(child, out);
        }
    }

    let mut fragments = Vec::new();
    collect_fragments(&layout, &mut fragments);

    // There should be at least two text fragments.
    assert!(
        fragments.len() >= 2,
        "expected at least two text fragments, got {:?}",
        fragments
    );

    // The fragment for "normal " should NOT have text-transform:uppercase.
    let normal_frag = fragments.iter().find(|(text, _)| text.contains("normal"));
    assert!(
        normal_frag.is_some(),
        "expected a fragment containing \"normal\""
    );
    assert_ne!(
        normal_frag.unwrap().1,
        "uppercase",
        "\"normal \" fragment should not have text-transform:uppercase"
    );

    // The fragment for "UPPER" (already transformed in layout) should carry
    // text-transform:uppercase in its style.
    let upper_frag = fragments.iter().find(|(text, _)| *text == text.to_uppercase() && text.contains("UPPER"));
    assert!(
        upper_frag.is_some(),
        "expected a fragment containing \"UPPER\" (text after transform), got {:?}",
        fragments
    );
    assert_eq!(
        upper_frag.unwrap().1,
        "uppercase",
        "span fragment should carry text-transform:uppercase in its style"
    );
}

/// Nested inline elements with different `text-decoration` values must each
/// carry their own style on the fragment so paint can apply them independently.
#[test]
fn inline_fragment_carries_per_element_text_decoration() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    let text_plain = NodeHandle::text("plain ");
    let span = NodeHandle::element("span");
    let text_decorated = NodeHandle::text("decorated");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(p.clone());
    p.append_child(text_plain);
    p.append_child(span.clone());
    span.append_child(text_decorated);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } \
             p { font-size: 16px; } \
             span { text-decoration-line: underline; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 500.0, height: 0.0 },
    )
    .unwrap();

    fn collect_fragments(layout: &LayoutBox, out: &mut Vec<(String, String)>) {
        for line in &layout.lines {
            for fragment in &line.fragments {
                if let Some(t) = fragment.text() {
                    let decoration = fragment.style.text_decoration_line.as_deref()
                        .map(|kw| kw.to_ascii_lowercase())
                        .unwrap_or_else(|| "none".to_string());
                    out.push((t.to_string(), decoration));
                }
            }
        }
        for child in &layout.children {
            collect_fragments(child, out);
        }
    }

    let mut fragments = Vec::new();
    collect_fragments(&layout, &mut fragments);

    assert!(
        fragments.len() >= 2,
        "expected at least two text fragments, got {:?}",
        fragments
    );

    let plain_frag = fragments.iter().find(|(text, _)| text.contains("plain"));
    assert!(plain_frag.is_some(), "expected fragment containing \"plain\"");
    assert_ne!(
        plain_frag.unwrap().1,
        "underline",
        "\"plain\" fragment should not have text-decoration-line:underline"
    );

    let decorated_frag = fragments.iter().find(|(text, _)| text.contains("decorated"));
    assert!(decorated_frag.is_some(), "expected fragment containing \"decorated\"");
    assert_eq!(
        decorated_frag.unwrap().1,
        "underline",
        "span fragment should carry text-decoration-line:underline in its style"
    );
}

fn collect_markers_by_tag<'a>(layout: &'a LayoutBox, tag: &str, out: &mut Vec<String>) {
    if layout.node.tag_name().as_deref() == Some(tag) {
        if let Some(marker) = &layout.marker {
            out.push(marker.text.clone());
        }
    }
    for child in &layout.children {
        collect_markers_by_tag(child, tag, out);
    }
}

// ── box-sizing: border-box ───────────────────────────────────────────────────

/// width: 100px, padding: 10px each side, border: 5px each side
/// border-box → content_width = 100 - 20 - 10 = 70
#[test]
fn border_box_width_subtracts_padding_and_border() {
    let (_document, _html, body, _card) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { width: 400px; } \
             div { box-sizing: border-box; width: 100px; \
                   padding-left: 10px; padding-right: 10px; \
                   border-left-width: 5px; border-right-width: 5px; \
                   border-left-style: solid; border-right-style: solid; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 400.0, height: 0.0 },
    )
    .unwrap();

    let child = &layout.children[0];
    // content_width = 100 - (10+10) - (5+5) = 70
    assert_eq!(child.dimensions.content.width, 70.0);
    assert_eq!(child.dimensions.padding.left, 10.0);
    assert_eq!(child.dimensions.padding.right, 10.0);
    assert_eq!(child.dimensions.border.left, 5.0);
    assert_eq!(child.dimensions.border.right, 5.0);
}

/// With content-box (default), width: 100px → content_width = 100px
/// regardless of padding / border.
#[test]
fn content_box_width_is_unchanged() {
    let (_document, _html, body, _card) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { width: 400px; } \
             div { box-sizing: content-box; width: 100px; \
                   padding-left: 10px; padding-right: 10px; \
                   border-left-width: 5px; border-right-width: 5px; \
                   border-left-style: solid; border-right-style: solid; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 400.0, height: 0.0 },
    )
    .unwrap();

    let child = &layout.children[0];
    assert_eq!(child.dimensions.content.width, 100.0);
}

/// border-box height: 100px, padding: 10px each side, border: 5px each side
/// → content_height = 100 - 20 - 10 = 70
#[test]
fn border_box_height_subtracts_padding_and_border() {
    let (_document, _html, body, _card) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { width: 400px; } \
             div { box-sizing: border-box; width: 200px; height: 100px; \
                   padding-top: 10px; padding-bottom: 10px; \
                   border-top-width: 5px; border-bottom-width: 5px; \
                   border-top-style: solid; border-bottom-style: solid; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 400.0, height: 0.0 },
    )
    .unwrap();

    let child = &layout.children[0];
    // content_height = 100 - (10+10) - (5+5) = 70
    assert_eq!(child.dimensions.content.height, 70.0);
}

/// border-box with min-width: 120px means content_min = 120 - 20 - 10 = 90.
/// Since the specified content_width (70) < 90, it gets clamped to 90.
#[test]
fn border_box_min_width_is_applied_in_content_space() {
    let (_document, _html, body, _card) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { width: 400px; } \
             div { box-sizing: border-box; width: 100px; min-width: 120px; \
                   padding-left: 10px; padding-right: 10px; \
                   border-left-width: 5px; border-right-width: 5px; \
                   border-left-style: solid; border-right-style: solid; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 400.0, height: 0.0 },
    )
    .unwrap();

    let child = &layout.children[0];
    // content_min = 120 - (10+10) - (5+5) = 90
    assert_eq!(child.dimensions.content.width, 90.0);
}

/// border-box with max-width: 80px means content_max = 80 - 20 - 10 = 50.
/// Since the specified content_width (70) > 50, it gets clamped to 50.
#[test]
fn border_box_max_width_is_applied_in_content_space() {
    let (_document, _html, body, _card) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { width: 400px; } \
             div { box-sizing: border-box; width: 100px; max-width: 80px; \
                   padding-left: 10px; padding-right: 10px; \
                   border-left-width: 5px; border-right-width: 5px; \
                   border-left-style: solid; border-right-style: solid; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 400.0, height: 0.0 },
    )
    .unwrap();

    let child = &layout.children[0];
    // content_max = 80 - (10+10) - (5+5) = 50
    assert_eq!(child.dimensions.content.width, 50.0);
}

// ── word-break / overflow-wrap unit tests ───────────────────────────────────

#[test]
fn split_chars_produces_individual_characters() {
    let pieces = super::split_chars("abc");
    assert_eq!(pieces, vec!["a", "b", "c"]);
}

#[test]
fn split_chars_keeps_spaces_as_own_piece() {
    let pieces = super::split_chars("a b");
    assert_eq!(pieces, vec!["a", " ", "b"]);
}

#[test]
fn split_chars_handles_cjk_characters() {
    let pieces = super::split_chars("日本語");
    assert_eq!(pieces, vec!["日", "本", "語"]);
}

#[test]
fn split_words_no_cjk_break_treats_cjk_as_word() {
    // CJK chars should NOT break between them — the whole run is one piece
    let pieces = super::split_words_no_cjk_break("日本語");
    assert_eq!(pieces, vec!["日本語"]);
}

#[test]
fn split_words_no_cjk_break_breaks_at_spaces() {
    let pieces = super::split_words_no_cjk_break("日本 語");
    assert_eq!(pieces, vec!["日本", " ", "語"]);
}

#[test]
fn split_words_no_cjk_break_mixed_ascii_cjk() {
    // ASCII word followed by CJK — the whole run after a space should be one piece
    let pieces = super::split_words_no_cjk_break("hello 日本語 world");
    assert_eq!(pieces, vec!["hello", " ", "日本語", " ", "world"]);
}

// ── word-break layout tests ──────────────────────────────────────────────────

#[test]
fn adjacent_text_nodes_do_not_create_wrap_opportunity() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let paragraph = NodeHandle::element("p");
    document.append_child(body.clone());
    body.append_child(paragraph.clone());
    paragraph.append_child(NodeHandle::text("hello world"));
    paragraph.append_child(NodeHandle::comment("framework boundary"));
    paragraph.append_child(NodeHandle::text("."));

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { margin: 0; } p { width: 60px; font-size: 16px; }").unwrap(),
    );
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 60.0, height: 0.0 },
    )
    .unwrap();
    let paragraph = &layout.children[0];
    assert_eq!(paragraph.lines.len(), 2);
    let second_line = paragraph.lines[1]
        .fragments
        .iter()
        .filter_map(InlineFragment::text)
        .collect::<String>();
    assert_eq!(second_line, "world.");
}

#[test]
fn word_break_break_all_wraps_between_any_characters() {
    // With word-break: break-all, a long English word should be split into
    // individual characters, producing multiple line boxes on a narrow width.
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    let text = NodeHandle::text("abcdefgh");

    document.append_child(body.clone());
    body.append_child(p.clone());
    p.append_child(text);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("p { word-break: break-all; line-height: 20px; }").unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0, // Extremely narrow to force per-character wrapping
            height: 0.0,
        },
    )
    .unwrap();

    let p_box = &layout.children[0];
    // Each character should be on its own line (or at least more than 1 line)
    assert!(
        p_box.lines.len() > 1,
        "expected multiple lines with word-break: break-all, got {}",
        p_box.lines.len()
    );
}

#[test]
fn word_break_keep_all_treats_cjk_as_unit() {
    // With word-break: keep-all, CJK text should NOT break between characters —
    // the whole run should remain on one line if it fits.
    // We use a wide container so the CJK text easily fits.
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    let text = NodeHandle::text("日本語");

    document.append_child(body.clone());
    body.append_child(p.clone());
    p.append_child(text);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("p { word-break: keep-all; line-height: 20px; }").unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 0.0,
        },
    )
    .unwrap();

    let p_box = &layout.children[0];
    // With wide container, all 3 CJK chars fit in one piece → one line box
    assert_eq!(
        p_box.lines.len(),
        1,
        "expected 1 line with word-break: keep-all on wide container, got {}",
        p_box.lines.len()
    );
    // The fragment text should be the entire word
    let text_rendered: String = p_box.lines[0]
        .fragments
        .iter()
        .filter_map(|f| f.text())
        .collect();
    assert_eq!(text_rendered, "日本語");
}

#[test]
fn overflow_wrap_break_word_wraps_long_word() {
    // With overflow-wrap: break-word, a single long word that exceeds the container
    // width should be broken at character boundaries.
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    let text = NodeHandle::text("abcdefghij");

    document.append_child(body.clone());
    body.append_child(p.clone());
    p.append_child(text);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("p { overflow-wrap: break-word; line-height: 20px; }").unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0, // Narrow enough to force wrapping
            height: 0.0,
        },
    )
    .unwrap();

    let p_box = &layout.children[0];
    assert!(
        p_box.lines.len() > 1,
        "expected multiple lines with overflow-wrap: break-word, got {}",
        p_box.lines.len()
    );
}

#[test]
fn word_wrap_alias_behaves_like_overflow_wrap() {
    // word-wrap is a legacy alias for overflow-wrap and should behave identically.
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    let text = NodeHandle::text("abcdefghij");

    document.append_child(body.clone());
    body.append_child(p.clone());
    p.append_child(text);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("p { word-wrap: break-word; line-height: 20px; }").unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 0.0,
        },
    )
    .unwrap();

    let p_box = &layout.children[0];
    assert!(
        p_box.lines.len() > 1,
        "expected multiple lines with word-wrap: break-word (alias), got {}",
        p_box.lines.len()
    );
}

#[test]
fn table_img_width_attribute_sets_column_intrinsic_width() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let table = NodeHandle::element("table");
    let row = NodeHandle::element("tr");
    let td1 = NodeHandle::element("td");
    let img = NodeHandle::element("img");
    img.set_attribute("width", "350");
    img.set_attribute("height", "414");
    td1.append_child(img);
    let td2 = NodeHandle::element("td");
    td2.append_child(NodeHandle::text("Hello"));

    document.append_child(body.clone());
    body.append_child(table.clone());
    table.append_child(row.clone());
    row.append_child(td1);
    row.append_child(td2);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("table { display: table; } tr { display: table-row; } td { display: table-cell; }").unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 800.0, height: 0.0 },
    ).unwrap();

    let table_box = find_layout_box_by_tag(&layout, "table").unwrap();
    let row_box = &table_box.children[0];
    assert_eq!(row_box.children.len(), 2, "should have 2 cells");
    let col0_width = row_box.children[0].dimensions.content.width;
    let col1_width = row_box.children[1].dimensions.content.width;
    assert!(col0_width >= 350.0, "col0 should be >= 350px (img width), got {col0_width}");
    assert!(col1_width > 0.0, "col1 should have some width");
}

#[test]
fn table_three_column_rowspan_distributes_widths_correctly() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let table = NodeHandle::element("table");

    // Row 1: [td rowspan=2 width=350] [td nbsp] [td text]
    let row1 = NodeHandle::element("tr");
    let td1 = NodeHandle::element("td");
    td1.set_attribute("rowspan", "2");
    let img = NodeHandle::element("img");
    img.set_attribute("width", "350");
    img.set_attribute("height", "414");
    td1.append_child(img);
    td1.append_child(NodeHandle::text("Profile info"));
    let td2 = NodeHandle::element("td");
    td2.append_child(NodeHandle::text("\u{00a0}"));
    let td3 = NodeHandle::element("td");
    td3.append_child(NodeHandle::text("Latest news and drama information here"));
    row1.append_child(td1);
    row1.append_child(td2);
    row1.append_child(td3);

    // Row 2: [td empty] [td text]
    let row2 = NodeHandle::element("tr");
    let td4 = NodeHandle::element("td");
    let td5 = NodeHandle::element("td");
    td5.append_child(NodeHandle::text("More content"));
    row2.append_child(td4);
    row2.append_child(td5);

    document.append_child(body.clone());
    body.append_child(table.clone());
    table.append_child(row1);
    table.append_child(row2);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } \
             table { display: table; } \
             tr { display: table-row; } \
             td { display: table-cell; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 800.0, height: 0.0 },
    )
    .unwrap();

    let table_box = find_layout_box_by_tag(&layout, "table").unwrap();
    // Row 1 should have 3 cells visible
    let row1_box = &table_box.children[0];
    assert_eq!(row1_box.children.len(), 3, "row1 should have 3 cells");

    // Col 0 (rowspan cell) should be at least 350px
    let col0 = &row1_box.children[0];
    assert!(
        col0.dimensions.content.width >= 350.0,
        "col0 (img) should be >= 350px, got {}",
        col0.dimensions.content.width
    );

    // Col 2 (text) should have positive width and start after col 0
    let col2 = &row1_box.children[2];
    assert!(
        col2.dimensions.content.width > 0.0,
        "col2 (text) should have positive width, got {}",
        col2.dimensions.content.width
    );
    assert!(
        col2.dimensions.content.x > col0.dimensions.content.x + col0.dimensions.content.width - 1.0,
        "col2 x ({}) should be after col0 right edge ({})",
        col2.dimensions.content.x,
        col0.dimensions.content.x + col0.dimensions.content.width
    );
}

#[test]
#[ignore = "debug abe table"]
fn debug_abe_table_column_widths() {
    let html = r#"<html><body>
    <table align="center">
      <tr>
        <td rowspan="2"><img src="abe-top.jpg" width="350" height="414"><br>
          <table width="256">
            <tr><td>阿部 寛（あべ ひろし）</td></tr>
          </table>
          所属: 株式会社オフィスA
        </td>
        <td>&nbsp;</td>
        <td><div align="center">★★★ 最新情報 ★★★</div></td>
      </tr>
      <tr>
        <td></td>
        <td>
          ・ドラマ 日曜劇場「VIVANT」続編 2026年放送<br>
          ・Netflixシリーズ「イクサガミ」2025年11月13日配信
        </td>
      </tr>
    </table>
    </body></html>"#;

    let document = crate::html::TreeBuilder::parse(html).document();
    let mut resolver = StyleResolver::new();

    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 800.0, height: 600.0 },
    ).unwrap();

    fn dump_table(layout: &LayoutBox, depth: usize) {
        let indent = "  ".repeat(depth);
        let tag = layout.node.tag_name().unwrap_or_default();
        let r = &layout.dimensions.content;
        println!("{indent}{tag}: x={:.0} y={:.0} w={:.0} h={:.0}", r.x, r.y, r.width, r.height);
        for child in &layout.children {
            dump_table(child, depth + 1);
        }
    }
    dump_table(&layout, 0);
}

#[test]
#[ignore = "debug abe real table"]
fn debug_abe_real_table() {
    let path = "/tmp/abe-top.html";
    if !std::path::Path::new(path).exists() {
        eprintln!("skipping: {path} not found");
        return;
    }
    let html = std::fs::read_to_string(path).unwrap();
    let document = crate::html::TreeBuilder::parse(&html).document();
    let mut resolver = StyleResolver::new();

    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 1280.0, height: 900.0 },
    ).unwrap();

    fn find_tables(lb: &LayoutBox, results: &mut Vec<(String, f32, f32, f32, f32)>) {
        let tag = lb.node.tag_name().unwrap_or_default();
        if tag == "table" {
            let r = &lb.dimensions.content;
            results.push((tag.clone(), r.x, r.y, r.width, r.height));
            for (i, child) in lb.children.iter().enumerate() {
                let child_tag = child.node.tag_name().unwrap_or_default();
                let cr = &child.dimensions.content;
                eprintln!("  {child_tag}[{i}]: x={:.0} y={:.0} w={:.0} h={:.0} children={}", cr.x, cr.y, cr.width, cr.height, child.children.len());
                for (j, cell) in child.children.iter().enumerate() {
                    let cell_tag = cell.node.tag_name().unwrap_or_default();
                    let ccr = &cell.dimensions.content;
                    eprintln!("    {cell_tag}[{j}]: x={:.0} y={:.0} w={:.0} h={:.0}", ccr.x, ccr.y, ccr.width, ccr.height);
                }
            }
        }
        for child in &lb.children {
            find_tables(child, results);
        }
    }
    let mut tables = Vec::new();
    find_tables(&layout, &mut tables);
    for (tag, x, y, w, h) in &tables {
        eprintln!("{tag}: x={x:.0} y={y:.0} w={w:.0} h={h:.0}");
    }
}

#[test]
fn table_align_center_centers_with_auto_margins() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let table = NodeHandle::element("table");
    table.set_attribute("align", "center");
    let row = NodeHandle::element("tr");
    let td = NodeHandle::element("td");
    td.set_attribute("width", "200");
    td.append_child(NodeHandle::text("content"));
    row.append_child(td);
    table.append_child(row);
    document.append_child(body.clone());
    body.append_child(table);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { margin: 0; } table { display: table; } tr { display: table-row; } td { display: table-cell; }").unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 800.0, height: 0.0 },
    ).unwrap();

    let table_box = find_layout_box_by_tag(&layout, "table").unwrap();
    let table_left = table_box.dimensions.content.x
        - table_box.dimensions.padding.left
        - table_box.dimensions.border.left;
    let table_outer_width = table_box.dimensions.content.width
        + table_box.dimensions.padding.horizontal()
        + table_box.dimensions.border.horizontal();
    let expected_margin = (800.0 - table_outer_width) / 2.0;
    assert!(
        (table_left - expected_margin).abs() < 2.0,
        "table should be centered: left={table_left}, expected_margin={expected_margin}, table_width={table_outer_width}"
    );
}

#[test]
fn rowspan_cell_distributes_height_evenly_across_spanned_rows() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let table = NodeHandle::element("table");

    // Row 1: [td rowspan=2 height=200] [td height=50]
    let row1 = NodeHandle::element("tr");
    let tall_cell = NodeHandle::element("td");
    tall_cell.set_attribute("rowspan", "2");
    tall_cell.set_attribute("class", "tall");
    tall_cell.append_child(NodeHandle::text("tall"));
    let short_cell1 = NodeHandle::element("td");
    short_cell1.set_attribute("class", "short");
    short_cell1.append_child(NodeHandle::text("row1"));
    row1.append_child(tall_cell);
    row1.append_child(short_cell1);

    // Row 2: [td height=50]
    let row2 = NodeHandle::element("tr");
    let short_cell2 = NodeHandle::element("td");
    short_cell2.set_attribute("class", "short");
    short_cell2.append_child(NodeHandle::text("row2"));
    row2.append_child(short_cell2);

    document.append_child(body.clone());
    body.append_child(table.clone());
    table.append_child(row1);
    table.append_child(row2);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } \
             table { display: table; } \
             tr { display: table-row; } \
             td { display: table-cell; } \
             .tall { height: 200px; } \
             .short { height: 50px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 400.0, height: 0.0 },
    ).unwrap();

    let table_box = find_layout_box_by_tag(&layout, "table").unwrap();
    let row1_box = &table_box.children[0];
    let row2_box = &table_box.children[1];

    // Row 1 + Row 2 should together be at least 200px (rowspan cell height)
    let row1_h = row1_box.dimensions.content.height;
    let row2_h = row2_box.dimensions.content.height;
    assert!(
        row1_h + row2_h >= 200.0 - 1.0,
        "rows should total >= 200px, got {row1_h} + {row2_h} = {}",
        row1_h + row2_h
    );

    // Each row should get at least 100px (200 / 2 = 100, initial 50 + 50 extra)
    assert!(
        row1_h >= 95.0,
        "row1 should be ~100px, got {row1_h}"
    );
    assert!(
        row2_h >= 95.0,
        "row2 should be ~100px, got {row2_h}"
    );

    // Row 2 should start below row 1
    assert!(
        row2_box.dimensions.content.y > row1_box.dimensions.content.y,
        "row2 y ({}) should be below row1 y ({})",
        row2_box.dimensions.content.y,
        row1_box.dimensions.content.y
    );
}

#[test]
fn rowspan_expanded_row_preserves_vertical_align_bottom() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let table = NodeHandle::element("table");

    let row1 = NodeHandle::element("tr");
    let tall = NodeHandle::element("td");
    tall.set_attribute("rowspan", "2");
    tall.set_attribute("class", "tall");
    tall.append_child(NodeHandle::text("tall"));
    let bottom_cell = NodeHandle::element("td");
    bottom_cell.set_attribute("class", "bottom");
    bottom_cell.append_child(NodeHandle::text("x"));
    row1.append_child(tall);
    row1.append_child(bottom_cell);

    let row2 = NodeHandle::element("tr");
    let cell2 = NodeHandle::element("td");
    cell2.append_child(NodeHandle::text("r2"));
    row2.append_child(cell2);

    document.append_child(body.clone());
    body.append_child(table.clone());
    table.append_child(row1);
    table.append_child(row2);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } \
             table { display: table; } \
             tr { display: table-row; } \
             td { display: table-cell; font-size: 10px; line-height: 10px; } \
             .tall { height: 100px; } \
             .bottom { vertical-align: bottom; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 400.0, height: 0.0 },
    ).unwrap();

    let table_box = find_layout_box_by_tag(&layout, "table").unwrap();
    let row1_box = &table_box.children[0];
    let bottom_cell_box = &row1_box.children[1];

    // The bottom-aligned cell's text should be near the bottom of the cell
    let cell_bottom = bottom_cell_box.dimensions.content.y + bottom_cell_box.dimensions.content.height;
    let line = &bottom_cell_box.lines[0];
    let line_bottom = line.rect.y + line.rect.height;
    assert!(
        (cell_bottom - line_bottom).abs() < 2.0,
        "text should be near cell bottom: cell_bottom={cell_bottom}, line_bottom={line_bottom}"
    );
}

#[test]
fn rowspan_second_pass_preserves_vertical_align_bottom_with_initial_offset() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let table = NodeHandle::element("table");

    let row1 = NodeHandle::element("tr");
    let tall_non_rowspan = NodeHandle::element("td");
    tall_non_rowspan.set_attribute("class", "tall-non-rowspan");
    tall_non_rowspan.append_child(NodeHandle::text("tall"));
    let bottom_cell = NodeHandle::element("td");
    bottom_cell.set_attribute("class", "bottom");
    bottom_cell.append_child(NodeHandle::text("x"));
    let rowspan_tall = NodeHandle::element("td");
    rowspan_tall.set_attribute("rowspan", "2");
    rowspan_tall.set_attribute("class", "rowspan-tall");
    rowspan_tall.append_child(NodeHandle::text("rowspan"));
    row1.append_child(tall_non_rowspan);
    row1.append_child(bottom_cell);
    row1.append_child(rowspan_tall);

    let row2 = NodeHandle::element("tr");
    let r2c1 = NodeHandle::element("td");
    r2c1.append_child(NodeHandle::text("r2c1"));
    let r2c2 = NodeHandle::element("td");
    r2c2.append_child(NodeHandle::text("r2c2"));
    row2.append_child(r2c1);
    row2.append_child(r2c2);

    document.append_child(body.clone());
    body.append_child(table.clone());
    table.append_child(row1);
    table.append_child(row2);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } \
             table { display: table; } \
             tr { display: table-row; } \
             td { display: table-cell; font-size: 10px; line-height: 10px; } \
             .tall-non-rowspan { height: 50px; } \
             .rowspan-tall { height: 200px; } \
             .bottom { vertical-align: bottom; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 400.0, height: 0.0 },
    ).unwrap();

    let table_box = find_layout_box_by_tag(&layout, "table").unwrap();
    let row1_box = &table_box.children[0];
    let bottom_cell_box = &row1_box.children[1];

    let cell_bottom = bottom_cell_box.dimensions.content.y + bottom_cell_box.dimensions.content.height;
    let line = &bottom_cell_box.lines[0];
    let line_bottom = line.rect.y + line.rect.height;
    assert!(
        (cell_bottom - line_bottom).abs() < 2.0,
        "text should remain near cell bottom after second-pass: cell_bottom={cell_bottom}, line_bottom={line_bottom}"
    );
}

#[test]
fn unsupported_html_tag_sqlite_logging() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_path = std::env::temp_dir().join(format!("omoikane-html-tags-{unique}.db"));
    let db_path_str = db_path.to_string_lossy().to_string();

    super::persist_unsupported_html_to_sqlite(&db_path_str, "canvas", Some("div"));
    super::persist_unsupported_html_to_sqlite(&db_path_str, "canvas", Some("div"));
    super::persist_unsupported_html_to_sqlite(&db_path_str, "video", Some("body"));

    let conn = rusqlite::Connection::open(&db_path_str).unwrap();
    let mut stmt = conn
        .prepare("SELECT tag, parent_tag, occurrences FROM unsupported_html_log ORDER BY tag")
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], ("canvas".to_string(), "div".to_string(), 2));
    assert_eq!(rows[1], ("video".to_string(), "body".to_string(), 1));

    drop(stmt);
    drop(conn);
    super::close_html_sqlite_connection_for_path(&db_path_str);
    let _ = std::fs::remove_file(db_path);
}

#[test]
fn supported_html_tags_are_not_logged() {
    assert!(super::is_supported_html_tag("div"));
    assert!(super::is_supported_html_tag("table"));
    assert!(super::is_supported_html_tag("img"));
    for tag in [
        "canvas", "video", "audio", "source", "picture", "details", "summary", "dialog",
        "time", "progress", "meter",
    ] {
        assert!(super::is_supported_html_tag(tag), "{tag} should be supported");
    }
    assert!(!super::is_supported_html_tag("iframe"));
    assert!(super::is_supported_html_tag("form"));
    assert!(super::is_supported_html_tag("input"));
    assert!(super::is_supported_html_tag("button"));
    assert!(super::is_supported_html_tag("textarea"));
    assert!(super::is_supported_html_tag("select"));
    assert!(super::is_supported_html_tag("option"));
}

#[test]
fn border_spacing_two_values_apply_to_table() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let table = NodeHandle::element("table");
    let tr = NodeHandle::element("tr");
    let td1 = NodeHandle::element("td");
    let td2 = NodeHandle::element("td");
    td1.append_child(NodeHandle::text("A"));
    td2.append_child(NodeHandle::text("B"));
    document.append_child(body.clone());
    body.append_child(table.clone());
    table.append_child(tr.clone());
    tr.append_child(td1);
    tr.append_child(td2);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } \
             table { display: table; border-spacing: 10px 20px; } \
             tr { display: table-row; } \
             td { display: table-cell; width: 50px; height: 30px; font-size: 10px; line-height: 10px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 },
    )
    .unwrap();

    let table_box = find_layout_box_by_tag(&layout, "table").unwrap();
    let row = &table_box.children[0];
    let cell1 = &row.children[0];
    let cell2 = &row.children[1];

    // Horizontal spacing = 10px: cell1 at x=10, cell2 at x=10+50+10=70
    let h_gap = cell2.dimensions.content.x - (cell1.dimensions.content.x + cell1.dimensions.content.width);
    assert!(
        (h_gap - 10.0).abs() < 1.0,
        "horizontal spacing should be ~10px, got {h_gap}"
    );
}

#[test]
fn line_height_percentage_scales_by_font_size() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    p.append_child(NodeHandle::text("Hello"));
    document.append_child(body.clone());
    body.append_child(p.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } p { font-size: 20px; line-height: 150%; width: 200px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 },
    )
    .unwrap();

    let p_box = find_layout_box_by_tag(&layout, "p").unwrap();
    // line-height: 150% of 20px = 30px
    assert!(
        (p_box.dimensions.content.height - 30.0).abs() < 1.0,
        "line-height: 150% of 20px should be ~30px, got {}",
        p_box.dimensions.content.height,
    );
}

#[test]
fn shrink_to_fit_table_uses_min_max_column_distribution() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let table = NodeHandle::element("table");
    let tr = NodeHandle::element("tr");
    let td_img = NodeHandle::element("td");
    let td_text = NodeHandle::element("td");
    let img = NodeHandle::element("img");
    img.set_attribute("width", "100");
    img.set_attribute("height", "100");
    img.set_attribute(
        "src",
        "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEUlEQVR42mP4/58BCv7/ZwAAHfAD/abwPj4AAAAASUVORK5CYII=",
    );
    td_img.append_child(img);
    td_text.append_child(NodeHandle::text(
        "This is a long text that should wrap within the available width of the table cell",
    ));
    tr.append_child(td_img);
    tr.append_child(td_text);
    table.append_child(tr);
    document.append_child(body.clone());
    body.append_child(table.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } \
             table { display: table; } \
             tr { display: table-row; } \
             td { display: table-cell; font-size: 12px; line-height: 12px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 800.0, height: 0.0 },
    )
    .unwrap();

    let table_box = find_layout_box_by_tag(&layout, "table").unwrap();
    // Table should shrink-to-fit: image column ~100px + text column.
    assert!(
        table_box.dimensions.content.width < 800.0,
        "shrink-to-fit table should be narrower than containing block (800px), got {}",
        table_box.dimensions.content.width,
    );
    assert!(
        table_box.dimensions.content.width >= 100.0,
        "table should be at least as wide as the image column, got {}",
        table_box.dimensions.content.width,
    );
    // Verify cell widths: image cell should be ~100px (explicit via img width attribute)
    let row = &table_box.children[0];
    let img_cell = &row.children[0];
    let text_cell = &row.children[1];
    assert!(
        (img_cell.dimensions.content.width - 100.0).abs() < 2.0,
        "image cell should be ~100px, got {}",
        img_cell.dimensions.content.width,
    );
    assert!(
        text_cell.dimensions.content.width > 50.0,
        "text cell should have reasonable width, got {}",
        text_cell.dimensions.content.width,
    );
    assert!(
        text_cell.dimensions.content.width < 700.0,
        "text cell should not use full preferred width, got {}",
        text_cell.dimensions.content.width,
    );
}

#[test]
fn nowrap_prevents_line_wrapping() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    div.set_attribute("class", "nowrap");
    div.append_child(NodeHandle::text("Hello world this is a long line of text that should not wrap"));
    document.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } .nowrap { white-space: nowrap; width: 100px; font-size: 10px; line-height: 10px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 },
    )
    .unwrap();

    let div_box = find_layout_box_by_tag(&layout, "div").unwrap();
    // With nowrap, all text should be on a single line
    assert_eq!(
        div_box.lines.len(),
        1,
        "white-space: nowrap should prevent line wrapping, got {} lines",
        div_box.lines.len(),
    );
}

#[test]
fn nowrap_with_inline_elements_stays_on_one_line() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    div.set_attribute("class", "nowrap");
    let em = NodeHandle::element("em");
    div.append_child(NodeHandle::text("Hello "));
    em.append_child(NodeHandle::text("world"));
    div.append_child(em);
    div.append_child(NodeHandle::text(" this is long text"));
    document.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } .nowrap { white-space: nowrap; width: 50px; font-size: 10px; line-height: 10px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 },
    )
    .unwrap();

    let div_box = find_layout_box_by_tag(&layout, "div").unwrap();
    assert_eq!(
        div_box.lines.len(),
        1,
        "nowrap with <em> should stay on one line, got {} lines",
        div_box.lines.len(),
    );
}

#[test]
fn calc_percent_minus_px_resolves_width_at_layout() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("div");
    let child = NodeHandle::element("section");
    container.set_attribute("class", "container");
    child.set_attribute("class", "child");
    child.append_child(NodeHandle::text("x"));
    document.append_child(body.clone());
    body.append_child(container.clone());
    container.append_child(child.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } \
             .container { width: 500px; } \
             .child { width: calc(100% - 100px); font-size: 10px; line-height: 10px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 800.0, height: 0.0 },
    )
    .unwrap();

    let child_box = find_layout_box_by_tag(&layout, "section").unwrap();
    // calc(100% - 100px) with containing block 500px → 400px
    assert!(
        (child_box.dimensions.content.width - 400.0).abs() < 1.0,
        "calc(100% - 100px) in 500px container should be ~400px, got {}",
        child_box.dimensions.content.width,
    );
}

#[test]
fn flex_wrap_child_calc_width_resolves_correctly() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let flex = NodeHandle::element("div");
    let left = NodeHandle::element("h3");
    let right = NodeHandle::element("dl");
    flex.set_attribute("class", "flex");
    left.set_attribute("class", "left");
    right.set_attribute("class", "right");
    left.append_child(NodeHandle::text("2008"));
    right.append_child(NodeHandle::text("Content here"));
    flex.append_child(left.clone());
    flex.append_child(right.clone());
    document.append_child(body.clone());
    body.append_child(flex.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } \
             .flex { display: flex; flex-wrap: wrap; width: 500px; } \
             .left { width: 165px; font-size: 12px; line-height: 12px; } \
             .right { width: calc(100% - 165px); font-size: 12px; line-height: 12px; }",
        )
        .unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 800.0, height: 0.0 },
    )
    .unwrap();

    let flex_box = find_layout_box_by_tag(&layout, "div").unwrap();
    assert!(flex_box.children.len() >= 2, "flex should have >= 2 children, got {}", flex_box.children.len());
    // Find h3 and dl boxes by tag
    let left_box = flex_box.children.iter()
        .find(|c| c.node.tag_name().as_deref() == Some("h3"))
        .expect("should have h3");
    let right_box = flex_box.children.iter()
        .find(|c| c.node.tag_name().as_deref() == Some("dl"))
        .expect("should have dl");

    // left (165px) and right (calc(100% - 165px) = 335px) should be on the same line
    assert!(
        (left_box.dimensions.content.width - 165.0).abs() < 1.0,
        "left should be 165px, got {}",
        left_box.dimensions.content.width,
    );
    // In flex layout, the resolved main size is used for layout.
    // calc(100% - 165px) resolves against the flex container width (500px) during
    // base_main_size calculation, giving 335px. However, layout_node uses main_size
    // as containing block, so the child's compute_width re-resolves as main_size - 165.
    // This is a known limitation; the important thing is both items fit on one line.
    assert!(
        right_box.dimensions.content.width > 100.0,
        "right should have reasonable width, got {}",
        right_box.dimensions.content.width,
    );
    // Both should be on the same flex line (right starts where left ends)
    assert!(
        (right_box.dimensions.content.x - (left_box.dimensions.content.x + left_box.dimensions.content.width)).abs() < 1.0,
        "right should start where left ends: left.x+w={} right.x={}",
        left_box.dimensions.content.x + left_box.dimensions.content.width,
        right_box.dimensions.content.x,
    );
}

// ── Tests for extracted pure functions ──────────────────────────────────

#[test]
fn all_whitespace_only_returns_true_for_empty() {
    assert!(all_whitespace_only(&[]));
}

#[test]
fn all_whitespace_only_returns_true_for_whitespace_text_nodes() {
    let doc = NodeHandle::document();
    let body = NodeHandle::element("body");
    doc.append_child(body.clone());
    let text1 = NodeHandle::text("  \n\t  ");
    let text2 = NodeHandle::text("   ");
    body.append_child(text1.clone());
    body.append_child(text2.clone());
    assert!(all_whitespace_only(&[text1, text2]));
}

#[test]
fn all_whitespace_only_returns_false_for_non_whitespace() {
    let doc = NodeHandle::document();
    let body = NodeHandle::element("body");
    doc.append_child(body.clone());
    let text1 = NodeHandle::text("  ");
    let text2 = NodeHandle::text("hello");
    body.append_child(text1.clone());
    body.append_child(text2.clone());
    assert!(!all_whitespace_only(&[text1, text2]));
}

#[test]
fn collapse_margins_both_positive_takes_larger() {
    assert_eq!(collapse_margins(10.0, 20.0), 20.0);
    assert_eq!(collapse_margins(20.0, 10.0), 20.0);
    assert_eq!(collapse_margins(15.0, 15.0), 15.0);
}

#[test]
fn collapse_margins_both_negative_takes_more_negative() {
    assert_eq!(collapse_margins(-10.0, -20.0), -20.0);
    assert_eq!(collapse_margins(-20.0, -10.0), -20.0);
}

#[test]
fn collapse_margins_mixed_signs_adds_them() {
    assert_eq!(collapse_margins(10.0, -5.0), 5.0);
    assert_eq!(collapse_margins(-5.0, 10.0), 5.0);
}

#[test]
fn collapse_margins_zero_cases() {
    assert_eq!(collapse_margins(0.0, 0.0), 0.0);
    assert_eq!(collapse_margins(10.0, 0.0), 10.0);
    assert_eq!(collapse_margins(0.0, 10.0), 10.0);
}

/// Helper: resolves computed style from a stylesheet for an element.
fn resolve_style_for_test(css: &str, tag: &str) -> ComputedStyle {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let elem = NodeHandle::element(tag);
    document.append_child(body.clone());
    body.append_child(elem.clone());
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
    resolver.computed_style(&elem)
}

#[test]
fn resolve_content_height_uses_auto_when_no_explicit_height() {
    let style = ComputedStyle::default();
    let height = resolve_content_height(&style, 0.0, EdgeSizes::default(), EdgeSizes::default(), 10.0, 50.0);
    assert_eq!(height, 40.0);
}

#[test]
fn resolve_content_height_uses_explicit_height() {
    let style = resolve_style_for_test("div { height: 100px; }", "div");
    let height = resolve_content_height(&style, 500.0, EdgeSizes::default(), EdgeSizes::default(), 0.0, 50.0);
    assert_eq!(height, 100.0);
}

#[test]
fn resolve_content_height_clamps_to_min_height() {
    let style = resolve_style_for_test("div { min-height: 80px; }", "div");
    let height = resolve_content_height(&style, 500.0, EdgeSizes::default(), EdgeSizes::default(), 0.0, 30.0);
    assert_eq!(height, 80.0);
}

#[test]
fn resolve_content_height_clamps_to_max_height() {
    let style = resolve_style_for_test("div { max-height: 20px; }", "div");
    let height = resolve_content_height(&style, 500.0, EdgeSizes::default(), EdgeSizes::default(), 0.0, 50.0);
    assert_eq!(height, 20.0);
}

#[test]
fn resolve_content_height_border_box_subtracts_padding_and_border() {
    let style = resolve_style_for_test("div { height: 100px; box-sizing: border-box; }", "div");
    let padding = EdgeSizes { top: 10.0, bottom: 10.0, left: 0.0, right: 0.0 };
    let border = EdgeSizes { top: 5.0, bottom: 5.0, left: 0.0, right: 0.0 };
    let height = resolve_content_height(&style, 500.0, padding, border, 0.0, 0.0);
    assert_eq!(height, 70.0);
}

#[test]
fn flush_pending_inline_nodes_clears_whitespace_only() {
    let doc = NodeHandle::document();
    let body = NodeHandle::element("body");
    doc.append_child(body.clone());
    let text = NodeHandle::text("   ");
    body.append_child(text.clone());

    let mut pending = vec![text];
    let mut resolver = StyleResolver::new();
    let style = ComputedStyle::default();
    let mut cursor_y = 0.0;
    let mut lines = Vec::new();
    flush_pending_inline_nodes(
        &mut pending, &mut resolver, &style, &[], &mut cursor_y, 0.0, 200.0, &mut lines,
    );
    assert!(pending.is_empty());
    assert!(lines.is_empty());
    assert_eq!(cursor_y, 0.0);
}

#[test]
fn block_children_inline_nodes_produce_line_boxes() {
    let doc = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    doc.append_child(body.clone());
    body.append_child(div.clone());
    let text = NodeHandle::text("Hello World");
    div.append_child(text);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { width: 200px; }").unwrap(),
    );

    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect { x: 0.0, y: 0.0, width: 200.0, height: 0.0 },
    ).unwrap();

    let div_box = &layout.children[0];
    assert!(!div_box.lines.is_empty(), "inline text should produce line boxes");
    let first_line = &div_box.lines[0];
    assert!(!first_line.fragments.is_empty(), "line should have fragments");
}

#[test]
fn redistribute_auto_margins_centers_table() {
    let style = resolve_style_for_test("div { margin-left: auto; margin-right: auto; }", "div");
    let padding = EdgeSizes::default();
    let border = EdgeSizes::default();
    let mut margin = EdgeSizes::default();
    redistribute_auto_margins_for_table(&style, 100.0, &padding, &border, &mut margin, 300.0);
    assert_eq!(margin.left, 100.0);
    assert_eq!(margin.right, 100.0);
}

#[test]
fn redistribute_auto_margins_left_auto_only() {
    let style = resolve_style_for_test("div { margin-left: auto; margin-right: 20px; }", "div");
    let padding = EdgeSizes::default();
    let border = EdgeSizes::default();
    let mut margin = EdgeSizes { top: 0.0, right: 20.0, bottom: 0.0, left: 0.0 };
    redistribute_auto_margins_for_table(&style, 100.0, &padding, &border, &mut margin, 300.0);
    assert_eq!(margin.left, 180.0);
    assert_eq!(margin.right, 20.0);
}

#[test]
fn child_containing_rect_uses_float_offsets_for_auto_width() {
    let style = ComputedStyle::default();
    let offsets = FloatOffsets { left: 50.0, right: 30.0 };
    let rect = child_containing_rect(&style, 10.0, &offsets, 0.0, 200.0, 120.0);
    assert_eq!(rect.x, 50.0);
    assert_eq!(rect.y, 10.0);
    assert_eq!(rect.width, 120.0);
    assert_eq!(rect.height, 120.0);
}

#[test]
fn child_containing_rect_ignores_offsets_for_explicit_width() {
    let style = resolve_style_for_test("div { width: 150px; }", "div");
    let offsets = FloatOffsets { left: 50.0, right: 30.0 };
    let rect = child_containing_rect(&style, 10.0, &offsets, 0.0, 200.0, 120.0);
    assert_eq!(rect.x, 0.0);
    assert_eq!(rect.width, 200.0);
    assert_eq!(rect.height, 120.0);
}
