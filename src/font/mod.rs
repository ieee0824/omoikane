//! Font loading and glyph rendering module.
//!
//! Provides TrueType/OpenType font support using the `ab_glyph` crate.
//! Handles font file loading, character-to-glyph mapping, and rasterization.

use ab_glyph::{Font as AbGlyphFont, FontVec, ScaleFont};
use std::path::Path;
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
