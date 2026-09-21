use super::*;
use crate::css::{Origin, parse_stylesheet};
use crate::html::TreeBuilder;
use std::sync::Arc;

const CSS: &str = "html,body{margin:0;padding:0}body{padding:24px;font:20px/28px 'Liberation Sans'}p{margin:0;width:330px}span{background:#fef08a}";
fn render(html: &str, extra: &str) -> (LayoutBox, crate::paint::Canvas) {
    let document = TreeBuilder::parse(html).document();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(&format!("{CSS}{extra}")).unwrap(),
    );
    let font = Arc::new(
        crate::font::Font::load_from_bytes(
            include_bytes!("../../tests/fixtures/acid2/LiberationSans-Regular.ttf").to_vec(),
        )
        .unwrap(),
    );
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 800.0,
        height: 600.0,
    };
    let layout = with_layout_fonts(vec![font.clone()], None, || {
        layout_tree(&document, &mut resolver, viewport).unwrap()
    });
    let canvas =
        crate::paint::paint_layout_with_fonts(&layout, &mut resolver, viewport, vec![font]);
    (layout, canvas)
}
fn rects(layout: &LayoutBox, id: &str) -> Vec<Rect> {
    let mut result = Vec::new();
    for line in &layout.lines {
        for f in &line.fragments {
            if f.node
                .attributes()
                .is_some_and(|a| a.get("id").is_some_and(|v| v == id))
            {
                result.push(f.rect);
            }
        }
    }
    for child in &layout.children {
        result.extend(rects(child, id));
    }
    result
}
#[test]
fn inline_boxes_paint_background_and_expose_one_box_per_line() {
    let (layout, canvas) = render("<p>A<span id='s'>BC D</span>E</p>", "");
    let r = rects(&layout, "s");
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].y, r[0].height), (26.0, 24.0));
    assert!((r[0].x - 37.333333).abs() < 0.02);
    assert!((r[0].width - 47.783333).abs() < 0.02);
    assert_eq!(
        canvas.pixel(40, 26),
        Some(crate::paint::Color::rgb(254, 240, 138))
    );
    assert_ne!(
        canvas.pixel(40, 25),
        Some(crate::paint::Color::rgb(254, 240, 138))
    );
    let (layout, _) = render(
        "<p>Before <span id='s'>one two three four five six seven</span> After</p>",
        "p{width:130px}",
    );
    let r = rects(&layout, "s");
    assert_eq!(r.len(), 3);
    assert_eq!(
        r.iter().map(|r| (r.y, r.height)).collect::<Vec<_>>(),
        [(26.0, 24.0), (54.0, 24.0), (82.0, 24.0)]
    );
}
#[test]
fn inline_boxes_preserve_nested_owners_and_empty_inline_positions() {
    let (layout, canvas) = render(
        "<p>A<span id='outer'>BC <span id='inner'>DE F</span> GH</span>I</p>",
        "#inner{background:#22c55e;font-size:14px;line-height:24px}",
    );
    let a = rects(&layout, "outer");
    let b = rects(&layout, "inner");
    assert_eq!((a.len(), b.len()), (1, 1));
    assert_eq!((a[0].y, a[0].height), (26.0, 24.0));
    assert_eq!((b[0].y, b[0].height), (32.0, 16.0));
    assert_eq!(
        canvas.pixel(73, 32),
        Some(crate::paint::Color::rgb(34, 197, 94))
    );
    let (layout, _) = render(
        "<p><span id='first'></span>A<span id='middle'></span>B<span id='last'></span><span id='hidden' style='display:none'>C</span></p>",
        "",
    );
    for id in ["first", "middle", "last"] {
        let r = rects(&layout, id);
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].y, r[0].width, r[0].height), (26.0, 0.0, 24.0));
    }
    assert!(rects(&layout, "hidden").is_empty());
    let (layout, _) = render("<p><span id='empty'></span></p><p>After</p>", "");
    assert_eq!(
        rects(&layout, "empty"),
        [Rect {
            x: 24.0,
            y: 24.0,
            width: 0.0,
            height: 0.0
        }]
    );
}
#[test]
fn inline_boxes_use_font_height_and_include_padding_and_border() {
    let (layout, _) = render(
        "<p>A<span id='s'>BC D</span>E</p>",
        "body{line-height:48px}",
    );
    let r = rects(&layout, "s");
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].y, r[0].height), (36.0, 24.0));
    let (layout, _) = render(
        "<p>A<span id='s'>BC D</span>E</p>",
        "span{padding:3px 5px;border:2px solid #1e293b}",
    );
    let r = rects(&layout, "s");
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].y, r[0].height), (21.0, 34.0));
    assert!((r[0].width - 61.783333).abs() < 0.02);
}

