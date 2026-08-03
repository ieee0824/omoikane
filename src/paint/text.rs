//! Text painting, text decoration, list markers, and inline image fragments.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::css::{ComputedStyle, ComputedValue};
use crate::font::{
    Font, FontError, FontStyle, FontWeight, GlyphRaster, WebFontRegistry,
    is_zero_advance_character, load_default_text_fonts,
};
use crate::layout::{FragmentStyle, InlineFragmentContent, LayoutBox, ListMarker, Rect};
use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};
use unicode_segmentation::UnicodeSegmentation;

use super::border::{EdgeSizesForPaint, paint_rect_borders};
use super::color::{parse_color, Color};
use super::{
    background_color, length_property, paint_background_image,
    Canvas, Image,
};

const MAX_RENDER_GLYPH_CACHE_ENTRIES: usize = 16_384;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct RenderGlyphCacheKey {
    font_identity: usize,
    ch: char,
    size_bits: u32,
}

#[derive(Default)]
struct RenderGlyphCache {
    active: bool,
    glyphs: HashMap<RenderGlyphCacheKey, Arc<GlyphRaster>>,
    #[cfg(test)]
    hits: usize,
    #[cfg(test)]
    misses: usize,
}

thread_local! {
    static RENDER_GLYPH_CACHE: RefCell<RenderGlyphCache> = RefCell::new(RenderGlyphCache::default());
}

struct RenderGlyphCacheGuard;

impl Drop for RenderGlyphCacheGuard {
    fn drop(&mut self) {
        RENDER_GLYPH_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            cache.active = false;
            cache.glyphs.clear();
        });
    }
}

/// Shares rasterized glyphs between text fragments in one paint operation.
/// Clearing both boundaries ensures pointer identities never outlive fonts.
pub(crate) fn with_render_glyph_cache<T>(paint: impl FnOnce() -> T) -> T {
    let owns_cache = RENDER_GLYPH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.active {
            return false;
        }
        cache.glyphs.clear();
        cache.active = true;
        #[cfg(test)]
        {
            cache.hits = 0;
            cache.misses = 0;
        }
        true
    });
    let _guard = if owns_cache {
        Some(RenderGlyphCacheGuard)
    } else {
        None
    };
    paint()
}

fn rasterize_cached(font: &Font, ch: char, size_px: f32) -> Result<Arc<GlyphRaster>, FontError> {
    let key = RenderGlyphCacheKey {
        font_identity: std::ptr::from_ref(font) as usize,
        ch,
        size_bits: size_px.to_bits(),
    };
    if let Some(glyph) = RENDER_GLYPH_CACHE.with(|cache| {
        let glyph = {
            let cache = cache.borrow();
            if !cache.active {
                return None;
            }
            cache.glyphs.get(&key).cloned()
        };
        #[cfg(test)]
        if glyph.is_some() {
            cache.borrow_mut().hits += 1;
        }
        glyph
    }) {
        return Ok(glyph);
    }

    let glyph = Arc::new(font.rasterize(ch, size_px)?);
    RENDER_GLYPH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.active {
            #[cfg(test)]
            {
                cache.misses += 1;
            }
            if cache.glyphs.len() < MAX_RENDER_GLYPH_CACHE_ENTRIES {
                cache.glyphs.insert(key, glyph.clone());
            }
        }
    });
    Ok(glyph)
}

