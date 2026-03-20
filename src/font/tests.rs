//! Tests for font module.

use super::*;

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

    let font = Font::load_from_file(std::path::Path::new(&font_path))
        .expect("Failed to load system font");

    // Rasterize a simple character
    let raster = font.rasterize('A', 20.0).expect("Failed to rasterize 'A'");

    // Check that rasterization produced non-zero dimensions
    assert!(raster.width > 0 || raster.height == 0, "Width must be > 0 or height must be 0");
    assert!(raster.advance_x > 0.0, "Advance width must be positive");
}

#[test]
#[ignore] // This test requires system fonts to be available
fn test_different_characters_different_advances() {
    let Some(font_path) = find_test_font() else {
        eprintln!("Skipping test: no system font found");
        return;
    };

    let font = Font::load_from_file(std::path::Path::new(&font_path))
        .expect("Failed to load system font");

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

    let font = Font::load_from_file(std::path::Path::new(&font_path))
        .expect("Failed to load system font");

    // Space character should have zero bitmap but positive advance
    let raster = font.rasterize(' ', 20.0).expect("Failed to rasterize space");
    assert_eq!(raster.width, 0);
    assert_eq!(raster.height, 0);
    assert!(raster.advance_x > 0.0, "Space should have positive advance width");
}

#[test]
#[ignore] // This test requires system fonts to be available
fn test_advance_scales_with_size() {
    let Some(font_path) = find_test_font() else {
        eprintln!("Skipping test: no system font found");
        return;
    };

    let font = Font::load_from_file(std::path::Path::new(&font_path))
        .expect("Failed to load system font");

    let advance_10 = font.glyph_advance('A', 10.0);
    let advance_20 = font.glyph_advance('A', 20.0);
    let advance_40 = font.glyph_advance('A', 40.0);

    // Advances should be approximately proportional to size
    assert!(advance_20 > advance_10, "Larger size should have larger advance");
    assert!(advance_40 > advance_20, "Even larger size should have larger advance");

    // Check approximate scaling (allowing 5% tolerance for rounding)
    let ratio_1 = advance_20 / advance_10;
    let ratio_2 = advance_40 / advance_20;

    assert!(ratio_1 > 1.8 && ratio_1 < 2.2, "Advance should roughly double with 2x size");
    assert!(ratio_2 > 1.8 && ratio_2 < 2.2, "Advance should roughly double with 2x size");
}

// ============================================================================
// Phase 2: System Font Discovery Tests
// ============================================================================

#[test]
fn test_generic_family_sans_serif_has_candidates() {
    let candidates = generic_family_fonts("sans-serif");
    assert!(!candidates.is_empty(), "sans-serif should have font candidates");
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
    assert!(!candidates.is_empty(), "monospace should have font candidates");
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
    assert!(path.is_some(), "Should find a sans-serif font on this system");

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
    assert!(font.is_ok(), "Should load sans-serif font: {:?}", font.err());

    let font = font.unwrap();
    // Verify it's a valid font by checking metrics
    let metrics = font.metrics();
    assert!(metrics.units_per_em > 0.0, "Font should have valid units_per_em");
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
    assert!(ratio > 1.8 && ratio < 2.2, "Width should roughly double at 2x size");
}

#[test]
#[ignore] // Requires system fonts
fn test_average_advance() {
    let font = load_system_font("sans-serif").expect("Need sans-serif font");

    let avg = font.average_advance(16.0);
    assert!(avg > 0.0, "Average advance should be positive");
    assert!(avg < 16.0, "Average advance should be less than font size for proportional fonts");
}

#[test]
#[ignore] // Requires system fonts
fn test_layout_metrics() {
    let font = load_system_font("sans-serif").expect("Need sans-serif font");

    let metrics = font.layout_metrics(16.0);

    assert_eq!(metrics.font_size, 16.0);
    assert!(metrics.ascent > 0.0, "Ascent should be positive");
    assert!(metrics.descent >= 0.0, "Descent should be non-negative");
    assert!(metrics.average_advance > 0.0, "Average advance should be positive");
}

    #[test]
    #[ignore = "debug test"]
    fn debug_hello_world_measurement() {
        use crate::font::load_system_font;
        
        let font = load_system_font("sans-serif").unwrap();
        let size = 32.0; // 2em = 32px
        
        // Check individual character advances
        println!("Font size: {}px", size);
        println!("' ' (space) advance: {:.2}px", font.glyph_advance(' ', size));
        println!("'H' advance: {:.2}px", font.glyph_advance('H', size));
        println!("'e' advance: {:.2}px", font.glyph_advance('e', size));
        println!("'l' advance: {:.2}px", font.glyph_advance('l', size));
        println!("'o' advance: {:.2}px", font.glyph_advance('o', size));
        
        // Measure text widths
        let hello = "Hello";
        let world = "World!";
        let full = "Hello World!";
        
        println!("\"{}\" width: {:.2}px", hello, font.measure_text_width(hello, size));
        println!("\"{}\" width: {:.2}px", world, font.measure_text_width(world, size));
        println!("\"{}\" width: {:.2}px", full, font.measure_text_width(full, size));
        println!("Sum Hello + space + World! = {:.2}px", 
            font.measure_text_width(hello, size) + 
            font.glyph_advance(' ', size) + 
            font.measure_text_width(world, size));
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
        println!("Regular space ' ' advance: {:.2}px", font.glyph_advance(space, size));
        println!("NBSP '\\u{{00A0}}' advance: {:.2}px", font.glyph_advance(nbsp, size));
        println!("'H' advance: {:.2}px", font.glyph_advance('H', size));
        
        // Measure text width
        println!("\"Hello World!\" (regular space): {:.2}px", font.measure_text_width("Hello World!", size));
        println!("\"Hello\\u{{00A0}}World!\" (NBSP): {:.2}px", font.measure_text_width("Hello\u{00A0}World!", size));
    }
