//! Bounded, paint-order-preserving extraction of viewport-fixed raster runs.
//!
//! This first-stage collector owns only pixels and numeric metadata. It does not
//! publish overscan coverage or connect the retained result to the GUI.

use super::*;

const MAX_RETAINED_RUNS: usize = 64;
const MAX_RETAINED_BYTES: usize = 64 * 1024 * 1024;
const MAX_VIEWPORT_FIXED_ROOTS: usize = (MAX_RETAINED_RUNS - 1) / 2;

/// Coordinate space of one extracted full-viewport raster run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetainedPaintSpace {
    Document,
    ViewportFixed,
}

/// One owned full-viewport raster in original paint order.
pub(crate) struct RetainedPaintRun {
    pub(crate) space: RetainedPaintSpace,
    pub(crate) image: Image,
}

/// Owned result of a successful retained-paint extraction.
pub(crate) struct RetainedPaintFrame {
    width: u32,
    height: u32,
    runs: Vec<RetainedPaintRun>,
}

impl RetainedPaintFrame {
    /// Returns the extracted runs in exact painter traversal order.
    pub(crate) fn runs(&self) -> &[RetainedPaintRun] {
        &self.runs
    }

    /// Re-composites all runs using the ordinary canvas source-over path.
    pub(crate) fn compose(&self) -> Canvas {
        let mut canvas = Canvas::new(self.width, self.height);
        for run in &self.runs {
            canvas.draw_image(&run.image, 0.0, 0.0);
        }
        canvas
    }
}

/// Conservative reason that extraction returned the ordinary painter result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetainedPaintError {
    InvalidViewport,
    Capacity,
    StickyPosition,
    TopLayer,
    FixedBackground,
    BackdropFilter,
    CrossSpaceEffect,
    CrossSpaceClip,
    TraversalMismatch,
    CollectionFailed,
}

/// Either a fully owned retained frame or the ordinary painter fallback.
pub(crate) enum RetainedPaintOutcome {
    Extracted(RetainedPaintFrame),
    Fallback {
        canvas: Canvas,
        reason: RetainedPaintError,
        discarded_runs: usize,
    },
    /// The requested output surface was unsafe to allocate, so no paint was
    /// attempted and no fallback canvas exists.
    Rejected {
        reason: RetainedPaintError,
    },
}

impl RetainedPaintOutcome {
    /// Returns pixels when the requested surface passed the allocation guard.
    pub(crate) fn into_canvas(self) -> Result<Canvas, RetainedPaintError> {
        match self {
            Self::Extracted(frame) => Ok(frame.compose()),
            Self::Fallback { canvas, .. } => Ok(canvas),
            Self::Rejected { reason } => Err(reason),
        }
    }
}

/// Identifies whether the current canvas is a root output or a local surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaintDestination {
    RootDocument,
    Temporary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct LayoutAddress(usize);

impl LayoutAddress {
    fn of(layout: &LayoutBox) -> Self {
        // The immutable layout tree outlives the complete paint call. These
        // addresses are call-local and are never included in retained output.
        Self(layout as *const LayoutBox as usize)
    }
}

struct ExtractionPlan {
    width: u32,
    height: u32,
    surface_bytes: usize,
    expected_runs: usize,
    fixed_roots: HashSet<LayoutAddress>,
}

#[derive(Clone, Copy)]
struct SurfaceDimensions {
    width: u32,
    height: u32,
    bytes: usize,
}

impl SurfaceDimensions {
    fn for_viewport(viewport: Rect) -> Result<Self, RetainedPaintError> {
        if !viewport.x.is_finite()
            || !viewport.y.is_finite()
            || !viewport.width.is_finite()
            || !viewport.height.is_finite()
            || viewport.width <= 0.0
            || viewport.height <= 0.0
        {
            return Err(RetainedPaintError::InvalidViewport);
        }
        let width = viewport.width.ceil() as f64;
        let height = viewport.height.ceil() as f64;
        if width > u32::MAX as f64 || height > u32::MAX as f64 {
            return Err(RetainedPaintError::Capacity);
        }
        let width = width as u32;
        let height = height as u32;
        let bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(RetainedPaintError::Capacity)?;
        if bytes > MAX_RETAINED_BYTES {
            return Err(RetainedPaintError::Capacity);
        }
        Ok(Self {
            width,
            height,
            bytes,
        })
    }
}

#[derive(Clone, Copy, Default)]
struct ScanState {
    fixed_containing_ancestor: bool,
    inside_viewport_fixed: bool,
    effect_ancestor: bool,
    clip_ancestor: bool,
}

