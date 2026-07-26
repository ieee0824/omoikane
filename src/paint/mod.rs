//! Pixel-based painting primitives and layout tree rendering.

pub(crate) mod border;
pub(crate) mod color;
pub(crate) mod image;
pub(crate) mod stylesheet;
pub(crate) mod text;

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Total virtual milliseconds the render pipeline advances the JS event loop to
/// drain script-scheduled timers before layout.
const TIMER_PUMP_MAX_VIRTUAL_MS: u64 = 10_000;
/// Virtual-time increment per event-loop step while pumping timers.
const TIMER_PUMP_STEP_MS: u64 = 10;
/// Hard cap on the number of timer tasks executed while pumping, guarding
/// against callbacks that endlessly re-schedule zero-delay timers.
const TIMER_PUMP_MAX_TASKS: usize = 100_000;
/// Maximum rendering opportunities driven before a static screenshot. This
/// settles short requestAnimationFrame initialization chains without allowing
/// a perpetual animation loop to block rendering.
const ANIMATION_FRAME_PUMP_MAX_FRAMES: usize = 8;
/// Nominal headless refresh interval used for animation-frame timestamps.
const ANIMATION_FRAME_INTERVAL_MS: u64 = 16;

thread_local! {
    static FORCE_OPACITY: Cell<bool> = const { Cell::new(false) };
    static LAST_RENDER_TIMINGS: RefCell<RenderTimings> = RefCell::new(RenderTimings::default());
}

/// Processing time spent in each stage of the most recent screenshot render.
///
/// Multiple documents rendered for one screenshot are accumulated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderTimings {
    pub stylesheets: Duration,
    pub fonts: Duration,
    pub javascript: Duration,
    pub javascript_runtime_init: Duration,
    pub javascript_document_scripts: Duration,
    pub javascript_load_events: Duration,
    pub timers: Duration,
    pub animation_frames: Duration,
    pub style_refresh: Duration,
    pub layout: Duration,
    pub paint: Duration,
    pub png_encode: Duration,
}

impl RenderTimings {
    fn add_assign(&mut self, other: &Self) {
        self.stylesheets += other.stylesheets;
        self.fonts += other.fonts;
        self.javascript += other.javascript;
        self.javascript_runtime_init += other.javascript_runtime_init;
        self.javascript_document_scripts += other.javascript_document_scripts;
        self.javascript_load_events += other.javascript_load_events;
        self.timers += other.timers;
        self.animation_frames += other.animation_frames;
        self.style_refresh += other.style_refresh;
        self.layout += other.layout;
        self.paint += other.paint;
        self.png_encode += other.png_encode;
    }
}

/// Takes the accumulated timings for the most recent screenshot render.
pub fn take_last_render_timings() -> RenderTimings {
    LAST_RENDER_TIMINGS.with(|timings| std::mem::take(&mut *timings.borrow_mut()))
}

pub(crate) fn clear_render_timings() {
    LAST_RENDER_TIMINGS.with(|timings| *timings.borrow_mut() = RenderTimings::default());
}