#[test]
fn inline_box_width_preserves_font_advances_and_records_firefox_rounding() {
    let font = crate::font::Font::load_from_bytes(
        include_bytes!("../../tests/fixtures/acid2/LiberationSans-Regular.ttf").to_vec(),
    )
    .unwrap();
    let glyphs = font
        .shape_text(
            "highlighted inline",
            20.0,
            crate::font::ShapingDirection::LeftToRight,
        )
        .unwrap();
    let raw: f32 = glyphs.iter().map(|g| g.x_advance).sum();
    let rounded: f32 = glyphs
        .iter()
        .map(|g| (g.x_advance * 60.0).round() / 60.0)
        .sum();
    assert!((raw - 149.00390625).abs() < 0.0001);
    assert!((rounded - 148.9666748046875).abs() < 0.0001);
    let (layout, _) = render(
        "<p>Normal <span id='s'>highlighted inline</span> text</p>",
        "",
    );
    assert!((rects(&layout, "s")[0].width - raw).abs() < 0.0001);

    let (layout, _) = render(
        "<p><span id='s'>A  \nBC  </span></p>",
        "span{white-space:pre}",
    );
    let regions = rects(&layout, "s");
    assert_eq!(regions.len(), 2);
    // Rustybuzz applies the font's A-space kerning across the preserved
    // trailing spaces. Firefox reports the same spaces without that pair
    // adjustment; #677 owns this shaping difference rather than #674's box
    // aggregation.
    assert!((regions[0].width - 23.349609).abs() < 0.0001, "{regions:?}");
    assert!((regions[1].width - 38.896484).abs() < 0.0001, "{regions:?}");
}

#[test]
fn text_baseline_uses_the_selected_face_metrics_and_half_leading() {
    fn find_text_line(layout: &LayoutBox) -> Option<(&LineBox, &InlineFragment)> {
        layout
            .lines
            .iter()
            .find_map(|line| {
                line.fragments
                    .iter()
                    .find(|fragment| {
                        matches!(
                            &fragment.content,
                            InlineFragmentContent::Text(text) if text == "A"
                        )
                    })
                    .map(|fragment| (line, fragment))
            })
            .or_else(|| layout.children.iter().find_map(find_text_line))
    }

    let (layout, _) = render("<p>A</p>", "");
    let (line, fragment) = find_text_line(&layout).expect("line containing fixed-font text");
    let font = crate::font::Font::load_from_bytes(
        include_bytes!("../../tests/fixtures/acid2/LiberationSans-Regular.ttf").to_vec(),
    )
    .unwrap();
    let metrics = font.layout_metrics(20.0);
    let expected =
        line.rect.y + (line.rect.height - metrics.ascent - metrics.descent) / 2.0 + metrics.ascent;

    assert!((fragment.metrics.ascent - metrics.ascent).abs() < 0.0001);
    assert!((fragment.metrics.descent - metrics.descent).abs() < 0.0001);
    assert!((line.baseline - expected).abs() < 0.0001);
    assert!((fragment.rect.y + fragment.metrics.ascent - line.baseline).abs() < 0.0001);
}

#[test]
fn trailing_preserved_newline_does_not_expose_a_marker_only_client_rect() {
    let (layout, _) = render("<p><span id='s' style='white-space:pre'>A\n</span></p>", "");
    let regions = rects(&layout, "s");
    assert_eq!(regions.len(), 1, "{regions:?}");
    assert_eq!((regions[0].y, regions[0].height), (26.0, 24.0));

    let (layout, _) = render("<p><span id='empty'></span></p>", "");
    assert_eq!(
        rects(&layout, "empty"),
        [Rect {
            x: 24.0,
            y: 24.0,
            width: 0.0,
            height: 0.0,
        }],
        "a standalone empty inline keeps its first zero-sized CSSOM rect"
    );
}

