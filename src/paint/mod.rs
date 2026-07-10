//! Pixel-based painting primitives and layout tree rendering.

pub(crate) mod border;
pub(crate) mod color;
pub(crate) mod image;
pub(crate) mod stylesheet;
pub(crate) mod text;

use std::cell::Cell;
use std::path::Path;

/// Total virtual milliseconds the render pipeline advances the JS event loop to
/// drain script-scheduled timers before layout.
const TIMER_PUMP_MAX_VIRTUAL_MS: u64 = 10_000;
/// Virtual-time increment per event-loop step while pumping timers.
const TIMER_PUMP_STEP_MS: u64 = 10;
/// Hard cap on the number of timer tasks executed while pumping, guarding
/// against callbacks that endlessly re-schedule zero-delay timers.
const TIMER_PUMP_MAX_TASKS: usize = 100_000;

thread_local! {
    static FORCE_OPACITY: Cell<bool> = const { Cell::new(false) };
}

/// Runs the given closure with `opacity: 0` overridden to `opacity: 1`.
/// Useful for rendering pages that use JS-driven fade-in animations.
pub fn with_force_opacity<T>(f: impl FnOnce() -> T) -> T {
    struct ForceOpacityGuard(bool);
    impl Drop for ForceOpacityGuard {
        fn drop(&mut self) {
            FORCE_OPACITY.with(|cell| cell.set(self.0));
        }
    }
    FORCE_OPACITY.with(|cell| {
        let _guard = ForceOpacityGuard(cell.get());
        cell.set(true);
        f()
    })
}

fn force_opacity_enabled() -> bool {
    FORCE_OPACITY.with(|cell| cell.get())
}

#[allow(unused_imports)]
use base64::Engine;

use crate::css::{
    ComputedStyle, ComputedValue, Origin, PseudoElement, StyleResolver,
};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::font::{Font, WebFontRegistry};
#[allow(unused_imports)]
use crate::layout::{InlineFragmentContent, LayoutBox, Rect, Visibility};

// Re-export public types from submodules
pub use color::Color;
pub use image::parse_data_uri;

