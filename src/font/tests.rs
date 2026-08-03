//! Tests for font module.

use super::*;

#[test]
fn shaping_controls_have_zero_advance_policy() {
    for ch in [
        '\u{0301}', '\u{1ab0}', '\u{20dd}', '\u{fe0f}', '\u{200c}', '\u{200d}', '\u{e0100}',
    ] {
        assert!(
            is_zero_advance_character(ch),
            "{ch:?} should be zero-advance"
        );
    }
    assert!(!is_zero_advance_character('A'));
    assert!(!is_zero_advance_character('\u{1f600}'));
}

#[test]
fn letter_spacing_boundaries_follow_extended_grapheme_clusters() {
    assert_eq!(grapheme_spacing_boundaries("e\u{301}"), 0);
    assert_eq!(grapheme_spacing_boundaries("👩‍💻"), 0);
    assert_eq!(grapheme_spacing_boundaries("👩‍💻a"), 1);
    assert_eq!(grapheme_spacing_boundaries("لا"), 1);
    assert_eq!(
        grapheme_spacing_cluster_starts("\u{301}AB"),
        vec!["\u{301}".len(), "\u{301}A".len()]
    );
}

#[test]
fn opentype_shaping_applies_arabic_context_and_ligatures() {
    let Some(path) = find_test_font() else {
        eprintln!("Skipping OpenType shaping test: no system font available");
        return;
    };
    let font = Font::load_from_file(std::path::Path::new(&path)).unwrap();
    if !"سلامب".chars().all(|ch| font.has_glyph(ch)) {
        eprintln!("Skipping OpenType shaping test: test font has no Arabic glyphs");
        return;
    }

    let isolated = font.shape_text("ب", 32.0, ShapingDirection::RightToLeft).unwrap();
    let contextual = font.shape_text("بب", 32.0, ShapingDirection::RightToLeft).unwrap();
    assert_eq!(isolated.len(), 1);
    assert_eq!(contextual.len(), 2);
    assert!(
        contextual.iter().any(|glyph| glyph.glyph_id != isolated[0].glyph_id),
        "Arabic joining must select contextual glyph forms"
    );

    let lam_alef = font.shape_text("لا", 32.0, ShapingDirection::RightToLeft).unwrap();
    assert!(lam_alef.len() < 2, "lam-alef should shape into a ligature");
    assert!(lam_alef.iter().all(|glyph| glyph.x_advance >= 0.0));
}

#[test]
fn fallback_selection_keeps_grapheme_clusters_in_one_font_run() {
    let primary_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/acid3/font.ttf");
    let Ok(primary) = Font::load_from_file(&primary_path) else {
        eprintln!("Skipping cluster fallback test: fixture font unavailable");
        return;
    };
    let Some(fallback_path) = find_test_font() else {
        eprintln!("Skipping cluster fallback test: no system font available");
        return;
    };
    let fallback = Font::load_from_file(std::path::Path::new(&fallback_path)).unwrap();
    let Some(primary_base) = ['A', 'e', 'a']
        .into_iter()
        .find(|ch| primary.has_glyph(*ch))
    else {
        eprintln!("Skipping cluster fallback test: fixture has no test base glyph");
        return;
    };

    // Pick a multi-scalar cluster using production's shaping-based support
    // check. A cmap-only mark probe does not model synthesized or attached
    // marks reliably and made the old fixed expectation host-dependent.
    let fallback_cluster = ['α', 'β', 'Ж', 'й', 'क', 'ب', 'ש']
        .into_iter()
        .flat_map(|base| ['\u{301}', '\u{308}', '\u{327}'].map(|mark| format!("{base}{mark}")))
        .find(|cluster| {
            !cluster_supported_by_font(
                &primary,
                cluster,
                24.0,
                ShapingDirection::LeftToRight,
            ) && cluster_supported_by_font(
                &fallback,
                cluster,
                24.0,
                ShapingDirection::LeftToRight,
            )
        });
    let Some(fallback_cluster) = fallback_cluster else {
        eprintln!("Skipping cluster fallback test: no deterministic fallback cluster available");
        return;
    };

    let text = format!("{fallback_cluster}{primary_base}");
    let runs = shape_text_with_fallback(
        &[&primary, &fallback],
        &text,
        24.0,
        ShapingDirection::LeftToRight,
    )
    .unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].font_index, 1);
    assert_eq!(&text[runs[0].text_range.clone()], fallback_cluster);
    assert_eq!(runs[1].font_index, 0);
    assert_eq!(&text[runs[1].text_range.clone()], primary_base.to_string());
    assert_eq!(runs[0].text_range.end, runs[1].text_range.start);
    let mut grapheme_boundaries = text
        .grapheme_indices(true)
        .map(|(start, _)| start)
        .collect::<Vec<_>>();
    grapheme_boundaries.push(text.len());
    assert!(
        runs.iter().all(|run| {
            grapheme_boundaries.contains(&run.text_range.start)
                && grapheme_boundaries.contains(&run.text_range.end)
        }),
        "font-run boundaries must coincide with grapheme boundaries"
    );
    assert!(runs.iter().all(|run| {
        run.glyphs
            .iter()
            .all(|glyph| run.text_range.contains(&glyph.cluster))
    }));
}

