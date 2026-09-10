use crate::css::{Origin, StyleResolver, parse_stylesheet};
use crate::dom::NodeHandle;
use crate::font::{FontVariantKey, SystemFontDatabase, system_font_database};
use crate::layout::{InlineFragmentContent, LayoutBox, Rect, layout_tree};
use std::path::Path;

fn find_text(layout: &LayoutBox) -> &crate::layout::InlineFragment {
    for line in &layout.lines {
        for fragment in &line.fragments {
            if matches!(&fragment.content, InlineFragmentContent::Text(text) if text == "AB") {
                return fragment;
            }
        }
    }
    for child in &layout.children {
        if let Some(result) = find_text_optional(child) {
            return result;
        }
    }
    panic!("expected the AB text fragment");
}

fn find_text_optional(layout: &LayoutBox) -> Option<&crate::layout::InlineFragment> {
    layout.lines.iter().flat_map(|line| &line.fragments)
        .find(|fragment| matches!(&fragment.content, InlineFragmentContent::Text(text) if text == "AB"))
        .or_else(|| layout.children.iter().find_map(find_text_optional))
}

#[test]
fn css_system_faces_use_the_same_width_and_pixels_in_layout_and_paint() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/anonymized-font-selection");
    let database = SystemFontDatabase::from_directories(&[fixture]);
    crate::font::with_test_font_database(database, || {
        let fallback = system_font_database()
            .load("Omoikane Fixture", FontVariantKey::normal())
            .unwrap();
        let mut images = Vec::new();
        for (weight, style, expected_width) in [
            (400, "normal", 20.0),
            (700, "normal", 28.0),
            (400, "italic", 24.0),
            (700, "italic", 32.0),
        ] {
            let div = NodeHandle::element("div");
            div.append_child(NodeHandle::text("AB"));
            let mut resolver = StyleResolver::new();
            resolver.add_stylesheet(Origin::Author, parse_stylesheet(&format!(
                "div {{ margin: 0; padding: 0; font-family: 'Missing', 'Omoikane Fixture'; font-size: 20px; line-height: 24px; font-weight: {weight}; font-style: {style}; color: black; background: white; }}"
            )).unwrap());
            let viewport = Rect {
                x: 0.0,
                y: 0.0,
                width: 120.0,
                height: 40.0,
            };
            let fonts = vec![fallback.clone()];
            let (canvas, trace) = crate::font::with_font_selection_diagnostics(|| {
                crate::layout::with_layout_fonts(fonts.clone(), None, || {
                    let layout = layout_tree(&div, &mut resolver, viewport).unwrap();
                    let fragment = find_text(&layout);
                    assert_eq!(fragment.rect.width, expected_width);
                    super::paint_layout_with_fonts(&layout, &mut resolver, viewport, fonts)
                })
            });
            let matching: Vec<_> = trace
                .iter()
                .filter(|record| {
                    record
                        .requested_families
                        .iter()
                        .any(|f| f == "omoikane fixture")
                })
                .collect();
            assert!(matching.iter().any(|r| r.phase == "layout"));
            assert!(matching.iter().any(|r| r.phase == "paint"));
            let chosen = matching[0].selected.as_ref().unwrap();
            assert_eq!(chosen.weight, weight);
            assert!(matching.iter().all(|r| r.selected.as_ref() == Some(chosen)));
            assert!(canvas.pixels().chunks_exact(4).any(|p| p == [0, 0, 0, 255]));
            if let Some(output) = std::env::var_os("OMOIKANE_BROWSER_REPORT_DIR") {
                let dir = std::path::PathBuf::from(output);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(
                    dir.join(format!(
                        "anonymized-font-selection.{weight}-{style}.actual.png"
                    )),
                    canvas.encode_png(),
                )
                .unwrap();
            }
            images.push(canvas.pixels().to_vec());
        }
        for i in 0..images.len() {
            for j in 0..i {
                assert_ne!(
                    images[i], images[j],
                    "different faces must paint different outlines"
                );
            }
        }
    });
}
