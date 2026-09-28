use std::path::{Path, PathBuf};

use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Canvas, render_document_pages};

fn css_page_wpt_root() -> Option<PathBuf> {
    let root = std::env::var_os("WPT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/wpt"))
        .join("css/css-page");
    if !root.exists() && std::env::var_os("WPT_REQUIRED").is_none() {
        return None;
    }
    assert!(
        root.exists(),
        "css-page WPT checkout missing: {}",
        root.display()
    );
    Some(root)
}

fn render_wpt_page(root: &Path, name: &str, sheet: Rect) -> Vec<Canvas> {
    let source = std::fs::read_to_string(root.join(name)).expect("read pinned WPT case");
    render_document_pages(&TreeBuilder::parse(&source).document(), sheet)
        .expect("render pinned WPT case")
}

fn assert_matching_pages(stem: &str, actual: &[Canvas], expected: &[Canvas]) {
    assert_eq!(actual.len(), expected.len(), "{stem} page count");
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(
            actual.width(),
            expected.width(),
            "{stem} page {index} width"
        );
        assert_eq!(
            actual.height(),
            expected.height(),
            "{stem} page {index} height"
        );
        let mismatches = actual
            .pixels()
            .chunks_exact(4)
            .zip(expected.pixels().chunks_exact(4))
            .filter(|(actual, expected)| actual != expected)
            .count();
        assert_eq!(mismatches, 0, "{stem} page {index} pixel mismatch");
    }
}

#[test]
fn css_page_layer_reftests_match_at_fixed_revision() {
    let Some(root) = css_page_wpt_root() else {
        return;
    };
    let sheet = Rect {
        width: 800.0,
        height: 1000.0,
        ..Rect::default()
    };
    for number in 1..=4 {
        let stem = format!("layers-{number:03}-print");
        let actual = render_wpt_page(&root, &format!("{stem}.html"), sheet);
        let expected = render_wpt_page(&root, &format!("{stem}-ref.html"), sheet);
        assert_matching_pages(&stem, &actual, &expected);
    }
}

#[test]
fn named_page_changes_and_explicit_breaks_do_not_insert_extra_pages() {
    let Some(root) = css_page_wpt_root() else {
        return;
    };
    let sheet = Rect {
        width: 800.0,
        height: 1000.0,
        ..Rect::default()
    };
    let expected = render_wpt_page(&root, "page-name-and-break-print-ref.html", sheet);
    assert_eq!(expected.len(), 2, "reference page count");
    for number in 1..=4 {
        let stem = format!("page-name-and-break-{number:03}-print");
        let actual = render_wpt_page(&root, &format!("{stem}.html"), sheet);
        assert_matching_pages(&stem, &actual, &expected);
    }
}
