use omoikane::paint::Canvas;

#[path = "common/print.rs"]
mod print_support;

fn contains_marker(page: &Canvas, marker: &str) -> bool {
    print_support::color_bounds(
        page,
        (0, 0, page.width(), page.height()),
        |pixel| match marker {
            "red" => pixel.r > 180 && pixel.g < 80 && pixel.b < 80,
            "yellow" => pixel.r > 180 && pixel.g > 180 && pixel.b < 80,
            "blue" => pixel.b > 180 && pixel.r < 80 && pixel.g < 80,
            "cyan" => pixel.b > 180 && pixel.g > 180 && pixel.r < 80,
            "lime" => pixel.g > 180 && pixel.r < 80 && pixel.b < 80,
            "magenta" => pixel.r > 180 && pixel.b > 180 && pixel.g < 80,
            _ => unreachable!(),
        },
    )
    .is_some()
}

#[test]
fn named_pages_nested_breaks_text_and_table_continue_in_order() {
    let pages = print_support::render_pages(
        include_str!("fixtures/print/combined-pagination.html"),
        100.0,
        60.0,
    );
    print_support::save_pages(&pages, Some("combined-pagination"));

    assert_eq!(pages.len(), 10);
    assert_eq!((pages[0].width(), pages[0].height()), (100, 60));
    for (index, page) in pages.iter().enumerate().skip(1) {
        assert_eq!((page.width(), page.height()), (140, 60), "page {index}");
    }

    let markers = ["red", "yellow", "blue", "cyan", "lime", "magenta"];
    let expected: [&[&str]; 10] = [
        &["red"],
        &[],
        &["lime"],
        &["blue"],
        &[],
        &["red"],
        &["yellow", "blue"],
        &["cyan", "lime"],
        &[],
        &["magenta"],
    ];
    for (index, (page, expected)) in pages.iter().zip(expected).enumerate() {
        for name in markers {
            assert_eq!(
                contains_marker(page, name),
                expected.contains(&name),
                "page {index} {name}"
            );
        }
    }
    for index in [1, 8] {
        assert!(
            pages[index]
                .pixels()
                .chunks_exact(4)
                .all(|pixel| pixel == [255, 255, 255, 255]),
            "blank page {}",
            index + 1
        );
    }
    assert!(
        print_support::color_bounds(&pages[4], (5, 5, 135, 55), |pixel| {
            pixel.r < 60 && pixel.g < 60 && pixel.b < 60
        })
        .is_some(),
        "long paragraph reaches page 5"
    );
}
