use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Color, render_document_pages};

#[test]
fn paragraph_lines_continue_on_the_next_page_without_partial_line_paint() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            @page { size: 100px 45px; margin: 0 }
            body, p { margin: 0 }
            p { font-size: 16px; line-height: 20px }
        </style></head><body><p>MMMM<br>MMMM<br>MMMM</p></body></html>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 100.0,
            height: 45.0,
            ..Rect::default()
        },
    )
    .unwrap();

    assert_eq!(pages.len(), 2);
    let has_ink = |page: usize, y_start: u32, y_end: u32| {
        (y_start..y_end)
            .any(|y| (0..100).any(|x| pages[page].pixel(x, y) != Some(Color::rgb(255, 255, 255))))
    };
    assert!(has_ink(0, 0, 20));
    assert!(has_ink(0, 20, 40));
    assert!(!has_ink(0, 40, 45));
    assert!(has_ink(1, 0, 20));
    assert!(!has_ink(1, 20, 45));
}

#[test]
fn inline_backgrounds_remain_with_their_lines_after_a_page_break() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            @page { size: 100px 45px; margin: 0 }
            body, p { margin: 0 }
            p { font-size: 16px; line-height: 20px }
        </style></head><body><p><span style="background: red">MMMM</span><br><span style="background: lime">MMMM</span><br><span style="background: blue">MMMM</span></p></body></html>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 100.0,
            height: 45.0,
            ..Rect::default()
        },
    )
    .unwrap();

    assert_eq!(pages.len(), 2);
    let has_color = |page: usize, y_start: u32, y_end: u32, color: Color| {
        (y_start..y_end).any(|y| (0..100).any(|x| pages[page].pixel(x, y) == Some(color)))
    };
    assert!(has_color(0, 0, 20, Color::rgb(255, 0, 0)));
    assert!(has_color(0, 20, 40, Color::rgb(0, 255, 0)));
    assert!(!has_color(0, 0, 45, Color::rgb(0, 0, 255)));
    assert!(has_color(1, 0, 20, Color::rgb(0, 0, 255)));
    assert!(!has_color(1, 0, 45, Color::rgb(255, 0, 0)));
    assert!(!has_color(1, 0, 45, Color::rgb(0, 255, 0)));
}
