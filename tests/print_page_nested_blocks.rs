use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Color, render_document_pages};

#[test]
fn nested_children_break_at_their_boundary_and_keep_parent_decoration() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            @page { size: 100px 50px; margin: 0 }
            body { margin: 0 }
            #parent { border: 2px solid black; background: lime }
            #parent > div { height: 30px }
            #first { background: blue }
            #second { background: red }
            #after { height: 15px; background: yellow }
        </style></head><body>
            <section id="parent"><div id="first"></div><div id="second"></div></section>
            <div id="after"></div>
        </body></html>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 100.0,
            height: 50.0,
            ..Rect::default()
        },
    )
    .unwrap();

    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].pixel(0, 5), Some(Color::rgb(0, 0, 0)));
    assert_eq!(pages[0].pixel(10, 5), Some(Color::rgb(0, 0, 255)));
    assert_eq!(pages[0].pixel(10, 35), Some(Color::rgb(255, 255, 255)));
    assert_eq!(pages[1].pixel(0, 5), Some(Color::rgb(0, 0, 0)));
    assert_eq!(pages[1].pixel(10, 5), Some(Color::rgb(255, 0, 0)));
    assert_eq!(pages[1].pixel(10, 30), Some(Color::rgb(0, 0, 0)));
    assert_eq!(pages[1].pixel(10, 40), Some(Color::rgb(255, 255, 0)));
}
