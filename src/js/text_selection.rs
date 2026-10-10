//! Active-document text hit testing for native pointer selection defaults.

use super::*;

pub(super) fn caret_hit_test_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, document_id)?;
    let x = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as f32;
    let y = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as f32;
    let scope_id = args
        .get(3)
        .filter(|id| !id.is_null_or_undefined())
        .map(|id| id.to_number(context).map(|id| id as usize))
        .transpose()?;
    if let Some(id) = scope_id {
        ensure_same_origin_node(context, id)?;
    }
    let point = with_host_state(|state| {
        Ok(caret_in_state(
            &mut state.borrow_mut(),
            document_id,
            (x, y),
            scope_id,
        ))
    })?;
    let Some(point) = point else {
        return Ok(JsValue::null());
    };
    Ok(boa_engine::object::builtins::JsArray::from_iter(
        [
            JsValue::from(point.node.identity() as f64),
            JsValue::from(point.offset as f64),
        ],
        context,
    )
    .into())
}

fn caret_in_state(
    state: &mut HostState,
    document_id: usize,
    point: (f32, f32),
    scope_id: Option<usize>,
) -> Option<crate::paint::TextCaretPoint> {
    let document = state.get_node(document_id)?;
    if document.node_type() != NodeType::Document
        || !state.document_is_active(document_id)
        || !point.0.is_finite()
        || !point.1.is_finite()
    {
        return None;
    }
    let viewport = state.viewport_for_document(&document);
    if point.0 < 0.0 || point.1 < 0.0 || point.0 > viewport.width || point.1 > viewport.height {
        return None;
    }
    let scope = match scope_id {
        Some(id) => Some(state.get_node(id)?),
        None => None,
    };
    caret_for_document(state, &document, viewport, point, scope.as_ref())
}

fn caret_for_document(
    state: &mut HostState,
    document: &NodeHandle,
    viewport: Rect,
    point: (f32, f32),
    scope: Option<&NodeHandle>,
) -> Option<crate::paint::TextCaretPoint> {
    let id = document.identity();
    if id == state.document.identity() {
        state.ensure_adjusted_layout();
        let layout = &state.adjusted_layout_cache.as_ref()?.root;
        let entry = state.document_styles.get_mut(&id)?;
        return crate::paint::hit_test_caret_layout(
            layout,
            entry.resolver.as_mut()?,
            viewport,
            point,
            scope,
            Some(&entry.web_fonts),
        );
    }
    let mut layout = build_child_document_layout(state, document, viewport)?;
    let scroll = state.window_scroll_for_document(id);
    let entry = state.document_styles.get_mut(&id)?;
    let resolver = entry.resolver.as_mut()?;
    crate::paint::apply_scroll_offsets(&mut layout, resolver, viewport, scroll);
    crate::paint::hit_test_caret_layout(
        &layout,
        resolver,
        viewport,
        point,
        scope,
        Some(&entry.web_fonts),
    )
}