#[cfg(test)]
pub(crate) fn render_glyph_cache_stats() -> (usize, usize) {
    RENDER_GLYPH_CACHE.with(|cache| {
        let cache = cache.borrow();
        (cache.hits, cache.misses)
    })
}

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
    fonts: &[Arc<Font>],
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
                    let transformed_text = transformed.as_deref().unwrap_or(text.as_str());
                    // Resolve bidi before selecting the physical paint axis.
                    // The vertical painter already maps the resulting visual
                    // sequence onto its direction-aware column cursor, so it
                    // must consume the same UAX#9 order as horizontal paint.
                    let (visual_text, vertical_mode) =
                        fragment_text_for_paint(transformed_text, &fragment.style);
                    let display_text = visual_text.as_ref();

                    // Try to resolve the best web font variant for this fragment.
                    // If the fragment has a registered web-font family, use it as the
                    // primary font and fall back to the global system font list.
                    let web_font_for_fragment =
                        select_fragment_web_font(web_fonts, &fragment.style);
                    if let Some(web_font) = web_font_for_fragment {
                        // Build a temporary font list: web variant first, then fallbacks
                        let mut variant_fonts: Vec<&Font> = vec![web_font];
                        variant_fonts.extend(fonts.iter().map(|font| font.as_ref()));
                        paint_fragment_text(
                            canvas,
                            fragment.rect,
                            display_text,
                            font_size,
                            fragment.metrics.ascent,
                            &variant_fonts,
                            frag_color,
                            clip,
                            fragment.metrics.letter_spacing,
                            vertical_mode,
                        );
                    } else if !fonts.is_empty() {
                        if let Some(vertical_mode) = vertical_mode {
                            let font_refs: Vec<&Font> =
                                fonts.iter().map(|font| font.as_ref()).collect();
                            paint_fragment_text(
                                canvas,
                                fragment.rect,
                                display_text,
                                font_size,
                                fragment.metrics.ascent,
                                &font_refs,
                                frag_color,
                                clip,
                                fragment.metrics.letter_spacing,
                                Some(vertical_mode),
                            );
                        } else {
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
                        }
                    } else {
                        // Fallback: placeholder rectangles
                        paint_text_placeholder_with_mode(
                            canvas,
                            fragment.rect,
                            display_text,
                            font_size,
                            frag_color,
                            clip,
                            fragment.metrics.letter_spacing,
                            vertical_mode,
                        );
                    }

                    // Draw text decorations after text
                    if let Some((vertical_rl, _)) = vertical_mode {
                        paint_text_decoration_vertical(
                            canvas,
                            fragment.rect,
                            font_size,
                            decoration_line,
                            decoration_color,
                            clip,
                            vertical_rl,
                        );
                    } else {
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
                InlineFragmentContent::FormControl(style, value, editing) => {
                    if let Some(background) = background_color(style) {
                        canvas.fill_rect_clipped(fragment.rect, background, clip);
                    }
                    let border = EdgeSizesForPaint::from_style(style);
                    if border.total_horizontal() > 0.0 || border.total_vertical() > 0.0 {
                        paint_rect_borders(canvas, fragment.rect, style, border, clip);
                    }
                    let content_rect = inline_fragment_content_rect(fragment.rect, style, border);
                    let color = fragment_text_color(&fragment.style).unwrap_or(fallback_color);
                    // Same font policy as the Text branch: the fragment's
                    // resolved web-font variant first, then the global fonts.
                    let mut fragment_fonts: Vec<&Font> = Vec::new();
                    if let Some(web_font) = select_fragment_web_font(web_fonts, &fragment.style) {
                        fragment_fonts.push(web_font);
                    }
                    fragment_fonts.extend(fonts.iter().map(|font| font.as_ref()));
                    let x_offset = if is_text_align_center(style) {
                        let text_width = measure_form_control_text_width(
                            value,
                            fragment.metrics.font_size,
                            &fragment_fonts,
                            fragment.metrics.letter_spacing,
                        );
                        ((content_rect.width - text_width) / 2.0).max(0.0)
                    } else {
                        0.0
                    };
                    let text_rect = Rect {
                        x: content_rect.x + x_offset,
                        y: content_rect.y
                            + ((content_rect.height - fragment.metrics.font_size) / 2.0).max(0.0),
                        width: (content_rect.width - x_offset).max(0.0),
                        height: fragment.metrics.font_size,
                    };
                    let mut caret_x = None;
                    if let Some(editing) = editing.filter(|state| state.focused) {
                        let before = text_prefix_by_utf16_offset(value, editing.selection_start);
                        let selected = text_prefix_by_utf16_offset(value, editing.selection_end);
                        let start_x = text_rect.x
                            + measure_form_control_text_width(
                                before,
                                fragment.metrics.font_size,
                                &fragment_fonts,
                                fragment.metrics.letter_spacing,
                            );
                        let end_x = text_rect.x
                            + measure_form_control_text_width(
                                selected,
                                fragment.metrics.font_size,
                                &fragment_fonts,
                                fragment.metrics.letter_spacing,
                            );
                        if editing.selection_start != editing.selection_end {
                            canvas.fill_rect_clipped(
                                Rect {
                                    x: start_x,
                                    y: text_rect.y,
                                    width: (end_x - start_x).max(1.0),
                                    height: text_rect.height,
                                },
                                Color::rgba(51, 153, 255, 120),
                                clip,
                            );
                        } else {
                            caret_x = Some(start_x);
                        }
                    }
                    if !value.is_empty() {
                        if fragment_fonts.is_empty() {
                            paint_text_placeholder(
                                canvas,
                                text_rect,
                                value,
                                fragment.metrics.font_size,
                                color,
                                clip,
                                fragment.metrics.letter_spacing,
                            );
                        } else {
                            paint_text_with_font_refs(
                                canvas,
                                text_rect,
                                value,
                                fragment.metrics.font_size,
                                fragment.metrics.ascent,
                                &fragment_fonts,
                                color,
                                clip,
                                fragment.metrics.letter_spacing,
                            );
                        }
                    }
                    if let Some(x) = caret_x {
                        canvas.fill_rect_clipped(
                            Rect { x, y: text_rect.y, width: 1.0, height: text_rect.height },
                            color,
                            clip,
                        );
                    }
                }
                InlineFragmentContent::IconFormControl(style, image, width, height) => {
                    if let Some(background) = background_color(style) {
                        canvas.fill_rect_clipped(fragment.rect, background, clip);
                    }
                    let border = EdgeSizesForPaint::from_style(style);
                    if border.total_horizontal() > 0.0 || border.total_vertical() > 0.0 {
                        paint_rect_borders(canvas, fragment.rect, style, border, clip);
                    }
                    let content_rect = inline_fragment_content_rect(fragment.rect, style, border);
                    let image_rect = Rect {
                        x: content_rect.x + ((content_rect.width - width) / 2.0).max(0.0),
                        y: content_rect.y + ((content_rect.height - height) / 2.0).max(0.0),
                        width: (*width).min(content_rect.width),
                        height: (*height).min(content_rect.height),
                    };
                    canvas.draw_image_scaled_clipped(image, image_rect, clip);
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
    fonts: &[Arc<Font>],
    color: Color,
    clip: Option<Rect>,
    letter_spacing: f32,
) {
    // Align paint baseline with layout's line-box baseline model to avoid vertical drift.
    let baseline_y = rect.y + layout_ascent;
    let mut cursor_x = rect.x;
    let mut cluster_origin_x = rect.x;
    let mut previous_char: Option<(char, usize)> = None;

    let chars: Vec<char> = text.chars().collect();
    let mut remaining_non_zero = chars
        .iter()
        .filter(|ch| !is_zero_advance_character(**ch))
        .count();
    for &ch in &chars {
        let zero_advance = is_zero_advance_character(ch);
        let preferred_font = zero_advance.then(|| previous_char.map(|(_, index)| index)).flatten();
        let (font_index, glyph, advance_x) = rasterize_with_fallback_preferred(
            fonts,
            ch,
            font_size,
            preferred_font,
        );
        let glyph = (!is_invisible_shaping_control(ch)).then_some(glyph).flatten();
        if !zero_advance
            && let Some((prev, prev_font_index)) = previous_char
            && prev_font_index == font_index
        {
            cursor_x += fonts[font_index].glyph_kerning(prev, ch, font_size);
        }
        if !zero_advance {
            cluster_origin_x = cursor_x;
        }

        if let Some(glyph) = glyph
            && glyph.width > 0 && glyph.height > 0 && !glyph.bitmap.is_empty() {
                let glyph_x = horizontal_glyph_origin(
                    cursor_x,
                    cluster_origin_x,
                    zero_advance,
                    glyph.offset_x,
                );
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

        cursor_x += advance_x;
        // Apply letter-spacing between characters only (not after the last one)
        if !zero_advance && remaining_non_zero > 1 {
            cursor_x += letter_spacing;
        }
        if !zero_advance {
            previous_char = Some((ch, font_index));
            remaining_non_zero -= 1;
        }
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
    let mut cluster_origin_x = rect.x;
    let mut previous_char: Option<(char, usize)> = None;

    let chars: Vec<char> = text.chars().collect();
    let mut remaining_non_zero = chars
        .iter()
        .filter(|ch| !is_zero_advance_character(**ch))
        .count();
    for &ch in &chars {
        let zero_advance = is_zero_advance_character(ch);
        let preferred_font = zero_advance.then(|| previous_char.map(|(_, index)| index)).flatten();
        let (font_index, glyph, advance_x) = rasterize_with_fallback_refs_preferred(
            fonts,
            ch,
            font_size,
            preferred_font,
        );
        let glyph = (!is_invisible_shaping_control(ch)).then_some(glyph).flatten();
        if !zero_advance
            && let Some((prev, prev_font_index)) = previous_char
            && prev_font_index == font_index
        {
            cursor_x += fonts[font_index].glyph_kerning(prev, ch, font_size);
        }
        if !zero_advance {
            cluster_origin_x = cursor_x;
        }

        if let Some(glyph) = glyph
            && glyph.width > 0 && glyph.height > 0 && !glyph.bitmap.is_empty() {
                let glyph_x = horizontal_glyph_origin(
                    cursor_x,
                    cluster_origin_x,
                    zero_advance,
                    glyph.offset_x,
                );
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

        cursor_x += advance_x;
        if !zero_advance && remaining_non_zero > 1 {
            cursor_x += letter_spacing;
        }
        if !zero_advance {
            previous_char = Some((ch, font_index));
            remaining_non_zero -= 1;
        }
    }
}

/// Returns the vertical paint mode carried by an inline fragment.
///
/// The first flag selects the vertical-rl glyph rotation (clockwise); the
/// second flag selects the inline base direction.  `None` keeps the existing
/// horizontal paint path byte-for-byte.
fn vertical_paint_mode(style: &FragmentStyle) -> Option<(bool, bool)> {
    let writing_mode = style.writing_mode.as_deref()?;
    let vertical_rl = matches!(writing_mode, "vertical-rl" | "sideways-rl");
    let vertical = vertical_rl || matches!(writing_mode, "vertical-lr" | "sideways-lr");
    vertical.then_some((vertical_rl, style.direction.as_deref() == Some("rtl")))
}

/// Resolves the text and physical inline axis as one paint-time decision.
///
/// Keeping these values together prevents a writing-mode-specific call site
/// from bypassing bidi resolution while the horizontal path still uses it.
fn fragment_text_for_paint<'a>(
    text: &'a str,
    style: &FragmentStyle,
) -> (Cow<'a, str>, Option<(bool, bool)>) {
    (bidi_visual_text(text, style), vertical_paint_mode(style))
}

/// Reorders one inline text fragment into Unicode visual order for painting.
///
/// Layout continues to measure and expose the source string in logical DOM
/// order. The paint cursor, however, advances along the physical inline axis,
/// so mixed-direction runs must be reordered before glyphs are rasterized.
/// `normal`, `embed`, and `isolate` use the declared base direction;
/// `plaintext` lets UAX#9 derive each paragraph's level from its first strong
/// character. Directional override modes force every grapheme cluster into
/// the declared direction while preserving the logical DOM/layout string.
fn bidi_visual_text<'a>(text: &'a str, style: &FragmentStyle) -> Cow<'a, str> {
    if text.is_empty() {
        return Cow::Borrowed(text);
    }

    if let Some(level) = style.resolved_bidi_level {
        if level % 2 == 0 {
            return Cow::Borrowed(text);
        }
        let mut visual = String::with_capacity(text.len());
        for cluster in text.graphemes(true).rev() {
            visual.push_str(cluster);
        }
        return Cow::Owned(visual);
    }

    let explicit_level = if style.direction.as_deref() == Some("rtl") {
        Level::rtl()
    } else {
        Level::ltr()
    };
    let paragraph_level = match style.unicode_bidi.as_deref() {
        Some("bidi-override" | "isolate-override") => {
            return bidi_override_visual_text(text, style);
        }
        Some("plaintext") => None,
        None | Some("normal" | "embed" | "isolate") => Some(explicit_level),
        _ => return Cow::Borrowed(text),
    };

    if !contains_bidi_rtl_candidate(text) {
        return Cow::Borrowed(text);
    }

    let bidi = BidiInfo::new(text, paragraph_level);
    if !bidi.has_rtl() {
        return Cow::Borrowed(text);
    }

    let mut visual = String::with_capacity(text.len());
    for paragraph in &bidi.paragraphs {
        visual.push_str(bidi.reorder_line(paragraph, paragraph.range.clone()).as_ref());
    }
    Cow::Owned(visual)
}

/// Applies CSS directional override without splitting extended grapheme
/// clusters. LTR override is already logical order; RTL override reverses the
/// cluster sequence while leaving every cluster's scalar order intact.
fn bidi_override_visual_text<'a>(text: &'a str, style: &FragmentStyle) -> Cow<'a, str> {
    if style.direction.as_deref() != Some("rtl") {
        return Cow::Borrowed(text);
    }

    let Some(first_cluster) = text.graphemes(true).next() else {
        return Cow::Borrowed(text);
    };
    if first_cluster.len() == text.len() {
        return Cow::Borrowed(text);
    }

    let mut visual = String::with_capacity(text.len());
    for cluster in text.graphemes(true).rev() {
        visual.push_str(cluster);
    }
    Cow::Owned(visual)
}

