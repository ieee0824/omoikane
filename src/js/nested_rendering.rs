//! Paint active nested browsing contexts inside their laid-out viewport boxes.

use super::*;

/// Paints one already-laid-out document while optionally counting nested
/// snapshot draw calls for the active attribution sample.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_layout_with_snapshot_attribution(
    diagnostics_enabled: bool,
    layout: &LayoutBox,
    resolver: &mut crate::css::StyleResolver,
    viewport: Rect,
    web_fonts: &crate::font::WebFontRegistry,
    visited_link_ids: impl IntoIterator<Item = usize>,
    nested_documents: &HashMap<usize, crate::paint::Image>,
    shadow_masks: Option<&crate::paint::border::FrameShadowMaskCache>,
) -> (crate::paint::Canvas, crate::paint::PaintRasterSummary) {
    if let Some(shadow_masks) = shadow_masks {
        return crate::paint::paint_layout_with_document_snapshots_cached(
            layout,
            resolver,
            viewport,
            crate::paint::text::load_text_fonts(),
            Some(web_fonts),
            visited_link_ids,
            nested_documents,
            diagnostics_enabled,
            shadow_masks,
        );
    }
    if diagnostics_enabled {
        crate::paint::paint_layout_with_document_snapshots_counted(
            layout,
            resolver,
            viewport,
            crate::paint::text::load_text_fonts(),
            Some(web_fonts),
            visited_link_ids,
            nested_documents,
        )
    } else {
        (
            crate::paint::paint_layout_with_document_snapshots(
                layout,
                resolver,
                viewport,
                crate::paint::text::load_text_fonts(),
                Some(web_fonts),
                visited_link_ids,
                nested_documents,
            ),
            crate::paint::PaintRasterSummary::default(),
        )
    }
}

/// Loads missing child documents and refreshes their style/layout caches to
/// build owned images before starting the parent paint traversal. Each child
/// uses its own viewport, URL, cookie and font context.
pub(super) fn collect_snapshots(
    state: &mut HostState,
    layout: &LayoutBox,
    depth: usize,
    attribution: &mut frame_diagnostics::PaintAttributionScope,
) -> Result<HashMap<usize, crate::paint::Image>, crate::paint::PaintError> {
    let mut snapshots = HashMap::new();
    if depth >= 32 || layout.content_visibility_contents_skipped {
        return Ok(snapshots);
    }
    if matches!(layout.node.tag_name().as_deref(), Some("iframe" | "frame")) {
        let rect = layout.dimensions.content;
        if rect.width > 0.0
            && rect.height > 0.0
            && layout.visibility == crate::layout::Visibility::Visible
        {
            attribution.record_snapshot_candidate();
            if let Some(image) = child_snapshot(state, &layout.node, rect, depth, attribution)? {
                snapshots.insert(layout.node.identity(), image);
            }
        }
    } else {
        for child in &layout.children {
            snapshots.extend(collect_snapshots(state, child, depth, attribution)?);
        }
    }
    Ok(snapshots)
}

fn child_snapshot(
    state: &mut HostState,
    frame: &NodeHandle,
    rect: Rect,
    depth: usize,
    attribution: &mut frame_diagnostics::PaintAttributionScope,
) -> Result<Option<crate::paint::Image>, crate::paint::PaintError> {
    let document = state
        .iframe_content_document(frame)
        .map_err(|_| crate::paint::PaintError::InvalidImageBuffer)?;
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: rect.width,
        height: rect.height,
    };
    let id = document.identity();
    let diagnostics_enabled = attribution.enabled();
    let preparation_style_profile =
        crate::css::style_profile::StyleProfileScope::begin_measurement(diagnostics_enabled, id);
    let Some(mut layout) = build_child_document_layout(state, &document, viewport) else {
        return Ok(None);
    };
    let scroll = state.window_scroll_for_document(id);
    let visited = state.visited_link_ids_for_paint(&document);
    let base = crate::paint::stylesheet::extract_document_base_url(
        &document,
        state.base_url_for_document(id).as_ref(),
    );
    let image_environment = state.image_environment(id);
    let time = state.event_loop.rendering_time_ms() as u64;
    let resolver = state
        .document_styles
        .get_mut(&id)
        .and_then(|entry| entry.resolver.as_mut())
        .ok_or(crate::paint::PaintError::InvalidImageBuffer)?;
    crate::paint::apply_scroll_offsets(&mut layout, resolver, viewport, scroll);
    let preparation_style_profile = preparation_style_profile.finish();
    let children = collect_snapshots(state, &layout, depth + 1, attribution)?;
    let entry = state
        .document_styles
        .get_mut(&id)
        .ok_or(crate::paint::PaintError::InvalidImageBuffer)?;
    let resolver = entry
        .resolver
        .as_mut()
        .ok_or(crate::paint::PaintError::InvalidImageBuffer)?;
    let raster_style_profile =
        crate::css::style_profile::StyleProfileScope::begin_measurement(diagnostics_enabled, id);
    let raster_started = attribution.stage_start();
    let ((canvas, raster_summary), image_reports) = image_environment.run(false, || {
        crate::layout::with_image_base_url(base, || {
            crate::layout::with_image_animation_time(time, || {
                paint_layout_with_snapshot_attribution(
                    diagnostics_enabled,
                    &layout,
                    resolver,
                    viewport,
                    &entry.web_fonts,
                    visited,
                    &children,
                    None,
                )
            })
        })
    });
    let raster_style_profile = raster_style_profile.finish();
    let style_profile = match (preparation_style_profile, raster_style_profile) {
        (Some(preparation), Some(raster)) => Some(preparation.merged(raster)),
        (Some(summary), None) | (None, Some(summary)) => Some(summary),
        (None, None) => None,
    };
    attribution.record_child_raster(id, raster_started, style_profile, raster_summary);
    state.queue_image_reports(id, image_reports);
    Ok(Some(canvas.into_image()))
}