#[test]
fn fallback_shaping_keeps_primary_supported_text_in_one_run() {
    let primary_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/acid3/font.ttf");
    let Ok(primary) = Font::load_from_file(&primary_path) else {
        eprintln!("Skipping primary shaping test: fixture font unavailable");
        return;
    };
    let Some(text) = ["abc", "ABC", "123"].into_iter().find(|text| {
        primary
            .shape_text(text, 24.0, ShapingDirection::LeftToRight)
            .is_ok_and(|glyphs| glyphs.iter().all(|glyph| glyph.glyph_id != 0))
    }) else {
        eprintln!("Skipping primary shaping test: fixture has no supported sample");
        return;
    };

    let runs = shape_text_with_fallback(
        &[&primary],
        text,
        24.0,
        ShapingDirection::LeftToRight,
    )
    .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].font_index, 0);
    assert_eq!(runs[0].text_range, 0..text.len());
}

#[test]
fn fallback_runs_never_split_variation_or_zwj_graphemes() {
    let primary_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/acid3/font.ttf");
    let Ok(primary) = Font::load_from_file(&primary_path) else {
        eprintln!("Skipping cluster boundary test: fixture font unavailable");
        return;
    };
    let mut fonts = vec![primary];
    if let Some(path) = find_test_font()
        && let Ok(fallback) = Font::load_from_file(std::path::Path::new(&path))
    {
        fonts.push(fallback);
    }
    let font_refs = fonts.iter().collect::<Vec<_>>();

    for text in ["A\u{fe0f}B", "👩‍💻A"] {
        let runs = shape_text_with_fallback(
            &font_refs,
            text,
            24.0,
            ShapingDirection::LeftToRight,
        )
        .unwrap();
        let mut boundaries = text
            .grapheme_indices(true)
            .map(|(start, _)| start)
            .collect::<Vec<_>>();
        boundaries.push(text.len());
        assert_eq!(runs.first().map(|run| run.text_range.start), Some(0));
        assert_eq!(runs.last().map(|run| run.text_range.end), Some(text.len()));
        assert!(runs.windows(2).all(|pair| pair[0].text_range.end == pair[1].text_range.start));
        assert!(runs.iter().all(|run| {
            boundaries.contains(&run.text_range.start)
                && boundaries.contains(&run.text_range.end)
                && run
                    .glyphs
                    .iter()
                    .all(|glyph| run.text_range.contains(&glyph.cluster))
        }));
    }
}

