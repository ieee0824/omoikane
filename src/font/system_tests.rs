use super::*;
use std::sync::Arc;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/anonymized-font-selection")
}

fn database() -> SystemFontDatabase {
    SystemFontDatabase::from_directories(&[fixture_dir()])
}

#[test]
fn installed_faces_match_width_weight_and_style_from_metadata() {
    let db = database();
    assert!(db.unreadable_paths().is_empty());
    for (weight, style, expected, advance) in [
        (400, FontStyle::Normal, "Regular", 10.0),
        (700, FontStyle::Normal, "Bold", 14.0),
        (400, FontStyle::Italic, "Italic", 12.0),
        (700, FontStyle::Italic, "BoldItalic", 16.0),
        (400, FontStyle::Oblique, "Italic", 12.0),
    ] {
        let variant = FontVariantKey::new(FontWeight(weight), style);
        let face = db.select("oMOIKANE fixture", variant).unwrap();
        assert!(
            face.path
                .ends_with(format!("OmoikaneFixture-{expected}.ttf"))
        );
        assert_eq!(face.width, 5, "normal stretch excludes the condensed face");
        let font = db.load("Omoikane Fixture", variant).unwrap();
        assert_eq!(font.system_face(), Some(face));
        assert_eq!(font.glyph_advance('A', 20.0), advance);
        assert_eq!(
            font.shape_text("AB", 20.0, ShapingDirection::LeftToRight)
                .unwrap()
                .iter()
                .map(|g| g.x_advance)
                .sum::<f32>(),
            2.0 * advance
        );
        assert_eq!(font.rasterize('A', 20.0).unwrap().advance_x, advance);
    }
    assert!(
        db.select("Omoikane", FontVariantKey::normal()).is_none(),
        "no substring family matches"
    );
}

#[test]
fn system_family_selection_decodes_mac_roman_collection_names() {
    let bytes = std::fs::read(fixture_dir().join("OmoikaneMacRoman.ttc")).unwrap();
    let face = rustybuzz::ttf_parser::Face::parse(&bytes, 0).unwrap();
    assert!(face.names().into_iter().all(|name| !name.is_unicode()));
    let db = database();
    for (index, weight, style, name, advance) in [
        (0, 400, FontStyle::Normal, "Regular", 10.0),
        (1, 700, FontStyle::Normal, "Bold", 14.0),
        (2, 400, FontStyle::Italic, "Italic", 12.0),
    ] {
        let variant = FontVariantKey::new(FontWeight(weight), style);
        let selected = db
            .select("Omoikane Café", variant)
            .expect("Mac Roman family");
        assert_eq!(selected.face_index, index);
        assert_eq!(
            selected.full_name.as_deref(),
            Some(format!("Omoikane Café {name}").as_str())
        );
        assert_eq!((selected.weight, selected.style), (weight, style));
        let loaded = db.load("Omoikane Café", variant).unwrap();
        assert_eq!(loaded.face_index(), index);
        assert_eq!(loaded.rasterize('A', 20.0).unwrap().advance_x, advance);
    }
}

#[test]
fn collection_selection_keeps_its_face_through_shaping_and_rasterization() {
    let bytes = std::fs::read(fixture_dir().join("OmoikaneFixture.ttc")).unwrap();
    for (index, expected, standalone) in
        [(0, 14.0, "Bold"), (1, 10.0, "Regular"), (2, 12.0, "Italic")]
    {
        let font = Font::load_from_bytes_at_index(bytes.clone(), index).unwrap();
        let standalone =
            Font::load_from_file(&fixture_dir().join(format!("OmoikaneFixture-{standalone}.ttf")))
                .unwrap();
        assert_eq!(font.face_index(), index);
        assert_eq!(font.glyph_advance('A', 20.0), expected);
        assert_eq!(
            font.shape_text("AB", 20.0, ShapingDirection::LeftToRight)
                .unwrap()
                .iter()
                .map(|g| g.x_advance)
                .sum::<f32>(),
            2.0 * expected
        );
        assert_eq!(font.rasterize('A', 20.0).unwrap().advance_x, expected);
        let actual = font.rasterize('A', 20.0).unwrap();
        let reference = standalone.rasterize('A', 20.0).unwrap();
        assert_eq!(actual.bitmap, reference.bitmap);
        assert_eq!(
            (actual.width, actual.height),
            (reference.width, reference.height)
        );
        assert_eq!(
            font.shape_text("AB", 20.0, ShapingDirection::LeftToRight)
                .unwrap(),
            standalone
                .shape_text("AB", 20.0, ShapingDirection::LeftToRight)
                .unwrap()
        );
    }
    assert!(Font::load_from_bytes_at_index(bytes, 3).is_err());
}

#[test]
fn system_cache_does_not_substitute_an_already_loaded_style() {
    system::with_test_database(database(), || {
        let mut cache = FontCache::new(8);
        let regular = cache.get_or_load("Omoikane Fixture").unwrap();
        let bold = cache
            .get_or_load_variant(
                "Omoikane Fixture",
                FontVariantKey::new(FontWeight(700), FontStyle::Normal),
            )
            .unwrap();
        let italic = cache
            .get_or_load_variant(
                "Omoikane Fixture",
                FontVariantKey::new(FontWeight(400), FontStyle::Italic),
            )
            .unwrap();
        assert!(!Arc::ptr_eq(&regular, &bold));
        assert!(!Arc::ptr_eq(&regular, &italic));
        assert_eq!(regular.glyph_advance('A', 20.0), 10.0);
        assert_eq!(bold.glyph_advance('A', 20.0), 14.0);
        assert_eq!(italic.glyph_advance('A', 20.0), 12.0);
        assert!(Arc::ptr_eq(
            &regular,
            &cache.get_or_load("Omoikane Fixture").unwrap()
        ));
        assert_eq!(cache.len(), 3);
    });
}

