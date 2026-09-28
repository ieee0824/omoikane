use omoikane::paint::Canvas;

#[path = "common/print.rs"]
mod print_support;

fn has_marker(page: &Canvas, marker: &str) -> bool {
    print_support::color_bounds(
        page,
        (0, 0, page.width(), page.height()),
        |pixel| match marker {
            "red" => pixel.r > 180 && pixel.g < 80 && pixel.b < 80,
            "yellow" => pixel.r > 180 && pixel.g > 180 && pixel.b < 80,
            "blue" => pixel.b > 180 && pixel.r < 80 && pixel.g < 80,
            "cyan" => pixel.g > 180 && pixel.b > 180 && pixel.r < 80,
            "lime" => pixel.g > 180 && pixel.r < 80 && pixel.b < 80,
            "magenta" => pixel.r > 180 && pixel.b > 180 && pixel.g < 80,
            "orange" => pixel.r > 180 && (90..170).contains(&pixel.g) && pixel.b < 80,
            _ => unreachable!(),
        },
    )
    .is_some()
}

fn assert_line_groups(pages: &[Canvas]) {
    assert_eq!(pages.len(), 3);
    for (index, expected) in [
        [true, true, false, false, false],
        [false, false, true, true, false],
        [false, false, false, false, true],
    ]
    .into_iter()
    .enumerate()
    {
        for (marker, present) in ["red", "yellow", "blue", "cyan", "lime"]
            .into_iter()
            .zip(expected)
        {
            assert_eq!(
                has_marker(&pages[index], marker),
                present,
                "page {index} {marker}"
            );
        }
    }
    assert!(
        print_support::color_bounds(&pages[2], (0, 20, 70, 40), |pixel| {
            pixel.r < 60 && pixel.g < 60 && pixel.b < 60
        })
        .is_some(),
        "content after the table must reach the final page"
    );
}

#[test]
fn oversized_table_cell_keeps_text_lines_on_complete_pages() {
    let pages = print_support::render_pages(
        include_str!("fixtures/print/tall-table-cell.html"),
        100.0,
        45.0,
    );
    print_support::save_pages(&pages, Some("tall-table-cell"));
    assert_line_groups(&pages);
}

#[test]
fn rowspanned_cell_keeps_lines_and_neighbor_cells_in_order() {
    let pages = print_support::render_pages(
        include_str!("fixtures/print/tall-table-rowspan.html"),
        160.0,
        45.0,
    );
    print_support::save_pages(&pages, Some("tall-table-rowspan"));
    assert_line_groups(&pages);
    assert!(has_marker(&pages[0], "magenta"));
    assert!(!has_marker(&pages[0], "orange"));
    assert!(!has_marker(&pages[1], "magenta"));
    assert!(has_marker(&pages[1], "orange"));
    assert!(!has_marker(&pages[2], "magenta"));
    assert!(!has_marker(&pages[2], "orange"));
}

#[test]
fn single_line_taller_than_a_page_still_reaches_following_content() {
    let pages = print_support::render_pages(
        include_str!("fixtures/print/tall-table-line.html"),
        100.0,
        45.0,
    );
    print_support::save_pages(&pages, Some("tall-table-line"));
    assert_eq!(pages.len(), 2);
    assert!(has_marker(&pages[0], "red"));
    assert!(!has_marker(&pages[1], "red"));
    assert!(
        print_support::color_bounds(&pages[1], (0, 0, 70, 40), |pixel| {
            pixel.r < 60 && pixel.g < 60 && pixel.b < 60
        })
        .is_some(),
        "following content is not lost after an oversized line"
    );
}
