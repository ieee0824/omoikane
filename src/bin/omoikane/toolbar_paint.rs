//! Paints the address toolbar into the native `0RGB` surface.
//!
//! Painting reads only its arguments and writes only inside the toolbar
//! area. Text is rasterized at physical pixel size so it stays sharp on
//! scaled displays; without a font the field is drawn without text.

use omoikane::font::Font;

use super::chrome_layout::PhysicalRect;
use super::device_scale::DeviceScale;
use super::url_bar::UrlBar;

const BACKGROUND: u32 = 0xf1f3f4;
const DIVIDER: u32 = 0xdadce0;
const FIELD_BACKGROUND: u32 = 0xffffff;
const FIELD_BORDER: u32 = 0xc4c7c5;
const FIELD_FOCUS_BORDER: u32 = 0x1a73e8;
const TEXT: u32 = 0x202124;
const SELECTION: u32 = 0xc2dbff;

/// Space around the URL field inside the toolbar, in logical pixels.
const FIELD_MARGIN: f64 = 6.0;
/// Horizontal space between the field border and its text, in logical pixels.
const TEXT_PADDING: f64 = 8.0;
/// URL text size in logical pixels.
const FONT_SIZE: f64 = 14.0;

/// Returns the URL field's border box inside the toolbar `area`. Painting and
/// click hit-testing share it so the clickable area matches what is drawn.
pub(super) fn url_field(area: PhysicalRect, scale: DeviceScale) -> PhysicalRect {
    let margin = (FIELD_MARGIN * scale.factor()).round() as u32;
    inset(area, margin, margin)
}

/// Paints the toolbar and the URL field for `bar` into `area` of `target`.
///
/// `target` is a surface of rows `stride` pixels wide. Pixels outside `area`
/// or past the end of `target` are never written.
pub(super) fn paint_toolbar(
    target: &mut [u32],
    stride: u32,
    area: PhysicalRect,
    scale: DeviceScale,
    bar: &UrlBar,
    font: Option<&Font>,
) {
    let mut canvas = Canvas {
        pixels: target,
        stride,
        clip: area,
    };
    let px = |logical: f64| (logical * scale.factor()).round() as u32;
    let hairline = px(1.0).max(1);
    canvas.fill(area, BACKGROUND);
    canvas.fill(
        PhysicalRect {
            y: (area.y + area.height).saturating_sub(hairline),
            height: hairline,
            ..area
        },
        DIVIDER,
    );

    let field = url_field(area, scale);
    if field.width == 0 || field.height == 0 {
        return;
    }
    let border = if bar.is_editing() {
        FIELD_FOCUS_BORDER
    } else {
        FIELD_BORDER
    };
    canvas.fill(field, border);
    let inner = inset(field, hairline, hairline);
    canvas.fill(inner, FIELD_BACKGROUND);

    let text_area = inset(inner, px(TEXT_PADDING).saturating_sub(hairline), 0);
    canvas.clip = intersect(canvas.clip, text_area);
    let line = font.map(|font| TextLine::layout(font, bar.display_text(), px_f32(scale)));
    let (top, bottom) = line
        .as_ref()
        .map_or((text_area.y, text_area.y + text_area.height), |line| {
            line.vertical_extent(text_area)
        });
    let caret_x = bar
        .caret()
        .map(|caret| line.as_ref().map_or(0.0, |line| line.x_at(caret)));
    // Scroll just enough to keep the caret inside the field.
    let scroll = caret_x.map_or(0.0, |x| {
        (x + hairline as f32 - text_area.width as f32).max(0.0)
    });
    let to_screen = |x: f32| text_area.x as f32 + x - scroll;

    if let (Some(selection), Some(line)) = (bar.selection(), &line) {
        let start = to_screen(line.x_at(selection.start)).round().max(0.0) as u32;
        let end = to_screen(line.x_at(selection.end)).round().max(0.0) as u32;
        canvas.fill(
            PhysicalRect {
                x: start,
                y: top,
                width: end.saturating_sub(start),
                height: bottom - top,
            },
            SELECTION,
        );
    }
    if let (Some(line), Some(font)) = (&line, font) {
        line.paint(&mut canvas, font, to_screen);
    }
    if let Some(x) = caret_x.filter(|_| bar.selection().is_none()) {
        canvas.fill(
            PhysicalRect {
                x: to_screen(x).round().max(0.0) as u32,
                y: top,
                width: hairline,
                height: bottom - top,
            },
            TEXT,
        );
    }
}

