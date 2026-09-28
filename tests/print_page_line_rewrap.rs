use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Color, render_document_pages};

#[test]
fn continued_text_paints_at_the_later_pages_wider_line_width() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            body, p { margin: 0 }
            p { font-size: 10px; line-height: 20px; word-break: break-all; color: red }
            @page { size: 200px 40px; margin: 0 }
            @page :first { size: 80px 40px; margin: 0 }
        </style></head><body><p>ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz</p></body></html>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 80.0,
            height: 40.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!((pages[0].width(), pages[1].width()), (80, 200));
    assert!((140..190).any(|x| {
        (0..40).any(|y| {
            pages[1]
                .pixel(x, y)
                .is_some_and(|pixel| pixel != Color::rgb(255, 255, 255))
        })
    }));
}

#[test]
fn continued_text_paints_on_pages_with_narrower_line_width() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            body, p { margin: 0 }
            p { font-size: 10px; line-height: 20px; word-break: break-all; color: red }
            @page { size: 80px 40px; margin: 0 }
            @page :first { size: 200px 40px; margin: 0 }
        </style></head><body><p>ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz</p></body></html>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 200.0,
            height: 40.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert!(pages.len() >= 3);
    assert_eq!(pages[0].width(), 200);
    assert!(pages.iter().skip(1).all(|page| page.width() == 80));
    assert!(pages.iter().skip(1).all(|page| {
        (0..80).any(|x| {
            (0..40).any(|y| {
                page.pixel(x, y)
                    .is_some_and(|pixel| pixel != Color::rgb(255, 255, 255))
            })
        })
    }));
}
