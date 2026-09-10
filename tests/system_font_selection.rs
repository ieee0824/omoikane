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
    let mut checks = Vec::new();
    write_report(&json!({"status":"inventory_collected", "cases":cases,
        "inventory":database.faces(), "unreadable":database.unreadable_paths()}));
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
            let bytes = std::fs::read(&selected.path).unwrap();
            files.entry(selected.path.clone()).or_insert_with(
                || json!({"bytes": bytes.len(), "sha256": format!("{:x}", Sha256::digest(&bytes))}),
            );
            let face = rustybuzz::ttf_parser::Face::parse(&bytes, selected.face_index).unwrap();
            let names: Vec<_> = face
                .names()
                .into_iter()
                .filter(|name| matches!(name.name_id, 1 | 2 | 4 | 16 | 17 | 21 | 22))
                .map(|name| {
                    json!({"id":name.name_id,
                    "platform":format!("{:?}", name.platform_id),
                    "encoding":name.encoding_id, "language":name.language_id,
                    "unicode_name":name.to_string()})
                })
                .collect();
            let mac_style = face
                .raw_face()
                .table(rustybuzz::ttf_parser::Tag::from_bytes(b"head"))
                .and_then(|table| table.get(44..46))
                .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]));
            cases.push(
                json!({"family":family,"weight":weight,"style":style,"selected":selected,
                "source_metadata":{"name_records":names, "has_os2":face.tables().os2.is_some(),
                    "head_mac_style":mac_style}}),
            );
            checks.push((family, weight, style, font));
        }
    }
    let mut report = json!({"status":"collected", "cases":cases, "selected_files":files,
        "inventory":database.faces(), "unreadable":database.unreadable_paths()});
    // Preserve the actual choices even if an assertion below fails on a runner.
    write_report(&report);
    for (family, weight, style, font) in checks {
        let selected = font.system_face().unwrap();
        assert_eq!(selected.weight, weight, "{family}: {selected:?}");
        assert_eq!(
            selected.style == FontStyle::Normal,
            style == FontStyle::Normal,
            "{family} should select the requested upright/slanted style: {selected:?}"
        );
        assert_eq!(font.face_index(), selected.face_index);
    }
    report["status"] = json!("passed");
    write_report(&report);
}

fn write_report(report: &serde_json::Value) {
    if let Some(root) = std::env::var_os("OMOIKANE_BROWSER_REPORT_DIR") {
        let dir = std::path::PathBuf::from(root).join("fonts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("system-inventory.json"),
            serde_json::to_vec_pretty(report).unwrap(),
        )
        .unwrap();
    }
}