fn px_f32(scale: DeviceScale) -> f32 {
    (FONT_SIZE * scale.factor()) as f32
}

/// One line of URL text laid out from x = 0 at physical font size.
struct TextLine {
    size_px: f32,
    ascent: f32,
    descent: f32,
    /// Pen position before each character, plus the end position.
    stops: Vec<(usize, f32)>,
    text: String,
}

impl TextLine {
    fn layout(font: &Font, text: &str, size_px: f32) -> Self {
        let metrics = font.metrics().at_size(size_px);
        let mut pen = 0.0;
        let mut stops = Vec::with_capacity(text.len() + 1);
        for (offset, ch) in text.char_indices() {
            stops.push((offset, pen));
            pen += font.glyph_advance(ch, size_px);
        }
        stops.push((text.len(), pen));
        Self {
            size_px,
            ascent: metrics.ascender,
            descent: metrics.descender,
            stops,
            text: text.to_string(),
        }
    }

    /// Returns the pen x position at a byte offset on a `char` boundary.
    fn x_at(&self, offset: usize) -> f32 {
        self.stops
            .iter()
            .find(|(stop, _)| *stop >= offset)
            .map_or(0.0, |(_, x)| *x)
    }

    /// Returns the top and bottom rows of the line box centred in `area`.
    fn vertical_extent(&self, area: PhysicalRect) -> (u32, u32) {
        let height = (self.ascent - self.descent).ceil().max(0.0) as u32;
        let height = height.min(area.height);
        let top = area.y + (area.height - height) / 2;
        (top, top + height)
    }

    fn paint(&self, canvas: &mut Canvas, font: &Font, to_screen: impl Fn(f32) -> f32) {
        let (top, _) = self.vertical_extent(canvas.clip);
        let baseline = top as f32 + self.ascent;
        for ((_, x), ch) in self.stops.iter().zip(self.text.chars()) {
            let Ok(glyph) = font.rasterize(ch, self.size_px) else {
                continue;
            };
            let left = (to_screen(*x) + glyph.offset_x).round() as i64;
            let glyph_top = (baseline + glyph.offset_y).round() as i64;
            for row in 0..glyph.height {
                for column in 0..glyph.width {
                    let alpha = glyph.bitmap[(row * glyph.width + column) as usize];
                    canvas.blend(
                        left + i64::from(column),
                        glyph_top + i64::from(row),
                        TEXT,
                        alpha,
                    );
                }
            }
        }
    }
}

/// Clipped drawing into surface rows.
struct Canvas<'a> {
    pixels: &'a mut [u32],
    stride: u32,
    clip: PhysicalRect,
}

impl Canvas<'_> {
    fn fill(&mut self, rect: PhysicalRect, color: u32) {
        let rect = intersect(rect, self.clip);
        for y in rect.y..rect.y + rect.height {
            let start = y as usize * self.stride as usize + rect.x as usize;
            let Some(row) = self.pixels.get_mut(start..start + rect.width as usize) else {
                return;
            };
            row.fill(color);
        }
    }

    fn blend(&mut self, x: i64, y: i64, color: u32, alpha: u8) {
        let clip = self.clip;
        let inside = x >= i64::from(clip.x)
            && y >= i64::from(clip.y)
            && x < i64::from(clip.x) + i64::from(clip.width)
            && y < i64::from(clip.y) + i64::from(clip.height);
        if !inside || alpha == 0 {
            return;
        }
        let index = y as usize * self.stride as usize + x as usize;
        if let Some(pixel) = self.pixels.get_mut(index) {
            *pixel = mix(*pixel, color, alpha);
        }
    }
}

