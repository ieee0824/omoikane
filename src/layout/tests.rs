use crate::css::{Origin, parse_stylesheet};
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
    assert!(
        line.fragments
            .iter()
            .any(|fragment| matches!(fragment.content, InlineFragmentContent::GeneratedBox(_)))
    );
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
    assert_eq!(row_box.children[0].dimensions.content.width, 54.0);
    assert_eq!(row_box.children[1].dimensions.content.x, 62.0);
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
