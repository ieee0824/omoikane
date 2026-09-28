use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Color, render_document, render_document_pages};

#[test]
fn later_named_page_relayouts_block_and_percentage_width_child() {
    let document = TreeBuilder::parse(
        r#"<style>
            @page narrow { size: 120px 100px; margin: 10px }
            @page wide { size: 200px 100px; margin: 10px }
            body { margin: 0 }
            section { height: 40px }
            #first { page: narrow; height: 80px; background: green }
            #second { page: wide; background: blue }
            #inner { width: 50%; height: 20px; margin-left: auto; background: red }
        </style>
        <section id="first"></section>
        <section id="second"><div id="inner"></div></section>"#,
    )
    .document();
    let sheet = Rect {
        width: 120.0,
        height: 100.0,
        ..Rect::default()
    };
    let pages = render_document_pages(&document, sheet).unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!((pages[1].width(), pages[1].height()), (200, 100));
    assert_eq!(pages[1].pixel(50, 20), Some(Color::rgb(0, 0, 255)));
    assert_eq!(pages[1].pixel(150, 20), Some(Color::rgb(255, 0, 0)));

    let screen = render_document(&document, sheet).unwrap();
    assert_eq!(screen.pixel(50, 95), Some(Color::rgb(0, 0, 255)));
    assert_eq!((screen.width(), screen.height()), (120, 100));
}

#[test]
fn later_page_margins_relayout_block_and_child_within_content() {
    let document = TreeBuilder::parse(
        r#"<style>
            @page first { size: 200px 100px; margin: 10px }
            @page second { size: 200px 100px; margin: 10px 40px }
            body { margin: 0 }
            #first { page: first; height: 80px }
            #second { page: second; height: 40px; background: blue }
            #inner { width: 50%; height: 20px; margin-left: auto; background: red }
        </style>
        <section id="first"></section>
        <section id="second"><div id="inner"></div></section>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 200.0,
            height: 100.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[1].pixel(40, 20), Some(Color::rgb(0, 0, 255)));
    assert_eq!(pages[1].pixel(99, 20), Some(Color::rgb(0, 0, 255)));
    assert_eq!(pages[1].pixel(100, 20), Some(Color::rgb(255, 0, 0)));
    assert_eq!(pages[1].pixel(159, 20), Some(Color::rgb(255, 0, 0)));
    assert_eq!(pages[1].pixel(160, 20), Some(Color::rgb(255, 255, 255)));
}

#[test]
fn each_page_uses_container_query_styles_for_its_own_width() {
    let document = TreeBuilder::parse(
        r#"<style>
            @page narrow { size: 120px 100px; margin: 10px }
            @page wide { size: 200px 100px; margin: 10px }
            body { margin: 0 }
            section { height: 80px; container-type: inline-size }
            #first { page: narrow }
            #second { page: wide }
            .item { width: 20px; height: 20px; background: blue }
            @container (width >= 150px) { .item { background: red } }
        </style>
        <section id="first"><div class="item"></div></section>
        <section id="second"><div class="item"></div></section>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 120.0,
            height: 100.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].pixel(15, 15), Some(Color::rgb(0, 0, 255)));
    assert_eq!(pages[1].pixel(15, 15), Some(Color::rgb(255, 0, 0)));
}