fn mix(under: u32, over: u32, alpha: u8) -> u32 {
    let alpha = u32::from(alpha);
    let channel = |shift: u32| {
        let under = (under >> shift) & 0xff;
        let over = (over >> shift) & 0xff;
        ((over * alpha + under * (255 - alpha) + 127) / 255) << shift
    };
    channel(16) | channel(8) | channel(0)
}

fn inset(rect: PhysicalRect, horizontal: u32, vertical: u32) -> PhysicalRect {
    PhysicalRect {
        x: rect.x + horizontal.min(rect.width),
        y: rect.y + vertical.min(rect.height),
        width: rect.width.saturating_sub(horizontal * 2),
        height: rect.height.saturating_sub(vertical * 2),
    }
}

fn intersect(a: PhysicalRect, b: PhysicalRect) -> PhysicalRect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    PhysicalRect {
        x,
        y,
        width: right.saturating_sub(x),
        height: bottom.saturating_sub(y),
    }
}

#[cfg(test)]
mod tests {
    use super::super::chrome_layout::ChromeLayout;
    use super::*;
    use crate::url_bar::UrlBarKey;

    const SENTINEL: u32 = 0x00ff00ff;

    fn test_font() -> Font {
        Font::load_from_bytes(
            include_bytes!("../../../tests/fixtures/acid2/LiberationSans-Regular.ttf").to_vec(),
        )
        .unwrap()
    }

    /// Paints a toolbar into a 2x-tall window buffer prefilled with SENTINEL.
    fn paint(
        width: u32,
        factor: f64,
        bar: &UrlBar,
        font: Option<&Font>,
    ) -> (Vec<u32>, ChromeLayout) {
        let scale = DeviceScale::new(factor);
        let height = (100.0 * factor) as u32;
        let layout = ChromeLayout::new(width, height, scale, true);
        let mut pixels = vec![SENTINEL; (width * height) as usize];
        paint_toolbar(&mut pixels, width, layout.toolbar(), scale, bar, font);
        (pixels, layout)
    }

    fn count_in(
        pixels: &[u32],
        width: u32,
        rect: PhysicalRect,
        test: impl Fn(u32) -> bool,
    ) -> usize {
        (rect.y..rect.y + rect.height)
            .flat_map(|y| (rect.x..rect.x + rect.width).map(move |x| (x, y)))
            .filter(|&(x, y)| test(pixels[(y * width + x) as usize]))
            .count()
    }

    fn is_dark(pixel: u32) -> bool {
        [16, 8, 0]
            .iter()
            .all(|shift| (pixel >> shift) & 0xff < 0x80)
    }

    #[test]
    fn paints_only_inside_the_toolbar_area() {
        for factor in [1.0, 2.0] {
            let bar = UrlBar::new("http://127.0.0.1:8000/before");
            let font = test_font();
            let (pixels, layout) = paint(400, factor, &bar, Some(&font));
            let toolbar = layout.toolbar();
            assert!(
                count_in(&pixels, 400, toolbar, |p| p == SENTINEL) == 0,
                "{factor}"
            );
            assert!(
                count_in(&pixels, 400, layout.page(), |p| p != SENTINEL) == 0,
                "{factor}"
            );
            // Background at the top-left corner and divider on the last row.
            assert_eq!(pixels[0], BACKGROUND);
            let last_row = (toolbar.height - 1) * 400;
            assert_eq!(pixels[last_row as usize], DIVIDER);
        }
    }

