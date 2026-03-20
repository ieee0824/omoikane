//! Font loading and glyph rendering module.
//!
//! Provides TrueType/OpenType font support using the `ab_glyph` crate.
//! Handles font file loading, character-to-glyph mapping, and rasterization.

use ab_glyph::{Font as AbGlyphFont, FontVec, ScaleFont};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fmt, io};

#[cfg(test)]
mod tests;

/// Error type for font operations.
#[derive(Debug, Clone)]
pub enum FontError {
    /// Font file not found or couldn't be read.
    IoError(String),
    /// Invalid or unsupported font format.
    InvalidFont(String),
    /// Other font operation error.
    Other(String),
}

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FontError::IoError(msg) => write!(f, "IO error: {}", msg),
            FontError::InvalidFont(msg) => write!(f, "Invalid font: {}", msg),
            FontError::Other(msg) => write!(f, "Font error: {}", msg),
        }
    }
}

impl From<io::Error> for FontError {
    fn from(err: io::Error) -> Self {
        FontError::IoError(err.to_string())
    }
}

/// Rasterized glyph bitmap and metrics.
#[derive(Debug, Clone)]
pub struct GlyphRaster {
    /// Bitmap width in pixels.
    pub width: u32,
    /// Bitmap height in pixels.
    pub height: u32,
    /// Alpha channel values (one u8 per pixel, row-major order).
    pub bitmap: Vec<u8>,
    /// Horizontal advance width in pixels.
    pub advance_x: f32,
    /// Vertical advance width (usually 0 for horizontal text).
    pub advance_y: f32,
}

/// Font representation wrapping `ab_glyph::FontVec`.
pub struct Font {
    inner: FontVec,
}

impl Font {
    /// Load a font from a file path.
    pub fn load_from_file(path: &Path) -> Result<Self, FontError> {
        let data = std::fs::read(path)?;
        let font_vec = FontVec::try_from_vec_and_index(data, 0)
            .map_err(|_| FontError::InvalidFont("Failed to parse font data".to_string()))?;
        Ok(Font { inner: font_vec })
    }

    /// Rasterize a character at a given font size.
    pub fn rasterize(&self, ch: char, size_px: f32) -> Result<GlyphRaster, FontError> {
        // Get glyph ID for character
        let glyph_id = self.inner.glyph_id(ch);

        // Get scaled advance width for layout
        let advance_x = {
            let scaled = self.inner.as_scaled(size_px);
            scaled.h_advance(glyph_id)
        };

        // Create a glyph with scale at position (0,0)
        let glyph = glyph_id.with_scale(size_px);

        // Get the outlined glyph (None for space characters and glyphs with no outline)
        let Some(outlined) = self.inner.outline_glyph(glyph) else {
            return Ok(GlyphRaster {
                width: 0,
                height: 0,
                bitmap: vec![],
                advance_x,
                advance_y: 0.0,
            });
        };

        // Get pixel bounds
        let bounds = outlined.px_bounds();
        let width = bounds.width().ceil() as u32;
        let height = bounds.height().ceil() as u32;

        // Zero-size glyphs return empty bitmap
        if width == 0 || height == 0 {
            return Ok(GlyphRaster {
                width: 0,
                height: 0,
                bitmap: vec![],
                advance_x,
                advance_y: 0.0,
            });
        }

        // Create bitmap buffer
        let mut bitmap = vec![0u8; (width * height) as usize];

        // Rasterize glyph by calling draw callback
        outlined.draw(|x, y, coverage| {
            if let Some(pixel) = bitmap.get_mut((y * width + x) as usize) {
                *pixel = (coverage * 255.0).clamp(0.0, 255.0) as u8;
            }
        });

        Ok(GlyphRaster {
            width,
            height,
            bitmap,
            advance_x,
            advance_y: 0.0,
        })
    }

    /// Get the horizontal advance width for a character at a given font size.
    pub fn glyph_advance(&self, ch: char, size_px: f32) -> f32 {
        let glyph_id = self.inner.glyph_id(ch);
        let scaled = self.inner.as_scaled(size_px);
        scaled.h_advance(glyph_id)
    }