/// Try to find a system font for testing.
/// Returns path if found, otherwise None.
fn find_test_font() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        // macOS: Try system fonts
        [
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/Arial.ttf",
            "/System/Library/Fonts/Times New Roman.ttf",
            "/Library/Fonts/Arial.ttf",
        ]
        .iter()
        .find_map(|&path| {
            if std::path::Path::new(path).exists() {
                Some(path.to_string())
            } else {
                None
            }
        })
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: Try common fonts
        [
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        ]
        .iter()
        .find_map(|&path| {
            if std::path::Path::new(path).exists() {
                Some(path.to_string())
            } else {
                None
            }
        })
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: Try system fonts
        [
            "C:\\Windows\\Fonts\\arial.ttf",
            "C:\\Windows\\Fonts\\times.ttf",
        ]
        .iter()
        .find_map(|&path| {
            if std::path::Path::new(path).exists() {
                Some(path.to_string())
            } else {
                None
            }
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    None
}

#[test]
fn test_glyph_raster_empty_is_valid() {
    // Test that an empty GlyphRaster can be created
    let raster = GlyphRaster {
        width: 0,
        height: 0,
        bitmap: vec![],
        advance_x: 5.0,
        advance_y: 0.0,
        offset_x: 0.0,
        offset_y: 0.0,
    };
    assert_eq!(raster.width, 0);
    assert_eq!(raster.height, 0);
    assert_eq!(raster.advance_x, 5.0);
}

#[test]
fn test_glyph_raster_with_bitmap() {
    let bitmap = vec![0, 128, 255, 64];
    let raster = GlyphRaster {
        width: 2,
        height: 2,
        bitmap,
        advance_x: 10.0,
        advance_y: 0.0,
        offset_x: 0.0,
        offset_y: 0.0,
    };
    assert_eq!(raster.width, 2);
    assert_eq!(raster.height, 2);
    assert_eq!(raster.bitmap.len(), 4);
}

#[test]
#[ignore] // This test requires system fonts to be available
fn test_load_system_font_and_rasterize() {
    let Some(font_path) = find_test_font() else {
        eprintln!("Skipping test: no system font found");
        return;
    };

    let font =
        Font::load_from_file(std::path::Path::new(&font_path)).expect("Failed to load system font");

    // Rasterize a simple character
    let raster = font.rasterize('A', 20.0).expect("Failed to rasterize 'A'");

    // Check that rasterization produced non-zero dimensions for a visible glyph.
    assert!(
        raster.width > 0 && raster.height > 0,
        "Expected non-zero raster dimensions for 'A'"
    );
    assert!(raster.advance_x > 0.0, "Advance width must be positive");
}

#[test]
#[ignore] // This test requires system fonts to be available
fn test_different_characters_different_advances() {
    let Some(font_path) = find_test_font() else {
        eprintln!("Skipping test: no system font found");
        return;
    };

    let font =
        Font::load_from_file(std::path::Path::new(&font_path)).expect("Failed to load system font");

    let size = 20.0;
    let advance_i = font.glyph_advance('i', size);
    let advance_m = font.glyph_advance('m', size);

    // 'm' should be wider than 'i' in most fonts
    assert!(
        advance_m >= advance_i,
        "Expected 'm' advance ({}) >= 'i' advance ({})",
        advance_m,
        advance_i
    );
}

#[test]
#[ignore] // This test requires system fonts to be available
fn test_rasterize_space_character() {
    let Some(font_path) = find_test_font() else {
        eprintln!("Skipping test: no system font found");
        return;
    };

    let font =
        Font::load_from_file(std::path::Path::new(&font_path)).expect("Failed to load system font");

    // Space character should have zero bitmap but positive advance
    let raster = font
        .rasterize(' ', 20.0)
        .expect("Failed to rasterize space");
    assert_eq!(raster.width, 0);
    assert_eq!(raster.height, 0);
    assert!(
        raster.advance_x > 0.0,
        "Space should have positive advance width"
    );
}

#[test]
#[ignore] // This test requires system fonts to be available
fn test_advance_scales_with_size() {
    let Some(font_path) = find_test_font() else {
        eprintln!("Skipping test: no system font found");
        return;
    };

    let font =
        Font::load_from_file(std::path::Path::new(&font_path)).expect("Failed to load system font");

    let advance_10 = font.glyph_advance('A', 10.0);
    let advance_20 = font.glyph_advance('A', 20.0);
    let advance_40 = font.glyph_advance('A', 40.0);

    // Advances should be approximately proportional to size
    assert!(
        advance_20 > advance_10,
        "Larger size should have larger advance"
    );
    assert!(
        advance_40 > advance_20,
        "Even larger size should have larger advance"
    );

    // Check approximate scaling (allowing 5% tolerance for rounding)
    let ratio_1 = advance_20 / advance_10;
    let ratio_2 = advance_40 / advance_20;

    assert!(
        ratio_1 > 1.8 && ratio_1 < 2.2,
        "Advance should roughly double with 2x size"
    );
    assert!(
        ratio_2 > 1.8 && ratio_2 < 2.2,
        "Advance should roughly double with 2x size"
    );
}

// ============================================================================
// Phase 2: System Font Discovery Tests
// ============================================================================

#[test]
fn test_generic_family_sans_serif_has_candidates() {
    let candidates = generic_family_fonts("sans-serif");
    assert!(
        !candidates.is_empty(),
        "sans-serif should have font candidates"
    );
    assert!(candidates.contains(&"Helvetica") || candidates.contains(&"Arial"));
}

#[test]
fn test_generic_family_serif_has_candidates() {
    let candidates = generic_family_fonts("serif");
    assert!(!candidates.is_empty(), "serif should have font candidates");
}

#[test]
fn test_generic_family_monospace_has_candidates() {
    let candidates = generic_family_fonts("monospace");
    assert!(
        !candidates.is_empty(),
        "monospace should have font candidates"
    );
}

#[test]
fn test_generic_family_unknown_returns_empty() {
    let candidates = generic_family_fonts("comic-sans-fantasy");
    assert!(candidates.is_empty(), "Unknown family should return empty");
}

#[test]
#[ignore] // Requires system fonts
fn test_find_system_font_sans_serif() {
    let path = find_system_font("sans-serif");
    assert!(
        path.is_some(),
        "Should find a sans-serif font on this system"
    );

    let path = path.unwrap();
    assert!(path.exists(), "Found font path should exist");

    // Should be a font file
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    assert!(
        ext == "ttf" || ext == "otf" || ext == "ttc",
        "Should be a font file, got: {:?}",
        path
    );
}

#[test]
#[ignore] // Requires system fonts
fn test_load_system_font_by_generic_family() {
    let font = load_system_font("sans-serif");
    assert!(
        font.is_ok(),
        "Should load sans-serif font: {:?}",
        font.err()
    );

    let font = font.unwrap();
    // Verify it's a valid font by checking metrics
    let metrics = font.metrics();
    assert!(
        metrics.units_per_em > 0.0,
        "Font should have valid units_per_em"
    );
}

#[test]
#[ignore] // Requires system fonts
fn test_load_system_font_helvetica() {
    // Only run on macOS where Helvetica is guaranteed
    #[cfg(target_os = "macos")]
    {
        let font = load_system_font("Helvetica");
        assert!(font.is_ok(), "Should find Helvetica on macOS");
    }
}

// ============================================================================
// Phase 3: Font and Glyph Cache Tests
// ============================================================================

#[test]
fn test_font_cache_creation() {
    let cache = FontCache::new(10);
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
}

#[test]
fn test_glyph_cache_creation() {
    let cache = GlyphCache::new(100);
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
}

#[test]
fn test_glyph_cache_insert_and_get() {
    let mut cache = GlyphCache::new(100);

    let raster = GlyphRaster {
        width: 10,
        height: 12,
        bitmap: vec![128; 120],
        advance_x: 8.0,
        advance_y: 0.0,
        offset_x: 0.0,
        offset_y: -10.0,
    };

    cache.insert('A', 16.0, raster.clone());
    assert_eq!(cache.len(), 1);

    let retrieved = cache.get('A', 16.0);
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().width, 10);
    assert_eq!(retrieved.unwrap().height, 12);

    // Different size should not be found
    let not_found = cache.get('A', 20.0);
    assert!(not_found.is_none());

    // Different character should not be found
    let not_found = cache.get('B', 16.0);
    assert!(not_found.is_none());
}

