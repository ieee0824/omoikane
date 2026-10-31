//! Paint Timing and Largest Contentful Paint (Issue #1293).
//!
//! At each rendering opportunity whose layout changed, the main Document's
//! layout tree is scanned for painted and contentful content and the largest
//! contentful element in the viewport. Script owns the timeline entries and
//! decides what to report; it tells the host once nothing is left to observe.

use super::*;
use crate::css::{ComputedStyle, ComputedValue};
use crate::layout::{InlineFragmentContent, LayoutBox, Visibility};

/// Host state for the rendering-opportunity scan.
#[derive(Default)]
pub(super) struct State {
    /// The layout generation scanned last; unchanged frames are skipped.
    scanned_generation: Option<u64>,
    /// Set by script once first paint, first contentful paint and the final
    /// largest contentful paint candidate have been reported.
    finished: bool,
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_paint_timing_finished"),
        0,
        NativeFunction::from_fn_ptr(paint_timing_finished_native),
    )
}

/// `__omoikane_paint_timing_finished()` stops the per-frame scan.
fn paint_timing_finished_native(
    _this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    with_host_state(|state| {
        state.borrow_mut().paint_timing.finished = true;
        Ok(JsValue::undefined())
    })
}

/// How a box paints, from its computed style.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BoxPaint {
    /// `opacity: 0`: nothing in the subtree is visible.
    pub(crate) transparent: bool,
    /// A visible background color or image.
    pub(crate) background: bool,
    /// A background image, which paints content as an image does.
    pub(crate) background_image: bool,
}

impl BoxPaint {
    pub(crate) fn from_style(style: &ComputedStyle) -> Self {
        let transparent =
            matches!(style.get("opacity"), Some(ComputedValue::Number(value)) if *value <= 0.0);
        let color = match style.get("background-color") {
            Some(ComputedValue::Color(color)) => !is_transparent_color(color),
            _ => false,
        };
        let image = matches!(style.get("background-image"), Some(value) if !matches!(value, ComputedValue::Keyword(keyword) if keyword == "none"));
        Self {
            transparent,
            background: color || image,
            background_image: image,
        }
    }
}

fn is_transparent_color(color: &str) -> bool {
    let color = color.trim();
    color.eq_ignore_ascii_case("transparent")
        || (color.starts_with("rgba(") && color.trim_end_matches(')').ends_with(", 0"))
}

/// The largest contentful element found by [`scan`].
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub(crate) node: NodeHandle,
    /// Visible area in the viewport, in CSS pixels.
    pub(crate) size: f32,
}

/// What one rendering opportunity paints.
#[derive(Debug, Clone, Default)]
pub(crate) struct PaintScan {
    /// Something other than the default canvas is painted.
    pub(crate) painted: bool,
    /// Text or an image is painted.
    pub(crate) contentful: bool,
    /// The largest text block or image in the viewport.
    pub(crate) largest: Option<Candidate>,
}

/// Scans `root` for painted content. `paint` reports each element box's
/// [`BoxPaint`]; `viewport` bounds candidate sizes.
pub(crate) fn scan(
    root: &LayoutBox,
    viewport: Rect,
    paint: &mut dyn FnMut(&NodeHandle) -> BoxPaint,
) -> PaintScan {
    let mut scan = PaintScan::default();
    let mut text_blocks: Vec<(NodeHandle, Rect)> = Vec::new();
    let mut images: Vec<(NodeHandle, Rect)> = Vec::new();
    visit(
        root,
        viewport,
        paint,
        &mut scan,
        &mut text_blocks,
        &mut images,
    );
    for (node, rect) in text_blocks.into_iter().chain(images) {
        let size = visible_area(rect, viewport);
        if size > scan.largest.as_ref().map_or(0.0, |largest| largest.size) {
            scan.largest = Some(Candidate { node, size });
        }
    }
    scan
}

