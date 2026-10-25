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
                    let kind = node.attribute_value_ns(None, "type").unwrap_or_default();
                    let kind = crate::dom::input_value::canonical_type(&kind);
                    let numeric = if matches!(
                        kind,
                        "number" | "range" | "date" | "month" | "week" | "time" | "datetime-local"
                    ) {
                        args.get(2).and_then(JsValue::as_string).and_then(|value| {
                            crate::dom::input_range::NumericInputValue::parse(
                                kind,
                                &value.to_std_string_escaped(),
                            )
                        })
                    } else {
                        None
                    };
                    node.set_control_numeric_value(numeric);
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
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_sanitize_range"),
        5,
        NativeFunction::from_copy_closure(|_, args, context| {
            let value = string_argument(args.first(), "", context)?;
            let min = string_argument(args.get(1), "", context)?;
            let max = string_argument(args.get(2), "", context)?;
            let step = string_argument(args.get(3), "", context)?;
            let default = string_argument(args.get(4), "", context)?;
            let sanitized =
                crate::dom::input_range::sanitize_range(&value, &min, &max, &step, &default);
            if crate::dom::input_range::number("range", &value) == Some(sanitized) {
                Ok(JsValue::from(
                    args.first()
                        .and_then(JsValue::as_string)
                        .expect("internal string")
                        .clone(),
                ))
            } else {
                Ok(JsValue::from(sanitized))
            }
        }),
    )?;
    register_numeric_values(context, bindings)?;
    register_input_flags(context, bindings)
}

fn register_numeric_values(
    context: &mut Context,
    bindings: &mut BootstrapBindings,
) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_input_number"),
        3,
        NativeFunction::from_copy_closure(|_, args, context| {
            let kind = string_argument(args.first(), "", context)?;
            let value = string_argument(args.get(1), "", context)?;
            let attribute = args.get(2).is_some_and(JsValue::to_boolean);
            let result = if attribute {
                crate::dom::input_range::attribute_number(&kind, &value)
            } else {
                crate::dom::input_range::number(&kind, &value)
            };
            Ok(JsValue::from(result.unwrap_or(f64::NAN)))
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_advance_input_number"),
        7,
        NativeFunction::from_copy_closure(|_, args, _| {
            let number = |index: usize| {
                args.get(index)
                    .and_then(JsValue::as_number)
                    .unwrap_or(f64::NAN)
            };
            let range = crate::dom::input_range::StepRange {
                minimum: number(1).is_finite().then(|| number(1)),
                maximum: number(2).is_finite().then(|| number(2)),
                step: number(3),
                base: number(4),
            };
            let value = number(0);
            let result = range.advance(
                if value.is_finite() { value } else { 0.0 },
                number(5) as i32,
                args.get(6).is_some_and(JsValue::to_boolean),
            );
            Ok(JsValue::from(result.unwrap_or(f64::NAN)))
        }),
    )?;
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_input_step_mismatch"),
        4,
        NativeFunction::from_copy_closure(|_, args, _| {
            let number = |index: usize| {
                args.get(index)
                    .and_then(JsValue::as_number)
                    .unwrap_or(f64::NAN)
            };
            Ok(JsValue::from(crate::dom::input_range::step_mismatch(
                number(0),
                number(1),
                number(2),
                number(3) as u64,
            )))
        }),
    )
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