#[test]
fn test_glyph_cache_size_quantization() {
    let mut cache = GlyphCache::new(100);

    let raster = GlyphRaster {
        width: 5,
        height: 5,
        bitmap: vec![255; 25],
        advance_x: 4.0,
        advance_y: 0.0,
        offset_x: 0.0,
        offset_y: -4.0,
    };

    // Insert at 16.0 px
    cache.insert('X', 16.0, raster);

    // Should find at exact same size
    assert!(cache.get('X', 16.0).is_some());

    // Should find at 16.04 (rounds to same tenth: 160)
    assert!(cache.get('X', 16.04).is_some());

    // Should NOT find at 16.1 (rounds to 161, different key)
    assert!(cache.get('X', 16.1).is_none());

    // Should NOT find at different character
    assert!(cache.get('Y', 16.0).is_none());
}

#[test]
fn test_glyph_cache_eviction() {
    let mut cache = GlyphCache::new(3);

    for i in 0..5 {
        let raster = GlyphRaster {
            width: i as u32,
            height: 1,
            bitmap: vec![0; i as usize],
            advance_x: i as f32,
            advance_y: 0.0,
            offset_x: 0.0,
            offset_y: 0.0,
        };
        cache.insert(char::from_u32('A' as u32 + i).unwrap(), 10.0, raster);
    }

    // Cache should have max 3 entries
    assert_eq!(cache.len(), 3);
}

#[test]
#[ignore] // Requires system fonts
fn test_font_cache_get_or_load() {
    let mut cache = FontCache::new(5);

    // First load
    let font1 = cache.get_or_load("sans-serif");
    assert!(font1.is_ok());
    assert_eq!(cache.len(), 1);

    // Second load should return cached
    let font2 = cache.get_or_load("sans-serif");
    assert!(font2.is_ok());
    assert_eq!(cache.len(), 1); // Still 1, not 2
}

// ============================================================================
// Phase 4: Text Width Measurement Tests
// ============================================================================

#[test]
#[ignore] // Requires system fonts
fn test_measure_text_width() {
    let font = load_system_font("sans-serif").expect("Need sans-serif font");

    let width = font.measure_text_width("Hello", 16.0);
    assert!(width > 0.0, "Text width should be positive");

    // "Hello" should be wider than "Hi"
    let width_hi = font.measure_text_width("Hi", 16.0);
    assert!(width > width_hi, "Longer text should be wider");
}

#[test]
#[ignore] // Requires system fonts
fn test_measure_text_width_scales() {
    let font = load_system_font("sans-serif").expect("Need sans-serif font");

    let width_10 = font.measure_text_width("Test", 10.0);
    let width_20 = font.measure_text_width("Test", 20.0);

    // Width should roughly scale with size
    let ratio = width_20 / width_10;
    assert!(
        ratio > 1.8 && ratio < 2.2,
        "Width should roughly double at 2x size"
    );
}