/// Returns scalar paint order while reversing vertical RTL by grapheme rather
/// than by code point, so marks/selectors/joiners stay with their base glyph.
fn vertical_paint_characters(text: &str, direction_rtl: bool) -> Vec<char> {
    if direction_rtl {
        text.graphemes(true).rev().flat_map(str::chars).collect()
    } else {
        text.chars().collect()
    }
}

fn horizontal_glyph_origin(
    cursor_x: f32,
    cluster_origin_x: f32,
    zero_advance: bool,
    offset_x: f32,
) -> f32 {
    (if zero_advance { cluster_origin_x } else { cursor_x }) + offset_x
}

fn is_invisible_shaping_control(ch: char) -> bool {
    matches!(ch as u32, 0xfe00..=0xfe0f | 0xe0100..=0xe01ef | 0x200c | 0x200d)
}

fn vertical_glyph_cell(
    cursor_y: f32,
    previous_cell: Option<(f32, f32, usize)>,
    zero_advance: bool,
    direction_rtl: bool,
    advance: f32,
) -> (f32, f32) {
    if zero_advance {
        previous_cell.map_or((cursor_y, 0.0), |(start, advance, _)| (start, advance))
    } else if direction_rtl {
        (cursor_y - advance, advance)
    } else {
        (cursor_y, advance)
    }
}