pub(crate) fn record_render_timings(timings: &RenderTimings) {
    LAST_RENDER_TIMINGS.with(|total| total.borrow_mut().add_assign(timings));
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
    AffineTransform, ComputedStyle, ComputedValue, Origin, PseudoElement, StyleResolver,
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
pub(crate) use border::{
    BoxShadow, fill_quad_clipped, has_any_solid_border, paint_borders, paint_box_shadow,
    paint_outer_box_shadow, paint_rect_borders, paint_zero_sized_border_box, parse_box_shadow,
    split_box_shadow_layers,
};
#[allow(unused_imports)]
pub(crate) use border::{EdgeSizesForPaint, border_color_side, has_solid_border_side};
#[allow(unused_imports)]
pub(crate) use color::{
    ColorStop, LinearGradient, interpolate_gradient_color, named_color, paint_linear_gradient,
    parse_color, parse_gradient_direction, parse_linear_gradient, split_gradient_args,
};
#[allow(unused_imports)]
pub(crate) use image::{
    decode_jpeg, decode_png, decode_png_fallback, hex_value, paeth_predictor,
    parse_background_image_value, parse_size_token, percent_decode, unfilter_png_scanline,
};
#[allow(unused_imports)]
pub(crate) use stylesheet::{
    WebFont, at_import_starts_at, collect_author_stylesheets, collect_stylesheet_with_imports,
    collect_text_contents, extract_author_stylesheets, extract_document_base_url,
    extract_import_hrefs, extract_import_hrefs_forgiving, fetch_font_face_fonts,
    fetch_relative_stylesheet, fetch_stylesheet_by_url, find_base_elements, matches_screen_media,
    materialize_local_assets, non_empty_token, normalize_unquoted_urls, parse_import_href,
    parse_stylesheet_forgiving, resolve_relative_stylesheet_url, rewrite_local_asset_attribute,
    salvage_style_rule, same_origin, split_declarations_forgiving, unquote_css_token,
};
#[allow(unused_imports)]
pub(crate) use text::{
    TextDecorationLines, apply_text_transform, inline_fragment_content_rect,
    is_cjk_preferred_character, load_text_fonts, paint_inline_image_fragment, paint_list_marker,
    paint_list_marker_placeholder, paint_text_decoration, paint_text_placeholder,
    paint_text_with_font, paint_text_with_font_refs, paint_text_with_registry,
    rasterize_with_fallback, rasterize_with_fallback_refs, text_color, text_decoration_color,
    text_decoration_line,
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

    /// Decodes the first frame of a GIF image into RGBA pixels.
    pub fn decode_gif(bytes: &[u8]) -> Result<Self, PaintError> {
        image::decode_gif(bytes)
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
    TopLeftCorner,
    TopRightCorner,
    BottomRightCorner,
    BottomLeftCorner,
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
                if let Some(ca) = clip_area
                    && (fx < ca.x || fx >= ca.x + ca.width || fy < ca.y || fy >= ca.y + ca.height)
                {
                    continue;
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

        let outer_tl = outer_tl
            .min(outer.width / 2.0)
            .min(outer.height / 2.0)
            .max(0.0);
        let outer_tr = outer_tr
            .min(outer.width / 2.0)
            .min(outer.height / 2.0)
            .max(0.0);
        let outer_br = outer_br
            .min(outer.width / 2.0)
            .min(outer.height / 2.0)
            .max(0.0);
        let outer_bl = outer_bl
            .min(outer.width / 2.0)
            .min(outer.height / 2.0)
            .max(0.0);

        // Scan narrow edge strips plus corner squares. This keeps thin pill
        // borders proportional to their perimeter instead of scanning the
        // entire border-box area.
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
            BorderRegion::TopLeftCorner => Rect {
                x: outer.x,
                y: outer.y,
                width: outer_tl,
                height: outer_tl,
            },
            BorderRegion::TopRightCorner => Rect {
                x: outer.x + outer.width - outer_tr,
                y: outer.y,
                width: outer_tr,
                height: outer_tr,
            },
            BorderRegion::BottomRightCorner => Rect {
                x: outer.x + outer.width - outer_br,
                y: outer.y + outer.height - outer_br,
                width: outer_br,
                height: outer_br,
            },
            BorderRegion::BottomLeftCorner => Rect {
                x: outer.x,
                y: outer.y + outer.height - outer_bl,
                width: outer_bl,
                height: outer_bl,
            },
        };
        let Some(strip) = normalize_rect(strip) else {
            return;
        };

        let inner = inner_rect;
        let inner_tl = inner_tl
            .min(inner.width / 2.0)
            .min(inner.height / 2.0)
            .max(0.0);
        let inner_tr = inner_tr
            .min(inner.width / 2.0)
            .min(inner.height / 2.0)
            .max(0.0);
        let inner_br = inner_br
            .min(inner.width / 2.0)
            .min(inner.height / 2.0)
            .max(0.0);
        let inner_bl = inner_bl
            .min(inner.width / 2.0)
            .min(inner.height / 2.0)
            .max(0.0);

        let x0 = strip.x.floor().max(0.0) as i32;
        let y0 = strip.y.floor().max(0.0) as i32;
        let x1 = (strip.x + strip.width).ceil().min(self.width as f32) as i32;
        let y1 = (strip.y + strip.height).ceil().min(self.height as f32) as i32;

        for py in y0..y1 {
            for px in x0..x1 {
                let fx = px as f32 + 0.5;
                let fy = py as f32 + 0.5;

                // クリップチェック（ピクセル中心を基準に判定）
                if let Some(ca) = clip_area
                    && (fx < ca.x || fx >= ca.x + ca.width || fy < ca.y || fy >= ca.y + ca.height)
                {
                    continue;
                }

                // outer の内側かつ inner の外側
                if !point_in_rounded_rect(
                    fx,
                    fy,
                    outer.x,
                    outer.y,
                    outer.width,
                    outer.height,
                    outer_tl,
                    outer_tr,
                    outer_br,
                    outer_bl,
                ) {
                    continue;
                }
                if point_in_rounded_rect(
                    fx,
                    fy,
                    inner.x,
                    inner.y,
                    inner.width,
                    inner.height,
                    inner_tl,
                    inner_tr,
                    inner_br,
                    inner_bl,
                ) {
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

    /// Composites every pixel over an opaque background in place.
    ///
    /// Layout canvases retain transparency for embedding and paint tests, while
    /// browser screenshots use this before encoding to match the opaque page
    /// surface produced by desktop browsers.
    pub(crate) fn composite_over(&mut self, background: Color) {
        for pixel in self.pixels.chunks_exact_mut(4) {
            let alpha = u32::from(pixel[3]);
            let inverse = 255 - alpha;
            pixel[0] =
                ((u32::from(pixel[0]) * alpha + u32::from(background.r) * inverse) / 255) as u8;
            pixel[1] =
                ((u32::from(pixel[1]) * alpha + u32::from(background.g) * inverse) / 255) as u8;
            pixel[2] =
                ((u32::from(pixel[2]) * alpha + u32::from(background.b) * inverse) / 255) as u8;
            pixel[3] = 255;
        }
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

        if x0 >= x1 || y0 >= y1 {
            return;
        }

        if color.a == 255 {
            let row_start = x0 as usize * 4;
            let row_end = x1 as usize * 4;
            let stride = self.width as usize * 4;
            let rgba = [color.r, color.g, color.b, color.a];
            for y in y0 as usize..y1 as usize {
                for pixel in
                    self.pixels[y * stride + row_start..y * stride + row_end].chunks_exact_mut(4)
                {
                    pixel.copy_from_slice(&rgba);
                }
            }
            return;
        }

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

    pub(crate) fn draw_image_scaled_clipped(
        &mut self,
        image: &Image,
        destination: Rect,
        clip: Option<Rect>,
    ) {
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
    fonts: Vec<Arc<Font>>,
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
    fonts: Vec<Arc<Font>>,
    web_fonts: Option<&WebFontRegistry>,
) -> Canvas {
    let width = viewport.width.ceil().max(1.0) as u32;
    let height = viewport.height.ceil().max(1.0) as u32;
    let mut canvas = Canvas::new(width, height);
    if let Some(background) = viewport_background_color(layout, resolver) {
        canvas.fill_rect(viewport, background);
    }
    paint_box(
        &mut canvas,
        layout,
        resolver,
        None,
        viewport,
        &fonts,
        web_fonts,
    );
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
    render_document_with_url_internal(document, viewport, base_url, true)
}

/// Renders a Document whose browser lifecycle has already executed scripts.
///
/// Session-owned Documents must not run their script elements again merely
/// because the embedder asks for a screenshot. Their current DOM is the input
/// snapshot; only styles, layout, and paint are evaluated here.
pub(crate) fn render_document_snapshot_with_url(
    document: &NodeHandle,
    viewport: Rect,
    base_url: Option<&crate::http::Url>,
) -> Result<Canvas, PaintError> {
    render_document_with_url_internal(document, viewport, base_url, false)
}

fn render_document_with_url_internal(
    document: &NodeHandle,
    viewport: Rect,
    base_url: Option<&crate::http::Url>,
    execute_javascript: bool,
) -> Result<Canvas, PaintError> {
    let mut timings = RenderTimings::default();
    let effective_base = stylesheet::extract_document_base_url(document, base_url);
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(viewport.width, viewport.height);
    let mut parsed_sheets = Vec::new();

    // Execute <script> tags and fire DOMContentLoaded before layout.
    // JS may modify the DOM (e.g., classList.add for fade-in animations,
    // injecting <style> elements), so this must happen before layout.
    if execute_javascript {
        let javascript_start = Instant::now();
        let runtime_init_start = Instant::now();
        if let Ok(mut runtime) = crate::js::JsRuntime::with_document(document.clone()) {
            timings.javascript_runtime_init = runtime_init_start.elapsed();
            // Keep getComputedStyle / layout-metric queries issued by page scripts
            // consistent with the render viewport.
            runtime.set_viewport(viewport.width, viewport.height);
            let document_scripts_start = Instant::now();
            let errors = runtime.execute_document_scripts(effective_base.as_ref());
            timings.javascript_document_scripts = document_scripts_start.elapsed();
            for err in &errors {
                eprintln!("[omoikane][js-error] {err}");
            }
            // Wire `on*` inline handlers (e.g. <body onload>) and fire the `load`
            // event. Per the HTML load order this happens after scripts run and
            // DOMContentLoaded has fired (inside execute_document_scripts), so page
            // load handlers run before the timer pump advances virtual time.
            let load_events_start = Instant::now();
            if let Err(err) = runtime.wire_inline_event_handlers() {
                eprintln!("[omoikane][js-error] {err}");
            }
            if let Err(err) = runtime.fire_load() {
                eprintln!("[omoikane][js-error] {err}");
            }
            timings.javascript_load_events = load_events_start.elapsed();
            timings.javascript = javascript_start.elapsed();
            // Drive script-scheduled timers (setTimeout/setInterval) in virtual
            // time so that DOM mutations from deferred callbacks settle before
            // layout. Bounded by a virtual-time budget and a task-count cap so an
            // infinite setInterval cannot hang the render.
            let timers_start = Instant::now();
            let tasks_run = runtime.run_timers(
                TIMER_PUMP_MAX_VIRTUAL_MS,
                TIMER_PUMP_STEP_MS,
                TIMER_PUMP_MAX_TASKS,
            );
            timings.timers = timers_start.elapsed();
            // A rendering opportunity is distinct from the promise-job and timer
            // queues. Drive it explicitly after deferred page initialization and
            // before rebuilding styles/layout for the screenshot.
            let animation_frames_start = Instant::now();
            let frame_callbacks_run = runtime.run_animation_frames(
                ANIMATION_FRAME_PUMP_MAX_FRAMES,
                ANIMATION_FRAME_INTERVAL_MS,
            );
            timings.animation_frames = animation_frames_start.elapsed();
            if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
                eprintln!(
                    "[omoikane][event-loop] completed {tasks_run} macrotasks and {frame_callbacks_run} animation-frame callbacks"
                );
                if let Ok(value) = runtime.eval(
                    "JSON.stringify({ scripts: globalThis.__SCRIPTS_LOADED__ || null, rootChildren: (document.getElementById('react-root') || {}).childElementCount || 0, timersPending: false })",
                ) && let Some(value) = value.as_string() {
                    eprintln!("[omoikane][bootstrap-state] {}", value.to_std_string_escaped());
                }
            }
            // CSSOM insertRule/deleteRule mutations are batched by the JS shim so
            // frameworks can install large generated stylesheets without an O(n²)
            // text rewrite. Commit the batch before rebuilding the native resolver.
            if let Err(err) = runtime.eval("__omoikane_flush_stylesheets()") {
                eprintln!("[omoikane][js-error] {err}");
            }
        } else {
            timings.javascript_runtime_init = runtime_init_start.elapsed();
            timings.javascript = javascript_start.elapsed();
        }
    }

    // Build the native resolver once from the final DOM. In the JavaScript
    // path this deliberately happens after scripts, timers, animation frames,
    // and batched CSSOM mutations have settled; the earlier resolver was never
    // consumed by layout or paint and only duplicated stylesheet parsing.
    let stylesheets_start = Instant::now();
    for css_text in stylesheet::extract_author_stylesheets(document, base_url)? {
        parsed_sheets.push(stylesheet::parse_stylesheet_forgiving(&css_text));
    }
    for sheet in &parsed_sheets {
        resolver.add_stylesheet(Origin::Author, sheet.clone());
    }
    if execute_javascript {
        timings.style_refresh = stylesheets_start.elapsed();
    } else {
        timings.stylesheets = stylesheets_start.elapsed();
    }

    // Collect @font-face rules from the same final stylesheet set used for
    // layout, including rules injected by page scripts.
    let fonts_start = Instant::now();
    let fetched_web_fonts =
        stylesheet::fetch_font_face_fonts(&parsed_sheets, effective_base.as_ref());
    let mut web_font_registry = WebFontRegistry::new();
    for wf in fetched_web_fonts {
        web_font_registry.push_shared(&wf.family, wf.weight, wf.style, wf.font);
    }
    let all_fonts = text::load_text_fonts();
    let layout_fonts = all_fonts.clone();
    let web_font_registry = Arc::new(web_font_registry);
    let web_font_registry_opt = if web_font_registry.is_empty() {
        None
    } else {
        Some(web_font_registry.as_ref())
    };
    let layout_web_fonts = web_font_registry_opt.map(|_| Arc::clone(&web_font_registry));
    timings.fonts = fonts_start.elapsed();

    let result = crate::layout::with_layout_fonts(layout_fonts, layout_web_fonts, || {
        crate::layout::with_image_base_url(effective_base, || {
            let layout_start = Instant::now();
            let layout = crate::layout::layout_tree(document, &mut resolver, viewport)?;
            timings.layout = layout_start.elapsed();
            let paint_start = Instant::now();
            let canvas = paint_layout_with_web_fonts(
                &layout,
                &mut resolver,
                viewport,
                all_fonts,
                web_font_registry_opt,
            );
            timings.paint = paint_start.elapsed();
            Some(canvas)
        })
    });
    record_render_timings(&timings);
    result.ok_or(PaintError::InvalidImageBuffer)
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
    let canvas = render_document_with_url(document, viewport, base_url)?;
    let encode_start = Instant::now();
    let png = canvas.encode_png();
    record_render_timings(&RenderTimings {
        png_encode: encode_start.elapsed(),
        ..RenderTimings::default()
    });
    Ok(png)
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
    text_fonts: &[Arc<Font>],
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
    text_fonts: &[Arc<Font>],
    web_fonts: Option<&WebFontRegistry>,
) {
    if layout.visibility == Visibility::Hidden {
        return;
    }

    if !layout.transform.is_identity() {
        paint_transformed_box(
            canvas,
            layout,
            resolver,
            inherited_clip,
            viewport,
            text_fonts,
            web_fonts,
        );
        return;
    }

    paint_box_internal_untransformed(
        canvas,
        layout,
        resolver,
        inherited_clip,
        viewport,
        include_phase_descendants,
        text_fonts,
        web_fonts,
    );
}

const TRANSFORM_SURFACE_TILE_SIZE: u32 = 2048;

#[allow(clippy::too_many_arguments)]
fn paint_transformed_box(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    inherited_clip: Option<Rect>,
    viewport: Rect,
    text_fonts: &[Arc<Font>],
    web_fonts: Option<&WebFontRegistry>,
) {
    let Some(inverse) = layout.transform.inverse() else {
        return;
    };
    let canvas_bounds = Rect {
        x: 0.0,
        y: 0.0,
        width: canvas.width() as f32,
        height: canvas.height() as f32,
    };
    let destination_bounds = if let Some(clip) = inherited_clip {
        let Some(bounds) = intersect(clip, canvas_bounds) else {
            return;
        };
        bounds
    } else {
        canvas_bounds
    };
    let required_source = transformed_rect_bounds(destination_bounds, inverse);
    let required_source = Rect {
        x: required_source.x - 1.0,
        y: required_source.y - 1.0,
        width: required_source.width + 2.0,
        height: required_source.height + 2.0,
    };
    let Some(source_region) = intersect(subtree_paint_bounds(layout, resolver), required_source)
    else {
        return;
    };
    let source_x0 = source_region.x.floor() as i32;
    let source_y0 = source_region.y.floor() as i32;
    let source_x1 = (source_region.x + source_region.width).ceil() as i32;
    let source_y1 = (source_region.y + source_region.height).ceil() as i32;
    let tile_size = TRANSFORM_SURFACE_TILE_SIZE as i32;

    // Paint source-space tiles instead of a viewport-sized surface. This keeps
    // pixels that begin outside the viewport but transform into view, while the
    // inverse-mapped destination bounds and tiling prevent unbounded allocation.
    for tile_y in (source_y0..source_y1).step_by(tile_size as usize) {
        for tile_x in (source_x0..source_x1).step_by(tile_size as usize) {
            let tile_x1 = (tile_x + tile_size).min(source_x1);
            let tile_y1 = (tile_y + tile_size).min(source_y1);
            let tile_width = (tile_x1 - tile_x).max(1) as u32;
            let tile_height = (tile_y1 - tile_y).max(1) as u32;
            let mut translated_layout = layout.clone();
            translate_layout_for_paint(
                &mut translated_layout,
                -(tile_x as f32),
                -(tile_y as f32),
            );
            let translated_viewport = Rect {
                x: viewport.x - tile_x as f32,
                y: viewport.y - tile_y as f32,
                width: viewport.width,
                height: viewport.height,
            };
            let mut offscreen = Canvas::new(tile_width, tile_height);
            // A transformed element establishes a paint containment boundary
            // for positioned descendants. Its ancestor clip is applied later
            // in destination space; local overflow/clip-path is painted here.
            paint_box_internal_untransformed(
                &mut offscreen,
                &translated_layout,
                resolver,
                None,
                translated_viewport,
                true,
                text_fonts,
                web_fonts,
            );
            let tile_transform = layout
                .transform
                .multiply(AffineTransform::translate(tile_x as f32, tile_y as f32));
            composite_affine(
                canvas,
                &offscreen,
                tile_transform,
                inherited_clip,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: tile_width as f32,
                    height: tile_height as f32,
                },
            );
        }
    }
}

fn translate_layout_for_paint(layout: &mut LayoutBox, dx: f32, dy: f32) {
    layout.dimensions.content.x += dx;
    layout.dimensions.content.y += dy;
    for line in &mut layout.lines {
        line.rect.x += dx;
        line.rect.y += dy;
        for fragment in &mut line.fragments {
            fragment.rect.x += dx;
            fragment.rect.y += dy;
        }
    }
    if let Some(marker) = &mut layout.marker {
        marker.x += dx;
        marker.y += dy;
    }
    layout.transform = AffineTransform::translate(dx, dy)
        .multiply(layout.transform)
        .multiply(AffineTransform::translate(-dx, -dy));
    for child in &mut layout.children {
        translate_layout_for_paint(child, dx, dy);
    }
}

fn paint_box_internal_untransformed(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    inherited_clip: Option<Rect>,
    viewport: Rect,
    include_phase_descendants: bool,
    text_fonts: &[Arc<Font>],
    web_fonts: Option<&WebFontRegistry>,
) {
    if layout.visibility == Visibility::Hidden {
        return;
    }

    let style = resolver.computed_style(&layout.node);
    let border_box = border_box_rect(layout);
    let padding_box = padding_box_rect(layout);
    let inherited_clip = if let Some(inset_clip) = clip_path_inset_rect(&style, border_box) {
        let Some(inset_clip) = inset_clip else {
            return;
        };
        if let Some(current) = inherited_clip {
            let Some(combined) = intersect(current, inset_clip) else {
                return;
            };
            Some(combined)
        } else {
            Some(inset_clip)
        }
    } else {
        inherited_clip
    };

    let backdrop_filters = style_filters(&style, "backdrop-filter");
    if !backdrop_filters.is_empty() {
        apply_backdrop_filters(canvas, &backdrop_filters, border_box, inherited_clip);
    }

    // opacity、filter、または解決可能な mask-image がある場合、要素サブツリーを
    // オフスクリーンバッファに描画してからまとめて合成する。
    let opacity = element_opacity(&style);
    let filters = element_filters(&style);
    let mask = mask_image(&style);
    let needs_offscreen =
        opacity.is_some_and(|v| v < 1.0) || !filters.is_empty() || mask.is_some();

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
        apply_filters(&mut offscreen, &filters);
        offscreen.multiply_alpha(opacity_value);
        if let Some(mask) = &mask {
            apply_mask_alpha(&mut offscreen, mask, &style, offset_border_box);
        }
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

fn composite_affine(
    destination: &mut Canvas,
    source: &Canvas,
    transform: AffineTransform,
    clip: Option<Rect>,
    source_hint: Rect,
) {
    let Some(inverse) = transform.inverse() else {
        return;
    };
    let width = source.width() as i32;
    let height = source.height() as i32;
    let hint_x0 = source_hint.x.floor().max(0.0).min(width as f32) as i32;
    let hint_y0 = source_hint.y.floor().max(0.0).min(height as f32) as i32;
    let hint_x1 = (source_hint.x + source_hint.width)
        .ceil()
        .max(0.0)
        .min(width as f32) as i32;
    let hint_y1 = (source_hint.y + source_hint.height)
        .ceil()
        .max(0.0)
        .min(height as f32) as i32;
    let mut source_min_x = hint_x1;
    let mut source_min_y = hint_y1;
    let mut source_max_x = hint_x0;
    let mut source_max_y = hint_y0;
    for y in hint_y0..hint_y1 {
        for x in hint_x0..hint_x1 {
            let index = ((y * width + x) * 4 + 3) as usize;
            if source.pixels[index] != 0 {
                source_min_x = source_min_x.min(x);
                source_min_y = source_min_y.min(y);
                source_max_x = source_max_x.max(x + 1);
                source_max_y = source_max_y.max(y + 1);
            }
        }
    }
    if source_min_x >= source_max_x || source_min_y >= source_max_y {
        return;
    }

    let corners = [
        transform.transform_point(source_min_x as f32, source_min_y as f32),
        transform.transform_point(source_max_x as f32, source_min_y as f32),
        transform.transform_point(source_min_x as f32, source_max_y as f32),
        transform.transform_point(source_max_x as f32, source_max_y as f32),
    ];
    let min_x = corners
        .iter()
        .map(|point| point.0)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as i32;
    let min_y = corners
        .iter()
        .map(|point| point.1)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as i32;
    let max_x = corners
        .iter()
        .map(|point| point.0)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(destination.width() as f32) as i32;
    let max_y = corners
        .iter()
        .map(|point| point.1)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(destination.height() as f32) as i32;

    for y in min_y..max_y {
        for x in min_x..max_x {
            if let Some(clip) = clip
                && (x as f32 + 0.5 < clip.x
                    || y as f32 + 0.5 < clip.y
                    || x as f32 + 0.5 >= clip.x + clip.width
                    || y as f32 + 0.5 >= clip.y + clip.height)
            {
                continue;
            }
            let (source_x, source_y) = inverse.transform_point(x as f32 + 0.5, y as f32 + 0.5);
            let source_x = source_x.floor() as i32;
            let source_y = source_y.floor() as i32;
            if source_x < source_min_x
                || source_y < source_min_y
                || source_x >= source_max_x
                || source_y >= source_max_y
            {
                continue;
            }
            let source_index = ((source_y * width + source_x) * 4) as usize;
            let alpha = source.pixels[source_index + 3];
            if alpha == 0 {
                continue;
            }
            let color = Color {
                r: source.pixels[source_index],
                g: source.pixels[source_index + 1],
                b: source.pixels[source_index + 2],
                a: alpha,
            };
            let destination_index =
                ((y as u32 * destination.width() + x as u32) * 4) as usize;
            blend_pixel(
                &mut destination.pixels[destination_index..destination_index + 4],
                color,
            );
        }
    }
}

fn subtree_paint_bounds(layout: &LayoutBox, resolver: &mut StyleResolver) -> Rect {
    let mut bounds = border_box_rect(layout);
    for line in &layout.lines {
        bounds = union_rect(bounds, line.rect);
        for fragment in &line.fragments {
            bounds = union_rect(bounds, fragment.rect);
        }
    }
    if let Some(marker) = &layout.marker {
        bounds = union_rect(
            bounds,
            Rect {
                x: marker.x - marker.font_size,
                y: marker.y,
                width: marker.font_size * 2.0,
                height: marker.font_size * 1.5,
            },
        );
    }

    let style = resolver.computed_style(&layout.node);
    let shadow_value = match style.get("box-shadow") {
        Some(ComputedValue::Keyword(value)) | Some(ComputedValue::String(value)) => {
            Some(value.as_str())
        }
        _ => None,
    };
    if let Some(shadow_value) = shadow_value {
        let border_box = border_box_rect(layout);
        for shadow in border::parse_box_shadow(shadow_value) {
            if shadow.inset {
                continue;
            }
            let extent = shadow.blur_radius * 2.0 + shadow.spread_radius.abs();
            bounds = union_rect(
                bounds,
                Rect {
                    x: border_box.x + shadow.offset_x - extent,
                    y: border_box.y + shadow.offset_y - extent,
                    width: border_box.width + extent * 2.0,
                    height: border_box.height + extent * 2.0,
                },
            );
        }
    }

    for child in &layout.children {
        let child_bounds = subtree_paint_bounds(child, resolver);
        bounds = union_rect(
            bounds,
            transformed_rect_bounds(child_bounds, child.transform),
        );
    }
    bounds
}

fn transformed_rect_bounds(rect: Rect, transform: AffineTransform) -> Rect {
    if transform.is_identity() {
        return rect;
    }
    let corners = [
        transform.transform_point(rect.x, rect.y),
        transform.transform_point(rect.x + rect.width, rect.y),
        transform.transform_point(rect.x, rect.y + rect.height),
        transform.transform_point(rect.x + rect.width, rect.y + rect.height),
    ];
    let min_x = corners
        .iter()
        .map(|point| point.0)
        .fold(f32::INFINITY, f32::min);
    let min_y = corners
        .iter()
        .map(|point| point.1)
        .fold(f32::INFINITY, f32::min);
    let max_x = corners
        .iter()
        .map(|point| point.0)
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = corners
        .iter()
        .map(|point| point.1)
        .fold(f32::NEG_INFINITY, f32::max);
    Rect {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    }
}

fn union_rect(left: Rect, right: Rect) -> Rect {
    let x = left.x.min(right.x);
    let y = left.y.min(right.y);
    let right_edge = (left.x + left.width).max(right.x + right.width);
    let bottom_edge = (left.y + left.height).max(right.y + right.height);
    Rect {
        x,
        y,
        width: right_edge - x,
        height: bottom_edge - y,
    }
}

fn apply_mask_alpha(canvas: &mut Canvas, mask: &Image, style: &ComputedStyle, area: Rect) {
    let (tile_width, tile_height) = mask_size(
        style,
        area,
        mask.width().max(1) as f32,
        mask.height().max(1) as f32,
    );
    if tile_width <= 0.0 || tile_height <= 0.0 {
        canvas.multiply_alpha(0.0);
        return;
    }
    let (position_x, position_y) =
        mask_position(style, area.width, area.height, tile_width, tile_height);
    let anchor_x = area.x + position_x;
    let anchor_y = area.y + position_y;
    let repeat = mask_repeat(style);
    let width = canvas.width as i32;
    let height = canvas.height as i32;
    let area_x0 = area.x.floor().max(0.0) as i32;
    let area_y0 = area.y.floor().max(0.0) as i32;
    let area_x1 = (area.x + area.width).ceil().min(width as f32) as i32;
    let area_y1 = (area.y + area.height).ceil().min(height as f32) as i32;

    for y in 0..height {
        for x in 0..width {
            let pixel_x = x as f32;
            let pixel_y = y as f32;
            let inside_area = x >= area_x0 && x < area_x1 && y >= area_y0 && y < area_y1;
            let mask_alpha = if inside_area {
                sample_mask_alpha(
                    mask,
                    pixel_x,
                    pixel_y,
                    anchor_x,
                    anchor_y,
                    tile_width,
                    tile_height,
                    repeat,
                )
            } else {
                0
            };
            let index = ((y * width + x) * 4) as usize;
            canvas.pixels[index + 3] =
                ((canvas.pixels[index + 3] as u16 * mask_alpha as u16 + 127) / 255) as u8;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sample_mask_alpha(
    mask: &Image,
    x: f32,
    y: f32,
    anchor_x: f32,
    anchor_y: f32,
    tile_width: f32,
    tile_height: f32,
    repeat: bool,
) -> u8 {
    let tile_x = if repeat {
        anchor_x + ((x - anchor_x) / tile_width).floor() * tile_width
    } else {
        anchor_x
    };
    let tile_y = if repeat {
        anchor_y + ((y - anchor_y) / tile_height).floor() * tile_height
    } else {
        anchor_y
    };
    let u = (x - tile_x) / tile_width;
    let v = (y - tile_y) / tile_height;
    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
        return 0;
    }
    let source_x = (u * mask.width as f32).floor() as u32;
    let source_y = (v * mask.height as f32).floor() as u32;
    let index = ((source_y * mask.width + source_x) * 4) as usize;
    mask.pixels[index + 3]
}

fn paint_replaced_image_box(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    style: &ComputedStyle,
    clip: Option<Rect>,
) {
    let is_positioned = matches!(
        style.get("position"),
        Some(ComputedValue::Keyword(position))
            if position.eq_ignore_ascii_case("absolute")
                || position.eq_ignore_ascii_case("fixed")
    );
    if !is_positioned {
        return;
    }
    if layout.node.tag_name().as_deref() != Some("img") {
        return;
    }
    let Some(attributes) = layout.node.attributes() else {
        return;
    };
    let Some(source) = attributes.get("src") else {
        return;
    };
    let Some(image) = crate::layout::decode_or_fetch_image_asset(source) else {
        return;
    };

    canvas.draw_image_scaled_clipped(&image, layout.dimensions.content, clip);
}

#[allow(clippy::too_many_arguments)]
fn paint_box_internal_to(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    inherited_clip: Option<Rect>,
    viewport: Rect,
    include_phase_descendants: bool,
    text_fonts: &[Arc<Font>],
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
    if layout.overflow == crate::layout::Overflow::Hidden {
        match inherited_clip {
            Some(current) => {
                if let Some(image_clip) = intersect(current, padding_box) {
                    paint_replaced_image_box(canvas, layout, style, Some(image_clip));
                }
            }
            None => paint_replaced_image_box(canvas, layout, style, Some(padding_box)),
        }
    } else {
        paint_replaced_image_box(canvas, layout, style, inherited_clip);
    }
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
            Some(current) => {
                let Some(combined) = intersect(current, padding_box) else {
                    return;
                };
                Some(combined)
            }
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
        paint_box_internal(
            canvas, child, resolver, clip, viewport, true, text_fonts, web_fonts,
        );
    }
    for child in normal_block_children {
        paint_box_internal(
            canvas, child, resolver, clip, viewport, false, text_fonts, web_fonts,
        );
    }
    for child in float_children {
        paint_box_internal(
            canvas, child, resolver, clip, viewport, true, text_fonts, web_fonts,
        );
    }
    text::paint_text_with_registry(canvas, layout, style, clip, viewport, text_fonts, web_fonts);
    text::paint_list_marker(canvas, layout, style, clip, text_fonts);
    for child in inline_children {
        paint_box_internal(
            canvas, child, resolver, clip, viewport, false, text_fonts, web_fonts,
        );
    }
    for child in auto_positioned_children {
        paint_box_internal(
            canvas, child, resolver, clip, viewport, true, text_fonts, web_fonts,
        );
    }
    for child in positive_positioned_children {
        paint_box_internal(
            canvas, child, resolver, clip, viewport, true, text_fonts, web_fonts,
        );
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
    if !layout.transform.is_identity() {
        return;
    }
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

fn mask_image(style: &ComputedStyle) -> Option<Image> {
    let value = match style.get("mask-image") {
        Some(ComputedValue::Keyword(value)) | Some(ComputedValue::String(value)) => value.trim(),
        _ => return None,
    };
    // The scoped implementation accepts URL images only. Unsupported image
    // functions (such as gradients) and load failures mean no mask.
    if !value.to_ascii_lowercase().starts_with("url(") {
        return None;
    }
    image::parse_background_image_value(value)
}

fn background_repeat(style: &ComputedStyle) -> bool {
    image_repeat(style, "background-repeat")
}

fn mask_repeat(style: &ComputedStyle) -> bool {
    image_repeat(style, "mask-repeat")
}

fn image_repeat(style: &ComputedStyle, property: &str) -> bool {
    !matches!(
        style.get(property),
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
    image_position(
        style,
        "background-position",
        container_w,
        container_h,
        image_w,
        image_h,
    )
}

fn mask_position(
    style: &ComputedStyle,
    container_w: f32,
    container_h: f32,
    image_w: f32,
    image_h: f32,
) -> (f32, f32) {
    image_position(
        style,
        "mask-position",
        container_w,
        container_h,
        image_w,
        image_h,
    )
}

fn image_position(
    style: &ComputedStyle,
    prefix: &str,
    container_w: f32,
    container_h: f32,
    image_w: f32,
    image_h: f32,
) -> (f32, f32) {
    let x_property = format!("{prefix}-x");
    let y_property = format!("{prefix}-y");
    let x = resolve_image_position(style, &x_property, container_w, image_w);
    let y = resolve_image_position(style, &y_property, container_h, image_h);
    (x, y)
}

fn resolve_image_position(
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
        Some(ComputedValue::Keyword(k)) => match k.to_ascii_lowercase().as_str() {
            "center" => (container_size - image_size) * 0.5,
            "right" if is_x => container_size - image_size,
            "left" if is_x => 0.0,
            "bottom" if !is_x => container_size - image_size,
            "top" if !is_x => 0.0,
            _ => 0.0,
        },
        _ => 0.0,
    }
}

fn background_size(style: &ComputedStyle, area: Rect, image_w: f32, image_h: f32) -> (f32, f32) {
    image::background_size(style, area, image_w, image_h)
}

fn mask_size(style: &ComputedStyle, area: Rect, image_w: f32, image_h: f32) -> (f32, f32) {
    image::mask_size(style, area, image_w, image_h)
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

#[derive(Clone, Copy, Default)]
struct ClipPathInsetLength {
    px: f32,
    percentage: f32,
}

impl ClipPathInsetLength {
    fn resolve(self, basis: f32) -> f32 {
        self.px + basis * self.percentage / 100.0
    }
}

/// Returns `None` for unsupported clip shapes, `Some(None)` for an empty inset,
/// and `Some(Some(rect))` for a non-empty inset clip.
fn clip_path_inset_rect(style: &ComputedStyle, border_box: Rect) -> Option<Option<Rect>> {
    let value = match style.get("clip-path") {
        Some(ComputedValue::Keyword(value)) | Some(ComputedValue::String(value)) => value.trim(),
        _ => return None,
    };
    let open = value.find('(')?;
    if !value[..open].trim().eq_ignore_ascii_case("inset") || !value.ends_with(')') {
        return None;
    }

    let mut lengths = Vec::new();
    for component in split_top_level_whitespace(&value[open + 1..value.len() - 1]) {
        if component.eq_ignore_ascii_case("round") {
            break;
        }
        lengths.push(parse_clip_path_inset_length(component)?);
    }
    let edges = match lengths.as_slice() {
        [all] => [*all; 4],
        [vertical, horizontal] => [*vertical, *horizontal, *vertical, *horizontal],
        [top, horizontal, bottom] => [*top, *horizontal, *bottom, *horizontal],
        [top, right, bottom, left] => [*top, *right, *bottom, *left],
        _ => return None,
    };

    let top = edges[0].resolve(border_box.height);
    let right = edges[1].resolve(border_box.width);
    let bottom = edges[2].resolve(border_box.height);
    let left = edges[3].resolve(border_box.width);
    let rect = Rect {
        x: border_box.x + left,
        y: border_box.y + top,
        width: border_box.width - left - right,
        height: border_box.height - top - bottom,
    };
    Some(normalize_rect(rect))
}

fn split_top_level_whitespace(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (index, ch) in value.char_indices() {
        if ch.is_ascii_whitespace() && depth == 0 {
            if let Some(part_start) = start.take() {
                parts.push(&value[part_start..index]);
            }
            continue;
        }
        if start.is_none() {
            start = Some(index);
        }
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    if let Some(part_start) = start {
        parts.push(&value[part_start..]);
    }
    parts
}

fn parse_clip_path_inset_length(value: &str) -> Option<ClipPathInsetLength> {
    let value = value.trim();
    if value.len() >= 6
        && value
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("calc("))
        && value.ends_with(')')
    {
        return parse_clip_path_inset_calc(&value[5..value.len() - 1]);
    }
    if let Some(number) = value.strip_suffix("px") {
        return Some(ClipPathInsetLength {
            px: number.trim().parse().ok()?,
            percentage: 0.0,
        });
    }
    if let Some(number) = value.strip_suffix('%') {
        return Some(ClipPathInsetLength {
            px: 0.0,
            percentage: number.trim().parse().ok()?,
        });
    }
    let number = value.parse::<f32>().ok()?;
    (number == 0.0).then_some(ClipPathInsetLength::default())
}

fn parse_clip_path_inset_calc(value: &str) -> Option<ClipPathInsetLength> {
    if let Some((left, right)) = value.split_once(" + ") {
        let mut result = parse_clip_path_inset_length(left)?;
        let right = parse_clip_path_inset_length(right)?;
        result.px += right.px;
        result.percentage += right.percentage;
        return Some(result);
    }
    if let Some((left, right)) = value.split_once(" - ") {
        let mut result = parse_clip_path_inset_length(left)?;
        let right = parse_clip_path_inset_length(right)?;
        result.px -= right.px;
        result.percentage -= right.percentage;
        return Some(result);
    }
    parse_clip_path_inset_length(value)
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

fn element_filters(style: &ComputedStyle) -> Vec<crate::css::FilterFunction> {
    style_filters(style, "filter")
}

fn style_filters(style: &ComputedStyle, property: &str) -> Vec<crate::css::FilterFunction> {
    match style.get(property) {
        Some(ComputedValue::Keyword(value)) | Some(ComputedValue::String(value)) => {
            crate::css::parse_filter_list(value).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

fn apply_backdrop_filters(
    canvas: &mut Canvas,
    filters: &[crate::css::FilterFunction],
    border_box: Rect,
    inherited_clip: Option<Rect>,
) {
    let Some(area) = normalize_rect(border_box) else {
        return;
    };
    let area = if let Some(clip) = inherited_clip {
        let Some(intersection) = intersect(area, clip) else {
            return;
        };
        intersection
    } else {
        area
    };
    let mut filtered = canvas.clone();
    apply_filters(&mut filtered, filters);
    let x0 = area.x.floor().max(0.0) as usize;
    let y0 = area.y.floor().max(0.0) as usize;
    let x1 = (area.x + area.width).ceil().min(canvas.width as f32) as usize;
    let y1 = (area.y + area.height).ceil().min(canvas.height as f32) as usize;
    for y in y0..y1 {
        let start = (y * canvas.width as usize + x0) * 4;
        let end = (y * canvas.width as usize + x1) * 4;
        canvas.pixels[start..end].copy_from_slice(&filtered.pixels[start..end]);
    }
}

fn apply_filters(canvas: &mut Canvas, filters: &[crate::css::FilterFunction]) {
    let Some((x0, y0, x1, y1)) = alpha_bounds(canvas) else {
        return;
    };
    let (left, top, right, bottom) = filter_padding(filters);
    let crop_x0 = x0.saturating_sub(left);
    let crop_y0 = y0.saturating_sub(top);
    let crop_x1 = (x1 + right).min(canvas.width as usize);
    let crop_y1 = (y1 + bottom).min(canvas.height as usize);
    let crop_width = crop_x1 - crop_x0;
    let crop_height = crop_y1 - crop_y0;
    let mut cropped = Canvas::new(crop_width as u32, crop_height as u32);
    for y in crop_y0..crop_y1 {
        let source_start = (y * canvas.width as usize + crop_x0) * 4;
        let source_end = source_start + crop_width * 4;
        let target_start = (y - crop_y0) * crop_width * 4;
        cropped.pixels[target_start..target_start + crop_width * 4]
            .copy_from_slice(&canvas.pixels[source_start..source_end]);
    }
    apply_filters_full(&mut cropped, filters);
    for y in crop_y0..crop_y1 {
        let target_start = (y * canvas.width as usize + crop_x0) * 4;
        let source_start = (y - crop_y0) * crop_width * 4;
        canvas.pixels[target_start..target_start + crop_width * 4]
            .copy_from_slice(&cropped.pixels[source_start..source_start + crop_width * 4]);
    }
}

fn apply_filters_full(canvas: &mut Canvas, filters: &[crate::css::FilterFunction]) {
    use crate::css::FilterFunction;
    for filter in filters {
        match filter {
            FilterFunction::Blur(radius) => box_blur(canvas, radius.round() as usize),
            FilterFunction::Brightness(amount) => {
                apply_color_filter(canvas, |value| value * amount, 1.0)
            }
            FilterFunction::Contrast(amount) => {
                apply_color_filter(canvas, |value| (value - 0.5) * amount + 0.5, 1.0)
            }
            FilterFunction::DropShadow { offset_x, offset_y, blur, color } => {
                apply_drop_shadow(canvas, *offset_x, *offset_y, *blur, *color)
            }
            FilterFunction::Grayscale(amount) => apply_color_matrix(canvas, grayscale_matrix(*amount)),
            FilterFunction::HueRotate(degrees) => apply_color_matrix(canvas, hue_rotate_matrix(*degrees)),
            FilterFunction::Invert(amount) => {
                apply_color_filter(canvas, |value| value * (1.0 - amount) + (1.0 - value) * amount, 1.0)
            }
            FilterFunction::Opacity(amount) => apply_color_filter(canvas, |value| value, *amount),
            FilterFunction::Saturate(amount) => apply_color_matrix(canvas, saturate_matrix(*amount)),
            FilterFunction::Sepia(amount) => apply_color_matrix(canvas, sepia_matrix(*amount)),
        }
    }
}

fn alpha_bounds(canvas: &Canvas) -> Option<(usize, usize, usize, usize)> {
    let width = canvas.width as usize;
    let mut x0 = width;
    let mut y0 = canvas.height as usize;
    let mut x1 = 0usize;
    let mut y1 = 0usize;
    for (index, pixel) in canvas.pixels.chunks_exact(4).enumerate() {
        if pixel[3] == 0 { continue; }
        let x = index % width;
        let y = index / width;
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x + 1);
        y1 = y1.max(y + 1);
    }
    (x0 < x1 && y0 < y1).then_some((x0, y0, x1, y1))
}

fn filter_padding(filters: &[crate::css::FilterFunction]) -> (usize, usize, usize, usize) {
    use crate::css::FilterFunction;
    let (mut left, mut top, mut right, mut bottom) = (0usize, 0usize, 0usize, 0usize);
    for filter in filters {
        match filter {
            FilterFunction::Blur(radius) => {
                let radius = radius.ceil() as usize;
                left += radius; top += radius; right += radius; bottom += radius;
            }
            FilterFunction::DropShadow { offset_x, offset_y, blur, .. } => {
                let blur = blur.ceil() as usize;
                left += blur + (-offset_x.floor()).max(0.0) as usize;
                right += blur + offset_x.ceil().max(0.0) as usize;
                top += blur + (-offset_y.floor()).max(0.0) as usize;
                bottom += blur + offset_y.ceil().max(0.0) as usize;
            }
            _ => {}
        }
    }
    (left, top, right, bottom)
}

fn apply_drop_shadow(canvas: &mut Canvas, offset_x: f32, offset_y: f32, blur: f32, color: Color) {
    let source = canvas.clone();
    let mut shadow = Canvas::new(canvas.width, canvas.height);
    let mut result = Canvas::new(canvas.width, canvas.height);
    for (index, pixel) in source.pixels.chunks_exact(4).enumerate() {
        shadow.pixels[index * 4 + 3] =
            ((pixel[3] as u16 * color.a as u16) / 255) as u8;
    }
    box_blur(&mut shadow, blur.round() as usize);
    let dx = offset_x.round() as i32;
    let dy = offset_y.round() as i32;
    for y in 0..canvas.height as i32 {
        for x in 0..canvas.width as i32 {
            let sx = x - dx;
            let sy = y - dy;
            if sx < 0 || sy < 0 || sx >= canvas.width as i32 || sy >= canvas.height as i32 {
                continue;
            }
            let source_index = (sy as usize * canvas.width as usize + sx as usize) * 4;
            let alpha = shadow.pixels[source_index + 3];
            if alpha == 0 { continue; }
            let target_index = (y as usize * canvas.width as usize + x as usize) * 4;
            blend_pixel(&mut result.pixels[target_index..target_index + 4], Color { a: alpha, ..color });
        }
    }
    for (target, source) in result.pixels.chunks_exact_mut(4).zip(source.pixels.chunks_exact(4)) {
        blend_pixel(target, Color { r: source[0], g: source[1], b: source[2], a: source[3] });
    }
    *canvas = result;
}

fn apply_color_filter(canvas: &mut Canvas, map: impl Fn(f32) -> f32, alpha: f32) {
    for pixel in canvas.pixels.chunks_exact_mut(4) {
        for channel in &mut pixel[..3] {
            *channel = (map(*channel as f32 / 255.0).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        pixel[3] = (pixel[3] as f32 * alpha.clamp(0.0, 1.0)).round() as u8;
    }
}

fn apply_color_matrix(canvas: &mut Canvas, matrix: [[f32; 3]; 3]) {
    for pixel in canvas.pixels.chunks_exact_mut(4) {
        let rgb = [pixel[0] as f32 / 255.0, pixel[1] as f32 / 255.0, pixel[2] as f32 / 255.0];
        for row in 0..3 {
            let value = matrix[row][0] * rgb[0] + matrix[row][1] * rgb[1] + matrix[row][2] * rgb[2];
            pixel[row] = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
}

fn grayscale_matrix(amount: f32) -> [[f32; 3]; 3] {
    let a = amount.clamp(0.0, 1.0);
    let keep = 1.0 - a;
    [[keep + 0.2126*a, 0.7152*a, 0.0722*a], [0.2126*a, keep + 0.7152*a, 0.0722*a], [0.2126*a, 0.7152*a, keep + 0.0722*a]]
}

fn saturate_matrix(amount: f32) -> [[f32; 3]; 3] {
    let a = amount.max(0.0);
    [[0.213 + 0.787*a, 0.715 - 0.715*a, 0.072 - 0.072*a], [0.213 - 0.213*a, 0.715 + 0.285*a, 0.072 - 0.072*a], [0.213 - 0.213*a, 0.715 - 0.715*a, 0.072 + 0.928*a]]
}

fn sepia_matrix(amount: f32) -> [[f32; 3]; 3] {
    let a = amount.clamp(0.0, 1.0);
    let keep = 1.0 - a;
    [[keep + 0.393*a, 0.769*a, 0.189*a], [0.349*a, keep + 0.686*a, 0.168*a], [0.272*a, 0.534*a, keep + 0.131*a]]
}

fn hue_rotate_matrix(degrees: f32) -> [[f32; 3]; 3] {
    let radians = degrees.to_radians();
    let c = radians.cos();
    let s = radians.sin();
    [[0.213 + c*0.787 - s*0.213, 0.715 - c*0.715 - s*0.715, 0.072 - c*0.072 + s*0.928], [0.213 - c*0.213 + s*0.143, 0.715 + c*0.285 + s*0.140, 0.072 - c*0.072 - s*0.283], [0.213 - c*0.213 - s*0.787, 0.715 - c*0.715 + s*0.715, 0.072 + c*0.928 + s*0.072]]
}

fn box_blur(canvas: &mut Canvas, radius: usize) {
    if radius == 0 || canvas.width == 0 || canvas.height == 0 {
        return;
    }
    let width = canvas.width as usize;
    let height = canvas.height as usize;
    let stride = width + 1;
    let mut sums = vec![[0u64; 4]; stride * (height + 1)];
    for y in 0..height {
        let mut row = [0u64; 4];
        for x in 0..width {
            let source = (y * width + x) * 4;
            let above = y * stride + x + 1;
            let current = (y + 1) * stride + x + 1;
            let alpha = canvas.pixels[source + 3] as u64;
            for channel in 0..3 {
                row[channel] += canvas.pixels[source + channel] as u64 * alpha;
                sums[current][channel] = sums[above][channel] + row[channel];
            }
            row[3] += alpha;
            sums[current][3] = sums[above][3] + row[3];
        }
    }
    for y in 0..height {
        for x in 0..width {
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius + 1).min(width);
            let y0 = y.saturating_sub(radius);
            let y1 = (y + radius + 1).min(height);
            let count = ((x1 - x0) * (y1 - y0)) as u64;
            let bottom_right = sums[y1 * stride + x1];
            let bottom_left = sums[y1 * stride + x0];
            let top_right = sums[y0 * stride + x1];
            let top_left = sums[y0 * stride + x0];
            let index = (y * width + x) * 4;
            let sum = |channel: usize| {
                bottom_right[channel] + top_left[channel]
                    - bottom_left[channel] - top_right[channel]
            };
            let alpha_sum = sum(3);
            for channel in 0..3 {
                canvas.pixels[index + channel] = if alpha_sum == 0 {
                    0
                } else {
                    (sum(channel) / alpha_sum).min(255) as u8
                };
            }
            canvas.pixels[index + 3] = (alpha_sum / count) as u8;
        }
    }
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
            if let Some(clip_rect) = clip
                && (px < clip_rect.x
                    || px >= clip_rect.x + clip_rect.width
                    || py < clip_rect.y
                    || py >= clip_rect.y + clip_rect.height)
            {
                continue;
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
    if src_a == 255 {
        pixel.copy_from_slice(&[color.r, color.g, color.b, color.a]);
        return;
    }
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
    const NMAX: usize = 5_552;
    let mut a = 1u32;
    let mut b = 0u32;
    for chunk in data.chunks(NMAX) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
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
                let (tile_w, tile_h) = background_size(style, area, area.width, area.height);
                let tile_w = tile_w.max(1.0);
                let tile_h = tile_h.max(1.0);
                let repeat = background_repeat(style);
                let fixed = background_attachment_fixed(style);
                let (pos_cw, pos_ch) = if fixed {
                    (viewport.width, viewport.height)
                } else {
                    (area.width, area.height)
                };
                let (position_x, position_y) =
                    background_position(style, pos_cw, pos_ch, tile_w, tile_h);
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
                        let origin_rect = Rect {
                            x: 0.0,
                            y: 0.0,
                            width: tile_w,
                            height: tile_h,
                        };
                        color::paint_linear_gradient(
                            &mut tile_canvas,
                            &gradient,
                            origin_rect,
                            None,
                        );
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
                        let tile_rect = Rect {
                            x: tx,
                            y: ty,
                            width: tile_w,
                            height: tile_h,
                        };
                        if let Some(ref img) = tile_image {
                            // Fast path: blit pre-rendered tile buffer
                            canvas.draw_image_scaled_clipped(img, tile_rect, clip.or(Some(area)));
                        } else {
                            // Fallback (no-repeat, tile too large, or image validation failed): render directly
                            color::paint_linear_gradient(
                                canvas,
                                &gradient,
                                tile_rect,
                                clip.or(Some(area)),
                            );
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

    let (tile_width, tile_height) = background_size(
        style,
        area,
        image.width().max(1) as f32,
        image.height().max(1) as f32,
    );
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
    let (position_x, position_y) = background_position(
        style,
        pos_container_w,
        pos_container_h,
        tile_width,
        tile_height,
    );
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
    /// alpha チャンネルへ複数回のbox blurを適用し、作業バッファを再利用する。
    /// カーネルは常に`2r+1`ピクセル幅で、端では実効カーネルサイズを調整する。
    pub(crate) fn box_blur_alpha_passes(&mut self, radius: u32, passes: usize) {
        if radius == 0 || passes == 0 {
            return;
        }
        let w = self.width as usize;
        let h = self.height as usize;
        let r = radius as usize;
        let mut alphas: Vec<u8> = self.pixels.iter().skip(3).step_by(4).copied().collect();
        let mut blurred = vec![0u8; w * h];

        for _ in 0..passes {
            // 水平方向 blur
            for y in 0..h {
                let row_start = y * w;
                let mut sum: u32 = 0;
                let init_right = r.min(w.saturating_sub(1));
                for x in 0..=init_right {
                    sum += alphas[row_start + x] as u32;
                }
                for x in 0..w {
                    let left = x.saturating_sub(r);
                    let right = (x + r).min(w.saturating_sub(1));
                    blurred[row_start + x] = (sum / (right - left + 1) as u32) as u8;
                    if x + r + 1 < w {
                        sum += alphas[row_start + x + r + 1] as u32;
                    }
                    if x >= r {
                        sum = sum.saturating_sub(alphas[row_start + x - r] as u32);
                    }
                }
            }

            // 垂直方向 blur。結果をalphasへ戻し、次のpassの入力として再利用する。
            for x in 0..w {
                let mut sum: u32 = 0;
                let init_bottom = r.min(h.saturating_sub(1));
                for y in 0..=init_bottom {
                    sum += blurred[y * w + x] as u32;
                }
                for y in 0..h {
                    let top = y.saturating_sub(r);
                    let bottom = (y + r).min(h.saturating_sub(1));
                    alphas[y * w + x] = (sum / (bottom - top + 1) as u32) as u8;
                    if y + r + 1 < h {
                        sum += blurred[(y + r + 1) * w + x] as u32;
                    }
                    if y >= r {
                        sum = sum.saturating_sub(blurred[(y - r) * w + x] as u32);
                    }
                }
            }
        }

        for (i, &a) in alphas.iter().enumerate() {
            self.pixels[i * 4 + 3] = a;
        }
    }

    /// 別キャンバス（shadow_buf）をメインキャンバスに合成する（clip あり）。
    /// `r`, `g`, `b` は合成時に使う色成分（影の色）。
    /// `offset_x`, `offset_y` は shadow_buf の左上隅がメインキャンバスのどこに対応するか。
    pub(crate) fn composite_canvas_clipped(
        &mut self,
        src: &Canvas,
        offset_x: i32,
        offset_y: i32,
        r: u8,
        g: u8,
        b: u8,
        clip: Option<Rect>,
    ) {
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