#[test]
fn registered_web_family_overrides_previously_cached_system_faces() {
    system::with_test_database(database(), || {
        let mut cache = FontCache::new(8);
        let system = cache.get_or_load("Omoikane Fixture").unwrap();
        let data = std::fs::read(fixture_dir().join("OmoikaneFixture-Bold.ttf")).unwrap();
        let web = cache
            .register_web_font_with_variant(
                "Omoikane Fixture",
                FontWeight(700),
                FontStyle::Normal,
                data,
            )
            .unwrap();
        let selected = cache.get_or_load("Omoikane Fixture").unwrap();
        assert!(Arc::ptr_eq(&web, &selected));
        assert!(!Arc::ptr_eq(&system, &selected));
        assert_eq!(selected.glyph_advance('A', 20.0), 14.0);
        assert!(selected.system_face().is_none());
        cache.clear();
        assert_eq!(
            cache
                .get_or_load("Omoikane Fixture")
                .unwrap()
                .glyph_advance('A', 20.0),
            10.0
        );
    });
}

#[test]
fn css_weight_search_preserves_400_500_and_directional_fallback_rules() {
    for (target, expected) in [
        (400, [400, 500, 300, 100, 600, 700, 900]),
        (450, [500, 400, 300, 100, 600, 700, 900]),
        (500, [500, 400, 300, 100, 600, 700, 900]),
        (200, [100, 300, 400, 500, 600, 700, 900]),
        (650, [700, 900, 600, 500, 400, 300, 100]),
    ] {
        let mut candidates = [100, 300, 400, 500, 600, 700, 900];
        candidates.sort_by_key(|weight| system::weight_rank(target, *weight));
        assert_eq!(candidates, expected, "requested weight {target}");
    }
}

#[test]
fn family_lists_preserve_order_and_quoted_commas_for_web_and_system_fonts() {
    system::with_test_database(database(), || {
        let fallback = system_font_database()
            .load("Omoikane Fixture", FontVariantKey::normal())
            .unwrap();
        let fallbacks = [fallback];
        let variant = FontVariantKey::new(FontWeight(700), FontStyle::Normal);
        let family = FontFamilyKey::new("'Missing Font', 'Omoikane Fixture'");
        let selected = select_text_font("layout", Some(family), variant, None, &fallbacks).unwrap();
        assert_eq!(selected.as_ref().glyph_advance('A', 20.0), 14.0);
        let mut registry = WebFontRegistry::new();
        registry.push(
            "Comma, Family",
            FontWeight(700),
            FontStyle::Normal,
            Font::load_from_bytes(
                std::fs::read(fixture_dir().join("OmoikaneFixture-Italic.ttf")).unwrap(),
            )
            .unwrap(),
        );
        let family = FontFamilyKey::new("'Comma, Family', 'Omoikane Fixture'");
        let selected =
            select_text_font("layout", Some(family), variant, Some(&registry), &fallbacks).unwrap();
        assert!(selected.as_ref().system_face().is_none());
        assert_eq!(selected.as_ref().glyph_advance('A', 20.0), 12.0);
    });
}

#[test]
fn selected_system_faces_keep_cjk_and_combining_clusters_in_fallback_runs() {
    let db = database();
    let primary = db
        .load("Omoikane Fixture", FontVariantKey::normal())
        .unwrap();
    let fallback = db
        .load("Omoikane Fallback", FontVariantKey::normal())
        .unwrap();
    let text = "A\u{301}中B";
    let runs = shape_text_with_fallback(
        &[primary.as_ref(), fallback.as_ref()],
        text,
        20.0,
        ShapingDirection::LeftToRight,
    )
    .unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].font_index, 1);
    assert_eq!(&text[runs[0].text_range.clone()], "A\u{301}中");
    assert_eq!(runs[1].font_index, 0);
    assert_eq!(&text[runs[1].text_range.clone()], "B");
    assert!(runs.iter().flat_map(|r| &r.glyphs).all(|g| g.glyph_id != 0));
    assert_eq!(
        runs.iter()
            .flat_map(|r| &r.glyphs)
            .map(|g| g.x_advance)
            .sum::<f32>(),
        46.0
    );
}

#[test]
fn font_diagnostic_scopes_restore_after_nesting_and_panics() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let capture = || {
        select_text_font("layout", None, FontVariantKey::normal(), None, &[]);
    };
    let (_, outer) = with_font_selection_diagnostics(|| {
        capture();
        let (_, inner) = with_font_selection_diagnostics(capture);
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0].uses, 1);
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                with_font_selection_diagnostics(|| {
                    capture();
                    panic!("diagnostic scope test");
                });
            }))
            .is_err()
        );
        capture();
    });
    assert_eq!(outer.len(), 1);
    assert_eq!(outer[0].uses, 2);
    assert_eq!(outer[0].source, "none");
}
