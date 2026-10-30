//! Natives for the HTML `select` option list and `selectedcontent` updates.
//!
//! Selectedness lives in the native DOM so the parser, layout and script see
//! one state. Tree mutations record the selects whose `selectedcontent`
//! elements must be refreshed; the bootstrap takes them after each mutation
//! and performs the cloning with script-visible DOM operations.

use super::*;
use crate::dom::select::{self, SelectedContentUpdate, SubtreeSelects, TreeMutation};

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_select_options"),
        1,
        NativeFunction::from_fn_ptr(select_options_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_option_nearest_select"),
        1,
        NativeFunction::from_fn_ptr(option_nearest_select_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_selectedcontent_select"),
        1,
        NativeFunction::from_fn_ptr(selectedcontent_select_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_reset_select"),
        1,
        NativeFunction::from_fn_ptr(reset_select_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_option_selected_attribute_changed"),
        1,
        NativeFunction::from_fn_ptr(option_selected_attribute_changed_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_select_only"),
        2,
        NativeFunction::from_fn_ptr(select_only_native),
    )?;
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_take_selectedcontent_updates"),
        0,
        NativeFunction::from_fn_ptr(take_updates_native),
    )
}

fn node_argument(args: &[JsValue], context: &mut Context) -> JsResult<Option<NodeHandle>> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| Ok(state.borrow().get_node(node_id)))
}

/// `__omoikane_select_options(selectId)`: the select's list of options.
fn select_options_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let Some(select) = node_argument(args, context)? else {
        return Ok(JsValue::undefined());
    };
    let ids = select::list_of_options(&select)
        .into_iter()
        .map(|option| JsValue::from(option.identity() as f64));
    Ok(JsArray::from_iter(ids, context).into())
}

/// `__omoikane_option_nearest_select(optionId)`: the nearest ancestor select.
fn option_nearest_select_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node = node_argument(args, context)?;
    Ok(node_to_js_value(
        node.as_ref().and_then(select::nearest_ancestor_select),
    ))
}

/// `__omoikane_selectedcontent_select(id)`: the select an enabled
/// `selectedcontent` element reflects, or null.
fn selectedcontent_select_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node = node_argument(args, context)?;
    Ok(node_to_js_value(
        node.as_ref().and_then(select::selected_content_select),
    ))
}

/// `__omoikane_reset_select(selectId)`: runs the selectedness setting
/// algorithm and returns whether any option changed.
fn reset_select_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_node_render_mutation(&[node_id], |state| {
        let Some(select) = state.borrow().get_node(node_id) else {
            return Ok(JsValue::from(false));
        };
        let changed = select::run_selectedness_setting(&select);
        if changed {
            invalidate_select_options(&mut state.borrow_mut(), &select);
        }
        Ok(JsValue::from(changed))
    })
}

/// `__omoikane_option_selected_attribute_changed(optionId)`: lets a non-dirty
/// option follow its `selected` attribute and records a synchronous update.
fn option_selected_attribute_changed_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_node_render_mutation(&[node_id], |state| {
        let Some(option) = state.borrow().get_node(node_id) else {
            return Ok(JsValue::undefined());
        };
        let mut state = state.borrow_mut();
        state.invalidate_style_cache_for_node(&option);
        if let Some(select) = select::apply_selected_attribute(&option) {
            invalidate_select_options(&mut state, &select);
            record_updates(
                &mut state,
                vec![SelectedContentUpdate {
                    select,
                    deferred: false,
                }],
            );
        }
        Ok(JsValue::undefined())
    })
}

/// `__omoikane_select_only(selectId, optionId | null)`: the `selectedIndex`
/// and `value` setters. Always records a synchronous `selectedcontent` update.
fn select_only_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let select_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, select_id)?;
    let option_value = args.get(1).cloned().unwrap_or_default();
    let option_id = if option_value.is_null_or_undefined() {
        None
    } else {
        let id = parse_node_id(Some(&option_value), context)?;
        ensure_same_origin_node(context, id)?;
        Some(id)
    };
    with_node_render_mutation(&[select_id], |state| {
        let (select, option) = {
            let state = state.borrow();
            (
                state.get_node(select_id),
                option_id.and_then(|id| state.get_node(id)),
            )
        };
        let Some(select) = select else {
            return Ok(JsValue::from(false));
        };
        let changed = select::select_only(&select, option.as_ref());
        let mut state = state.borrow_mut();
        invalidate_select_options(&mut state, &select);
        // Assigning the current value still re-clones the selected option.
        record_updates(
            &mut state,
            vec![SelectedContentUpdate {
                select,
                deferred: false,
            }],
        );
        Ok(JsValue::from(changed))
    })
}

/// `__omoikane_take_selectedcontent_updates()`: drains `[selectId, deferred]`
/// pairs recorded by tree mutations; undefined when there are none.
fn take_updates_native(_: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let updates = with_host_state(|state| {
        Ok(std::mem::take(
            &mut state.borrow_mut().pending_selectedcontent_updates,
        ))
    })?;
    if updates.is_empty() {
        return Ok(JsValue::undefined());
    }
    let pairs: Vec<JsValue> = updates
        .into_iter()
        .map(|(id, deferred)| {
            JsArray::from_iter([JsValue::from(id as f64), JsValue::from(deferred)], context).into()
        })
        .collect();
    Ok(JsArray::from_iter(pairs, context).into())
}

/// Invalidates `:checked` and `:default`-dependent styles of a select's options.
pub(super) fn invalidate_select_options(state: &mut HostState, select: &NodeHandle) {
    state.invalidate_style_cache_for_node(select);
    for option in select::list_of_options(select) {
        state.invalidate_style_cache_for_node(&option);
    }
}

/// Captures a subtree's select relationships before a tree mutation.
pub(super) fn capture(node: &NodeHandle) -> SubtreeSelects {
    SubtreeSelects::capture(node)
}

/// Applies the option and `selectedcontent` mutation steps after a tree
/// mutation and records the selects to refresh.
pub(super) fn finish(state: &mut HostState, captured: SubtreeSelects, mutation: TreeMutation) {
    let outcome = captured.finish(mutation);
    for select in &outcome.reset {
        invalidate_select_options(state, select);
    }
    record_updates(state, outcome.updates);
}

pub(super) fn record_updates(state: &mut HostState, updates: Vec<SelectedContentUpdate>) {
    for update in updates {
        let entry = (update.select.identity(), update.deferred);
        if !state.pending_selectedcontent_updates.contains(&entry) {
            state.pending_selectedcontent_updates.push(entry);
        }
    }
}