    /// Get font metrics from the font tables.
    pub fn metrics(&self) -> FontMetricsTable {
        let upm = self.inner.units_per_em().unwrap_or(1000.0);
        FontMetricsTable {
            units_per_em: upm,
            ascender: self.inner.ascent_unscaled(),
            descender: self.inner.descent_unscaled(),
            line_gap: self.inner.line_gap_unscaled(),
        }
    }
}

/// Font metrics extracted from font tables.
#[derive(Debug, Clone, Copy)]
pub struct FontMetricsTable {
    /// Font design units per em.
    pub units_per_em: f32,
    /// Ascender height in design units.
    pub ascender: f32,
    /// Descender height in design units (negative value).
    pub descender: f32,
    /// Line gap in design units.
    pub line_gap: f32,
}

impl FontMetricsTable {
    /// Convert metrics to pixel values at a given font size.
    pub fn at_size(&self, size_px: f32) -> FontMetricsPixel {
        let scale = size_px / self.units_per_em;
        FontMetricsPixel {
            ascender: self.ascender * scale,
            descender: self.descender * scale,
            line_gap: self.line_gap * scale,
        }
    }
}

/// Font metrics in pixel units.
#[derive(Debug, Clone, Copy)]
pub struct FontMetricsPixel {
    /// Ascender height in pixels.
    pub ascender: f32,
    /// Descender height in pixels.
    pub descender: f32,
    /// Line gap in pixels.
    pub line_gap: f32,
}

// ============================================================================
// System Font Discovery (Phase 2)
// ============================================================================

/// Get platform-specific system font directories.
#[cfg(target_os = "macos")]
fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/System/Library/Fonts"),
        PathBuf::from("/Library/Fonts"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join("Library/Fonts"));
    }
    dirs
}

#[cfg(target_os = "linux")]
fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".local/share/fonts"));
        dirs.push(PathBuf::from(&home).join(".fonts"));
    }
    dirs
}

#[cfg(target_os = "windows")]
fn system_font_dirs() -> Vec<PathBuf> {
    vec![PathBuf::from("C:\\Windows\\Fonts")]
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn system_font_dirs() -> Vec<PathBuf> {
    vec![]
}

/// Map generic font family names to platform-specific font names.
fn generic_family_fonts(family: &str) -> Vec<&'static str> {
    match family.to_lowercase().as_str() {
        "sans-serif" => vec![
            "Helvetica",
            "Arial",
            "Liberation Sans",
            "DejaVu Sans",
            "Nimbus Sans",
            "FreeSans",
        ],
        "serif" => vec![
            "Times New Roman",
            "Times",
            "Liberation Serif",
            "DejaVu Serif",
            "Nimbus Roman",
            "FreeSerif",
        ],
        "monospace" | "mono" => vec![
            "Courier New",
            "Courier",
            "Liberation Mono",
            "DejaVu Sans Mono",
            "Nimbus Mono",
            "FreeMono",
            "Menlo",
            "Monaco",
        ],
        _ => vec![],
    }
}

/// Find a system font file matching the given family name.
///
/// Searches platform-specific font directories for TTF/OTF files whose
/// filename contains the requested family name (case-insensitive).
///
/// For generic families (sans-serif, serif, monospace), tries common
/// font names for each platform.
pub fn find_system_font(family: &str) -> Option<PathBuf> {
    // First try generic family mapping
    let candidates = generic_family_fonts(family);
    if !candidates.is_empty() {
        for candidate in candidates {
            if let Some(path) = search_font_dirs(candidate) {
                return Some(path);
            }
        }
    }

    // Then try direct family name search
    search_font_dirs(family)
}

/// Search system font directories for a font matching the given name.
fn search_font_dirs(name: &str) -> Option<PathBuf> {
    let name_lower = name.to_lowercase();

    for dir in system_font_dirs() {
        if !dir.exists() {
            continue;
        }

        if let Some(path) = search_font_dir_recursive(&dir, &name_lower) {
            return Some(path);
        }
    }

    None
}

