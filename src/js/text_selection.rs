//! Active-document pointer selection and CSSOM text insertion geometry.

use super::*;

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    for (name, length, function) in [
        (
            js_string!("__omoikane_caret_hit_test"),
            4,
            NativeFunction::from_copy_closure(caret_hit_test_native),
        ),
        (
            js_string!("__omoikane_caret_rect"),
            2,
            NativeFunction::from_copy_closure(caret_rect_native),
        ),
        (
            js_string!("__omoikane_text_range_rects"),
            3,
            NativeFunction::from_copy_closure(text_range_rects_native),
        ),
    ] {
        register_private_builtin_callable(context, bindings, name, length, function)?;
    }
    Ok(())
}

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
            args.get(4).is_some_and(JsValue::to_boolean),
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
    include_controls: bool,
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
    caret_for_document(
        state,
        &document,
        viewport,
        point,
        scope.as_ref(),
        include_controls,
    )
}

fn caret_for_document(
    state: &mut HostState,
    document: &NodeHandle,
    viewport: Rect,
    point: (f32, f32),
    scope: Option<&NodeHandle>,
    include_controls: bool,
) -> Option<crate::paint::TextCaretPoint> {
    with_caret_layout(state, document, viewport, |layout, resolver, fonts| {
        crate::paint::hit_test_caret_layout(
            layout,
            resolver,
            viewport,
            point,
            scope,
            Some(fonts),
            include_controls,
        )
    })
}

pub(super) fn caret_rect_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let offset = args.get(1).cloned().unwrap_or_default().to_u32(context)? as usize;
    let rect = with_host_state(|state| {
        let mut state = state.borrow_mut();
        let Some(node) = state.get_node(node_id) else {
            return Ok(None);
        };
        if node.data_utf16().is_some_and(|data| offset > data.len()) {
            return Ok(None);
        }
        let Some(document) = document_root_for_node(&node) else {
            return Ok(None);
        };
        if !state.document_is_active(document.identity()) {
            return Ok(None);
        }
        let viewport = state.viewport_for_document(&document);
        Ok(with_caret_layout(
            &mut state,
            &document,
            viewport,
            |layout, _, fonts| {
                crate::paint::text_caret_rect_layout(
                    layout,
                    &node,
                    offset,
                    AffineTransform::identity(),
                    Some(fonts),
                )
            },
        ))
    })?;
    let Some(rect) = rect else {
        return Ok(JsValue::null());
    };
    Ok(boa_engine::object::builtins::JsArray::from_iter(
        [rect.x, rect.y, rect.width, rect.height].map(|value| JsValue::from(value as f64)),
        context,
    )
    .into())
}

pub(super) fn text_range_rects_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let start = args.get(1).cloned().unwrap_or_default().to_u32(context)? as usize;
    let end = args.get(2).cloned().unwrap_or_default().to_u32(context)? as usize;
    let rects = with_host_state(|state| {
        let mut state = state.borrow_mut();
        let Some(node) = state.get_node(node_id) else {
            return Ok(Vec::new());
        };
        let Some(document) = document_root_for_node(&node) else {
            return Ok(Vec::new());
        };
        if !state.document_is_active(document.identity()) {
            return Ok(Vec::new());
        }
        let viewport = state.viewport_for_document(&document);
        Ok(
            with_caret_layout(&mut state, &document, viewport, |layout, _, fonts| {
                let mut rects = Vec::new();
                if start == end {
                    if let Some(rect) = crate::paint::text_caret_rect_layout(
                        layout,
                        &node,
                        start,
                        AffineTransform::identity(),
                        Some(fonts),
                    ) {
                        rects.push(rect);
                    }
                } else {
                    crate::paint::text_range_rects_layout(
                        layout,
                        &node,
                        start,
                        end,
                        AffineTransform::identity(),
                        Some(fonts),
                        &mut rects,
                    );
                }
                Some(rects)
            })
            .unwrap_or_default(),
        )
    })?;
    let rects = rects
        .into_iter()
        .map(|rect| {
            boa_engine::object::builtins::JsArray::from_iter(
                [rect.x, rect.y, rect.width, rect.height].map(|value| JsValue::from(value as f64)),
                context,
            )
            .into()
        })
        .collect::<Vec<JsValue>>();
    Ok(boa_engine::object::builtins::JsArray::from_iter(rects, context).into())
}

fn with_caret_layout<R>(
    state: &mut HostState,
    document: &NodeHandle,
    viewport: Rect,
    calculate: impl FnOnce(&LayoutBox, &mut StyleResolver, &crate::font::WebFontRegistry) -> Option<R>,
) -> Option<R> {
    let id = document.identity();
    if id == state.document.identity() {
        state.ensure_adjusted_layout();
        let layout = &state.adjusted_layout_cache.as_ref()?.root;
        let entry = state.document_styles.get_mut(&id)?;
        return calculate(layout, entry.resolver.as_mut()?, &entry.web_fonts);
    }
    let mut layout = build_child_document_layout(state, document, viewport)?;
    let scroll = state.window_scroll_for_document(id);
    let entry = state.document_styles.get_mut(&id)?;
    let resolver = entry.resolver.as_mut()?;
    crate::paint::apply_scroll_offsets(&mut layout, resolver, viewport, scroll);
    calculate(&layout, resolver, &entry.web_fonts)
}