// Re-export crate-internal items so that `use crate::paint::*` in tests and sibling modules works.
// Many of these are only referenced from test code, hence the allow.
#[allow(unused_imports)]
pub(crate) use border::{EdgeSizesForPaint, has_solid_border_side, border_color_side};
#[allow(unused_imports)]
pub(crate) use color::{
    parse_color, named_color, split_gradient_args, parse_linear_gradient, parse_gradient_direction,
    interpolate_gradient_color, paint_linear_gradient, ColorStop, LinearGradient,
};
#[allow(unused_imports)]
pub(crate) use image::{
    decode_png, decode_png_fallback, unfilter_png_scanline, paeth_predictor, decode_jpeg,
    percent_decode, hex_value, parse_background_image_value, parse_size_token,
};
#[allow(unused_imports)]
pub(crate) use text::{
    paint_text_with_font, paint_text_with_font_refs, paint_text_with_registry,
    paint_text_placeholder,
    apply_text_transform, TextDecorationLines, text_decoration_line, text_decoration_color,
    paint_text_decoration, paint_list_marker, paint_list_marker_placeholder, load_text_fonts,
    rasterize_with_fallback, rasterize_with_fallback_refs, is_cjk_preferred_character,
    paint_inline_image_fragment, inline_fragment_content_rect, text_color,
};
#[allow(unused_imports)]
pub(crate) use border::{
    paint_borders, paint_rect_borders, paint_zero_sized_border_box, fill_quad_clipped,
    has_any_solid_border, BoxShadow, parse_box_shadow, split_box_shadow_layers,
    paint_box_shadow, paint_outer_box_shadow,
};
#[allow(unused_imports)]
pub(crate) use stylesheet::{
    extract_author_stylesheets, collect_author_stylesheets, collect_stylesheet_with_imports,
    extract_import_hrefs, extract_import_hrefs_forgiving, at_import_starts_at,
    parse_import_href, unquote_css_token, non_empty_token, fetch_relative_stylesheet,
    resolve_relative_stylesheet_url, fetch_stylesheet_by_url, parse_stylesheet_forgiving,
    salvage_style_rule, normalize_unquoted_urls, split_declarations_forgiving,
    matches_screen_media, same_origin, find_base_elements, extract_document_base_url,
    collect_text_contents, materialize_local_assets, rewrite_local_asset_attribute,
    fetch_font_face_fonts, WebFont,
};

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
        image::decode_png(bytes)
    }

    /// Decodes a JPEG image into RGBA pixels.
    pub fn decode_jpeg(bytes: &[u8]) -> Result<Self, PaintError> {
        image::decode_jpeg(bytes)
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

/// ボーダー描画で使う領域区分（角丸ボーダー時の色割り当て用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BorderRegion {
    Top,
    Bottom,
    Left,
    Right,
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

    /// Consumes the canvas and returns the raw RGBA pixel buffer.
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    /// Sets a single pixel at `(x, y)` to the given color.
    pub fn set_pixel(&mut self, x: u32, y: u32, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        self.pixels[offset] = color.r;
        self.pixels[offset + 1] = color.g;
        self.pixels[offset + 2] = color.b;
        self.pixels[offset + 3] = color.a;
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

    /// Fills a rectangle with rounded corners. Each corner radius (tl, tr, br, bl) is in pixels.
    /// Pixels outside the rounded corners are not drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_rounded_rect(
        &mut self,
        rect: Rect,
        color: Color,
        tl: f32,
        tr: f32,
        br: f32,
        bl: f32,
        clip: Option<Rect>,
    ) {
        if color.a == 0 {
            return;
        }
        let Some(area) = normalize_rect(rect) else {
            return;
        };

        // クリップ矩形とのインターセクションを求める（描画範囲の制限のみ、形状は保持）
        let clip_area = clip.and_then(normalize_rect);

        let x0 = area.x.floor().max(0.0) as i32;
        let y0 = area.y.floor().max(0.0) as i32;
        let x1 = (area.x + area.width).ceil().min(self.width as f32) as i32;
        let y1 = (area.y + area.height).ceil().min(self.height as f32) as i32;

        let rx = area.x;
        let ry = area.y;
        let rw = area.width;
        let rh = area.height;

        // 各コーナーの半径を矩形の半分に収める
        let tl = tl.min(rw / 2.0).min(rh / 2.0).max(0.0);
        let tr = tr.min(rw / 2.0).min(rh / 2.0).max(0.0);
        let br = br.min(rw / 2.0).min(rh / 2.0).max(0.0);
        let bl = bl.min(rw / 2.0).min(rh / 2.0).max(0.0);

        for py in y0..y1 {
            for px in x0..x1 {
                let fx = px as f32 + 0.5;
                let fy = py as f32 + 0.5;

                // クリップチェック（ピクセル中心を基準に判定）
                if let Some(ca) = clip_area {
                    if fx < ca.x
                        || fx >= ca.x + ca.width
                        || fy < ca.y
                        || fy >= ca.y + ca.height
                    {
                        continue;
                    }
                }

                // ピクセル中心が角丸矩形の内側かどうか判定
                if !point_in_rounded_rect(fx, fy, rx, ry, rw, rh, tl, tr, br, bl) {
                    continue;
                }

                let index = ((py as u32 * self.width + px as u32) * 4) as usize;
                blend_pixel(&mut self.pixels[index..index + 4], color);
            }
        }
    }

    /// 角丸ボーダーの1辺を描画する。
    /// outer_rect/outer_radii: ボーダー外枠（border_box）の角丸形状
    /// inner_rect/inner_radii: ボーダー内枠（padding_box）の角丸形状
    /// border_region と border_width: どの辺をどの幅で描画するか
    /// 「outer の内側かつ inner の外側」のピクセルのみ color で塗る。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn fill_rounded_rect_annulus(
        &mut self,
        outer_rect: Rect,
        outer_tl: f32,
        outer_tr: f32,
        outer_br: f32,
        outer_bl: f32,
        inner_rect: Rect,
        inner_tl: f32,
        inner_tr: f32,
        inner_br: f32,
        inner_bl: f32,
        color: Color,
        clip: Option<Rect>,
        region: BorderRegion,
        border_width: f32,
    ) {
        if color.a == 0 {
            return;
        }
        let Some(outer) = normalize_rect(outer_rect) else {
            return;
        };

        let clip_area = clip.and_then(normalize_rect);

        // 辺ごとに描画範囲を絞る（Left/Right はフル高さ、コーナーは Top/Bottom が後から上書き）
        let strip = match region {
            BorderRegion::Top => Rect {
                x: outer.x,
                y: outer.y,
                width: outer.width,
                height: border_width,
            },
            BorderRegion::Bottom => Rect {
                x: outer.x,
                y: outer.y + outer.height - border_width,
                width: outer.width,
                height: border_width,
            },
            BorderRegion::Left => Rect {
                x: outer.x,
                y: outer.y,
                width: border_width,
                height: outer.height,
            },
            BorderRegion::Right => Rect {
                x: outer.x + outer.width - border_width,
                y: outer.y,
                width: border_width,
                height: outer.height,
            },
        };
        let Some(strip) = normalize_rect(strip) else {
            return;
        };

        let outer_tl = outer_tl.min(outer.width / 2.0).min(outer.height / 2.0).max(0.0);
        let outer_tr = outer_tr.min(outer.width / 2.0).min(outer.height / 2.0).max(0.0);
        let outer_br = outer_br.min(outer.width / 2.0).min(outer.height / 2.0).max(0.0);
        let outer_bl = outer_bl.min(outer.width / 2.0).min(outer.height / 2.0).max(0.0);

        let inner = inner_rect;
        let inner_tl = inner_tl.min(inner.width / 2.0).min(inner.height / 2.0).max(0.0);
        let inner_tr = inner_tr.min(inner.width / 2.0).min(inner.height / 2.0).max(0.0);
        let inner_br = inner_br.min(inner.width / 2.0).min(inner.height / 2.0).max(0.0);
        let inner_bl = inner_bl.min(inner.width / 2.0).min(inner.height / 2.0).max(0.0);

        let x0 = strip.x.floor().max(0.0) as i32;
        let y0 = strip.y.floor().max(0.0) as i32;
        let x1 = (strip.x + strip.width).ceil().min(self.width as f32) as i32;
        let y1 = (strip.y + strip.height).ceil().min(self.height as f32) as i32;

        for py in y0..y1 {
            for px in x0..x1 {
                let fx = px as f32 + 0.5;
                let fy = py as f32 + 0.5;

                // クリップチェック（ピクセル中心を基準に判定）
                if let Some(ca) = clip_area {
                    if fx < ca.x
                        || fx >= ca.x + ca.width
                        || fy < ca.y
                        || fy >= ca.y + ca.height
                    {
                        continue;
                    }
                }

                // outer の内側かつ inner の外側
                if !point_in_rounded_rect(fx, fy, outer.x, outer.y, outer.width, outer.height, outer_tl, outer_tr, outer_br, outer_bl) {
                    continue;
                }
                if point_in_rounded_rect(fx, fy, inner.x, inner.y, inner.width, inner.height, inner_tl, inner_tr, inner_br, inner_bl) {
                    continue;
                }

                let index = ((py as u32 * self.width + px as u32) * 4) as usize;
                blend_pixel(&mut self.pixels[index..index + 4], color);
            }
        }
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

    pub(crate) fn fill_rect_clipped(&mut self, rect: Rect, color: Color, clip: Option<Rect>) {
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

    pub(crate) fn draw_image_scaled_clipped(&mut self, image: &Image, destination: Rect, clip: Option<Rect>) {
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
    pub(crate) fn draw_glyph_mask(
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
    let text_fonts = text::load_text_fonts();
    paint_layout_with_fonts(layout, resolver, viewport, text_fonts)
}

/// Paints a layout tree using the provided font list for text rendering.
///
/// Web fonts should be placed before system fonts in the list for priority.
pub fn paint_layout_with_fonts(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    fonts: Vec<Font>,
) -> Canvas {
    paint_layout_with_web_fonts(layout, resolver, viewport, fonts, None)
}

/// Paints a layout tree using the provided font list and an optional web font registry.
///
/// When `web_fonts` is `Some`, per-fragment font-weight and font-style are used to
/// select the best variant from the registry before falling back to `fonts`.
pub fn paint_layout_with_web_fonts(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    fonts: Vec<Font>,
    web_fonts: Option<&WebFontRegistry>,
) -> Canvas {
    let width = viewport.width.ceil().max(1.0) as u32;
    let height = viewport.height.ceil().max(1.0) as u32;
    let mut canvas = Canvas::new(width, height);
    if let Some(background) = viewport_background_color(layout, resolver) {
        canvas.fill_rect(viewport, background);
    }
    paint_box(&mut canvas, layout, resolver, None, viewport, &fonts, web_fonts);
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
    let effective_base = stylesheet::extract_document_base_url(document, base_url);
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(viewport.width, viewport.height);

    let mut parsed_sheets = Vec::new();
    for css_text in stylesheet::extract_author_stylesheets(document, base_url)? {
        let sheet = stylesheet::parse_stylesheet_forgiving(&css_text);
        parsed_sheets.push(sheet);
    }
    for sheet in &parsed_sheets {
        resolver.add_stylesheet(Origin::Author, sheet.clone());
    }

    // Collect @font-face rules and fetch web fonts, building a variant registry
    let fetched_web_fonts = stylesheet::fetch_font_face_fonts(&parsed_sheets, effective_base.as_ref());

    let mut web_font_registry = WebFontRegistry::new();
    for wf in fetched_web_fonts {
        web_font_registry.push(&wf.family, wf.weight, wf.style, wf.font);
    }

    // Build combined system font list for glyph fallback
    let all_fonts = text::load_text_fonts();

    // Avoid passing an empty registry to skip unnecessary lookups.
    let web_font_registry_opt = if web_font_registry.is_empty() {
        None
    } else {
        Some(&web_font_registry)
    };

    // Execute <script> tags and fire DOMContentLoaded before layout.
    // JS may modify the DOM (e.g., classList.add for fade-in animations,
    // injecting <style> elements), so this must happen before layout.
    if let Ok(mut runtime) = crate::js::JsRuntime::with_document(document.clone()) {
        // Keep getComputedStyle / layout-metric queries issued by page scripts
        // consistent with the render viewport.
        runtime.set_viewport(viewport.width, viewport.height);
        let errors = runtime.execute_document_scripts(effective_base.as_ref());
        for err in &errors {
            eprintln!("[omoikane][js-error] {err}");
        }
        // Wire `on*` inline handlers (e.g. <body onload>) and fire the `load`
        // event. Per the HTML load order this happens after scripts run and
        // DOMContentLoaded has fired (inside execute_document_scripts), so page
        // load handlers run before the timer pump advances virtual time.
        if let Err(err) = runtime.wire_inline_event_handlers() {
            eprintln!("[omoikane][js-error] {err}");
        }
        if let Err(err) = runtime.fire_load() {
            eprintln!("[omoikane][js-error] {err}");
        }
        // Drive script-scheduled timers (setTimeout/setInterval) in virtual
        // time so that DOM mutations from deferred callbacks settle before
        // layout. Bounded by a virtual-time budget and a task-count cap so an
        // infinite setInterval cannot hang the render.
        runtime.run_timers(
            TIMER_PUMP_MAX_VIRTUAL_MS,
            TIMER_PUMP_STEP_MS,
            TIMER_PUMP_MAX_TASKS,
        );
        // Re-extract stylesheets and rebuild resolver after JS may have
        // modified the DOM (inserted/removed <style>/<link> elements).
        resolver = StyleResolver::new();
        resolver.set_viewport(viewport.width, viewport.height);
        parsed_sheets.clear();
        if let Ok(css_texts) = stylesheet::extract_author_stylesheets(document, base_url) {
            for css_text in css_texts {
                let sheet = stylesheet::parse_stylesheet_forgiving(&css_text);
                parsed_sheets.push(sheet);
            }
        }
        for sheet in &parsed_sheets {
            resolver.add_stylesheet(Origin::Author, sheet.clone());
        }
    }

    crate::layout::with_image_base_url(effective_base, || {
        let layout = crate::layout::layout_tree(document, &mut resolver, viewport)?;
        Some(paint_layout_with_web_fonts(
            &layout,
            &mut resolver,
            viewport,
            all_fonts,
            web_font_registry_opt,
        ))
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
    stylesheet::materialize_local_assets(document, base_path)?;
    render_document(document, viewport)
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
    web_fonts: Option<&WebFontRegistry>,
) {
    paint_box_internal(
        canvas,
        layout,
        resolver,
        inherited_clip,
        viewport,
        true,
        text_fonts,
        web_fonts,
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
    web_fonts: Option<&WebFontRegistry>,
) {
    if layout.visibility == Visibility::Hidden {
        return;
    }

    let style = resolver.computed_style(&layout.node);
    let border_box = border_box_rect(layout);
    let padding_box = padding_box_rect(layout);

    // opacity が 1.0 未満の場合、オフスクリーンバッファに描画してから合成する
    let opacity = element_opacity(&style);
    let needs_offscreen = opacity.is_some_and(|v| v < 1.0);

    if needs_offscreen {
        let opacity_value = opacity.unwrap_or(1.0);
        // オフスクリーンバッファはキャンバス全体サイズで作成する。
        // border_box に限定すると、子孫要素が border_box 外にはみ出した場合（例: overflow: visible の
        // 子孫や box-shadow）に正しく合成できなくなるため、キャンバス全体を使う必要がある。
        let buf_x = 0i32;
        let buf_y = 0i32;
        let buf_w = canvas.width();
        let buf_h = canvas.height();
        if buf_w == 0 || buf_h == 0 {
            return;
        }
        // オフスクリーンバッファはキャンバス全体と同一座標系（原点は (0, 0)）
        let offset_border_box = Rect {
            x: border_box.x - buf_x as f32,
            y: border_box.y - buf_y as f32,
            width: border_box.width,
            height: border_box.height,
        };
        let offset_padding_box = Rect {
            x: padding_box.x - buf_x as f32,
            y: padding_box.y - buf_y as f32,
            width: padding_box.width,
            height: padding_box.height,
        };
        let offset_inherited_clip = inherited_clip.map(|c| Rect {
            x: c.x - buf_x as f32,
            y: c.y - buf_y as f32,
            width: c.width,
            height: c.height,
        });
        let offset_viewport = Rect {
            x: viewport.x - buf_x as f32,
            y: viewport.y - buf_y as f32,
            width: viewport.width,
            height: viewport.height,
        };
        let mut offscreen = Canvas::new(buf_w, buf_h);
        paint_box_internal_to(
            &mut offscreen,
            layout,
            resolver,
            offset_inherited_clip,
            offset_viewport,
            include_phase_descendants,
            text_fonts,
            web_fonts,
            &style,
            offset_border_box,
            offset_padding_box,
        );
        offscreen.multiply_alpha(opacity_value);
        // メインキャンバスに合成
        let dst_w = canvas.width() as i32;
        let dst_h = canvas.height() as i32;
        let src_w = buf_w as i32;
        let src_h = buf_h as i32;
        for sy in 0..src_h {
            let dy = buf_y + sy;
            if dy < 0 || dy >= dst_h {
                continue;
            }
            for sx in 0..src_w {
                let dx = buf_x + sx;
                if dx < 0 || dx >= dst_w {
                    continue;
                }
                let src_idx = (sy * src_w + sx) as usize * 4;
                let a = offscreen.pixels[src_idx + 3];
                if a == 0 {
                    continue;
                }
                let color = Color {
                    r: offscreen.pixels[src_idx],
                    g: offscreen.pixels[src_idx + 1],
                    b: offscreen.pixels[src_idx + 2],
                    a,
                };
                let dst_idx = (dy * dst_w + dx) as usize * 4;
                blend_pixel(&mut canvas.pixels[dst_idx..dst_idx + 4], color);
            }
        }
        return;
    }

    paint_box_internal_to(
        canvas,
        layout,
        resolver,
        inherited_clip,
        viewport,
        include_phase_descendants,
        text_fonts,
        web_fonts,
        &style,
        border_box,
        padding_box,
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_box_internal_to(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    inherited_clip: Option<Rect>,
    viewport: Rect,
    include_phase_descendants: bool,
    text_fonts: &[Font],
    web_fonts: Option<&WebFontRegistry>,
    style: &ComputedStyle,
    border_box: Rect,
    padding_box: Rect,
) {
    // box-shadow を背景より前（下）に描画する
    border::paint_box_shadow(canvas, style, border_box, inherited_clip);

    if let Some(background) = background_color(style) {
        if has_border_radius(style) {
            let (tl, tr, br, bl) = border_radius_corners(style);
            canvas.fill_rounded_rect(border_box, background, tl, tr, br, bl, inherited_clip);
        } else {
            canvas.fill_rect_clipped(border_box, background, inherited_clip);
        }
    }
    paint_background_image(canvas, style, border_box, inherited_clip, viewport);
    paint_block_generated_pseudo_box(
        canvas,
        layout,
        resolver,
        PseudoElement::Before,
        inherited_clip,
        viewport,
    );

    border::paint_borders(canvas, layout, style, inherited_clip);

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
        paint_box_internal(canvas, child, resolver, clip, viewport, true, text_fonts, web_fonts);
    }
    for child in normal_block_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, false, text_fonts, web_fonts);
    }
    for child in float_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, true, text_fonts, web_fonts);
    }
    text::paint_text_with_registry(canvas, layout, style, clip, viewport, text_fonts, web_fonts);
    text::paint_list_marker(canvas, layout, style, clip, text_fonts);
    for child in inline_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, false, text_fonts, web_fonts);
    }
    for child in auto_positioned_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, true, text_fonts, web_fonts);
    }
    for child in positive_positioned_children {
        paint_box_internal(canvas, child, resolver, clip, viewport, true, text_fonts, web_fonts);
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

    let border = border::EdgeSizesForPaint::from_style(style);
    if border.total_horizontal() == 0.0 && border.total_vertical() == 0.0 {
        return;
    }

    if rect.width == border.total_horizontal() && rect.height == border.total_vertical() {
        border::paint_zero_sized_border_box(canvas, rect, style, border, clip);
        return;
    }

    border::paint_rect_borders(canvas, rect, style, border, clip);
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

    let border = border::EdgeSizesForPaint::from_style(&style);
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
        Some(ComputedValue::Keyword(keyword)) => image::parse_background_image_value(keyword),
        Some(ComputedValue::String(value)) => image::parse_background_image_value(value),
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

/// Returns background-position as `(x, y)` pixel offsets.
///
/// Supports px values, percentage values, and keywords (center/left/right/top/bottom).
/// For percentages: `position = (container_size - image_size) * percentage / 100`
fn background_position(
    style: &ComputedStyle,
    container_w: f32,
    container_h: f32,
    image_w: f32,
    image_h: f32,
) -> (f32, f32) {
    let x = resolve_bg_position(style, "background-position-x", container_w, image_w);
    let y = resolve_bg_position(style, "background-position-y", container_h, image_h);
    (x, y)
}

fn resolve_bg_position(
    style: &ComputedStyle,
    property: &str,
    container_size: f32,
    image_size: f32,
) -> f32 {
    let is_x = property.ends_with("-x");
    match style.get(property) {
        Some(ComputedValue::Px(v)) => *v,
        Some(ComputedValue::Number(v)) if *v == 0.0 => 0.0,
        Some(ComputedValue::Percentage(p)) => (container_size - image_size) * p / 100.0,
        Some(ComputedValue::Keyword(k)) => {
            match k.to_ascii_lowercase().as_str() {
                "center" => (container_size - image_size) * 0.5,
                "right" if is_x => container_size - image_size,
                "left" if is_x => 0.0,
                "bottom" if !is_x => container_size - image_size,
                "top" if !is_x => 0.0,
                _ => 0.0,
            }
        }
        _ => 0.0,
    }
}

fn background_size(style: &ComputedStyle, area: Rect, image_w: f32, image_h: f32) -> (f32, f32) {
    image::background_size(style, area, image_w, image_h)
}

fn border_color(style: &ComputedStyle) -> Option<Color> {
    resolve_color_value(style.get("border-color"), style)
        .or_else(|| color_property(style.get("color")))
}

fn color_property(value: Option<&ComputedValue>) -> Option<Color> {
    match value {
        Some(ComputedValue::Color(color)) => parse_color(color),
        Some(ComputedValue::Keyword(color)) => parse_color(color),
        _ => None,
    }
}

/// Resolves a color value, handling `currentColor` by looking up the element's `color` property.
pub(crate) fn resolve_color_value(
    value: Option<&ComputedValue>,
    style: &ComputedStyle,
) -> Option<Color> {
    match value {
        Some(ComputedValue::Color(c)) if c.eq_ignore_ascii_case("currentcolor") => {
            color_property(style.get("color"))
        }
        Some(ComputedValue::Keyword(k)) if k.eq_ignore_ascii_case("currentcolor") => {
            color_property(style.get("color"))
        }
        _ => color_property(value),
    }
}

fn length_property(style: &ComputedStyle, name: &str) -> Option<f32> {
    match style.get(name) {
        Some(ComputedValue::Px(value)) => Some(*value),
        Some(ComputedValue::Number(value)) => Some(*value),
        _ => None,
    }
}

/// スタイルから各コーナーの border-radius を返す（TL, TR, BR, BL）。
fn border_radius_corners(style: &ComputedStyle) -> (f32, f32, f32, f32) {
    let tl = length_property(style, "border-top-left-radius").unwrap_or(0.0);
    let tr = length_property(style, "border-top-right-radius").unwrap_or(0.0);
    let br = length_property(style, "border-bottom-right-radius").unwrap_or(0.0);
    let bl = length_property(style, "border-bottom-left-radius").unwrap_or(0.0);
    (tl, tr, br, bl)
}

/// border-radius が設定されているか確認する。
fn has_border_radius(style: &ComputedStyle) -> bool {
    let (tl, tr, br, bl) = border_radius_corners(style);
    tl > 0.0 || tr > 0.0 || br > 0.0 || bl > 0.0
}

/// `opacity` プロパティの値を返す（0.0〜1.0）。未指定の場合は `None`。
/// `with_force_opacity` が有効な場合、0.0 の値は 1.0 に上書きされる。
fn element_opacity(style: &ComputedStyle) -> Option<f32> {
    let value = match style.get("opacity") {
        Some(ComputedValue::Number(v)) => Some(v.clamp(0.0, 1.0)),
        Some(ComputedValue::Px(v)) => Some(v.clamp(0.0, 1.0)),
        Some(ComputedValue::Keyword(k)) if k == "1" || k == "1.0" => Some(1.0),
        _ => None,
    };
    if value == Some(0.0) && force_opacity_enabled() {
        return Some(1.0);
    }
    value
}

fn normalize_rect(rect: Rect) -> Option<Rect> {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        None
    } else {
        Some(rect)
    }
}