/// Recursively search a directory for font files matching the name.
fn search_font_dir_recursive(dir: &Path, name_lower: &str) -> Option<PathBuf> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return None,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            // Recurse into subdirectories
            if let Some(found) = search_font_dir_recursive(&path, name_lower) {
                return Some(found);
            }
        } else if let Some(ext) = path.extension() {
            // Check if it's a font file
            let ext_lower = ext.to_string_lossy().to_lowercase();
            if ext_lower == "ttf" || ext_lower == "otf" || ext_lower == "ttc" {
                // Check if filename matches
                if let Some(stem) = path.file_stem() {
                    let stem_lower = stem.to_string_lossy().to_lowercase();
                    // Match if stem contains the search name (handles "Arial-Bold.ttf" etc.)
                    if stem_lower.contains(name_lower)
                        || name_lower.contains(&stem_lower)
                        || fuzzy_font_match(&stem_lower, name_lower)
                    {
                        return Some(path);
                    }
                }
            }
        }
    }

    None
}

/// Fuzzy matching for font names (handles common variations).
fn fuzzy_font_match(filename: &str, query: &str) -> bool {
    // Remove common suffixes and separators for comparison
    let clean_filename = filename
        .replace('-', "")
        .replace('_', "")
        .replace("regular", "")
        .replace("bold", "")
        .replace("italic", "")
        .replace("oblique", "");

    let clean_query = query
        .replace('-', "")
        .replace('_', "")
        .replace(' ', "");

    clean_filename.contains(&clean_query) || clean_query.contains(&clean_filename)
}

/// Load a system font by family name.
///
/// Searches system font directories and returns the first matching font.
/// Supports generic families: sans-serif, serif, monospace.
pub fn load_system_font(family: &str) -> Result<Font, FontError> {
    let path = find_system_font(family).ok_or_else(|| {
        FontError::Other(format!("System font '{}' not found", family))
    })?;

    Font::load_from_file(&path)
}

// ============================================================================
// Font and Glyph Caching (Phase 3)
// ============================================================================

/// Cache for loaded fonts, keyed by family name.
///
/// Fonts are expensive to load from disk, so we cache them by family name.
/// Uses `Arc<Font>` to allow sharing across multiple users.
pub struct FontCache {
    fonts: HashMap<String, Arc<Font>>,
    max_entries: usize,
}

impl FontCache {
    /// Create a new font cache with the specified maximum entries.
    pub fn new(max_entries: usize) -> Self {
        Self {
            fonts: HashMap::new(),
            max_entries,
        }
    }

    /// Get or load a font by family name.
    ///
    /// If the font is already cached, returns a clone of the Arc.
    /// Otherwise, loads the font from the system and caches it.
    pub fn get_or_load(&mut self, family: &str) -> Result<Arc<Font>, FontError> {
        let key = family.to_lowercase();

        if let Some(font) = self.fonts.get(&key) {
            return Ok(Arc::clone(font));
        }

        // Evict oldest entry if at capacity (simple FIFO eviction)
        if self.fonts.len() >= self.max_entries {
            if let Some(oldest_key) = self.fonts.keys().next().cloned() {
                self.fonts.remove(&oldest_key);
            }
        }

        let font = Arc::new(load_system_font(family)?);
        self.fonts.insert(key, Arc::clone(&font));
        Ok(font)
    }

    /// Clear all cached fonts.
    pub fn clear(&mut self) {
        self.fonts.clear();
    }

    /// Get the number of cached fonts.
    pub fn len(&self) -> usize {
        self.fonts.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }
}

impl Default for FontCache {
    fn default() -> Self {
        Self::new(20) // Default to 20 fonts
    }
}

/// Cache key for rasterized glyphs: (character, size in tenths of pixels).
/// Size is stored as integer tenths to allow HashMap lookup with floating point sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphCacheKey {
    ch: char,
    size_tenths: u32,
}

impl GlyphCacheKey {
    fn new(ch: char, size_px: f32) -> Self {
        Self {
            ch,
            size_tenths: (size_px * 10.0).round() as u32,
        }
    }
}

