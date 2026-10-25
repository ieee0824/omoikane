//! Synchronizes owned selector metadata; actual UTF-16 control values stay in JS.

use super::*;

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_set_control_value_empty"),
        2,
        NativeFunction::from_copy_closure(|_, args, context| {
            let id = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, id)?;
            let empty = args.get(1).and_then(JsValue::as_boolean);
            with_host_state(|host| {
                let mut state = host.borrow_mut();
                if let Some(node) = state.get_node(id) {
                    node.set_control_dirty_value_empty(empty);
                    // Every value change can affect validity, even when its
                    // emptiness stays the same. Preserve the existing full
                    // style/validation invalidation for these mutations.
                    state.invalidate_style_cache_for_node(&node);
                }
                Ok(JsValue::undefined())
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_sanitize_input_value"),
        3,
        NativeFunction::from_copy_closure(sanitize_input_value),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_button_type"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let id = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, id)?;
            with_host_state(|host| {
                let node = host
                    .borrow()
                    .get_node(id)
                    .ok_or_else(|| JsNativeError::typ().with_message("Invalid button receiver"))?;
                let kind = crate::dom::form_controls::button_type(&node)
                    .ok_or_else(|| JsNativeError::typ().with_message("Invalid button receiver"))?;
                Ok(js_string!(kind).into())
            })
        }),
    )?;
    register_input_flags(context, bindings)
}

fn register_input_flags(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_input_type"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let node = input_node(args, context)?;
            let kind = node.attribute_value_ns(None, "type").unwrap_or_default();
            Ok(js_string!(crate::dom::input_value::canonical_type(&kind)).into())
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_get_indeterminate"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            Ok(JsValue::from(
                input_node(args, context)?.input_indeterminate(),
            ))
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_set_indeterminate"),
        2,
        NativeFunction::from_copy_closure(|_, args, context| {
            let node = input_node(args, context)?;
            let value = args.get(1).is_some_and(JsValue::to_boolean);
            with_render_mutation(|host| {
                node.set_input_indeterminate(value);
                host.borrow_mut().invalidate_style_cache_for_node(&node);
                Ok(JsValue::undefined())
            })
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_reset_checkedness"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let node = input_node(args, context)?;
            with_render_mutation(|host| {
                node.reset_input_checkedness();
                host.borrow_mut().invalidate_style_cache_for_node(&node);
                Ok(JsValue::undefined())
            })
        }),
    )
}

fn input_node(args: &[JsValue], context: &mut Context) -> JsResult<NodeHandle> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    with_host_state(|host| {
        host.borrow()
            .get_node(id)
            .filter(|node| {
                crate::dom::form_controls::html_local_name(node).as_deref() == Some("input")
            })
            .ok_or_else(|| {
                JsNativeError::typ()
                    .with_message("Invalid input receiver")
                    .into()
            })
    })
}

/// Computes a value without DOM access; unchanged strings keep their JS storage.
fn sanitize_input_value(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    use crate::dom::input_value::{self, Normalization};
    let kind = string_argument(args.first(), "", context)?;
    let value = args
        .get(1)
        .and_then(JsValue::as_string)
        .ok_or_else(|| JsNativeError::typ().with_message("Input value must be a string"))?;
    let multiple = args.get(2).is_some_and(JsValue::to_boolean);
    let units = match input_value::normalize(&kind, multiple, value.iter()) {
        Normalization::Unchanged => return Ok(JsValue::from(value.clone())),
        Normalization::Empty => return Ok(JsValue::from(js_string!(""))),
        Normalization::Rewrite(units) => units,
        Normalization::Filter { start, end } => value
            .iter()
            .enumerate()
            .filter_map(|(index, unit)| {
                (index >= start && index < end && !matches!(unit, 10 | 13)).then_some(unit)
            })
            .collect(),
    };
    Ok(JsValue::from(JsString::from(units.as_slice())))
}