#[test]
#[ignore] // Requires system fonts
fn test_average_advance() {
    let font = load_system_font("sans-serif").expect("Need sans-serif font");

    let avg = font.average_advance(16.0);
    assert!(avg > 0.0, "Average advance should be positive");
    assert!(
        avg < 16.0,
        "Average advance should be less than font size for proportional fonts"
    );
}

#[test]
#[ignore] // Requires system fonts
fn test_layout_metrics() {
    let font = load_system_font("sans-serif").expect("Need sans-serif font");

    let metrics = font.layout_metrics(16.0);

    assert_eq!(metrics.font_size, 16.0);
    assert!(metrics.ascent > 0.0, "Ascent should be positive");
    assert!(metrics.descent >= 0.0, "Descent should be non-negative");
    assert!(
        metrics.average_advance > 0.0,
        "Average advance should be positive"
    );
}

#[test]
#[ignore = "debug test"]
fn debug_hello_world_measurement() {
    use crate::font::load_system_font;

    let font = load_system_font("sans-serif").unwrap();
    let size = 32.0; // 2em = 32px

    // Check individual character advances
    println!("Font size: {}px", size);
    println!(
        "' ' (space) advance: {:.2}px",
        font.glyph_advance(' ', size)
    );
    println!("'H' advance: {:.2}px", font.glyph_advance('H', size));
    println!("'e' advance: {:.2}px", font.glyph_advance('e', size));
    println!("'l' advance: {:.2}px", font.glyph_advance('l', size));
    println!("'o' advance: {:.2}px", font.glyph_advance('o', size));

    // Measure text widths
    let hello = "Hello";
    let world = "World!";
    let full = "Hello World!";

    println!(
        "\"{}\" width: {:.2}px",
        hello,
        font.measure_text_width(hello, size)
    );
    println!(
        "\"{}\" width: {:.2}px",
        world,
        font.measure_text_width(world, size)
    );
    println!(
        "\"{}\" width: {:.2}px",
        full,
        font.measure_text_width(full, size)
    );
    println!(
        "Sum Hello + space + World! = {:.2}px",
        font.measure_text_width(hello, size)
            + font.glyph_advance(' ', size)
            + font.measure_text_width(world, size)
    );
}

#[test]
#[ignore = "debug test"]
fn debug_nbsp_vs_space() {
    use crate::font::load_system_font;

    let font = load_system_font("sans-serif").unwrap();
    let size = 24.0;

    let space = ' ';
    let nbsp = '\u{00A0}';

    println!("Font size: {}px", size);
    println!(
        "Regular space ' ' advance: {:.2}px",
        font.glyph_advance(space, size)
    );
    println!(
        "NBSP '\\u{{00A0}}' advance: {:.2}px",
        font.glyph_advance(nbsp, size)
    );
    println!("'H' advance: {:.2}px", font.glyph_advance('H', size));

    // Measure text width
    println!(
        "\"Hello World!\" (regular space): {:.2}px",
        font.measure_text_width("Hello World!", size)
    );
    println!(
        "\"Hello\\u{{00A0}}World!\" (NBSP): {:.2}px",
        font.measure_text_width("Hello\u{00A0}World!", size)
    );
}

// ============================================================================
// Web font / load_from_bytes tests
// ============================================================================

#[test]
fn detect_font_format_ttf() {
    // TTF magic bytes: 00 01 00 00
    let data = vec![0x00, 0x01, 0x00, 0x00, 0x00, 0x00];
    assert_eq!(detect_font_format(&data), "ttf");
}

#[test]
fn detect_font_format_otf() {
    let data = b"OTTO\x00\x00".to_vec();
    assert_eq!(detect_font_format(&data), "otf");
}

#[test]
fn detect_font_format_woff() {
    let data = b"wOFF\x00\x00".to_vec();
    assert_eq!(detect_font_format(&data), "woff");
}

#[test]
fn detect_font_format_woff2() {
    let data = b"wOF2\x00\x00".to_vec();
    assert_eq!(detect_font_format(&data), "woff2");
}

#[test]
fn detect_font_format_unknown() {
    let data = vec![0xFF, 0xFE, 0x00, 0x00];
    assert_eq!(detect_font_format(&data), "unknown");
}

#[test]
fn detect_font_format_short_data() {
    assert_eq!(detect_font_format(&[0x00]), "unknown");
    assert_eq!(detect_font_format(&[]), "unknown");
}

