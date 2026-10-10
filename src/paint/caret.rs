//! Text insertion positions from painted layout geometry and shaped glyphs.

use super::*;
use crate::dom::{Node, NodeHandle};
use crate::font::{
    FontFallbackCandidate, ShapingDirection, WebFontRegistry, grapheme_spacing_cluster_starts,
    shape_text_with_fallback_candidates,
};
use crate::layout::{FragmentStyle, InlineFragment, InlineTextSource};
use unicode_bidi::{BidiInfo, Level};
use unicode_segmentation::UnicodeSegmentation;

/// A document Text boundary selected at a painted insertion position.
pub(crate) struct TextCaretPoint {
    pub(crate) node: NodeHandle,
    pub(crate) offset: usize,
}

struct Candidate {
    fragment: InlineFragment,
    point: (f32, f32),
    distance: (f32, f32),
}

/// Finds the nearest selectable rendered Text boundary. `scope` restricts a
/// press to its hit target; dragging can search the whole active document.
pub(crate) fn hit_test_caret_layout(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    point: (f32, f32),
    scope: Option<&NodeHandle>,
    web_fonts: Option<&WebFontRegistry>,
) -> Option<TextCaretPoint> {
    let mut candidate = None;
    collect_candidates(
        layout,
        resolver,
        AffineTransform::identity(),
        Some(viewport),
        viewport,
        point,
        scope,
        &mut candidate,
    );
    let candidate = candidate?;
    let fragment = &candidate.fragment;
    let text = fragment.text()?;
    let fonts = text::load_text_fonts();
    let selected = text::select_fragment_font(web_fonts, fragment, &fonts);
    let web = text::select_fragment_web_fonts(web_fonts, fragment);
    let mut candidates: Vec<_> = web
        .iter()
        .map(|face| FontFallbackCandidate {
            font: face.font,
            unicode_range: Some(face.unicode_range),
        })
        .collect();
    if web.is_empty()
        && let Some(selected) = selected.as_ref()
    {
        candidates.push(FontFallbackCandidate::unrestricted(selected.as_ref()));
    }
    candidates.extend(
        fonts
            .iter()
            .map(Arc::as_ref)
            .map(FontFallbackCandidate::unrestricted),
    );
    let vertical = is_vertical(&fragment.style);
    let can_shape = fragment.style.resolved_bidi_level.is_some()
        || matches!(
            text::bidi_visual_text(text, &fragment.style),
            std::borrow::Cow::Borrowed(_)
        );
    let carets = (!vertical && can_shape)
        .then(|| {
            shaped_carets(
                text,
                fragment.metrics.font_size,
                fragment.metrics.letter_spacing,
                &fragment.style,
                &candidates,
            )
        })
        .flatten()
        .unwrap_or_else(|| scalar_carets(text, fragment, &candidates, vertical));
    let query = if vertical {
        candidate.point.1 - fragment.rect.y
    } else {
        candidate.point.0 - fragment.rect.x
    };
    let (byte, _) = carets
        .into_iter()
        .min_by(|a, b| (a.1 - query).abs().total_cmp(&(b.1 - query).abs()))?;
    dom_boundary(&fragment.text_source, text, byte)
}

fn descendant_of(node: &NodeHandle, ancestor: &NodeHandle) -> bool {
    let mut current = Some(node.clone());
    while let Some(node) = current {
        if node == *ancestor {
            return true;
        }
        current = node.parent_node().or_else(|| node.shadow_host());
    }
    false
}

fn distance_to_range(value: f32, start: f32, length: f32) -> f32 {
    (start - value).max(value - start - length).max(0.0)
}

fn is_vertical(style: &FragmentStyle) -> bool {
    matches!(
        style.writing_mode.as_deref(),
        Some("vertical-rl" | "vertical-lr" | "sideways-rl" | "sideways-lr")
    )
}