impl ExtractionPlan {
    fn analyze(
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        dimensions: SurfaceDimensions,
    ) -> Result<Self, RetainedPaintError> {
        let mut fixed_roots = HashSet::new();
        scan_layout(layout, resolver, ScanState::default(), &mut fixed_roots)?;
        let expected_runs = fixed_roots
            .len()
            .checked_mul(2)
            .and_then(|runs| runs.checked_add(1))
            .ok_or(RetainedPaintError::Capacity)?;
        let retained_bytes = dimensions
            .bytes
            .checked_mul(expected_runs)
            .ok_or(RetainedPaintError::Capacity)?;
        if expected_runs > MAX_RETAINED_RUNS || retained_bytes > MAX_RETAINED_BYTES {
            return Err(RetainedPaintError::Capacity);
        }
        Ok(Self {
            width: dimensions.width,
            height: dimensions.height,
            surface_bytes: dimensions.bytes,
            expected_runs,
            fixed_roots,
        })
    }
}

fn scan_layout(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    state: ScanState,
    fixed_roots: &mut HashSet<LayoutAddress>,
) -> Result<(), RetainedPaintError> {
    // A top-layer backdrop is painted before visibility is checked.
    if layout.node.top_layer_order().is_some() {
        return Err(RetainedPaintError::TopLayer);
    }
    if layout.visibility == Visibility::Hidden {
        return Ok(());
    }

    let style = paint_box_style(layout, resolver);
    if is_sticky_for_paint(&style) {
        return Err(RetainedPaintError::StickyPosition);
    }
    if has_fixed_background(&style) {
        return Err(RetainedPaintError::FixedBackground);
    }
    if has_non_none_value(&style, "backdrop-filter") {
        return Err(RetainedPaintError::BackdropFilter);
    }

    let viewport_fixed = is_fixed_for_paint(&style)
        && !state.fixed_containing_ancestor
        && !state.inside_viewport_fixed;
    if viewport_fixed && state.effect_ancestor {
        return Err(RetainedPaintError::CrossSpaceEffect);
    }
    if viewport_fixed && state.clip_ancestor {
        return Err(RetainedPaintError::CrossSpaceClip);
    }
    if viewport_fixed {
        if fixed_roots.len() >= MAX_VIEWPORT_FIXED_ROOTS {
            return Err(RetainedPaintError::Capacity);
        }
        fixed_roots.insert(LayoutAddress::of(layout));
    }
    if layout.content_visibility_contents_skipped {
        return Ok(());
    }
    scan_inline_fragment_styles(layout, resolver)?;

    let grouping_effect = element_opacity(&style).is_some_and(|value| value < 1.0)
        || has_non_none_value(&style, "filter")
        || has_non_none_layer(&style, "mask-image");
    let descendant_clip = layout.overflow.clips_overflow()
        || crate::layout::has_containment(&style, "paint")
        || has_non_none_value(&style, "clip-path");
    let child_state = ScanState {
        fixed_containing_ancestor: state.fixed_containing_ancestor
            || crate::layout::establishes_fixed_containing_block(&style),
        inside_viewport_fixed: state.inside_viewport_fixed || viewport_fixed,
        effect_ancestor: if state.inside_viewport_fixed || viewport_fixed {
            false
        } else {
            state.effect_ancestor || grouping_effect
        },
        clip_ancestor: if state.inside_viewport_fixed || viewport_fixed {
            false
        } else {
            state.clip_ancestor || descendant_clip
        },
    };
    for child in &layout.children {
        scan_layout(child, resolver, child_state, fixed_roots)?;
    }
    Ok(())
}

fn scan_inline_fragment_styles(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
) -> Result<(), RetainedPaintError> {
    for line in &layout.lines {
        let fragments = line
            .text_overflow
            .as_ref()
            .map_or(line.fragments.as_slice(), |overflow| {
                overflow.fragments.as_slice()
            });
        for fragment in fragments {
            if fragment.style.visibility == Visibility::Hidden {
                continue;
            }
            let captured = match &fragment.content {
                InlineFragmentContent::Image(_, style)
                | InlineFragmentContent::InlineBox(style)
                | InlineFragmentContent::GeneratedBox(style)
                | InlineFragmentContent::FormControl(style, ..) => Some(style),
                _ => None,
            };
            let Some(captured) = captured else {
                continue;
            };
            let paint = text::fragment_box_paint_style(fragment, resolver);
            if has_fixed_background(paint.as_deref().unwrap_or(captured)) {
                return Err(RetainedPaintError::FixedBackground);
            }
        }
    }
    Ok(())
}

fn has_non_none_value(style: &ComputedStyle, property: &str) -> bool {
    match style.get(property) {
        None => false,
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => {
            let value = value.trim();
            !value.is_empty() && !value.eq_ignore_ascii_case("none")
        }
        Some(_) => true,
    }
}

