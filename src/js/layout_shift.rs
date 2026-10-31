//! Layout Instability (Issue #1293).
//!
//! At each rendering opportunity whose layout changed, the position of every
//! painted element box and text node of the main Document is compared with
//! the previous layout. Boxes whose starting point moved are unstable; their
//! visible area before and after the move forms the impact region, from which
//! the layout shift score is computed and reported to script.

use std::collections::HashMap;

use super::*;
use crate::css::AffineTransform;
use crate::css::ComputedValue;
use crate::layout::{InlineFragmentContent, LayoutBox, PositionScheme, Visibility};

/// The smallest move, in CSS pixels, that makes a box unstable.
const MOVEMENT_THRESHOLD: f32 = 3.0;

/// The most `LayoutShiftAttribution` sources an entry carries.
const MAX_SOURCES: usize = 5;

/// Where a tracked box was in one rendering.
#[derive(Debug, Clone)]
struct Tracked {
    node: NodeHandle,
    /// Border box in document coordinates, without transforms or scrolling.
    /// A box that moves only on screen was scrolled or transformed; one
    /// that moves only here was scrolled or transformed back into place.
    layout: Rect,
    /// Border box with transforms and every scroll offset applied, in
    /// viewport coordinates. A box that moves only here was scrolled.
    visual: Rect,
    /// The ancestors' overflow clip, in viewport coordinates.
    clip: Option<Rect>,
}

/// Box positions of one rendering.
#[derive(Debug, Clone, Default)]
pub(crate) struct Snapshot {
    boxes: HashMap<usize, Tracked>,
    /// The viewport size, at the origin.
    viewport: Rect,
}

/// Host state for the per-frame comparison.
#[derive(Default)]
pub(super) struct State {
    scanned_generation: Option<u64>,
    previous: Option<Snapshot>,
}

/// Whether a computed `opacity` hides the subtree, which is then not
/// tracked.
pub(crate) fn transparent(opacity: Option<&ComputedValue>) -> bool {
    matches!(opacity, Some(ComputedValue::Number(value)) if *value <= 0.0)
}

/// One moved box of a layout shift, with its visible rectangles in viewport
/// coordinates.
#[derive(Debug, Clone)]
pub(crate) struct Source {
    pub(crate) node: NodeHandle,
    pub(crate) previous: Rect,
    pub(crate) current: Rect,
}

/// A layout shift between two renderings.
#[derive(Debug, Clone)]
pub(crate) struct Shift {
    pub(crate) score: f64,
    pub(crate) sources: Vec<Source>,
}

/// The coordinate spaces inherited from ancestors while collecting boxes.
#[derive(Clone, Copy)]
struct Spaces {
    visual: AffineTransform,
    clip: Option<Rect>,
}

/// Records the painted element boxes and text nodes under `root`, seen
/// through a viewport of `viewport_size` scrolled by `scroll`.
pub(crate) fn snapshot(
    root: &LayoutBox,
    viewport_size: (f32, f32),
    scroll: (f32, f32),
    transparent: &mut dyn FnMut(&NodeHandle) -> bool,
) -> Snapshot {
    let mut snapshot = Snapshot {
        boxes: HashMap::new(),
        viewport: Rect {
            x: 0.0,
            y: 0.0,
            width: viewport_size.0,
            height: viewport_size.1,
        },
    };
    let spaces = Spaces {
        visual: AffineTransform::translate(-scroll.0, -scroll.1),
        clip: None,
    };
    collect(root, spaces, transparent, &mut snapshot.boxes);
    snapshot
}

fn collect(
    layout: &LayoutBox,
    spaces: Spaces,
    transparent: &mut dyn FnMut(&NodeHandle) -> bool,
    boxes: &mut HashMap<usize, Tracked>,
) {
    // Fixed and sticky boxes move with the viewport, not with layout.
    if matches!(
        layout.position_scheme,
        PositionScheme::Fixed | PositionScheme::Sticky
    ) {
        return;
    }
    let element = layout.pseudo.is_none() && layout.node.node_type() == NodeType::Element;
    if element && transparent(&layout.node) {
        return;
    }
    let spaces = Spaces {
        visual: spaces.visual.multiply(layout.transform),
        clip: spaces.clip,
    };
    let visible = layout.visibility == Visibility::Visible;
    if visible && element {
        track(boxes, &layout.node, layout.dimensions.border_box(), spaces);
    }
    for fragment in layout.lines.iter().flat_map(|line| &line.fragments) {
        let text = matches!(&fragment.content, InlineFragmentContent::Text(text) if !text.trim().is_empty());
        if visible && text && fragment.node.node_type() == NodeType::Text {
            track(boxes, &fragment.node, fragment.rect, spaces);
        }
    }
    let mut children = spaces;
    if layout.overflow.clips_overflow() {
        let padding_box = inset(layout.dimensions.border_box(), &layout.dimensions.border);
        let own = bounds(padding_box, spaces.visual);
        children.clip = Some(spaces.clip.map_or(own, |clip| intersect(clip, own)));
    }
    if layout.is_scroll_container() {
        let (x, y) = layout.scroll_offset();
        children.visual = spaces.visual.multiply(AffineTransform::translate(-x, -y));
    }
    for child in &layout.children {
        collect(child, children, transparent, boxes);
    }
}