fn find_text_line<'a>(layout: &'a LayoutBox, text: &str) -> Option<&'a LineBox> {
    layout
        .lines
        .iter()
        .find(|line| {
            line.fragments.iter().any(
                |fragment| matches!(&fragment.content, InlineFragmentContent::Text(value) if value == text),
            )
        })
        .or_else(|| {
            layout
                .children
                .iter()
                .find_map(|child| find_text_line(child, text))
        })
}

#[test]
fn mixed_font_sizes_expand_horizontal_line_from_baseline_extents() {
    let (layout, _) = render(
        "<p>H<span>H</span>H</p>",
        "p{font-size:20px;line-height:20px}span{font-size:40px;line-height:20px}",
    );
    let line = find_text_line(&layout, "H").expect("mixed-size line");
    let extents = line
        .fragments
        .iter()
        .filter_map(|fragment| {
            matches!(fragment.content, InlineFragmentContent::Text(_)).then(|| {
                let half_leading =
                    (fragment.rect.height - fragment.metrics.ascent - fragment.metrics.descent)
                        / 2.0;
                (
                    fragment.metrics.ascent + half_leading,
                    fragment.metrics.descent + half_leading,
                )
            })
        })
        .collect::<Vec<_>>();
    let expected_above = extents.iter().map(|(above, _)| *above).fold(0.0, f32::max);
    let expected_below = extents.iter().map(|(_, below)| *below).fold(0.0, f32::max);
    let expected_height = expected_above + expected_below;

    assert!(
        expected_height > 20.0,
        "fixed font must expose the regression"
    );
    assert!(
        (expected_height - 26.933594).abs() < 0.001,
        "{expected_height}"
    );
    assert!((line.rect.height - 27.0).abs() < 0.001, "{line:?}");
    assert!((line.baseline - line.rect.y - expected_above).abs() < 0.001);
}

#[test]
fn mixed_font_size_line_height_forms_and_vertical_align_use_baseline_extents() {
    for child_line_height in ["20px", "0.5", "50%", "normal"] {
        let (layout, _) = render(
            "<p>H<span>H</span>H</p>",
            &format!(
                "p{{font-size:20px;line-height:20px}}span{{font-size:40px;line-height:{child_line_height}}}"
            ),
        );
        let line = find_text_line(&layout, "H").expect("mixed-size line");
        let mut above = 0.0_f32;
        let mut below = 0.0_f32;
        for fragment in &line.fragments {
            if !matches!(fragment.content, InlineFragmentContent::Text(_)) {
                continue;
            }
            let ascent = fragment.metrics.ascent.ceil();
            let descent = fragment.metrics.descent.ceil();
            let half_leading = (fragment.rect.height - ascent - descent) / 2.0;
            above = above.max(ascent + half_leading);
            below = below.max(descent + half_leading);
        }
        assert!(
            (line.rect.height - above - below).abs() < 0.001,
            "line-height {child_line_height}: {line:?}"
        );
    }

    let (layout, _) = render(
        "<p>H<span>H</span>H<br>H<span>H</span>H</p>",
        "p{font-size:20px;line-height:20px}span{font-size:40px;line-height:20px;vertical-align:5px}",
    );
    let lines = layout
        .children
        .iter()
        .flat_map(|html| &html.children)
        .flat_map(|body| &body.children)
        .find(|child| child.node.tag_name().as_deref() == Some("p"))
        .expect("paragraph layout")
        .lines
        .as_slice();
    assert_eq!(lines.len(), 2);
    assert!((lines[1].rect.y - lines[0].rect.y - lines[0].rect.height).abs() < 0.001);
    assert_eq!(lines[0].rect.height, lines[1].rect.height);
    assert!(lines[0].rect.height > 20.0);
}

#[test]
fn mixed_font_sizes_do_not_apply_horizontal_extents_to_vertical_columns() {
    let (layout, _) = render(
        "<p>H<span>H</span>H</p>",
        "p{writing-mode:vertical-lr;width:80px;height:160px;font-size:20px;line-height:20px}\
         span{font-size:40px;line-height:20px}",
    );
    let line = find_text_line(&layout, "H").expect("vertical mixed-size line");
    assert_eq!(
        line.rect.width, 20.0,
        "vertical column keeps its central baseline width"
    );
}

