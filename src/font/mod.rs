//! Font loading and glyph rendering module.
//!
//! Provides TrueType/OpenType font support using the `ab_glyph` crate.
//! Handles font file loading, character-to-glyph mapping, and rasterization.

use ab_glyph::{Font as AbGlyphFont, FontVec, GlyphId, ScaleFont};
use rustybuzz::{Direction, Face, UnicodeBuffer};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
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

/// One positioned glyph produced by OpenType shaping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    pub glyph_id: u16,
    pub cluster: usize,
    pub x_advance: f32,
    pub y_advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapingDirection {
    LeftToRight,
    RightToLeft,
    TopToBottom,
    BottomToTop,
}

/// Font representation wrapping `ab_glyph::FontVec`.
pub struct Font {
    inner: FontVec,
}

/// Returns whether a code point is a shaping control or combining mark that
/// must not advance the inline cursor on its own.  `ab_glyph` exposes glyph
/// metrics one scalar at a time, so these code points need an explicit zero
/// advance policy until a full OpenType shaping pass is available.
pub(crate) fn is_zero_advance_character(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0300..=0x036f // Combining Diacritical Marks
            | 0x1ab0..=0x1aff // Combining Diacritical Marks Extended
            | 0x1dc0..=0x1dff // Combining Diacritical Marks Supplement
            | 0x20d0..=0x20ff // Combining Diacritical Marks for Symbols
            | 0xfe00..=0xfe0f // Variation Selectors
            | 0xfe20..=0xfe2f // Combining Half Marks
            | 0xe0100..=0xe01ef // Variation Selectors Supplement
            | 0x200c // Zero Width Non-Joiner
            | 0x200d // Zero Width Joiner
    )
}

impl Font {
    /// Load a font from a file path.
    pub fn load_from_file(path: &Path) -> Result<Self, FontError> {
        let data = std::fs::read(path)?;
        Self::load_from_bytes(data)
    }

    /// Load a font from raw bytes (TTF, OTF, WOFF, or WOFF2).
    ///
    /// WOFF fonts are decompressed (zlib) before parsing.
    /// WOFF2 fonts are decompressed (brotli) and reconstructed as sfnt before parsing.
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