    #[test]
    fn draws_readable_text_inside_the_field() {
        let font = test_font();
        let bar = UrlBar::new("http://127.0.0.1:8000/before");
        let (at_1x, layout_1x) = paint(400, 1.0, &bar, Some(&font));
        let (at_2x, layout_2x) = paint(800, 2.0, &bar, Some(&font));
        let dark_1x = count_in(&at_1x, 400, layout_1x.toolbar(), is_dark);
        let dark_2x = count_in(&at_2x, 800, layout_2x.toolbar(), is_dark);
        assert!(dark_1x > 100, "{dark_1x}");
        // Text rasterized at physical size covers about four times the pixels.
        assert!(dark_2x > dark_1x * 3, "{dark_1x} {dark_2x}");

        let (empty, layout) = paint(400, 1.0, &UrlBar::new(""), Some(&font));
        assert_eq!(count_in(&empty, 400, layout.toolbar(), is_dark), 0);
    }

    #[test]
    fn focus_changes_the_border_and_shows_selection_or_caret() {
        let font = test_font();
        let mut bar = UrlBar::new("https://example.test/");
        let (idle, layout) = paint(400, 1.0, &bar, Some(&font));
        let border_at = (layout.toolbar().y + 6) * 400 + 200;
        assert_eq!(idle[border_at as usize], FIELD_BORDER);

        bar.focus();
        let (selected, _) = paint(400, 1.0, &bar, Some(&font));
        assert_eq!(selected[border_at as usize], FIELD_FOCUS_BORDER);
        assert!(count_in(&selected, 400, layout.toolbar(), |p| p == SELECTION) > 100);

        bar.key(UrlBarKey::End, false);
        let (caret, _) = paint(400, 1.0, &bar, Some(&font));
        assert_eq!(
            count_in(&caret, 400, layout.toolbar(), |p| p == SELECTION),
            0
        );
        assert!(count_in(&caret, 400, layout.toolbar(), |p| p == TEXT) > 0);
    }

    #[test]
    fn long_urls_scroll_to_keep_the_caret_visible() {
        let font = test_font();
        let mut bar = UrlBar::new(format!("https://example.test/{}", "segment/".repeat(40)));
        bar.focus();
        bar.key(UrlBarKey::End, false);
        let (pixels, layout) = paint(300, 1.0, &bar, Some(&font));
        // The caret is a solid TEXT column near the right edge of the field.
        let toolbar = layout.toolbar();
        let caret_columns: Vec<u32> = (0..300)
            .filter(|&x| {
                count_in(
                    &pixels,
                    300,
                    PhysicalRect {
                        x,
                        width: 1,
                        ..toolbar
                    },
                    |p| p == TEXT,
                ) >= 12
            })
            .collect();
        let rightmost = *caret_columns.last().expect("caret column");
        assert!((270..300 - 6).contains(&rightmost), "{caret_columns:?}");
    }

    #[test]
    fn missing_font_and_tiny_areas_do_not_panic() {
        let mut bar = UrlBar::new("https://example.test/");
        bar.focus();
        let (pixels, layout) = paint(200, 1.0, &bar, None);
        let border_at = (layout.toolbar().y + 6) * 200 + 100;
        assert_eq!(pixels[border_at as usize], FIELD_FOCUS_BORDER);

        let font = test_font();
        for (width, height) in [(0, 0), (1, 1), (10, 3), (13, 40)] {
            let area = PhysicalRect {
                x: 0,
                y: 0,
                width,
                height,
            };
            let mut pixels = vec![SENTINEL; (width * height) as usize];
            paint_toolbar(
                &mut pixels,
                width,
                area,
                DeviceScale::new(2.0),
                &bar,
                Some(&font),
            );
        }
        // A buffer shorter than the area stops at its end.
        let mut short = vec![SENTINEL; 10];
        let area = PhysicalRect {
            x: 0,
            y: 0,
            width: 10,
            height: 40,
        };
        paint_toolbar(
            &mut short,
            10,
            area,
            DeviceScale::new(1.0),
            &bar,
            Some(&font),
        );
    }

    #[test]
    fn alpha_blending_rounds_between_the_two_colors() {
        assert_eq!(mix(0xffffff, 0x000000, 0), 0xffffff);
        assert_eq!(mix(0xffffff, 0x000000, 255), 0x000000);
        assert_eq!(mix(0xff0000, 0x0000ff, 128), 0x7f0080);
    }
}
