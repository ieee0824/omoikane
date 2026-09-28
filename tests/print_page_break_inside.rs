use omoikane::paint::{Canvas, Color};

#[path = "common/print.rs"]
mod print_support;

fn pixel(page: &Canvas, x: u32, y: u32) -> Color {
    page.pixel(x, y).expect("pixel within printed page")
}

fn has_color(page: &Canvas, color: Color) -> bool {
    print_support::color_bounds(page, (80, 0, 100, page.height()), |pixel| pixel == color).is_some()
}

#[test]
fn avoid_page_moves_a_fitting_block_before_its_first_line() {
    let pages = print_support::render_pages(
        include_str!("fixtures/print/break-inside-avoid-page.html"),
        100.0,
        45.0,
    );
    print_support::save_pages(&pages, Some("break-inside-avoid-page"));
    assert_eq!(pages.len(), 2);
    assert_eq!(pixel(&pages[0], 90, 10), Color::rgb(255, 0, 0));
    assert_eq!(pixel(&pages[0], 90, 20), Color::rgb(255, 255, 255));
    assert_eq!(pixel(&pages[1], 90, 10), Color::rgb(0, 0, 255));
    assert_eq!(pixel(&pages[1], 90, 42), Color::rgb(0, 255, 0));
}

#[test]
fn avoid_keyword_also_keeps_a_fitting_block_together() {
    let fixture = include_str!("fixtures/print/break-inside-avoid-page.html");
    let pages = print_support::render_pages(&fixture.replace("avoid-page", "avoid"), 100.0, 45.0);
    assert_eq!(pages.len(), 2);
    assert!(!has_color(&pages[0], Color::rgb(0, 0, 255)));
    assert!(has_color(&pages[1], Color::rgb(0, 0, 255)));
}

#[test]
fn avoid_page_on_an_oversized_block_still_reaches_following_content() {
    let pages = print_support::render_pages(
        include_str!("fixtures/print/break-inside-oversized.html"),
        100.0,
        45.0,
    );
    print_support::save_pages(&pages, Some("break-inside-oversized"));
    assert_eq!(pages.len(), 4);
    assert!(has_color(&pages[0], Color::rgb(255, 0, 0)));
    assert!(!has_color(&pages[0], Color::rgb(0, 0, 255)));
    assert!(has_color(&pages[1], Color::rgb(0, 0, 255)));
    assert!(has_color(&pages[2], Color::rgb(0, 0, 255)));
    assert!(has_color(pages.last().unwrap(), Color::rgb(0, 255, 0)));
}

#[test]
fn avoid_page_keeps_a_fitting_table_row_on_one_page() {
    let pages = print_support::render_pages(
        include_str!("fixtures/print/break-inside-table-row.html"),
        100.0,
        45.0,
    );
    print_support::save_pages(&pages, Some("break-inside-table-row"));
    assert_eq!(pages.len(), 2);
    assert!(has_color(&pages[0], Color::rgb(255, 0, 0)));
    assert!(!has_color(&pages[0], Color::rgb(0, 0, 255)));
    assert!(has_color(&pages[1], Color::rgb(0, 0, 255)));
    assert!(has_color(&pages[1], Color::rgb(0, 255, 0)));
}