/// 点 (px, py) が角丸矩形の内側にあるか判定する。
/// 矩形の左上 (rx, ry)、サイズ (rw, rh)、各コーナー半径 (tl, tr, br, bl)。
#[allow(clippy::too_many_arguments)]
fn point_in_rounded_rect(
    px: f32,
    py: f32,
    rx: f32,
    ry: f32,
    rw: f32,
    rh: f32,
    tl: f32,
    tr: f32,
    br: f32,
    bl: f32,
) -> bool {
    // 矩形の外側は除外
    if px < rx || px > rx + rw || py < ry || py > ry + rh {
        return false;
    }

    // 左上コーナー
    if px < rx + tl && py < ry + tl {
        let dx = px - (rx + tl);
        let dy = py - (ry + tl);
        return dx * dx + dy * dy <= tl * tl;
    }
    // 右上コーナー
    if px > rx + rw - tr && py < ry + tr {
        let dx = px - (rx + rw - tr);
        let dy = py - (ry + tr);
        return dx * dx + dy * dy <= tr * tr;
    }
    // 右下コーナー
    if px > rx + rw - br && py > ry + rh - br {
        let dx = px - (rx + rw - br);
        let dy = py - (ry + rh - br);
        return dx * dx + dy * dy <= br * br;
    }
    // 左下コーナー
    if px < rx + bl && py > ry + rh - bl {
        let dx = px - (rx + bl);
        let dy = py - (ry + rh - bl);
        return dx * dx + dy * dy <= bl * bl;
    }

    true
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

fn paint_background_image(
    canvas: &mut Canvas,
    style: &ComputedStyle,
    rect: Rect,
    clip: Option<Rect>,
    viewport: Rect,
) {
    let Some(area) = normalize_rect(rect) else {
        return;
    };

    // Check if the background-image is a linear-gradient
    let bg_image_value = style.get("background-image");
    let raw_bg_str: Option<String> = match bg_image_value {
        Some(ComputedValue::Keyword(kw)) => Some(kw.clone()),
        Some(ComputedValue::String(s)) => Some(s.clone()),
        _ => None,
    };
    let gradient_str = raw_bg_str.as_deref().and_then(|s| {
        let lower = s.trim_start().to_ascii_lowercase();
        if lower.starts_with("linear-gradient(") {
            Some(s.trim_start())
        } else {
            None
        }
    });

    if let Some(gradient_str) = gradient_str {
        // Normalise to lowercase for parsing (function names are case-insensitive in CSS)
        let normalised = gradient_str.to_ascii_lowercase();
        if let Some(gradient) = color::parse_linear_gradient(&normalised) {
            // Check if a background-size is set to tile the gradient
            let has_explicit_size = match style.get("background-size") {
                None => false,
                Some(ComputedValue::Keyword(kw)) => !kw.eq_ignore_ascii_case("auto"),
                _ => true,
            };
            if has_explicit_size {
                // Gradient with explicit tile size — render into an offscreen tile buffer once,
                // then blit (draw_image_scaled_clipped) for each repeated position.
                // This avoids per-pixel gradient computation for every tile copy.
                let (tile_w, tile_h) =
                    background_size(style, area, area.width, area.height);
                let tile_w = tile_w.max(1.0);
                let tile_h = tile_h.max(1.0);
                let repeat = background_repeat(style);
                let fixed = background_attachment_fixed(style);
                let (pos_cw, pos_ch) = if fixed { (viewport.width, viewport.height) } else { (area.width, area.height) };
                let (position_x, position_y) = background_position(style, pos_cw, pos_ch, tile_w, tile_h);
                let anchor_x = if fixed { viewport.x + position_x } else { area.x + position_x };
                let anchor_y = if fixed { viewport.y + position_y } else { area.y + position_y };
                let x_end = area.x + area.width;
                let y_end = area.y + area.height;

                // Render one tile at origin (0,0) into an offscreen canvas.
                // Guard with a maximum pixel budget to avoid OOM on huge background-size.
                const MAX_TILE_PIXELS: u64 = 16_777_216; // 4096 x 4096
                let tile_image = if repeat {
                    let tw = tile_w.ceil().max(1.0) as u32;
                    let th = tile_h.ceil().max(1.0) as u32;
                    let pixels = tw as u64 * th as u64;
                    if pixels <= MAX_TILE_PIXELS && tw > 0 && th > 0 {
                        let mut tile_canvas = Canvas::new(tw, th);
                        let origin_rect = Rect { x: 0.0, y: 0.0, width: tile_w, height: tile_h };
                        color::paint_linear_gradient(&mut tile_canvas, &gradient, origin_rect, None);
                        Image::new(tw, th, tile_canvas.pixels).ok()
                    } else {
                        None // tile too large, fall back to per-tile rendering
                    }
                } else {
                    None
                };

                let mut ty = if repeat {
                    anchor_y + ((area.y - anchor_y) / tile_h).floor() * tile_h
                } else {
                    anchor_y
                };
                while ty < y_end {
                    let mut tx = if repeat {
                        anchor_x + ((area.x - anchor_x) / tile_w).floor() * tile_w
                    } else {
                        anchor_x
                    };
                    while tx < x_end {
                        let tile_rect = Rect { x: tx, y: ty, width: tile_w, height: tile_h };
                        if let Some(ref img) = tile_image {
                            // Fast path: blit pre-rendered tile buffer
                            canvas.draw_image_scaled_clipped(img, tile_rect, clip.or(Some(area)));
                        } else {
                            // Fallback (no-repeat, tile too large, or image validation failed): render directly
                            color::paint_linear_gradient(canvas, &gradient, tile_rect, clip.or(Some(area)));
                        }
                        if !repeat {
                            return;
                        }
                        tx += tile_w;
                    }
                    ty += tile_h;
                }
            } else {
                // Default: gradient fills the entire area
                color::paint_linear_gradient(canvas, &gradient, area, clip.or(Some(area)));
            }
            return;
        }
    }

    // Regular image
    let Some(image) = background_image(style) else {
        return;
    };

    let (tile_width, tile_height) =
        background_size(style, area, image.width().max(1) as f32, image.height().max(1) as f32);
    let tile_width = tile_width.max(1.0);
    let tile_height = tile_height.max(1.0);
    let x_end = area.x + area.width;
    let y_end = area.y + area.height;
    let repeat = background_repeat(style);
    let fixed = background_attachment_fixed(style);
    let (pos_container_w, pos_container_h) = if fixed {
        (viewport.width, viewport.height)
    } else {
        (area.width, area.height)
    };
    let (position_x, position_y) = background_position(style, pos_container_w, pos_container_h, tile_width, tile_height);
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
            let dest = Rect {
                x,
                y,
                width: tile_width,
                height: tile_height,
            };
            canvas.draw_image_scaled_clipped(&image, dest, clip.or(Some(area)));
            if !repeat {
                return;
            }
            x += tile_width;
        }
        y += tile_height;
    }
}

