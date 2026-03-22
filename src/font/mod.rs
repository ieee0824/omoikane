//! Font loading and glyph rendering module.
//!
//! Provides TrueType/OpenType font support using the `ab_glyph` crate.
//! Handles font file loading, character-to-glyph mapping, and rasterization.

use ab_glyph::{Font as AbGlyphFont, FontVec, ScaleFont};
use std::collections::{HashMap, HashSet};
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

impl std::error::Error for FontError {}

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
    /// Horizontal offset from the pen position to the bitmap left edge.
    pub offset_x: f32,
    /// Vertical offset from the baseline to the bitmap top edge (typically negative).
    pub offset_y: f32,
}

/// Font representation wrapping `ab_glyph::FontVec`.
pub struct Font {
    inner: FontVec,
}

impl Font {
    /// Load a font from a file path.
    pub fn load_from_file(path: &Path) -> Result<Self, FontError> {
        let data = std::fs::read(path)?;
        Self::load_from_bytes(data)
    }

    /// Load a font from raw bytes (TTF, OTF, or WOFF).
    ///
    /// WOFF fonts are decompressed (zlib) before parsing.
    /// WOFF2 is not yet supported; use TTF/OTF directly.
    pub fn load_from_bytes(data: Vec<u8>) -> Result<Self, FontError> {
        let data = decode_font_data(data)?;
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
                offset_x: 0.0,
                offset_y: 0.0,
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
                offset_x: bounds.min.x,
                offset_y: bounds.min.y,
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
            offset_x: bounds.min.x,
            offset_y: bounds.min.y,
        })
    }

    /// Returns true when this font has a dedicated glyph for `ch`.
    ///
    /// `ab_glyph` returns glyph id 0 when a code point is missing and the
    /// font falls back to `.notdef`.
    pub fn has_glyph(&self, ch: char) -> bool {
        self.inner.glyph_id(ch).0 != 0
    }

    /// Get the horizontal advance width for a character at a given font size.
    pub fn glyph_advance(&self, ch: char, size_px: f32) -> f32 {
        let glyph_id = self.inner.glyph_id(ch);
        let scaled = self.inner.as_scaled(size_px);
        scaled.h_advance(glyph_id)
    }

    /// Get additional horizontal kerning for a pair of characters at a given font size.
    pub fn glyph_kerning(&self, previous: char, current: char, size_px: f32) -> f32 {
        let prev_id = self.inner.glyph_id(previous);
        let curr_id = self.inner.glyph_id(current);
        let scaled = self.inner.as_scaled(size_px);
        scaled.kern(prev_id, curr_id)
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
// Font Data Decoding (WOFF / raw TTF/OTF)
// ============================================================================

/// Detect font format from magic bytes and decode if necessary.
///
/// - `wOFF` (WOFF1): decompress each table with zlib/flate2
/// - `wOF2` (WOFF2): not yet supported, returns an error
/// - Otherwise: assume raw TTF/OTF and return as-is
fn decode_font_data(data: Vec<u8>) -> Result<Vec<u8>, FontError> {
    if data.len() < 4 {
        return Err(FontError::InvalidFont("Font data too short".to_string()));
    }
    let magic = &data[..4];
    if magic == b"wOF2" {
        return Err(FontError::InvalidFont(
            "WOFF2 format is not yet supported; use TTF/OTF".to_string(),
        ));
    }
    if magic == b"wOFF" {
        return decode_woff1(data);
    }
    // Raw TTF/OTF (or TTC)
    Ok(data)
}

/// Decode a WOFF1 container into raw OpenType/TrueType data.
///
/// WOFF1 spec: <https://www.w3.org/TR/WOFF/>
fn decode_woff1(data: Vec<u8>) -> Result<Vec<u8>, FontError> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    if data.len() < 44 {
        return Err(FontError::InvalidFont("WOFF header too short".to_string()));
    }

    let read_u16 = |off: usize| -> u16 { u16::from_be_bytes([data[off], data[off + 1]]) };
    let read_u32 = |off: usize| -> u32 {
        u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
    };

    let sfnt_flavor = read_u32(4);
    let num_tables = read_u16(12) as usize;
    let _total_sfnt_size = read_u32(16) as usize;

    // Build the output sfnt
    // sfnt header: 12 bytes + 16 bytes per table record
    let header_size = 12 + 16 * num_tables;
    let mut sfnt = Vec::new();

    // Write sfnt header
    sfnt.extend_from_slice(&sfnt_flavor.to_be_bytes());
    sfnt.extend_from_slice(&(num_tables as u16).to_be_bytes());
    // searchRange, entrySelector, rangeShift — compute from numTables
    let entry_selector = (num_tables as f64).log2().floor() as u16;
    let search_range = (1u16 << entry_selector) * 16;
    let range_shift = (num_tables as u16) * 16 - search_range;
    sfnt.extend_from_slice(&search_range.to_be_bytes());
    sfnt.extend_from_slice(&entry_selector.to_be_bytes());
    sfnt.extend_from_slice(&range_shift.to_be_bytes());

    // Parse WOFF table directory (starts at offset 44)
    struct WoffTableEntry {
        tag: [u8; 4],
        comp_offset: usize,
        comp_length: usize,
        orig_length: usize,
        orig_checksum: u32,
    }

    let mut entries = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let base = 44 + i * 20;
        if base + 20 > data.len() {
            return Err(FontError::InvalidFont("WOFF table directory truncated".to_string()));
        }
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&data[base..base + 4]);
        entries.push(WoffTableEntry {
            tag,
            comp_offset: read_u32(base + 4) as usize,
            comp_length: read_u32(base + 8) as usize,
            orig_length: read_u32(base + 12) as usize,
            orig_checksum: read_u32(base + 16),
        });
    }

    // Reserve space for table records (we'll fill offsets later)
    let table_records_start = sfnt.len();
    sfnt.resize(header_size, 0);

    // Decompress each table and write it, recording offsets
    struct SfntRecord {
        tag: [u8; 4],
        checksum: u32,
        offset: u32,
        length: u32,
    }
    let mut records = Vec::with_capacity(num_tables);

    for entry in &entries {
        // Pad to 4-byte boundary
        while sfnt.len() % 4 != 0 {
            sfnt.push(0);
        }
        let offset = sfnt.len() as u32;

        if entry.comp_length >= entry.orig_length {
            // Not compressed — copy raw
            let end = entry.comp_offset
                .checked_add(entry.orig_length)
                .ok_or_else(|| FontError::InvalidFont("WOFF table offset overflow".to_string()))?;
            if end > data.len() {
                return Err(FontError::InvalidFont("WOFF table data out of bounds".to_string()));
            }
            sfnt.extend_from_slice(&data[entry.comp_offset..end]);
        } else {
            // Zlib compressed
            let end = entry.comp_offset
                .checked_add(entry.comp_length)
                .ok_or_else(|| FontError::InvalidFont("WOFF table offset overflow".to_string()))?;
            if end > data.len() {
                return Err(FontError::InvalidFont("WOFF table data out of bounds".to_string()));
            }
            let mut decoder = ZlibDecoder::new(&data[entry.comp_offset..end]);
            let mut decompressed = Vec::with_capacity(entry.orig_length);
            decoder.read_to_end(&mut decompressed).map_err(|e| {
                FontError::InvalidFont(format!("WOFF zlib decompression failed: {}", e))
            })?;
            if decompressed.len() != entry.orig_length {
                return Err(FontError::InvalidFont(format!(
                    "WOFF decompressed size mismatch: expected {}, got {}",
                    entry.orig_length,
                    decompressed.len()
                )));
            }
            sfnt.extend_from_slice(&decompressed);
        }

        records.push(SfntRecord {
            tag: entry.tag,
            checksum: entry.orig_checksum,
            offset,
            length: entry.orig_length as u32,
        });
    }

    // Write table records
    for (i, rec) in records.iter().enumerate() {
        let base = table_records_start + i * 16;
        sfnt[base..base + 4].copy_from_slice(&rec.tag);
        sfnt[base + 4..base + 8].copy_from_slice(&rec.checksum.to_be_bytes());
        sfnt[base + 8..base + 12].copy_from_slice(&rec.offset.to_be_bytes());
        sfnt[base + 12..base + 16].copy_from_slice(&rec.length.to_be_bytes());
    }

    Ok(sfnt)
}

