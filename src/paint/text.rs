//! Text painting, text decoration, list markers, and inline image fragments.

use crate::css::{ComputedStyle, ComputedValue};
use crate::font::{Font, FontStyle, FontWeight, GlyphRaster, WebFontRegistry, load_default_text_fonts};
use crate::layout::{FragmentStyle, InlineFragmentContent, LayoutBox, ListMarker, Rect};

use super::border::{EdgeSizesForPaint, paint_rect_borders};
use super::color::{parse_color, Color};
use super::{
    background_color, length_property, paint_background_image,
    Canvas, Image,
};

/// Paint text using the provided fonts and an optional web font registry.
///
/// When `web_fonts` is `Some`, the fragment's `font_family`, `font_weight`, and
/// `font_style` are used to select the best web font variant before falling back
/// to the global `fonts` list.
pub(crate) fn paint_text_with_registry(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    style: &ComputedStyle,
    clip: Option<Rect>,
    _viewport: Rect,
    fonts: &[Font],
    web_fonts: Option<&WebFontRegistry>,
) {
    // Fallback color from the containing block's style (used when fragment
    // style has no explicit color).
    let fallback_color = text_color(style).unwrap_or(Color::rgb(0, 0, 0));
    // Containing-block decoration: `text-decoration-*` is NOT a CSS inherited
    // property, but decorations set on an ancestor box visually propagate to
    // descendant inline content.  We use the box-level style as a fallback so
    // that existing cases like `<p style="text-decoration:underline"><span>…</span></p>`
    // continue to work when the span itself has no explicit decoration.
    let block_decoration_line = text_decoration_line(style);
    let block_decoration_color = text_decoration_color(style, fallback_color);

    for line in &layout.lines {
        for fragment in &line.fragments {
            match &fragment.content {
                InlineFragmentContent::Text(text) => {
                    let font_size = fragment.metrics.font_size.max(1.0);

                    // Per-fragment style is used for text-transform and color so
                    // that nested inline elements (e.g. <span>) can have
                    // independent styling.
                    let frag_color = fragment_text_color(&fragment.style)
                        .unwrap_or(fallback_color);
                    let text_transform = fragment_text_transform(&fragment.style);

                    // For text-decoration, distinguish "property not present" from
                    // "present but none". If the fragment has an explicit
                    // text-decoration-line (even none), use it; otherwise fall back
                    // to the containing block's decoration.
                    let has_frag_decoration = fragment.style.text_decoration_line.is_some();
                    let (decoration_line, decoration_color) = if has_frag_decoration {
                        (fragment_decoration_line(&fragment.style), fragment_decoration_color(&fragment.style, frag_color))
                    } else {
                        (block_decoration_line, block_decoration_color)
                    };

                    let transformed = apply_text_transform(text, text_transform);
                    let display_text = transformed.as_deref().unwrap_or(text.as_str());

                    // Try to resolve the best web font variant for this fragment.
                    // If the fragment has a registered web-font family, use it as the
                    // primary font and fall back to the global system font list.
                    let web_font_for_fragment = web_fonts.and_then(|registry| {
                        let family = fragment.style.font_family.as_deref()?;
                        let weight = FontWeight::parse(
                            fragment.style.font_weight.as_deref().unwrap_or("normal"),
                        );
                        let style = FontStyle::parse(
                            fragment.style.font_style.as_deref().unwrap_or("normal"),
                        );
                        registry.select_best(family, weight, style)
                    });

                    if let Some(web_font) = web_font_for_fragment {
                        // Build a temporary font list: web variant first, then fallbacks
                        let mut variant_fonts: Vec<&Font> = vec![web_font];
                        variant_fonts.extend(fonts.iter());
                        paint_text_with_font_refs(
                            canvas,
                            fragment.rect,
                            display_text,
                            font_size,
                            fragment.metrics.ascent,
                            &variant_fonts,
                            frag_color,
                            clip,
                            fragment.metrics.letter_spacing,
                        );
                    } else if !fonts.is_empty() {
                        paint_text_with_font(
                            canvas,
                            fragment.rect,
                            display_text,
                            font_size,
                            fragment.metrics.ascent,
                            fonts,
                            frag_color,
                            clip,
                            fragment.metrics.letter_spacing,
                        );
                    } else {
                        // Fallback: placeholder rectangles
                        paint_text_placeholder(
                            canvas,
                            fragment.rect,
                            display_text,
                            font_size,
                            frag_color,
                            clip,
                            fragment.metrics.letter_spacing,
                        );
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
                    super::paint_generated_box(canvas, fragment.rect, style, clip, _viewport);
                }
            }
        }
    }
}

/// Returns the `text-transform` value from style.
/// Returns the `color` from a `FragmentStyle`, if present.
fn fragment_text_color(style: &FragmentStyle) -> Option<Color> {
    style.color.as_deref().and_then(parse_color)
}

/// Returns the `text-transform` keyword from a `FragmentStyle`.
/// The value is pre-normalized to lowercase in `FragmentStyle::from_computed`.
fn fragment_text_transform(style: &FragmentStyle) -> &'static str {
    match style.text_transform.as_deref() {
        Some("uppercase") => "uppercase",
        Some("lowercase") => "lowercase",
        Some("capitalize") => "capitalize",
        _ => "none",
    }
}