impl Canvas {
    /// alpha チャンネルにのみ box blur を適用する（色成分は影の色のまま、アルファだけ拡散）。
    ///
    /// カーネルは常に `2r+1` ピクセル幅だが、端ではクランプして実効カーネルサイズを動的に計算する。
    pub(crate) fn box_blur_alpha(&mut self, radius: u32) {
        if radius == 0 {
            return;
        }
        let w = self.width as usize;
        let h = self.height as usize;
        let r = radius as usize;

        // 水平方向 blur
        let mut alphas: Vec<u8> = self.pixels.iter().skip(3).step_by(4).copied().collect();
        let mut blurred = vec![0u8; w * h];
        for y in 0..h {
            let row_start = y * w;
            let mut sum: u32 = 0;
            // x=0 の初期ウィンドウ: [max(0, 0-r), min(w-1, 0+r)] = [0, min(r, w-1)]
            let init_right = r.min(w.saturating_sub(1));
            for x in 0..=init_right {
                sum += alphas[row_start + x] as u32;
            }
            for x in 0..w {
                // 実効カーネルサイズ: min(w-1, x+r) - max(0, x-r) + 1
                let left = x.saturating_sub(r);
                let right = (x + r).min(w.saturating_sub(1));
                let kernel_size = (right - left + 1) as u32;
                blurred[row_start + x] = (sum / kernel_size) as u8;
                // ウィンドウを右にずらす: 右端を追加、左端を除去
                if x + r + 1 < w {
                    sum += alphas[row_start + x + r + 1] as u32;
                }
                if x >= r {
                    sum = sum.saturating_sub(alphas[row_start + x - r] as u32);
                }
            }
        }

        // 垂直方向 blur
        alphas = blurred.clone();
        for x in 0..w {
            let mut sum: u32 = 0;
            // y=0 の初期ウィンドウ
            let init_bottom = r.min(h.saturating_sub(1));
            for y in 0..=init_bottom {
                sum += alphas[y * w + x] as u32;
            }
            for y in 0..h {
                let top = y.saturating_sub(r);
                let bottom = (y + r).min(h.saturating_sub(1));
                let kernel_size = (bottom - top + 1) as u32;
                blurred[y * w + x] = (sum / kernel_size) as u8;
                if y + r + 1 < h {
                    sum += alphas[(y + r + 1) * w + x] as u32;
                }
                if y >= r {
                    sum = sum.saturating_sub(alphas[(y - r) * w + x] as u32);
                }
            }
        }

        // blurred alpha をピクセルに書き戻す
        for (i, &a) in blurred.iter().enumerate() {
            self.pixels[i * 4 + 3] = a;
        }
    }

