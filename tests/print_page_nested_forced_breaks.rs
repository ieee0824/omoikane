use omoikane::css::{Origin, StyleResolver, parse_stylesheet};
use omoikane::html::TreeBuilder;
use omoikane::layout::{Rect, layout_paged_tree};
use omoikane::paint::{Color, render_document_pages};

#[test]
fn nested_break_before_moves_the_child_and_following_flow_to_a_new_page() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            @page { size: 100px 100px; margin: 0 }
            body, section { margin: 0 }
            section > div, #after { height: 20px }
            #first { background: red }
            #second { background: blue; break-before: page }
            #after { background: yellow }
        </style></head><body>
            <section><div id="first"></div><div id="second"></div></section>
            <div id="after"></div>
        </body></html>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 100.0,
            height: 100.0,
            ..Rect::default()
        },
    )
    .unwrap();

    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].pixel(5, 5), Some(Color::rgb(255, 0, 0)));
    assert_eq!(pages[0].pixel(5, 25), Some(Color::rgb(255, 255, 255)));
    assert_eq!(pages[1].pixel(5, 5), Some(Color::rgb(0, 0, 255)));
    assert_eq!(pages[1].pixel(5, 25), Some(Color::rgb(255, 255, 0)));
}

#[test]
fn nested_break_after_moves_the_next_child_and_following_flow() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            @page { size: 100px 100px; margin: 0 }
            body, section { margin: 0 }
            section > div, #after { height: 20px }
            #first { background: red; break-after: page }
            #second { background: blue }
            #after { background: yellow }
        </style></head><body>
            <section><div id="first"></div><div id="second"></div></section>
            <div id="after"></div>
        </body></html>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 100.0,
            height: 100.0,
            ..Rect::default()
        },
    )
    .unwrap();

    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].pixel(5, 5), Some(Color::rgb(255, 0, 0)));
    assert_eq!(pages[0].pixel(5, 25), Some(Color::rgb(255, 255, 255)));
    assert_eq!(pages[1].pixel(5, 5), Some(Color::rgb(0, 0, 255)));
    assert_eq!(pages[1].pixel(5, 25), Some(Color::rgb(255, 255, 0)));
}

#[test]
fn nested_side_breaks_insert_only_the_needed_blank_page() {
    for (side, expected_pages) in [("left", 2), ("right", 3)] {
        let document = TreeBuilder::parse(&format!(
            r#"<!doctype html><html><head><style>
                @page {{ size: 100px 100px; margin: 0 }}
                body, section {{ margin: 0 }}
                section > div {{ height: 20px }}
                #first {{ background: red }}
                #second {{ background: blue; break-before: {side} }}
            </style></head><body>
                <section><div id="first"></div><div id="second"></div></section>
            </body></html>"#
        ))
        .document();
        let pages = render_document_pages(
            &document,
            Rect {
                width: 100.0,
                height: 100.0,
                ..Rect::default()
            },
        )
        .unwrap();
        assert_eq!(pages.len(), expected_pages, "{side}");
        assert_eq!(pages[0].pixel(5, 5), Some(Color::rgb(255, 0, 0)));
        assert_eq!(
            pages.last().unwrap().pixel(5, 5),
            Some(Color::rgb(0, 0, 255))
        );
        if expected_pages == 3 {
            assert_eq!(pages[1].pixel(5, 5), Some(Color::rgb(255, 255, 255)));
        }
    }
}

#[test]
fn first_nested_side_break_sets_the_first_page_side_without_a_blank() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><body><section><div id="first"></div></section></body></html>"#,
    )
    .document();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@page { size: 100px 100px; margin: 0 } \
             body, section { margin: 0 } #first { height: 20px; break-before: left }",
        )
        .unwrap(),
    );
    let paged = layout_paged_tree(
        &document,
        &mut resolver,
        Rect {
            width: 100.0,
            height: 100.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(paged.pages.len(), 1);
    assert_eq!(paged.pages[0].selector.side, omoikane::css::PageSide::Left);
    assert!(!paged.pages[0].selector.blank);
}

#[test]
fn side_break_inside_a_float_does_not_set_the_first_page_side() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><body><section><div id="first"></div></section></body></html>"#,
    )
    .document();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@page { size: 100px 100px; margin: 0 } \
             body, section { margin: 0 } section { float: left } \
             #first { height: 20px; break-before: left }",
        )
        .unwrap(),
    );
    let paged = layout_paged_tree(
        &document,
        &mut resolver,
        Rect {
            width: 100.0,
            height: 100.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(paged.pages.len(), 1);
    assert_eq!(paged.pages[0].selector.side, omoikane::css::PageSide::Right);
}

#[test]
fn adjacent_nested_breaks_do_not_duplicate_content_or_pages() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            @page { size: 100px 100px; margin: 0 }
            body, section { margin: 0 }
            section > div { height: 20px }
            #first { background: red; break-after: left }
            #second { background: blue; break-before: right }
        </style></head><body>
            <section><div id="first"></div><div id="second"></div></section>
        </body></html>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 100.0,
            height: 100.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(pages.len(), 3);
    assert_eq!(pages[0].pixel(5, 5), Some(Color::rgb(255, 0, 0)));
    assert_eq!(pages[1].pixel(5, 5), Some(Color::rgb(255, 255, 255)));
    assert_eq!(pages[2].pixel(5, 5), Some(Color::rgb(0, 0, 255)));
}

#[test]
fn nested_named_page_returns_to_the_following_sibling_name() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><body>
            <section><div id="first"></div><div id="second"></div></section>
            <div id="after"></div>
        </body></html>"#,
    )
    .document();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@page { size: 100px 100px; margin: 0 } \
             body, section { margin: 0 } section > div, #after { height: 20px } \
             #second { page: chapter; break-before: page } \
             #after { break-before: page }",
        )
        .unwrap(),
    );
    let paged = layout_paged_tree(
        &document,
        &mut resolver,
        Rect {
            width: 100.0,
            height: 100.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(paged.pages.len(), 3);
    assert_eq!(paged.pages[0].selector.name, None);
    assert_eq!(paged.pages[1].selector.name.as_deref(), Some("chapter"));
    assert_eq!(paged.pages[2].selector.name, None);
}

#[test]
fn first_nested_named_page_uses_its_page_geometry() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><body><section><div id="first"></div></section></body></html>"#,
    )
    .document();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@page { size: 100px 100px; margin: 0 } \
             @page chapter { size: 80px 80px; margin: 0 } \
             body, section { margin: 0 } #first { height: 20px; page: chapter }",
        )
        .unwrap(),
    );
    let paged = layout_paged_tree(
        &document,
        &mut resolver,
        Rect {
            width: 100.0,
            height: 100.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(paged.pages.len(), 1);
    assert_eq!(paged.pages[0].selector.name.as_deref(), Some("chapter"));
    assert_eq!(paged.pages[0].geometry.width, 80.0);
}
