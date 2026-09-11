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

#[test]
fn collapsed_margins_match_firefox() {
    let cases: &[(&str, &[(&str, [f32; 4])])] = &[
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-block.html"
            ),
            &[
                ("outer", [24.0, 24.0, 400.0, 164.0]),
                ("first", [44.0, 44.0, 360.0, 64.0]),
                ("second", [44.0, 120.0, 180.0, 48.0]),
                ("relative", [24.0, 212.0, 400.0, 130.0]),
                ("absolute", [332.0, 286.0, 80.0, 40.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-nested-positive.html"
            ),
            &[
                ("p", [0.0, 30.0, 800.0, 20.0]),
                ("a", [0.0, 30.0, 800.0, 20.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-nested-mixed.html"
            ),
            &[
                ("p", [0.0, 15.0, 800.0, 20.0]),
                ("a", [0.0, 15.0, 800.0, 20.0]),
                ("b", [0.0, 15.0, 800.0, 20.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-siblings-positive.html"
            ),
            &[
                ("a", [0.0, 1.0, 800.0, 10.0]),
                ("b", [0.0, 41.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-siblings-mixed.html"
            ),
            &[
                ("a", [0.0, 1.0, 800.0, 10.0]),
                ("b", [0.0, 26.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-empty-chain.html"
            ),
            &[
                ("a", [0.0, 1.0, 800.0, 10.0]),
                ("e", [0.0, 31.0, 800.0, 0.0]),
                ("b", [0.0, 36.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-empty-first.html"
            ),
            &[
                ("p", [0.0, 40.0, 800.0, 10.0]),
                ("e", [0.0, 40.0, 800.0, 0.0]),
                ("b", [0.0, 40.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-bottom.html"
            ),
            &[
                ("p", [0.0, 1.0, 800.0, 20.0]),
                ("a", [0.0, 1.0, 800.0, 20.0]),
                ("b", [0.0, 51.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-root.html"
            ),
            &[("a", [10.0, 40.0, 780.0, 20.0])],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-clear.html"
            ),
            &[
                ("p", [0.0, 1.0, 800.0, 60.0]),
                ("f", [0.0, 1.0, 30.0, 50.0]),
                ("a", [0.0, 51.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-flow-root.html"
            ),
            &[
                ("p", [0.0, 10.0, 800.0, 50.0]),
                ("a", [0.0, 40.0, 800.0, 20.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-overflow.html"
            ),
            &[
                ("p", [0.0, 10.0, 800.0, 50.0]),
                ("a", [0.0, 40.0, 800.0, 20.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-border.html"
            ),
            &[
                ("p", [0.0, 10.0, 800.0, 51.0]),
                ("a", [0.0, 41.0, 800.0, 20.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-padding.html"
            ),
            &[
                ("p", [0.0, 10.0, 800.0, 51.0]),
                ("a", [0.0, 41.0, 800.0, 20.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-float.html"
            ),
            &[
                ("p", [0.0, 10.0, 200.0, 50.0]),
                ("a", [0.0, 40.0, 200.0, 20.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-absolute.html"
            ),
            &[("p", [0.0, 10.0, 0.0, 50.0]), ("a", [0.0, 40.0, 0.0, 20.0])],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-positioned.html"
            ),
            &[
                ("p", [0.0, 30.0, 800.0, 50.0]),
                ("a", [0.0, 30.0, 800.0, 20.0]),
                ("viewport-abs", [0.0, 7.0, 0.0, 10.0]),
                ("fixed", [0.0, 9.0, 0.0, 10.0]),
                ("rel", [0.0, 55.0, 800.0, 30.0]),
                ("local-abs", [0.0, 58.0, 0.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-flex-item.html"
            ),
            &[
                ("p", [0.0, 0.0, 800.0, 60.0]),
                ("a", [0.0, 10.0, 0.0, 50.0]),
                ("b", [0.0, 40.0, 0.0, 20.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-grid-item.html"
            ),
            &[
                ("p", [0.0, 0.0, 800.0, 60.0]),
                ("a", [0.0, 10.0, 800.0, 50.0]),
                ("b", [0.0, 40.0, 800.0, 20.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-nested-negative.html"
            ),
            &[
                ("p", [0.0, -30.0, 800.0, 20.0]),
                ("a", [0.0, -30.0, 800.0, 20.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-siblings-negative.html"
            ),
            &[
                ("a", [0.0, 40.0, 800.0, 10.0]),
                ("b", [0.0, 30.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-overflow-clip.html"
            ),
            &[
                ("p", [0.0, 30.0, 800.0, 20.0]),
                ("a", [0.0, 30.0, 800.0, 20.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-empty-min-height.html"
            ),
            &[
                ("p", [0.0, 20.0, 800.0, 55.0]),
                ("e", [0.0, 20.0, 800.0, 5.0]),
                ("b", [0.0, 65.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-margin-empty-border.html"
            ),
            &[
                ("p", [0.0, 20.0, 800.0, 51.0]),
                ("e", [0.0, 20.0, 800.0, 1.0]),
                ("b", [0.0, 61.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-clear-large-margin-normal.html"
            ),
            &[
                ("p", [0.0, 1.0, 800.0, 60.0]),
                ("f", [0.0, 1.0, 30.0, 50.0]),
                ("a", [0.0, 51.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-clear-large-margin-flow-root.html"
            ),
            &[
                ("p", [0.0, 1.0, 800.0, 70.0]),
                ("f", [0.0, 1.0, 30.0, 50.0]),
                ("a", [0.0, 61.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-clear-negative-normal.html"
            ),
            &[
                ("p", [0.0, 1.0, 800.0, 60.0]),
                ("f", [0.0, 1.0, 30.0, 50.0]),
                ("a", [0.0, 51.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-clear-negative-flow-root.html"
            ),
            &[
                ("p", [0.0, 1.0, 800.0, 60.0]),
                ("f", [0.0, 1.0, 30.0, 50.0]),
                ("a", [0.0, 51.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-clear-zero-margin-normal.html"
            ),
            &[
                ("p", [0.0, 1.0, 800.0, 60.0]),
                ("f", [0.0, 1.0, 30.0, 50.0]),
                ("a", [0.0, 51.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-clear-zero-margin-flow-root.html"
            ),
            &[
                ("p", [0.0, 1.0, 800.0, 60.0]),
                ("f", [0.0, 1.0, 30.0, 50.0]),
                ("a", [0.0, 51.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-clear-positive-normal.html"
            ),
            &[
                ("p", [0.0, 1.0, 800.0, 60.0]),
                ("f", [0.0, 1.0, 30.0, 50.0]),
                ("a", [0.0, 51.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-clear-positive-flow-root.html"
            ),
            &[
                ("p", [0.0, 1.0, 800.0, 60.0]),
                ("f", [0.0, 1.0, 30.0, 50.0]),
                ("a", [0.0, 51.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-clear-zero-clearance-normal.html"
            ),
            &[
                ("p", [0.0, 1.0, 800.0, 60.0]),
                ("f", [0.0, 1.0, 30.0, 50.0]),
                ("a", [0.0, 51.0, 800.0, 10.0]),
            ],
        ),
        (
            include_str!(
                "../../tests/fixtures/anonymized-layout-corrections/anonymized-clear-zero-clearance-flow-root.html"
            ),
            &[
                ("p", [0.0, 1.0, 800.0, 60.0]),
                ("f", [0.0, 1.0, 30.0, 50.0]),
                ("a", [0.0, 51.0, 800.0, 10.0]),
            ],
        ),
    ];
    let mut mismatches = Vec::new();
    for (html, expected) in cases {
        let css = html
            .split_once("<style>")
            .unwrap()
            .1
            .split_once("</style>")
            .unwrap()
            .0;
        let root = render(html, css);
        for (id, expected) in *expected {
            let r = by_id(&root, id).dimensions.border_box();
            let actual = [r.x, r.y, r.width, r.height];
            if actual != *expected {
                mismatches.push(format!("#{id}: {actual:?}, expected {expected:?}; {css}"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn cleared_child_uses_the_full_width_below_the_float() {
    let root = render(
        "<div id='p'><div id='f'></div><div id='a'></div></div>",
        "html,body{margin:0;padding:0}body{padding-top:1px}#p{display:flow-root}#f{float:left;width:30px;height:50px}#a{clear:both;margin-top:10px;height:10px}",
    );
    assert_eq!(
        by_id(&root, "a").dimensions.border_box(),
        Rect {
            x: 0.0,
            y: 51.0,
            width: 800.0,
            height: 10.0
        }
    );
}
