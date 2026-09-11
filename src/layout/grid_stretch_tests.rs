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
            include_bytes!("../../tests/fixtures/acid2/LiberationSans-Regular.ttf").to_vec(),
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

const GRID: &str = "html,body{margin:0;padding:0}body{padding-top:1px;font:16px/24px 'Liberation Sans'}*{box-sizing:border-box}#grid{display:grid;grid-template-columns:120px 1fr 2fr;grid-template-rows:60px 90px;gap:12px;width:600px;margin:24px}";

#[test]
fn empty_auto_grid_items_stretch_to_their_rows() {
    for extra in ["", "#grid>div{height:100%}"] {
        let root = render(
            "<div id='grid'><div id='g0'></div><div id='g1'></div><div id='g2'></div><div id='g3'></div><div id='g4'></div><div id='g5'></div></div>",
            &format!("{GRID}{extra}"),
        );
        for (i, (x, width)) in [(24.0, 120.0), (156.0, 152.0), (320.0, 304.0)]
            .iter()
            .cycle()
            .take(6)
            .enumerate()
        {
            let r = by_id(&root, &format!("g{i}")).dimensions.border_box();
            assert_eq!(
                (r.x, r.y, r.width, r.height),
                (
                    *x,
                    if i < 3 { 25.0 } else { 97.0 },
                    *width,
                    if i < 3 { 60.0 } else { 90.0 }
                )
            );
        }
    }
}

#[test]
fn grid_stretch_respects_content_explicit_height_alignment_and_constraints() {
    for (extra, content, height) in [
        ("", "Text", 60.0),
        ("height:20px", "", 20.0),
        ("align-self:start", "", 0.0),
        ("align-self:start", "Text", 24.0),
        ("padding:5px;border:2px solid black", "", 60.0),
        ("min-height:80px", "", 80.0),
        ("max-height:30px", "", 30.0),
        (
            "padding:5px;border:2px solid black;max-height:30px",
            "",
            30.0,
        ),
    ] {
        let root = render(
            &format!("<div id='grid'><div id='item' style='{extra}'>{content}</div></div>"),
            GRID,
        );
        assert_eq!(
            by_id(&root, "item").dimensions.border_box().height,
            height,
            "{extra}"
        );
    }
}

#[test]
fn grid_stretch_provides_a_percentage_basis_for_descendants() {
    let root = render(
        "<div id='grid'><div id='item' style='padding:5px;border:2px solid black'><div id='child' style='height:100%'></div></div></div>",
        GRID,
    );
    assert_eq!(by_id(&root, "item").dimensions.content.height, 46.0);
    assert_eq!(by_id(&root, "child").dimensions.content.height, 46.0);
}

#[test]
fn grid_auto_rows_and_subgrids_reflow_percentage_descendants() {
    for extra in ["grid-template-rows:auto;", "grid-template-rows:80px;"] {
        let root = render(
            "<div id='grid'><div id='item'><div id='child' style='height:100%'></div></div><div style='height:80px'></div></div>",
            &format!("{GRID}#grid{{grid-template-columns:100px 100px;{extra}}}"),
        );
        assert_eq!(by_id(&root, "item").dimensions.content.height, 80.0);
        assert_eq!(by_id(&root, "child").dimensions.content.height, 80.0);
    }
    let root = render(
        "<div id='grid'><div id='item' style='display:grid;grid-template-rows:subgrid'><div id='child'></div></div></div>",
        GRID,
    );
    assert_eq!(by_id(&root, "item").dimensions.content.height, 60.0);
    assert_eq!(by_id(&root, "child").dimensions.content.height, 60.0);
}
