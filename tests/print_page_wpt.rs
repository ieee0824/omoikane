use std::path::PathBuf;

use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::render_document_pages;

#[test]
fn css_page_layer_reftests_match_at_fixed_revision() {
    let root = std::env::var_os("WPT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/wpt"));
    let root = root.join("css/css-page");
    if !root.exists() && std::env::var_os("WPT_REQUIRED").is_none() {
        return;
    }
    assert!(
        root.exists(),
        "css-page WPT checkout missing: {}",
        root.display()
    );
    let sheet = Rect {
        width: 800.0,
        height: 1000.0,
        ..Rect::default()
    };
    for number in 1..=4 {
        let stem = format!("layers-{number:03}-print");
        let test = std::fs::read_to_string(root.join(format!("{stem}.html"))).unwrap();
        let reference = std::fs::read_to_string(root.join(format!("{stem}-ref.html"))).unwrap();
        let test_document = TreeBuilder::parse(&test).document();
        let reference_document = TreeBuilder::parse(&reference).document();
        let actual = render_document_pages(&test_document, sheet).unwrap();
        let expected = render_document_pages(&reference_document, sheet).unwrap();
        assert_eq!(actual.len(), expected.len(), "{stem} page count");
        for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
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
}
