//! Unmodified MQ4 reftests against their fixed WPT references.
use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Color, render_document};

#[test]
fn resolution_math_wpt_reference_matches_pixels() {
    compare_reference("css/mediaqueries/mq-calc-resolution.html");
}

#[test]
fn negative_range_minimum_wpt_reference_matches_pixels() {
    compare_reference("css/mediaqueries/mq-negative-range-001.html");
}

#[test]
fn negative_range_negation_wpt_reference_matches_pixels() {
    compare_reference("css/mediaqueries/mq-negative-range-002.html");
}

#[test]
fn malformed_range_can_be_unknown_in_a_matching_disjunction() {
    compare_reference("css/mediaqueries/mq-range-001.html");
}

#[test]
fn viewport_aspect_ratio_wpt_references_match_pixels() {
    for index in 1..=6 {
        compare_reference(&format!("css/mediaqueries/aspect-ratio-{index:03}.html"));
    }
}

#[test]
fn numeric_math_wpt_references_match_pixels() {
    for index in 1..=8 {
        compare_reference(&format!("css/mediaqueries/mq-calc-{index:03}.html"));
    }
}

fn compare_reference(test_path: &str) {
    let root =
        std::path::PathBuf::from(std::env::var("WPT_ROOT").unwrap_or_else(|_| "target/wpt".into()));
    let paths = [test_path, "css/reference/ref-filled-green-100px-square.xht"];
    if paths.iter().any(|path| !root.join(path).is_file()) {
        assert_ne!(
            std::env::var("WPT_REQUIRED").as_deref(),
            Ok("1"),
            "required resolution math WPT files missing: {}",
            root.display()
        );
        eprintln!("WPT checkout missing; resolution math reftest skipped");
        return;
    }
    let render = |path: &str| {
        let html = std::fs::read_to_string(root.join(path)).unwrap();
        let document = if path.ends_with(".xht") {
            omoikane::xml::parse(html.as_bytes()).unwrap()
        } else {
            TreeBuilder::parse(&html).document()
        };
        render_document(
            &document,
            Rect {
                width: 800.0,
                height: 600.0,
                ..Default::default()
            },
        )
        .unwrap()
    };
    let actual = render(paths[0]);
    let expected = render(paths[1]);
    assert_eq!(
        actual.pixel(10, 70),
        Some(Color::rgb(0, 128, 0)),
        "asserted square must be green"
    );
    let different = actual
        .pixels()
        .chunks_exact(4)
        .zip(expected.pixels().chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    eprintln!("{test_path}: {different} differing pixels");
    assert_eq!(different, 0);
}
