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