fn has_non_none_layer(style: &ComputedStyle, property: &str) -> bool {
    match style.get(property) {
        None => false,
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => {
            crate::css::split_top_level_commas(value)
                .into_iter()
                .map(str::trim)
                .any(|value| !value.is_empty() && !value.eq_ignore_ascii_case("none"))
        }
        Some(_) => true,
    }
}

fn has_fixed_background(style: &ComputedStyle) -> bool {
    style
        .get("background-attachment")
        .map(ComputedValue::css_text)
        .is_some_and(|value| {
            crate::css::split_top_level_commas(&value)
                .into_iter()
                .any(|value| value.trim().eq_ignore_ascii_case("fixed"))
        })
}

pub(super) struct RetainedPaintCollector {
    plan: ExtractionPlan,
    runs: Vec<RetainedPaintRun>,
    retained_bytes: usize,
    error: Option<RetainedPaintError>,
    #[cfg(test)]
    fail_after_runs: Option<usize>,
}

impl RetainedPaintCollector {
    fn new(plan: ExtractionPlan) -> Self {
        Self {
            runs: Vec::with_capacity(plan.expected_runs),
            retained_bytes: 0,
            error: None,
            #[cfg(test)]
            fail_after_runs: None,
            plan,
        }
    }

    #[cfg(test)]
    fn with_failure_after(mut self, run_count: usize) -> Self {
        self.fail_after_runs = Some(run_count);
        self
    }

    fn is_fixed_root(&self, layout: &LayoutBox) -> bool {
        self.plan.fixed_roots.contains(&LayoutAddress::of(layout))
    }

    fn flush_document(&mut self, canvas: &mut Canvas) -> bool {
        if self.error.is_some() {
            return false;
        }
        let replacement = Canvas::new(canvas.width(), canvas.height());
        let completed = std::mem::replace(canvas, replacement);
        self.push(completed, RetainedPaintSpace::Document)
    }

    fn push_fixed(&mut self, canvas: Canvas) -> bool {
        self.push(canvas, RetainedPaintSpace::ViewportFixed)
    }

    fn push(&mut self, canvas: Canvas, space: RetainedPaintSpace) -> bool {
        #[cfg(test)]
        if self.fail_after_runs == Some(self.runs.len()) {
            self.error = Some(RetainedPaintError::CollectionFailed);
            return false;
        }
        let next_bytes = self.retained_bytes.checked_add(self.plan.surface_bytes);
        if canvas.width() != self.plan.width
            || canvas.height() != self.plan.height
            || self.runs.len() >= self.plan.expected_runs
            || self.runs.len() >= MAX_RETAINED_RUNS
            || next_bytes.is_none_or(|bytes| bytes > MAX_RETAINED_BYTES)
        {
            self.error = Some(RetainedPaintError::CollectionFailed);
            return false;
        }
        self.retained_bytes = next_bytes.unwrap_or(self.retained_bytes);
        self.runs.push(RetainedPaintRun {
            space,
            image: canvas.into_image(),
        });
        true
    }

    fn finish(mut self, canvas: Canvas) -> Result<RetainedPaintFrame, PaintAttemptFailure> {
        if self.error.is_none() {
            self.push(canvas, RetainedPaintSpace::Document);
        }
        if let Some(error) = self.error {
            return Err(PaintAttemptFailure {
                reason: error,
                discarded_runs: self.runs.len(),
            });
        }
        if self.runs.len() != self.plan.expected_runs {
            return Err(PaintAttemptFailure {
                reason: RetainedPaintError::TraversalMismatch,
                discarded_runs: self.runs.len(),
            });
        }
        Ok(RetainedPaintFrame {
            width: self.plan.width,
            height: self.plan.height,
            runs: self.runs,
        })
    }
}

struct PaintAttemptFailure {
    reason: RetainedPaintError,
    discarded_runs: usize,
}

/// Switches a classified viewport-fixed root into its own full-viewport run.
/// Returns true when the caller must not paint the subtree again.
pub(super) fn intercept_viewport_fixed_root(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    context: PaintContext<'_>,
    options: PaintBoxOptions,
) -> bool {
    if context.retained_destination != PaintDestination::RootDocument {
        return false;
    }
    let Some(collector) = context.retained_collector else {
        return false;
    };
    if !collector.borrow().is_fixed_root(layout) {
        return false;
    }
    if !collector.borrow_mut().flush_document(canvas) {
        return true;
    }

    let mut fixed_canvas = Canvas::new(canvas.width(), canvas.height());
    paint_box_internal(
        &mut fixed_canvas,
        layout,
        resolver,
        PaintContext {
            retained_collector: None,
            retained_destination: PaintDestination::Temporary,
            ..context
        },
        options,
    );
    collector.borrow_mut().push_fixed(fixed_canvas);
    true
}