fn vertical_cursor_after(
    cursor_y: f32,
    cell_start: f32,
    advance: f32,
    spacing: f32,
    zero_advance: bool,
    direction_rtl: bool,
) -> f32 {
    if zero_advance {
        cursor_y
    } else if direction_rtl {
        cell_start - spacing
    } else {
        cell_start + advance + spacing
    }
}

/// Avoid running the full paragraph algorithm for the overwhelmingly common
/// pure-LTR paint fragments. Explicit RTL/embedding controls are included so
/// that their presence still reaches the UAX#9 resolver.
fn contains_bidi_rtl_candidate(text: &str) -> bool {
    if text.is_ascii() {
        return false;
    }
    text.chars().any(|ch| {
        matches!(
            bidi_class(ch),
            BidiClass::R
                | BidiClass::AL
                | BidiClass::AN
                | BidiClass::RLE
                | BidiClass::RLI
                | BidiClass::RLO
                | BidiClass::FSI
        )
    })
}

/// Paints one text fragment using the direction produced by vertical inline
/// layout.  Latin and other horizontal glyphs are rotated into the vertical
/// column; CJK glyphs still use the same deterministic fallback font selection.
fn paint_fragment_text(
    canvas: &mut Canvas,
    rect: Rect,
    text: &str,
    font_size: f32,
    layout_ascent: f32,
    fonts: &[&Font],
    color: Color,
    clip: Option<Rect>,
    letter_spacing: f32,
    vertical_mode: Option<(bool, bool)>,
) {
    if let Some((vertical_rl, direction_rtl)) = vertical_mode {
        paint_text_vertical_with_font_refs(
            canvas,
            rect,
            text,
            font_size,
            layout_ascent,
            fonts,
            color,
            clip,
            letter_spacing,
            vertical_rl,
            direction_rtl,
        );
    } else {
        paint_text_with_font_refs(
            canvas,
            rect,
            text,
            font_size,
            layout_ascent,
            fonts,
            color,
            clip,
            letter_spacing,
        );
    }
}

/// Paints a horizontal glyph run into a vertical line box.  The layout stage
/// has already transposed the fragment rectangle, so this function only maps
/// glyph advances onto the physical y axis and keeps the paint order
/// direction-aware.
fn paint_text_vertical_with_font_refs(
    canvas: &mut Canvas,
    rect: Rect,
    text: &str,
    font_size: f32,
    _layout_ascent: f32,
    fonts: &[&Font],
    color: Color,
    clip: Option<Rect>,
    letter_spacing: f32,
    vertical_rl: bool,
    direction_rtl: bool,
) {
    let chars = vertical_paint_characters(text, direction_rtl);

    let mut cursor_y = if direction_rtl {
        rect.y + rect.height
    } else {
        rect.y
    };
    let clockwise = vertical_rl;
    let mut previous_cell = None;
    let mut remaining_non_zero = chars
        .iter()
        .filter(|ch| !is_zero_advance_character(**ch))
        .count();
    for ch in chars.iter().copied() {
        let zero_advance = is_zero_advance_character(ch);
        let preferred_font = zero_advance
            .then(|| previous_cell.map(|(_, _, index)| index))
            .flatten();
        let (font_index, glyph, advance_x) = rasterize_with_fallback_refs_preferred(
            fonts,
            ch,
            font_size,
            preferred_font,
        );
        let glyph = (!is_invisible_shaping_control(ch)).then_some(glyph).flatten();
        let advance = if zero_advance {
            0.0
        } else {
            advance_x.max(1.0)
        };
        let (cell_start, paint_advance) = vertical_glyph_cell(
            cursor_y,
            previous_cell,
            zero_advance,
            direction_rtl,
            advance,
        );

        if let Some(glyph) = glyph
            && glyph.width > 0
            && glyph.height > 0
            && !glyph.bitmap.is_empty()
        {
            let rotated_width = glyph.height as f32;
            let rotated_height = glyph.width as f32;
            let glyph_x = rect.x + ((rect.width - rotated_width) * 0.5).max(0.0);
            let glyph_y = cell_start + ((paint_advance - rotated_height) * 0.5).max(0.0);
            draw_rotated_glyph_mask(
                canvas,
                glyph_x,
                glyph_y,
                glyph.width,
                glyph.height,
                &glyph.bitmap,
                clockwise,
                color,
                clip,
            );
        }

        let spacing = if !zero_advance && remaining_non_zero > 1 {
            letter_spacing
        } else {
            0.0
        };
        cursor_y = vertical_cursor_after(
            cursor_y,
            cell_start,
            advance,
            spacing,
            zero_advance,
            direction_rtl,
        );
        if !zero_advance {
            previous_cell = Some((cell_start, advance, font_index));
            remaining_non_zero -= 1;
        }
    }
}

