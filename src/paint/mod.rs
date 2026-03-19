//! Pixel-based painting primitives and layout tree rendering.

use std::fs;
use std::io::Read;
use std::path::Path;

use crate::css::{ComputedStyle, ComputedValue, Origin, StyleResolver, Stylesheet, parse_stylesheet};
use crate::dom::{Node, NodeHandle, NodeType};
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
    paint_box(&mut canvas, layout, resolver, None);
    canvas
}

/// Renders a DOM document into a canvas using inline and linked author stylesheets.
pub fn render_document(document: &NodeHandle, viewport: Rect) -> Result<Canvas, PaintError> {
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(document)? {
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
    let width = actual.width().max(expected.width());
    let height = actual.height().max(expected.height());
    let mut diff = Canvas::new(width, height);
    let mut changed = 0usize;

    for y in 0..height {
        for x in 0..width {
            let left = actual.pixel(x, y).unwrap_or(Color::rgba(0, 0, 0, 0));
            let right = expected.pixel(x, y).unwrap_or(Color::rgba(0, 0, 0, 0));
            let color = if left == right {
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
    paint_background_image(canvas, &style, border_box, inherited_clip);

    paint_borders(canvas, layout, &style, inherited_clip);
    paint_text(canvas, layout, &style, inherited_clip);

    let clip = if layout.overflow == crate::layout::Overflow::Hidden {
        match inherited_clip {
            Some(current) => intersect(current, padding_box),
            None => Some(padding_box),
        }
    } else {
        inherited_clip
    };

    for child in &layout.children {
        paint_box(canvas, child, resolver, clip);
    }
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
                InlineFragmentContent::Image(image) => {
                    canvas.draw_image_clipped(image, fragment.rect.x, fragment.rect.y, clip);
                }
                InlineFragmentContent::GeneratedBox => {}
            }
        }
    }
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
        .trim_matches('\'');
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

fn paint_background_image(
    canvas: &mut Canvas,
    style: &ComputedStyle,
    rect: Rect,
    clip: Option<Rect>,
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
    let mut y = area.y;
    while y < y_end {
        let mut x = area.x;
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

fn extract_author_stylesheets(document: &NodeHandle) -> Result<Vec<String>, PaintError> {
    let mut stylesheets = Vec::new();
    collect_author_stylesheets(document, &mut stylesheets)?;
    Ok(stylesheets)
}

fn collect_author_stylesheets(
    node: &NodeHandle,
    out: &mut Vec<String>,
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
                let href = attributes.get("href").cloned().unwrap_or_default();
                if rel.contains("stylesheet") && href.starts_with("data:text/css") {
                    match parse_data_uri(&href)? {
                        DataUri::Text { data, .. } => out.push(data),
                        DataUri::Binary { data, .. } => {
                            out.push(String::from_utf8_lossy(&data).into_owned())
                        }
                    }
                }
            }
            _ => {}
        }
    }

    for child in node.child_nodes() {
        collect_author_stylesheets(&child, out)?;
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

    for ch in input.chars() {
        current.push(ch);
        match ch {
            '{' => depth += 1,
            '}' => {
                if depth > 0 {
                    depth -= 1;
                }
                if depth == 0 {
                    if let Ok(stylesheet) = parse_stylesheet(&current) {
                        rules.extend(stylesheet.rules);
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

fn decode_png(bytes: &[u8]) -> Result<Image, PaintError> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < SIGNATURE.len() || &bytes[..8] != SIGNATURE {
        return Err(PaintError::InvalidPngSignature);
    }

    let mut cursor = 8usize;
    let mut width = None;
    let mut height = None;
    let mut color_type = None;
    let mut bit_depth = None;
    let mut interlace_method = None;
    let mut compressed = Vec::new();

    while cursor + 8 <= bytes.len() {
        let length = u32::from_be_bytes(
            bytes[cursor..cursor + 4]
                .try_into()
                .map_err(|_| PaintError::CorruptPng)?,
        ) as usize;
        cursor += 4;
        let chunk_type = &bytes[cursor..cursor + 4];
        cursor += 4;
        if cursor + length + 4 > bytes.len() {
            return Err(PaintError::CorruptPng);
        }
        let data = &bytes[cursor..cursor + length];
        cursor += length;
        let _crc = &bytes[cursor..cursor + 4];
        cursor += 4;

        match chunk_type {
            b"IHDR" => {
                if data.len() != 13 {
                    return Err(PaintError::CorruptPng);
                }
                width = Some(u32::from_be_bytes(
                    data[0..4].try_into().map_err(|_| PaintError::CorruptPng)?,
                ));
                height = Some(u32::from_be_bytes(
                    data[4..8].try_into().map_err(|_| PaintError::CorruptPng)?,
                ));
                bit_depth = Some(data[8]);
                color_type = Some(data[9]);
                interlace_method = Some(data[12]);
            }
            b"IDAT" => compressed.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
    }

    let width = width.ok_or(PaintError::MissingPngHeader)?;
    let height = height.ok_or(PaintError::MissingPngHeader)?;
    let bit_depth = bit_depth.ok_or(PaintError::MissingPngHeader)?;
    let color_type = color_type.ok_or(PaintError::MissingPngHeader)?;
    let interlace_method = interlace_method.ok_or(PaintError::MissingPngHeader)?;

    if bit_depth != 8 || interlace_method != 0 {
        return Err(PaintError::UnsupportedPngFormat);
    }

    let bytes_per_pixel = match color_type {
        6 => 4usize,
        2 => 3usize,
        _ => return Err(PaintError::UnsupportedPngFormat),
    };
    let stride = width as usize * bytes_per_pixel;
    let expected = (stride + 1) * height as usize;

    let mut decoder = ZlibDecoder::new(compressed.as_slice());
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|_| PaintError::DecompressionFailed)?;
    if decompressed.len() != expected {
        return Err(PaintError::CorruptPng);
    }

    let mut previous = vec![0u8; stride];
    let mut reconstructed = vec![0u8; width as usize * height as usize * 4];

    for row in 0..height as usize {
        let row_offset = row * (stride + 1);
        let filter = decompressed[row_offset];
        let filtered = &decompressed[row_offset + 1..row_offset + 1 + stride];
        let mut current = vec![0u8; stride];
        unfilter_scanline(filter, filtered, &previous, bytes_per_pixel, &mut current)?;

        match color_type {
            6 => {
                let start = row * width as usize * 4;
                reconstructed[start..start + stride].copy_from_slice(&current);
            }
            2 => {
                for column in 0..width as usize {
                    let source = column * 3;
                    let dest = (row * width as usize + column) * 4;
                    reconstructed[dest] = current[source];
                    reconstructed[dest + 1] = current[source + 1];
                    reconstructed[dest + 2] = current[source + 2];
                    reconstructed[dest + 3] = 255;
                }
            }
            _ => unreachable!(),
        }

        previous = current;
    }

    Image::new(width, height, reconstructed)
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

fn unfilter_scanline(
    filter: u8,
    filtered: &[u8],
    previous: &[u8],
    bytes_per_pixel: usize,
    out: &mut [u8],
) -> Result<(), PaintError> {
    match filter {
        0 => out.copy_from_slice(filtered),
        1 => {
            for index in 0..filtered.len() {
                let left = if index >= bytes_per_pixel {
                    out[index - bytes_per_pixel]
                } else {
                    0
                };
                out[index] = filtered[index].wrapping_add(left);
            }
        }
        2 => {
            for index in 0..filtered.len() {
                out[index] = filtered[index].wrapping_add(previous[index]);
            }
        }
        3 => {
            for index in 0..filtered.len() {
                let left = if index >= bytes_per_pixel {
                    out[index - bytes_per_pixel]
                } else {
                    0
                };
                let up = previous[index];
                out[index] = filtered[index].wrapping_add(((left as u16 + up as u16) / 2) as u8);
            }
        }
        4 => {
            for index in 0..filtered.len() {
                let left = if index >= bytes_per_pixel {
                    out[index - bytes_per_pixel]
                } else {
                    0
                };
                let up = previous[index];
                let upper_left = if index >= bytes_per_pixel {
                    previous[index - bytes_per_pixel]
                } else {
                    0
                };
                out[index] =
                    filtered[index].wrapping_add(paeth_predictor(left, up, upper_left));
            }
        }
        _ => return Err(PaintError::UnsupportedPngFormat),
    }

    Ok(())
}

fn paeth_predictor(left: u8, up: u8, upper_left: u8) -> u8 {
    let left = left as i32;
    let up = up as i32;
    let upper_left = upper_left as i32;
    let predictor = left + up - upper_left;
    let left_distance = (predictor - left).abs();
    let up_distance = (predictor - up).abs();
    let upper_left_distance = (predictor - upper_left).abs();

    if left_distance <= up_distance && left_distance <= upper_left_distance {
        left as u8
    } else if up_distance <= upper_left_distance {
        up as u8
    } else {
        upper_left as u8
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use crate::css::{Origin, StyleResolver, parse_stylesheet};
    use crate::html::TreeBuilder;
    use crate::dom::NodeHandle;
    use crate::layout::{Rect, layout_tree};

    use super::*;

    #[test]
    fn fills_rectangles_on_canvas() {
        let mut canvas = Canvas::new(4, 4);
        canvas.fill_rect(
            Rect {
                x: 1.0,
                y: 1.0,
                width: 2.0,
                height: 2.0,
            },
            Color::rgb(255, 0, 0),
        );

        assert_eq!(canvas.pixel(0, 0), Some(Color::rgba(0, 0, 0, 0)));
        assert_eq!(canvas.pixel(1, 1), Some(Color::rgb(255, 0, 0)));
        assert_eq!(canvas.pixel(2, 2), Some(Color::rgb(255, 0, 0)));
        assert_eq!(canvas.pixel(3, 3), Some(Color::rgba(0, 0, 0, 0)));
    }

    #[test]
    fn paints_backgrounds_and_borders_from_layout_boxes() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let panel = NodeHandle::element("div");
        document.append_child(body.clone());
        body.append_child(panel);

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body { margin: 0; } \
                 div { width: 20px; height: 20px; background-color: #ff0000; border: 2px solid #0000ff; }",
            )
            .unwrap(),
        );
        let layout = layout_tree(
            &document,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 40.0,
            },
        )
        .unwrap();

        let mut paint_resolver = StyleResolver::new();
        paint_resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body { margin: 0; } \
                 div { width: 20px; height: 20px; background-color: #ff0000; border: 2px solid #0000ff; }",
            )
            .unwrap(),
        );
        let canvas = paint_layout(
            &layout,
            &mut paint_resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 40.0,
            },
        );

        assert_eq!(canvas.pixel(1, 1), Some(Color::rgb(0, 0, 255)));
        assert_eq!(canvas.pixel(3, 3), Some(Color::rgb(255, 0, 0)));
    }

    #[test]
    fn clips_children_when_overflow_is_hidden() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let parent = NodeHandle::element("div");
        let child = NodeHandle::element("div");
        document.append_child(body.clone());
        body.append_child(parent.clone());
        parent.append_child(child.clone());

        let stylesheet =
            "body { margin: 0; } \
             .parent { width: 10px; height: 10px; overflow: hidden; background-color: white; } \
             .child { width: 20px; height: 20px; background-color: red; }";
        parent.set_attribute("class", "parent");
        child.set_attribute("class", "child");

        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let layout = layout_tree(
            &document,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 30.0,
                height: 30.0,
            },
        )
        .unwrap();

        let mut paint_resolver = StyleResolver::new();
        paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let canvas = paint_layout(
            &layout,
            &mut paint_resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 30.0,
                height: 30.0,
            },
        );

        assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(255, 0, 0)));
        assert_eq!(canvas.pixel(15, 15), Some(Color::rgba(0, 0, 0, 0)));
    }

    #[test]
    fn skips_hidden_boxes() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let panel = NodeHandle::element("div");
        document.append_child(body.clone());
        body.append_child(panel);

        let stylesheet =
            "body { margin: 0; } div { width: 10px; height: 10px; background-color: red; visibility: hidden; }";
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let layout = layout_tree(
            &document,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 20.0,
                height: 20.0,
            },
        )
        .unwrap();

        let mut paint_resolver = StyleResolver::new();
        paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let canvas = paint_layout(
            &layout,
            &mut paint_resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 20.0,
                height: 20.0,
            },
        );

        assert_eq!(canvas.pixel(5, 5), Some(Color::rgba(0, 0, 0, 0)));
    }

    #[test]
    fn paints_inline_text_fragments() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let paragraph = NodeHandle::element("p");
        paragraph.append_child(NodeHandle::text("hello"));
        document.append_child(body.clone());
        body.append_child(paragraph);

        let stylesheet = "body { margin: 0; } p { color: blue; font-size: 10px; }";
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let layout = layout_tree(
            &document,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 50.0,
                height: 20.0,
            },
        )
        .unwrap();

        let mut paint_resolver = StyleResolver::new();
        paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let canvas = paint_layout(
            &layout,
            &mut paint_resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 50.0,
                height: 20.0,
            },
        );

        let painted_pixels = canvas
            .pixels()
            .chunks_exact(4)
            .filter(|rgba| rgba[2] == 255 && rgba[3] == 255)
            .count();
        assert!(painted_pixels > 0);
    }

    #[test]
    fn paints_tiled_background_images_from_data_uris() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let div = NodeHandle::element("div");
        document.append_child(body.clone());
        body.append_child(div);

        let stylesheet = "body { margin: 0; } div { width: 4px; height: 4px; background: url(\"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADElEQVR4AQEFAPr/AP8AAP9zftimAAAAAElFTkSuQmCC\"); }";
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
        )
        .unwrap();

        let mut paint_resolver = StyleResolver::new();
        paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let canvas = paint_layout(
            &layout,
            &mut paint_resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
        );

        assert_eq!(canvas.pixel(1, 1), Some(Color::rgb(255, 0, 0)));
        assert_eq!(canvas.pixel(3, 3), Some(Color::rgb(255, 0, 0)));
    }

    #[test]
    fn paints_non_repeating_background_images_once() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let div = NodeHandle::element("div");
        document.append_child(body.clone());
        body.append_child(div);

        let stylesheet =
            "body { margin: 0; } div { width: 4px; height: 4px; background: url(\"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADElEQVR4AQEFAPr/AP8AAP9zftimAAAAAElFTkSuQmCC\") no-repeat; }";
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
        )
        .unwrap();

        let mut paint_resolver = StyleResolver::new();
        paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let canvas = paint_layout(
            &layout,
            &mut paint_resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
        );

        assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(255, 0, 0)));
        assert_eq!(canvas.pixel(1, 0), Some(Color::rgba(0, 0, 0, 0)));
        assert_eq!(canvas.pixel(0, 1), Some(Color::rgba(0, 0, 0, 0)));
    }

    #[test]
    fn paints_side_specific_border_colors() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let div = NodeHandle::element("div");
        document.append_child(body.clone());
        body.append_child(div);

        let stylesheet =
            "body { margin: 0; } \
             div { width: 4px; height: 4px; border-top: solid red 1px; border-left: solid blue 1px; border-right: solid yellow 1px; border-bottom: solid black 1px; }";
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let layout = layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
        )
        .unwrap();

        let mut paint_resolver = StyleResolver::new();
        paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let canvas = paint_layout(
            &layout,
            &mut paint_resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
        );

        assert_eq!(canvas.pixel(1, 0), Some(Color::rgb(255, 0, 0)));
        assert_eq!(canvas.pixel(0, 1), Some(Color::rgb(0, 0, 255)));
        assert_eq!(canvas.pixel(5, 1), Some(Color::rgb(255, 255, 0)));
        assert_eq!(canvas.pixel(1, 5), Some(Color::rgb(0, 0, 0)));
    }

    #[test]
    fn paints_children_in_z_index_order() {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let container = NodeHandle::element("div");
        let low = NodeHandle::element("div");
        let high = NodeHandle::element("div");
        low.set_attribute("class", "low");
        high.set_attribute("class", "high");
        document.append_child(body.clone());
        body.append_child(container.clone());
        container.append_child(low);
        container.append_child(high);

        let stylesheet =
            "body { margin: 0; } \
             .low { position: absolute; left: 0; top: 0; width: 10px; height: 10px; background-color: blue; z-index: 1; } \
             .high { position: absolute; left: 0; top: 0; width: 10px; height: 10px; background-color: red; z-index: 10; }";
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let layout = layout_tree(
            &document,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 20.0,
                height: 20.0,
            },
        )
        .unwrap();

        let mut paint_resolver = StyleResolver::new();
        paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
        let canvas = paint_layout(
            &layout,
            &mut paint_resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 20.0,
                height: 20.0,
            },
        );

        assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(255, 0, 0)));
    }

    #[test]
    fn encodes_canvas_as_png() {
        let mut canvas = Canvas::new(2, 1);
        canvas.fill_rect(
            Rect {
                x: 0.0,
                y: 0.0,
                width: 2.0,
                height: 1.0,
            },
            Color::rgb(255, 0, 0),
        );

        let png = canvas.encode_png();
        assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert!(png.windows(4).any(|window| window == b"IHDR"));
        assert!(png.windows(4).any(|window| window == b"IDAT"));
        assert!(png.windows(4).any(|window| window == b"IEND"));
    }

    #[test]
    fn decodes_png_images_into_rgba_pixels() {
        let mut canvas = Canvas::new(2, 1);
        canvas.fill_rect(
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            Color::rgb(255, 0, 0),
        );
        canvas.fill_rect(
            Rect {
                x: 1.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            Color::rgba(0, 255, 0, 128),
        );

        let image = Image::decode_png(&canvas.encode_png()).unwrap();
        assert_eq!(image.width(), 2);
        assert_eq!(image.height(), 1);
        assert_eq!(
            image.pixels(),
            &[
                255, 0, 0, 255,
                0, 255, 0, 128,
            ]
        );
    }

    #[test]
    fn draws_images_with_alpha_compositing() {
        let image = Image::new(1, 1, vec![255, 0, 0, 128]).unwrap();
        let mut canvas = Canvas::new(1, 1);
        canvas.fill_rect(
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            Color::rgb(0, 0, 255),
        );

        canvas.draw_image(&image, 0.0, 0.0);

        assert_eq!(canvas.pixel(0, 0), Some(Color::rgba(128, 0, 127, 255)));
    }

    #[test]
    fn parses_text_and_png_data_uris() {
        let text = parse_data_uri("data:,hello%20world").unwrap();
        assert_eq!(
            text,
            DataUri::Text {
                mime_type: "text/plain".to_string(),
                data: "hello world".to_string(),
            }
        );

        let mut canvas = Canvas::new(1, 1);
        canvas.fill_rect(
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            Color::rgb(255, 0, 0),
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(canvas.encode_png());
        let image = parse_data_uri(&format!("data:image/png;base64,{encoded}")).unwrap();
        match image {
            DataUri::Binary { mime_type, data } => {
                assert_eq!(mime_type, "image/png");
                assert_eq!(Image::decode_png(&data).unwrap().pixels(), &[255, 0, 0, 255]);
            }
            DataUri::Text { .. } => panic!("expected binary data uri"),
        }
    }

    #[test]
    fn parses_percent_encoded_base64_data_uri() {
        let image = parse_data_uri(
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADElEQVR4AQEFAPr%2FAP8AAP9zftimAAAAAElFTkSuQmCC",
        )
        .unwrap();

        match image {
            DataUri::Binary { mime_type, data } => {
                assert_eq!(mime_type, "image/png");
                assert_eq!(Image::decode_png(&data).unwrap().width(), 1);
            }
            DataUri::Text { .. } => panic!("expected binary data uri"),
        }
    }

    #[test]
    fn renders_acid2_fixture_to_png() {
        let html = fs::read_to_string(acid2_fixture_path()).unwrap();
        let document = TreeBuilder::parse(&html).document();

        let png = render_document_png(
            &document,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
        )
        .unwrap();

        assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    }

    #[test]
    fn renders_official_reference_fixture_to_png() {
        let html = fs::read_to_string(acid2_official_reference_html_path()).unwrap();
        let document = TreeBuilder::parse(&html).document();

        let png = render_document_with_base_path(
            &document,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
            &acid2_fixture_dir(),
        )
        .unwrap()
        .encode_png();

        assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    }

    #[test]
    fn acid2_fixture_matches_local_baseline_png() {
        let html = fs::read_to_string(acid2_fixture_path()).unwrap();
        let document = TreeBuilder::parse(&html).document();
        let actual = render_document(
            &document,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
        )
        .unwrap();

        let reference_path = acid2_baseline_path();
        assert!(
            reference_path.exists(),
            "missing local Acid2 baseline image at {}",
            reference_path.display()
        );
        let expected = Image::decode_png(&fs::read(reference_path).unwrap()).unwrap();
        let expected_canvas = Canvas::new(expected.width(), expected.height());
        let mut expected_canvas = expected_canvas;
        expected_canvas.draw_image(&expected, 0.0, 0.0);

        let (diff, changed) = diff_canvases(&actual, &expected_canvas);
        if changed > 0 {
            fs::create_dir_all(acid2_output_dir()).unwrap();
            fs::write(acid2_output_dir().join("acid2.actual.png"), actual.encode_png()).unwrap();
            fs::write(acid2_output_dir().join("acid2.diff.png"), diff.encode_png()).unwrap();
        }
        assert_eq!(
            changed,
            0,
            "acid2 rendering diverged from the checked-in local baseline; wrote diff assets to tests/output/acid2"
        );
    }

    #[test]
    fn official_acid2_reference_assets_are_checked_in() {
        let reference_html = fs::read_to_string(acid2_official_reference_html_path()).unwrap();
        assert!(reference_html.contains("The Second Acid Test (Reference Rendering)"));

        let reference_png = fs::read(acid2_official_reference_png_path()).unwrap();
        let decoded = Image::decode_png(&reference_png).unwrap();
        assert_eq!(decoded.width(), 168);
        assert_eq!(decoded.height(), 168);
    }

    #[test]
    fn official_reference_fixture_only_lays_out_hello_world_text() {
        let html = fs::read_to_string(acid2_official_reference_html_path()).unwrap();
        let document = TreeBuilder::parse(&html).document();
        materialize_local_assets(&document, &acid2_fixture_dir()).unwrap();

        let mut resolver = StyleResolver::new();
        for stylesheet in extract_author_stylesheets(&document).unwrap() {
            resolver.add_stylesheet(
                Origin::Author,
                parse_stylesheet_forgiving(&stylesheet).unwrap(),
            );
        }

        let layout = crate::layout::layout_tree(
            &document,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
        )
        .unwrap();

        let texts = collect_layout_texts(&layout);
        let joined = texts.concat();
        assert_eq!(joined.trim(), "Hello World!");
    }

    #[test]
    #[ignore = "documents current gap to the official Acid2 reference rendering"]
    fn acid2_fixture_matches_official_reference_rendering() {
        let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
        let acid2_document = TreeBuilder::parse(&acid2_html).document();
        let mut acid2_resolver = StyleResolver::new();
        for stylesheet in extract_author_stylesheets(&acid2_document).unwrap() {
            acid2_resolver.add_stylesheet(
                Origin::Author,
                parse_stylesheet_forgiving(&stylesheet).unwrap(),
            );
        }
        let mut acid2_layout = crate::layout::layout_tree(
            &acid2_document,
            &mut acid2_resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
        )
        .unwrap();
        if let Some(top_y) = find_layout_box_by_id(&acid2_layout, "top").map(|top| top.dimensions.content.y) {
            translate_layout_box_for_test(&mut acid2_layout, 0.0, -top_y);
        }
        let actual = paint_layout(
            &acid2_layout,
            &mut acid2_resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
        );

        let reference_html = fs::read_to_string(acid2_official_reference_html_path()).unwrap();
        let reference_document = TreeBuilder::parse(&reference_html).document();
        let expected = render_document_with_base_path(
            &reference_document,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
            &acid2_fixture_dir(),
        )
        .unwrap();

        let (diff, changed) = diff_canvases(&actual, &expected);
        if changed > 0 {
            fs::create_dir_all(acid2_output_dir()).unwrap();
            fs::write(acid2_output_dir().join("acid2.official-reference.actual.png"), actual.encode_png()).unwrap();
            fs::write(
                acid2_output_dir().join("acid2.official-reference.expected.png"),
                expected.encode_png(),
            )
            .unwrap();
            fs::write(
                acid2_output_dir().join("acid2.official-reference.diff.png"),
                diff.encode_png(),
            )
            .unwrap();
        }

        assert_eq!(
            changed,
            0,
            "acid2 rendering diverged from official reference rendering; wrote diff assets to tests/output/acid2"
        );
    }

    #[test]
    #[ignore = "used only to refresh the checked-in local Acid2 baseline image"]
    fn refresh_acid2_baseline_png() {
        let html = fs::read_to_string(acid2_fixture_path()).unwrap();
        let document = TreeBuilder::parse(&html).document();
        let png = render_document_png(
            &document,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
        )
        .unwrap();

        fs::create_dir_all(acid2_fixture_dir()).unwrap();
        fs::write(acid2_baseline_path(), png).unwrap();
    }

    fn acid2_fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/acid2")
    }

    fn acid2_fixture_path() -> PathBuf {
        acid2_fixture_dir().join("acid2.html")
    }

    fn acid2_baseline_path() -> PathBuf {
        acid2_fixture_dir().join("acid2.baseline.png")
    }

    fn acid2_official_reference_html_path() -> PathBuf {
        acid2_fixture_dir().join("reference.html")
    }

    fn acid2_official_reference_png_path() -> PathBuf {
        acid2_fixture_dir().join("reference.png")
    }

    fn acid2_output_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/output/acid2")
    }

    fn collect_layout_texts(layout: &LayoutBox) -> Vec<String> {
        let mut texts = Vec::new();
        collect_layout_texts_into(layout, &mut texts);
        texts
    }

    fn collect_layout_texts_into(layout: &LayoutBox, out: &mut Vec<String>) {
        for line in &layout.lines {
            for fragment in &line.fragments {
                if let InlineFragmentContent::Text(text) = &fragment.content {
                    out.push(text.clone());
                }
            }
        }
        for child in &layout.children {
            collect_layout_texts_into(child, out);
        }
    }

    fn find_layout_box_by_id<'a>(layout: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
        if layout
            .node
            .attributes()
            .and_then(|attributes| attributes.get("id").cloned())
            .as_deref()
            == Some(id)
        {
            return Some(layout);
        }

        for child in &layout.children {
            if let Some(found) = find_layout_box_by_id(child, id) {
                return Some(found);
            }
        }

        None
    }

    fn translate_layout_box_for_test(layout: &mut LayoutBox, dx: f32, dy: f32) {
        layout.dimensions.content.x += dx;
        layout.dimensions.content.y += dy;
        for line in &mut layout.lines {
            line.rect.x += dx;
            line.rect.y += dy;
            line.baseline += dy;
            for fragment in &mut line.fragments {
                fragment.rect.x += dx;
                fragment.rect.y += dy;
            }
        }
        for child in &mut layout.children {
            translate_layout_box_for_test(child, dx, dy);
        }
    }
}
