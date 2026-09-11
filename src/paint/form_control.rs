//! Multiline form-control text layout and painting.

use crate::css::{ComputedStyle, ComputedValue};
use crate::font::{
    Font, ShapingDirection, grapheme_spacing_cluster_starts, shape_text_with_fallback,
};
use crate::layout::{FragmentStyle, Rect, TextControlPaintState};
use unicode_bidi::{BidiClass, bidi_class};
use unicode_segmentation::UnicodeSegmentation;

use super::text::{
    measure_form_control_text_width, paint_shaped_horizontal_text, paint_text_placeholder,
    text_prefix_by_utf16_offset,
};
use super::{Canvas, Color};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TextareaVisualLine<'a> {
    pub(crate) text: &'a str,
    pub(crate) start_utf16: usize,
    pub(crate) end_utf16: usize,
    pub(crate) width: f32,
}

/// Splits a textarea value into hard and soft visual lines.
///
/// Hard breaks accept LF, CRLF, and CR so native state remains safe even when
/// it was populated outside the DOM setter. Soft wrapping measures every word
/// boundary once and only revisits graphemes in a token that cannot fit on an
/// otherwise empty line. The amount of measured text is therefore bounded by
/// two passes over the value.
pub(crate) fn textarea_visual_lines<'a>(
    value: &'a str,
    max_width: f32,
    soft_wrap: bool,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<TextareaVisualLine<'a>> {
    let mut lines = Vec::new();
    let mut hard_start_byte = 0usize;
    let mut hard_start_utf16 = 0usize;
    let mut byte_offset = 0usize;
    let mut utf16_offset = 0usize;

    while byte_offset < value.len() {
        let ch = value[byte_offset..].chars().next().unwrap();
        if matches!(ch, '\r' | '\n') {
            wrap_hard_line(
                &value[hard_start_byte..byte_offset],
                hard_start_utf16,
                max_width,
                soft_wrap,
                &mut measure,
                &mut lines,
            );
            let separator_bytes =
                if ch == '\r' && value.as_bytes().get(byte_offset + 1).copied() == Some(b'\n') {
                    2
                } else {
                    1
                };
            let separator_utf16 = separator_bytes;
            byte_offset += separator_bytes;
            utf16_offset += separator_utf16;
            hard_start_byte = byte_offset;
            hard_start_utf16 = utf16_offset;
            continue;
        }
        byte_offset += ch.len_utf8();
        utf16_offset += ch.len_utf16();
    }

    wrap_hard_line(
        &value[hard_start_byte..],
        hard_start_utf16,
        max_width,
        soft_wrap,
        &mut measure,
        &mut lines,
    );
    lines
}