fn track(boxes: &mut HashMap<usize, Tracked>, node: &NodeHandle, rect: Rect, spaces: Spaces) {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    let layout = rect;
    let visual = bounds(rect, spaces.visual);
    boxes
        .entry(node.identity())
        .and_modify(|tracked| {
            tracked.layout = union(tracked.layout, layout);
            tracked.visual = union(tracked.visual, visual);
        })
        .or_insert_with(|| Tracked {
            node: node.clone(),
            layout,
            visual,
            clip: spaces.clip,
        });
}

/// Compares two renderings (Layout Instability §2.3). A box is unstable when
/// its starting point moved both in layout and on screen. Returns `None`
/// when nothing visible moved.
pub(crate) fn compare(previous: &Snapshot, current: &Snapshot) -> Option<Shift> {
    let moved = |a: Rect, b: Rect| {
        (b.x - a.x).abs() >= MOVEMENT_THRESHOLD || (b.y - a.y).abs() >= MOVEMENT_THRESHOLD
    };
    let viewport = current.viewport;
    let mut region = Vec::new();
    let mut distance: f32 = 0.0;
    let mut sources = Vec::new();
    for (id, now) in &current.boxes {
        let Some(old) = previous.boxes.get(id) else {
            continue;
        };
        if !moved(old.layout, now.layout) || !moved(old.visual, now.visual) {
            continue;
        }
        let before = visible_rect(old.visual, old.clip, previous.viewport);
        let after = visible_rect(now.visual, now.clip, viewport);
        if before.is_none() && after.is_none() {
            continue;
        }
        let (dx, dy) = (now.visual.x - old.visual.x, now.visual.y - old.visual.y);
        distance = distance.max(dx.abs().max(dy.abs()));
        region.extend(before);
        region.extend(after);
        sources.push(Source {
            node: now.node.clone(),
            previous: before.unwrap_or_default(),
            current: after.unwrap_or_default(),
        });
    }
    let view_area = f64::from(viewport.width) * f64::from(viewport.height);
    let max_dimension = viewport.width.max(viewport.height);
    if region.is_empty() || view_area <= 0.0 || max_dimension <= 0.0 {
        return None;
    }
    let impact = union_area(&region) / view_area;
    let distance = f64::from(distance.min(max_dimension) / max_dimension);
    Some(Shift {
        score: impact * distance,
        sources: select_sources(sources),
    })
}

/// The part of `rect` inside `clip` and `viewport`.
fn visible_rect(rect: Rect, clip: Option<Rect>, viewport: Rect) -> Option<Rect> {
    let visible = intersect(rect, viewport);
    let visible = clip.map_or(visible, |clip| intersect(visible, clip));
    (visible.width > 0.0 && visible.height > 0.0).then_some(visible)
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let left = a.x.max(b.x);
    let top = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    Rect {
        x: left,
        y: top,
        width: (right - left).max(0.0),
        height: (bottom - top).max(0.0),
    }
}

fn inset(rect: Rect, edges: &crate::layout::EdgeSizes) -> Rect {
    Rect {
        x: rect.x + edges.left,
        y: rect.y + edges.top,
        width: (rect.width - edges.left - edges.right).max(0.0),
        height: (rect.height - edges.top - edges.bottom).max(0.0),
    }
}