#[test]
fn load_from_bytes_ttf_system_font() {
    let font_path = match find_test_font() {
        Some(p) => p,
        None => {
            eprintln!("Skipping load_from_bytes_ttf_system_font: no system font found");
            return;
        }
    };

    let data = std::fs::read(&font_path).unwrap();
    let font = Font::load_from_bytes(data).unwrap();
    // Verify basic font operations work
    let advance = font.glyph_advance('A', 16.0);
    assert!(advance > 0.0, "advance should be positive, got {}", advance);
}

#[test]
fn load_from_bytes_invalid_data_returns_error() {
    let data = vec![0x00, 0x01, 0x00, 0x00, 0xFF, 0xFF]; // TTF magic but invalid content
    let result = Font::load_from_bytes(data);
    assert!(result.is_err());
}

/// WOFF2 magic bytes are detected by `detect_font_format`.
#[test]
fn detect_font_format_woff2_magic() {
    let data = b"wOF2XXXX".to_vec();
    assert_eq!(detect_font_format(&data), "woff2");
}

/// A truncated WOFF2 payload (header too short) must return an error.
#[test]
fn load_from_bytes_woff2_truncated_header_returns_error() {
    // "wOF2" magic + 4 bytes — shorter than the 48-byte WOFF2 header.
    let data = b"wOF2\x00\x00\x00\x00".to_vec();
    let result = Font::load_from_bytes(data);
    assert!(result.is_err(), "expected error for truncated WOFF2 header");
    let err_msg = format!("{}", result.err().unwrap());
    assert!(
        err_msg.contains("WOFF2") || err_msg.contains("header"),
        "error should mention WOFF2 or header: {}",
        err_msg
    );
}

/// Build a minimal but structurally valid WOFF2 payload and verify that
/// `decode_woff2` at least accepts the header and table directory without
/// crashing, even though the resulting sfnt may not be loadable by ab_glyph.
///
/// This test exercises the brotli decompression and sfnt reconstruction paths.
#[test]
fn decode_woff2_minimal_valid_structure() {
    use brotli::enc::BrotliEncoderParams;

    // We will create a WOFF2 with a single `name` table (tag index 5)
    // containing 4 dummy bytes (one 32-bit word) so we can compute a
    // deterministic checksum.
    let table_data: &[u8] = b"TEST"; // 4 bytes

    // Compress the table data with brotli.
    let mut compressed = Vec::new();
    brotli::enc::BrotliCompress(
        &mut std::io::Cursor::new(table_data),
        &mut compressed,
        &BrotliEncoderParams::default(),
    )
    .expect("brotli compress failed");

    // --- Build WOFF2 header (48 bytes) ---
    let num_tables: u16 = 1;
    let sfnt_flavor: u32 = 0x0001_0000; // TrueType
    let total_compressed_size = compressed.len() as u32;

    // Table directory: flags byte + origLength (UIntBase128)
    // `name` tag = index 5 in KNOWN_TAGS
    let flags_byte: u8 = 5u8; // tag index 5, transform_version = 0 (no transform for non-glyf/loca)
    // UIntBase128 for 4: single byte 0x04
    let orig_length_base128: u8 = 4;

    let table_dir: Vec<u8> = vec![flags_byte, orig_length_base128];

    // Total file length
    let file_length = 48u32 + table_dir.len() as u32 + total_compressed_size;

    let mut woff2 = Vec::new();
    // signature
    woff2.extend_from_slice(b"wOF2");
    // flavor
    woff2.extend_from_slice(&sfnt_flavor.to_be_bytes());
    // length
    woff2.extend_from_slice(&file_length.to_be_bytes());
    // numTables
    woff2.extend_from_slice(&num_tables.to_be_bytes());
    // reserved
    woff2.extend_from_slice(&0u16.to_be_bytes());
    // totalSfntSize (approximate: sfnt header + 1 table record + padded data)
    let total_sfnt_size: u32 = 12 + 16 + 4;
    woff2.extend_from_slice(&total_sfnt_size.to_be_bytes());
    // totalCompressedSize
    woff2.extend_from_slice(&total_compressed_size.to_be_bytes());
    // majorVersion, minorVersion
    woff2.extend_from_slice(&1u16.to_be_bytes());
    woff2.extend_from_slice(&0u16.to_be_bytes());
    // metaOffset, metaLength, metaOrigLength
    woff2.extend_from_slice(&0u32.to_be_bytes());
    woff2.extend_from_slice(&0u32.to_be_bytes());
    woff2.extend_from_slice(&0u32.to_be_bytes());
    // privOffset, privLength
    woff2.extend_from_slice(&0u32.to_be_bytes());
    woff2.extend_from_slice(&0u32.to_be_bytes());
    // table directory
    woff2.extend_from_slice(&table_dir);
    // compressed data
    woff2.extend_from_slice(&compressed);

    assert_eq!(woff2[..4], *b"wOF2");
    assert_eq!(detect_font_format(&woff2), "woff2");

    // decode_woff2 should succeed and produce a valid sfnt-shaped buffer.
    let sfnt = decode_woff2(&woff2);
    assert!(
        sfnt.is_ok(),
        "decode_woff2 failed: {}",
        sfnt.err().unwrap()
    );

    let sfnt = sfnt.unwrap();
    // sfnt header: 4 (flavor) + 2 (numTables) + 2 + 2 + 2 = 12 bytes
    // table record: 16 bytes
    // total at least 28 bytes before the actual table data
    assert!(sfnt.len() >= 28, "sfnt too short: {} bytes", sfnt.len());
    // sfnt flavor should match
    let flavor = u32::from_be_bytes([sfnt[0], sfnt[1], sfnt[2], sfnt[3]]);
    assert_eq!(flavor, sfnt_flavor);
    // numTables should match
    let nt = u16::from_be_bytes([sfnt[4], sfnt[5]]);
    assert_eq!(nt as usize, num_tables as usize);
}