fn wrap_hard_line<'a>(
    hard_line: &'a str,
    hard_start_utf16: usize,
    max_width: f32,
    soft_wrap: bool,
    measure: &mut impl FnMut(&str) -> f32,
    lines: &mut Vec<TextareaVisualLine<'a>>,
) {
    let first_line_index = lines.len();
    let mut line_start_byte = 0usize;
    let mut line_start_utf16 = hard_start_utf16;
    let mut line_end_byte = 0usize;
    let mut line_end_utf16 = hard_start_utf16;
    let mut line_width = 0.0f32;
    let can_wrap = soft_wrap && max_width.is_finite() && max_width > 0.0;

    for (piece_start, piece) in hard_line.split_word_bound_indices() {
        let piece_end = piece_start + piece.len();
        let piece_utf16 = piece.encode_utf16().count();
        let piece_width = measure(piece);

        if can_wrap && piece_width > max_width && !piece.chars().all(char::is_whitespace) {
            if line_end_byte > line_start_byte {
                push_visual_line(
                    hard_line,
                    line_start_byte,
                    line_end_byte,
                    line_start_utf16,
                    line_end_utf16,
                    line_width,
                    lines,
                );
                line_start_byte = piece_start;
                line_start_utf16 = line_end_utf16;
                line_width = 0.0;
            }

            for (relative_start, grapheme) in piece.grapheme_indices(true) {
                let grapheme_start = piece_start + relative_start;
                let grapheme_end = grapheme_start + grapheme.len();
                let grapheme_utf16 = grapheme.encode_utf16().count();
                let grapheme_width = measure(grapheme);
                if line_end_byte > line_start_byte && line_width + grapheme_width > max_width {
                    push_visual_line(
                        hard_line,
                        line_start_byte,
                        line_end_byte,
                        line_start_utf16,
                        line_end_utf16,
                        line_width,
                        lines,
                    );
                    line_start_byte = grapheme_start;
                    line_start_utf16 = line_end_utf16;
                    line_width = 0.0;
                }
                line_end_byte = grapheme_end;
                line_end_utf16 += grapheme_utf16;
                line_width += grapheme_width;
            }
            continue;
        }

        if can_wrap && line_end_byte > line_start_byte && line_width + piece_width > max_width {
            if piece.chars().all(char::is_whitespace) {
                // Preserved trailing whitespace belongs to the preceding line,
                // but may hang outside its available width under `pre-wrap`.
                line_end_byte = piece_end;
                line_end_utf16 += piece_utf16;
                line_width += piece_width;
                push_visual_line(
                    hard_line,
                    line_start_byte,
                    line_end_byte,
                    line_start_utf16,
                    line_end_utf16,
                    line_width,
                    lines,
                );
                line_start_byte = piece_end;
                line_start_utf16 = line_end_utf16;
                line_width = 0.0;
                continue;
            }
            push_visual_line(
                hard_line,
                line_start_byte,
                line_end_byte,
                line_start_utf16,
                line_end_utf16,
                line_width,
                lines,
            );
            line_start_byte = piece_start;
            line_start_utf16 = line_end_utf16;
            line_width = 0.0;
        }

        line_end_byte = piece_end;
        line_end_utf16 += piece_utf16;
        line_width += piece_width;
    }

    if line_end_byte > line_start_byte {
        push_visual_line(
            hard_line,
            line_start_byte,
            line_end_byte,
            line_start_utf16,
            line_end_utf16,
            line_width,
            lines,
        );
    } else if lines.len() == first_line_index {
        lines.push(TextareaVisualLine {
            text: "",
            start_utf16: hard_start_utf16,
            end_utf16: hard_start_utf16,
            width: 0.0,
        });
    }
}

