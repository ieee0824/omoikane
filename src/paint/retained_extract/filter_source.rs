//! Owned sparse pre-filter pixels. CSSOM and canonical filter semantics stay on the owner.
use super::*;

#[path = "filter_source_ancestor_diagnostics.rs"]
mod ancestor_diagnostics;
use ancestor_diagnostics::FilterAncestorTrace;
#[path = "filter_source_clip.rs"]
mod clip_chain;
use clip_chain::{MAX_OWNED_OVERFLOW_CLIPS, OwnedOverflowClipChain};

const MAX_FILTER_SOURCE_GROUPS: usize = 256;
const MAX_FILTER_CLIP_ASSOCIATIONS: usize = MAX_FILTER_SOURCE_GROUPS * MAX_OWNED_OVERFLOW_CLIPS;
const MAX_RAW_FIXED_CLASSIFICATIONS: usize = 1024;
const MAX_RAW_STICKY_BOXES: usize = 1024;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct FilterRecipe {
    pub(super) node: usize,
    bounds: Rect,
    geometry: BoundsNode,
    clips: OwnedOverflowClipChain,
    paint_owner_node_id: Option<usize>,
    paint_ordinal: Option<usize>,
    paint_offset: Option<PaintOffsetKey>,
    atomic_inline: Option<AtomicPaintInvocation>,
    filters: Vec<crate::css::FilterFunction>,
    opacity: f32,
    active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PaintOffsetKey {
    x: u32,
    y: u32,
}

impl PaintOffsetKey {
    fn of(offset: PaintOffset) -> Self {
        Self {
            x: offset.x.to_bits(),
            y: offset.y.to_bits(),
        }
    }
}

/// Replays only numeric geometry, in the canonical subtree walk's f32 order.
/// Every accepted descendant receives the same document translation. Ink,
/// fragments, transforms and alternate scroll chains are deliberately excluded.
#[derive(Clone, Debug, PartialEq)]
struct BoundsNode {
    dimensions: crate::layout::BoxDimensions,
    children: Vec<BoundsNode>,
    padding: (usize, usize, usize, usize),
    nodes: usize,
}

impl BoundsNode {
    fn capture(
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        budget: &mut usize,
        depth: usize,
    ) -> Result<Self, RetainedPaintError> {
        if *budget == 0 || depth >= 128 {
            return filter_reject(layout, "bounds_node_limit", 0);
        }
        *budget -= 1;
        let style = paint_box_style(layout, resolver);
        if !layout.lines.is_empty()
            || layout.marker.is_some()
            || !layout.block_fragments.is_empty()
            || layout.pseudo.is_some()
            || layout.transform != AffineTransform::identity()
            || has_transform_style(&style)
            || is_fixed_for_paint(&style)
            || is_sticky_for_paint(&style)
            || layout.node.scroll_offset() != (0.0, 0.0)
            || has_non_none_value(&style, PropertyId::BoxShadow)
            || outline::solid_outline_bounds(&style, border_box_rect(layout)).is_some()
            || !bounded_filters(&element_filters(&style))
        {
            return filter_reject(layout, "unmodelled_source_bounds", 0);
        }
        let dimensions = layout.dimensions;
        let content = dimensions.content;
        let edges = [
            dimensions.padding.top,
            dimensions.padding.right,
            dimensions.padding.bottom,
            dimensions.padding.left,
            dimensions.border.top,
            dimensions.border.right,
            dimensions.border.bottom,
            dimensions.border.left,
        ];
        if ![content.x, content.y, content.width, content.height]
            .into_iter()
            .all(|value| value.is_finite() && value.abs() <= 1_048_576.0)
            || content.width < 0.0
            || content.height < 0.0
            || !edges
                .into_iter()
                .all(|value| value.is_finite() && (0.0..=1_048_576.0).contains(&value))
        {
            return filter_reject(layout, "bounds_component_range", 0);
        }
        let rect = dimensions.border_box();
        if ![
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            rect.x + rect.width,
            rect.y + rect.height,
        ]
        .into_iter()
        .all(|value| value.is_finite() && value.abs() <= 1_048_576.0)
            || rect.width < 0.0
            || rect.height < 0.0
        {
            return filter_reject(layout, "bounds_range", 0);
        }
        let children = layout
            .children
            .iter()
            .map(|child| Self::capture(child, resolver, budget, depth + 1))
            .collect::<Result<Vec<_>, _>>()?;
        let nodes = 1 + children.iter().map(|child| child.nodes).sum::<usize>();
        Ok(Self {
            dimensions,
            children,
            padding: filter_padding(&element_filters(&style)),
            nodes,
        })
    }

    fn at_scroll(&self, scroll: (f64, f64)) -> Rect {
        let mut dimensions = self.dimensions;
        dimensions.content.x += -(scroll.0 as f32);
        dimensions.content.y += -(scroll.1 as f32);
        let mut bounds = dimensions.border_box();
        for child in &self.children {
            bounds = union_rect(bounds, child.at_scroll(scroll));
        }
        let (left, top, right, bottom) = self.padding;
        bounds.x -= left as f32;
        bounds.y -= top as f32;
        bounds.width += (left + right) as f32;
        bounds.height += (top + bottom) as f32;
        bounds
    }

    fn heap_bytes(&self) -> Option<usize> {
        self.children
            .capacity()
            .checked_mul(std::mem::size_of::<Self>())
            .and_then(|bytes| {
                self.children
                    .iter()
                    .try_fold(bytes, |sum, child| sum.checked_add(child.heap_bytes()?))
            })
    }

    fn rounding_slack(&self) -> f32 {
        // With accepted coordinates/scroll below 2^20, intermediates stay below
        // 2^22 (ULP <= .5). Fewer than 16 rounded bounds operations per node;
        // this deliberately loose 16px/node envelope bounds raw-shift error.
        16.0 * (self.nodes + 1) as f32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl PixelRect {
    fn contains(self, x: i64, y: i64) -> bool {
        x >= i64::from(self.x)
            && y >= i64::from(self.y)
            && x < i64::from(self.x) + i64::from(self.width)
            && y < i64::from(self.y) + i64::from(self.height)
    }

    fn bytes(self) -> Result<usize, RetainedPaintError> {
        (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|area| area.checked_mul(4))
            .ok_or(RetainedPaintError::Capacity)
    }
}

impl FilterRecipe {
    fn buffer(&self, viewport: Rect, scroll: (f64, f64)) -> Option<PixelRect> {
        let bounds = self.geometry.at_scroll(scroll);
        let inherited_clip = self.clips.replay(viewport, scroll)?;
        let area = intersect(bounds, inherited_clip)?;
        let x = area.x.floor().max(0.0) as i32;
        let y = area.y.floor().max(0.0) as i32;
        let right = (area.x + area.width).ceil().min(viewport.width.ceil()) as i32;
        let bottom = (area.y + area.height).ceil().min(viewport.height.ceil()) as i32;
        (right > x && bottom > y).then_some(PixelRect {
            x,
            y,
            width: (right - x) as u32,
            height: (bottom - y) as u32,
        })
    }

    fn scratch(&self, dimensions: SurfaceDimensions) -> Result<usize, RetainedPaintError> {
        // Canonical buffers never exceed the viewport. Include blur integral
        // sums, crop/source/drop-shadow canvases and their transient copies.
        let width = ((self.bounds.width + 2.0 * self.geometry.rounding_slack())
            .ceil()
            .max(0.0) as usize)
            .saturating_add(2)
            .min(dimensions.width as usize);
        let height = ((self.bounds.height + 2.0 * self.geometry.rounding_slack())
            .ceil()
            .max(0.0) as usize)
            .saturating_add(2)
            .min(dimensions.height as usize);
        width
            .checked_add(1)
            .and_then(|w| height.checked_add(1).and_then(|h| w.checked_mul(h)))
            .and_then(|area| area.checked_mul(64))
            .ok_or(RetainedPaintError::Capacity)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RawFixedSpace {
    Document,
    Viewport,
}

#[derive(Clone)]
pub(super) struct FilterSourcePlan {
    recipes: HashMap<usize, FilterRecipe>,
    ordered_active: Vec<usize>,
    skipped_by_clip_owner: HashMap<usize, Vec<usize>>,
    raw_fixed: HashMap<LayoutAddress, RawFixedSpace>,
    raw_viewport_fixed_count: usize,
    raw_sticky: HashSet<LayoutAddress>,
    viewport: Rect,
    pub(super) scroll: (i32, i32),
    pub(super) source_bytes: usize,
    pub(super) scratch_bytes: usize,
    pub(super) metadata_bytes: usize,
}

impl FilterSourcePlan {
    pub(super) fn analyze(
        view: OverscanPaintView<'_>,
        resolver: &mut StyleResolver,
        sticky: &StickyPlan,
        coverage: &ResolvedVerticalOverscanPlan,
        dimensions: SurfaceDimensions,
    ) -> Result<Self, RetainedPaintError> {
        let mut plan = Self {
            recipes: HashMap::new(),
            ordered_active: Vec::new(),
            skipped_by_clip_owner: HashMap::new(),
            raw_fixed: HashMap::new(),
            raw_viewport_fixed_count: 0,
            raw_sticky: HashSet::new(),
            viewport: view.viewport,
            scroll: (
                coverage.document_x,
                coverage.sample_y[coverage.current_index],
            ),
            source_bytes: 0,
            scratch_bytes: 0,
            metadata_bytes: 0,
        };
        let context = PaintContext {
            viewport: view.viewport,
            text_fonts: view.fonts,
            web_fonts: view.web_fonts,
            nested_documents: Some(view.nested_documents),
            nested_snapshot_draw_calls: None,
            propagated_background_node: None,
            shadow_masks: Some(view.shadow_masks),
            retained_collector: None,
            retained_destination: PaintDestination::Temporary,
            effect_sources: None,
            effect_source_generation: 0,
            effect_source_nodes: None,
            effect_source_summary: None,
        };
        let mut ancestor_trace =
            FilterAncestorTrace::new(std::env::var_os("OMOIKANE_TRACE_RETAINED_FILTER").is_some());
        let mut ancestor_clips = OwnedOverflowClipChain::default();
        plan.walk(
            view.raw_layout,
            resolver,
            context,
            ScanState::default(),
            sticky,
            InvariantOverflowClip::raw(view.viewport, coverage),
            coverage,
            0,
            &mut ancestor_trace,
            &mut ancestor_clips,
            false,
            PhaseClipRisk::default(),
            None,
        )?;
        plan.bind_paint_order(view.raw_layout, resolver, sticky, coverage)?;
        plan.raw_fixed = HashMap::new();
        plan.raw_viewport_fixed_count = 0;
        plan.raw_sticky = HashSet::new();
        plan.index_skipped_targets()?;
        if !plan.recipes.is_empty()
            && (view.viewport.width.fract() != 0.0 || view.viewport.height.fract() != 0.0)
        {
            return filter_reject(
                view.raw_layout,
                "fractional_viewport_extent",
                plan.recipes.len(),
            );
        }
        for recipe in plan.recipes.values().filter(|recipe| recipe.active) {
            plan.scratch_bytes = plan.scratch_bytes.max(recipe.scratch(dimensions)?);
            for &y in &coverage.sample_y {
                if let Some(rect) =
                    recipe.buffer(view.viewport, (coverage.document_x as f64, y as f64))
                {
                    plan.source_bytes = plan
                        .source_bytes
                        .checked_add(rect.bytes()?)
                        .ok_or(RetainedPaintError::Capacity)?;
                }
            }
        }
        plan.metadata_bytes = plan.owned_metadata_bytes(coverage.sample_y.len())?;
        Ok(plan)
    }

    fn owned_metadata_bytes(&self, samples: usize) -> Result<usize, RetainedPaintError> {
        let fail = || RetainedPaintError::Capacity;
        let buckets = self
            .recipes
            .capacity()
            .checked_mul(2)
            .and_then(|count| {
                count.checked_mul(
                    std::mem::size_of::<FilterRecipe>() + std::mem::size_of::<usize>() + 16,
                )
            })
            .ok_or_else(fail)?;
        let bytes = self
            .recipes
            .values()
            .try_fold(buckets, |sum, recipe| {
                let instructions = recipe
                    .filters
                    .capacity()
                    .checked_mul(std::mem::size_of::<crate::css::FilterFunction>())?;
                sum.checked_add(instructions)?
                    .checked_add(recipe.geometry.heap_bytes()?)?
                    .checked_add(recipe.clips.heap_bytes()?)
            })
            .ok_or_else(fail)?;
        let ordered = self
            .ordered_active
            .capacity()
            .checked_mul(std::mem::size_of::<usize>())
            .ok_or_else(fail)?;
        let skipped_buckets = self
            .skipped_by_clip_owner
            .capacity()
            .checked_mul(2)
            .and_then(|count| {
                count.checked_mul(
                    std::mem::size_of::<usize>() + std::mem::size_of::<Vec<usize>>() + 16,
                )
            })
            .ok_or_else(fail)?;
        let skipped_targets = self
            .skipped_by_clip_owner
            .values()
            .try_fold(0usize, |sum, targets| {
                targets
                    .capacity()
                    .checked_mul(std::mem::size_of::<usize>())
                    .and_then(|bytes| sum.checked_add(bytes))
            })
            .ok_or_else(fail)?;
        let coverage = self
            .active_count()
            .checked_mul(samples)
            .and_then(|count| count.checked_mul(std::mem::size_of::<SourceSample>()))
            .ok_or_else(fail)?;
        // Raw plan, per-sample view/collector plans, retained recipe and current
        // candidate can coexist. Six copies conservatively cover Vec capacities.
        bytes
            .checked_add(ordered)
            .and_then(|bytes| bytes.checked_add(skipped_buckets))
            .and_then(|bytes| bytes.checked_add(skipped_targets))
            .and_then(|bytes| bytes.checked_add(coverage))
            .and_then(|bytes| bytes.checked_mul(6))
            .ok_or_else(fail)
    }

    pub(super) fn diagnose_capacity(&self, phase: &str, runs: usize, bytes: usize) {
        if std::env::var_os("OMOIKANE_TRACE_RETAINED_FILTER").is_some() {
            eprintln!(
                "{}",
                crate::diagnostics::json_record(
                    "OMOIKANE_RETAINED_SOURCE_FILTER",
                    &serde_json::json!({"reason":"capacity","phase":phase,"groups":self.recipes.len(),
                    "active_groups":self.active_count(),"runs":runs,"retained_bytes":bytes,
                    "source_bytes":self.source_bytes,"scratch_bytes":self.scratch_bytes,"metadata_bytes":self.metadata_bytes})
                )
            );
        }
    }

    pub(super) fn active_count(&self) -> usize {
        self.ordered_active.len()
    }

    pub(super) fn permits(&self, layout: &LayoutBox) -> bool {
        self.recipes.contains_key(&layout.node.identity())
    }

    fn recipe(&self, layout: &LayoutBox) -> Option<&FilterRecipe> {
        self.recipes
            .get(&layout.node.identity())
            .filter(|recipe| recipe.active)
    }

    fn invocation_matches(
        &self,
        recipe: &FilterRecipe,
        paint_owner_node_id: Option<usize>,
        inherited_clip: Option<Rect>,
        paint_offset: PaintOffset,
        atomic_inline: Option<AtomicPaintInvocation>,
    ) -> bool {
        recipe.paint_owner_node_id == paint_owner_node_id
            && recipe.paint_ordinal.is_some()
            && recipe.paint_offset == Some(PaintOffsetKey::of(paint_offset))
            && recipe.atomic_inline == atomic_inline
            && recipe
                .clips
                .replay(self.viewport, (self.scroll.0 as f64, self.scroll.1 as f64))
                == Some(inherited_clip.unwrap_or(self.viewport))
    }

    fn active_node(&self, index: usize) -> Option<usize> {
        self.ordered_active.get(index).copied()
    }

    fn bind_paint_order(
        &mut self,
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        sticky: &StickyPlan,
        coverage: &ResolvedVerticalOverscanPlan,
    ) -> Result<(), RetainedPaintError> {
        self.ordered_active.clear();
        for recipe in self.recipes.values_mut() {
            recipe.clips = OwnedOverflowClipChain::default();
            recipe.paint_owner_node_id = None;
            recipe.paint_ordinal = None;
            recipe.paint_offset = None;
        }
        let mut clips = OwnedOverflowClipChain::default();
        let mut ordinal = 0usize;
        self.bind_paint_box(
            layout,
            resolver,
            sticky,
            InvariantOverflowClip::raw(self.viewport, coverage),
            &mut clips,
            None,
            None,
            PaintOffset::default(),
            true,
            PhaseClipRisk::default(),
            0,
            &mut ordinal,
        )?;
        self.validate_paint_bindings(ordinal)
    }

    #[allow(clippy::too_many_arguments)]
    fn bind_paint_box(
        &mut self,
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        sticky: &StickyPlan,
        invariant_clip: InvariantOverflowClip,
        clips: &mut OwnedOverflowClipChain,
        paint_owner_node_id: Option<usize>,
        atomic_inline: Option<AtomicPaintInvocation>,
        paint_offset: PaintOffset,
        include_phase_descendants: bool,
        phase_risk: PhaseClipRisk,
        depth: usize,
        ordinal: &mut usize,
    ) -> Result<(), RetainedPaintError> {
        if layout.node.top_layer_order().is_some() {
            return Err(RetainedPaintError::TopLayer);
        }
        if layout.visibility == Visibility::Hidden {
            return Ok(());
        }
        if depth >= 128 {
            return filter_reject(layout, "paint_binding_depth", self.recipes.len());
        }
        let style = paint_box_style(layout, resolver);
        let phase_risk = phase_risk.extend(layout, &style);
        let planned = self.recipes.contains_key(&layout.node.identity());
        let (sticky_positioned, sticky_space) =
            self.validate_invocation_classifiers(layout, resolver, sticky, &style)?;
        let fixed_candidate = is_fixed_for_paint(&style)
            || (sticky_positioned && sticky_space == Some(StickySpace::Viewport));
        let address = LayoutAddress::of(layout);
        if has_viewport_edge_filter(&style) && phase_risk.unproven() {
            return phase_clip_reject(layout, self.recipes.len(), phase_risk);
        }
        if has_viewport_edge_filter(&style) && !planned {
            return filter_reject(layout, "unplanned_paint_invocation", self.recipes.len());
        }
        let fixed_space = self.raw_fixed_space(address, fixed_candidate)?;
        if fixed_space == Some(RawFixedSpace::Viewport) {
            return self.reject_filter_descendants(
                layout,
                resolver,
                sticky,
                "fixed_filter_descendant",
                true,
            );
        }
        if !layout.block_fragments.is_empty()
            || layout.transform != AffineTransform::identity()
            || has_transform_style(&style)
            || sticky_positioned
            || layout.node.scroll_offset() != (0.0, 0.0)
        {
            if planned {
                return filter_reject(layout, "paint_owner_barrier", self.recipes.len());
            }
            return self.reject_filter_descendants(
                layout,
                resolver,
                sticky,
                "barrier_filter_descendant",
                false,
            );
        }
        let intercepted = self.bind_recipe(
            layout,
            paint_owner_node_id,
            atomic_inline,
            paint_offset,
            clips,
            ordinal,
        )?;
        if intercepted || layout.content_visibility_contents_skipped {
            return Ok(());
        }
        if inline_fragments_have_viewport_edge_filter(layout, resolver) {
            return filter_reject(
                layout,
                "unplanned_inline_paint_invocation",
                self.recipes.len(),
            );
        }
        if element_opacity(&style).is_some_and(|value| value < 1.0)
            || has_non_none_value(&style, PropertyId::Filter)
            || has_non_none_layer(&style, PropertyId::MaskImage)
            || has_non_none_value(&style, PropertyId::BackdropFilter)
            || has_non_none_value(&style, PropertyId::ClipPath)
            || crate::layout::has_containment(&style, "paint")
        {
            return self.reject_filter_descendants(
                layout,
                resolver,
                sticky,
                "effect_filter_descendant",
                false,
            );
        }
        let checkpoint = clips.checkpoint();
        let overflow_is_invariant =
            layout.overflow.clips_overflow() && invariant_clip.covers_viewport(layout, &style);
        if layout.overflow.clips_overflow() && !overflow_is_invariant {
            clips.push(layout);
        }
        self.bind_paint_children(
            layout,
            resolver,
            sticky,
            invariant_clip,
            clips,
            atomic_inline,
            paint_offset,
            include_phase_descendants,
            phase_risk,
            depth,
            ordinal,
        )?;
        clips.truncate(checkpoint);
        Ok(())
    }

    fn validate_invocation_classifiers(
        &self,
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        sticky: &StickyPlan,
        style: &ComputedStyle,
    ) -> Result<(bool, Option<StickySpace>), RetainedPaintError> {
        if has_fixed_background(style) {
            return Err(RetainedPaintError::FixedBackground);
        }
        if has_non_none_value(style, PropertyId::BackdropFilter) {
            return Err(RetainedPaintError::BackdropFilter);
        }
        scan_inline_fragment_styles(layout, resolver)?;
        let sticky_positioned = is_sticky_for_paint(style);
        let sticky_space = sticky.space(layout);
        if sticky_positioned
            && (sticky_space.is_none() || !self.raw_sticky.contains(&LayoutAddress::of(layout)))
        {
            return Err(RetainedPaintError::StickyPosition);
        }
        Ok((sticky_positioned, sticky_space))
    }

    fn record_raw_fixed_classification(
        &mut self,
        address: LayoutAddress,
        space: RawFixedSpace,
    ) -> Result<(), RetainedPaintError> {
        if self.raw_fixed.len() >= MAX_RAW_FIXED_CLASSIFICATIONS
            || (space == RawFixedSpace::Viewport
                && self.raw_viewport_fixed_count >= MAX_VIEWPORT_FIXED_ROOTS)
        {
            return Err(RetainedPaintError::Capacity);
        }
        if self.raw_fixed.contains_key(&address) {
            return Err(RetainedPaintError::TraversalMismatch);
        }
        self.raw_fixed.insert(address, space);
        if space == RawFixedSpace::Viewport {
            self.raw_viewport_fixed_count = self
                .raw_viewport_fixed_count
                .checked_add(1)
                .ok_or(RetainedPaintError::Capacity)?;
        }
        Ok(())
    }

    fn raw_fixed_space(
        &self,
        address: LayoutAddress,
        fixed_candidate: bool,
    ) -> Result<Option<RawFixedSpace>, RetainedPaintError> {
        match (fixed_candidate, self.raw_fixed.get(&address).copied()) {
            (false, None) => Ok(None),
            (true, Some(space)) => Ok(Some(space)),
            _ => Err(RetainedPaintError::TraversalMismatch),
        }
    }

    fn reject_filter_descendants(
        &self,
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        sticky: &StickyPlan,
        reason: &str,
        inside_viewport_fixed: bool,
    ) -> Result<(), RetainedPaintError> {
        if inline_fragments_have_viewport_edge_filter(layout, resolver) {
            return filter_reject(layout, reason, self.recipes.len());
        }
        let atomic_selection = AtomicInlineSelection::build(layout)
            .map_err(|()| RetainedPaintError::TraversalMismatch)?;
        let mut budget = 1024usize;
        for child in &layout.children {
            if atomic_selection.classify(child) == AtomicInlineChildPaint::Skipped {
                continue;
            }
            self.reject_filter_subtree(
                child,
                resolver,
                sticky,
                reason,
                inside_viewport_fixed,
                0,
                &mut budget,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn reject_filter_subtree(
        &self,
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        sticky: &StickyPlan,
        reason: &str,
        inside_viewport_fixed: bool,
        depth: usize,
        budget: &mut usize,
    ) -> Result<(), RetainedPaintError> {
        if depth >= 128 || *budget == 0 {
            return filter_reject(layout, "barrier_filter_scan_limit", self.recipes.len());
        }
        *budget -= 1;
        if layout.node.top_layer_order().is_some() {
            return Err(RetainedPaintError::TopLayer);
        }
        let style = paint_box_style(layout, resolver);
        if has_fixed_background(&style) {
            return Err(RetainedPaintError::FixedBackground);
        }
        if has_non_none_value(&style, PropertyId::BackdropFilter) {
            return Err(RetainedPaintError::BackdropFilter);
        }
        scan_inline_fragment_styles(layout, resolver)?;
        let address = LayoutAddress::of(layout);
        let fixed_candidate = is_fixed_for_paint(&style)
            || (is_sticky_for_paint(&style) && sticky.space(layout) == Some(StickySpace::Viewport));
        let viewport_fixed = !inside_viewport_fixed
            && self.raw_fixed_space(address, fixed_candidate)? == Some(RawFixedSpace::Viewport);
        if is_sticky_for_paint(&style)
            && (sticky.space(layout).is_none() || !self.raw_sticky.contains(&address))
        {
            return Err(RetainedPaintError::StickyPosition);
        }
        if has_viewport_edge_filter(&style)
            || inline_fragments_have_viewport_edge_filter(layout, resolver)
        {
            return filter_reject(layout, reason, self.recipes.len());
        }
        let atomic_selection = AtomicInlineSelection::build(layout)
            .map_err(|()| RetainedPaintError::TraversalMismatch)?;
        for child in &layout.children {
            if atomic_selection.classify(child) == AtomicInlineChildPaint::Skipped {
                continue;
            }
            self.reject_filter_subtree(
                child,
                resolver,
                sticky,
                reason,
                inside_viewport_fixed || viewport_fixed,
                depth + 1,
                budget,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn bind_paint_children(
        &mut self,
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        sticky: &StickyPlan,
        invariant_clip: InvariantOverflowClip,
        clips: &mut OwnedOverflowClipChain,
        atomic_inline: Option<AtomicPaintInvocation>,
        paint_offset: PaintOffset,
        include_phase_descendants: bool,
        phase_risk: PhaseClipRisk,
        depth: usize,
        ordinal: &mut usize,
    ) -> Result<(), RetainedPaintError> {
        let atomic_selection = AtomicInlineSelection::build(layout)
            .map_err(|()| RetainedPaintError::TraversalMismatch)?;
        let (negative, normal, floats, inline, auto, positive) =
            partition_children_for_paint(layout, resolver, include_phase_descendants);
        let owner = Some(layout.node.identity());
        for (children, recurse_phases) in [(negative, true), (normal, false), (floats, true)] {
            self.bind_paint_group(
                children,
                resolver,
                sticky,
                invariant_clip,
                clips,
                owner,
                atomic_inline,
                paint_offset,
                recurse_phases,
                phase_risk,
                depth,
                ordinal,
            )?;
        }
        self.bind_atomic_inline_phase(
            layout,
            &atomic_selection,
            resolver,
            sticky,
            invariant_clip,
            clips,
            paint_offset,
            phase_risk.for_atomic_child(LayoutAddress::of(layout), atomic_inline.is_some()),
            depth,
            ordinal,
        )?;
        for (children, recurse_phases) in [(inline, false), (auto, true), (positive, true)] {
            self.bind_paint_group(
                children,
                resolver,
                sticky,
                invariant_clip,
                clips,
                owner,
                atomic_inline,
                paint_offset,
                recurse_phases,
                phase_risk,
                depth,
                ordinal,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn bind_paint_group(
        &mut self,
        children: Vec<&LayoutBox>,
        resolver: &mut StyleResolver,
        sticky: &StickyPlan,
        invariant_clip: InvariantOverflowClip,
        clips: &mut OwnedOverflowClipChain,
        owner: Option<usize>,
        atomic_inline: Option<AtomicPaintInvocation>,
        paint_offset: PaintOffset,
        include_phase_descendants: bool,
        phase_risk: PhaseClipRisk,
        depth: usize,
        ordinal: &mut usize,
    ) -> Result<(), RetainedPaintError> {
        for child in children {
            self.bind_paint_box(
                child,
                resolver,
                sticky,
                invariant_clip,
                clips,
                owner,
                atomic_inline,
                paint_offset,
                include_phase_descendants,
                phase_risk,
                depth + 1,
                ordinal,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn bind_atomic_inline_phase(
        &mut self,
        layout: &LayoutBox,
        selection: &AtomicInlineSelection,
        resolver: &mut StyleResolver,
        sticky: &StickyPlan,
        invariant_clip: InvariantOverflowClip,
        clips: &mut OwnedOverflowClipChain,
        paint_offset: PaintOffset,
        phase_risk: PhaseClipRisk,
        depth: usize,
        ordinal: &mut usize,
    ) -> Result<(), RetainedPaintError> {
        for selected in selection.selected().iter().copied() {
            let child = selection
                .selected_child(layout, selected)
                .map_err(|()| RetainedPaintError::TraversalMismatch)?;
            self.bind_paint_box(
                child,
                resolver,
                sticky,
                invariant_clip,
                clips,
                Some(layout.node.identity()),
                Some(selected.invocation),
                paint_offset,
                true,
                phase_risk,
                depth + 1,
                ordinal,
            )?;
        }
        Ok(())
    }

    fn bind_recipe(
        &mut self,
        layout: &LayoutBox,
        paint_owner_node_id: Option<usize>,
        atomic_inline: Option<AtomicPaintInvocation>,
        paint_offset: PaintOffset,
        clips: &OwnedOverflowClipChain,
        ordinal: &mut usize,
    ) -> Result<bool, RetainedPaintError> {
        if !self.recipes.contains_key(&layout.node.identity()) {
            return Ok(false);
        }
        if clips.capacity_exceeded() {
            return Err(RetainedPaintError::Capacity);
        }
        if clips.unrepresentable() {
            return filter_reject(layout, "unrepresentable_effective_clip", self.recipes.len());
        }
        let recipe = self
            .recipes
            .get_mut(&layout.node.identity())
            .ok_or(RetainedPaintError::TraversalMismatch)?;
        if recipe.atomic_inline != atomic_inline
            || recipe.paint_owner_node_id.is_some()
            || recipe.paint_ordinal.is_some()
            || recipe.paint_offset.is_some()
        {
            return Err(RetainedPaintError::TraversalMismatch);
        }
        recipe.clips = clips.clone();
        recipe.paint_owner_node_id = paint_owner_node_id;
        recipe.paint_ordinal = Some(*ordinal);
        recipe.paint_offset = Some(PaintOffsetKey::of(paint_offset));
        *ordinal = (*ordinal)
            .checked_add(1)
            .ok_or(RetainedPaintError::Capacity)?;
        if recipe.active {
            self.ordered_active.push(recipe.node);
        }
        Ok(recipe.active)
    }

    fn validate_paint_bindings(&self, ordinal: usize) -> Result<(), RetainedPaintError> {
        let mut ordinals = self
            .recipes
            .values()
            .map(|recipe| recipe.paint_ordinal)
            .collect::<Option<Vec<_>>>()
            .ok_or(RetainedPaintError::TraversalMismatch)?;
        ordinals.sort_unstable();
        if ordinal != self.recipes.len()
            || ordinals != (0..self.recipes.len()).collect::<Vec<_>>()
            || self
                .recipes
                .values()
                .any(|recipe| recipe.paint_offset.is_none())
            || self.ordered_active.len()
                != self.recipes.values().filter(|recipe| recipe.active).count()
            || self.ordered_active.windows(2).any(|nodes| {
                self.recipes[&nodes[0]].paint_ordinal >= self.recipes[&nodes[1]].paint_ordinal
            })
        {
            return Err(RetainedPaintError::TraversalMismatch);
        }
        Ok(())
    }

    fn index_skipped_targets(&mut self) -> Result<(), RetainedPaintError> {
        self.skipped_by_clip_owner.clear();
        let mut unique = HashSet::new();
        if self.ordered_active.len() > MAX_FILTER_SOURCE_GROUPS
            || self.ordered_active.len()
                != self.recipes.values().filter(|recipe| recipe.active).count()
            || self.ordered_active.iter().any(|node| {
                !unique.insert(*node)
                    || self
                        .recipes
                        .get(node)
                        .is_none_or(|recipe| !recipe.active || recipe.paint_ordinal.is_none())
            })
        {
            return Err(RetainedPaintError::TraversalMismatch);
        }
        let mut associations = 0usize;
        for &target in &self.ordered_active {
            let recipe = self
                .recipes
                .get(&target)
                .ok_or(RetainedPaintError::TraversalMismatch)?;
            for owner in recipe.clips.owner_node_ids() {
                associations = associations
                    .checked_add(1)
                    .ok_or(RetainedPaintError::Capacity)?;
                if associations > MAX_FILTER_CLIP_ASSOCIATIONS {
                    return Err(RetainedPaintError::Capacity);
                }
                let targets = self.skipped_by_clip_owner.entry(owner).or_default();
                if targets.contains(&target) {
                    return Err(RetainedPaintError::TraversalMismatch);
                }
                targets.push(target);
            }
        }
        Ok(())
    }

    fn skipped_target_count(&self, owner: usize) -> usize {
        self.skipped_by_clip_owner.get(&owner).map_or(0, Vec::len)
    }

    fn skipped_recipe(&self, owner: usize, index: usize) -> Option<&FilterRecipe> {
        let target = *self.skipped_by_clip_owner.get(&owner)?.get(index)?;
        self.recipes.get(&target).filter(|recipe| recipe.active)
    }

    pub(super) fn inactive(&self) -> Vec<FilterRecipe> {
        self.recipes
            .values()
            .filter(|recipe| !recipe.active)
            .cloned()
            .collect()
    }

    pub(super) fn at_scroll(&self, scroll: (i32, i32)) -> Self {
        let mut result = self.clone();
        result.scroll = scroll;
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(
        &mut self,
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        context: PaintContext<'_>,
        state: ScanState,
        sticky: &StickyPlan,
        clip: InvariantOverflowClip,
        coverage: &ResolvedVerticalOverscanPlan,
        depth: usize,
        ancestor_trace: &mut FilterAncestorTrace,
        ancestor_clips: &mut OwnedOverflowClipChain,
        unsupported_clip_ancestor: bool,
        phase_clip_risk: PhaseClipRisk,
        atomic_inline: Option<AtomicPaintInvocation>,
    ) -> Result<(), RetainedPaintError> {
        if layout.visibility == Visibility::Hidden {
            return Ok(());
        }
        if depth >= 128 {
            return filter_reject(layout, "depth", self.recipes.len());
        }
        let style = paint_box_style(layout, resolver);
        let sticky_positioned = is_sticky_for_paint(&style);
        if sticky_positioned {
            if self.raw_sticky.len() >= MAX_RAW_STICKY_BOXES
                || !self.raw_sticky.insert(LayoutAddress::of(layout))
            {
                return Err(RetainedPaintError::StickyPosition);
            }
        }
        let phase_clip_risk = if ancestor_clips.is_empty() {
            phase_clip_risk
        } else {
            phase_clip_risk.extend(layout, &style)
        };
        let fixed_candidate = is_fixed_for_paint(&style)
            || (sticky_positioned && sticky.space(layout) == Some(StickySpace::Viewport));
        let fixed =
            fixed_candidate && !state.fixed_containing_ancestor && !state.inside_viewport_fixed;
        if fixed {
            self.record_raw_fixed_classification(
                LayoutAddress::of(layout),
                RawFixedSpace::Viewport,
            )?;
        } else if fixed_candidate && !state.inside_viewport_fixed {
            self.record_raw_fixed_classification(
                LayoutAddress::of(layout),
                RawFixedSpace::Document,
            )?;
        }
        if has_viewport_edge_filter(&style) && !fixed && !state.inside_viewport_fixed {
            if phase_clip_risk.unproven() {
                ancestor_trace.report(layout.node.identity(), state);
                return phase_clip_reject(layout, self.recipes.len(), phase_clip_risk);
            }
            if state.effect_ancestor || state.clip_geometry_unproven || unsupported_clip_ancestor {
                ancestor_trace.report(layout.node.identity(), state);
                return filter_reject(layout, "ancestor_effect_or_clip", self.recipes.len());
            }
            let mut source_budget = 1024usize;
            plain_source(layout, resolver, 0, &mut source_budget)?;
            let mut budget = 1024;
            let geometry = BoundsNode::capture(layout, resolver, &mut budget, 0)?;
            let bounds = subtree_paint_bounds(layout, resolver, context);
            if geometry.at_scroll((0.0, 0.0)) != bounds {
                return filter_reject(layout, "raw_bounds_replay", self.recipes.len());
            }
            if ![
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                bounds.x + bounds.width,
                bounds.y + bounds.height,
            ]
            .into_iter()
            .all(|value| value.is_finite() && value.abs() <= 1_048_576.0)
            {
                return filter_reject(layout, "geometry", self.recipes.len());
            }
            if coverage.sample_y.iter().any(|y| *y > 1_048_576) || coverage.document_x > 1_048_576 {
                return filter_reject(layout, "scroll_geometry_range", self.recipes.len());
            }
            let slack = geometry.rounding_slack();
            let first = coverage.sample_y[0] as f32;
            let end =
                *coverage.sample_y.last().expect("validated samples") as f32 + self.viewport.height;
            let active = !(bounds.y + bounds.height < first - slack
                || bounds.y > end + slack
                || bounds.x + bounds.width < coverage.document_x as f32 - slack
                || bounds.x > coverage.document_x as f32 + self.viewport.width + slack);
            let recipe = FilterRecipe {
                node: layout.node.identity(),
                bounds,
                geometry,
                clips: OwnedOverflowClipChain::default(),
                paint_owner_node_id: None,
                paint_ordinal: None,
                paint_offset: None,
                atomic_inline,
                filters: element_filters(&style),
                opacity: element_opacity(&style).unwrap_or(1.0),
                active,
            };
            if self.recipes.len() >= MAX_FILTER_SOURCE_GROUPS
                || self.recipes.contains_key(&recipe.node)
            {
                return filter_reject(layout, "identity_or_group_limit", self.recipes.len());
            }
            let node = recipe.node;
            self.recipes.insert(node, recipe);
        }
        if layout.content_visibility_contents_skipped {
            return Ok(());
        }
        let child = child_scan_state(layout, &style, state, fixed, Some(clip));
        let unsupported_clip_ancestor = unsupported_clip_ancestor
            || crate::layout::has_containment(&style, "paint")
            || has_non_none_value(&style, PropertyId::ClipPath);
        let clip_checkpoint = ancestor_clips.checkpoint();
        let overflow_is_invariant = layout.overflow.clips_overflow()
            && !child.clip_geometry_unproven
            && clip.covers_viewport(layout, &style);
        let mut child_phase_clip_risk = phase_clip_risk;
        if layout.overflow.clips_overflow() && !overflow_is_invariant {
            ancestor_clips.push(layout);
            child_phase_clip_risk = child_phase_clip_risk.extend_clip_owner_contents(layout);
        }
        let trace_checkpoint = ancestor_trace.checkpoint();
        ancestor_trace.push(layout, &style, child, clip);
        let atomic_selection = AtomicInlineSelection::build(layout)
            .map_err(|()| RetainedPaintError::TraversalMismatch)?;
        for next in &layout.children {
            let (next_phase_risk, next_atomic_inline) = match atomic_selection.classify(next) {
                AtomicInlineChildPaint::Ordinary => (child_phase_clip_risk, atomic_inline),
                AtomicInlineChildPaint::Skipped => continue,
                AtomicInlineChildPaint::Selected(invocation) => (
                    child_phase_clip_risk
                        .for_atomic_child(LayoutAddress::of(layout), atomic_inline.is_some()),
                    Some(invocation),
                ),
            };
            self.walk(
                next,
                resolver,
                context,
                child,
                sticky,
                clip,
                coverage,
                depth + 1,
                ancestor_trace,
                ancestor_clips,
                unsupported_clip_ancestor,
                next_phase_risk,
                next_atomic_inline,
            )?;
        }
        ancestor_trace.truncate(trace_checkpoint);
        ancestor_clips.truncate(clip_checkpoint);
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
struct PhaseClipRisk {
    positioned: bool,
    explicit_z_index: bool,
    floated: bool,
    fixed: bool,
    sticky: bool,
    nested_atomic: bool,
    lines: bool,
    line_owner: Option<LayoutAddress>,
    marker: bool,
    pseudo: bool,
}

impl PhaseClipRisk {
    fn add_line_owner(&mut self, layout: &LayoutBox) {
        if layout.lines.is_empty() {
            return;
        }
        let address = LayoutAddress::of(layout);
        if !self.lines {
            self.lines = true;
            self.line_owner = Some(address);
        } else if self.line_owner != Some(address) {
            self.line_owner = None;
        }
    }

    fn extend(mut self, layout: &LayoutBox, style: &ComputedStyle) -> Self {
        let positioned = is_positioned_for_paint(style);
        self.positioned |= positioned;
        self.explicit_z_index |= has_explicit_z_index(style);
        self.floated |= is_float_for_paint(style);
        self.fixed |= is_fixed_for_paint(style);
        self.sticky |= is_sticky_for_paint(style);
        self.add_line_owner(layout);
        self.marker |= layout.marker.is_some();
        self.pseudo |= layout.pseudo.is_some();
        self
    }

    fn for_atomic_child(mut self, owner: LayoutAddress, nested: bool) -> Self {
        if nested {
            self.nested_atomic = true;
        } else if self.lines && self.line_owner == Some(owner) {
            self.lines = false;
            self.line_owner = None;
        }
        self
    }

    fn unproven(self) -> bool {
        self.explicit_z_index
            || self.floated
            || self.fixed
            || self.sticky
            || self.nested_atomic
            || self.lines
            || self.marker
            || self.pseudo
    }

    fn extend_clip_owner_contents(mut self, layout: &LayoutBox) -> Self {
        // The hook is anchored at this owner's real paint phase, so the
        // owner's own position/float is safe. Keep line risk until an exact
        // selected AtomicInline child proves this owner's text-phase entry.
        self.add_line_owner(layout);
        self.marker |= layout.marker.is_some();
        self.pseudo |= layout.pseudo.is_some();
        self
    }
}

fn has_explicit_z_index(style: &ComputedStyle) -> bool {
    match style.get_by_id(PropertyId::ZIndex) {
        Some(ComputedValue::Number(_)) => true,
        Some(ComputedValue::Keyword(value)) => value.parse::<i32>().is_ok(),
        _ => false,
    }
}

fn bounded_filters(filters: &[crate::css::FilterFunction]) -> bool {
    use crate::css::FilterFunction;
    filters.len() <= 64
        && filters.iter().all(|filter| match filter {
            FilterFunction::Blur(radius) => {
                radius.is_finite() && (0.0..=1_048_576.0).contains(radius)
            }
            FilterFunction::DropShadow {
                offset_x,
                offset_y,
                blur,
                ..
            } => [*offset_x, *offset_y, *blur]
                .into_iter()
                .all(|value| value.is_finite() && value.abs() <= 1_048_576.0),
            _ => true,
        })
}

fn plain_source(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    depth: usize,
    budget: &mut usize,
) -> Result<(), RetainedPaintError> {
    if depth >= 128 || *budget == 0 {
        return filter_reject(layout, "source_subtree_limit", 0);
    }
    *budget -= 1;
    if layout.node.top_layer_order().is_some() {
        return Err(RetainedPaintError::TopLayer);
    }
    let style = paint_box_style(layout, resolver);
    if has_fixed_background(&style) {
        return Err(RetainedPaintError::FixedBackground);
    }
    if has_non_none_value(&style, PropertyId::BackdropFilter) {
        return Err(RetainedPaintError::BackdropFilter);
    }
    scan_inline_fragment_styles(layout, resolver)?;
    if is_fixed_for_paint(&style) {
        return filter_reject(layout, "fixed_source_descendant", 0)
            .map_err(|_| RetainedPaintError::CrossSpaceEffect);
    }
    if !bounded_filters(&element_filters(&style)) {
        return filter_reject(layout, "instruction_capacity", 0);
    }
    // The root element's edge filter is the recipe being validated. Inline
    // fragment filters are independent painter invocations and are rejected
    // unconditionally below, including on this root.
    let nested_viewport_edge_filter = depth != 0 && has_viewport_edge_filter(&style);
    if !layout.block_fragments.is_empty()
        || layout.transform != AffineTransform::identity()
        || has_transform_style(&style)
        || is_fixed_for_paint(&style)
        || is_sticky_for_paint(&style)
        || layout.node.scroll_offset() != (0.0, 0.0)
        || has_non_none_value(&style, PropertyId::ClipPath)
        || has_non_none_layer(&style, PropertyId::MaskImage)
        || crate::layout::has_containment(&style, "paint")
        || nested_viewport_edge_filter
    {
        return filter_reject(layout, "unsupported_source_subtree", 0);
    }
    if inline_fragments_have_viewport_edge_filter(layout, resolver) {
        return filter_reject(layout, "inline_source_filter", 0);
    }
    let atomic_selection =
        AtomicInlineSelection::build(layout).map_err(|()| RetainedPaintError::TraversalMismatch)?;
    for child in &layout.children {
        if atomic_selection.classify(child) == AtomicInlineChildPaint::Skipped {
            continue;
        }
        plain_source(child, resolver, depth + 1, budget)?;
    }
    Ok(())
}

fn filter_reject<T>(
    layout: &LayoutBox,
    reason: &str,
    groups: usize,
) -> Result<T, RetainedPaintError> {
    if std::env::var_os("OMOIKANE_TRACE_RETAINED_FILTER").is_some() {
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_RETAINED_SOURCE_FILTER",
                &serde_json::json!({"node_id":layout.node.identity(),"tag":layout.node.tag_name()
                .map(|tag|tag.chars().take(48).collect::<String>()),"reason":reason,"groups":groups})
            )
        );
    }
    Err(RetainedPaintError::ViewportEdgeFilter)
}

fn phase_clip_reject<T>(
    layout: &LayoutBox,
    groups: usize,
    risk: PhaseClipRisk,
) -> Result<T, RetainedPaintError> {
    if std::env::var_os("OMOIKANE_TRACE_RETAINED_FILTER").is_some() {
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_RETAINED_SOURCE_FILTER",
                &serde_json::json!({
                    "node_id":layout.node.identity(),
                    "tag":layout.node.tag_name().map(|tag|tag.chars().take(48).collect::<String>()),
                    "reason":"phase_clip_unproven","groups":groups,
                    "positioned":risk.positioned,"explicit_z_index":risk.explicit_z_index,
                    "float":risk.floated,"fixed":risk.fixed,"sticky":risk.sticky,
                    "nested_atomic":risk.nested_atomic,"lines":risk.lines,
                    "marker":risk.marker,"pseudo":risk.pseudo,
                })
            )
        );
    }
    Err(RetainedPaintError::ViewportEdgeFilter)
}

struct SparsePixels {
    origin: (i32, i32),
    image: Image,
}
struct SourceSample {
    coverage: PixelRect,
    pixels: Option<SparsePixels>,
}

pub(super) struct FilterSourceLayer {
    recipe: FilterRecipe,
    viewport: Rect,
    samples: Vec<SourceSample>,
}

impl FilterSourceLayer {
    fn capture(
        recipe: FilterRecipe,
        plan: &FilterSourcePlan,
        source: Option<(&Canvas, (i32, i32))>,
    ) -> Result<Self, RetainedPaintError> {
        let expected = recipe.buffer(plan.viewport, (plan.scroll.0 as f64, plan.scroll.1 as f64));
        let pixels = match source {
            Some((canvas, origin)) => {
                if expected
                    != Some(PixelRect {
                        x: origin.0,
                        y: origin.1,
                        width: canvas.width(),
                        height: canvas.height(),
                    })
                {
                    return Err(RetainedPaintError::TraversalMismatch);
                }
                alpha_bounds(canvas).map(|(x0, y0, x1, y1)| SparsePixels {
                    origin: (
                        origin.0 + plan.scroll.0 + x0 as i32,
                        origin.1 + plan.scroll.1 + y0 as i32,
                    ),
                    image: copy_region_out(canvas, x0, y0, x1 - x0, y1 - y0).into_image(),
                })
            }
            None if expected.is_none() => None,
            None => return Err(RetainedPaintError::TraversalMismatch),
        };
        Ok(Self {
            recipe,
            viewport: plan.viewport,
            samples: vec![SourceSample {
                coverage: PixelRect {
                    x: plan.scroll.0,
                    y: plan.scroll.1,
                    width: plan.viewport.width.ceil() as u32,
                    height: plan.viewport.height.ceil() as u32,
                },
                pixels,
            }],
        })
    }

    pub(super) fn append(&mut self, other: Self) -> Result<(), RetainedPaintError> {
        if self.recipe != other.recipe || self.viewport != other.viewport {
            return Err(RetainedPaintError::OverscanRunMismatch);
        }
        self.samples.extend(other.samples);
        Ok(())
    }

    pub(super) fn bytes(&self) -> usize {
        self.samples
            .iter()
            .filter_map(|sample| sample.pixels.as_ref())
            .map(|pixels| {
                pixels.image.pixels.capacity().saturating_add(
                    pixels
                        .image
                        .animation_regions
                        .capacity()
                        .saturating_mul(std::mem::size_of::<animation::AnimationRegion>()),
                )
            })
            .fold(0usize, usize::saturating_add)
    }

    pub(super) fn sample_count(&self) -> usize {
        self.samples.len()
    }

    fn pixel(&self, x: i64, y: i64) -> Option<[u8; 4]> {
        let sample = self
            .samples
            .iter()
            .find(|sample| sample.coverage.contains(x, y))?;
        let Some(pixels) = &sample.pixels else {
            return Some([0; 4]);
        };
        let rect = PixelRect {
            x: pixels.origin.0,
            y: pixels.origin.1,
            width: pixels.image.width(),
            height: pixels.image.height(),
        };
        if !rect.contains(x, y) {
            return Some([0; 4]);
        }
        let index = ((y - i64::from(rect.y)) as usize * rect.width as usize
            + (x - i64::from(rect.x)) as usize)
            * 4;
        Some(
            pixels.image.pixels()[index..index + 4]
                .try_into()
                .expect("RGBA pixel"),
        )
    }

    fn sampled_pixel(&self, x: f64, y: f64) -> Result<[u8; 4], RetainedPaintError> {
        let x0 = x.floor() as i64;
        let y0 = y.floor() as i64;
        let fx = x - x.floor();
        let fy = y - y.floor();
        let x1 = x0 + if fx != 0.0 { 1 } else { 0 };
        let y1 = y0 + if fy != 0.0 { 1 } else { 0 };
        let points = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)];
        let mut samples = [[0; 4]; 4];
        for (sample, (x, y)) in samples.iter_mut().zip(points) {
            *sample = self
                .pixel(x, y)
                .ok_or(RetainedPaintError::IncompleteOverscanCoverage)?;
        }
        Ok(interpolate_overscan_samples(samples, fx, fy))
    }

    pub(super) fn validate(&self, required: OverscanRasterRect) -> Result<(), RetainedPaintError> {
        // Source coverage includes known-transparent pixels, independently of
        // sparse image bounds. Never use the full-document image validator.
        let mut intervals = self
            .samples
            .iter()
            .map(|sample| sample.coverage)
            .collect::<Vec<_>>();
        intervals.sort_by_key(|rect| rect.y);
        let mut end = required.top;
        for rect in intervals {
            if i64::from(rect.x) > required.left
                || i64::from(rect.x) + i64::from(rect.width) < required.right
                || i64::from(rect.y) > end
            {
                continue;
            }
            end = end.max(i64::from(rect.y) + i64::from(rect.height));
        }
        if end < required.bottom {
            return Err(RetainedPaintError::IncompleteOverscanCoverage);
        }
        Ok(())
    }

    pub(super) fn compose_current(&self, destination: &mut Canvas) {
        let coverage = self.samples[0].coverage;
        self.compose(destination, (coverage.x as f64, coverage.y as f64))
            .expect("captured source has validated current-viewport coverage");
    }

    pub(super) fn compose(
        &self,
        destination: &mut Canvas,
        scroll: (f64, f64),
    ) -> Result<(), RetainedPaintError> {
        let Some(rect) = self.recipe.buffer(self.viewport, scroll) else {
            return Ok(());
        };
        let mut source = Canvas::new(rect.width, rect.height);
        let clip = Rect {
            x: 0.0,
            y: 0.0,
            width: rect.width as f32,
            height: rect.height as f32,
        };
        for pixels in self
            .samples
            .iter()
            .filter_map(|sample| sample.pixels.as_ref())
        {
            source.record_animation_regions_from_scaled_image(
                &pixels.image,
                Rect {
                    x: pixels.origin.0 as f32 - scroll.0 as f32 - rect.x as f32,
                    y: pixels.origin.1 as f32 - scroll.1 as f32 - rect.y as f32,
                    width: pixels.image.width() as f32,
                    height: pixels.image.height() as f32,
                },
                clip,
            );
        }
        for y in 0..rect.height {
            for x in 0..rect.width {
                let pixel = self.sampled_pixel(
                    scroll.0 + f64::from(rect.x) + f64::from(x),
                    scroll.1 + f64::from(rect.y) + f64::from(y),
                )?;
                let index = (y as usize * rect.width as usize + x as usize) * 4;
                source.pixels[index..index + 4].copy_from_slice(&pixel);
            }
        }
        apply_filters(&mut source, &self.recipe.filters);
        source.multiply_alpha(self.recipe.opacity);
        blend_source_buffer(destination, &source, rect);
        Ok(())
    }
}

/// Match the canonical effect branch's pixel transfer. `draw_image` performs
/// normalized f32 texture sampling even at 1:1 scale, so it is not equivalent.
fn blend_source_buffer(destination: &mut Canvas, source: &Canvas, rect: PixelRect) {
    destination.record_animation_regions_from_canvas(
        source,
        AffineTransform::translate(rect.x as f32, rect.y as f32),
        None,
    );
    for y in 0..source.height() {
        let dy = i64::from(rect.y) + i64::from(y);
        if dy < 0 || dy >= i64::from(destination.height()) {
            continue;
        }
        for x in 0..source.width() {
            let dx = i64::from(rect.x) + i64::from(x);
            if dx < 0 || dx >= i64::from(destination.width()) {
                continue;
            }
            let index = (y as usize * source.width() as usize + x as usize) * 4;
            let pixel = &source.pixels[index..index + 4];
            destination.blend_pixel(
                dx as u32,
                dy as u32,
                Color::rgba(pixel[0], pixel[1], pixel[2], pixel[3]),
            );
        }
    }
}

pub(super) fn intercept(
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
    let invocation_matches = {
        let state = collector.borrow();
        let Some(plan) = state.plan.filter_sources.as_ref() else {
            return false;
        };
        let Some(recipe) = plan.recipe(layout) else {
            return false;
        };
        plan.invocation_matches(
            recipe,
            options.retained_paint_owner,
            options.inherited_clip,
            options.offset,
            options.retained_atomic_inline,
        )
    };
    if !invocation_matches {
        collector_error(collector, RetainedPaintError::TraversalMismatch);
        return true;
    }
    if !collector.borrow_mut().flush_document(canvas) {
        return true;
    }
    let before = collector.borrow().runs.len();
    paint_box_internal_untransformed(
        canvas,
        layout,
        resolver,
        PaintContext {
            retained_destination: PaintDestination::FilterSource,
            ..context
        },
        options,
    );
    let missing = collector.borrow().runs.len() == before;
    if missing {
        record(collector, layout, None);
    }
    true
}

pub(super) fn capture(
    layout: &LayoutBox,
    context: PaintContext<'_>,
    source: &Canvas,
    origin: (i32, i32),
) -> bool {
    if context.retained_destination != PaintDestination::FilterSource {
        return false;
    }
    if let Some(collector) = context.retained_collector {
        record(collector, layout, Some((source, origin)));
    }
    true
}

pub(super) fn record_skipped(canvas: &mut Canvas, layout: &LayoutBox, context: PaintContext<'_>) {
    if context.retained_destination != PaintDestination::RootDocument {
        return;
    }
    let Some(collector) = context.retained_collector else {
        return;
    };
    let owner = layout.node.identity();
    let count = {
        let state = collector.borrow();
        let Some(plan) = state.plan.filter_sources.as_ref() else {
            return;
        };
        let count = plan.skipped_target_count(owner);
        if count == 0 {
            return;
        }
        let seen = recorded_filter_count(&state.runs);
        let valid = (0..count).all(|index| {
            plan.skipped_recipe(owner, index).is_some_and(|recipe| {
                plan.active_node(seen + index) == Some(recipe.node)
                    && recipe.clips.contains_owner(owner)
                    && recipe
                        .buffer(plan.viewport, (plan.scroll.0 as f64, plan.scroll.1 as f64))
                        .is_none()
                    && !state.runs.iter().any(|run| {
                        run.identity == RetainedPaintRunIdentity::FilterSource(recipe.node)
                    })
            })
        });
        if !valid {
            drop(state);
            collector_error(collector, RetainedPaintError::TraversalMismatch);
            return;
        }
        count
    };
    for index in 0..count {
        let recipe = collector
            .borrow()
            .plan
            .filter_sources
            .as_ref()
            .and_then(|plan| plan.skipped_recipe(owner, index))
            .cloned();
        let Some(recipe) = recipe else {
            collector_error(collector, RetainedPaintError::TraversalMismatch);
            return;
        };
        if !collector.borrow_mut().flush_document(canvas) {
            return;
        }
        record_recipe(collector, recipe, None);
        if collector.borrow().error.is_some() {
            return;
        }
    }
}

fn record(
    collector: &RefCell<RetainedPaintCollector>,
    layout: &LayoutBox,
    source: Option<(&Canvas, (i32, i32))>,
) {
    let recipe = collector
        .borrow()
        .plan
        .filter_sources
        .as_ref()
        .and_then(|plan| plan.recipe(layout))
        .cloned();
    match recipe {
        Some(recipe) => record_recipe(collector, recipe, source),
        None => collector_error(collector, RetainedPaintError::TraversalMismatch),
    }
}

fn record_recipe(
    collector: &RefCell<RetainedPaintCollector>,
    recipe: FilterRecipe,
    source: Option<(&Canvas, (i32, i32))>,
) {
    let mut collector = collector.borrow_mut();
    #[cfg(test)]
    if collector.fail_after_runs == Some(collector.runs.len()) {
        collector.error = Some(RetainedPaintError::CollectionFailed);
        return;
    }
    let seen = recorded_filter_count(&collector.runs);
    if collector
        .plan
        .filter_sources
        .as_ref()
        .and_then(|plan| plan.active_node(seen))
        != Some(recipe.node)
    {
        collector.error = Some(RetainedPaintError::TraversalMismatch);
        return;
    }
    if collector
        .runs
        .iter()
        .any(|run| run.identity == RetainedPaintRunIdentity::FilterSource(recipe.node))
    {
        collector.error = Some(RetainedPaintError::TraversalMismatch);
        return;
    }
    let result = collector
        .plan
        .filter_sources
        .as_ref()
        .map(|plan| FilterSourceLayer::capture(recipe.clone(), plan, source));
    match result {
        Some(Ok(layer)) => {
            let bytes = layer.bytes();
            if collector.runs.len() >= collector.plan.expected_runs
                || collector
                    .retained_bytes
                    .checked_add(bytes)
                    .is_none_or(|size| {
                        !super::super::gradient_tiles::retained_peak_fits(size, MAX_RETAINED_BYTES)
                    })
            {
                collector.error = Some(RetainedPaintError::Capacity);
                return;
            }
            collector.retained_bytes += bytes;
            collector.runs.push(RetainedPaintRun {
                space: RetainedPaintSpace::Document,
                identity: RetainedPaintRunIdentity::FilterSource(recipe.node),
                image: Canvas::new(0, 0).into_image(),
                document_layers: Vec::new(),
                filter_source: Some(layer),
            });
        }
        Some(Err(reason)) => collector.error = Some(reason),
        None => collector.error = Some(RetainedPaintError::TraversalMismatch),
    }
}

fn recorded_filter_count(runs: &[RetainedPaintRun]) -> usize {
    runs.iter()
        .filter(|run| matches!(run.identity, RetainedPaintRunIdentity::FilterSource(_)))
        .count()
}

fn collector_error(collector: &RefCell<RetainedPaintCollector>, reason: RetainedPaintError) {
    let mut collector = collector.borrow_mut();
    if collector.error.is_none() {
        collector.error = Some(reason);
    }
}

#[cfg(test)]
#[path = "filter_source_tests.rs"]
mod tests;
