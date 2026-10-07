//! Static individual-transform WPT reftests against their unmodified references.
use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Color, render_document};

#[test]
fn individual_transform_wpt_references_match_pixels() {
    let root =
        std::path::PathBuf::from(std::env::var("WPT_ROOT").unwrap_or_else(|_| "target/wpt".into()));
    let directory = root.join("css/css-transforms/individual-transform");
    if !directory.exists() {
        assert_ne!(
            std::env::var("WPT_REQUIRED").as_deref(),
            Ok("1"),
            "WPT checkout required: {}",
            directory.display()
        );
        eprintln!("WPT checkout missing; individual-transform reftests skipped");
        return;
    }
    for (name, reference) in [
        ("stacking-context-001.html", "stacking-context-ref.html"),
        (
            "individual-transform-1.html",
            "individual-transform-1-ref.html",
        ),
        (
            "individual-transform-2a.html",
            "individual-transform-2-ref.html",
        ),
        (
            "individual-transform-2b.html",
            "individual-transform-2-ref.html",
        ),
        (
            "individual-transform-2c.html",
            "individual-transform-2-ref.html",
        ),
        (
            "individual-transform-2d.html",
            "individual-transform-2-ref.html",
        ),
        (
            "individual-transform-2e.html",
            "individual-transform-2-ref.html",
        ),
        (
            "individual-transform-3.html",
            "../../reference/ref-filled-green-200px-square.html",
        ),
        (
            "stacking-context-002.html",
            "../../reference/ref-filled-green-100px-square.xht",
        ),
        (
            "stacking-context-003.html",
            "../../reference/ref-filled-green-100px-square.xht",
        ),
        (
            "stacking-context-004.html",
            "../../reference/ref-filled-green-100px-square.xht",
        ),
    ] {
        let render = |name: &str| {
            let path = directory.join(name);
            let html = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let document = if path.extension().is_some_and(|extension| extension == "xht") {
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
        let actual = render(name);
        let expected = render(reference);
        let different = actual
            .pixels()
            .chunks_exact(4)
            .zip(expected.pixels().chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        eprintln!("{name}: {different} differing pixels");
        assert_eq!(different, 0, "{name}");
        // Also check the asserted square's color: a shared missing stacking
        // context in the test and reference must not produce a false pass.
        if name.starts_with("stacking-context-") {
            let color = if name == "stacking-context-001.html" {
                Color::rgb(0, 0, 255)
            } else {
                Color::rgb(0, 128, 0)
            };
            assert_eq!(actual.pixel(10, 70), Some(color), "{name}: asserted square");
        }
    }
}
