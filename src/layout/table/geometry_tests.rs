use super::*;
use crate::css::{Origin, parse_stylesheet};
use crate::html::TreeBuilder;

fn render(html: &str, css: &str) -> LayoutBox {
    let document = TreeBuilder::parse(html).document();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
    let mut fonts = crate::font::WebFontRegistry::new();
    fonts.push(
        "Liberation Sans",
        crate::font::FontWeight(400),
        crate::font::FontStyle::Normal,
        crate::font::Font::load_from_bytes(
            include_bytes!("../../../tests/fixtures/acid2/LiberationSans-Regular.ttf").to_vec(),
        )
        .unwrap(),
    );
    crate::layout::with_layout_fonts(Vec::new(), Some(std::sync::Arc::new(fonts)), || {
        crate::layout::layout_tree(
            &document,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
        )
        .unwrap()
    })
}

fn by_id<'a>(layout: &'a LayoutBox, id: &str) -> &'a LayoutBox {
    fn find<'a>(layout: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
        if layout
            .node
            .attributes()
            .is_some_and(|attrs| attrs.get("id").is_some_and(|v| v == id))
        {
            return Some(layout);
        }
        layout.children.iter().find_map(|child| find(child, id))
    }
    find(layout, id).unwrap_or_else(|| panic!("missing #{id}"))
}

const HTML: &str = "<!doctype html><html><body><table id='table'><tr><th id='h1'>Item</th><th id='h2'>Value</th></tr><tr><td id='d1'>Alpha</td><td id='d2'>120</td></tr><tr><td colspan='2' id='span'>Combined cell</td></tr></table></body></html>";
const CSS: &str = "html,body{margin:0;padding:0}body{font-family:'Liberation Sans';font-size:16px;line-height:24px}*{box-sizing:border-box}table{margin:24px;width:500px;table-layout:fixed}td,th{border:2px solid black;padding:8px;text-align:left}";

#[test]
fn firefox_collapsed_fixed_table_dimensions() {
    let root = render(HTML, &format!("{CSS}table{{border-collapse:collapse}}"));
    let table = by_id(&root, "table").dimensions.border_box();
    assert_eq!((table.width, table.height), (500.0, 128.0));
    // Compare table-relative positions: root margin propagation is tracked in #671.
    for (id, x, y, width) in [
        ("h1", 1.0, 1.0, 249.0),
        ("h2", 250.0, 1.0, 249.0),
        ("d1", 1.0, 43.0, 249.0),
        ("d2", 250.0, 43.0, 249.0),
        ("span", 1.0, 85.0, 498.0),
    ] {
        let rect = by_id(&root, id).dimensions.border_box();
        assert_eq!(
            (rect.x - table.x, rect.y - table.y, rect.width, rect.height),
            (x, y, width, 42.0),
            "#{id}"
        );
    }
}

#[test]
fn separate_cell_padding_and_border_are_counted_once() {
    let root = render(
        HTML,
        &format!("{CSS}table{{border-collapse:separate;border-spacing:4px 6px}}"),
    );
    let table = by_id(&root, "table").dimensions.border_box();
    assert_eq!((table.width, table.height), (500.0, 156.0));
    for (id, x, y, width) in [
        ("h1", 4.0, 6.0, 244.0),
        ("h2", 252.0, 6.0, 244.0),
        ("d1", 4.0, 56.0, 244.0),
        ("d2", 252.0, 56.0, 244.0),
        ("span", 4.0, 106.0, 492.0),
    ] {
        let rect = by_id(&root, id).dimensions.border_box();
        assert_eq!(
            (rect.x - table.x, rect.y - table.y, rect.width, rect.height),
            (x, y, width, 44.0),
            "#{id}"
        );
    }
}

#[test]
fn fixed_columns_use_col_then_first_row_and_ignore_later_widths() {
    for (border, available) in [("collapse", 498.0), ("separate", 488.0)] {
        for (cols, first, expected) in [
            ("", "", available / 2.0),
            ("", "style='width:100px'", 100.0),
            (
                "<colgroup><col style='width:100px'><col></colgroup>",
                "style='width:180px'",
                100.0,
            ),
        ] {
            let html = format!(
                "<table id='table'>{cols}<tr><td id='a' {first}>first</td><td id='b'>second</td></tr><tr><td id='c' style='width:450px'>A much longer later row</td><td>value</td></tr></table>"
            );
            let root = render(
                &html,
                &format!("{CSS}table{{border-collapse:{border};border-spacing:4px 6px}}"),
            );
            assert_eq!(by_id(&root, "a").dimensions.border_box().width, expected);
            assert_eq!(by_id(&root, "c").dimensions.border_box().width, expected);
            assert_eq!(
                by_id(&root, "b").dimensions.border_box().width,
                available - expected
            );
            assert_eq!(
                by_id(&root, "c").dimensions.border_box().height,
                if expected == 100.0 {
                    if border == "collapse" { 90.0 } else { 92.0 }
                } else if border == "collapse" {
                    42.0
                } else {
                    44.0
                }
            );
        }
    }
}

#[test]
fn fixed_colspan_hint_includes_internal_spacing_once() {
    for (border, column, remainder) in [("collapse", 100.0, 298.0), ("separate", 98.0, 288.0)] {
        let html = "<table id='table'><tr><td id='span' colspan='2' style='width:200px'>Span</td><td id='b'>Other</td></tr><tr><td id='c'>C</td><td id='d'>D</td><td>E</td></tr></table>";
        let root = render(
            html,
            &format!("{CSS}table{{border-collapse:{border};border-spacing:4px 6px}}"),
        );
        assert_eq!(by_id(&root, "span").dimensions.border_box().width, 200.0);
        assert_eq!(by_id(&root, "b").dimensions.border_box().width, remainder);
        assert_eq!(by_id(&root, "c").dimensions.border_box().width, column);
        assert_eq!(by_id(&root, "d").dimensions.border_box().width, column);
    }
}

