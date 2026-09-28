use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Color, render_document_pages};

#[test]
fn forced_right_break_inserts_a_content_empty_page_without_reordering_boxes() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            body { margin: 0 }
            @page { size: 100px 100px; margin: 0 }
            div { width: 20px; height: 20px }
            #first { background: red }
            #second { break-before: right; background: blue }
        </style></head><body>
            <div id="first"></div><div id="second"></div>
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
    assert!(
        pages[1]
            .pixels()
            .chunks_exact(4)
            .all(|pixel| pixel == [255, 255, 255, 255])
    );
    assert_eq!(pages[2].pixel(5, 5), Some(Color::rgb(0, 0, 255)));
}
