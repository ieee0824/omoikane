use super::stylesheet::fetch_font_face_fonts;
use crate::css::parse_stylesheet;
use crate::font::{ShapingDirection, SystemFontDatabase, with_font_selection_diagnostics};
use crate::html::TreeBuilder;
use crate::layout::Rect;
use base64::Engine;

fn font_rule() -> String {
    let data = base64::engine::general_purpose::STANDARD.encode(include_bytes!(
        "../../tests/fixtures/anonymized-arabic-fallback/DejaVuSans.ttf"
    ));
    format!("@font-face{{font-family:AuditArabic;src:url(data:font/ttf;base64,{data})}}")
}

#[test]
fn embedded_font_loads_without_a_base_and_retries_after_an_invalid_source() {
    let valid = font_rule();
    let sheet = parse_stylesheet(&format!(
        "@font-face{{font-family:AuditArabic;src:url(data:font/ttf;base64,invalid)}}{valid}{valid}"
    ))
    .unwrap();
    let base: crate::http::Url = "http://localhost/fixture.html".parse().unwrap();
    for base in [None, Some(&base)] {
        let fonts = fetch_font_face_fonts(std::slice::from_ref(&sheet), base);
        assert_eq!(
            fonts.len(),
            1,
            "only successfully loaded variants are deduplicated"
        );
        let glyphs = fonts[0]
            .font
            .shape_text("العربية", 24.0, ShapingDirection::RightToLeft)
            .unwrap();
        assert_eq!(
            glyphs.iter().map(|g| g.glyph_id).collect::<Vec<_>>(),
            [5262, 5358, 5259, 5288, 5318, 5337, 1365]
        );
    }
}

#[test]
fn embedded_arabic_face_is_used_by_layout_and_paint_without_system_fonts() {
    crate::font::with_test_font_database(SystemFontDatabase::from_directories(&[]), || {
        let html = format!(
            "<!doctype html><html><head><style>{}html,body{{margin:0;background:white}}p{{margin:0;font:24px/40px AuditArabic}}</style></head><body><p>Latin العربية</p></body></html>",
            font_rule(),
        );
        let document = TreeBuilder::parse(&html).document();
        let (canvas, records) = with_font_selection_diagnostics(|| {
            super::render_document(
                &document,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 300.0,
                    height: 80.0,
                },
            )
            .unwrap()
        });
        for phase in ["layout", "paint"] {
            assert!(
                records.iter().any(|record| record.phase == phase
                    && record.source == "web"
                    && record
                        .requested_families
                        .iter()
                        .any(|family| family == "auditarabic")),
                "{phase} must use the embedded face: {records:?}"
            );
        }
        assert!(
            canvas
                .pixels()
                .chunks_exact(4)
                .any(|pixel| pixel[..3] != [255, 255, 255])
        );
    });
}