#[test]
fn inline_margins_advance_without_expanding_the_border_box() {
    let (plain, _) = render(
        "<p><span id='first'>Alpha</span><span id='second'>Beta</span></p>",
        "span{background:transparent}",
    );
    let (spaced, _) = render(
        "<p><span id='first'>Alpha</span><span id='second'>Beta</span></p>",
        "span{background:transparent}#second{margin-left:18px}",
    );
    let plain_first = rects(&plain, "first")[0];
    let plain_second = rects(&plain, "second")[0];
    let spaced_first = rects(&spaced, "first")[0];
    let spaced_second = rects(&spaced, "second")[0];

    assert!((spaced_second.x - plain_second.x - 18.0).abs() < 0.001);
    assert!(
        (spaced_second.width - plain_second.width).abs() < 0.001,
        "{plain_second:?} {spaced_second:?}"
    );
    assert_eq!(spaced_first, plain_first);

    let (negative, _) = render(
        "<p><span id='first'>Alpha</span><span id='second'>Beta</span></p>",
        "span{background:transparent}#second{margin-left:-5px}",
    );
    let negative_second = rects(&negative, "second")[0];
    assert!((negative_second.x - plain_second.x + 5.0).abs() < 0.001);

    let (automatic, _) = render(
        "<p><span id='first'>Alpha</span><span id='second'>Beta</span></p>",
        "span{background:transparent}#second{margin-left:auto}",
    );
    assert_eq!(rects(&automatic, "second")[0], plain_second);
}

#[test]
fn inline_margins_stay_outside_padding_border_and_follow_rtl_start() {
    let html = "<p><span id='second'>Beta</span></p>";
    let base_css = "p{direction:rtl;text-align:left}\
                    span{background:transparent;padding:0 5px;border:2px solid transparent}";
    let (plain, _) = render(html, base_css);
    let (spaced, _) = render(
        html,
        &format!("{base_css}#second{{margin-right:18px;margin-left:7px}}"),
    );
    let plain_second = rects(&plain, "second")[0];
    let spaced_second = rects(&spaced, "second")[0];
    assert!(
        (spaced_second.width - plain_second.width).abs() < 0.001,
        "{plain_second:?} {spaced_second:?}"
    );
    assert!(
        (spaced_second.x - plain_second.x - 7.0).abs() < 0.001,
        "RTL end margin stays after the second border box: {plain_second:?} {spaced_second:?}"
    );

    let plain_line = find_text_line(&plain, "Beta").unwrap();
    let spaced_line = find_text_line(&spaced, "Beta").unwrap();
    assert!((spaced_line.rect.width - plain_line.rect.width - 25.0).abs() < 0.001);
}

#[test]
fn inline_margin_uses_the_vertical_inline_axis() {
    let (plain, _) = render(
        "<p><span id='first'>A</span><span id='second'>B</span></p>",
        "p{writing-mode:vertical-lr;width:80px;height:160px}span{background:transparent}",
    );
    let (spaced, _) = render(
        "<p><span id='first'>A</span><span id='second'>B</span></p>",
        "p{writing-mode:vertical-lr;width:80px;height:160px}span{background:transparent}\
         #second{margin-top:18px}",
    );
    let plain_second = rects(&plain, "second")[0];
    let spaced_second = rects(&spaced, "second")[0];
    assert!((spaced_second.y - plain_second.y - 18.0).abs() < 0.001);
    assert!((spaced_second.height - plain_second.height).abs() < 0.001);
}

