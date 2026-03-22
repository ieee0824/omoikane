//! Pixel-based painting primitives and layout tree rendering.

use std::collections::HashSet;
use std::fs;
use std::io::Cursor;
use std::io::Read;
use std::path::Path;

use crate::css::{
    ComputedStyle, ComputedValue, Origin, PseudoElement, StyleResolver, Stylesheet,
    parse_stylesheet,
};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::font::{Font, GlyphRaster, load_default_text_fonts};
use crate::http::url::resolve_url;
use crate::layout::{InlineFragmentContent, LayoutBox, Rect, Visibility};
use base64::Engine;
use flate2::read::ZlibDecoder;
/// An RGBA color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// A decoded RGBA image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Image {
    /// Creates an image from raw RGBA pixels.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, PaintError> {
        if pixels.len() != width as usize * height as usize * 4 {
            return Err(PaintError::InvalidImageBuffer);
        }

        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// Decodes a PNG image into RGBA pixels.
    pub fn decode_png(bytes: &[u8]) -> Result<Self, PaintError> {
        decode_png(bytes)
    }

    /// Decodes a JPEG image into RGBA pixels.
    pub fn decode_jpeg(bytes: &[u8]) -> Result<Self, PaintError> {
        decode_jpeg(bytes)
    }

    /// Returns the image width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Returns the image height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Returns the image pixels in RGBA order.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

/// Errors returned by the paint module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaintError {
    InvalidImageBuffer,
    InvalidDataUri,
    InvalidBase64,
    InvalidStylesheet,
    InvalidPngSignature,
    MissingPngHeader,
    UnsupportedPngFormat,
    CorruptPng,
    DecompressionFailed,
    InvalidJpeg,
    UnsupportedJpegFormat,
}

/// Parsed contents of a `data:` URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataUri {
    Text { mime_type: String, data: String },
    Binary { mime_type: String, data: Vec<u8> },
}

impl Color {
    /// Creates an opaque color.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Creates a color with an explicit alpha channel.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

/// A simple RGBA bitmap canvas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    /// Creates a transparent canvas with the given dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width as usize * height as usize * 4],
        }
    }

    /// Returns the canvas width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Returns the canvas height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Returns the raw RGBA pixels.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Returns the pixel color at `(x, y)`, if in bounds.
    pub fn pixel(&self, x: u32, y: u32) -> Option<Color> {
        if x >= self.width || y >= self.height {
            return None;
        }

        let index = ((y * self.width + x) * 4) as usize;
        Some(Color {
            r: self.pixels[index],
            g: self.pixels[index + 1],
            b: self.pixels[index + 2],
            a: self.pixels[index + 3],
        })
    }

    /// Fills a rectangle, alpha-blending it over the existing pixels.
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.fill_rect_clipped(rect, color, None);
    }

    /// Encodes the canvas as a PNG image.
    pub fn encode_png(&self) -> Vec<u8> {
        let mut png = Vec::new();
        png.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&self.width.to_be_bytes());
        ihdr.extend_from_slice(&self.height.to_be_bytes());
        ihdr.push(8);
        ihdr.push(6);
        ihdr.push(0);
        ihdr.push(0);
        ihdr.push(0);
        write_chunk(&mut png, b"IHDR", &ihdr);

        let mut raw = Vec::with_capacity(self.pixels.len() + self.height as usize);
        let stride = self.width as usize * 4;
        for row in 0..self.height as usize {
            raw.push(0);
            let start = row * stride;
            raw.extend_from_slice(&self.pixels[start..start + stride]);
        }
        let compressed = zlib_compress_uncompressed(&raw);
        write_chunk(&mut png, b"IDAT", &compressed);
        write_chunk(&mut png, b"IEND", &[]);
        png
    }

    /// Draws an RGBA image at the given destination origin.
    pub fn draw_image(&mut self, image: &Image, x: f32, y: f32) {
        self.draw_image_clipped(image, x, y, None);
    }

    fn fill_rect_clipped(&mut self, rect: Rect, color: Color, clip: Option<Rect>) {
        if color.a == 0 {
            return;
        }

        let Some(mut area) = normalize_rect(rect) else {
            return;
        };

        if let Some(clip_rect) = clip.and_then(normalize_rect) {
            let Some(intersection) = intersect(area, clip_rect) else {
                return;
            };
            area = intersection;
        }

        let x0 = area.x.floor().max(0.0) as i32;
        let y0 = area.y.floor().max(0.0) as i32;
        let x1 = (area.x + area.width).ceil().min(self.width as f32) as i32;
        let y1 = (area.y + area.height).ceil().min(self.height as f32) as i32;

        for y in y0..y1 {
            for x in x0..x1 {
                let index = ((y as u32 * self.width + x as u32) * 4) as usize;
                blend_pixel(&mut self.pixels[index..index + 4], color);
            }
        }
    }

    fn draw_image_clipped(&mut self, image: &Image, x: f32, y: f32, clip: Option<Rect>) {
        self.draw_image_scaled_clipped(
            image,
            Rect {
                x,
                y,
                width: image.width as f32,
                height: image.height as f32,
            },
            clip,
        );
    }

    fn draw_image_scaled_clipped(&mut self, image: &Image, destination: Rect, clip: Option<Rect>) {
        let Some(destination) = normalize_rect(destination) else {
            return;
        };
        if destination.width <= 0.0 || destination.height <= 0.0 {
            return;
        };
        let mut area = destination;

        if let Some(clip_rect) = clip.and_then(normalize_rect) {
            let Some(intersection) = intersect(area, clip_rect) else {
                return;
            };
            area = intersection;
        }

        let x0 = area.x.floor().max(0.0) as i32;
        let y0 = area.y.floor().max(0.0) as i32;
        let x1 = (area.x + area.width).ceil().min(self.width as f32) as i32;
        let y1 = (area.y + area.height).ceil().min(self.height as f32) as i32;

        for dest_y in y0..y1 {
            for dest_x in x0..x1 {
                let u = (dest_x as f32 - destination.x) / destination.width;
                let v = (dest_y as f32 - destination.y) / destination.height;
                let source_x = (u * image.width as f32).floor() as i32;
                let source_y = (v * image.height as f32).floor() as i32;
                if source_x < 0
                    || source_y < 0
                    || source_x >= image.width as i32
                    || source_y >= image.height as i32
                {
                    continue;
                }

                let source_index = ((source_y as u32 * image.width + source_x as u32) * 4) as usize;
                let color = Color {
                    r: image.pixels[source_index],
                    g: image.pixels[source_index + 1],
                    b: image.pixels[source_index + 2],
                    a: image.pixels[source_index + 3],
                };
                let dest_index = ((dest_y as u32 * self.width + dest_x as u32) * 4) as usize;
                blend_pixel(&mut self.pixels[dest_index..dest_index + 4], color);
            }
        }
    }

    /// Draws a glyph alpha mask at the given position with the specified color.
    ///
    /// The `mask` is a single-channel alpha bitmap (one u8 per pixel, row-major order).
    /// Each mask value is multiplied with the color's alpha to produce the final alpha.
    fn draw_glyph_mask(
        &mut self,
        x: f32,
        y: f32,
        mask_width: u32,
        mask_height: u32,
        mask: &[u8],
        color: Color,
        clip: Option<Rect>,
    ) {
        if mask_width == 0 || mask_height == 0 || mask.is_empty() {
            return;
        }

        let destination = Rect {
            x,
            y,
            width: mask_width as f32,
            height: mask_height as f32,
        };
        let Some(mut area) = normalize_rect(destination) else {
            return;
        };

        if let Some(clip_rect) = clip.and_then(normalize_rect) {
            let Some(intersection) = intersect(area, clip_rect) else {
                return;
            };
            area = intersection;
        }

        let x0 = area.x.floor().max(0.0) as i32;
        let y0 = area.y.floor().max(0.0) as i32;
        let x1 = (area.x + area.width).ceil().min(self.width as f32) as i32;
        let y1 = (area.y + area.height).ceil().min(self.height as f32) as i32;

        for dest_y in y0..y1 {
            for dest_x in x0..x1 {
                let mask_x = (dest_x as f32 - x).floor() as i32;
                let mask_y = (dest_y as f32 - y).floor() as i32;
                if mask_x < 0
                    || mask_y < 0
                    || mask_x >= mask_width as i32
                    || mask_y >= mask_height as i32
                {
                    continue;
                }

                let mask_index = (mask_y as u32 * mask_width + mask_x as u32) as usize;
                let mask_alpha = mask.get(mask_index).copied().unwrap_or(0);
                if mask_alpha == 0 {
                    continue;
                }

                // Combine mask alpha with color alpha
                let combined_alpha = ((color.a as u32 * mask_alpha as u32 + 127) / 255) as u8;
                let glyph_color = Color {
                    r: color.r,
                    g: color.g,
                    b: color.b,
                    a: combined_alpha,
                };

                let dest_index = ((dest_y as u32 * self.width + dest_x as u32) * 4) as usize;
                blend_pixel(&mut self.pixels[dest_index..dest_index + 4], glyph_color);
            }
        }
    }
}

/// Paints a layout tree into a new canvas using the provided viewport size.
pub fn paint_layout(layout: &LayoutBox, resolver: &mut StyleResolver, viewport: Rect) -> Canvas {
    let width = viewport.width.ceil().max(1.0) as u32;
    let height = viewport.height.ceil().max(1.0) as u32;
    let mut canvas = Canvas::new(width, height);
    if let Some(background) = viewport_background_color(layout, resolver) {
        canvas.fill_rect(viewport, background);
    }
    let text_fonts = load_text_fonts();
    paint_box(&mut canvas, layout, resolver, None, viewport, &text_fonts);
    canvas
}

/// Renders a DOM document into a canvas using inline and linked author stylesheets.
pub fn render_document(document: &NodeHandle, viewport: Rect) -> Result<Canvas, PaintError> {
    render_document_with_url(document, viewport, None)
}

/// Renders a DOM document into a canvas, fetching external stylesheets relative to `base_url`.
pub fn render_document_with_url(
    document: &NodeHandle,
    viewport: Rect,
    base_url: Option<&crate::http::Url>,
) -> Result<Canvas, PaintError> {
    let effective_base = extract_document_base_url(document, base_url);
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(document, base_url)? {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet)?);
    }
    crate::layout::with_image_base_url(effective_base, || {
        let layout = crate::layout::layout_tree(document, &mut resolver, viewport)?;
        Some(paint_layout(&layout, &mut resolver, viewport))
    })
    .ok_or(PaintError::InvalidImageBuffer)
}

/// Encodes the rendered document directly as PNG.
pub fn render_document_png(document: &NodeHandle, viewport: Rect) -> Result<Vec<u8>, PaintError> {
    Ok(render_document(document, viewport)?.encode_png())
}

/// Encodes the rendered document as PNG, fetching external stylesheets relative to `base_url`.
pub fn render_document_png_with_url(
    document: &NodeHandle,
    viewport: Rect,
    base_url: Option<&crate::http::Url>,
) -> Result<Vec<u8>, PaintError> {
    Ok(render_document_with_url(document, viewport, base_url)?.encode_png())
}

/// Renders a DOM document into a canvas, resolving local fixture assets from `base_path`.
pub fn render_document_with_base_path(
    document: &NodeHandle,
    viewport: Rect,
    base_path: &Path,
) -> Result<Canvas, PaintError> {
    materialize_local_assets(document, base_path)?;
    render_document(document, viewport)
}