        self.rasterize_glyph_id(glyph_id, size_px, advance_x)
    }

    /// Shapes a complete script run and returns positioned glyphs in visual order.
    pub fn shape_text(
        &self,
        text: &str,
        size_px: f32,
        direction: ShapingDirection,
    ) -> Result<Vec<ShapedGlyph>, FontError> {
        let face = Face::from_slice(self.inner.font_data(), 0)
            .ok_or_else(|| FontError::InvalidFont("Failed to build shaping face".to_string()))?;
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.set_direction(match direction {
            ShapingDirection::LeftToRight => Direction::LeftToRight,
            ShapingDirection::RightToLeft => Direction::RightToLeft,
            ShapingDirection::TopToBottom => Direction::TopToBottom,
            ShapingDirection::BottomToTop => Direction::BottomToTop,
        });
        buffer.guess_segment_properties();
        let glyphs = rustybuzz::shape(&face, &[], buffer);
        let scale = size_px / face.units_per_em().max(1) as f32;
        Ok(glyphs
            .glyph_infos()
            .iter()
            .zip(glyphs.glyph_positions())
            .map(|(info, position)| ShapedGlyph {
                glyph_id: info.glyph_id as u16,
                cluster: info.cluster as usize,
                x_advance: position.x_advance as f32 * scale,
                y_advance: position.y_advance as f32 * scale,
                x_offset: position.x_offset as f32 * scale,
                y_offset: position.y_offset as f32 * scale,
            })
            .collect())
    }

    /// Rasterizes a glyph selected by the shaping engine.
    pub fn rasterize_glyph(&self, glyph_id: u16, size_px: f32) -> Result<GlyphRaster, FontError> {
        let glyph_id = GlyphId(glyph_id);
        let advance_x = self.inner.as_scaled(size_px).h_advance(glyph_id);
        self.rasterize_glyph_id(glyph_id, size_px, advance_x)
    }

    fn rasterize_glyph_id(
        &self,
        glyph_id: GlyphId,
        size_px: f32,
        advance_x: f32,
    ) -> Result<GlyphRaster, FontError> {
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
/// - `wOF2` (WOFF2): decompress with brotli, reconstruct sfnt
/// - Otherwise: assume raw TTF/OTF and return as-is
fn decode_font_data(data: Vec<u8>) -> Result<Vec<u8>, FontError> {
    if data.len() < 4 {
        return Err(FontError::InvalidFont("Font data too short".to_string()));
    }
    let magic = &data[..4];
    if magic == b"wOF2" {
        return decode_woff2(&data);
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

/// Decode a WOFF2 container into raw OpenType/TrueType (sfnt) data.
///
/// WOFF2 spec: <https://www.w3.org/TR/WOFF2/>
///
/// The process is:
/// 1. Parse the 48-byte WOFF2 header.
/// 2. Parse the variable-length table directory entries (UIntBase128 encoded sizes).
/// 3. Brotli-decompress the compressed font data block.
/// 4. Reassemble an sfnt binary from the decompressed table data.
///
/// Note: the `glyf`/`loca` transform (WOFF2 §5) is not applied. Fonts that
/// use the transformed format will fail to load; TTF/OTF equivalents should
/// be used in that case.
pub(crate) fn decode_woff2(data: &[u8]) -> Result<Vec<u8>, FontError> {
    use std::io::Read;

    // --- WOFF2 Header (48 bytes) ---
    // Offset  Size  Field
    //  0       4    signature (= 0x774F4632 "wOF2")
    //  4       4    flavor (sfnt version tag)
    //  8       4    length (total WOFF2 file size)
    // 12       2    numTables
    // 14       2    reserved
    // 16       4    totalSfntSize
    // 20       4    totalCompressedSize
    // 24       2    majorVersion
    // 26       2    minorVersion
    // 28       4    metaOffset
    // 32       4    metaLength
    // 36       4    metaOrigLength
    // 40       4    privOffset
    // 44       4    privLength
    if data.len() < 48 {
        return Err(FontError::InvalidFont("WOFF2 header too short".to_string()));
    }

    let read_u16 = |off: usize| -> u16 { u16::from_be_bytes([data[off], data[off + 1]]) };
    let read_u32 = |off: usize| -> u32 {
        u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
    };

    let sfnt_flavor = read_u32(4);
    let num_tables = read_u16(12) as usize;
    let total_compressed_size = read_u32(20) as usize;

    if num_tables == 0 {
        return Err(FontError::InvalidFont("WOFF2 has no tables".to_string()));
    }

    // --- Parse table directory (variable-length entries) ---
    // Each entry:
    //   flags      1 byte  (bits 0-5: table tag index, bits 6-7: transform version)
    //   tag        0 or 4 bytes  (only present when flags & 0x3f == 0x3f)
    //   origLength UIntBase128
    //   transformLength UIntBase128  (only when transform != 0)
    //
    // Known tag indices (WOFF2 spec Table 3):
    const KNOWN_TAGS: [&[u8; 4]; 63] = [
        b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post",
        b"cvt ", b"fpgm", b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT",
        b"EBLC", b"gasp", b"hdmx", b"kern", b"LTSH", b"PCLT", b"VDMX", b"vhea",
        b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC", b"JSTF", b"MATH",
        b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar",
        b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar",
        b"gvar", b"hsty", b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop",
        b"trak", b"Zapf", b"Silf", b"Glat", b"Gloc", b"Feat", b"Sill",
    ];

    struct Woff2TableEntry {
        tag: [u8; 4],
        orig_length: usize,
        transform_length: Option<usize>, // Some when transform is applied
    }

    /// Decode a UIntBase128 value from `data` starting at `*pos`.
    /// Returns the decoded value and advances `*pos`.
    fn read_uint_base128(data: &[u8], pos: &mut usize) -> Result<usize, FontError> {
        let mut accum: u32 = 0;
        for i in 0..5 {
            if *pos >= data.len() {
                return Err(FontError::InvalidFont(
                    "WOFF2 UIntBase128: unexpected end of data".to_string(),
                ));
            }
            let byte = data[*pos];
            *pos += 1;
            // Leading zeros are invalid for multi-byte sequences
            if i == 0 && byte == 0x80 {
                return Err(FontError::InvalidFont(
                    "WOFF2 UIntBase128: leading zero byte".to_string(),
                ));
            }
            // Overflow check: top 7 bits of accum must be 0 before shifting
            if accum & 0xfe00_0000 != 0 {
                return Err(FontError::InvalidFont(
                    "WOFF2 UIntBase128: value overflow".to_string(),
                ));
            }
            accum = (accum << 7) | (byte & 0x7f) as u32;
            if byte & 0x80 == 0 {
                return Ok(accum as usize);
            }
        }
        Err(FontError::InvalidFont(
            "WOFF2 UIntBase128: sequence too long".to_string(),
        ))
    }

    let mut pos = 48usize; // start of table directory
    let mut entries: Vec<Woff2TableEntry> = Vec::with_capacity(num_tables);

    for _ in 0..num_tables {
        if pos >= data.len() {
            return Err(FontError::InvalidFont(
                "WOFF2 table directory truncated".to_string(),
            ));
        }
        let flags = data[pos];
        pos += 1;

        let tag_index = (flags & 0x3f) as usize;
        let transform_version = (flags >> 6) & 0x03;

        // Resolve the 4-byte tag
        let tag: [u8; 4] = if tag_index == 0x3f {
            // Arbitrary tag follows
            if pos + 4 > data.len() {
                return Err(FontError::InvalidFont(
                    "WOFF2 table tag truncated".to_string(),
                ));
            }
            let mut t = [0u8; 4];
            t.copy_from_slice(&data[pos..pos + 4]);
            pos += 4;
            t
        } else if tag_index < KNOWN_TAGS.len() {
            *KNOWN_TAGS[tag_index]
        } else {
            return Err(FontError::InvalidFont(format!(
                "WOFF2 unknown tag index: {}",
                tag_index
            )));
        };

        let orig_length = read_uint_base128(data, &mut pos)?;

        // WOFF2 spec §5.2 — transform_version semantics:
        // For glyf (tag index 10) and loca (tag index 11):
        //   transform_version 0 = transformed (transform_length field IS present)
        //   transform_version 3 = no transform (transform_length field absent)
        //   transform_version 1/2 = reserved → reject
        // For all other tables:
        //   transform_version 0 = no transform (transform_length field absent)
        //   transform_version 3 = transform applied (unsupported) → reject
        //   transform_version 1/2 = reserved → reject
        let is_glyf_or_loca = tag == *b"glyf" || tag == *b"loca";
        let has_transform = if is_glyf_or_loca {
            match transform_version {
                0 => true,  // transformed format (transform_length present)
                3 => false, // no transform
                v => return Err(FontError::InvalidFont(format!(
                    "WOFF2 glyf/loca reserved transform_version {}", v
                ))),
            }
        } else {
            match transform_version {
                0 => false, // no transform
                3 => return Err(FontError::InvalidFont(format!(
                    "WOFF2 table '{}' transform version 3 is not supported",
                    std::str::from_utf8(&tag).unwrap_or("????")
                ))),
                v => return Err(FontError::InvalidFont(format!(
                    "WOFF2 table '{}' reserved transform_version {}",
                    std::str::from_utf8(&tag).unwrap_or("????"), v
                ))),
            }
        };

        let transform_length = if has_transform {
            Some(read_uint_base128(data, &mut pos)?)
        } else {
            None
        };

        entries.push(Woff2TableEntry {
            tag,
            orig_length,
            transform_length,
        });
    }

    // Compressed data follows the table directory
    let compressed_start = pos;
    let compressed_end = compressed_start
        .checked_add(total_compressed_size)
        .ok_or_else(|| FontError::InvalidFont("WOFF2 compressed data overflow".to_string()))?;
    if compressed_end > data.len() {
        return Err(FontError::InvalidFont(
            "WOFF2 compressed data out of bounds".to_string(),
        ));
    }

    // Decompress the data block with brotli
    // Use checked_add to avoid overflow, and cap at 100 MB as a sanity limit.
    const MAX_ORIG_SIZE: usize = 100 * 1024 * 1024; // 100 MB
    let total_orig: usize = entries.iter().try_fold(0usize, |acc, e| {
        let stored = e.transform_length.unwrap_or(e.orig_length);
        acc.checked_add(stored)
    }).ok_or_else(|| FontError::InvalidFont("WOFF2 total decompressed size overflow".to_string()))?;
    if total_orig > MAX_ORIG_SIZE {
        return Err(FontError::InvalidFont(format!(
            "WOFF2 total decompressed size {} exceeds limit {}",
            total_orig, MAX_ORIG_SIZE
        )));
    }

    let mut decompressed = Vec::with_capacity(total_orig);
    {
        let mut reader = brotli::Decompressor::new(
            &data[compressed_start..compressed_end],
            4096,
        );
        reader.read_to_end(&mut decompressed).map_err(|e| {
            FontError::InvalidFont(format!("WOFF2 brotli decompression failed: {}", e))
        })?;
    }

    if decompressed.len() < total_orig {
        return Err(FontError::InvalidFont(format!(
            "WOFF2 decompressed size too small: expected at least {}, got {}",
            total_orig,
            decompressed.len()
        )));
    }

    // --- Rebuild sfnt binary ---
    let sfnt_num_tables = num_tables;
    let entry_selector = if sfnt_num_tables > 0 {
        (sfnt_num_tables as f64).log2().floor() as u16
    } else {
        0
    };
    let search_range = (1u16 << entry_selector) * 16;
    let range_shift = (sfnt_num_tables as u16) * 16 - search_range;
    let header_size = 12 + 16 * sfnt_num_tables;

    let mut sfnt = Vec::new();
    // sfnt header
    sfnt.extend_from_slice(&sfnt_flavor.to_be_bytes());
    sfnt.extend_from_slice(&(sfnt_num_tables as u16).to_be_bytes());
    sfnt.extend_from_slice(&search_range.to_be_bytes());
    sfnt.extend_from_slice(&entry_selector.to_be_bytes());
    sfnt.extend_from_slice(&range_shift.to_be_bytes());
    // Reserve space for table records
    let table_records_start = sfnt.len();
    sfnt.resize(header_size, 0);

    struct SfntRecord {
        tag: [u8; 4],
        checksum: u32,
        offset: u32,
        length: u32,
    }
    let mut records: Vec<SfntRecord> = Vec::with_capacity(sfnt_num_tables);

    let mut decomp_offset = 0usize;

    for entry in &entries {
        // The stored (possibly transformed) byte count
        let stored_len = entry.transform_length.unwrap_or(entry.orig_length);

        let table_data = decompressed
            .get(decomp_offset..decomp_offset + stored_len)
            .ok_or_else(|| {
                FontError::InvalidFont(format!(
                    "WOFF2 decompressed data too short for table '{}'",
                    std::str::from_utf8(&entry.tag).unwrap_or("????")
                ))
            })?;

        // If any transform is present we cannot reverse it; reject the font.
        if entry.transform_length.is_some() {
            return Err(FontError::InvalidFont(format!(
                "WOFF2 table '{}' transform is not supported; use TTF/OTF or untransformed WOFF2",
                std::str::from_utf8(&entry.tag).unwrap_or("????")
            )));
        }

        // Pad to 4-byte boundary
        while sfnt.len() % 4 != 0 {
            sfnt.push(0);
        }
        let offset = sfnt.len() as u32;

        sfnt.extend_from_slice(table_data);

        // Calculate sfnt checksum for this table
        let checksum = sfnt_table_checksum(table_data);

        records.push(SfntRecord {
            tag: entry.tag,
            checksum,
            offset,
            length: entry.orig_length as u32,
        });

        decomp_offset = decomp_offset.checked_add(stored_len).ok_or_else(|| {
            FontError::InvalidFont("WOFF2 decompressed offset overflow".to_string())
        })?;
    }

    // Write table records into the reserved area
    for (i, rec) in records.iter().enumerate() {
        let base = table_records_start + i * 16;
        sfnt[base..base + 4].copy_from_slice(&rec.tag);
        sfnt[base + 4..base + 8].copy_from_slice(&rec.checksum.to_be_bytes());
        sfnt[base + 8..base + 12].copy_from_slice(&rec.offset.to_be_bytes());
        sfnt[base + 12..base + 16].copy_from_slice(&rec.length.to_be_bytes());
    }

    Ok(sfnt)
}

/// Compute the sfnt table checksum (sum of 32-bit big-endian words, wrapping).
fn sfnt_table_checksum(data: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    let chunks = data.chunks(4);
    for chunk in chunks {
        let word = match chunk.len() {
            4 => u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
            3 => u32::from_be_bytes([chunk[0], chunk[1], chunk[2], 0]),
            2 => u32::from_be_bytes([chunk[0], chunk[1], 0, 0]),
            1 => u32::from_be_bytes([chunk[0], 0, 0, 0]),
            _ => 0,
        };
        sum = sum.wrapping_add(word);
    }
    sum
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
        .replace(['-', '_'], "")
        .replace("regular", "")
        .replace("bold", "")
        .replace("italic", "")
        .replace("oblique", "");

    let clean_query = query.replace(['-', '_', ' '], "");

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
/// Returns `true` if the character is in a CJK Unicode block and should
/// preferentially use a CJK-capable font.
pub fn is_cjk_preferred_character(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3000..=0x30FF // CJK Symbols/Punctuation, Hiragana, Katakana
            | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
            | 0xFF00..=0xFFEF // Half-width and Full-width Forms
    )
}

/// The first successfully loaded family becomes the primary font.
/// Remaining fonts are fallback candidates (with CJK-preferred families included).
pub fn load_default_text_fonts() -> Vec<Font> {
    let mut fonts = Vec::new();
    let mut loaded_families = HashSet::new();
    let families = if cfg!(target_os = "macos") {
        &[
            "Hiragino Kaku Gothic ProN",
            "Hiragino Sans",
            "sans-serif",
            "Yu Gothic",
            "Noto Sans CJK JP",
            "Noto Sans JP",
        ][..]
    } else {
        &[
            "sans-serif",
            "Noto Sans CJK JP",
            "Noto Sans JP",
            "Yu Gothic",
            "Meiryo",
            "MS Gothic",
            "IPA Gothic",
            "IPAGothic",
        ][..]
    };

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
// Font Variant Key (weight / style)
// ============================================================================

/// Parsed font-weight value normalised to a numeric value (100–900).
///
/// CSS Fonts Module Level 3 §5.2 defines numeric weights 100–900.
/// The keywords `normal` and `bold` map to 400 and 700 respectively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FontWeight(pub u16);

impl FontWeight {
    /// Parse a CSS font-weight descriptor value (e.g. `"bold"`, `"400"`).
    ///
    /// Returns `FontWeight(400)` (normal) when the value cannot be parsed.
    pub fn parse(value: &str) -> Self {
        let trimmed = value.trim();
        if trimmed.eq_ignore_ascii_case("normal") {
            Self(400)
        } else if trimmed.eq_ignore_ascii_case("bold") || trimmed.eq_ignore_ascii_case("bolder") {
            Self(700) // simplified — treat as bold
        } else if trimmed.eq_ignore_ascii_case("lighter") {
            Self(300) // simplified
        } else if let Ok(n) = trimmed.parse::<u16>() {
            Self(n.clamp(1, 1000))
        } else {
            Self(400)
        }
    }
}

impl Default for FontWeight {
    fn default() -> Self {
        Self(400)
    }
}

/// Parsed font-style value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[derive(Default)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
    Oblique,
}

impl FontStyle {
    /// Parse a CSS font-style descriptor value.
    pub fn parse(value: &str) -> Self {
        let trimmed = value.trim();
        if trimmed.eq_ignore_ascii_case("italic") {
            Self::Italic
        } else if trimmed.eq_ignore_ascii_case("oblique") {
            Self::Oblique
        } else {
            Self::Normal
        }
    }
}


/// Cache key for a specific font variant (weight + style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FontVariantKey {
    pub weight: FontWeight,
    pub style: FontStyle,
}

/// Stable, case-insensitive identifier for a CSS font family.
///
/// Keys are interned in a process-wide table: names that fold to the same
/// trimmed, Unicode-lowercased string share a key, and distinct names always
/// receive distinct keys, so key equality is exactly folded-name equality
/// (no hash collisions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FontFamilyKey(u32);

static FONT_FAMILY_KEY_INTERN: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();

impl FontFamilyKey {
    /// Interns `family` (trimmed and Unicode-lowercased) and returns its key.
    pub fn new(family: &str) -> Self {
        let folded = family.trim().to_lowercase();
        let intern = FONT_FAMILY_KEY_INTERN.get_or_init(|| Mutex::new(HashMap::new()));
        let mut intern = intern.lock().unwrap_or_else(PoisonError::into_inner);
        let next = intern.len() as u32;
        Self(*intern.entry(folded).or_insert(next))
    }
}

impl FontVariantKey {
    /// Create a new variant key.
    pub fn new(weight: FontWeight, style: FontStyle) -> Self {
        Self { weight, style }
    }

    /// Normal weight (400), normal style.
    pub fn normal() -> Self {
        Self {
            weight: FontWeight(400),
            style: FontStyle::Normal,
        }
    }
}

// ============================================================================
// Font and Glyph Caching (Phase 3)
// ============================================================================

/// Cache for loaded fonts, keyed by family name and variant (weight + style).
///
/// Fonts are expensive to load from disk, so we cache them by family name.
/// Uses `Arc<Font>` to allow sharing across multiple users.
///
/// Each family can have multiple variants (e.g. weight 400/700, italic/normal).
/// `select_best_variant` implements the CSS Fonts Module Level 3 §5.2 matching
/// algorithm to pick the closest available variant for a requested weight/style.
pub struct FontCache {
    /// `(family_lowercase, variant_key) -> Arc<Font>`
    fonts: HashMap<(String, FontVariantKey), Arc<Font>>,
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

    /// Get or load a font by family name with the default variant (normal weight/style).
    ///
    /// If the font is already cached, returns a clone of the Arc.
    /// Otherwise, loads the font from the system and caches it.
    pub fn get_or_load(&mut self, family: &str) -> Result<Arc<Font>, FontError> {
        self.get_or_load_variant(family, FontVariantKey::normal())
    }

    /// Get or load a font by family name and variant.
    ///
    /// If the font is already cached for the exact variant, returns a clone of the Arc.
    /// Otherwise, falls back to any cached variant for the same family, or loads from
    /// the system.
    pub fn get_or_load_variant(
        &mut self,
        family: &str,
        variant: FontVariantKey,
    ) -> Result<Arc<Font>, FontError> {
        let key = (family.to_lowercase(), variant);

        if let Some(font) = self.fonts.get(&key) {
            return Ok(Arc::clone(font));
        }

        // Try any cached variant of this family before hitting disk
        if let Some(font) = self.select_best_variant(family, variant.weight, variant.style) {
            return Ok(font);
        }

        // Evict an arbitrary entry if at capacity
        if self.fonts.len() >= self.max_entries
            && let Some(oldest_key) = self.fonts.keys().next().cloned() {
                self.fonts.remove(&oldest_key);
            }

        let font = Arc::new(load_system_font(family)?);
        self.fonts.insert(key, Arc::clone(&font));
        Ok(font)
    }

    /// Register a web font by family name with the default variant (normal weight/style).
    ///
    /// The font data can be TTF, OTF, WOFF (zlib-compressed), or WOFF2 (brotli-compressed).
    /// Web fonts take priority over system fonts in `get_or_load`.
    /// If the cache is at capacity, an existing entry is evicted to make room.
    pub fn register_web_font(&mut self, family: &str, data: Vec<u8>) -> Result<Arc<Font>, FontError> {
        self.register_web_font_with_variant(family, FontWeight::default(), FontStyle::default(), data)
    }

    /// Register a web font with explicit weight and style descriptors.
    ///
    /// Allows the same family to have multiple variants loaded side-by-side.
    /// The weight/style values match those declared in the `@font-face` rule.
    pub fn register_web_font_with_variant(
        &mut self,
        family: &str,
        weight: FontWeight,
        style: FontStyle,
        data: Vec<u8>,
    ) -> Result<Arc<Font>, FontError> {
        let key = (family.to_lowercase(), FontVariantKey::new(weight, style));
        // Evict an arbitrary entry if at capacity and this variant is not already cached
        if !self.fonts.contains_key(&key) && self.fonts.len() >= self.max_entries
            && let Some(oldest_key) = self.fonts.keys().next().cloned() {
                self.fonts.remove(&oldest_key);
            }
        let font = Arc::new(Font::load_from_bytes(data)?);
        self.fonts.insert(key, Arc::clone(&font));
        Ok(font)
    }

    /// Select the best available variant for the given family, target weight, and style.
    ///
    /// Implements a simplified version of CSS Fonts Module Level 3 §5.2:
    /// 1. If an exact match exists, return it.
    /// 2. Style matching: italic/oblique are interchangeable; normal is separate.
    /// 3. Weight matching: for bold requests (≥600) prefer heavier variants first;
    ///    for light requests (≤400) prefer lighter variants first.
    ///
    /// Returns `None` when the family has no registered variants at all.
    pub fn select_best_variant(
        &self,
        family: &str,
        target_weight: FontWeight,
        target_style: FontStyle,
    ) -> Option<Arc<Font>> {
        let family_lower = family.to_lowercase();

        // Style-matching pass: prefer matching style, then compatible style
        let style_score = |s: FontStyle| -> u8 {
            match (target_style, s) {
                (a, b) if a == b => 0,
                // italic <-> oblique are close
                (FontStyle::Italic, FontStyle::Oblique)
                | (FontStyle::Oblique, FontStyle::Italic) => 1,
                _ => 2,
            }
        };

        // Weight-matching score: lower is better
        let weight_score = |w: FontWeight| -> u16 {
            let tw = target_weight.0;
            let cw = w.0;
            if cw >= tw {
                // CSS: for weights ≥600, prefer upward first; for ≤400, prefer downward first
                if tw >= 600 {
                    cw - tw // prefer the smallest amount above target
                } else {
                    // downward preferred for thin/normal; upward is distant
                    (cw - tw).saturating_add(500)
                }
            } else {
                // cw < tw
                if tw <= 400 {
                    tw - cw // prefer closest below for thin weights
                } else {
                    (tw - cw).saturating_add(500)
                }
            }
        };

        // Iterate directly without collecting — clone only the winning Arc.
        self.fonts
            .iter()
            .filter(|((fam, _), _)| fam == &family_lower)
            .min_by_key(|((_, vk), _)| {
                let ss = style_score(vk.style) as u32 * 100_000;
                let ws = weight_score(vk.weight) as u32;
                ss + ws
            })
            .map(|(_, font)| Arc::clone(font))
    }

    /// Check if a font for the given family is already cached (any variant).
    pub fn contains(&self, family: &str) -> bool {
        let family_lower = family.to_lowercase();
        self.fonts.keys().any(|(fam, _)| fam == &family_lower)
    }

    /// Clear all cached fonts.
    pub fn clear(&mut self) {
        self.fonts.clear();
    }

    /// Get the number of cached font variants.
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
        Self::new(20) // Default to 20 font variants
    }
}

// ============================================================================
// Web Font Registry
// ============================================================================

/// Registry of web fonts loaded from `@font-face` rules.
///
/// Stores multiple variants (weight + style) per font family, and exposes
/// `select_best` to pick the closest variant for a requested weight/style pair
/// using the CSS Fonts Module Level 3 §5.2 matching algorithm.
///
/// Use `WebFontRegistry::push` to add variants after loading, then pass a reference
/// to the paint stage for per-fragment font selection.
#[derive(Default)]
pub struct WebFontRegistry {
    /// `family_lowercase -> Vec<(key, font)>`
    entries: HashMap<FontFamilyKey, Vec<(FontVariantKey, Arc<Font>)>>,
}

impl WebFontRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a font variant to the registry.
    pub fn push(&mut self, family: &str, weight: FontWeight, style: FontStyle, font: Font) {
        self.push_shared(family, weight, style, Arc::new(font));
    }

    /// Add a shared font variant without duplicating its underlying font data.
    pub fn push_shared(
        &mut self,
        family: &str,
        weight: FontWeight,
        style: FontStyle,
        font: Arc<Font>,
    ) {
        let key = FontVariantKey::new(weight, style);
        self.entries
            .entry(FontFamilyKey::new(family))
            .or_default()
            .push((key, font));
    }

    /// Select the best available font for the given family, weight, and style.
    ///
    /// Returns `None` when no variant for the family is registered.
    pub fn select_best(
        &self,
        family: &str,
        target_weight: FontWeight,
        target_style: FontStyle,
    ) -> Option<&Font> {
        self.select_best_by_key(FontFamilyKey::new(family), target_weight, target_style)
    }

    /// Select the best variant using a precomputed family key.
    pub fn select_best_by_key(
        &self,
        family: FontFamilyKey,
        target_weight: FontWeight,
        target_style: FontStyle,
    ) -> Option<&Font> {
        let variants = self.entries.get(&family)?;
        if variants.is_empty() {
            return None;
        }

        // Exact match shortcut
        if let Some((_, font)) = variants
            .iter()
            .find(|(k, _)| k.weight == target_weight && k.style == target_style)
        {
            return Some(font.as_ref());
        }

        // Scoring: style score dominates, weight score breaks ties
        let style_score = |s: FontStyle| -> u8 {
            match (target_style, s) {
                (a, b) if a == b => 0,
                (FontStyle::Italic, FontStyle::Oblique)
                | (FontStyle::Oblique, FontStyle::Italic) => 1,
                _ => 2,
            }
        };

        let weight_score = |w: FontWeight| -> u16 {
            let tw = target_weight.0;
            let cw = w.0;
            if cw >= tw {
                if tw >= 600 {
                    cw - tw
                } else {
                    (cw - tw).saturating_add(500)
                }
            } else {
                // cw < tw
                if tw <= 400 {
                    tw - cw
                } else {
                    (tw - cw).saturating_add(500)
                }
            }
        };

        variants
            .iter()
            .min_by_key(|(k, _)| {
                (style_score(k.style) as u32) * 100_000 + weight_score(k.weight) as u32
            })
            .map(|(_, font)| font.as_ref())
    }

    /// Returns `true` when any variant for the family is registered.
    pub fn contains_family(&self, family: &str) -> bool {
        self.entries.contains_key(&FontFamilyKey::new(family))
    }

    /// Returns `true` when no fonts have been registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns an iterator over all registered (family, variant_key, font) tuples.
    ///
    /// Useful for building a flat `Vec<Font>` for APIs that do not support registries.
    pub fn iter_fonts(&self) -> impl Iterator<Item = &Font> {
        self.entries
            .values()
            .flat_map(|variants| variants.iter().map(|(_, font)| font.as_ref()))
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
        if self.glyphs.len() >= self.max_entries
            && let Some(oldest_key) = self.glyphs.keys().next().cloned() {
                self.glyphs.remove(&oldest_key);
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
            if self.glyphs.len() >= self.max_entries
                && let Some(oldest_key) = self.glyphs.keys().next().cloned() {
                    self.glyphs.remove(&oldest_key);
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