/// Rotates a single-channel glyph mask without changing the canvas API.  A
/// vertical-rl column uses clockwise rotation; vertical-lr uses the inverse.
fn draw_rotated_glyph_mask(
    canvas: &mut Canvas,
    x: f32,
    y: f32,
    width: u32,
    height: u32,
    mask: &[u8],
    clockwise: bool,
    color: Color,
    clip: Option<Rect>,
) {
    let expected = width as usize * height as usize;
    if mask.len() < expected || width == 0 || height == 0 {
        return;
    }
    let mut rotated = vec![0; expected];
    for source_y in 0..height {
        for source_x in 0..width {
            let destination = if clockwise {
                (source_x * height + (height - 1 - source_y)) as usize
            } else {
                ((width - 1 - source_x) * height + source_y) as usize
            };
            rotated[destination] = mask[(source_y * width + source_x) as usize];
        }
    }
    canvas.draw_glyph_mask(x, y, height, width, &rotated, color, clip);
}

/// Loads the default system text fonts, shared via `Arc` so that layout and
/// paint can reuse a single set without re-reading font files from disk.
pub(crate) fn load_text_fonts() -> Vec<Arc<Font>> {
    load_default_text_fonts().into_iter().map(Arc::new).collect()
}

pub(crate) fn rasterize_with_fallback(
    fonts: &[Arc<Font>],
    ch: char,
    font_size: f32,
) -> (usize, Option<Arc<GlyphRaster>>, f32) {
    let prefer_cjk = is_cjk_preferred_character(ch);
    let try_index = |index: usize| -> Option<(usize, Option<Arc<GlyphRaster>>, f32)> {
        let font = &fonts[index];
        // Missing glyphs must not rasterize as .notdef until every real
        // fallback candidate has been exhausted.
        if !ch.is_whitespace() && !font.has_glyph(ch) {
            return None;
        }
        match rasterize_cached(font, ch, font_size) {
            Ok(glyph) => {
                if glyph.width > 0 && glyph.height > 0 && !glyph.bitmap.is_empty() {
                    let advance = if is_zero_advance_character(ch) {
                        0.0
                    } else if glyph.advance_x > 0.0 {
                        glyph.advance_x
                    } else {
                        font.glyph_advance(ch, font_size)
                    };
                    return Some((index, Some(glyph), advance));
                }

                // Whitespace and control-like glyphs can be outline-less but still have advance.
                if ch.is_whitespace() || is_zero_advance_character(ch) {
                    return Some((
                        index,
                        None,
                        if is_zero_advance_character(ch) {
                            0.0
                        } else {
                            font.glyph_advance(ch, font_size)
                        },
                    ));
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

    if is_zero_advance_character(ch) {
        return (0, None, 0.0);
    }

    // No font owns the character. Render the primary font's .notdef as the
    // visible last resort, while retaining an advance if rasterization fails.
    if let Some(primary) = fonts.first() {
        if let Ok(glyph) = rasterize_cached(primary, ch, font_size)
            && glyph.width > 0
            && glyph.height > 0
            && !glyph.bitmap.is_empty()
        {
            let advance = if glyph.advance_x > 0.0 {
                glyph.advance_x
            } else {
                primary.glyph_advance(ch, font_size)
            };
            return (0, Some(glyph), advance);
        }
        return (0, None, primary.glyph_advance(ch, font_size));
    }
    (0, None, (font_size * 0.6).max(1.0))
}

fn rasterize_with_fallback_preferred(
    fonts: &[Arc<Font>],
    ch: char,
    font_size: f32,
    preferred: Option<usize>,
) -> (usize, Option<Arc<GlyphRaster>>, f32) {
    if let Some(index) = preferred.filter(|index| fonts[*index].has_glyph(ch))
        && let Ok(glyph) = rasterize_cached(&fonts[index], ch, font_size)
    {
        return (index, Some(glyph), 0.0);
    }
    rasterize_with_fallback(fonts, ch, font_size)
}

/// Like `rasterize_with_fallback` but accepts `&[&Font]` references.
pub(crate) fn rasterize_with_fallback_refs(
    fonts: &[&Font],
    ch: char,
    font_size: f32,
) -> (usize, Option<Arc<GlyphRaster>>, f32) {
    let prefer_cjk = is_cjk_preferred_character(ch);
    let try_index = |index: usize| -> Option<(usize, Option<Arc<GlyphRaster>>, f32)> {
        let font = fonts[index];
        if !ch.is_whitespace() && !font.has_glyph(ch) {
            return None;
        }
        match rasterize_cached(font, ch, font_size) {
            Ok(glyph) => {
                if glyph.width > 0 && glyph.height > 0 && !glyph.bitmap.is_empty() {
                    let advance = if is_zero_advance_character(ch) {
                        0.0
                    } else if glyph.advance_x > 0.0 {
                        glyph.advance_x
                    } else {
                        font.glyph_advance(ch, font_size)
                    };
                    return Some((index, Some(glyph), advance));
                }
                if ch.is_whitespace() || is_zero_advance_character(ch) {
                    return Some((
                        index,
                        None,
                        if is_zero_advance_character(ch) {
                            0.0
                        } else {
                            font.glyph_advance(ch, font_size)
                        },
                    ));
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

    if is_zero_advance_character(ch) {
        return (0, None, 0.0);
    }

    if let Some(primary) = fonts.first() {
        if let Ok(glyph) = rasterize_cached(primary, ch, font_size)
            && glyph.width > 0
            && glyph.height > 0
            && !glyph.bitmap.is_empty()
        {
            let advance = if glyph.advance_x > 0.0 {
                glyph.advance_x
            } else {
                primary.glyph_advance(ch, font_size)
            };
            return (0, Some(glyph), advance);
        }
        return (0, None, primary.glyph_advance(ch, font_size));
    }
    (0, None, (font_size * 0.6).max(1.0))
}

fn rasterize_with_fallback_refs_preferred(
    fonts: &[&Font],
    ch: char,
    font_size: f32,
    preferred: Option<usize>,
) -> (usize, Option<Arc<GlyphRaster>>, f32) {
    if let Some(index) = preferred.filter(|index| fonts[*index].has_glyph(ch))
        && let Ok(glyph) = rasterize_cached(fonts[index], ch, font_size)
    {
        return (index, Some(glyph), 0.0);
    }
    rasterize_with_fallback_refs(fonts, ch, font_size)
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
    let mut remaining_non_zero = chars
        .iter()
        .filter(|ch| !is_zero_advance_character(**ch))
        .count();
    for ch in chars.iter().copied() {
        let zero_advance = is_zero_advance_character(ch);
        if !ch.is_whitespace() && !zero_advance {
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
        if !zero_advance {
            cursor_x += advance;
        }
        if !zero_advance && remaining_non_zero > 1 {
            cursor_x += letter_spacing;
        }
        if !zero_advance {
            remaining_non_zero -= 1;
        }
    }
}

pub(crate) fn paint_text_placeholder_with_mode(
    canvas: &mut Canvas,
    rect: Rect,
    text: &str,
    font_size: f32,
    color: Color,
    clip: Option<Rect>,
    letter_spacing: f32,
    vertical_mode: Option<(bool, bool)>,
) {
    let Some((_vertical_rl, direction_rtl)) = vertical_mode else {
        paint_text_placeholder(canvas, rect, text, font_size, color, clip, letter_spacing);
        return;
    };

    let chars = vertical_paint_characters(text, direction_rtl);
    let advance = (font_size * 0.6).max(1.0);
    let glyph_width = (font_size * 0.7).min(rect.width).max(1.0);
    let glyph_height = (font_size * 0.45).max(1.0);
    let mut cursor_y = if direction_rtl {
        rect.y + rect.height
    } else {
        rect.y
    };
    let mut remaining_non_zero = chars
        .iter()
        .filter(|ch| !is_zero_advance_character(**ch))
        .count();
    for ch in chars.iter().copied() {
        let zero_advance = is_zero_advance_character(ch);
        let cell_start = if zero_advance {
            cursor_y
        } else if direction_rtl {
            cursor_y - advance
        } else {
            cursor_y
        };
        if !ch.is_whitespace() && !zero_advance {
            canvas.fill_rect_clipped(
                Rect {
                    x: rect.x + ((rect.width - glyph_width) * 0.5).max(0.0),
                    y: cell_start + ((advance - glyph_height) * 0.5).max(0.0),
                    width: glyph_width,
                    height: glyph_height,
                },
                color,
                clip,
            );
        }
        let spacing = if !zero_advance && remaining_non_zero > 1 {
            letter_spacing
        } else {
            0.0
        };
        cursor_y = if direction_rtl {
            if zero_advance {
                cell_start
            } else {
                cell_start - spacing
            }
        } else {
            if zero_advance {
                cell_start
            } else {
                cell_start + advance + spacing
            }
        };
        if !zero_advance {
            remaining_non_zero -= 1;
        }
    }
}

fn paint_text_decoration_vertical(
    canvas: &mut Canvas,
    rect: Rect,
    font_size: f32,
    decoration: TextDecorationLines,
    color: Color,
    clip: Option<Rect>,
    vertical_rl: bool,
) {
    if decoration.is_none() {
        return;
    }
    let thickness = (font_size * 0.075).max(1.0);
    let draw = |canvas: &mut Canvas, x: f32| {
        canvas.fill_rect_clipped(
            Rect {
                x,
                y: rect.y,
                width: thickness,
                height: rect.height,
            },
            color,
            clip,
        );
    };
    if decoration.underline {
        draw(
            canvas,
            if vertical_rl {
                rect.x + rect.width - thickness
            } else {
                rect.x
            },
        );
    }
    if decoration.overline {
        draw(
            canvas,
            if vertical_rl {
                rect.x
            } else {
                rect.x + rect.width - thickness
            },
        );
    }
    if decoration.line_through {
        draw(canvas, rect.x + ((rect.width - thickness) * 0.5).max(0.0));
    }
}

/// Paints the list marker (if any) for a `display: list-item` box.
pub(crate) fn paint_list_marker(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    style: &ComputedStyle,
    clip: Option<Rect>,
    fonts: &[Arc<Font>],
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
    let destination = super::image::object_fit_destination(
        content_rect,
        image.width as f32,
        image.height as f32,
        style,
    );
    // Clipping to the content box is what turns `cover`'s overflow into a crop.
    let clip = match clip {
        Some(clip) => match super::intersect(clip, content_rect) {
            Some(intersection) => intersection,
            None => return,
        },
        None => content_rect,
    };
    canvas.draw_image_scaled_clipped(image, destination, Some(clip));
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

/// Returns `true` when `text-align: center` is set on `style`.
fn is_text_align_center(style: &ComputedStyle) -> bool {
    matches!(
        style.get("text-align"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("center")
    )
}

/// Resolves the best registered web-font variant for a fragment's
/// `font-family` / `font-weight` / `font-style`, if any.
///
/// Shared by the `Text` and `FormControl` paint branches so both apply the
/// same web-font selection policy.
fn select_fragment_web_font<'a>(
    web_fonts: Option<&'a WebFontRegistry>,
    style: &FragmentStyle,
) -> Option<&'a Font> {
    let registry = web_fonts?;
    let family = style.font_family.as_deref()?;
    let weight = FontWeight::parse(style.font_weight.as_deref().unwrap_or("normal"));
    let font_style = FontStyle::parse(style.font_style.as_deref().unwrap_or("normal"));
    registry.select_best(family, weight, font_style)
}

/// Measures the painted advance width of `text`, mirroring the advance model of
/// [`paint_text_with_font_refs`] / [`paint_text_placeholder`] exactly:
/// non-zero advances, kerning between adjacent advancing characters drawn from
/// the same font, and letter-spacing between advancing characters.
///
/// Used to horizontally center form-control labels; when `fonts` is empty the
/// placeholder advance of `font_size * 0.6` is used to match the glyph fallback.
pub(crate) fn measure_form_control_text_width(
    text: &str,
    font_size: f32,
    fonts: &[&Font],
    letter_spacing: f32,
) -> f32 {
    if fonts.is_empty() {
        let advancing_count = text
            .chars()
            .filter(|ch| !is_zero_advance_character(*ch))
            .count();
        if advancing_count == 0 {
            return 0.0;
        }
        return advancing_count as f32 * (font_size * 0.6).max(1.0)
            + letter_spacing * (advancing_count - 1) as f32;
    }
    let mut width = 0.0;
    let mut advancing_count = 0usize;
    let mut previous: Option<(char, usize)> = None;
    for ch in text.chars() {
        if is_zero_advance_character(ch) {
            continue;
        }
        advancing_count += 1;
        let (font_index, _, advance) = rasterize_with_fallback_refs(fonts, ch, font_size);
        if let Some((prev, prev_index)) = previous
            && prev_index == font_index
        {
            width += fonts[font_index].glyph_kerning(prev, ch, font_size);
        }
        width += advance;
        previous = Some((ch, font_index));
    }
    if advancing_count > 1 {
        width += letter_spacing * (advancing_count - 1) as f32;
    }
    width
}

fn text_prefix_by_utf16_offset(value: &str, offset: usize) -> &str {
    if offset == 0 {
        return "";
    }
    let mut utf16_offset = 0;
    for (byte_offset, ch) in value.char_indices() {
        if utf16_offset >= offset {
            return &value[..byte_offset];
        }
        utf16_offset += ch.len_utf16();
    }
    value
}

#[cfg(test)]
mod bidi_tests {
    use super::{
        FragmentStyle, bidi_visual_text, fragment_text_for_paint, horizontal_glyph_origin,
        is_invisible_shaping_control, vertical_cursor_after, vertical_glyph_cell,
        vertical_paint_characters,
    };

    #[test]
    fn mixed_ltr_text_is_reordered_into_visual_runs() {
        let style = FragmentStyle {
            direction: Some("ltr".to_string()),
            unicode_bidi: Some("normal".to_string()),
            ..FragmentStyle::default()
        };
        assert_eq!(bidi_visual_text("abc אבג", &style), "abc גבא");
    }

    #[test]
    fn mixed_rtl_text_uses_the_rtl_paragraph_level() {
        let style = FragmentStyle {
            direction: Some("rtl".to_string()),
            unicode_bidi: Some("normal".to_string()),
            ..FragmentStyle::default()
        };
        assert_eq!(bidi_visual_text("abc אבג", &style), "גבא abc");
    }

    #[test]
    fn vertical_ltr_text_uses_the_same_visual_run_order() {
        let style = FragmentStyle {
            direction: Some("ltr".to_string()),
            unicode_bidi: Some("normal".to_string()),
            writing_mode: Some("vertical-rl".to_string()),
            ..FragmentStyle::default()
        };
        let (visual_text, vertical_mode) = fragment_text_for_paint("abc אבג", &style);
        assert_eq!(vertical_mode, Some((true, false)));
        assert_eq!(visual_text, "abc גבא");
    }

    #[test]
    fn vertical_rtl_text_uses_the_rtl_visual_run_order() {
        let style = FragmentStyle {
            direction: Some("rtl".to_string()),
            unicode_bidi: Some("normal".to_string()),
            writing_mode: Some("vertical-lr".to_string()),
            ..FragmentStyle::default()
        };
        let (visual_text, vertical_mode) = fragment_text_for_paint("abc אבג", &style);
        assert_eq!(vertical_mode, Some((false, true)));
        assert_eq!(visual_text, "גבא abc");
    }

    #[test]
    fn rtl_bidi_override_reverses_grapheme_clusters() {
        let style = FragmentStyle {
            direction: Some("rtl".to_string()),
            unicode_bidi: Some("bidi-override".to_string()),
            ..FragmentStyle::default()
        };
        assert_eq!(bidi_visual_text("abc אבג", &style), "גבא cba");
        assert_eq!(bidi_visual_text("a\u{301}b", &style), "ba\u{301}");
        assert_eq!(bidi_visual_text("A👩‍💻B", &style), "B👩‍💻A");
    }

    #[test]
    fn ltr_bidi_override_keeps_logical_cluster_order() {
        let style = FragmentStyle {
            direction: Some("ltr".to_string()),
            unicode_bidi: Some("bidi-override".to_string()),
            ..FragmentStyle::default()
        };
        assert_eq!(bidi_visual_text("abc אבג", &style), "abc אבג");
    }

    #[test]
    fn isolate_override_uses_the_same_directional_override() {
        let style = FragmentStyle {
            direction: Some("rtl".to_string()),
            unicode_bidi: Some("isolate-override".to_string()),
            ..FragmentStyle::default()
        };
        assert_eq!(bidi_visual_text("abc אבג", &style), "גבא cba");
    }

    #[test]
    fn vertical_override_routes_cluster_order_through_paint_state() {
        let style = FragmentStyle {
            direction: Some("rtl".to_string()),
            unicode_bidi: Some("isolate-override".to_string()),
            writing_mode: Some("vertical-rl".to_string()),
            ..FragmentStyle::default()
        };
        let (visual_text, vertical_mode) = fragment_text_for_paint("a\u{301}b", &style);
        assert_eq!(visual_text, "ba\u{301}");
        assert_eq!(vertical_mode, Some((true, true)));
        assert_eq!(
            vertical_paint_characters(visual_text.as_ref(), true),
            vec!['a', '\u{301}', 'b']
        );
    }

    #[test]
    fn embed_and_isolate_use_the_declared_base_direction() {
        for mode in ["embed", "isolate"] {
            let style = FragmentStyle {
                direction: Some("rtl".to_string()),
                unicode_bidi: Some(mode.to_string()),
                ..FragmentStyle::default()
            };
            assert_eq!(bidi_visual_text("abc אבג", &style), "גבא abc");
        }
    }

    #[test]
    fn plaintext_uses_the_first_strong_character_instead_of_css_direction() {
        let css_rtl = FragmentStyle {
            direction: Some("rtl".to_string()),
            unicode_bidi: Some("plaintext".to_string()),
            ..FragmentStyle::default()
        };
        assert_eq!(bidi_visual_text("abc אבג", &css_rtl), "abc גבא");

        let css_ltr = FragmentStyle {
            direction: Some("ltr".to_string()),
            unicode_bidi: Some("plaintext".to_string()),
            ..FragmentStyle::default()
        };
        assert_eq!(bidi_visual_text("אבג abc", &css_ltr), "abc גבא");
    }

    #[test]
    fn vertical_plaintext_uses_detected_level_and_keeps_vertical_cursor_mode() {
        let style = FragmentStyle {
            direction: Some("rtl".to_string()),
            unicode_bidi: Some("plaintext".to_string()),
            writing_mode: Some("vertical-lr".to_string()),
            ..FragmentStyle::default()
        };
        let (visual_text, vertical_mode) = fragment_text_for_paint("abc אבג", &style);
        assert_eq!(visual_text, "abc גבא");
        assert_eq!(vertical_mode, Some((false, true)));
    }

    #[test]
    fn vertical_rtl_reverses_clusters_without_splitting_marks_or_joiners() {
        assert_eq!(
            vertical_paint_characters("a\u{301}b", true),
            vec!['b', 'a', '\u{301}']
        );
        assert_eq!(
            vertical_paint_characters("A👩‍💻B", true),
            "B👩‍💻A".chars().collect::<Vec<_>>()
        );
    }

    #[test]
    fn zero_advance_glyphs_reuse_the_base_glyph_cell() {
        assert_eq!(horizontal_glyph_origin(18.0, 10.0, true, 1.5), 11.5);
        assert_eq!(horizontal_glyph_origin(18.0, 10.0, false, 1.5), 19.5);

        let previous = Some((20.0, 8.0, 0));
        assert_eq!(vertical_glyph_cell(28.0, previous, true, false, 0.0), (20.0, 8.0));
        assert_eq!(vertical_glyph_cell(20.0, previous, true, true, 0.0), (20.0, 8.0));
        assert_eq!(vertical_cursor_after(28.0, 20.0, 0.0, 0.0, true, false), 28.0);
        assert_eq!(vertical_cursor_after(20.0, 20.0, 0.0, 0.0, true, true), 20.0);
        assert!(is_invisible_shaping_control('\u{fe0f}'));
        assert!(is_invisible_shaping_control('\u{200c}'));
        assert!(is_invisible_shaping_control('\u{200d}'));
        assert!(!is_invisible_shaping_control('\u{301}'));
    }

    #[test]
    fn line_resolved_level_drives_fragment_paint_order() {
        let odd = FragmentStyle {
            direction: Some("ltr".to_string()),
            unicode_bidi: Some("normal".to_string()),
            resolved_bidi_level: Some(1),
            ..FragmentStyle::default()
        };
        assert_eq!(bidi_visual_text("אב", &odd), "בא");

        let even = FragmentStyle { resolved_bidi_level: Some(2), ..odd };
        assert_eq!(bidi_visual_text("אב", &even), "אב");
    }
}