fn visit(
    layout: &LayoutBox,
    viewport: Rect,
    paint: &mut dyn FnMut(&NodeHandle) -> BoxPaint,
    scan: &mut PaintScan,
    text_blocks: &mut Vec<(NodeHandle, Rect)>,
    images: &mut Vec<(NodeHandle, Rect)>,
) {
    let box_paint = if layout.pseudo.is_none() && layout.node.node_type() == NodeType::Element {
        paint(&layout.node)
    } else {
        BoxPaint::default()
    };
    // A transform that collapses the box (such as `scale(0)`) paints nothing.
    if box_paint.transparent || layout.transform.inverse().is_none() {
        return;
    }
    let visible = layout.visibility == Visibility::Visible;
    let border_box = layout.dimensions.border_box();
    if visible && box_paint.background && visible_area(border_box, viewport) > 0.0 {
        scan.painted = true;
        if box_paint.background_image {
            scan.contentful = true;
            add_rect(images, layout.node.clone(), border_box);
        }
    }
    for fragment in layout.lines.iter().flat_map(|line| &line.fragments) {
        // Content outside the viewport is not painted.
        if !visible || visible_area(fragment.rect, viewport) <= 0.0 {
            continue;
        }
        match &fragment.content {
            InlineFragmentContent::Text(text) if !text.trim().is_empty() => {
                scan.painted = true;
                scan.contentful = true;
                let element = if fragment.node.node_type() == NodeType::Element {
                    fragment.node.clone()
                } else {
                    fragment
                        .node
                        .parent_node()
                        .unwrap_or_else(|| layout.node.clone())
                };
                add_rect(text_blocks, element, fragment.rect);
            }
            InlineFragmentContent::Image(..) => {
                scan.painted = true;
                scan.contentful = true;
                add_rect(images, fragment.node.clone(), fragment.rect);
            }
            _ => {}
        }
    }
    for child in &layout.children {
        visit(child, viewport, paint, scan, text_blocks, images);
    }
}

fn add_rect(entries: &mut Vec<(NodeHandle, Rect)>, node: NodeHandle, rect: Rect) {
    if let Some((_, existing)) = entries
        .iter_mut()
        .find(|(entry, _)| entry.identity() == node.identity())
    {
        *existing = union(*existing, rect);
    } else {
        entries.push((node, rect));
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect {
        x,
        y,
        width: (a.x + a.width).max(b.x + b.width) - x,
        height: (a.y + a.height).max(b.y + b.height) - y,
    }
}

fn visible_area(rect: Rect, viewport: Rect) -> f32 {
    let width = (rect.x + rect.width).min(viewport.x + viewport.width) - rect.x.max(viewport.x);
    let height = (rect.y + rect.height).min(viewport.y + viewport.height) - rect.y.max(viewport.y);
    width.max(0.0) * height.max(0.0)
}

impl JsRuntime {
    /// Runs the paint timing step of a rendering opportunity for the main
    /// Document and reports the result to its script.
    pub(super) fn mark_paint_timing(&mut self) -> JsResult<()> {
        let (document_id, scan) = {
            let mut guard = self.host_state.borrow_mut();
            let state = &mut *guard;
            if state.paint_timing.finished || state.page_hidden {
                return Ok(());
            }
            state.ensure_layout();
            if state.paint_timing.scanned_generation == Some(state.layout_generation) {
                return Ok(());
            }
            state.paint_timing.scanned_generation = Some(state.layout_generation);
            let document_id = state.document.identity();
            let viewport = Rect {
                x: 0.0,
                y: 0.0,
                width: state.viewport.width,
                height: state.viewport.height,
            };
            let Some(root) = state.layout_root.as_ref() else {
                return Ok(());
            };
            let mut resolver = state
                .document_styles
                .get_mut(&document_id)
                .and_then(|entry| entry.resolver.as_mut());
            let mut paint = |node: &NodeHandle| {
                resolver
                    .as_mut()
                    .map(|resolver| BoxPaint::from_style(&resolver.computed_style(node)))
                    .unwrap_or_default()
            };
            (document_id, scan(root, viewport, &mut paint))
        };
        let (node_id, size) = scan.largest.as_ref().map_or((-1.0, 0.0), |largest| {
            (largest.node.identity() as f64, largest.size)
        });
        self.eval_in_document_realm(
            document_id,
            &format!(
                "if (typeof globalThis.__omoikane_paint_timing_frame === 'function') \
                 globalThis.__omoikane_paint_timing_frame({}, {}, {node_id}, {size});",
                scan.painted, scan.contentful
            ),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_colors_are_detected() {
        assert!(is_transparent_color("rgba(0, 0, 0, 0)"));
        assert!(is_transparent_color("transparent"));
        assert!(!is_transparent_color("rgb(255, 0, 0)"));
        assert!(!is_transparent_color("rgba(0, 0, 0, 0.5)"));
    }

    #[test]
    fn visible_area_clips_to_the_viewport() {
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        let rect = Rect {
            x: 50.0,
            y: 90.0,
            width: 100.0,
            height: 20.0,
        };
        assert_eq!(visible_area(rect, viewport), 500.0);
        assert_eq!(
            union(
                rect,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 10.0,
                    height: 10.0
                }
            ),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 150.0,
                height: 110.0
            }
        );
    }
}
