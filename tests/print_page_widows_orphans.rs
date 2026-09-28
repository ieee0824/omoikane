use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Color, render_document_pages};

#[test]
fn widows_and_orphans_keep_three_lines_on_each_page() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            @page { size: 100px 85px; margin: 0 }
            body, p { margin: 0 }
            p { font-size: 16px; line-height: 20px; orphans: 3; widows: 3 }
        </style></head><body><p>MMMM<br>MMMM<br>MMMM<br>MMMM<br>MMMM<br>MMMM</p></body></html>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 100.0,
            height: 85.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(pages.len(), 2);
    let has_ink = |page: usize, start_y: u32, end_y: u32| {
        (start_y..end_y)
            .any(|y| (0..100).any(|x| pages[page].pixel(x, y) != Some(Color::rgb(255, 255, 255))))
    };
    assert!(has_ink(0, 40, 60));
    assert!(!has_ink(0, 60, 85));
    assert!(has_ink(1, 40, 60));
    assert!(!has_ink(1, 60, 85));
}

#[test]
fn unconstrained_lines_use_the_last_complete_line_that_fits() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            @page { size: 100px 85px; margin: 0 }
            body, p { margin: 0 }
            p { font-size: 16px; line-height: 20px; orphans: 1; widows: 1 }
        </style></head><body><p>MMMM<br>MMMM<br>MMMM<br>MMMM<br>MMMM<br>MMMM</p></body></html>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 100.0,
            height: 85.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(pages.len(), 2);
    let has_ink = |page: usize, start_y: u32, end_y: u32| {
        (start_y..end_y)
            .any(|y| (0..100).any(|x| pages[page].pixel(x, y) != Some(Color::rgb(255, 255, 255))))
    };
    assert!(has_ink(0, 60, 80));
    assert!(!has_ink(0, 80, 85));
    assert!(has_ink(1, 20, 40));
    assert!(!has_ink(1, 40, 85));
}

#[test]
fn impossible_line_minima_still_advance_one_complete_line_at_a_time() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            @page { size: 100px 45px; margin: 0 }
            body, p { margin: 0 }
            p { font-size: 16px; line-height: 20px; orphans: 3; widows: 3 }
        </style></head><body><p>MMMM<br>MMMM<br>MMMM<br>MMMM<br>MMMM</p></body></html>"#,
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
    assert_eq!(pages.len(), 3);
    let has_ink = |page: usize, start_y: u32, end_y: u32| {
        (start_y..end_y)
            .any(|y| (0..100).any(|x| pages[page].pixel(x, y) != Some(Color::rgb(255, 255, 255))))
    };
    assert!(has_ink(0, 20, 40));
    assert!(!has_ink(0, 40, 45));
    assert!(has_ink(1, 20, 40));
    assert!(!has_ink(1, 40, 45));
    assert!(has_ink(2, 0, 20));
    assert!(!has_ink(2, 20, 45));
}

#[test]
fn nested_paragraph_keeps_widows_and_orphans_across_page_slices() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            @page { size: 100px 85px; margin: 0 }
            body, section, p { margin: 0 }
            p { font-size: 16px; line-height: 20px; orphans: 3; widows: 3 }
        </style></head><body><section><p>MMMM<br>MMMM<br>MMMM<br>MMMM<br>MMMM<br>MMMM</p></section></body></html>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 100.0,
            height: 85.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(pages.len(), 2);
    let has_ink = |page: usize, start_y: u32, end_y: u32| {
        (start_y..end_y)
            .any(|y| (0..100).any(|x| pages[page].pixel(x, y) != Some(Color::rgb(255, 255, 255))))
    };
    assert!(has_ink(0, 40, 60));
    assert!(!has_ink(0, 60, 85));
    assert!(has_ink(1, 40, 60));
    assert!(!has_ink(1, 60, 85));
}