/// Detect font format from raw bytes.
///
/// Returns `"woff"`, `"woff2"`, `"ttf"`, `"otf"`, or `"unknown"`.
pub fn detect_font_format(data: &[u8]) -> &'static str {
    if data.len() < 4 {
        return "unknown";
    }
    match &data[..4] {
        b"wOFF" => "woff",
        b"wOF2" => "woff2",
        [0x00, 0x01, 0x00, 0x00] => "ttf",
        b"OTTO" => "otf",
        b"true" => "ttf",
        b"ttcf" => "ttf", // TrueType Collection
        _ => "unknown",
    }
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

    let clean_query = query.replace('-', "").replace('_', "").replace(' ', "");

    clean_filename.contains(&clean_query) || clean_query.contains(&clean_filename)
}

/// Load a system font by family name.
///
/// Searches system font directories and returns the first matching font.
/// Supports generic families: sans-serif, serif, monospace.
pub fn load_system_font(family: &str) -> Result<Font, FontError> {
    let path = find_system_font(family)
        .ok_or_else(|| FontError::Other(format!("System font '{}' not found", family)))?;

    Font::load_from_file(&path)
}

/// Load default text fonts shared by layout and paint.
///
/// The first successfully loaded family becomes the primary font.
/// Remaining fonts are fallback candidates (with CJK-preferred families included).
pub fn load_default_text_fonts() -> Vec<Font> {
    let mut fonts = Vec::new();
    let mut loaded_families = HashSet::new();
    let families = [
        "sans-serif",
        "Hiragino Sans",
        "Hiragino Kaku Gothic ProN",
        "Yu Gothic",
        "Meiryo",
        "MS Gothic",
        "Noto Sans CJK JP",
        "Noto Sans JP",
        "IPA Gothic",
        "IPAGothic",
    ];

    for family in families {
        if !loaded_families.insert(family.to_ascii_lowercase()) {
            continue;
        }
        if let Ok(font) = load_system_font(family) {
            fonts.push(font);
        }
    }

    fonts
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

        // Evict an arbitrary entry if at capacity (HashMap iteration order is non-deterministic)
        if self.fonts.len() >= self.max_entries {
            if let Some(oldest_key) = self.fonts.keys().next().cloned() {
                self.fonts.remove(&oldest_key);
            }
        }

        let font = Arc::new(load_system_font(family)?);
        self.fonts.insert(key, Arc::clone(&font));
        Ok(font)
    }

    /// Register a web font by family name from raw font bytes.
    ///
    /// The font data can be TTF, OTF, or WOFF (zlib-compressed).
    /// WOFF2 is not yet supported.
    /// Web fonts take priority over system fonts in `get_or_load`.
    /// If the cache is at capacity, an existing entry is evicted to make room.
    pub fn register_web_font(&mut self, family: &str, data: Vec<u8>) -> Result<Arc<Font>, FontError> {
        let key = family.to_lowercase();
        // Evict an arbitrary entry if at capacity and this family is not already cached
        if !self.fonts.contains_key(&key) && self.fonts.len() >= self.max_entries {
            if let Some(oldest_key) = self.fonts.keys().next().cloned() {
                self.fonts.remove(&oldest_key);
            }
        }
        let font = Arc::new(Font::load_from_bytes(data)?);
        self.fonts.insert(key, Arc::clone(&font));
        Ok(font)
    }

    /// Check if a font for the given family is already cached (system or web).
    pub fn contains(&self, family: &str) -> bool {
        self.fonts.contains_key(&family.to_lowercase())
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
        // Validate and normalize the size to avoid relying on implicit float-to-int
        // casting behavior (NaN/∞/negative -> 0, large -> saturate).
        let size_tenths = if size_px.is_finite() && size_px > 0.0 {
            let scaled = (size_px * 10.0).round();
            scaled.clamp(0.0, u32::MAX as f32) as u32
        } else {
            0
        };
        Self { ch, size_tenths }
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
        // Evict an arbitrary entry if at capacity (HashMap iteration order is non-deterministic)
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
            // Evict an arbitrary entry if at capacity
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
    /// This sums advances and pair kerning for all characters in the string.
    pub fn measure_text_width(&self, text: &str, size_px: f32) -> f32 {
        let scaled = self.inner.as_scaled(size_px);
        let mut width = 0.0;
        let mut previous_id = None;

        for ch in text.chars() {
            let glyph_id = self.inner.glyph_id(ch);
            if let Some(prev_id) = previous_id {
                width += scaled.kern(prev_id, glyph_id);
            }
            width += scaled.h_advance(glyph_id);
            previous_id = Some(glyph_id);
        }

        width
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