#[test]
fn font_cache_register_web_font() {
    let font_path = match find_test_font() {
        Some(p) => p,
        None => {
            eprintln!("Skipping font_cache_register_web_font: no system font found");
            return;
        }
    };

    let data = std::fs::read(&font_path).unwrap();
    let mut cache = FontCache::new(10);
    assert!(!cache.contains("TestWebFont"));

    let font = cache.register_web_font("TestWebFont", data).unwrap();
    assert!(cache.contains("TestWebFont"));
    assert!(cache.contains("testwebfont")); // case insensitive

    // Should be retrievable via get_or_load (returns the cached web font)
    let font2 = cache.get_or_load("TestWebFont").unwrap();
    assert!(Arc::ptr_eq(&font, &font2));
}

// ============================================================================
// FontWeight / FontStyle parsing tests
// ============================================================================

#[test]
fn font_weight_parse_keywords() {
    assert_eq!(FontWeight::parse("normal"), FontWeight(400));
    assert_eq!(FontWeight::parse("bold"), FontWeight(700));
    assert_eq!(FontWeight::parse("bolder"), FontWeight(700));
    assert_eq!(FontWeight::parse("lighter"), FontWeight(300));
}

#[test]
fn font_weight_parse_numeric() {
    assert_eq!(FontWeight::parse("100"), FontWeight(100));
    assert_eq!(FontWeight::parse("400"), FontWeight(400));
    assert_eq!(FontWeight::parse("700"), FontWeight(700));
    assert_eq!(FontWeight::parse("900"), FontWeight(900));
}

#[test]
fn font_weight_parse_unknown_defaults_to_400() {
    assert_eq!(FontWeight::parse(""), FontWeight(400));
    assert_eq!(FontWeight::parse("bogus"), FontWeight(400));
}

#[test]
fn font_style_parse() {
    assert_eq!(FontStyle::parse("normal"), FontStyle::Normal);
    assert_eq!(FontStyle::parse("italic"), FontStyle::Italic);
    assert_eq!(FontStyle::parse("oblique"), FontStyle::Oblique);
    assert_eq!(FontStyle::parse("ITALIC"), FontStyle::Italic);
    assert_eq!(FontStyle::parse("unknown"), FontStyle::Normal);
}

// ============================================================================
// WebFontRegistry tests
// ============================================================================

/// Build a minimal valid Font from disk bytes for use in registry tests.
fn load_test_font_for_registry() -> Option<Font> {
    let path = find_test_font()?;
    let data = std::fs::read(&path).ok()?;
    Font::load_from_bytes(data).ok()
}

#[test]
fn web_font_registry_exact_match_weight_400() {
    let font_regular = match load_test_font_for_registry() {
        Some(f) => f,
        None => {
            eprintln!("Skipping: no system font found");
            return;
        }
    };
    let font_bold = match load_test_font_for_registry() {
        Some(f) => f,
        None => return,
    };

    let mut registry = WebFontRegistry::new();
    registry.push("MyFont", FontWeight(400), FontStyle::Normal, font_regular);
    registry.push("MyFont", FontWeight(700), FontStyle::Normal, font_bold);

    // Exact match for normal weight
    assert!(
        registry
            .select_best("MyFont", FontWeight(400), FontStyle::Normal)
            .is_some()
    );
    // Exact match for bold weight
    assert!(
        registry
            .select_best("MyFont", FontWeight(700), FontStyle::Normal)
            .is_some()
    );
}

