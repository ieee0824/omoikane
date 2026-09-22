use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Color, render_document, render_document_pages};

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
    if let Ok(directory) = std::env::var("OMOIKANE_PRINT_ARTIFACTS") {
        std::fs::create_dir_all(&directory).unwrap();
        for (index, page) in pages.iter().enumerate() {
            std::fs::write(
                format!("{directory}/page-{}.png", index + 1),
                page.encode_png(),
            )
            .unwrap();
        }
    }
}