/// Runs the real painter with a bounded local collector. Unsupported or failed
/// extraction is discarded and the ordinary painter is rerun for this frame.
pub(crate) fn paint_layout_with_retained_fallback(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    fonts: &[Arc<Font>],
    web_fonts: Option<&WebFontRegistry>,
) -> RetainedPaintOutcome {
    paint_layout_with_retained_fallback_impl(
        layout,
        resolver,
        viewport,
        fonts,
        web_fonts,
        None,
        &[],
        None,
        None,
    )
}

/// Extracts a live document with its existing child snapshots, private visited
/// colors, and owner-held shadow cache. Every borrowed input is call-local;
/// only owned pixel runs are returned. Collection does not change CSSOM state.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_layout_with_retained_document_snapshots(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    fonts: &[Arc<Font>],
    web_fonts: Option<&WebFontRegistry>,
    visited_link_ids: &[usize],
    nested_documents: &HashMap<usize, Image>,
    shadow_masks: &border::FrameShadowMaskCache,
) -> RetainedPaintOutcome {
    paint_layout_with_retained_fallback_impl(
        layout,
        resolver,
        viewport,
        fonts,
        web_fonts,
        Some(nested_documents),
        visited_link_ids,
        Some(shadow_masks),
        None,
    )
}

#[cfg(test)]
pub(super) fn paint_layout_with_injected_collection_failure(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    fail_after_runs: usize,
) -> RetainedPaintOutcome {
    paint_layout_with_retained_fallback_impl(
        layout,
        resolver,
        viewport,
        &[],
        None,
        None,
        &[],
        None,
        Some(fail_after_runs),
    )
}

#[allow(clippy::too_many_arguments)]
fn paint_layout_with_retained_fallback_impl(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    fonts: &[Arc<Font>],
    web_fonts: Option<&WebFontRegistry>,
    nested_documents: Option<&HashMap<usize, Image>>,
    visited_link_ids: &[usize],
    external_shadow_masks: Option<&border::FrameShadowMaskCache>,
    fail_after_runs: Option<usize>,
) -> RetainedPaintOutcome {
    #[cfg(not(test))]
    let _ = fail_after_runs;
    let dimensions = match SurfaceDimensions::for_viewport(viewport) {
        Ok(dimensions) => dimensions,
        Err(reason) => return RetainedPaintOutcome::Rejected { reason },
    };
    resolver.begin_visited_paint(visited_link_ids.iter().copied());
    let result = match ExtractionPlan::analyze(layout, resolver, dimensions) {
        Err(reason) => Err(PaintAttemptFailure {
            reason,
            discarded_runs: 0,
        }),
        Ok(plan) => {
            let mut canvas = Canvas::new(dimensions.width, dimensions.height);
            let collector = RetainedPaintCollector::new(plan);
            #[cfg(test)]
            let collector = match fail_after_runs {
                Some(limit) => collector.with_failure_after(limit),
                None => collector,
            };
            let collector = RefCell::new(collector);
            let viewport_background = viewport_background_color(layout, resolver);
            if let Some(background) = viewport_background.color {
                canvas.fill_rect(viewport, background);
            }
            let local_shadow_masks = external_shadow_masks
                .is_none()
                .then(|| border::FrameShadowMaskCache::new(false));
            let shadow_masks = external_shadow_masks
                .or(local_shadow_masks.as_ref())
                .expect("a local or owner-held shadow-mask cache");
            text::with_render_glyph_cache(|| {
                paint_box_internal(
                    &mut canvas,
                    layout,
                    resolver,
                    PaintContext {
                        viewport,
                        text_fonts: fonts,
                        web_fonts,
                        nested_documents,
                        nested_snapshot_draw_calls: None,
                        propagated_background_node: viewport_background.propagated_node,
                        shadow_masks: Some(shadow_masks),
                        retained_collector: Some(&collector),
                        retained_destination: PaintDestination::RootDocument,
                    },
                    PaintBoxOptions {
                        inherited_clip: None,
                        include_phase_descendants: true,
                        paint_decorations: true,
                        offset: PaintOffset::default(),
                    },
                );
            });
            collector.into_inner().finish(canvas)
        }
    };

    resolver.end_visited_paint();
    match result {
        Ok(frame) => RetainedPaintOutcome::Extracted(frame),
        Err(failure) => RetainedPaintOutcome::Fallback {
            canvas: match nested_documents {
                Some(snapshots) => {
                    paint_layout_with_document_snapshots_internal(
                        layout,
                        resolver,
                        viewport,
                        fonts.to_vec(),
                        web_fonts,
                        visited_link_ids.iter().copied(),
                        snapshots,
                        false,
                        external_shadow_masks,
                    )
                    .0
                }
                None => paint_layout_with_web_fonts(
                    layout,
                    resolver,
                    viewport,
                    fonts.to_vec(),
                    web_fonts,
                ),
            },
            reason: failure.reason,
            discarded_runs: failure.discarded_runs,
        },
    }
}