    /// 別キャンバス（shadow_buf）をメインキャンバスに合成する（clip あり）。
    /// `r`, `g`, `b` は合成時に使う色成分（影の色）。
    /// `offset_x`, `offset_y` は shadow_buf の左上隅がメインキャンバスのどこに対応するか。
    pub(crate) fn composite_canvas_clipped(&mut self, src: &Canvas, offset_x: i32, offset_y: i32, r: u8, g: u8, b: u8, clip: Option<Rect>) {
        let dst_w = self.width as i32;
        let dst_h = self.height as i32;
        let src_w = src.width as i32;
        let src_h = src.height as i32;

        let clip_area = clip.and_then(normalize_rect);

        for sy in 0..src_h {
            let dy = offset_y + sy;
            if dy < 0 || dy >= dst_h {
                continue;
            }
            for sx in 0..src_w {
                let dx = offset_x + sx;
                if dx < 0 || dx >= dst_w {
                    continue;
                }

                // clip チェック
                if let Some(ca) = clip_area {
                    let fx = dx as f32 + 0.5;
                    let fy = dy as f32 + 0.5;
                    if fx < ca.x || fx >= ca.x + ca.width || fy < ca.y || fy >= ca.y + ca.height {
                        continue;
                    }
                }

                let src_idx = (sy * src_w + sx) as usize * 4;
                let src_a = src.pixels[src_idx + 3];
                if src_a == 0 {
                    continue;
                }
                let color = Color { r, g, b, a: src_a };
                let dst_idx = (dy * dst_w + dx) as usize * 4;
                blend_pixel(&mut self.pixels[dst_idx..dst_idx + 4], color);
            }
        }
    }

    /// キャンバスの全ピクセルの alpha に `factor` (0.0〜1.0) を乗算する。
    pub fn multiply_alpha(&mut self, factor: f32) {
        let factor = factor.clamp(0.0, 1.0);
        for pixel in self.pixels.chunks_exact_mut(4) {
            let a = pixel[3] as f32;
            pixel[3] = (a * factor).round() as u8;
        }
    }
}

#[cfg(test)]
mod tests;
