//! Pixel-based painting primitives and layout tree rendering.

use std::fs;
use std::io::Cursor;
use std::io::Read;
use std::path::Path;

use crate::css::{ComputedStyle, ComputedValue, Origin, PseudoElement, StyleResolver, Stylesheet, parse_stylesheet};
use crate::dom::{Node, NodeHandle, NodeType};
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
}

/// Parsed contents of a `data:` URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataUri {
    Text {
        mime_type: String,
        data: String,
    },
    Binary {
        mime_type: String,
        data: Vec<u8>,
    },
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
        let destination = Rect {
            x,
            y,
            width: image.width as f32,
            height: image.height as f32,
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
                let source_x = (dest_x as f32 - x).floor() as i32;
                let source_y = (dest_y as f32 - y).floor() as i32;
                if source_x < 0
                    || source_y < 0
                    || source_x >= image.width as i32
                    || source_y >= image.height as i32
                {
                    continue;
                }

                let source_index =
                    ((source_y as u32 * image.width + source_x as u32) * 4) as usize;
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
}

/// Paints a layout tree into a new canvas using the provided viewport size.
pub fn paint_layout(layout: &LayoutBox, resolver: &mut StyleResolver, viewport: Rect) -> Canvas {
    let width = viewport.width.ceil().max(1.0) as u32;
    let height = viewport.height.ceil().max(1.0) as u32;
    let mut canvas = Canvas::new(width, height);
    if let Some(background) = viewport_background_color(layout, resolver) {
        canvas.fill_rect(viewport, background);
    }
    paint_box(&mut canvas, layout, resolver, None, viewport);
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
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(document, base_url)? {
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet_forgiving(&stylesheet)?,
        );
    }
    let layout = crate::layout::layout_tree(document, &mut resolver, viewport)
        .ok_or(PaintError::InvalidImageBuffer)?;
    Ok(paint_layout(&layout, &mut resolver, viewport))
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
    node.set_attribute(
        attribute_name,
        format!("data:{mime_type};base64,{encoded}"),
    );

    Ok(())
}

/// Computes a per-pixel diff image and count between two canvases.
pub fn diff_canvases(actual: &Canvas, expected: &Canvas) -> (Canvas, usize) {
    diff_canvases_with_tolerance(actual, expected, 0)
}

/// Same as [`diff_canvases`] but allows a per-channel tolerance when comparing pixels.
/// A tolerance of 1 treats pixels that differ by at most 1 on every channel as matching.
pub fn diff_canvases_with_tolerance(actual: &Canvas, expected: &Canvas, tolerance: u8) -> (Canvas, usize) {
    let width = actual.width().max(expected.width());
    let height = actual.height().max(expected.height());
    let mut diff = Canvas::new(width, height);
    let mut changed = 0usize;

    for y in 0..height {
        for x in 0..width {
            let left = actual.pixel(x, y).unwrap_or(Color::rgba(0, 0, 0, 0));
            let right = expected.pixel(x, y).unwrap_or(Color::rgba(0, 0, 0, 0));
            let within_tolerance = (left.r as i16 - right.r as i16).unsigned_abs() <= tolerance as u16
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
) {
    paint_box_internal(
        canvas,
        layout,
        resolver,
        inherited_clip,
        viewport,
        true,
    );
}

fn paint_box_internal(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    inherited_clip: Option<Rect>,
    viewport: Rect,
    include_phase_descendants: bool,
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
        paint_box_internal(canvas, child, resolver, clip, viewport, true);
    }
    for child in normal_block_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, false);
    }
    for child in float_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, true);
    }
    paint_text(canvas, layout, &style, clip, viewport);
    for child in inline_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, false);
    }
    for child in auto_positioned_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, true);
    }
    for child in positive_positioned_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, true);
    }

    paint_block_generated_pseudo_box(canvas, layout, resolver, PseudoElement::After, clip, viewport);
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
    viewport: Rect,
) {
    let color = text_color(style).unwrap_or(Color::rgb(0, 0, 0));

    for line in &layout.lines {
        for fragment in &line.fragments {
            match &fragment.content {
                InlineFragmentContent::Text(text) => {
                    let mut cursor_x = fragment.rect.x;
                    let advance = fragment.metrics.average_advance.max(1.0);
                    let font_size = fragment.metrics.font_size.max(1.0);
                    let glyph_height = (font_size * 0.7).max(1.0);
                    let glyph_y = fragment.rect.y + (font_size - glyph_height) * 0.5;

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
                InlineFragmentContent::Image(image, style) => {
                    paint_inline_image_fragment(canvas, fragment.rect, image, style, clip, viewport);
                }
                InlineFragmentContent::GeneratedBox(style) => {
                    paint_generated_box(canvas, fragment.rect, style, clip, viewport);
                }
            }
        }
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
    canvas.draw_image_clipped(
        image,
        content_rect.x,
        content_rect.y,
        clip,
    );
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
    let data_uri = parse_data_uri(url).ok()?;
    match data_uri {
        DataUri::Binary { mime_type, data } if mime_type.eq_ignore_ascii_case("image/png") => {
            Image::decode_png(&data).ok()
        }
        _ => None,
    }
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
    match lower.as_str() {
        "black" => return Some(Color::rgb(0, 0, 0)),
        "white" => return Some(Color::rgb(255, 255, 255)),
        "red" => return Some(Color::rgb(255, 0, 0)),
        "green" => return Some(Color::rgb(0, 128, 0)),
        "blue" => return Some(Color::rgb(0, 0, 255)),
        "yellow" => return Some(Color::rgb(255, 255, 0)),
        "navy" => return Some(Color::rgb(0, 0, 128)),
        "purple" => return Some(Color::rgb(128, 0, 128)),
        "maroon" => return Some(Color::rgb(128, 0, 0)),
        "gray" | "grey" => return Some(Color::rgb(128, 128, 128)),
        _ => {}
    }

    if let Some(hex) = lower.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                Some(Color::rgb(r, g, b))
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Color::rgb(r, g, b))
            }
            _ => None,
        };
    }

    None
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