fn materialize_local_assets(node: &NodeHandle, base_path: &Path) -> Result<(), PaintError> {
    if node.node_type() == NodeType::Element {
        match node.tag_name().as_deref() {
            Some("img") => rewrite_local_asset_attribute(node, "src", base_path)?,
            Some("link") => rewrite_local_asset_attribute(node, "href", base_path)?,
            _ => {}
        }
    }

    for child in node.child_nodes() {
        materialize_local_assets(&child, base_path)?;
    }

    Ok(())
}

fn rewrite_local_asset_attribute(
    node: &NodeHandle,
    attribute_name: &str,
    base_path: &Path,
) -> Result<(), PaintError> {
    let attributes = node.attributes().unwrap_or_default();
    let Some(value) = attributes.get(attribute_name) else {
        return Ok(());
    };
    if value.is_empty()
        || value.starts_with("data:")
        || value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with('#')
        || value.contains(':')
    {
        return Ok(());
    }

    let asset_path = base_path.join(value);
    if !asset_path.is_file() {
        return Ok(());
    }

    let mime_type = match asset_path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("png") => "image/png",
        Some(ext) if ext.eq_ignore_ascii_case("css") => "text/css",
        _ => return Ok(()),
    };

    let data = fs::read(asset_path).map_err(|_| PaintError::InvalidDataUri)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(data);
    node.set_attribute(attribute_name, format!("data:{mime_type};base64,{encoded}"));

    Ok(())
}

/// Computes a per-pixel diff image and count between two canvases.
pub fn diff_canvases(actual: &Canvas, expected: &Canvas) -> (Canvas, usize) {
    diff_canvases_with_tolerance(actual, expected, 0)
}

/// Same as [`diff_canvases`] but allows a per-channel tolerance when comparing pixels.
/// A tolerance of 1 treats pixels that differ by at most 1 on every channel as matching.
pub fn diff_canvases_with_tolerance(
    actual: &Canvas,
    expected: &Canvas,
    tolerance: u8,
) -> (Canvas, usize) {
    let width = actual.width().max(expected.width());
    let height = actual.height().max(expected.height());
    let mut diff = Canvas::new(width, height);
    let mut changed = 0usize;

    for y in 0..height {
        for x in 0..width {
            let left = actual.pixel(x, y).unwrap_or(Color::rgba(0, 0, 0, 0));
            let right = expected.pixel(x, y).unwrap_or(Color::rgba(0, 0, 0, 0));
            let within_tolerance = (left.r as i16 - right.r as i16).unsigned_abs()
                <= tolerance as u16
                && (left.g as i16 - right.g as i16).unsigned_abs() <= tolerance as u16
                && (left.b as i16 - right.b as i16).unsigned_abs() <= tolerance as u16
                && (left.a as i16 - right.a as i16).unsigned_abs() <= tolerance as u16;
            let color = if within_tolerance {
                Color::rgba(0, 0, 0, 0)
            } else {
                changed += 1;
                Color::rgb(255, 0, 255)
            };
            diff.fill_rect(
                Rect {
                    x: x as f32,
                    y: y as f32,
                    width: 1.0,
                    height: 1.0,
                },
                color,
            );
        }
    }

    (diff, changed)
}

fn paint_box(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    inherited_clip: Option<Rect>,
    viewport: Rect,
    text_fonts: &[Font],
) {
    paint_box_internal(
        canvas,
        layout,
        resolver,
        inherited_clip,
        viewport,
        true,
        text_fonts,
    );
}

fn paint_box_internal(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    inherited_clip: Option<Rect>,
    viewport: Rect,
    include_phase_descendants: bool,
    text_fonts: &[Font],
) {
    if layout.visibility == Visibility::Hidden {
        return;
    }

    let style = resolver.computed_style(&layout.node);
    let border_box = border_box_rect(layout);
    let padding_box = padding_box_rect(layout);

    if let Some(background) = background_color(&style) {
        canvas.fill_rect_clipped(border_box, background, inherited_clip);
    }
    paint_background_image(canvas, &style, border_box, inherited_clip, viewport);
    paint_block_generated_pseudo_box(
        canvas,
        layout,
        resolver,
        PseudoElement::Before,
        inherited_clip,
        viewport,
    );

    paint_borders(canvas, layout, &style, inherited_clip);

    let clip = if layout.overflow == crate::layout::Overflow::Hidden {
        match inherited_clip {
            Some(current) => intersect(current, padding_box),
            None => Some(padding_box),
        }
    } else {
        inherited_clip
    };

    let mut negative_positioned_children = Vec::new();
    let mut normal_block_children = Vec::new();
    let mut float_children = Vec::new();
    let mut inline_children = Vec::new();
    let mut auto_positioned_children = Vec::new();
    let mut positive_positioned_children = Vec::new();
    for child in &layout.children {
        let child_style = resolver.computed_style(&child.node);
        if is_positioned_for_paint(&child_style) {
            if include_phase_descendants {
                if child.z_index < 0 {
                    negative_positioned_children.push(child);
                } else if child.z_index > 0 {
                    positive_positioned_children.push(child);
                } else {
                    auto_positioned_children.push(child);
                }
            }
            continue;
        }

        if is_float_for_paint(&child_style) {
            if include_phase_descendants {
                float_children.push(child);
            }
            continue;
        }

        if include_phase_descendants {
            collect_phase_descendants(
                child,
                resolver,
                &mut float_children,
                &mut negative_positioned_children,
                &mut auto_positioned_children,
                &mut positive_positioned_children,
            );
        }

        if child.lines.is_empty() {
            normal_block_children.push(child);
        } else {
            inline_children.push(child);
        }
    }

    negative_positioned_children.sort_by_key(|child| child.z_index);
    positive_positioned_children.sort_by_key(|child| child.z_index);

    for child in negative_positioned_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, true, text_fonts);
    }
    for child in normal_block_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, false, text_fonts);
    }
    for child in float_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, true, text_fonts);
    }
    paint_text(canvas, layout, &style, clip, viewport, text_fonts);
    for child in inline_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, false, text_fonts);
    }
    for child in auto_positioned_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, true, text_fonts);
    }
    for child in positive_positioned_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, true, text_fonts);
    }

    paint_block_generated_pseudo_box(
        canvas,
        layout,
        resolver,
        PseudoElement::After,
        clip,
        viewport,
    );
}

fn collect_phase_descendants<'a>(
    layout: &'a LayoutBox,
    resolver: &mut StyleResolver,
    float_children: &mut Vec<&'a LayoutBox>,
    negative_positioned_children: &mut Vec<&'a LayoutBox>,
    auto_positioned_children: &mut Vec<&'a LayoutBox>,
    positive_positioned_children: &mut Vec<&'a LayoutBox>,
) {
    for child in &layout.children {
        let child_style = resolver.computed_style(&child.node);
        if is_positioned_for_paint(&child_style) {
            if child.z_index < 0 {
                negative_positioned_children.push(child);
            } else if child.z_index > 0 {
                positive_positioned_children.push(child);
            } else {
                auto_positioned_children.push(child);
            }
            continue;
        }

        if is_float_for_paint(&child_style) {
            float_children.push(child);
            continue;
        }

        collect_phase_descendants(
            child,
            resolver,
            float_children,
            negative_positioned_children,
            auto_positioned_children,
            positive_positioned_children,
        );
    }
}

fn is_float_for_paint(style: &ComputedStyle) -> bool {
    matches!(
        style.get("float"),
        Some(ComputedValue::Keyword(keyword))
            if keyword.eq_ignore_ascii_case("left") || keyword.eq_ignore_ascii_case("right")
    )
}

fn is_positioned_for_paint(style: &ComputedStyle) -> bool {
    matches!(
        style.get("position"),
        Some(ComputedValue::Keyword(keyword))
            if keyword.eq_ignore_ascii_case("absolute")
                || keyword.eq_ignore_ascii_case("fixed")
                || keyword.eq_ignore_ascii_case("relative")
    )
}

fn viewport_background_color(layout: &LayoutBox, resolver: &mut StyleResolver) -> Option<Color> {
    if layout.node.node_type() != NodeType::Document {
        return None;
    }

    let root = layout
        .children
        .iter()
        .find(|child| child.node.tag_name().as_deref() == Some("html"))
        .or_else(|| layout.children.first())?;
    let root_style = resolver.computed_style(&root.node);
    if let Some(color) = background_color(&root_style) {
        return Some(color);
    }

    let body = root
        .children
        .iter()
        .find(|child| child.node.tag_name().as_deref() == Some("body"))?;
    let body_style = resolver.computed_style(&body.node);
    background_color(&body_style)
}

