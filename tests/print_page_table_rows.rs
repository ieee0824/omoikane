use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Color, render_document_pages};

#[test]
fn table_rows_continue_on_separate_pages_with_stable_columns() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            @page { size: 100px 50px; margin: 0 }
            body { margin: 0 }
            table { width: 100px; table-layout: fixed; border-spacing: 0 }
            td { height: 30px; padding: 0; border: 0 }
            #after { height: 20px; background: black }
        </style></head><body>
            <table>
              <tr><td style="background:red"></td><td style="background:yellow"></td></tr>
              <tr><td style="background:lime"></td><td style="background:cyan"></td></tr>
              <tr><td style="background:blue"></td><td style="background:magenta"></td></tr>
            </table>
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

    assert_eq!(pages.len(), 3);
    for (page, left, right) in [
        (&pages[0], Color::rgb(255, 0, 0), Color::rgb(255, 255, 0)),
        (&pages[1], Color::rgb(0, 255, 0), Color::rgb(0, 255, 255)),
        (&pages[2], Color::rgb(0, 0, 255), Color::rgb(255, 0, 255)),
    ] {
        assert_eq!(page.pixel(10, 10), Some(left));
        assert_eq!(page.pixel(75, 10), Some(right));
    }
    assert_eq!(pages[0].pixel(10, 40), Some(Color::rgb(255, 255, 255)));
    assert_eq!(pages[1].pixel(10, 40), Some(Color::rgb(255, 255, 255)));
    assert_eq!(pages[2].pixel(10, 40), Some(Color::rgb(0, 0, 0)));
}

#[test]
fn row_group_rows_are_available_as_page_boundaries() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            @page { size: 100px 50px; margin: 0 }
            body { margin: 0 }
            table { width: 100px; table-layout: fixed; border-spacing: 0 }
            td { height: 30px; padding: 0; border: 0 }
        </style></head><body><table><tbody>
            <tr><td style="background:red"></td><td style="background:yellow"></td></tr>
            <tr><td style="background:blue"></td><td style="background:cyan"></td></tr>
        </tbody></table></body></html>"#,
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
    assert_eq!(pages[0].pixel(10, 10), Some(Color::rgb(255, 0, 0)));
    assert_eq!(pages[0].pixel(75, 10), Some(Color::rgb(255, 255, 0)));
    assert_eq!(pages[0].pixel(10, 40), Some(Color::rgb(255, 255, 255)));
    assert_eq!(pages[1].pixel(10, 10), Some(Color::rgb(0, 0, 255)));
    assert_eq!(pages[1].pixel(75, 10), Some(Color::rgb(0, 255, 255)));
}

#[test]
fn oversized_rowspan_does_not_hide_the_table_end_boundary() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
            @page { size: 100px 65px; margin: 0 }
            body { margin: 0 }
            table { width: 100px; table-layout: fixed; border-spacing: 0 }
            td { height: 30px; padding: 0; border: 0 }
            #after { height: 30px; background: black }
        </style></head><body>
            <table><tr><td rowspan="99" style="background:red"></td><td style="background:yellow"></td></tr>
            <tr><td style="background:blue"></td></tr></table>
            <div id="after"></div>
        </body></html>"#,
    )
    .document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 100.0,
            height: 65.0,
            ..Rect::default()
        },
    )
    .unwrap();

    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].pixel(75, 40), Some(Color::rgb(0, 0, 255)));
    assert_eq!(pages[0].pixel(10, 62), Some(Color::rgb(255, 255, 255)));
    assert_eq!(pages[1].pixel(10, 5), Some(Color::rgb(0, 0, 0)));
    assert_eq!(pages[1].pixel(10, 25), Some(Color::rgb(0, 0, 0)));
}
