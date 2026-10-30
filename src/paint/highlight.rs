//! Paints CSS Custom Highlight API pseudo-elements (`::highlight()`) over text
//! fragments (Issue #1297).
//!
//! Script publishes each Text node's highlighted UTF-16 intervals to the DOM;
//! this module turns them into per-fragment rectangles with the background
//! and text colors of the matching `::highlight(<name>)` style.

use super::*;
use crate::css::PseudoElement;
use crate::font::WebFontRegistry;
use crate::layout::InlineFragment;

/// One highlighted interval of a text fragment, in layout coordinates.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct HighlightOverlay {
    pub(super) rect: Rect,
    pub(super) background: Option<Color>,
    /// Replacement text color; `None` keeps the fragment's own color.
    pub(super) color: Option<Color>,
}

/// Returns the fragment's highlight overlays from bottom to top.
pub(super) fn fragment_highlight_overlays(
    fragment: &InlineFragment,
    resolver: &mut StyleResolver,
    web_fonts: Option<&WebFontRegistry>,
) -> Vec<HighlightOverlay> {
    let mut overlays = Vec::new();
    for span in &fragment.text_source {
        let highlights = span.node.text_highlights();
        if highlights.is_empty() {
            continue;
        }
        let Some(origin) = span.node.parent_node() else {
            continue;
        };
        let origin_color = text::text_color(&resolver.computed_style(&origin));
        for highlight in highlights {
            let start = highlight.start.max(span.dom_range.start);
            let end = highlight.end.min(span.dom_range.end);
            if start >= end {
                continue;
            }
            let Some(style) = resolver
                .computed_pseudo_style(&origin, PseudoElement::Highlight(highlight.name_key))
            else {
                continue;
            };
            let background = background_color(&style).filter(|color| color.a > 0);
            // Highlight pseudos inherit the originating color, so only a
            // different value replaces the text color.
            let color = text::text_color(&style).filter(|color| Some(*color) != origin_color);
            if background.is_none() && color.is_none() {
                continue;
            }
            let Some(rect) =
                caret::fragment_text_range_rect(fragment, &span.node, start, end, web_fonts)
            else {
                continue;
            };
            overlays.push((
                highlight.order,
                HighlightOverlay {
                    rect,
                    background,
                    color,
                },
            ));
        }
    }
    overlays.sort_by_key(|(order, _)| *order);
    overlays.into_iter().map(|(_, overlay)| overlay).collect()
}

/// Paints the highlight backgrounds beneath a fragment's glyphs.
pub(super) fn paint_highlight_backgrounds(
    canvas: &mut Canvas,
    overlays: &[HighlightOverlay],
    offset: PaintOffset,
    clip: Option<Rect>,
) {
    for overlay in overlays {
        if let Some(background) = overlay.background {
            canvas.fill_rect_clipped(offset.rect(overlay.rect), background, clip);
        }
    }
}