fn paint_borders(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    style: &ComputedStyle,
    clip: Option<Rect>,
) {
    if !has_any_solid_border(style) {
        return;
    }
    let border_box = border_box_rect(layout);
    let padding_box = padding_box_rect(layout);
    let border = layout.dimensions.border;

    if border.top > 0.0 && has_solid_border_side(style, "top") {
        canvas.fill_rect_clipped(
            Rect {
                x: border_box.x,
                y: border_box.y,
                width: border_box.width,
                height: border.top,
            },
            border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.bottom > 0.0 && has_solid_border_side(style, "bottom") {
        canvas.fill_rect_clipped(
            Rect {
                x: border_box.x,
                y: padding_box.y + padding_box.height,
                width: border_box.width,
                height: border.bottom,
            },
            border_color_side(style, "bottom").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.left > 0.0 && has_solid_border_side(style, "left") {
        canvas.fill_rect_clipped(
            Rect {
                x: border_box.x,
                y: border_box.y + border.top,
                width: border.left,
                height: border_box.height - border.top - border.bottom,
            },
            border_color_side(style, "left").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.right > 0.0 && has_solid_border_side(style, "right") {
        canvas.fill_rect_clipped(
            Rect {
                x: padding_box.x + padding_box.width,
                y: border_box.y + border.top,
                width: border.right,
                height: border_box.height - border.top - border.bottom,
            },
            border_color_side(style, "right").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
}

fn paint_text(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    style: &ComputedStyle,
    clip: Option<Rect>,
    _viewport: Rect,
    fonts: &[Font],
) {
    let color = text_color(style).unwrap_or(Color::rgb(0, 0, 0));
    let text_transform = text_transform_value(style);
    let decoration_line = text_decoration_line(style);
    let decoration_color = text_decoration_color(style, color);

    for line in &layout.lines {
        for fragment in &line.fragments {
            match &fragment.content {
                InlineFragmentContent::Text(text) => {
                    let font_size = fragment.metrics.font_size.max(1.0);
                    let transformed = apply_text_transform(text, text_transform);
                    let display_text = transformed.as_deref().unwrap_or(text.as_str());

                    if !fonts.is_empty() {
                        paint_text_with_font(
                            canvas,
                            fragment.rect,
                            display_text,
                            font_size,
                            fragment.metrics.ascent,
                            &fonts,
                            color,
                            clip,
                        );
                    } else {
                        // Fallback: placeholder rectangles
                        paint_text_placeholder(canvas, fragment.rect, display_text, font_size, color, clip);
                    }

                    // Draw text decorations after text
                    paint_text_decoration(
                        canvas,
                        fragment.rect,
                        fragment.metrics.ascent,
                        fragment.metrics.descent,
                        font_size,
                        decoration_line,
                        decoration_color,
                        clip,
                    );
                }
                InlineFragmentContent::Image(image, style) => {
                    paint_inline_image_fragment(
                        canvas,
                        fragment.rect,
                        image,
                        style,
                        clip,
                        _viewport,
                    );
                }
                InlineFragmentContent::GeneratedBox(style) => {
                    paint_generated_box(canvas, fragment.rect, style, clip, _viewport);
                }
            }
        }
    }
}

/// Returns the `text-transform` value from style.
fn text_transform_value(style: &ComputedStyle) -> &'static str {
    match style.get("text-transform") {
        Some(ComputedValue::Keyword(kw)) => match kw.to_ascii_lowercase().as_str() {
            "uppercase" => "uppercase",
            "lowercase" => "lowercase",
            "capitalize" => "capitalize",
            _ => "none",
        },
        _ => "none",
    }
}

/// Apply text-transform to the given text, returning Some(transformed) or None if no change.
fn apply_text_transform(text: &str, transform: &str) -> Option<String> {
    match transform {
        "uppercase" => Some(text.to_uppercase()),
        "lowercase" => Some(text.to_lowercase()),
        "capitalize" => {
            let mut result = String::with_capacity(text.len());
            let mut capitalize_next = true;
            for ch in text.chars() {
                if ch.is_whitespace() {
                    capitalize_next = true;
                    result.push(ch);
                } else if capitalize_next {
                    for c in ch.to_uppercase() {
                        result.push(c);
                    }
                    capitalize_next = false;
                } else {
                    result.push(ch);
                }
            }
            Some(result)
        }
        _ => None,
    }
}

/// Text decoration line kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextDecorationLine {
    None,
    Underline,
    Overline,
    LineThrough,
}

/// Returns the text-decoration-line value from style.
fn text_decoration_line(style: &ComputedStyle) -> TextDecorationLine {
    match style.get("text-decoration-line") {
        Some(ComputedValue::Keyword(kw)) => match kw.to_ascii_lowercase().as_str() {
            "underline" => TextDecorationLine::Underline,
            "overline" => TextDecorationLine::Overline,
            "line-through" => TextDecorationLine::LineThrough,
            _ => TextDecorationLine::None,
        },
        _ => TextDecorationLine::None,
    }
}

/// Returns the text-decoration-color, falling back to the text color.
fn text_decoration_color(style: &ComputedStyle, fallback: Color) -> Color {
    match style.get("text-decoration-color") {
        Some(ComputedValue::Color(c)) => parse_color(c).unwrap_or(fallback),
        Some(ComputedValue::Keyword(c)) => parse_color(c).unwrap_or(fallback),
        _ => fallback,
    }
}

/// Draw text decoration lines (underline, overline, line-through) for a fragment.
fn paint_text_decoration(
    canvas: &mut Canvas,
    rect: Rect,
    ascent: f32,
    descent: f32,
    font_size: f32,
    decoration: TextDecorationLine,
    color: Color,
    clip: Option<Rect>,
) {
    if decoration == TextDecorationLine::None {
        return;
    }

    let line_thickness = (font_size * 0.075).max(1.0);

    let line_y = match decoration {
        TextDecorationLine::Underline => rect.y + ascent + descent * 0.5,
        TextDecorationLine::Overline => rect.y,
        TextDecorationLine::LineThrough => rect.y + ascent * 0.6,
        TextDecorationLine::None => return,
    };

    let line_rect = Rect {
        x: rect.x,
        y: line_y,
        width: rect.width,
        height: line_thickness,
    };

    canvas.fill_rect_clipped(line_rect, color, clip);
}

/// Paint text using actual font glyphs.
fn paint_text_with_font(
    canvas: &mut Canvas,
    rect: Rect,
    text: &str,
    font_size: f32,
    layout_ascent: f32,
    fonts: &[Font],
    color: Color,
    clip: Option<Rect>,
) {
    // Align paint baseline with layout's line-box baseline model to avoid vertical drift.
    let baseline_y = rect.y + layout_ascent;
    let mut cursor_x = rect.x;
    let mut previous_char: Option<(char, usize)> = None;

    for ch in text.chars() {
        let (font_index, glyph, advance_x) = rasterize_with_fallback(fonts, ch, font_size);
        if let Some((prev, prev_font_index)) = previous_char
            && prev_font_index == font_index
        {
            cursor_x += fonts[font_index].glyph_kerning(prev, ch, font_size);
        }

        if let Some(glyph) = glyph {
            if glyph.width > 0 && glyph.height > 0 && !glyph.bitmap.is_empty() {
                // Calculate glyph position
                // offset_y is from baseline to bitmap top (typically negative for glyphs above baseline)
                let glyph_x = cursor_x + glyph.offset_x;
                let glyph_y = baseline_y + glyph.offset_y;

                canvas.draw_glyph_mask(
                    glyph_x,
                    glyph_y,
                    glyph.width,
                    glyph.height,
                    &glyph.bitmap,
                    color,
                    clip,
                );
            }
        }

        // Keep text flow moving even if a glyph cannot be rasterized by any candidate font.
        cursor_x += advance_x;
        previous_char = Some((ch, font_index));
    }
}

fn load_text_fonts() -> Vec<Font> {
    load_default_text_fonts()
}

fn rasterize_with_fallback(
    fonts: &[Font],
    ch: char,
    font_size: f32,
) -> (usize, Option<GlyphRaster>, f32) {
    let prefer_cjk = is_cjk_preferred_character(ch);
    let try_index = |index: usize| -> Option<(usize, Option<GlyphRaster>, f32)> {
        let font = &fonts[index];
        // Allow primary font to render .notdef as a visible last resort.
        if index != 0 && !ch.is_whitespace() && !font.has_glyph(ch) {
            return None;
        }
        match font.rasterize(ch, font_size) {
            Ok(glyph) => {
                if glyph.width > 0 && glyph.height > 0 && !glyph.bitmap.is_empty() {
                    let advance = if glyph.advance_x > 0.0 {
                        glyph.advance_x
                    } else {
                        font.glyph_advance(ch, font_size)
                    };
                    return Some((index, Some(glyph), advance));
                }

                // Whitespace and control-like glyphs can be outline-less but still have advance.
                if ch.is_whitespace() {
                    return Some((index, None, font.glyph_advance(ch, font_size)));
                }
            }
            Err(_) => return None,
        }
        None
    };

    if prefer_cjk && fonts.len() > 1 {
        for index in 1..fonts.len() {
            if let Some(result) = try_index(index) {
                return result;
            }
        }
        if let Some(result) = try_index(0) {
            return result;
        }
    } else {
        for index in 0..fonts.len() {
            if let Some(result) = try_index(index) {
                return result;
            }
        }
    }

    // Fallback to the primary font advance to avoid collapsing text runs.
    let primary_advance = fonts
        .first()
        .map(|font| font.glyph_advance(ch, font_size))
        .unwrap_or((font_size * 0.6).max(1.0));
    (0, None, primary_advance)
}

fn is_cjk_preferred_character(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3000..=0x30FF // CJK Symbols/Punctuation, Hiragana, Katakana
            | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
            | 0xFF66..=0xFF9F // Half-width Katakana
    )
}

/// Paint text as placeholder rectangles (fallback when no font available).
fn paint_text_placeholder(
    canvas: &mut Canvas,
    rect: Rect,
    text: &str,
    font_size: f32,
    color: Color,
    clip: Option<Rect>,
) {
    let mut cursor_x = rect.x;
    let advance = (font_size * 0.6).max(1.0); // Approximate advance
    let glyph_height = (font_size * 0.7).max(1.0);
    let glyph_y = rect.y + (font_size - glyph_height) * 0.5;

    for ch in text.chars() {
        if !ch.is_whitespace() {
            canvas.fill_rect_clipped(
                Rect {
                    x: cursor_x,
                    y: glyph_y,
                    width: (advance * 0.7).max(1.0),
                    height: glyph_height,
                },
                color,
                clip,
            );
        }
        cursor_x += advance;
    }
}

fn paint_inline_image_fragment(
    canvas: &mut Canvas,
    rect: Rect,
    image: &Image,
    style: &ComputedStyle,
    clip: Option<Rect>,
    viewport: Rect,
) {
    if let Some(background) = background_color(style) {
        canvas.fill_rect_clipped(rect, background, clip);
    }
    paint_background_image(canvas, style, rect, clip, viewport);

    let border = EdgeSizesForPaint::from_style(style);
    if border.total_horizontal() > 0.0 || border.total_vertical() > 0.0 {
        paint_rect_borders(canvas, rect, style, border, clip);
    }

    let content_rect = inline_fragment_content_rect(rect, style, border);
    canvas.draw_image_scaled_clipped(image, content_rect, clip);
}

fn inline_fragment_content_rect(
    rect: Rect,
    style: &ComputedStyle,
    border: EdgeSizesForPaint,
) -> Rect {
    let padding_left = length_property(style, "padding-left")
        .or_else(|| length_property(style, "padding"))
        .unwrap_or(0.0);
    let padding_right = length_property(style, "padding-right")
        .or_else(|| length_property(style, "padding"))
        .unwrap_or(0.0);
    let padding_top = length_property(style, "padding-top")
        .or_else(|| length_property(style, "padding"))
        .unwrap_or(0.0);
    let padding_bottom = length_property(style, "padding-bottom")
        .or_else(|| length_property(style, "padding"))
        .unwrap_or(0.0);

    Rect {
        x: rect.x + border.left + padding_left,
        y: rect.y + border.top + padding_top,
        width: (rect.width - border.left - border.right - padding_left - padding_right).max(0.0),
        height: (rect.height - border.top - border.bottom - padding_top - padding_bottom).max(0.0),
    }
}

fn paint_generated_box(
    canvas: &mut Canvas,
    rect: Rect,
    style: &ComputedStyle,
    clip: Option<Rect>,
    viewport: Rect,
) {
    if let Some(background) = background_color(style) {
        canvas.fill_rect_clipped(rect, background, clip);
    }

    paint_background_image(canvas, style, rect, clip, viewport);

    let border = EdgeSizesForPaint::from_style(style);
    if border.total_horizontal() == 0.0 && border.total_vertical() == 0.0 {
        return;
    }

    if rect.width == border.total_horizontal() && rect.height == border.total_vertical() {
        paint_zero_sized_border_box(canvas, rect, style, border, clip);
        return;
    }

    paint_rect_borders(canvas, rect, style, border, clip);
}

fn paint_block_generated_pseudo_box(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    pseudo: PseudoElement,
    clip: Option<Rect>,
    viewport: Rect,
) {
    let Some(style) = resolver.computed_pseudo_style(&layout.node, pseudo) else {
        return;
    };
    if !matches!(
        style.get("content"),
        Some(ComputedValue::String(content)) if content.is_empty()
    ) {
        return;
    }
    if !matches!(
        style.get("display"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("block")
    ) {
        return;
    }

    let border = EdgeSizesForPaint::from_style(&style);
    let padding_left = length_property(&style, "padding-left")
        .or_else(|| length_property(&style, "padding"))
        .unwrap_or(0.0);
    let padding_right = length_property(&style, "padding-right")
        .or_else(|| length_property(&style, "padding"))
        .unwrap_or(0.0);
    let padding_top = length_property(&style, "padding-top")
        .or_else(|| length_property(&style, "padding"))
        .unwrap_or(0.0);
    let padding_bottom = length_property(&style, "padding-bottom")
        .or_else(|| length_property(&style, "padding"))
        .unwrap_or(0.0);
    let content_width = length_property(&style, "width").unwrap_or(
        (layout.dimensions.content.width
            - padding_left
            - padding_right
            - border.left
            - border.right)
            .max(0.0),
    );
    let content_height = length_property(&style, "height").unwrap_or(0.0);
    let total_width = content_width + padding_left + padding_right + border.left + border.right;
    let total_height = content_height + padding_top + padding_bottom + border.top + border.bottom;
    if total_width <= 0.0 && total_height <= 0.0 {
        return;
    }

    let y = match pseudo {
        PseudoElement::Before => layout.dimensions.content.y,
        PseudoElement::After => {
            layout.dimensions.content.y + layout.dimensions.content.height - total_height
        }
    };
    paint_generated_box(
        canvas,
        Rect {
            x: layout.dimensions.content.x,
            y,
            width: total_width,
            height: total_height,
        },
        &style,
        clip,
        viewport,
    );
}

#[derive(Clone, Copy)]
struct EdgeSizesForPaint {
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
}

impl EdgeSizesForPaint {
    fn from_style(style: &ComputedStyle) -> Self {
        Self {
            top: length_property(style, "border-top-width")
                .or_else(|| length_property(style, "border-width"))
                .unwrap_or(0.0),
            right: length_property(style, "border-right-width")
                .or_else(|| length_property(style, "border-width"))
                .unwrap_or(0.0),
            bottom: length_property(style, "border-bottom-width")
                .or_else(|| length_property(style, "border-width"))
                .unwrap_or(0.0),
            left: length_property(style, "border-left-width")
                .or_else(|| length_property(style, "border-width"))
                .unwrap_or(0.0),
        }
    }

    fn total_horizontal(self) -> f32 {
        self.left + self.right
    }

    fn total_vertical(self) -> f32 {
        self.top + self.bottom
    }
}

fn paint_rect_borders(
    canvas: &mut Canvas,
    rect: Rect,
    style: &ComputedStyle,
    border: EdgeSizesForPaint,
    clip: Option<Rect>,
) {
    if border.top > 0.0 && has_solid_border_side(style, "top") {
        canvas.fill_rect_clipped(
            Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: border.top,
            },
            border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.bottom > 0.0 && has_solid_border_side(style, "bottom") {
        canvas.fill_rect_clipped(
            Rect {
                x: rect.x,
                y: rect.y + rect.height - border.bottom,
                width: rect.width,
                height: border.bottom,
            },
            border_color_side(style, "bottom").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.left > 0.0 && has_solid_border_side(style, "left") {
        canvas.fill_rect_clipped(
            Rect {
                x: rect.x,
                y: rect.y + border.top,
                width: border.left,
                height: (rect.height - border.top - border.bottom).max(0.0),
            },
            border_color_side(style, "left").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.right > 0.0 && has_solid_border_side(style, "right") {
        canvas.fill_rect_clipped(
            Rect {
                x: rect.x + rect.width - border.right,
                y: rect.y + border.top,
                width: border.right,
                height: (rect.height - border.top - border.bottom).max(0.0),
            },
            border_color_side(style, "right").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
}

fn paint_zero_sized_border_box(
    canvas: &mut Canvas,
    rect: Rect,
    style: &ComputedStyle,
    border: EdgeSizesForPaint,
    clip: Option<Rect>,
) {
    let inner_left = rect.x + border.left;
    let inner_top = rect.y + border.top;
    let inner_right = rect.x + rect.width - border.right;
    let inner_bottom = rect.y + rect.height - border.bottom;

    let paint_top = |canvas: &mut Canvas| {
        if border.top > 0.0 && has_solid_border_side(style, "top") {
            fill_quad_clipped(
                canvas,
                (rect.x, rect.y),
                (rect.x + rect.width, rect.y),
                (inner_right, inner_top),
                (inner_left, inner_top),
                border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0)),
                clip,
            );
        }
    };
    let paint_bottom = |canvas: &mut Canvas| {
        if border.bottom > 0.0 && has_solid_border_side(style, "bottom") {
            fill_quad_clipped(
                canvas,
                (rect.x, rect.y + rect.height),
                (rect.x + rect.width, rect.y + rect.height),
                (inner_right, inner_bottom),
                (inner_left, inner_bottom),
                border_color_side(style, "bottom").unwrap_or(Color::rgb(0, 0, 0)),
                clip,
            );
        }
    };
    let paint_left = |canvas: &mut Canvas| {
        if border.left > 0.0 && has_solid_border_side(style, "left") {
            fill_quad_clipped(
                canvas,
                (rect.x, rect.y),
                (rect.x, rect.y + rect.height),
                (inner_left, inner_bottom),
                (inner_left, inner_top),
                border_color_side(style, "left").unwrap_or(Color::rgb(0, 0, 0)),
                clip,
            );
        }
    };
    let paint_right = |canvas: &mut Canvas| {
        if border.right > 0.0 && has_solid_border_side(style, "right") {
            fill_quad_clipped(
                canvas,
                (rect.x + rect.width, rect.y),
                (rect.x + rect.width, rect.y + rect.height),
                (inner_right, inner_bottom),
                (inner_right, inner_top),
                border_color_side(style, "right").unwrap_or(Color::rgb(0, 0, 0)),
                clip,
            );
        }
    };

    if border.top == 0.0 && border.bottom > 0.0 {
        paint_bottom(canvas);
        paint_left(canvas);
        paint_right(canvas);
    } else if border.bottom == 0.0 && border.top > 0.0 {
        paint_left(canvas);
        paint_right(canvas);
        if has_solid_border_side(style, "top") {
            let color = border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0));
            fill_triangle_clipped_inclusive(
                canvas,
                (rect.x, rect.y),
                (rect.x + rect.width, rect.y),
                (inner_right, inner_top),
                color,
                clip,
            );
            fill_triangle_clipped_inclusive(
                canvas,
                (rect.x, rect.y),
                (inner_right, inner_top),
                (inner_left, inner_top),
                color,
                clip,
            );
        }
    } else {
        paint_top(canvas);
        paint_bottom(canvas);
        paint_left(canvas);
        paint_right(canvas);
    }
}

fn fill_quad_clipped(
    canvas: &mut Canvas,
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    p4: (f32, f32),
    color: Color,
    clip: Option<Rect>,
) {
    fill_triangle_clipped(canvas, p1, p2, p3, color, clip);
    fill_triangle_clipped(canvas, p1, p3, p4, color, clip);
}

fn border_box_rect(layout: &LayoutBox) -> Rect {
    let content = layout.dimensions.content;
    let padding = layout.dimensions.padding;
    let border = layout.dimensions.border;
    Rect {
        x: content.x - padding.left - border.left,
        y: content.y - padding.top - border.top,
        width: content.width + padding.left + padding.right + border.left + border.right,
        height: content.height + padding.top + padding.bottom + border.top + border.bottom,
    }
}

fn padding_box_rect(layout: &LayoutBox) -> Rect {
    let content = layout.dimensions.content;
    let padding = layout.dimensions.padding;
    Rect {
        x: content.x - padding.left,
        y: content.y - padding.top,
        width: content.width + padding.left + padding.right,
        height: content.height + padding.top + padding.bottom,
    }
}

fn background_color(style: &ComputedStyle) -> Option<Color> {
    color_property(style.get("background-color"))
}

fn background_image(style: &ComputedStyle) -> Option<Image> {
    match style.get("background-image") {
        Some(ComputedValue::Keyword(keyword)) => parse_background_image_value(keyword),
        Some(ComputedValue::String(value)) => parse_background_image_value(value),
        _ => None,
    }
}

fn background_repeat(style: &ComputedStyle) -> bool {
    !matches!(
        style.get("background-repeat"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("no-repeat")
    )
}

fn background_attachment_fixed(style: &ComputedStyle) -> bool {
    matches!(
        style.get("background-attachment"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("fixed")
    )
}

fn background_position(style: &ComputedStyle) -> (f32, f32) {
    (
        length_property(style, "background-position-x").unwrap_or(0.0),
        length_property(style, "background-position-y").unwrap_or(0.0),
    )
}

fn parse_background_image_value(value: &str) -> Option<Image> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("none") {
        return None;
    }
    let url = trimmed
        .strip_prefix("url(")
        .and_then(|value| value.strip_suffix(')'))
        .unwrap_or(trimmed)
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_start_matches("\\\"")
        .trim_end_matches("\\\"")
        .trim_start_matches("\\'")
        .trim_end_matches("\\'");
    crate::layout::decode_or_fetch_image_asset(url).or_else(|| {
        let data_uri = parse_data_uri(url).ok()?;
        match data_uri {
            DataUri::Binary { mime_type, data } if mime_type.eq_ignore_ascii_case("image/png") => {
                Image::decode_png(&data).ok()
            }
            DataUri::Binary { mime_type, data }
                if mime_type.eq_ignore_ascii_case("image/jpeg")
                    || mime_type.eq_ignore_ascii_case("image/jpg") =>
            {
                Image::decode_jpeg(&data).ok()
            }
            _ => None,
        }
    })
}

fn border_color(style: &ComputedStyle) -> Option<Color> {
    color_property(style.get("border-color")).or_else(|| color_property(style.get("color")))
}

fn border_color_side(style: &ComputedStyle, side: &str) -> Option<Color> {
    color_property(style.get(&format!("border-{side}-color"))).or_else(|| border_color(style))
}

fn text_color(style: &ComputedStyle) -> Option<Color> {
    color_property(style.get("color"))
}

fn color_property(value: Option<&ComputedValue>) -> Option<Color> {
    match value {
        Some(ComputedValue::Color(color)) => parse_color(color),
        Some(ComputedValue::Keyword(color)) => parse_color(color),
        _ => None,
    }
}

fn length_property(style: &ComputedStyle, name: &str) -> Option<f32> {
    match style.get(name) {
        Some(ComputedValue::Px(value)) => Some(*value),
        Some(ComputedValue::Number(value)) => Some(*value),
        _ => None,
    }
}

fn paint_background_image(
    canvas: &mut Canvas,
    style: &ComputedStyle,
    rect: Rect,
    clip: Option<Rect>,
    viewport: Rect,
) {
    let Some(image) = background_image(style) else {
        return;
    };
    let Some(area) = normalize_rect(rect) else {
        return;
    };

    let tile_width = image.width().max(1) as f32;
    let tile_height = image.height().max(1) as f32;
    let x_end = area.x + area.width;
    let y_end = area.y + area.height;
    let repeat = background_repeat(style);
    let (position_x, position_y) = background_position(style);
    let fixed = background_attachment_fixed(style);
    let anchor_x = if fixed {
        viewport.x + position_x
    } else {
        area.x + position_x
    };
    let anchor_y = if fixed {
        viewport.y + position_y
    } else {
        area.y + position_y
    };
    let mut y = if repeat {
        anchor_y + ((area.y - anchor_y) / tile_height).floor() * tile_height
    } else {
        anchor_y
    };
    while y < y_end {
        let mut x = if repeat {
            anchor_x + ((area.x - anchor_x) / tile_width).floor() * tile_width
        } else {
            anchor_x
        };
        while x < x_end {
            canvas.draw_image_clipped(&image, x, y, clip.or(Some(area)));
            if !repeat {
                return;
            }
            x += tile_width;
        }
        y += tile_height;
    }
}

fn has_any_solid_border(style: &ComputedStyle) -> bool {
    ["top", "right", "bottom", "left"]
        .into_iter()
        .any(|side| has_solid_border_side(style, side))
}

fn has_solid_border_side(style: &ComputedStyle, side: &str) -> bool {
    if matches!(
        style.get(&format!("border-{side}-style")),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("solid")
    ) {
        return true;
    }

    matches!(
        style.get("border-style"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("solid")
    )
}

fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("transparent") {
        return Some(Color::rgba(0, 0, 0, 0));
    }

    let lower = value.to_ascii_lowercase();

    // Named color lookup (CSS Level 4 extended set)
    if let Some(c) = named_color(&lower) {
        return Some(c);
    }

    // Hex colors: #RGB, #RGBA, #RRGGBB, #RRGGBBAA
    if let Some(hex) = lower.strip_prefix('#') {
        if !hex.is_ascii() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                Some(Color::rgb(r, g, b))
            }
            4 => {
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                let a = u8::from_str_radix(&hex[3..4].repeat(2), 16).ok()?;
                Some(Color::rgba(r, g, b, a))
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Color::rgb(r, g, b))
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some(Color::rgba(r, g, b, a))
            }
            _ => None,
        };
    }

    // Functional color notations handled directly for robustness
    if let Some(color) = parse_color_function(&lower) {
        return Some(color);
    }

    None
}

/// Parses `rgb()`, `rgba()`, `hsl()`, `hsla()` function notation from a string.
fn parse_color_function(value: &str) -> Option<Color> {
    let (name, args_str) = parse_function_call(value)?;

    match name {
        "rgb" | "rgba" => parse_rgb_args(args_str),
        "hsl" | "hsla" => parse_hsl_args(args_str),
        _ => None,
    }
}

/// Splits a CSS function call string into `(name, args)`.
fn parse_function_call(value: &str) -> Option<(&str, &str)> {
    let paren = value.find('(')?;
    let name = value[..paren].trim();
    if !value.ends_with(')') {
        return None;
    }
    let args = &value[paren + 1..value.len() - 1];
    Some((name, args))
}

/// Parses `rgb()` / `rgba()` argument string.
///
/// Supports both comma-separated `rgb(r, g, b)` / `rgba(r, g, b, a)` and
/// modern space-separated `rgb(r g b / a)` syntax.
fn parse_rgb_args(args: &str) -> Option<Color> {
    let parts = split_color_args(args);
    let nums: Vec<f32> = parts.iter().filter_map(|s| s.parse().ok()).collect();
    match nums.as_slice() {
        [r, g, b] => Some(Color::rgb(*r as u8, *g as u8, *b as u8)),
        [r, g, b, a] => {
            let alpha = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
            Some(Color::rgba(*r as u8, *g as u8, *b as u8, alpha))
        }
        _ => None,
    }
}

/// Parses `hsl()` / `hsla()` argument string.
fn parse_hsl_args(args: &str) -> Option<Color> {
    let parts = split_color_args(args);
    let nums: Vec<f32> = parts
        .iter()
        .filter_map(|s| s.trim_end_matches('%').parse().ok())
        .collect();

    match nums.as_slice() {
        [h, s, l] => {
            let (r, g, b) = hsl_to_rgb(*h, *s / 100.0, *l / 100.0);
            Some(Color::rgb(r, g, b))
        }
        [h, s, l, a] => {
            let (r, g, b) = hsl_to_rgb(*h, *s / 100.0, *l / 100.0);
            let alpha = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
            Some(Color::rgba(r, g, b, alpha))
        }
        _ => None,
    }
}

/// Splits a CSS color function argument string by commas or whitespace+slash.
///
/// Handles both `255, 0, 0, 0.5` and `255 0 0 / 0.5` forms.
fn split_color_args(args: &str) -> Vec<String> {
    if args.contains(',') {
        args.split(',').map(|s| s.trim().to_string()).collect()
    } else {
        // Modern syntax: "r g b / a" — strip "/" and split by whitespace
        args.split_whitespace()
            .filter(|s| *s != "/")
            .map(|s| s.to_string())
            .collect()
    }
}

// HSL→RGB conversion is shared with src/css/style.rs
use crate::css::style::hsl_to_rgb;

/// Returns the RGB color for a CSS named color keyword.
///
/// Supports CSS Level 4 named colors (140+ colors).
#[allow(clippy::too_many_lines)]
fn named_color(name: &str) -> Option<Color> {
    let c = match name {
        // CSS Level 1 / basic
        "black" => Color::rgb(0, 0, 0),
        "white" => Color::rgb(255, 255, 255),
        "red" => Color::rgb(255, 0, 0),
        "green" => Color::rgb(0, 128, 0),
        "blue" => Color::rgb(0, 0, 255),
        "yellow" => Color::rgb(255, 255, 0),
        "navy" => Color::rgb(0, 0, 128),
        "purple" => Color::rgb(128, 0, 128),
        "maroon" => Color::rgb(128, 0, 0),
        "gray" | "grey" => Color::rgb(128, 128, 128),
        "silver" => Color::rgb(192, 192, 192),
        "aqua" | "cyan" => Color::rgb(0, 255, 255),
        "teal" => Color::rgb(0, 128, 128),
        "lime" => Color::rgb(0, 255, 0),
        "fuchsia" | "magenta" => Color::rgb(255, 0, 255),
        "olive" => Color::rgb(128, 128, 0),
        // Orange / red family
        "orange" => Color::rgb(255, 165, 0),
        "orangered" => Color::rgb(255, 69, 0),
        "darkorange" => Color::rgb(255, 140, 0),
        "coral" => Color::rgb(255, 127, 80),
        "tomato" => Color::rgb(255, 99, 71),
        "salmon" => Color::rgb(250, 128, 114),
        "lightsalmon" => Color::rgb(255, 160, 122),
        "darksalmon" => Color::rgb(233, 150, 122),
        "crimson" => Color::rgb(220, 20, 60),
        "firebrick" => Color::rgb(178, 34, 34),
        "darkred" => Color::rgb(139, 0, 0),
        "indianred" => Color::rgb(205, 92, 92),
        // Pink family
        "pink" => Color::rgb(255, 192, 203),
        "lightpink" => Color::rgb(255, 182, 193),
        "hotpink" => Color::rgb(255, 105, 180),
        "deeppink" => Color::rgb(255, 20, 147),
        "palevioletred" => Color::rgb(219, 112, 147),
        "mediumvioletred" => Color::rgb(199, 21, 133),
        // Gold / yellow / brown
        "gold" => Color::rgb(255, 215, 0),
        "goldenrod" => Color::rgb(218, 165, 32),
        "darkgoldenrod" => Color::rgb(184, 134, 11),
        "palegoldenrod" => Color::rgb(238, 232, 170),
        "peru" => Color::rgb(205, 133, 63),
        "chocolate" => Color::rgb(210, 105, 30),
        "sienna" => Color::rgb(160, 82, 45),
        "saddlebrown" => Color::rgb(139, 69, 19),
        "brown" => Color::rgb(165, 42, 42),
        "tan" => Color::rgb(210, 180, 140),
        "burlywood" => Color::rgb(222, 184, 135),
        "wheat" => Color::rgb(245, 222, 179),
        "sandybrown" => Color::rgb(244, 164, 96),
        "rosybrown" => Color::rgb(188, 143, 143),
        // Purple / violet
        "lavender" => Color::rgb(230, 230, 250),
        "thistle" => Color::rgb(216, 191, 216),
        "plum" => Color::rgb(221, 160, 221),
        "violet" => Color::rgb(238, 130, 238),
        "orchid" => Color::rgb(218, 112, 214),
        "mediumorchid" => Color::rgb(186, 85, 211),
        "darkorchid" => Color::rgb(153, 50, 204),
        "darkviolet" => Color::rgb(148, 0, 211),
        "blueviolet" => Color::rgb(138, 43, 226),
        "indigo" => Color::rgb(75, 0, 130),
        "slateblue" => Color::rgb(106, 90, 205),
        "darkslateblue" => Color::rgb(72, 61, 139),
        "mediumpurple" => Color::rgb(147, 112, 219),
        "rebeccapurple" => Color::rgb(102, 51, 153),
        // Blue family
        "lightblue" => Color::rgb(173, 216, 230),
        "powderblue" => Color::rgb(176, 224, 230),
        "lightskyblue" => Color::rgb(135, 206, 250),
        "skyblue" => Color::rgb(135, 206, 235),
        "deepskyblue" => Color::rgb(0, 191, 255),
        "dodgerblue" => Color::rgb(30, 144, 255),
        "cornflowerblue" => Color::rgb(100, 149, 237),
        "steelblue" => Color::rgb(70, 130, 180),
        "royalblue" => Color::rgb(65, 105, 225),
        "mediumblue" => Color::rgb(0, 0, 205),
        "darkblue" => Color::rgb(0, 0, 139),
        "midnightblue" => Color::rgb(25, 25, 112),
        "azure" => Color::rgb(240, 255, 255),
        "aliceblue" => Color::rgb(240, 248, 255),
        "ghostwhite" => Color::rgb(248, 248, 255),
        "lavenderblush" => Color::rgb(255, 240, 245),
        // Green family
        "mintcream" => Color::rgb(245, 255, 250),
        "honeydew" => Color::rgb(240, 255, 240),
        "lightgreen" => Color::rgb(144, 238, 144),
        "palegreen" => Color::rgb(152, 251, 152),
        "limegreen" => Color::rgb(50, 205, 50),
        "mediumseagreen" => Color::rgb(60, 179, 113),
        "seagreen" => Color::rgb(46, 139, 87),
        "forestgreen" => Color::rgb(34, 139, 34),
        "darkgreen" => Color::rgb(0, 100, 0),
        "yellowgreen" => Color::rgb(154, 205, 50),
        "olivedrab" => Color::rgb(107, 142, 35),
        "darkolivegreen" => Color::rgb(85, 107, 47),
        "mediumaquamarine" => Color::rgb(102, 205, 170),
        "aquamarine" => Color::rgb(127, 255, 212),
        "turquoise" => Color::rgb(64, 224, 208),
        "mediumturquoise" => Color::rgb(72, 209, 204),
        "darkturquoise" => Color::rgb(0, 206, 209),
        "lightseagreen" => Color::rgb(32, 178, 170),
        "cadetblue" => Color::rgb(95, 158, 160),
        "darkcyan" => Color::rgb(0, 139, 139),
        "darkslategray" | "darkslategrey" => Color::rgb(47, 79, 79),
        "slategray" | "slategrey" => Color::rgb(112, 128, 144),
        "lightslategray" | "lightslategrey" => Color::rgb(119, 136, 153),
        // Gray shades
        "darkgray" | "darkgrey" => Color::rgb(169, 169, 169),
        "dimgray" | "dimgrey" => Color::rgb(105, 105, 105),
        "lightgray" | "lightgrey" => Color::rgb(211, 211, 211),
        "gainsboro" => Color::rgb(220, 220, 220),
        "whitesmoke" => Color::rgb(245, 245, 245),
        "snow" => Color::rgb(255, 250, 250),
        "seashell" => Color::rgb(255, 245, 238),
        "floralwhite" => Color::rgb(255, 250, 240),
        "ivory" => Color::rgb(255, 255, 240),
        "linen" => Color::rgb(250, 240, 230),
        "oldlace" => Color::rgb(253, 245, 230),
        "antiquewhite" => Color::rgb(250, 235, 215),
        "bisque" => Color::rgb(255, 228, 196),
        "blanchedalmond" => Color::rgb(255, 235, 205),
        "moccasin" => Color::rgb(255, 228, 181),
        "navajowhite" => Color::rgb(255, 222, 173),
        "peachpuff" => Color::rgb(255, 218, 185),
        "mistyrose" => Color::rgb(255, 228, 225),
        "papayawhip" => Color::rgb(255, 239, 213),
        "lightyellow" => Color::rgb(255, 255, 224),
        "lemonchiffon" => Color::rgb(255, 250, 205),
        "cornsilk" => Color::rgb(255, 248, 220),
        "beige" => Color::rgb(245, 245, 220),
        "khaki" => Color::rgb(240, 230, 140),
        "darkkhaki" => Color::rgb(189, 183, 107),
        // Chartreuse / spring
        "chartreuse" => Color::rgb(127, 255, 0),
        "lawngreen" => Color::rgb(124, 252, 0),
        "greenyellow" => Color::rgb(173, 255, 47),
        "springgreen" => Color::rgb(0, 255, 127),
        "mediumslateblue" => Color::rgb(123, 104, 238),
        "mediumspringgreen" => Color::rgb(0, 250, 154),
        // Missing CSS Level 4 colors
        "darkmagenta" => Color::rgb(139, 0, 139),
        "darkseagreen" => Color::rgb(143, 188, 143),
        "lightcoral" => Color::rgb(240, 128, 128),
        "lightcyan" => Color::rgb(224, 255, 255),
        "lightgoldenrodyellow" => Color::rgb(250, 250, 210),
        "lightsteelblue" => Color::rgb(176, 196, 222),
        "paleturquoise" => Color::rgb(175, 238, 238),
        _ => return None,
    };
    Some(c)
}

fn normalize_rect(rect: Rect) -> Option<Rect> {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        None
    } else {
        Some(rect)
    }
}

fn intersect(left: Rect, right: Rect) -> Option<Rect> {
    let x0 = left.x.max(right.x);
    let y0 = left.y.max(right.y);
    let x1 = (left.x + left.width).min(right.x + right.width);
    let y1 = (left.y + left.height).min(right.y + right.height);
    if x1 <= x0 || y1 <= y0 {
        None
    } else {
        Some(Rect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        })
    }
}

fn fill_triangle_clipped(
    canvas: &mut Canvas,
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    color: Color,
    clip: Option<Rect>,
) {
    fill_triangle_clipped_inner(canvas, p0, p1, p2, color, clip, false);
}

fn fill_triangle_clipped_inclusive(
    canvas: &mut Canvas,
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    color: Color,
    clip: Option<Rect>,
) {
    fill_triangle_clipped_inner(canvas, p0, p1, p2, color, clip, true);
}

fn fill_triangle_clipped_inner(
    canvas: &mut Canvas,
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    color: Color,
    clip: Option<Rect>,
    inclusive_edges: bool,
) {
    if color.a == 0 {
        return;
    }

    let min_x = p0.0.min(p1.0).min(p2.0).floor().max(0.0) as i32;
    let min_y = p0.1.min(p1.1).min(p2.1).floor().max(0.0) as i32;
    let max_x = p0.0.max(p1.0).max(p2.0).ceil().min(canvas.width as f32) as i32;
    let max_y = p0.1.max(p1.1).max(p2.1).ceil().min(canvas.height as f32) as i32;
    let clip = clip.and_then(normalize_rect);

    for y in min_y..max_y {
        for x in min_x..max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            if !(if inclusive_edges {
                point_in_triangle_inclusive((px, py), p0, p1, p2)
            } else {
                point_in_triangle((px, py), p0, p1, p2)
            }) {
                continue;
            }
            if let Some(clip_rect) = clip {
                if px < clip_rect.x
                    || px >= clip_rect.x + clip_rect.width
                    || py < clip_rect.y
                    || py >= clip_rect.y + clip_rect.height
                {
                    continue;
                }
            }

            let index = ((y as u32 * canvas.width + x as u32) * 4) as usize;
            blend_pixel(&mut canvas.pixels[index..index + 4], color);
        }
    }
}

fn point_in_triangle(point: (f32, f32), p0: (f32, f32), p1: (f32, f32), p2: (f32, f32)) -> bool {
    let area = triangle_sign(p0, p1, p2);
    if area == 0.0 {
        return false;
    }

    let mut w0 = triangle_sign(point, p1, p2);
    let mut w1 = triangle_sign(point, p2, p0);
    let mut w2 = triangle_sign(point, p0, p1);
    if area < 0.0 {
        w0 = -w0;
        w1 = -w1;
        w2 = -w2;
    }

    const EPSILON: f32 = 1e-6;
    (w0 > EPSILON || (w0.abs() <= EPSILON && is_top_left_edge(p1, p2)))
        && (w1 > EPSILON || (w1.abs() <= EPSILON && is_top_left_edge(p2, p0)))
        && (w2 > EPSILON || (w2.abs() <= EPSILON && is_top_left_edge(p0, p1)))
}

fn point_in_triangle_inclusive(
    point: (f32, f32),
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
) -> bool {
    let area = triangle_sign(p0, p1, p2);
    if area == 0.0 {
        return false;
    }

    let mut w0 = triangle_sign(point, p1, p2);
    let mut w1 = triangle_sign(point, p2, p0);
    let mut w2 = triangle_sign(point, p0, p1);
    if area < 0.0 {
        w0 = -w0;
        w1 = -w1;
        w2 = -w2;
    }

    const EPSILON: f32 = 1e-6;
    w0 >= -EPSILON && w1 >= -EPSILON && w2 >= -EPSILON
}

fn triangle_sign(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32)) -> f32 {
    (p0.0 - p2.0) * (p1.1 - p2.1) - (p1.0 - p2.0) * (p0.1 - p2.1)
}

fn is_top_left_edge(start: (f32, f32), end: (f32, f32)) -> bool {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    dy < 0.0 || (dy == 0.0 && dx > 0.0)
}

fn blend_pixel(pixel: &mut [u8], color: Color) {
    let src_a = color.a as u32;
    let dst_a = pixel[3] as u32;
    let out_a = src_a + (dst_a * (255 - src_a) + 127) / 255;
    if out_a == 0 {
        pixel.copy_from_slice(&[0, 0, 0, 0]);
        return;
    }

    let blend_channel = |src: u8, dst: u8| -> u8 {
        let src = src as u32;
        let dst = dst as u32;
        let premultiplied = src * src_a + ((dst * dst_a * (255 - src_a) + 127) / 255);
        ((premultiplied + out_a / 2) / out_a) as u8
    };

    pixel[0] = blend_channel(color.r, pixel[0]);
    pixel[1] = blend_channel(color.g, pixel[1]);
    pixel[2] = blend_channel(color.b, pixel[2]);
    pixel[3] = out_a as u8;
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);

    let mut crc_input = Vec::with_capacity(kind.len() + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn zlib_compress_uncompressed(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x78);
    out.push(0x01);

    let mut offset = 0usize;
    while offset < data.len() {
        let remaining = data.len() - offset;
        let chunk_len = remaining.min(u16::MAX as usize);
        let is_last = offset + chunk_len == data.len();

        out.push(if is_last { 0x01 } else { 0x00 });
        let len = chunk_len as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(&data[offset..offset + chunk_len]);
        offset += chunk_len;
    }

    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let mut a = 1u32;
    let mut b = 0u32;
    for byte in data {
        a = (a + *byte as u32) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg() & 0xedb8_8320;
            crc = (crc >> 1) ^ mask;
        }
    }
    !crc
}

/// Returns true if the media attribute value applies to screen rendering.
///
/// Matches:
/// - Empty string or missing attribute (defaults to "all")
/// - "all" or "screen" as whole-word media types
/// - Comma-separated lists (e.g., "print, screen")
/// - Media queries with "only" modifier (e.g., "only screen")
///
/// Does NOT match:
/// - "print" or other non-screen media types
/// - "not screen" (negated screen)
/// - Substrings (e.g., "small" does NOT match just because it contains "all")
fn matches_screen_media(media: Option<&str>) -> bool {
    let media = match media {
        None => return true, // No media attr = all media
        Some(s) => s.trim(),
    };

    if media.is_empty() {
        return true; // Empty media attr = all media
    }

    // Parse as comma-separated list of media queries
    for query in media.split(',') {
        let query = query.trim();
        if query.is_empty() {
            // Empty entry means "all"
            return true;
        }

        let query_lower = query.to_ascii_lowercase();
        let mut tokens = query_lower.split_whitespace();

        let first = tokens.next();
        let (modifier, media_type) = match first {
            None => {
                // Empty query -> defaults to "all"
                (None::<&str>, Some("all"))
            }
            Some(tok) if tok == "not" || tok == "only" => {
                // Modifier followed by media type
                let mt = tokens.next();
                (Some(tok), mt)
            }
            Some(tok) if tok.starts_with('(') => {
                // Leading feature without explicit type (e.g., "(min-width: 800px)")
                // defaults to "all"
                (None::<&str>, Some("all"))
            }
            Some(tok) => {
                // First token is the media type
                (None::<&str>, Some(tok))
            }
        };

        let media_type = media_type.unwrap_or("all");
        let is_screen_like = media_type == "screen" || media_type == "all";

        if !is_screen_like {
            // Non-screen media type such as "print", "speech", etc.
            continue;
        }

        match modifier {
            Some("not") => {
                // "not screen" or "not all" explicitly excludes screen
                continue;
            }
            _ => {
                // Matches screen/all (with or without "only")
                return true;
            }
        }
    }

    // No query matched screen/all
    false
}

/// Checks if two URLs have the same origin (scheme + host + port).
fn same_origin(a: &crate::http::Url, b: &crate::http::Url) -> bool {
    a.scheme() == b.scheme() && a.host() == b.host() && a.port() == b.port()
}

/// Recursively finds all `<base>` elements in document order.
fn find_base_elements(node: &NodeHandle, result: &mut Vec<NodeHandle>) {
    if node.node_type() == crate::dom::NodeType::Element {
        if node.tag_name().as_deref() == Some("base") {
            result.push(node.clone());
        }
    }
    for child in node.child_nodes() {
        find_base_elements(&child, result);
    }
}

/// Extracts the document's base URL from the first `<base href="...">` element with a valid href.
///
/// Scans all `<base>` elements in document order and uses the first one with a
/// non-empty, resolvable `href`. For SSRF protection, absolute URLs are only honored
/// if they have the same origin (scheme + host + port) as the fallback_base.
/// Returns the fallback base if no valid same-origin `<base>` is found.
fn extract_document_base_url(
    document: &NodeHandle,
    fallback_base: Option<&crate::http::Url>,
) -> Option<crate::http::Url> {
    let mut base_elements = Vec::new();
    find_base_elements(document, &mut base_elements);

    for base_elem in base_elements {
        if let Some(attrs) = base_elem.attributes() {
            if let Some(href) = attrs.get("href") {
                let href = href.trim();
                if href.is_empty() {
                    continue; // Skip empty href, try next <base>
                }

                // Absolute URL
                if href.contains("://") {
                    if let Ok(url) = href.parse::<crate::http::Url>() {
                        // SSRF protection: only honor same-origin absolute base URLs
                        if let Some(ref original) = fallback_base {
                            if same_origin(&url, original) {
                                return Some(url);
                            }
                        }
                        // If no fallback_base provided, don't enable fetching via <base>
                        continue;
                    }
                    continue; // Invalid absolute URL, try next <base>
                }

                // Relative URL (resolve against fallback_base)
                if let Some(base) = fallback_base {
                    if let Ok(url) = resolve_url(base, href) {
                        // Relative URLs always resolve to same origin
                        return Some(url);
                    }
                }
            }
        }
    }
    fallback_base.cloned()
}

fn extract_author_stylesheets(
    document: &NodeHandle,
    base_url: Option<&crate::http::Url>,
) -> Result<Vec<String>, PaintError> {
    // Compute effective base URL considering <base> element
    let effective_base = extract_document_base_url(document, base_url);

    let mut stylesheets = Vec::new();
    let mut client = effective_base.as_ref().map(|_| crate::http::Client::new());
    collect_author_stylesheets(
        document,
        &mut stylesheets,
        effective_base.as_ref(),
        &mut client,
    )?;
    Ok(stylesheets)
}

const MAX_EXTERNAL_STYLESHEET_BYTES: usize = 1024 * 1024; // 1 MiB limit
const MAX_IMPORT_DEPTH: usize = 5;

fn collect_author_stylesheets(
    node: &NodeHandle,
    out: &mut Vec<String>,
    base_url: Option<&crate::http::Url>,
    client: &mut Option<crate::http::Client>,
) -> Result<(), PaintError> {
    if node.node_type() == NodeType::Element {
        match node.tag_name().as_deref() {
            Some("style") => {
                let css = collect_text_contents(node);
                if !css.trim().is_empty() {
                    let mut active_import_urls = HashSet::new();
                    collect_stylesheet_with_imports(
                        css,
                        base_url,
                        base_url,
                        out,
                        client,
                        0,
                        &mut active_import_urls,
                    )?;
                }
            }
            Some("link") => {
                let attributes = node.attributes().unwrap_or_default();
                let rel = attributes.get("rel").cloned().unwrap_or_default();
                let href = attributes
                    .get("href")
                    .cloned()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let media = attributes.get("media").map(|s| s.as_str());

                if rel
                    .split_whitespace()
                    .any(|token| token.eq_ignore_ascii_case("stylesheet"))
                    && !href.is_empty()
                    && matches_screen_media(media)
                {
                    if href.starts_with("data:text/css") {
                        let mut active_import_urls = HashSet::new();
                        match parse_data_uri(&href)? {
                            DataUri::Text { data, .. } => collect_stylesheet_with_imports(
                                data,
                                None,
                                base_url,
                                out,
                                client,
                                0,
                                &mut active_import_urls,
                            )?,
                            DataUri::Binary { data, .. } => collect_stylesheet_with_imports(
                                String::from_utf8_lossy(&data).into_owned(),
                                None,
                                base_url,
                                out,
                                client,
                                0,
                                &mut active_import_urls,
                            )?,
                        }
                    } else if let Some(base) = base_url {
                        if let Some((css, resolved)) =
                            fetch_relative_stylesheet(base, &href, client, base_url)
                        {
                            let mut active_import_urls = HashSet::new();
                            collect_stylesheet_with_imports(
                                css,
                                Some(&resolved),
                                base_url,
                                out,
                                client,
                                0,
                                &mut active_import_urls,
                            )?;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    for child in node.child_nodes() {
        collect_author_stylesheets(&child, out, base_url, client)?;
    }

    Ok(())
}

fn collect_stylesheet_with_imports(
    css: String,
    stylesheet_url: Option<&crate::http::Url>,
    document_base: Option<&crate::http::Url>,
    out: &mut Vec<String>,
    client: &mut Option<crate::http::Client>,
    depth: usize,
    active_import_urls: &mut HashSet<String>,
) -> Result<(), PaintError> {
    if depth < MAX_IMPORT_DEPTH {
        let import_base = stylesheet_url.or(document_base);
        if let Some(base) = import_base {
            for import_href in extract_import_hrefs(&css) {
                let Some(import_url) =
                    resolve_relative_stylesheet_url(base, &import_href, document_base)
                else {
                    continue;
                };
                let import_url_string = import_url.to_string();
                if !active_import_urls.insert(import_url_string.clone()) {
                    continue;
                }
                if let Some(import_css) = fetch_stylesheet_by_url(&import_url, client) {
                    collect_stylesheet_with_imports(
                        import_css,
                        Some(&import_url),
                        document_base,
                        out,
                        client,
                        depth + 1,
                        active_import_urls,
                    )?;
                }
                active_import_urls.remove(&import_url_string);
            }
        }
    }

    out.push(css);
    Ok(())
}

fn extract_import_hrefs(css: &str) -> Vec<String> {
    let Ok(stylesheet) = parse_stylesheet(css) else {
        return extract_import_hrefs_forgiving(css);
    };

    let mut hrefs = Vec::new();
    for rule in stylesheet.rules {
        if let crate::css::Rule::At(at_rule) = rule {
            if at_rule.name.eq_ignore_ascii_case("import") {
                if let Some(href) = parse_import_href(&at_rule.prelude) {
                    hrefs.push(href);
                }
            }
        }
    }
    hrefs
}

fn extract_import_hrefs_forgiving(css: &str) -> Vec<String> {
    let mut hrefs = Vec::new();
    let chars: Vec<char> = css.chars().collect();
    let mut index = 0usize;
    let mut in_string = None::<char>;
    let mut paren_depth = 0usize;

    while index < chars.len() {
        let ch = chars[index];

        if let Some(quote) = in_string {
            if ch == '\\' && index + 1 < chars.len() {
                index += 2;
                continue;
            }
            if ch == quote {
                in_string = None;
            }
            index += 1;
            continue;
        }

        if ch == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            index = (index + 2).min(chars.len());
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = Some(ch);
                index += 1;
                continue;
            }
            '(' => {
                paren_depth += 1;
                index += 1;
                continue;
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                index += 1;
                continue;
            }
            _ => {}
        }

        if paren_depth == 0 && at_import_starts_at(&chars, index) {
            let mut prelude_start = index + 7;
            while prelude_start < chars.len() && chars[prelude_start].is_ascii_whitespace() {
                prelude_start += 1;
            }
            let mut cursor = prelude_start;
            let mut local_in_string = None::<char>;
            let mut local_paren_depth = 0usize;
            while cursor < chars.len() {
                let c = chars[cursor];
                if let Some(quote) = local_in_string {
                    if c == '\\' && cursor + 1 < chars.len() {
                        cursor += 2;
                        continue;
                    }
                    if c == quote {
                        local_in_string = None;
                    }
                    cursor += 1;
                    continue;
                }
                if c == '"' || c == '\'' {
                    local_in_string = Some(c);
                    cursor += 1;
                    continue;
                }
                if c == '(' {
                    local_paren_depth += 1;
                    cursor += 1;
                    continue;
                }
                if c == ')' {
                    local_paren_depth = local_paren_depth.saturating_sub(1);
                    cursor += 1;
                    continue;
                }
                if c == ';' && local_paren_depth == 0 {
                    let prelude: String = chars[prelude_start..cursor].iter().collect();
                    if let Some(href) = parse_import_href(&prelude) {
                        hrefs.push(href);
                    }
                    cursor += 1;
                    break;
                }
                cursor += 1;
            }
            index = cursor;
            continue;
        }

        index += 1;
    }

    hrefs
}

fn at_import_starts_at(chars: &[char], index: usize) -> bool {
    let target: [char; 7] = ['@', 'i', 'm', 'p', 'o', 'r', 't'];
    if index + target.len() > chars.len() {
        return false;
    }
    for (offset, expected) in target.iter().enumerate() {
        if chars[index + offset].to_ascii_lowercase() != *expected {
            return false;
        }
    }
    if index + target.len() < chars.len() {
        let next = chars[index + target.len()];
        if next.is_ascii_alphanumeric() || next == '-' || next == '_' {
            return false;
        }
    }
    true
}

fn parse_import_href(prelude: &str) -> Option<String> {
    let prelude = prelude.trim();
    if prelude.is_empty() {
        return None;
    }

    if prelude
        .get(0..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("url("))
    {
        let rest = &prelude[4..];
        let close = rest.find(')')?;
        let content = rest[..close].trim();
        // Media/supports conditions are out of scope for this phase.
        // Ignore @import rules with trailing prelude tokens.
        if !rest[close + 1..].trim().is_empty() {
            return None;
        }
        if let Some(quoted) = unquote_css_token(content) {
            return Some(quoted);
        }
        if content.starts_with('"') || content.starts_with('\'') {
            return None;
        }
        return non_empty_token(content);
    }

    if prelude.starts_with('"') || prelude.starts_with('\'') {
        let quote = prelude.chars().next()?;
        let mut escaped = false;
        for (index, ch) in prelude.char_indices().skip(1) {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == quote {
                let value = prelude[1..index].trim();
                if value.is_empty() {
                    return None;
                }
                if !prelude[index + ch.len_utf8()..].trim().is_empty() {
                    return None;
                }
                return Some(value.to_string());
            }
        }
        return None;
    }
    None
}

fn unquote_css_token(token: &str) -> Option<String> {
    let token = token.trim();
    let first = token.chars().next()?;
    if first != '"' && first != '\'' {
        return None;
    }
    let mut escaped = false;
    for (index, ch) in token.char_indices().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == first {
            let value = token[1..index].trim();
            if value.is_empty() {
                return None;
            }
            if !token[index + ch.len_utf8()..].trim().is_empty() {
                return None;
            }
            return Some(value.to_string());
        }
    }
    None
}

fn non_empty_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn fetch_relative_stylesheet(
    base: &crate::http::Url,
    href: &str,
    client: &mut Option<crate::http::Client>,
    document_base: Option<&crate::http::Url>,
) -> Option<(String, crate::http::Url)> {
    let resolved = resolve_relative_stylesheet_url(base, href, document_base)?;
    let css = fetch_stylesheet_by_url(&resolved, client)?;
    Some((css, resolved))
}

fn resolve_relative_stylesheet_url(
    base: &crate::http::Url,
    href: &str,
    document_base: Option<&crate::http::Url>,
) -> Option<crate::http::Url> {
    // Only fetch same-origin URLs that do not specify a scheme, to prevent SSRF attacks.
    // Absolute URLs (containing "://") and protocol-relative URLs ("//")
    // are skipped; this still allows relative and absolute-path references.
    if href.contains("://") || href.starts_with("//") {
        return None;
    }

    let resolved = resolve_url(base, href).ok()?;
    if let Some(document_base) = document_base {
        if !same_origin(&resolved, document_base) {
            return None;
        }
    }
    Some(resolved)
}

fn fetch_stylesheet_by_url(
    resolved: &crate::http::Url,
    client: &mut Option<crate::http::Client>,
) -> Option<String> {
    let url_str = resolved.to_string();
    let c = client.as_mut()?;
    let resp = c.get(&url_str).ok()?;
    if resp.status_code() != 200 {
        return None;
    }
    let body = resp.body();
    if body.len() > MAX_EXTERNAL_STYLESHEET_BYTES {
        return None;
    }
    std::str::from_utf8(body).ok().map(|s| s.to_owned())
}

fn collect_text_contents(node: &NodeHandle) -> String {
    let mut text = String::new();
    for child in node.child_nodes() {
        match child.node_type() {
            NodeType::Text => {
                if let Some(data) = child.data() {
                    text.push_str(&data);
                }
            }
            NodeType::Element => text.push_str(&collect_text_contents(&child)),
            _ => {}
        }
    }
    text
}

fn parse_stylesheet_forgiving(input: &str) -> Result<Stylesheet, PaintError> {
    if let Ok(stylesheet) = parse_stylesheet(input) {
        return Ok(stylesheet);
    }

    let mut rules = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut prev_backslash = false;

    for ch in input.chars() {
        current.push(ch);
        if prev_backslash {
            prev_backslash = false;
            continue;
        }
        if ch == '\\' {
            prev_backslash = true;
            continue;
        }
        match ch {
            '{' => depth += 1,
            '}' => {
                if depth > 0 {
                    depth -= 1;
                }
                if depth == 0 {
                    let trimmed = current.trim_start_matches(|c: char| c.is_ascii_whitespace());
                    if !trimmed.is_empty() {
                        if let Ok(stylesheet) = parse_stylesheet(trimmed) {
                            rules.extend(stylesheet.rules);
                        } else if let Some(rule) = salvage_style_rule(trimmed) {
                            rules.push(crate::css::Rule::Style(rule));
                        }
                    }
                    current.clear();
                }
            }
            _ => {}
        }
    }

    if rules.is_empty() {
        Err(PaintError::InvalidStylesheet)
    } else {
        Ok(Stylesheet { rules })
    }
}

fn salvage_style_rule(input: &str) -> Option<crate::css::StyleRule> {
    let open = input.find('{')?;
    let close = input.rfind('}')?;
    if close <= open {
        return None;
    }

    let selector = input[..open].trim();
    let body = &input[open + 1..close];
    let mut selectors = None;
    let mut declarations = Vec::new();

    for declaration in split_declarations_forgiving(body) {
        let normalized = normalize_unquoted_urls(&declaration);
        let candidate = format!("{selector} {{ {normalized}; }}");
        let Ok(stylesheet) = parse_stylesheet(&candidate) else {
            continue;
        };
        let Some(crate::css::Rule::Style(rule)) = stylesheet.rules.into_iter().next() else {
            continue;
        };
        if selectors.is_none() {
            selectors = Some(rule.selectors);
        }
        declarations.extend(rule.declarations);
    }

    if declarations.is_empty() {
        return None;
    }

    Some(crate::css::StyleRule {
        selectors: selectors?,
        declarations,
    })
}

fn normalize_unquoted_urls(input: &str) -> String {
    let mut output = String::new();
    let mut index = 0usize;

    while let Some(relative_start) = input[index..].find("url(") {
        let start = index + relative_start;
        output.push_str(&input[index..start + 4]);
        let content_start = start + 4;
        let Some(relative_end) = input[content_start..].find(')') else {
            output.push_str(&input[content_start..]);
            return output;
        };
        let end = content_start + relative_end;
        let content = input[content_start..end].trim();
        if content.starts_with('"') || content.starts_with('\'') {
            output.push_str(content);
        } else {
            output.push('"');
            output.push_str(content);
            output.push('"');
        }
        output.push(')');
        index = end + 1;
    }

    output.push_str(&input[index..]);
    output
}

fn split_declarations_forgiving(input: &str) -> Vec<String> {
    let mut declarations = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut in_string = None::<char>;
    let chars: Vec<char> = input.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        let ch = chars[index];
        if let Some(quote) = in_string {
            current.push(ch);
            if ch == quote {
                in_string = None;
            } else if ch == '\\' && index + 1 < chars.len() {
                index += 1;
                current.push(chars[index]);
            }
            index += 1;
            continue;
        }

        if ch == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            index = (index + 2).min(chars.len());
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = Some(ch);
                current.push(ch);
            }
            '(' => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(ch);
            }
            '[' => {
                bracket_depth += 1;
                current.push(ch);
            }
            ']' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                current.push(ch);
            }
            ';' if paren_depth == 0 && bracket_depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    declarations.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
        index += 1;
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        declarations.push(trimmed.to_string());
    }

    declarations
}

fn decode_png(bytes: &[u8]) -> Result<Image, PaintError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = match decoder.read_info() {
        Ok(reader) => reader,
        Err(_) => return decode_png_fallback(bytes),
    };
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = match reader.next_frame(&mut buffer) {
        Ok(info) => info,
        Err(_) => return decode_png_fallback(bytes),
    };
    let pixels = &buffer[..info.buffer_size()];

    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(info.width as usize * info.height as usize * 4);
            for chunk in pixels.chunks_exact(3) {
                out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity(info.width as usize * info.height as usize * 4);
            for chunk in pixels.chunks_exact(2) {
                out.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity(info.width as usize * info.height as usize * 4);
            for value in pixels {
                out.extend_from_slice(&[*value, *value, *value, 255]);
            }
            out
        }
        _ => return Err(PaintError::UnsupportedPngFormat),
    };

    Image::new(info.width, info.height, rgba)
}

