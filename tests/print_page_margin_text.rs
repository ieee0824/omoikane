use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Canvas, Color, render_document_pages};

fn colored_bounds(
    page: &Canvas,
    area: (u32, u32, u32, u32),
    predicate: impl Fn(Color) -> bool,
) -> Option<(u32, u32, u32, u32)> {
    let (left, top, right, bottom) = area;
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for y in top..bottom {
        for x in left..right {
            if page.pixel(x, y).is_some_and(&predicate) {
                bounds = Some(match bounds {
                    Some((min_x, min_y, max_x, max_y)) => {
                        (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
                    }
                    None => (x, y, x, y),
                });
            }
        }
    }
    bounds
}

#[test]
fn printed_margin_text_uses_page_color_font_and_box_alignment() {
    let document =
        TreeBuilder::parse(include_str!("fixtures/print/page-margin-text.html")).document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 200.0,
            height: 200.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(pages.len(), 1);
    let page = &pages[0];
    let top_red = colored_bounds(page, (40, 0, 160, 40), |pixel| {
        pixel.r > 200 && pixel.g < 100 && pixel.b < 100
    })
    .expect("top-left red text");
    let top_blue = colored_bounds(page, (40, 0, 160, 40), |pixel| {
        pixel.b > 200 && pixel.r < 100 && pixel.g < 100
    })
    .expect("top-center blue text");
    let top_purple = colored_bounds(page, (40, 0, 160, 40), |pixel| {
        pixel.r > 100 && pixel.b > 100 && pixel.g < 100
    })
    .expect("top-right purple text");
    assert!(top_red.0 < top_blue.0 && top_blue.0 < top_purple.0);
    let left_green = colored_bounds(page, (0, 40, 40, 160), |pixel| {
        pixel.g > 100 && pixel.r < 100 && pixel.b < 100
    })
    .expect("left-middle green text");
    let right_orange = colored_bounds(page, (160, 40, 200, 160), |pixel| {
        pixel.r > 100 && pixel.g > 50 && pixel.b < 100
    })
    .expect("right-middle orange text");
    assert!(left_green.1 > right_orange.1);
    assert!(top_red.3 - top_red.1 > left_green.3 - left_green.1);
    assert!(
        colored_bounds(page, (40, 160, 100, 200), |pixel| {
            pixel.r > 100 && pixel.g < 80 && pixel.b < 80
        })
        .is_some()
    );
    assert!(
        colored_bounds(page, (40, 160, 160, 200), |pixel| {
            pixel.r < 100 && pixel.g < 100 && pixel.b < 100
        })
        .is_some()
    );
    assert_eq!(page.pixel(80, 80), Some(Color::rgb(255, 255, 255)));
    if let Ok(directory) = std::env::var("OMOIKANE_PRINT_ARTIFACTS") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(format!("{directory}/page-1.png"), page.encode_png()).unwrap();
    }
}