fn push_visual_line<'a>(
    hard_line: &'a str,
    start_byte: usize,
    end_byte: usize,
    start_utf16: usize,
    end_utf16: usize,
    width: f32,
    lines: &mut Vec<TextareaVisualLine<'a>>,
) {
    lines.push(TextareaVisualLine {
        text: &hard_line[start_byte..end_byte],
        start_utf16,
        end_utf16,
        width,
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_textarea_value(
    canvas: &mut Canvas,
    content_rect: Rect,
    padding_rect: Rect,
    value: &str,
    editing: Option<TextControlPaintState>,
    style: &ComputedStyle,
    fragment_style: &FragmentStyle,
    font_size: f32,
    ascent: f32,
    fonts: &[&Font],
    color: Color,
    inherited_clip: Option<Rect>,
    letter_spacing: f32,
    soft_wrap: bool,
) {
    // Wrapping uses the content width, but the scrollable text viewport extends
    // through padding. Clipping to the content box cuts the last visible row.
    let Some(content_clip) = inherited_clip
        .map(|clip| super::intersect(clip, padding_rect))
        .unwrap_or(Some(padding_rect))
    else {
        return;
    };
    let mut measure = |text: &str| {
        measure_textarea_text_width(text, font_size, fonts, fragment_style, letter_spacing)
    };
    let lines = textarea_visual_lines(value, content_rect.width, soft_wrap, &mut measure);
    let line_height = textarea_line_height(style, font_size);
    let focused_editing = editing.filter(|state| state.focused);
    let caret_line = focused_editing
        .filter(|state| state.selection_start == state.selection_end)
        .and_then(|state| {
            let offset = state.selection_start.min(value.encode_utf16().count());
            lines
                .iter()
                .rposition(|line| line.start_utf16 <= offset && offset <= line.end_utf16)
                .map(|index| (index, offset))
        });

    for (index, line) in lines.iter().enumerate() {
        let line_top = content_rect.y + index as f32 * line_height;
        if line_top >= content_clip.y + content_clip.height {
            break;
        }
        let x_offset = text_align_offset(style, content_rect.width, line.width);
        let text_rect = Rect {
            x: content_rect.x + x_offset,
            y: line_top + (line_height - font_size) / 2.0,
            width: (content_rect.width - x_offset).max(0.0),
            height: font_size,
        };

        if let Some(state) = focused_editing
            && state.selection_start != state.selection_end
        {
            paint_textarea_selection(
                canvas,
                line,
                state,
                text_rect,
                font_size,
                fonts,
                fragment_style,
                letter_spacing,
                content_clip,
            );
        }

        if !line.text.is_empty() {
            if fonts.is_empty()
                || paint_shaped_horizontal_text(
                    canvas,
                    text_rect,
                    line.text,
                    font_size,
                    ascent,
                    fonts,
                    fragment_style,
                    color,
                    Some(content_clip),
                    letter_spacing,
                )
                .is_none()
            {
                paint_text_placeholder(
                    canvas,
                    text_rect,
                    line.text,
                    font_size,
                    color,
                    Some(content_clip),
                    letter_spacing,
                );
            }
        }

        if let Some((caret_index, caret_offset)) = caret_line
            && caret_index == index
        {
            let offset = caret_offset.saturating_sub(line.start_utf16);
            let prefix = text_prefix_by_utf16_offset(line.text, offset);
            let caret_x = text_rect.x
                + measure_textarea_text_width(
                    prefix,
                    font_size,
                    fonts,
                    fragment_style,
                    letter_spacing,
                );
            canvas.fill_rect_clipped(
                Rect {
                    x: caret_x,
                    y: text_rect.y,
                    width: 1.0,
                    height: font_size,
                },
                color,
                Some(content_clip),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_textarea_selection(
    canvas: &mut Canvas,
    line: &TextareaVisualLine<'_>,
    state: TextControlPaintState,
    text_rect: Rect,
    font_size: f32,
    fonts: &[&Font],
    fragment_style: &FragmentStyle,
    letter_spacing: f32,
    clip: Rect,
) {
    let selection_start = state.selection_start.min(state.selection_end);
    let selection_end = state.selection_start.max(state.selection_end);
    let start = selection_start.max(line.start_utf16).min(line.end_utf16);
    let end = selection_end.max(line.start_utf16).min(line.end_utf16);
    if start >= end {
        return;
    }
    let start_prefix = text_prefix_by_utf16_offset(line.text, start - line.start_utf16);
    let end_prefix = text_prefix_by_utf16_offset(line.text, end - line.start_utf16);
    let start_x = text_rect.x
        + measure_textarea_text_width(
            start_prefix,
            font_size,
            fonts,
            fragment_style,
            letter_spacing,
        );
    let end_x = text_rect.x
        + measure_textarea_text_width(end_prefix, font_size, fonts, fragment_style, letter_spacing);
    canvas.fill_rect_clipped(
        Rect {
            x: start_x,
            y: text_rect.y,
            width: (end_x - start_x).max(1.0),
            height: text_rect.height,
        },
        Color::rgba(51, 153, 255, 120),
        Some(clip),
    );
}

fn measure_textarea_text_width(
    text: &str,
    font_size: f32,
    fonts: &[&Font],
    style: &FragmentStyle,
    letter_spacing: f32,
) -> f32 {
    if fonts.is_empty() {
        return measure_form_control_text_width(text, font_size, fonts, letter_spacing);
    }
    let rtl = style
        .resolved_bidi_level
        .is_some_and(|level| level % 2 == 1)
        || style.resolved_bidi_level.is_none()
            && (style.direction.as_deref() == Some("rtl")
                || text.chars().any(|ch| {
                    matches!(bidi_class(ch), BidiClass::R | BidiClass::AL | BidiClass::AN)
                }));
    let direction = if rtl {
        ShapingDirection::RightToLeft
    } else {
        ShapingDirection::LeftToRight
    };
    if let Ok(runs) = shape_text_with_fallback(fonts, text, font_size, direction) {
        let advance = runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .map(|glyph| glyph.x_advance.abs())
            .sum::<f32>();
        let spacing = grapheme_spacing_cluster_starts(text)
            .len()
            .saturating_sub(1) as f32
            * letter_spacing;
        return advance + spacing;
    }
    measure_form_control_text_width(text, font_size, fonts, letter_spacing)
}

fn textarea_line_height(style: &ComputedStyle, font_size: f32) -> f32 {
    match style.get("line-height") {
        Some(ComputedValue::Px(value)) => *value,
        Some(ComputedValue::Number(value)) => *value * font_size,
        Some(ComputedValue::Percentage(value)) => font_size * value / 100.0,
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("normal") => {
            font_size * 1.2
        }
        _ => font_size * 1.2,
    }
}

fn text_align_offset(style: &ComputedStyle, available: f32, width: f32) -> f32 {
    let direction_rtl = matches!(
        style.get("direction"),
        Some(ComputedValue::Keyword(direction)) if direction.eq_ignore_ascii_case("rtl")
    );
    let aligned_to_end = matches!(
        style.get("text-align"),
        Some(ComputedValue::Keyword(align))
            if align.eq_ignore_ascii_case("right")
                || align.eq_ignore_ascii_case("end") && !direction_rtl
                || align.eq_ignore_ascii_case("start") && direction_rtl
    );
    if matches!(
        style.get("text-align"),
        Some(ComputedValue::Keyword(align)) if align.eq_ignore_ascii_case("center")
    ) {
        ((available - width) / 2.0).max(0.0)
    } else if aligned_to_end {
        (available - width).max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::{paint_textarea_value, textarea_visual_lines};
    use crate::css::{Origin, StyleResolver, parse_stylesheet};
    use crate::dom::NodeHandle;
    use crate::layout::{FragmentStyle, Rect, TextControlPaintState};
    use crate::paint::{Canvas, Color};

    #[test]
    fn visual_lines_preserve_hard_break_utf16_offsets_and_trailing_blank_line() {
        let lines = textarea_visual_lines("😀\r\nB\rC\n", 100.0, true, |text| {
            text.chars().count() as f32
        });
        assert_eq!(
            lines
                .iter()
                .map(|line| (line.text, line.start_utf16, line.end_utf16))
                .collect::<Vec<_>>(),
            [("😀", 0, 2), ("B", 4, 5), ("C", 6, 7), ("", 8, 8)]
        );
    }

    #[test]
    fn visual_lines_wrap_words_then_split_only_oversized_grapheme_runs() {
        let lines = textarea_visual_lines("one two abcdef", 4.0, true, |text| {
            text.chars().count() as f32
        });
        assert_eq!(
            lines.iter().map(|line| line.text).collect::<Vec<_>>(),
            ["one ", "two ", "abcd", "ef"]
        );
        assert_eq!(
            lines
                .iter()
                .map(|line| (line.start_utf16, line.end_utf16))
                .collect::<Vec<_>>(),
            [(0, 4), (4, 8), (8, 12), (12, 14)]
        );
    }

    #[test]
    fn visual_line_measurement_work_is_linear_in_value_length() {
        let value = "word ".repeat(2_000);
        let mut measured_bytes = 0usize;
        let lines = textarea_visual_lines(&value, 40.0, true, |text| {
            measured_bytes += text.len();
            text.len() as f32
        });
        assert!(lines.len() > 100);
        assert!(
            measured_bytes <= value.len() * 2,
            "measured {measured_bytes} bytes for {} input bytes",
            value.len()
        );
    }

    fn textarea_style(line_height: f32) -> crate::css::ComputedStyle {
        let document = NodeHandle::document();
        let textarea = NodeHandle::element("textarea");
        document.append_child(textarea.clone());
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(&format!("textarea{{line-height:{line_height}px}}")).unwrap(),
        );
        resolver.computed_style(&textarea)
    }

    #[test]
    fn painting_places_selection_and_caret_on_their_visual_lines() {
        let style = textarea_style(20.0);
        let fragment_style = FragmentStyle::default();
        let content = Rect {
            x: 0.0,
            y: 0.0,
            width: 60.0,
            height: 40.0,
        };
        let mut selection = Canvas::new(60, 40);
        paint_textarea_value(
            &mut selection,
            content,
            content,
            "A\nBC",
            Some(TextControlPaintState {
                selection_start: 2,
                selection_end: 3,
                focused: true,
            }),
            &style,
            &fragment_style,
            10.0,
            8.0,
            &[],
            Color::rgb(0, 0, 0),
            None,
            0.0,
            true,
        );
        assert!(
            (20..40).any(|y| (0..60)
                .any(|x| { selection.pixel(x, y).is_some_and(|pixel| pixel.b > pixel.r) })),
            "the selected B must be highlighted on the second line"
        );
        assert!(
            !(0..20).any(|y| (0..60)
                .any(|x| { selection.pixel(x, y).is_some_and(|pixel| pixel.b > pixel.r) })),
            "selection must not be painted on the first line"
        );

        let mut caret = Canvas::new(60, 40);
        paint_textarea_value(
            &mut caret,
            content,
            content,
            "A\n",
            Some(TextControlPaintState {
                selection_start: 2,
                selection_end: 2,
                focused: true,
            }),
            &style,
            &fragment_style,
            10.0,
            8.0,
            &[],
            Color::rgb(0, 0, 0),
            None,
            0.0,
            true,
        );
        assert_eq!(caret.pixel(0, 26), Some(Color::rgb(0, 0, 0)));
        assert_eq!(caret.pixel(0, 20), Some(Color::rgba(0, 0, 0, 0)));
    }

    #[test]
    fn textarea_clip_keeps_text_in_padding_and_preserves_border() {
        use crate::font::Font;
        use crate::html::TreeBuilder;
        use crate::layout::{layout_tree, with_layout_fonts};
        use std::sync::Arc;

        // Firefox paints the second row down to the padding edge at y=74,
        // beyond the content edge at y=68; the border occupies y=74..76.
        let html = include_str!("../../tests/fixtures/anonymized-textarea-lines/clip.html");
        let css = html
            .split_once("<style>")
            .unwrap()
            .1
            .split_once("</style>")
            .unwrap()
            .0;
        let document = TreeBuilder::parse(html).document();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
        let font = Arc::new(
            Font::load_from_bytes(
                include_bytes!("../../tests/fixtures/acid2/LiberationSans-Regular.ttf").to_vec(),
            )
            .unwrap(),
        );
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 280.0,
            height: 120.0,
        };
        let layout = with_layout_fonts(vec![font.clone()], None, || {
            layout_tree(&document, &mut resolver, viewport).unwrap()
        });
        let canvas =
            crate::paint::paint_layout_with_fonts(&layout, &mut resolver, viewport, vec![font]);
        assert!(
            (68..74)
                .any(|y| (32..200).any(|x| canvas.pixel(x, y) != Some(Color::rgb(255, 255, 255)))),
            "the visible second row must extend into the bottom padding"
        );
        assert!(
            (74..76)
                .all(|y| (24..244).all(|x| canvas.pixel(x, y) == Some(Color::rgb(100, 116, 139))))
        );
        assert!(
            (76..120)
                .all(|y| (24..244).all(|x| canvas.pixel(x, y) == Some(Color::rgb(255, 255, 255))))
        );
    }

    #[test]
    fn painting_respects_an_inherited_clip_inside_the_textarea() {
        let style = textarea_style(20.0);
        let mut canvas = Canvas::new(60, 60);
        paint_textarea_value(
            &mut canvas,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 60.0,
                height: 28.0,
            },
            Rect {
                x: 0.0,
                y: 0.0,
                width: 60.0,
                height: 34.0,
            },
            "first\nsecond\nthird",
            None,
            &style,
            &FragmentStyle::default(),
            10.0,
            8.0,
            &[],
            Color::rgb(0, 0, 0),
            Some(Rect {
                x: 0.0,
                y: 0.0,
                width: 60.0,
                height: 28.0,
            }),
            0.0,
            true,
        );
        assert!(
            (0..28)
                .any(|y| (0..60).any(|x| { canvas.pixel(x, y) != Some(Color::rgba(0, 0, 0, 0)) })),
            "visible rows must paint inside the inherited clip"
        );
        assert!(
            (28..60)
                .all(|y| (0..60).all(|x| { canvas.pixel(x, y) == Some(Color::rgba(0, 0, 0, 0)) })),
            "rows outside the inherited clip must remain clipped"
        );
    }
}
