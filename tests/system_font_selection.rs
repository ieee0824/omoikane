//! Installed-font diagnostics and generic-family contracts on each supported runner.

use omoikane::font::{FontCache, FontStyle, FontVariantKey, FontWeight, system_font_database};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[test]
fn generic_system_faces_honor_normal_bold_and_italic_requests() {
    let database = system_font_database();
    assert!(
        !database.faces().is_empty(),
        "installed font inventory must be available"
    );
    let mut files = BTreeMap::new();
    let mut cases = Vec::new();
    for family in ["sans-serif", "serif", "monospace"] {
        let mut cache = FontCache::new(8);
        for (weight, style) in [
            (400, FontStyle::Normal),
            (700, FontStyle::Normal),
            (400, FontStyle::Italic),
            (700, FontStyle::Italic),
        ] {
            let variant = FontVariantKey::new(FontWeight(weight), style);
            let font = cache.get_or_load_variant(family, variant).unwrap();
            let selected = font.system_face().expect("retain the actual file and face");
            assert_eq!(selected.weight, weight, "{family}: {selected:?}");
            assert_eq!(
                selected.style == FontStyle::Normal,
                style == FontStyle::Normal,
                "{family} should select the requested upright/slanted style: {selected:?}"
            );
            assert_eq!(font.face_index(), selected.face_index);
            let bytes = std::fs::read(&selected.path).unwrap();
            files.entry(selected.path.clone()).or_insert_with(
                || json!({"bytes": bytes.len(), "sha256": format!("{:x}", Sha256::digest(&bytes))}),
            );
            cases.push(json!({"family":family,"weight":weight,"style":style,"selected":selected}));
        }
    }
    if let Some(root) = std::env::var_os("OMOIKANE_BROWSER_REPORT_DIR") {
        let dir = std::path::PathBuf::from(root).join("fonts");
        std::fs::create_dir_all(&dir).unwrap();
        let report = json!({"status":"passed","cases":cases,"selected_files":files,
            "inventory":database.faces(),"unreadable":database.unreadable_paths()});
        std::fs::write(
            dir.join("system-inventory.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
    }
}