#[test]
fn inline_margins_apply_only_to_the_first_and_last_line_fragments() {
    let (layout, _) = render(
        "<p><span id='s'>A<br>B</span></p>",
        "#s{margin-left:10px;margin-right:12px;background:transparent}",
    );
    let regions = rects(&layout, "s");
    assert_eq!(regions.len(), 2, "{regions:?}");
    assert!((regions[0].x - 34.0).abs() < 0.001, "{regions:?}");
    assert!((regions[1].x - 24.0).abs() < 0.001, "{regions:?}");
    let lines = layout
        .children
        .iter()
        .flat_map(|html| &html.children)
        .flat_map(|body| &body.children)
        .find(|child| child.node.tag_name().as_deref() == Some("p"))
        .expect("paragraph layout")
        .lines
        .as_slice();
    assert_eq!(lines.len(), 2);
    assert!((lines[0].rect.width - regions[0].width - 10.0).abs() < 0.001);
    assert!((lines[1].rect.width - regions[1].width - 12.0).abs() < 0.001);
}

#[test]
fn visibility_hidden_inline_fragments_do_not_paint_but_keep_geometry() {
    let (layout, canvas) = render(
        "<p><span id='hidden'>Hidden</span></p>",
        "#hidden{visibility:hidden;background:#ef4444;border:2px solid #2563eb;\
         text-decoration:underline}",
    );
    let region = rects(&layout, "hidden")[0];
    assert!(region.width > 0.0 && region.height > 0.0);
    for y in region.y.floor() as u32..(region.y + region.height).ceil() as u32 {
        for x in region.x.floor() as u32..(region.x + region.width).ceil() as u32 {
            assert_eq!(
                canvas.pixel(x, y),
                Some(crate::paint::Color::rgba(0, 0, 0, 0))
            );
        }
    }
}

#[test]
fn visible_descendant_paints_inside_visibility_hidden_inline() {
    let (layout, canvas) = render(
        "<p><span id='hidden'>A<b id='visible'>B</b>C</span></p>",
        "#hidden{visibility:hidden;background:#ef4444}\
         #visible{visibility:visible;background:#22c55e}",
    );
    let parent = rects(&layout, "hidden")[0];
    let child = rects(&layout, "visible")[0];
    assert!(parent.width > child.width);
    assert_eq!(
        canvas.pixel(child.x.floor() as u32, child.y.floor() as u32),
        Some(crate::paint::Color::rgb(34, 197, 94))
    );
}

#[test]
fn visibility_hidden_suppresses_generated_and_replaced_inline_content() {
    const RED_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEUlEQVR42mP4/58BCv7/ZwAAHfAD/abwPj4AAAAASUVORK5CYII=";
    let (generated_layout, generated_canvas) = render(
        "<p><span id='hidden'></span></p>",
        "#hidden{visibility:hidden}#hidden::before{content:'Generated';background:#ef4444}",
    );
    let generated = rects(&generated_layout, "hidden")[0];
    for y in generated.y.floor() as u32..(generated.y + generated.height).ceil() as u32 {
        for x in generated.x.floor() as u32..(generated.x + generated.width).ceil() as u32 {
            assert_eq!(
                generated_canvas.pixel(x, y),
                Some(crate::paint::Color::rgba(0, 0, 0, 0))
            );
        }
    }

    let (image_layout, image_canvas) = render(
        &format!("<p><span id='hidden'><img src='{RED_PNG}'></span></p>"),
        "#hidden{visibility:hidden}img{width:20px;height:20px}",
    );
    let image_region = rects(&image_layout, "hidden")[0];
    assert!(image_region.width >= 20.0);
    assert_eq!(
        image_canvas.pixel(image_region.x as u32 + 10, image_region.y as u32 + 10),
        Some(crate::paint::Color::rgba(0, 0, 0, 0))
    );
}

#[test]
fn inline_start_margin_moves_with_its_automatic_line_wrap() {
    let (layout, _) = render(
        "<p>A <span id='s'>B</span></p>",
        "p{width:40px;font-size:16px;line-height:20px}#s{margin-left:18px;background:transparent}",
    );
    let regions = rects(&layout, "s");
    let lines = layout
        .children
        .iter()
        .flat_map(|html| &html.children)
        .flat_map(|body| &body.children)
        .find(|child| child.node.tag_name().as_deref() == Some("p"))
        .expect("paragraph layout")
        .lines
        .as_slice();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(regions.len(), 1, "{regions:?}");
    assert!((regions[0].x - lines[1].rect.x - 18.0).abs() < 0.001);
    assert!((lines[1].rect.width - regions[0].width - 18.0).abs() < 0.001);
}
