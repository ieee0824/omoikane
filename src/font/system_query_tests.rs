use super::*;
use crate::font::{FontStretch, FontWeight};

fn database() -> SystemFontDatabase {
    SystemFontDatabase::from_directories(&[
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/anonymized-font-selection")
    ])
}

fn member(name: &str) -> FontFamilyMember {
    FontFamilyMember {
        name: name.to_owned(),
        key: OnceLock::new(),
    }
}

#[test]
fn cached_queries_preserve_variants_aliases_and_missing_families() {
    let db = database();
    for name in [
        "Omoikane Fixture",
        " 'OMOIKANE Fixture' ",
        "Omoikane Café",
        "serif",
        "sans-serif",
        "Missing",
    ] {
        let family = member(name);
        for weight in [100, 400, 450, 500, 700, 900] {
            for style in [FontStyle::Normal, FontStyle::Italic, FontStyle::Oblique] {
                for stretch in [FontStretch::NORMAL, FontStretch(75_000)] {
                    let variant = FontVariantKey::with_stretch(FontWeight(weight), style, stretch);
                    let expected = db.select_index(name, variant);
                    for _ in 0..2 {
                        assert_eq!(db.select_member_index(&family, variant), expected);
                        assert_eq!(
                            db.load_member(&family, variant)
                                .map(|f| f.system_face.clone()),
                            db.load(name, variant).ok().map(|f| f.system_face.clone())
                        );
                    }
                }
            }
        }
    }
    assert!(db.selections.lock().unwrap().len() <= SELECTION_CACHE_CAPACITY);
}

#[test]
fn cached_missing_family_is_scoped_to_database_snapshot() {
    let empty = SystemFontDatabase::from_directories(&[]);
    let populated = database();
    let family = member("Omoikane Fixture");
    let variant = FontVariantKey::normal();
    assert_eq!(empty.select_member_index(&family, variant), None);
    assert!(populated.select_member_index(&family, variant).is_some());
    assert_eq!(empty.select_member_index(&family, variant), None);
}

#[test]
fn cached_face_index_does_not_cache_file_load_failures() {
    let directory = QueryTestDirectory::new();
    let file = directory.path().join("Regular.ttf");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/anonymized-font-selection/OmoikaneFixture-Regular.ttf");
    std::fs::copy(&source, &file).unwrap();
    let db = SystemFontDatabase::from_directories(&[directory.path().to_owned()]);
    let family = member("Omoikane Fixture");
    let variant = FontVariantKey::normal();
    std::fs::remove_file(&file).unwrap();
    assert!(db.load_member(&family, variant).is_none());
    std::fs::copy(source, &file).unwrap();
    let loaded = db
        .load_member(&family, variant)
        .expect("retry restored file");
    assert_eq!(loaded.glyph_advance('A', 20.0), 10.0);
    assert!(Arc::ptr_eq(
        &loaded,
        &db.load_member(&family, variant).unwrap()
    ));
}

#[test]
fn evicted_queries_recompute_the_same_selection() {
    let db = database();
    let variant = FontVariantKey::normal();
    let known = member("Omoikane Fixture");
    let expected = db.select_member_index(&known, variant);
    for i in 0..SELECTION_CACHE_CAPACITY * 2 {
        assert_eq!(
            db.select_member_index(&member(&format!("Missing {i}")), variant),
            None
        );
    }
    assert_eq!(
        db.selections.lock().unwrap().len(),
        SELECTION_CACHE_CAPACITY
    );
    assert_eq!(db.select_member_index(&known, variant), expected);
    assert_eq!(
        db.selections.lock().unwrap().len(),
        SELECTION_CACHE_CAPACITY
    );
}

#[test]
fn late_web_font_overrides_warm_system_query() {
    with_test_database(database(), || {
        let variant = FontVariantKey::normal();
        let fallback = system_font_database()
            .load("Omoikane Fixture", variant)
            .unwrap();
        let fallbacks = [fallback];
        let family = Some(FontFamilyKey::new("Omoikane Fixture"));
        let mut registry = super::super::WebFontRegistry::new();
        let initial =
            select_text_font_impl(family, None, variant, Some(&registry), &fallbacks).unwrap();
        assert_eq!(initial.as_ref().glyph_advance('A', 20.0), 10.0);
        registry.push(
            "Omoikane Fixture",
            FontWeight(400),
            FontStyle::Normal,
            Font::load_from_file(
                &Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/anonymized-font-selection/OmoikaneFixture-Bold.ttf"),
            )
            .unwrap(),
        );
        let selected =
            select_text_font_impl(family, None, variant, Some(&registry), &fallbacks).unwrap();
        assert!(matches!(selected, SelectedFont::Web(_)));
        assert_eq!(selected.as_ref().glyph_advance('A', 20.0), 14.0);
    });
}

struct QueryTestDirectory(PathBuf);

impl QueryTestDirectory {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("omoikane-font-query-{}-{id}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for QueryTestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cached_generic_family_preserves_alias_priority() {
    let mut db = database();
    let normal = FontVariantKey::normal();
    let regular = db.select_index("Omoikane Fixture", normal).unwrap();
    let bold = db
        .select_index(
            "Omoikane Fixture",
            FontVariantKey::new(FontWeight(700), FontStyle::Normal),
        )
        .unwrap();
    // Build a deterministic metadata snapshot with two supported generic aliases.
    db.families.insert("helvetica".into(), vec![regular]);
    db.families.insert("arial".into(), vec![bold]);
    let family = member("sans-serif");
    for _ in 0..2 {
        assert_eq!(db.select_member_index(&family, normal), Some(regular));
        assert_eq!(
            db.load_member(&family, normal)
                .unwrap()
                .glyph_advance('A', 20.0),
            10.0
        );
    }
}