/// Retains logical byte indices while following the painter's visual order.
fn visual_graphemes<'a>(text: &'a str, style: &FragmentStyle) -> Vec<(usize, &'a str, bool)> {
    let forced = style
        .resolved_bidi_level
        .map(|level| level % 2 == 1)
        .or_else(|| {
            matches!(
                style.unicode_bidi.as_deref(),
                Some("bidi-override" | "isolate-override")
            )
            .then_some(style.direction.as_deref() == Some("rtl"))
        });
    if let Some(rtl) = forced {
        let mut result: Vec<_> = text
            .grapheme_indices(true)
            .map(|(byte, cluster)| (byte, cluster, rtl))
            .collect();
        if rtl {
            result.reverse();
        }
        return result;
    }
    let base = if style.unicode_bidi.as_deref() == Some("plaintext") {
        None
    } else {
        Some(if style.direction.as_deref() == Some("rtl") {
            Level::rtl()
        } else {
            Level::ltr()
        })
    };
    let bidi = BidiInfo::new(text, base);
    let mut result = Vec::new();
    for paragraph in &bidi.paragraphs {
        let (levels, runs) = bidi.visual_runs(paragraph, paragraph.range.clone());
        for range in runs {
            let rtl = levels[range.start].is_rtl();
            let mut clusters: Vec<_> = text[range.clone()]
                .grapheme_indices(true)
                .map(|(byte, cluster)| (range.start + byte, cluster, rtl))
                .collect();
            if rtl {
                clusters.reverse();
            }
            result.extend(clusters);
        }
    }
    result
}