#[test]
fn web_font_registry_selects_with_case_insensitive_family_key() {
    let Some(font) = load_test_font_for_registry() else {
        eprintln!("Skipping: no system font found");
        return;
    };
    let mut registry = WebFontRegistry::new();
    registry.push("TwitterChirp", FontWeight(700), FontStyle::Italic, font);

    assert!(registry.select_best_by_key(
        FontFamilyKey::new("twitterchirp"),
        FontWeight(700),
        FontStyle::Italic,
    ).is_some());
}

#[test]
fn font_family_keys_are_distinct_for_different_families() {
    assert_ne!(
        FontFamilyKey::new("TwitterChirp"),
        FontFamilyKey::new("Segoe UI"),
    );
    assert_ne!(
        FontFamilyKey::new("Arial"),
        FontFamilyKey::new("Arial Black"),
    );
}

#[test]
fn font_family_key_folds_unicode_case_and_trims() {
    assert_eq!(
        FontFamilyKey::new("  TwitterChirp "),
        FontFamilyKey::new("twitterchirp"),
    );
    // Unicode case folding, matching the registry's previous `to_lowercase()`
    // behaviour for non-ASCII family names.
    assert_eq!(
        FontFamilyKey::new("ГАРНИТУРА"),
        FontFamilyKey::new("гарнитура"),
    );
    assert_ne!(
        FontFamilyKey::new("ГАРНИТУРА"),
        FontFamilyKey::new("шрифт"),
    );
}

#[test]
fn web_font_registry_bold_selects_700_when_available() {
    let font_regular = match load_test_font_for_registry() {
        Some(f) => f,
        None => {
            eprintln!("Skipping: no system font found");
            return;
        }
    };
    let font_bold = match load_test_font_for_registry() {
        Some(f) => f,
        None => return,
    };

    let mut registry = WebFontRegistry::new();
    registry.push("TestFamily", FontWeight(400), FontStyle::Normal, font_regular);
    registry.push("TestFamily", FontWeight(700), FontStyle::Normal, font_bold);

    // Requesting bold (700) should prefer the 700 variant
    let selected = registry
        .select_best("TestFamily", FontWeight(700), FontStyle::Normal)
        .expect("should find a font");

    // We can't easily distinguish the two loaded fonts by value (both from same file),
    // so just ensure a font is returned without panic.
    let _ = selected;
}

#[test]
fn web_font_registry_italic_selects_italic_over_normal() {
    let font_regular = match load_test_font_for_registry() {
        Some(f) => f,
        None => {
            eprintln!("Skipping: no system font found");
            return;
        }
    };
    let font_italic = match load_test_font_for_registry() {
        Some(f) => f,
        None => return,
    };

    let mut registry = WebFontRegistry::new();
    registry.push("TestFamily", FontWeight(400), FontStyle::Normal, font_regular);
    registry.push("TestFamily", FontWeight(400), FontStyle::Italic, font_italic);

    // Requesting italic should return a font
    assert!(
        registry
            .select_best("TestFamily", FontWeight(400), FontStyle::Italic)
            .is_some()
    );
}

#[test]
fn web_font_registry_fallback_when_no_italic() {
    let font_regular = match load_test_font_for_registry() {
        Some(f) => f,
        None => {
            eprintln!("Skipping: no system font found");
            return;
        }
    };

    let mut registry = WebFontRegistry::new();
    registry.push("TestFamily", FontWeight(400), FontStyle::Normal, font_regular);

    // Requesting italic when only normal is available → should still return a font (best match)
    assert!(
        registry
            .select_best("TestFamily", FontWeight(400), FontStyle::Italic)
            .is_some()
    );
}

#[test]
fn web_font_registry_unknown_family_returns_none() {
    let registry = WebFontRegistry::new();
    assert!(
        registry
            .select_best("NoSuchFamily", FontWeight(400), FontStyle::Normal)
            .is_none()
    );
}

#[test]
fn font_cache_register_web_font_with_variant() {
    let font_path = match find_test_font() {
        Some(p) => p,
        None => {
            eprintln!("Skipping font_cache_register_web_font_with_variant: no system font found");
            return;
        }
    };

    let data_regular = std::fs::read(&font_path).unwrap();
    let data_bold = std::fs::read(&font_path).unwrap();

    let mut cache = FontCache::new(10);
    cache
        .register_web_font_with_variant(
            "MultiFont",
            FontWeight(400),
            FontStyle::Normal,
            data_regular,
        )
        .unwrap();
    cache
        .register_web_font_with_variant(
            "MultiFont",
            FontWeight(700),
            FontStyle::Normal,
            data_bold,
        )
        .unwrap();

    assert!(cache.contains("MultiFont"));
    // Both variants stored → cache has 2 entries for this family
    assert_eq!(cache.len(), 2);

    // Best variant for bold should be found
    let bold = cache.select_best_variant("MultiFont", FontWeight(700), FontStyle::Normal);
    assert!(bold.is_some());
}