#[test]
fn rowspan_uses_border_box_and_translates_decorated_cells_once() {
    let html = "<table id='table'><tr><td>Initial</td><td>Initial</td></tr><tr><td id='span' rowspan='2' style='height:140px;vertical-align:bottom'>Tall</td><td id='d' style='vertical-align:bottom'>Second</td></tr><tr><td id='e' style='vertical-align:bottom'>Third</td></tr></table>";
    for (border, height, span_y, last_y, cell_height) in [
        ("collapse", 184.0, 43.0, 113.0, 70.0),
        ("separate", 202.0, 56.0, 129.0, 67.0),
    ] {
        let root = render(
            html,
            &format!("{CSS}table{{border-collapse:{border};border-spacing:4px 6px}}"),
        );
        let table = by_id(&root, "table").dimensions.border_box();
        assert_eq!(table.height, height);
        let span = by_id(&root, "span").dimensions.border_box();
        assert_eq!((span.y - table.y, span.height), (span_y, 140.0));
        let d = by_id(&root, "d").dimensions.border_box();
        let e = by_id(&root, "e").dimensions.border_box();
        assert_eq!(
            d.x, e.x,
            "second-pass movement must not add padding/border to x"
        );
        assert_eq!((e.y - table.y, e.height), (last_y, cell_height));
        for id in ["span", "d", "e"] {
            let cell = by_id(&root, id);
            let line = cell.lines.last().unwrap();
            assert_eq!(
                line.rect.y + line.rect.height,
                cell.dimensions.content.y + cell.dimensions.content.height,
                "bottom alignment of #{id}"
            );
        }
    }
}

#[test]
fn automatic_tables_keep_padding_inside_tracks_and_height_is_a_minimum() {
    for (border, height) in [("collapse", 128.0), ("separate", 156.0)] {
        let root = render(
            HTML,
            &format!(
                "{CSS}table{{table-layout:auto;border-collapse:{border};border-spacing:4px 6px}}"
            ),
        );
        let table = by_id(&root, "table").dimensions.border_box();
        assert_eq!((table.width, table.height), (500.0, height));
        let a = by_id(&root, "h1").dimensions.border_box();
        let b = by_id(&root, "h2").dimensions.border_box();
        assert!((a.x + a.width - b.x).abs() <= if border == "collapse" { 0.001 } else { 4.001 });
        assert!(b.x + b.width <= table.x + table.width);
        let small = render(
            "<table id='table'><tr><td id='a' style='height:10px'>Content</td><td>Other</td></tr></table>",
            &format!("{CSS}table{{border-collapse:{border};border-spacing:4px 6px}}"),
        );
        assert_eq!(
            by_id(&small, "a").dimensions.border_box().height,
            if border == "collapse" { 42.0 } else { 44.0 }
        );
    }
}

#[test]
fn collapsed_borders_paint_the_full_shared_edge_without_changing_cell_rects() {
    let css = format!(
        "{CSS}table{{border-collapse:collapse}}td{{background:white}}td div{{height:24px}}"
    );
    let root = render(
        "<table id='table'><tr><td><div></div></td><td><div></div></td></tr><tr><td><div></div></td><td><div></div></td></tr></table>",
        &css,
    );
    let table = by_id(&root, "table").dimensions.border_box();
    assert_eq!((table.width, table.height), (500.0, 86.0));
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(&css).unwrap());
    let canvas = crate::paint::paint_layout(
        &root,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    );
    let pixel = |x, y| {
        canvas
            .pixel(table.x as u32 + x, table.y as u32 + y)
            .unwrap()
    };
    for x in [0, 1, 249, 250, 498, 499] {
        assert_eq!(
            pixel(x, 20),
            crate::paint::Color::rgb(0, 0, 0),
            "vertical border at {x}"
        );
    }
    for y in [0, 1, 42, 43, 84, 85] {
        assert_eq!(
            pixel(20, y),
            crate::paint::Color::rgb(0, 0, 0),
            "horizontal border at {y}"
        );
    }
    for (x, y) in [
        (2, 20),
        (248, 20),
        (251, 20),
        (497, 20),
        (20, 2),
        (20, 41),
        (20, 44),
        (20, 83),
    ] {
        assert_eq!(
            pixel(x, y),
            crate::paint::Color::rgb(255, 255, 255),
            "content at {x},{y}"
        );
    }
}

#[test]
fn table_keywords_are_ascii_case_insensitive() {
    let root = render(
        HTML,
        &format!(
            "{CSS}table{{table-layout:FiXeD;border-collapse:CoLlApSe}}td,th{{display:TaBlE-CeLl}}"
        ),
    );
    let table = by_id(&root, "table").dimensions.border_box();
    assert_eq!((table.width, table.height), (500.0, 128.0));
    assert_eq!(by_id(&root, "h1").dimensions.border_box().width, 249.0);
    let html = HTML.replacen("<tr>", "<colgroup style='display:TaBlE-CoLuMn-GrOuP'><col style='display:TaBlE-CoLuMn;width:100px'><col></colgroup><tr>", 1);
    let root = render(
        &html,
        &format!("{CSS}table{{table-layout:FiXeD;border-collapse:CoLlApSe}}"),
    );
    assert_eq!(by_id(&root, "h1").dimensions.border_box().width, 100.0);
}