/// Returns `text-decoration-line` flags from a `FragmentStyle`.
/// The value is pre-normalized to lowercase in `FragmentStyle::from_computed`.
fn fragment_decoration_line(style: &FragmentStyle) -> TextDecorationLines {
    let mut lines = TextDecorationLines::default();
    if let Some(ref kw) = style.text_decoration_line {
        for part in kw.split_whitespace() {
            match part {
                "underline" => lines.underline = true,
                "overline" => lines.overline = true,
                "line-through" => lines.line_through = true,
                _ => {}
            }
        }
    }
    lines
}

/// Returns the `text-decoration-color` from a `FragmentStyle`, falling back to `fallback`.
fn fragment_decoration_color(style: &FragmentStyle, fallback: Color) -> Color {
    match &style.text_decoration_color {
        Some(s) => parse_color(s).unwrap_or(fallback),
        None => fallback,
    }
}

/// Apply text-transform to the given text, returning Some(transformed) or None if no change.
pub(crate) fn apply_text_transform(text: &str, transform: &str) -> Option<String> {
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

/// Text decoration line flags (supports multiple values like "underline line-through").
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TextDecorationLines {
    underline: bool,
    overline: bool,
    line_through: bool,
}

impl TextDecorationLines {
    pub(crate) fn is_none(&self) -> bool {
        !self.underline && !self.overline && !self.line_through
    }
}

/// Returns the text-decoration-line flags from style.
pub(crate) fn text_decoration_line(style: &ComputedStyle) -> TextDecorationLines {
    let mut lines = TextDecorationLines::default();
    if let Some(ComputedValue::Keyword(kw)) = style.get("text-decoration-line") {
        for part in kw.split_whitespace() {
            match part.to_ascii_lowercase().as_str() {
                "underline" => lines.underline = true,
                "overline" => lines.overline = true,
                "line-through" => lines.line_through = true,
                _ => {}
            }
        }
    }
    lines
}

/// Returns the text-decoration-color, falling back to the text color.
pub(crate) fn text_decoration_color(style: &ComputedStyle, fallback: Color) -> Color {
    use super::resolve_color_value;
    resolve_color_value(style.get("text-decoration-color"), style).unwrap_or(fallback)
}

/// Draw text decoration lines (underline, overline, line-through) for a fragment.
pub(crate) fn paint_text_decoration(
    canvas: &mut Canvas,
    rect: Rect,
    ascent: f32,
    descent: f32,
    font_size: f32,
    decoration: TextDecorationLines,
    color: Color,
    clip: Option<Rect>,
) {
    if decoration.is_none() {
        return;
    }

    let line_thickness = (font_size * 0.075).max(1.0);

    let mut draw_line = |line_y: f32| {
        let line_rect = Rect {
            x: rect.x,
            y: line_y,
            width: rect.width,
            height: line_thickness,
        };
        canvas.fill_rect_clipped(line_rect, color, clip);
    };

    if decoration.underline {
        draw_line(rect.y + ascent + descent * 0.5);
    }
    if decoration.overline {
        draw_line(rect.y);
    }
    if decoration.line_through {
        draw_line(rect.y + ascent * 0.6);
    }
}

/// Paint text using actual font glyphs.
pub(crate) fn paint_text_with_font(
    canvas: &mut Canvas,
    rect: Rect,
    text: &str,
    font_size: f32,
    layout_ascent: f32,
    fonts: &[Font],
    color: Color,
    clip: Option<Rect>,
    letter_spacing: f32,
) {
    // Align paint baseline with layout's line-box baseline model to avoid vertical drift.
    let baseline_y = rect.y + layout_ascent;
    let mut cursor_x = rect.x;
    let mut previous_char: Option<(char, usize)> = None;

    let chars: Vec<char> = text.chars().collect();
    let char_count = chars.len();
    for (i, &ch) in chars.iter().enumerate() {
        let (font_index, glyph, advance_x) = rasterize_with_fallback(fonts, ch, font_size);
        if let Some((prev, prev_font_index)) = previous_char
            && prev_font_index == font_index
        {
            cursor_x += fonts[font_index].glyph_kerning(prev, ch, font_size);
        }

        if let Some(glyph) = glyph {
            if glyph.width > 0 && glyph.height > 0 && !glyph.bitmap.is_empty() {
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

        cursor_x += advance_x;
        // Apply letter-spacing between characters only (not after the last one)
        if i + 1 < char_count {
            cursor_x += letter_spacing;
        }
        previous_char = Some((ch, font_index));
    }
}

/// Paint text using actual font glyphs, with fonts passed as references.
///
/// Identical to `paint_text_with_font` but accepts `&[&Font]` so that a
/// web-font variant can be prepended without cloning.
pub(crate) fn paint_text_with_font_refs(
    canvas: &mut Canvas,
    rect: Rect,
    text: &str,
    font_size: f32,
    layout_ascent: f32,
    fonts: &[&Font],
    color: Color,
    clip: Option<Rect>,
    letter_spacing: f32,
) {
    let baseline_y = rect.y + layout_ascent;
    let mut cursor_x = rect.x;
    let mut previous_char: Option<(char, usize)> = None;

    let chars: Vec<char> = text.chars().collect();
    let char_count = chars.len();
    for (i, &ch) in chars.iter().enumerate() {
        let (font_index, glyph, advance_x) = rasterize_with_fallback_refs(fonts, ch, font_size);
        if let Some((prev, prev_font_index)) = previous_char
            && prev_font_index == font_index
        {
            cursor_x += fonts[font_index].glyph_kerning(prev, ch, font_size);
        }

        if let Some(glyph) = glyph {
            if glyph.width > 0 && glyph.height > 0 && !glyph.bitmap.is_empty() {
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

        cursor_x += advance_x;
        if i + 1 < char_count {
            cursor_x += letter_spacing;
        }
        previous_char = Some((ch, font_index));
    }
}

pub(crate) fn load_text_fonts() -> Vec<Font> {
    load_default_text_fonts()
}

pub(crate) fn rasterize_with_fallback(
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
        // Web fonts (index 0) are tried first so that explicit @font-face
        // declarations take priority even for CJK characters.
        // If the web font has no glyph, fall through to CJK-capable system fonts.
        if let Some(result) = try_index(0) {
            return result;
        }
        for index in 1..fonts.len() {
            if let Some(result) = try_index(index) {
                return result;
            }
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

/// Like `rasterize_with_fallback` but accepts `&[&Font]` references.
pub(crate) fn rasterize_with_fallback_refs(
    fonts: &[&Font],
    ch: char,
    font_size: f32,
) -> (usize, Option<GlyphRaster>, f32) {
    let prefer_cjk = is_cjk_preferred_character(ch);
    let try_index = |index: usize| -> Option<(usize, Option<GlyphRaster>, f32)> {
        let font = fonts[index];
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
                if ch.is_whitespace() {
                    return Some((index, None, font.glyph_advance(ch, font_size)));
                }
            }
            Err(_) => return None,
        }
        None
    };

    if prefer_cjk && fonts.len() > 1 {
        // Web fonts (index 0) are tried first so that explicit @font-face
        // declarations take priority even for CJK characters.
        // If the web font has no glyph, fall through to CJK-capable system fonts.
        if let Some(result) = try_index(0) {
            return result;
        }
        for index in 1..fonts.len() {
            if let Some(result) = try_index(index) {
                return result;
            }
        }
    } else {
        for index in 0..fonts.len() {
            if let Some(result) = try_index(index) {
                return result;
            }
        }
    }

    let primary_advance = fonts
        .first()
        .map(|font| font.glyph_advance(ch, font_size))
        .unwrap_or((font_size * 0.6).max(1.0));
    (0, None, primary_advance)
}

pub(crate) use crate::font::is_cjk_preferred_character;

/// Paint text as placeholder rectangles (fallback when no font available).
///
/// `letter_spacing` is added between characters (after each character advance),
/// matching the CSS `letter-spacing` property so that the placeholder width
/// stays consistent with the layout-computed text width.
pub(crate) fn paint_text_placeholder(
    canvas: &mut Canvas,
    rect: Rect,
    text: &str,
    font_size: f32,
    color: Color,
    clip: Option<Rect>,
    letter_spacing: f32,
) {
    let mut cursor_x = rect.x;
    let advance = (font_size * 0.6).max(1.0); // Approximate advance
    let glyph_height = (font_size * 0.7).max(1.0);
    let glyph_y = rect.y + (font_size - glyph_height) * 0.5;

    let chars: Vec<char> = text.chars().collect();
    let char_count = chars.len();
    for (i, ch) in chars.iter().enumerate() {
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
        if i + 1 < char_count {
            cursor_x += letter_spacing;
        }
    }
}

/// Paints the list marker (if any) for a `display: list-item` box.
pub(crate) fn paint_list_marker(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    style: &ComputedStyle,
    clip: Option<Rect>,
    fonts: &[Font],
) {
    let Some(marker) = &layout.marker else {
        return;
    };

    let color = text_color(style).unwrap_or(Color::rgb(0, 0, 0));
    let font_size = marker.font_size.max(1.0);
    let ascent = font_size * 0.8;

    let rect = Rect {
        x: marker.x,
        y: marker.y,
        width: font_size * (marker.text.chars().count() as f32) * 0.6,
        height: font_size,
    };

    if !fonts.is_empty() {
        paint_text_with_font(
            canvas,
            rect,
            &marker.text,
            font_size,
            ascent,
            fonts,
            color,
            clip,
            0.0,
        );
    } else {
        paint_list_marker_placeholder(canvas, marker, font_size, color, clip);
    }
}

/// Returns `true` when the marker text is a bullet symbol (disc/circle/square).
///
/// These characters are Unicode glyphs that cannot be meaningfully rendered with
/// the placeholder-rectangle approach, so they fall back to a filled square.
/// Text-based markers (decimal, roman, alpha) can be rendered as rectangles with
/// `paint_text_placeholder`, which preserves the correct character count.
fn is_bullet_marker(text: &str) -> bool {
    // disc (U+2022 •), circle (U+25E6 ◦), square (U+25A0 ■)
    matches!(text, "\u{2022}" | "\u{25e6}" | "\u{25a0}")
}

/// Paints a list marker as a simple filled shape (fallback when no font is loaded).
///
/// - Bullet markers (disc/circle/square): rendered as a filled square.
/// - Text markers (decimal/roman/alpha): delegated to `paint_text_placeholder`
///   so that the correct number of character-width rectangles is drawn.
pub(crate) fn paint_list_marker_placeholder(
    canvas: &mut Canvas,
    marker: &ListMarker,
    font_size: f32,
    color: Color,
    clip: Option<Rect>,
) {
    if is_bullet_marker(&marker.text) {
        let size = (font_size * 0.35).max(2.0);
        let cx = marker.x + size * 0.5;
        let cy = marker.y + font_size * 0.5;

        // Render disc/circle/square as a filled square for simplicity in placeholder mode.
        canvas.fill_rect_clipped(
            Rect {
                x: cx - size * 0.5,
                y: cy - size * 0.5,
                width: size,
                height: size,
            },
            color,
            clip,
        );
    } else {
        // Text-based markers (e.g. "1.", "ii.", "a."): draw placeholder rectangles
        // per character. paint_text_placeholder sizes each char internally, so
        // rect.width is not used for rendering — we pass 0.0 to avoid confusion.
        let rect = Rect {
            x: marker.x,
            y: marker.y,
            width: 0.0,
            height: font_size,
        };
        // List markers do not have letter-spacing applied, so pass 0.0.
        paint_text_placeholder(canvas, rect, &marker.text, font_size, color, clip, 0.0);
    }
}

pub(crate) fn paint_inline_image_fragment(
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

pub(crate) fn inline_fragment_content_rect(
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

pub(crate) fn text_color(style: &ComputedStyle) -> Option<Color> {
    super::color_property(style.get("color"))
}