fn decode_png_fallback(bytes: &[u8]) -> Result<Image, PaintError> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < SIGNATURE.len() || &bytes[..8] != SIGNATURE {
        return Err(PaintError::InvalidPngSignature);
    }

    let mut index = 8usize;
    let mut width = None;
    let mut height = None;
    let mut bit_depth = 0u8;
    let mut color_type = 0u8;
    let mut interlace = 0u8;
    let mut compressed = Vec::new();

    while index + 8 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[index..index + 4].try_into().unwrap()) as usize;
        index += 4;
        let chunk_type = &bytes[index..index + 4];
        index += 4;
        if index + length + 4 > bytes.len() {
            return Err(PaintError::CorruptPng);
        }
        let data = &bytes[index..index + length];
        index += length;
        index += 4;

        match chunk_type {
            b"IHDR" => {
                if data.len() < 13 {
                    return Err(PaintError::MissingPngHeader);
                }
                width = Some(u32::from_be_bytes(data[0..4].try_into().unwrap()));
                height = Some(u32::from_be_bytes(data[4..8].try_into().unwrap()));
                bit_depth = data[8];
                color_type = data[9];
                interlace = data[12];
            }
            b"IDAT" => compressed.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
    }

    let width = width.ok_or(PaintError::MissingPngHeader)?;
    let height = height.ok_or(PaintError::MissingPngHeader)?;
    if bit_depth != 8 || interlace != 0 {
        return Err(PaintError::UnsupportedPngFormat);
    }

    let bytes_per_pixel = match color_type {
        0 => 1usize,
        2 => 3usize,
        4 => 2usize,
        6 => 4usize,
        _ => return Err(PaintError::UnsupportedPngFormat),
    };
    let stride = width as usize * bytes_per_pixel;
    let expected = height as usize * (1 + stride);
    let mut decompressed = Vec::new();
    ZlibDecoder::new(Cursor::new(compressed))
        .read_to_end(&mut decompressed)
        .map_err(|_| PaintError::DecompressionFailed)?;
    if decompressed.len() < expected {
        return Err(PaintError::CorruptPng);
    }

    let mut raw = vec![0u8; height as usize * stride];
    for row in 0..height as usize {
        let filter = decompressed[row * (stride + 1)];
        let src = &decompressed[row * (stride + 1) + 1..row * (stride + 1) + 1 + stride];
        let (previous_rows, current_and_rest) = raw.split_at_mut(row * stride);
        let dest = &mut current_and_rest[..stride];
        let prev = if row == 0 {
            None
        } else {
            Some(&previous_rows[(row - 1) * stride..row * stride])
        };
        unfilter_png_scanline(filter, src, prev, dest, bytes_per_pixel)?;
    }

    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    match color_type {
        0 => {
            for &value in &raw {
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        2 => {
            for chunk in raw.chunks_exact(3) {
                rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
        }
        4 => {
            for chunk in raw.chunks_exact(2) {
                rgba.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
        }
        6 => rgba = raw,
        _ => return Err(PaintError::UnsupportedPngFormat),
    }

    Image::new(width, height, rgba)
}

fn unfilter_png_scanline(
    filter: u8,
    src: &[u8],
    prev: Option<&[u8]>,
    dest: &mut [u8],
    bytes_per_pixel: usize,
) -> Result<(), PaintError> {
    match filter {
        0 => dest.copy_from_slice(src),
        1 => {
            for index in 0..src.len() {
                let left = if index >= bytes_per_pixel {
                    dest[index - bytes_per_pixel]
                } else {
                    0
                };
                dest[index] = src[index].wrapping_add(left);
            }
        }
        2 => {
            for index in 0..src.len() {
                let up = prev.map(|row| row[index]).unwrap_or(0);
                dest[index] = src[index].wrapping_add(up);
            }
        }
        3 => {
            for index in 0..src.len() {
                let left = if index >= bytes_per_pixel {
                    dest[index - bytes_per_pixel]
                } else {
                    0
                };
                let up = prev.map(|row| row[index]).unwrap_or(0);
                dest[index] = src[index].wrapping_add(((left as u16 + up as u16) / 2) as u8);
            }
        }
        4 => {
            for index in 0..src.len() {
                let a = if index >= bytes_per_pixel {
                    dest[index - bytes_per_pixel]
                } else {
                    0
                };
                let b = prev.map(|row| row[index]).unwrap_or(0);
                let c = if index >= bytes_per_pixel {
                    prev.map(|row| row[index - bytes_per_pixel]).unwrap_or(0)
                } else {
                    0
                };
                dest[index] = src[index].wrapping_add(paeth_predictor(a, b, c));
            }
        }
        _ => return Err(PaintError::UnsupportedPngFormat),
    }
    Ok(())
}

fn paeth_predictor(a: u8, b: u8, c: u8) -> u8 {
    let a = a as i32;
    let b = b as i32;
    let c = c as i32;
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

fn decode_jpeg(bytes: &[u8]) -> Result<Image, PaintError> {
    use jpeg_decoder::Decoder;

    let mut decoder = Decoder::new(bytes);
    let pixels = decoder.decode().map_err(|_| PaintError::InvalidJpeg)?;
    let info = decoder.info().ok_or(PaintError::InvalidJpeg)?;

    let expected_pixels = info.width as usize * info.height as usize;

    let rgba = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            // Validate buffer size matches expected RGB24 data
            let expected_bytes = expected_pixels * 3;
            if pixels.len() != expected_bytes {
                return Err(PaintError::InvalidJpeg);
            }
            let chunks = pixels.chunks_exact(3);
            if !chunks.remainder().is_empty() {
                return Err(PaintError::InvalidJpeg);
            }
            let mut out = Vec::with_capacity(expected_pixels * 4);
            for chunk in chunks {
                out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            out
        }
        jpeg_decoder::PixelFormat::L8 => {
            // Validate buffer size matches expected grayscale data
            if pixels.len() != expected_pixels {
                return Err(PaintError::InvalidJpeg);
            }
            let mut out = Vec::with_capacity(expected_pixels * 4);
            for value in pixels {
                out.extend_from_slice(&[value, value, value, 255]);
            }
            out
        }
        _ => return Err(PaintError::UnsupportedJpegFormat),
    };

    Image::new(info.width as u32, info.height as u32, rgba)
}

/// Parses a `data:` URI into either text or binary content.
pub fn parse_data_uri(uri: &str) -> Result<DataUri, PaintError> {
    let payload = uri
        .strip_prefix("data:")
        .ok_or(PaintError::InvalidDataUri)?;
    let (metadata, data) = payload.split_once(',').ok_or(PaintError::InvalidDataUri)?;
    let mut mime_type = "text/plain".to_string();
    let mut is_base64 = false;

    if !metadata.is_empty() {
        for (index, part) in metadata.split(';').enumerate() {
            if index == 0 && !part.is_empty() {
                mime_type = part.to_string();
                continue;
            }
            if part.eq_ignore_ascii_case("base64") {
                is_base64 = true;
            }
        }
    }

    if is_base64 {
        let decoded_payload = percent_decode(data);
        let data = base64::engine::general_purpose::STANDARD
            .decode(decoded_payload)
            .map_err(|_| PaintError::InvalidBase64)?;
        Ok(DataUri::Binary { mime_type, data })
    } else {
        Ok(DataUri::Text {
            mime_type,
            data: percent_decode(data),
        })
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                out.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
