use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Color, render_document, render_document_pages};

#[path = "common/print.rs"]
mod print_support;

fn blue_bounds(page: &omoikane::paint::Canvas) -> Option<(u32, u32, u32, u32)> {
    print_support::color_bounds(page, (0, 0, page.width(), page.height()), |pixel| {
        pixel == Color::rgb(0, 0, 255)
    })
}

#[test]
fn named_page_margins_place_each_box_on_its_own_sheet() {
    let document = TreeBuilder::parse(include_str!("fixtures/print/page-layers.html")).document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 120.0,
            height: 120.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].pixel(10, 10), Some(Color::rgb(0, 0, 255)));
    assert_eq!(pages[0].pixel(10, 30), Some(Color::rgb(255, 255, 255)));
    assert_eq!(pages[0].pixel(35, 35), Some(Color::rgb(255, 255, 255)));
    assert_eq!(pages[1].pixel(10, 10), Some(Color::rgb(255, 255, 255)));
    assert_eq!(pages[1].pixel(35, 35), Some(Color::rgb(255, 0, 0)));
    let screen = render_document(
        &document,
        Rect {
            width: 120.0,
            height: 120.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(screen.pixel(10, 10), Some(Color::rgb(0, 0, 255)));
    assert_eq!(screen.pixel(10, 30), Some(Color::rgb(255, 0, 0)));
    print_support::save_pages(&pages, None);
}

#[test]
fn named_page_orientation_rotates_after_layout_in_both_directions() {
    for (name, fixture, expected_blue, corner) in [
        (
            "left",
            include_str!("fixtures/print/page-orientation-left.html"),
            (10, 120, 44, 189),
            (0, 190),
        ),
        (
            "right",
            include_str!("fixtures/print/page-orientation-right.html"),
            (255, 10, 289, 79),
            (290, 0),
        ),
    ] {
        let pages = print_support::render_pages(fixture, 200.0, 300.0);
        assert_eq!(pages.len(), 2, "{name} page count");
        assert_eq!((pages[0].width(), pages[0].height()), (200, 300));
        assert_eq!(pages[0].pixel(10, 10), Some(Color::rgb(255, 0, 0)));
        assert_eq!((pages[1].width(), pages[1].height()), (300, 200));
        assert_eq!(
            blue_bounds(&pages[1]),
            Some(expected_blue),
            "{name} blue block"
        );
        assert_eq!(
            pages[1].pixel(corner.0, corner.1),
            Some(Color::rgb(0, 255, 0))
        );
        assert!(
            pages[1]
                .pixels()
                .chunks_exact(4)
                .any(|pixel| { pixel[0] > 180 && pixel[1] < 80 && pixel[2] > 180 && pixel[3] > 0 }),
            "{name} page counter text"
        );
        print_support::save_pages(&pages, Some(&format!("orientation-{name}")));
    }
}

#[test]
fn landscape_size_changes_layout_without_rotating_the_page() {
    let source = include_str!("fixtures/print/page-orientation-left.html")
        .replace("page-orientation: rotate-left", "size: 300px 200px");
    let pages = print_support::render_pages(&source, 200.0, 300.0);
    assert_eq!(pages.len(), 2);
    assert_eq!((pages[0].width(), pages[0].height()), (200, 300));
    assert_eq!((pages[1].width(), pages[1].height()), (300, 200));
    assert_eq!(blue_bounds(&pages[1]), Some((10, 10, 79, 44)));
}