fn point_in_triangle(
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

fn extract_author_stylesheets(
    document: &NodeHandle,
    base_url: Option<&crate::http::Url>,
) -> Result<Vec<String>, PaintError> {
    let mut stylesheets = Vec::new();
    let mut client = base_url.map(|_| crate::http::Client::new());
    collect_author_stylesheets(document, &mut stylesheets, base_url, &mut client)?;
    Ok(stylesheets)
}

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
                    out.push(css);
                }
            }
            Some("link") => {
                let attributes = node.attributes().unwrap_or_default();
                let rel = attributes.get("rel").cloned().unwrap_or_default();
                let href = attributes.get("href").cloned().unwrap_or_default().trim().to_string();
                if rel.split_whitespace()
                    .any(|token| token.eq_ignore_ascii_case("stylesheet"))
                    && !href.is_empty()
                {
                    if href.starts_with("data:text/css") {
                        match parse_data_uri(&href)? {
                            DataUri::Text { data, .. } => out.push(data),
                            DataUri::Binary { data, .. } => {
                                out.push(String::from_utf8_lossy(&data).into_owned())
                            }
                        }
                    } else if let Some(base) = base_url {
                        // Only fetch same-origin URLs that do not specify a scheme, to prevent SSRF attacks.
                        // Absolute URLs (containing "://") and protocol-relative URLs ("//")
                        // are skipped; this still allows relative and absolute-path references
                        // like "/css/style.css".
                        if !href.contains("://") && !href.starts_with("//") {
                            if let Ok(resolved) = resolve_url(base, &href) {
                                let url_str = resolved.to_string();
                                if let Some(c) = client {
                                    match c.get(&url_str) {
                                        Ok(resp) if resp.status_code() == 200 => {
                                            let body = resp.body();
                                            const MAX_EXTERNAL_STYLESHEET_BYTES: usize = 1024 * 1024; // 1 MiB limit
                                            if body.len() <= MAX_EXTERNAL_STYLESHEET_BYTES {
                                                if let Ok(css_str) = std::str::from_utf8(body) {
                                                    out.push(css_str.to_owned());
                                                }
                                            }
                                        }
                                        _ => {} // Skip on fetch failure
                                    }
                                }
                            }
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
                let left = if index >= bytes_per_pixel { dest[index - bytes_per_pixel] } else { 0 };
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
                let left = if index >= bytes_per_pixel { dest[index - bytes_per_pixel] } else { 0 };
                let up = prev.map(|row| row[index]).unwrap_or(0);
                dest[index] = src[index].wrapping_add(((left as u16 + up as u16) / 2) as u8);
            }
        }
        4 => {
            for index in 0..src.len() {
                let a = if index >= bytes_per_pixel { dest[index - bytes_per_pixel] } else { 0 };
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
            if let (Some(high), Some(low)) = (hex_value(bytes[index + 1]), hex_value(bytes[index + 2])) {
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
