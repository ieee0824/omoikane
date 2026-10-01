//! Paint active nested browsing contexts inside their laid-out viewport boxes.

use super::*;

/// Loads missing child documents and refreshes their style/layout caches to
/// build owned images before starting the parent paint traversal. Each child
/// uses its own viewport, URL, cookie and font context.
pub(super) fn collect_snapshots(
    state: &mut HostState,
    layout: &LayoutBox,
    depth: usize,
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
            if let Some(image) = child_snapshot(state, &layout.node, rect, depth)? {
                snapshots.insert(layout.node.identity(), image);
            }
        }
    } else {
        for child in &layout.children {
            snapshots.extend(collect_snapshots(state, child, depth)?);
        }
    }
    Ok(snapshots)
}

fn child_snapshot(
    state: &mut HostState,
    frame: &NodeHandle,
    rect: Rect,
    depth: usize,
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
    let Some(mut layout) = build_child_document_layout(state, &document, viewport) else {
        return Ok(None);
    };
    let id = document.identity();
    let scroll = state.window_scroll_for_document(id);
    let visited = state.visited_link_ids_for_paint(&document);
    let base = crate::paint::stylesheet::extract_document_base_url(
        &document,
        state.base_url_for_document(id).as_ref(),
    );
    let cookies = Arc::clone(&state.cookie_store);
    let site = state.location_href.parse::<crate::http::Url>().ok();
    let time = state.event_loop.rendering_time_ms() as u64;
    let resolver = state
        .document_styles
        .get_mut(&id)
        .and_then(|entry| entry.resolver.as_mut())
        .ok_or(crate::paint::PaintError::InvalidImageBuffer)?;
    crate::paint::apply_scroll_offsets(&mut layout, resolver, viewport, scroll);
    let children = collect_snapshots(state, &layout, depth + 1)?;
    let entry = state
        .document_styles
        .get_mut(&id)
        .ok_or(crate::paint::PaintError::InvalidImageBuffer)?;
    let resolver = entry
        .resolver
        .as_mut()
        .ok_or(crate::paint::PaintError::InvalidImageBuffer)?;
    let canvas = crate::layout::with_image_cookie_store(cookies, site, id, || {
        crate::layout::with_image_base_url(base, || {
            crate::layout::with_image_animation_time(time, || {
                crate::paint::paint_layout_with_document_snapshots(
                    &layout,
                    resolver,
                    viewport,
                    crate::paint::text::load_text_fonts(),
                    Some(&entry.web_fonts),
                    visited,
                    &children,
                )
            })
        })
    });
    let (width, height) = (canvas.width(), canvas.height());
    crate::paint::Image::new(width, height, canvas.into_pixels()).map(Some)
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
                root
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
