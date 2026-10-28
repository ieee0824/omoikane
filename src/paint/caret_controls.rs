//! Insertion geometry shared with the native form-control text painter.

use super::*;
use crate::layout::InlineFragmentContent;

pub(super) fn insertion_points(
    fragment: &InlineFragment,
    style: &ComputedStyle,
    web_fonts: Option<&WebFontRegistry>,
) -> Vec<(usize, Rect)> {
    if !fragment.node.has_tag_name("textarea")
        && (!fragment.node.has_tag_name("input")
            || fragment.node.get_attribute("type").is_some_and(|kind| {
                matches!(
                    kind.to_ascii_lowercase().as_str(),
                    "hidden"
                        | "checkbox"
                        | "radio"
                        | "button"
                        | "submit"
                        | "reset"
                        | "image"
                        | "file"
                        | "range"
                        | "color"
                        | "date"
                        | "month"
                        | "week"
                        | "time"
                        | "datetime-local"
                )
            }))
    {
        return Vec::new();
    }
    let InlineFragmentContent::FormControl(_, value, _) = &fragment.content else {
        return Vec::new();
    };
    let content = text::inline_fragment_content_rect(
        fragment.rect,
        style,
        EdgeSizesForPaint::from_style(style),
    );
    with_fragment_fonts(fragment, web_fonts, |fonts| {
        let size = fragment.metrics.font_size;
        let spacing = fragment.metrics.letter_spacing;
        if fragment.node.has_tag_name("textarea") {
            let soft_wrap = !fragment
                .node
                .get_attribute("wrap")
                .is_some_and(|wrap| wrap.eq_ignore_ascii_case("off"));
            let measure = |value: &str| {
                form_control::measure_textarea_text_width(
                    value,
                    size,
                    fonts,
                    &fragment.style,
                    spacing,
                )
            };
            let lines =
                form_control::textarea_visual_lines(value, content.width, soft_wrap, measure);
            let height = form_control::textarea_line_height(style, size);
            return lines
                .iter()
                .enumerate()
                .flat_map(|(index, line)| {
                    let origin = Rect {
                        x: content.x
                            + form_control::text_align_offset(style, content.width, line.width),
                        y: content.y + index as f32 * height + (height - size) / 2.0,
                        width: 0.0,
                        height: size,
                    };
                    line_points(line.text, line.start_utf16, origin, fragment, fonts)
                })
                .collect();
        }
        let measure = |value: &str| {
            text::measure_form_control_text_width_with_candidates(value, size, fonts, spacing)
        };
        let offset = if matches!(style.get("text-align"), Some(ComputedValue::Keyword(value)) if value == "center")
        {
            ((content.width - measure(value)) / 2.0).max(0.0)
        } else {
            0.0
        };
        line_points(
            value,
            0,
            Rect {
                x: content.x + offset,
                y: content.y + ((content.height - size) / 2.0).max(0.0),
                width: 0.0,
                height: size,
            },
            fragment,
            fonts,
        )
    })
}

fn line_points(
    value: &str,
    start: usize,
    origin: Rect,
    fragment: &InlineFragment,
    fonts: &[FontFallbackCandidate<'_>],
) -> Vec<(usize, Rect)> {
    let positions = shaped_carets(
        value,
        fragment.metrics.font_size,
        fragment.metrics.letter_spacing,
        &fragment.style,
        fonts,
    )
    .unwrap_or_else(|| scalar_carets(value, fragment, fonts, false));
    let mut offsets = std::collections::HashMap::new();
    let mut units = 0;
    for (byte, ch) in value.char_indices() {
        offsets.insert(byte, units);
        units += ch.len_utf16();
    }
    offsets.insert(value.len(), units);
    positions
        .into_iter()
        .map(|(byte, advance)| {
            (
                start + offsets[&byte],
                Rect {
                    x: origin.x + advance,
                    ..origin
                },
            )
        })
        .collect()
}