/// Matches the scalar glyph and placeholder painting paths without inserting
/// caret positions inside combining sequences or surrogate pairs.
fn scalar_carets(
    text: &str,
    fragment: &InlineFragment,
    fonts: &[FontFallbackCandidate<'_>],
    vertical: bool,
) -> Vec<(usize, f32)> {
    let mut clusters = visual_graphemes(text, &fragment.style);
    let backwards = vertical && fragment.style.direction.as_deref() == Some("rtl");
    if backwards {
        clusters.reverse();
    }
    let sign = if backwards { -1.0 } else { 1.0 };
    let mut cursor = if backwards { fragment.rect.height } else { 0.0 };
    let mut remaining = text
        .chars()
        .filter(|ch| !crate::font::is_zero_advance_character(*ch))
        .count();
    let mut result = Vec::new();
    for (start, cluster, rtl) in clusters {
        let end = start + cluster.len();
        result.push((if rtl && !vertical { end } else { start }, cursor));
        let font = text::fallback_font_for_cluster(fonts, cluster).map(|index| fonts[index].font);
        for ch in cluster.chars() {
            if crate::font::is_zero_advance_character(ch) {
                continue;
            }
            let advance = font
                .map_or(fragment.metrics.font_size * 0.6, |font| {
                    font.glyph_advance(ch, fragment.metrics.font_size)
                })
                .max(1.0);
            cursor += sign * advance;
            remaining -= 1;
            if remaining > 0 {
                cursor += sign * fragment.metrics.letter_spacing;
            }
        }
        result.push((if rtl && !vertical { start } else { end }, cursor));
    }
    result
}

fn collect_candidates(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    transform: AffineTransform,
    clip: Option<Rect>,
    viewport: Rect,
    point: (f32, f32),
    scope: Option<&NodeHandle>,
    best: &mut Option<Candidate>,
) {
    let Some(geometry) = hit_test_box_geometry(
        layout, resolver, transform, clip, viewport, point.0, point.1,
    ) else {
        return;
    };
    if layout.content_visibility_contents_skipped {
        return;
    }
    for line in &layout.lines {
        let fragments = line
            .text_overflow
            .as_ref()
            .map_or(&line.fragments, |paint| &paint.fragments);
        for fragment in fragments {
            if fragment.style.visibility == Visibility::Hidden
                || fragment.text_source.is_empty()
                || geometry.clip.is_some_and(|clip| {
                    intersect(
                        transformed_rect_bounds(fragment.rect, geometry.transform),
                        clip,
                    )
                    .is_none()
                })
                || scope.is_some_and(|scope| {
                    !fragment
                        .text_source
                        .iter()
                        .any(|source| descendant_of(&source.node, scope))
                })
            {
                continue;
            }
            let distance = if is_vertical(&fragment.style) {
                (
                    distance_to_range(geometry.local_point.0, line.rect.x, line.rect.width),
                    distance_to_range(
                        geometry.local_point.1,
                        fragment.rect.y,
                        fragment.rect.height,
                    ),
                )
            } else {
                (
                    distance_to_range(geometry.local_point.1, line.rect.y, line.rect.height),
                    distance_to_range(geometry.local_point.0, fragment.rect.x, fragment.rect.width),
                )
            };
            if best.as_ref().is_none_or(|best| {
                distance
                    .0
                    .total_cmp(&best.distance.0)
                    .then(distance.1.total_cmp(&best.distance.1))
                    .is_lt()
            }) {
                *best = Some(Candidate {
                    fragment: fragment.clone(),
                    point: geometry.local_point,
                    distance,
                });
            }
        }
    }
    for child in &layout.children {
        collect_candidates(
            child,
            resolver,
            geometry.transform,
            geometry.clip,
            viewport,
            geometry.query_point,
            scope,
            best,
        );
    }
}

fn dom_boundary(source: &[InlineTextSource], text: &str, byte: usize) -> Option<TextCaretPoint> {
    let span = source
        .iter()
        .find(|span| span.text_range.start <= byte && byte <= span.text_range.end)?;
    let offset = if span.direct {
        span.dom_range.start + text[span.text_range.start..byte].encode_utf16().count()
    } else if byte == span.text_range.start {
        span.dom_range.start
    } else {
        span.dom_range.end
    };
    Some(TextCaretPoint {
        node: span.node.clone(),
        offset,
    })
}

/// A glyph cluster in visual order, with byte positions in logical text.
struct ShapedCluster {
    start: usize,
    end: usize,
    advance: f32,
    rtl: bool,
}

fn shaped_clusters(
    text: &str,
    size: f32,
    style: &FragmentStyle,
    fonts: &[FontFallbackCandidate<'_>],
) -> Option<Vec<ShapedCluster>> {
    let base = style
        .resolved_bidi_level
        .map(Level::new)
        .transpose()
        .ok()?
        .unwrap_or_else(|| {
            if style.direction.as_deref() == Some("rtl") {
                Level::rtl()
            } else {
                Level::ltr()
            }
        });
    let bidi = BidiInfo::new(text, Some(base));
    let runs = if let Some(level) = style.resolved_bidi_level {
        vec![(0..text.len(), level % 2 == 1)]
    } else {
        bidi.paragraphs
            .iter()
            .flat_map(|paragraph| {
                let (levels, runs) = bidi.visual_runs(paragraph, paragraph.range.clone());
                runs.into_iter()
                    .map(|range| {
                        let rtl = levels[range.start].is_rtl();
                        (range, rtl)
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let mut result = Vec::new();
    for (range, rtl) in runs {
        let shaped = shape_text_with_fallback_candidates(
            fonts,
            &text[range.clone()],
            size,
            if rtl {
                ShapingDirection::RightToLeft
            } else {
                ShapingDirection::LeftToRight
            },
        )
        .ok()?;
        for run in shaped {
            let mut glyphs = run.glyphs.iter().peekable();
            let mut starts: Vec<_> = run.glyphs.iter().map(|glyph| glyph.cluster).collect();
            starts.sort_unstable();
            starts.dedup();
            starts.push(run.text_range.end);
            while let Some(glyph) = glyphs.next() {
                let start = glyph.cluster;
                let mut advance = glyph.x_advance.abs();
                while glyphs.peek().is_some_and(|glyph| glyph.cluster == start) {
                    advance += glyphs.next().unwrap().x_advance.abs();
                }
                let end = starts[starts.binary_search(&start).ok()? + 1];
                result.push(ShapedCluster {
                    start: range.start + start,
                    end: range.start + end,
                    advance,
                    rtl,
                });
            }
        }
    }
    Some(result)
}

/// Extended grapheme boundaries placed using full-run glyph advances. A
/// ligature lacking separate glyph clusters shares its actual advance between
/// its graphemes; combining sequences never gain an interior insertion point.
fn shaped_carets(
    text: &str,
    size: f32,
    spacing: f32,
    style: &FragmentStyle,
    fonts: &[FontFallbackCandidate<'_>],
) -> Option<Vec<(usize, f32)>> {
    let clusters = shaped_clusters(text, size, style, fonts)?;
    let spacing_clusters = grapheme_spacing_cluster_starts(text);
    let spacing_boundaries = spacing_clusters.len().saturating_sub(1);
    let mut applied_spacing = 0;
    let mut result = Vec::new();
    let mut cursor = 0.0;
    for (index, cluster) in clusters.iter().enumerate() {
        let mut boundaries: Vec<_> = text[cluster.start..cluster.end]
            .grapheme_indices(true)
            .map(|(byte, _)| cluster.start + byte)
            .collect();
        boundaries.push(cluster.end);
        let divisions = boundaries.len().saturating_sub(1).max(1);
        if cluster.rtl {
            boundaries.reverse();
        }
        for (index, byte) in boundaries.into_iter().enumerate() {
            result.push((
                byte,
                cursor + cluster.advance * index as f32 / divisions as f32,
            ));
        }
        cursor += cluster.advance;
        if applied_spacing < spacing_boundaries
            && spacing_clusters.binary_search(&cluster.start).is_ok()
            && clusters
                .get(index + 1)
                .is_some_and(|next| spacing_clusters.binary_search(&next.start).is_ok())
        {
            cursor += spacing;
            applied_spacing += 1;
        }
    }
    (!result.is_empty()).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaped_carets_follow_nonuniform_font_advances() {
        let font = crate::font::Font::load_from_bytes(
            include_bytes!("../../tests/fixtures/acid2/LiberationSans-Regular.ttf").to_vec(),
        )
        .unwrap();
        let fonts = [FontFallbackCandidate::unrestricted(&font)];
        let carets = shaped_carets("Wi", 20.0, 0.0, &FragmentStyle::default(), &fonts).unwrap();
        let after_w = carets.iter().find(|(byte, _)| *byte == 1).unwrap().1;
        let after_i = carets.iter().find(|(byte, _)| *byte == 2).unwrap().1;
        assert!(after_w > (after_i - after_w) * 2.0);
        assert!((after_w - font.glyph_advance('W', 20.0)).abs() < 0.1);
        let spaced = shaped_carets("Wi", 20.0, 3.0, &FragmentStyle::default(), &fonts).unwrap();
        assert!((spaced.last().unwrap().1 - after_i - 3.0).abs() < 0.1);
    }

    fn fragment(value: &str) -> InlineFragment {
        InlineFragment {
            node: NodeHandle::text(value),
            text_source: Vec::new(),
            content: crate::layout::InlineFragmentContent::Text(value.to_owned()),
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            metrics: crate::layout::FontMetrics::from_font_size(20.0),
            vertical_align: crate::layout::VerticalAlign::Baseline,
            source_end_token: 0,
            style: FragmentStyle::default(),
        }
    }

    #[test]
    fn placeholder_carets_preserve_graphemes_and_scalar_spacing() {
        let mut fragment = fragment("a\u{301}😀b");
        fragment.metrics.letter_spacing = 3.0;
        let carets = scalar_carets(fragment.text().unwrap(), &fragment, &[], false);
        assert_eq!(
            carets,
            vec![
                (0, 0.0),
                (3, 15.0),
                (3, 15.0),
                (7, 30.0),
                (7, 30.0),
                (8, 42.0)
            ]
        );
    }

    #[test]
    fn vertical_carets_follow_physical_axis_and_rtl_pen() {
        let mut fragment = fragment("ab");
        fragment.style.writing_mode = Some("vertical-rl".into());
        assert!(is_vertical(&fragment.style));
        assert_eq!(
            scalar_carets("ab", &fragment, &[], true),
            vec![(0, 0.0), (1, 12.0), (1, 12.0), (2, 24.0)]
        );
        fragment.style.direction = Some("rtl".into());
        assert_eq!(
            scalar_carets("ab", &fragment, &[], true),
            vec![(1, 100.0), (2, 88.0), (0, 88.0), (1, 76.0)]
        );
    }
}