/// Descends through nested viewport boxes for embedder pointer input.
pub(super) fn hit_child(
    state: &mut HostState,
    layout: &LayoutBox,
    target: NodeHandle,
    x: f32,
    y: f32,
    depth: usize,
) -> NodeHandle {
    if depth >= 32 || !matches!(target.tag_name().as_deref(), Some("iframe" | "frame")) {
        return target;
    }
    let Some(owner) = find_layout_box(layout, &target) else {
        return target;
    };
    let rect = owner.dimensions.content;
    let x = x - rect.x;
    let y = y - rect.y;
    if x < 0.0 || y < 0.0 || x >= rect.width || y >= rect.height {
        return target;
    }
    let Ok(document) = state.iframe_content_document(&target) else {
        return target;
    };
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: rect.width,
        height: rect.height,
    };
    let Some(mut child) = build_child_document_layout(state, &document, viewport) else {
        return target;
    };
    let scroll = state.window_scroll_for_document(document.identity());
    let Some(resolver) = state
        .document_styles
        .get_mut(&document.identity())
        .and_then(|entry| entry.resolver.as_mut())
    else {
        return target;
    };
    crate::paint::apply_scroll_offsets(&mut child, resolver, viewport, scroll);
    let Some(hit) = crate::paint::hit_test_layout(&child, resolver, viewport, x, y) else {
        return target;
    };
    hit_child(state, &child, hit, x, y, depth + 1)
}

/// Returns a document's viewport origin in top-level viewport coordinates.
pub(super) fn document_origin(state: &mut HostState, document: &NodeHandle) -> (f64, f64) {
    let mut document = document.clone();
    let mut origin = (0.0, 0.0);
    for _ in 0..32 {
        if document.identity() == state.document.identity() {
            break;
        }
        let owner = state.iframe_documents.iter().find_map(|(id, entry)| {
            (entry.document.identity() == document.identity())
                .then(|| state.get_node(*id))
                .flatten()
        });
        let Some(owner) = owner else {
            break;
        };
        let Some(parent) = owner_document_for_node(&owner) else {
            break;
        };
        let viewport = state.viewport_for_document(&parent);
        let layout = if parent.identity() == state.document.identity() {
            state.ensure_adjusted_layout();
            state
                .adjusted_layout_cache
                .as_ref()
                .map(|cache| cache.root.clone())
        } else {
            let scroll = state.window_scroll_for_document(parent.identity());
            build_child_document_layout(state, &parent, viewport).map(|mut root| {
                if let Some(resolver) = state
                    .document_styles
                    .get_mut(&parent.identity())
                    .and_then(|entry| entry.resolver.as_mut())
                {
                    crate::paint::apply_scroll_offsets(&mut root, resolver, viewport, scroll);
                }
                Rc::new(root)
            })
        };
        if let Some(layout) = layout
            && let Some(owner_box) = find_layout_box(&layout, &owner)
        {
            origin.0 += owner_box.dimensions.content.x as f64;
            origin.1 += owner_box.dimensions.content.y as f64;
        }
        document = parent;
    }
    origin
}