/// The bounding box of `rect` mapped through `transform`.
fn bounds(rect: Rect, transform: AffineTransform) -> Rect {
    if transform.is_identity() {
        return rect;
    }
    let corners = [
        transform.transform_point(rect.x, rect.y),
        transform.transform_point(rect.x + rect.width, rect.y),
        transform.transform_point(rect.x, rect.y + rect.height),
        transform.transform_point(rect.x + rect.width, rect.y + rect.height),
    ];
    let (mut left, mut top) = corners[0];
    let (mut right, mut bottom) = corners[0];
    for (x, y) in corners {
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}

/// Keeps the sources with the largest impact, dropping one whose rectangles
/// lie inside a kept source's rectangles.
fn select_sources(mut sources: Vec<Source>) -> Vec<Source> {
    let impact = |source: &Source| union_area(&[source.previous, source.current]);
    sources.sort_by(|a, b| {
        impact(b)
            .total_cmp(&impact(a))
            .then_with(|| a.node.identity().cmp(&b.node.identity()))
    });
    let mut kept: Vec<Source> = Vec::new();
    for source in sources {
        let enclosed = kept.iter().any(|outer| {
            contains(outer.previous, source.previous) && contains(outer.current, source.current)
        });
        if !enclosed {
            kept.push(source);
        }
        if kept.len() == MAX_SOURCES {
            break;
        }
    }
    kept
}

fn contains(outer: Rect, inner: Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x + inner.width <= outer.x + outer.width
        && inner.y + inner.height <= outer.y + outer.height
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

/// The area covered by `rects`, counting overlaps once.
fn union_area(rects: &[Rect]) -> f64 {
    let mut xs: Vec<f32> = rects
        .iter()
        .flat_map(|rect| [rect.x, rect.x + rect.width])
        .collect();
    xs.sort_by(f32::total_cmp);
    xs.dedup();
    let mut area = 0.0;
    for pair in xs.windows(2) {
        let (left, right) = (pair[0], pair[1]);
        let mut spans: Vec<(f32, f32)> = rects
            .iter()
            .filter(|rect| rect.x <= left && rect.x + rect.width >= right)
            .map(|rect| (rect.y, rect.y + rect.height))
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut covered = 0.0;
        let mut current: Option<(f32, f32)> = None;
        for (top, bottom) in spans {
            match current {
                Some((start, end)) if top <= end => current = Some((start, end.max(bottom))),
                _ => {
                    if let Some((start, end)) = current {
                        covered += f64::from(end - start);
                    }
                    current = Some((top, bottom));
                }
            }
        }
        if let Some((start, end)) = current {
            covered += f64::from(end - start);
        }
        area += covered * f64::from(right - left);
    }
    area
}

impl JsRuntime {
    /// Runs the layout shift step of a rendering opportunity for the main
    /// Document and reports a shift to its script.
    pub(super) fn mark_layout_shift(&mut self) -> JsResult<()> {
        let (document_id, shift) = {
            let mut guard = self.host_state.borrow_mut();
            let state = &mut *guard;
            if state.page_hidden {
                return Ok(());
            }
            state.ensure_layout();
            if state.layout_shift.scanned_generation == Some(state.layout_generation) {
                return Ok(());
            }
            state.layout_shift.scanned_generation = Some(state.layout_generation);
            let document_id = state.document.identity();
            let scroll = state.window_scroll;
            let viewport = (state.viewport.width, state.viewport.height);
            let Some(root) = state.layout_root.as_ref() else {
                return Ok(());
            };
            let mut resolver = state
                .document_styles
                .get_mut(&document_id)
                .and_then(|entry| entry.resolver.as_mut());
            // Only `opacity` is read: cloning whole styles per box would
            // dominate the cost of every frame that changes layout.
            let mut transparent = |node: &NodeHandle| {
                resolver.as_mut().is_some_and(|resolver| {
                    self::transparent(resolver.computed_property(node, "opacity").as_ref())
                })
            };
            let current = snapshot(root, viewport, scroll, &mut transparent);
            let shift = state
                .layout_shift
                .previous
                .as_ref()
                .and_then(|previous| compare(previous, &current));
            state.layout_shift.previous = Some(current);
            (document_id, shift)
        };
        let Some(shift) = shift else {
            return Ok(());
        };
        let rect = |rect: Rect| [rect.x, rect.y, rect.width, rect.height];
        let sources: Vec<_> = shift
            .sources
            .iter()
            .map(|source| {
                serde_json::json!({
                    "node": source.node.identity(),
                    "previousRect": rect(source.previous),
                    "currentRect": rect(source.current),
                })
            })
            .collect();
        let report = serde_json::json!({ "score": shift.score, "sources": sources });
        self.eval_in_document_realm(
            document_id,
            &format!(
                "if (typeof globalThis.__omoikane_layout_shift_frame === 'function') \
                 globalThis.__omoikane_layout_shift_frame({report});"
            ),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// A one-box rendering; `scroll` moves the box on screen only.
    fn snapshot_of(node: &NodeHandle, rect: Rect, viewport: Rect) -> Snapshot {
        scrolled(node, rect, (0.0, 0.0), viewport)
    }

    fn scrolled(node: &NodeHandle, rect: Rect, scroll: (f32, f32), viewport: Rect) -> Snapshot {
        let visual = Rect {
            x: rect.x - scroll.0,
            y: rect.y - scroll.1,
            ..rect
        };
        let tracked = Tracked {
            node: node.clone(),
            layout: rect,
            visual,
            clip: None,
        };
        Snapshot {
            boxes: HashMap::from([(node.identity(), tracked)]),
            viewport,
        }
    }

    fn element() -> NodeHandle {
        crate::html::TreeBuilder::parse("<div id=a></div>")
            .document()
            .query_selector("#a")
            .unwrap()
    }

    #[test]
    fn union_area_counts_overlaps_once() {
        assert_eq!(union_area(&[rect(0.0, 0.0, 10.0, 10.0)]), 100.0);
        assert_eq!(
            union_area(&[rect(0.0, 0.0, 10.0, 10.0), rect(5.0, 5.0, 10.0, 10.0)]),
            175.0
        );
    }

    #[test]
    fn a_moved_box_scores_its_impact_and_distance() {
        let viewport = rect(0.0, 0.0, 800.0, 600.0);
        let node = element();
        let before = snapshot_of(&node, rect(0.0, 0.0, 300.0, 200.0), viewport);
        let after = snapshot_of(&node, rect(0.0, 160.0, 300.0, 200.0), viewport);
        let shift = compare(&before, &after).unwrap();
        let expected = (300.0 * 360.0) / (800.0 * 600.0) * (160.0 / 800.0);
        assert!((shift.score - expected).abs() < 1e-9, "{}", shift.score);
        assert_eq!(shift.sources.len(), 1);
        assert_eq!(shift.sources[0].current, rect(0.0, 160.0, 300.0, 200.0));
        // Moves under the threshold are not shifts.
        let nudged = snapshot_of(&node, rect(2.0, 2.0, 300.0, 200.0), viewport);
        assert!(compare(&before, &nudged).is_none());
    }

    #[test]
    fn only_visible_parts_count_and_distance_is_clamped() {
        let viewport = rect(0.0, 0.0, 800.0, 600.0);
        let node = element();
        let outside = snapshot_of(&node, rect(0.0, 600.0, 600.0, 200.0), viewport);
        let further = snapshot_of(&node, rect(0.0, 800.0, 600.0, 200.0), viewport);
        assert!(compare(&outside, &further).is_none());
        let inside = snapshot_of(&node, rect(0.0, 400.0, 600.0, 200.0), viewport);
        let shift = compare(&outside, &inside).unwrap();
        let expected = (600.0 * 200.0) / (800.0 * 600.0) * (200.0 / 800.0);
        assert!((shift.score - expected).abs() < 1e-9);
        let far = snapshot_of(&node, rect(-16000.0, -12000.0, 800.0, 600.0), viewport);
        let home = snapshot_of(&node, viewport, viewport);
        assert!((compare(&far, &home).unwrap().score - 1.0).abs() < 1e-9);
    }

    #[test]
    fn scrolling_alone_is_not_a_shift() {
        let node = element();
        let viewport = rect(0.0, 0.0, 800.0, 600.0);
        let box_rect = rect(0.0, 100.0, 300.0, 200.0);
        let before = scrolled(&node, box_rect, (0.0, 0.0), viewport);
        let after = scrolled(&node, box_rect, (0.0, 50.0), viewport);
        assert!(compare(&before, &after).is_none());
        // A layout move cancelled by scrolling leaves the box in place.
        let moved = scrolled(&node, rect(0.0, 150.0, 300.0, 200.0), (0.0, 50.0), viewport);
        assert!(compare(&before, &moved).is_none());
    }

    #[test]
    fn clips_and_transforms_bound_the_visible_area() {
        let clip = rect(0.0, 0.0, 100.0, 100.0);
        let visible = visible_rect(
            rect(50.0, 50.0, 100.0, 100.0),
            Some(clip),
            rect(0.0, 0.0, 800.0, 600.0),
        );
        assert_eq!(visible, Some(rect(50.0, 50.0, 50.0, 50.0)));
        assert_eq!(
            visible_rect(
                rect(200.0, 0.0, 10.0, 10.0),
                Some(clip),
                rect(0.0, 0.0, 800.0, 600.0)
            ),
            None
        );
        let scaled = bounds(
            rect(10.0, 10.0, 10.0, 10.0),
            AffineTransform::scale(2.0, 2.0),
        );
        assert_eq!(scaled, rect(20.0, 20.0, 20.0, 20.0));
    }
}
