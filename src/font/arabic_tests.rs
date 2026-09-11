use super::*;

fn database() -> SystemFontDatabase {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    SystemFontDatabase::from_directories(&[
        root.join("acid2"),
        root.join("anonymized-arabic-fallback"),
        root.join("anonymized-font-selection"),
    ])
}

#[test]
fn default_fonts_shape_arabic_in_a_contextual_fallback_run() {
    if let Some(path) = std::env::var_os("OMOIKANE_ARABIC_FONT_REPORT") {
        // Optional host evidence is separate from the fixed database assertions
        // below. It calls the actual fallback shaper with the installed defaults.
        let fonts = load_default_text_fonts_shared();
        let refs: Vec<_> = fonts.iter().map(Arc::as_ref).collect();
        let mut records = Vec::new();
        for (text, direction) in [
            ("العربية", ShapingDirection::RightToLeft),
            ("Latin", ShapingDirection::LeftToRight),
            ("日本語", ShapingDirection::LeftToRight),
        ] {
            let runs = shape_text_with_fallback(&refs, text, 24.0, direction).unwrap();
            for run in runs {
                records.push(serde_json::json!({
                    "text": text,
                    "range": [run.text_range.start, run.text_range.end],
                    "face": fonts[run.font_index].system_face(),
                    "glyph_ids": run.glyphs.iter().map(|g| g.glyph_id).collect::<Vec<_>>(),
                    "clusters": run.glyphs.iter().map(|g| g.cluster).collect::<Vec<_>>(),
                }));
            }
        }
        std::fs::write(path, serde_json::to_vec_pretty(&records).unwrap()).unwrap();
    }
    system::with_test_database(database(), || {
        let fonts = load_default_text_fonts_shared();
        assert_eq!(
            fonts[0].system_face().unwrap().full_name.as_deref(),
            Some("Liberation Sans")
        );
        assert!(!fonts[0].has_glyph('ع'));
        let refs: Vec<_> = fonts.iter().map(Arc::as_ref).collect();
        let runs = shape_text_with_fallback(&refs, "العربية", 24.0, ShapingDirection::RightToLeft)
            .unwrap();
        assert_eq!(
            runs.len(),
            1,
            "Arabic joining must survive fallback selection"
        );
        let run = &runs[0];
        let face = fonts[run.font_index].system_face().unwrap();
        assert_eq!(face.full_name.as_deref(), Some("DejaVu Sans"));
        assert_eq!(run.text_range, 0.."العربية".len());
        assert_eq!(
            run.glyphs.iter().map(|g| g.glyph_id).collect::<Vec<_>>(),
            [5262, 5358, 5259, 5288, 5318, 5337, 1365]
        );
        assert!(
            run.glyphs
                .windows(2)
                .all(|pair| pair[0].cluster > pair[1].cluster),
            "RTL glyphs retain logical cluster offsets"
        );
        for glyph in &run.glyphs {
            let image = fonts[run.font_index]
                .rasterize_glyph(glyph.glyph_id, 24.0)
                .unwrap();
            assert!(image.bitmap.iter().any(|&value| value > 0));
        }
    });
}

#[test]
fn latin_arabic_and_cjk_keep_distinct_fonts_and_complete_clusters() {
    system::with_test_database(database(), || {
        let mut fonts = load_default_text_fonts_shared();
        fonts.push(
            system_font_database()
                .load("Omoikane Fallback", FontVariantKey::normal())
                .unwrap(),
        );
        let refs: Vec<_> = fonts.iter().map(Arc::as_ref).collect();
        let text = "AB العربية 中";
        let runs =
            shape_text_with_fallback(&refs, text, 24.0, ShapingDirection::LeftToRight).unwrap();
        assert!(runs.iter().flat_map(|r| &r.glyphs).all(|g| g.glyph_id != 0));
        let covered: String = runs.iter().map(|r| &text[r.text_range.clone()]).collect();
        assert_eq!(covered, text);
        for (piece, family) in [
            ("AB", "Liberation Sans"),
            ("العربية", "DejaVu Sans"),
            ("中", "Omoikane Fallback"),
        ] {
            let run = runs
                .iter()
                .find(|r| text[r.text_range.clone()].contains(piece))
                .unwrap();
            assert!(
                fonts[run.font_index]
                    .system_face()
                    .unwrap()
                    .families
                    .iter()
                    .any(|name| name == family)
            );
        }
        // Direct RTL shaping independently specifies the contextual glyphs; the
        // paint bidi pass splits this mixed paragraph into directional runs.
        let rtl = shape_text_with_fallback(&refs, "العربية", 24.0, ShapingDirection::RightToLeft)
            .unwrap();
        assert_eq!(
            rtl[0].glyphs.iter().map(|g| g.glyph_id).collect::<Vec<_>>(),
            [5262, 5358, 5259, 5288, 5318, 5337, 1365]
        );
    });
}