/// Cache for rasterized glyphs.
///
/// Glyph rasterization is CPU-intensive, so we cache the results.
/// This cache is per-font and stores rasterized bitmaps.
pub struct GlyphCache {
    glyphs: HashMap<GlyphCacheKey, GlyphRaster>,
    max_entries: usize,
}

impl GlyphCache {
    /// Create a new glyph cache with the specified maximum entries.
    pub fn new(max_entries: usize) -> Self {
        Self {
            glyphs: HashMap::new(),
            max_entries,
        }
    }

    /// Get a cached glyph raster, if present.
    pub fn get(&self, ch: char, size_px: f32) -> Option<&GlyphRaster> {
        let key = GlyphCacheKey::new(ch, size_px);
        self.glyphs.get(&key)
    }

    /// Insert a glyph raster into the cache.
    pub fn insert(&mut self, ch: char, size_px: f32, raster: GlyphRaster) {
        // Evict if at capacity (simple FIFO)
        if self.glyphs.len() >= self.max_entries {
            if let Some(oldest_key) = self.glyphs.keys().next().cloned() {
                self.glyphs.remove(&oldest_key);
            }
        }

        let key = GlyphCacheKey::new(ch, size_px);
        self.glyphs.insert(key, raster);
    }

    /// Get or rasterize a glyph using the provided font.
    pub fn get_or_rasterize(
        &mut self,
        font: &Font,
        ch: char,
        size_px: f32,
    ) -> Result<&GlyphRaster, FontError> {
        let key = GlyphCacheKey::new(ch, size_px);

        // Use entry API for efficient get-or-insert
        if !self.glyphs.contains_key(&key) {
            // Evict if at capacity
            if self.glyphs.len() >= self.max_entries {
                if let Some(oldest_key) = self.glyphs.keys().next().cloned() {
                    self.glyphs.remove(&oldest_key);
                }
            }

            let raster = font.rasterize(ch, size_px)?;
            self.glyphs.insert(key, raster);
        }

        Ok(self.glyphs.get(&key).unwrap())
    }

    /// Clear all cached glyphs.
    pub fn clear(&mut self) {
        self.glyphs.clear();
    }

    /// Get the number of cached glyphs.
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}

impl Default for GlyphCache {
    fn default() -> Self {
        Self::new(5000) // Default to 5000 glyphs
    }
}

// ============================================================================
// Text Width Measurement (Phase 4)
// ============================================================================

impl Font {
    /// Measure the total width of a text string at a given font size.
    ///
    /// This sums the advance widths of all characters in the string.
    pub fn measure_text_width(&self, text: &str, size_px: f32) -> f32 {
        text.chars()
            .map(|ch| self.glyph_advance(ch, size_px))
            .sum()
    }

    /// Calculate average character advance for a font at a given size.
    ///
    /// Uses a sample of common ASCII characters to estimate average width.
    pub fn average_advance(&self, size_px: f32) -> f32 {
        const SAMPLE: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let total: f32 = SAMPLE
            .chars()
            .map(|ch| self.glyph_advance(ch, size_px))
            .sum();
        total / SAMPLE.len() as f32
    }
}

/// Font metrics that can be used for layout calculations.
///
/// This struct mirrors `layout::FontMetrics` but can be populated
/// from actual font data instead of approximations.
#[derive(Debug, Clone, Copy)]
pub struct LayoutFontMetrics {
    pub font_size: f32,
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub average_advance: f32,
}

impl Font {
    /// Create layout-compatible font metrics from actual font data.
    ///
    /// These metrics can be used to populate `layout::FontMetrics`
    /// for more accurate text layout.
    pub fn layout_metrics(&self, size_px: f32) -> LayoutFontMetrics {
        let table = self.metrics();
        let px = table.at_size(size_px);

        LayoutFontMetrics {
            font_size: size_px,
            ascent: px.ascender.abs(),
            descent: px.descender.abs(),
            line_gap: px.line_gap.abs(),
            average_advance: self.average_advance(size_px),
        }
    }
}
