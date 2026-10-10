//! Eager iterator collection and explicit resource disposal.
use super::IteratorRecord;
use crate::{
    Context, JsNativeError, JsResult, JsValue, builtins::Array, js_string, object::JsObject,
};

/// Collects values using a captured next method, retaining yielded objects across GC.
pub(super) fn to_array(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let iterator = this
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("Iterator.toArray requires an object"))?;
    let _iterator_root = iterator.clone().root();
    let next = iterator.get(js_string!("next"), context)?;
    let mut record = IteratorRecord::new(iterator, next);
    let mut values = Vec::new();
    let mut roots = Vec::new();
    while let Some(value) = record.step_value(context)? {
        if let Some(object) = value.as_object() {
            roots.push(JsObject::root(object));
        }
        values.push(value);
    }
    Ok(Array::create_array_from_list(values, context).into())
}

/// Invokes return without interpreting its result, as required by @@dispose.
pub(super) fn dispose(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let method = this.get_v(js_string!("return"), context)?;
    if !method.is_null_or_undefined() {
        let callable = method
            .as_callable()
            .ok_or_else(|| JsNativeError::typ().with_message("iterator return must be callable"))?;
        callable.call(this, &[], context)?;
    }
    Ok(JsValue::undefined())
}

/// The stopping condition for callback consumers; confined to this invocation.
#[derive(Clone, Copy)]
enum Consumer {
    ForEach,
    Some,
    Every,
    Find,
}

pub(super) fn for_each(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    consume(this, args, Consumer::ForEach, context)
}
pub(super) fn some(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    consume(this, args, Consumer::Some, context)
}
pub(super) fn every(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    consume(this, args, Consumer::Every, context)
}
pub(super) fn find(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    consume(this, args, Consumer::Find, context)
}

fn consume(
    this: &JsValue,
    args: &[JsValue],
    mode: Consumer,
    context: &mut Context,
) -> JsResult<JsValue> {
    let iterator = this
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("iterator consumer requires an object"))?;
    let _iterator_root = iterator.clone().root();
    let callback = args.first().and_then(JsValue::as_callable);
    let Some(callback) = callback else {
        return IteratorRecord::new(iterator, JsValue::undefined()).close(
            Err(JsNativeError::typ()
                .with_message("callback must be callable")
                .into()),
            context,
        );
    };
    let _callback_root = callback.clone().root();
    let next = iterator.get(js_string!("next"), context)?;
    let mut record = IteratorRecord::new(iterator, next);
    let mut index = 0_u64;
    while let Some(value) = record.step_value(context)? {
        let _value_root = value.as_object().map(JsObject::root);
        if index >= 9_007_199_254_740_991 {
            return record.close(
                Err(JsNativeError::typ()
                    .with_message("iterator index exceeds safe integer")
                    .into()),
                context,
            );
        }
        let result = match callback.call(
            &JsValue::undefined(),
            &[value.clone(), index.into()],
            context,
        ) {
            Ok(result) => result,
            Err(error) => return record.close(Err(error), context),
        };
        let matched = result.to_boolean();
        let completion = match mode {
            Consumer::Some if matched => Some(JsValue::from(true)),
            Consumer::Every if !matched => Some(JsValue::from(false)),
            Consumer::Find if matched => Some(value),
            _ => None,
        };
        if let Some(completion) = completion {
            return record.close(Ok(completion), context);
        }
        index += 1;
    }
    Ok(match mode {
        Consumer::Some => false.into(),
        Consumer::Every => true.into(),
        Consumer::ForEach | Consumer::Find => JsValue::undefined(),
    })
}

/// Reduces a captured iterator while rooting the accumulator across user callbacks.
pub(super) fn reduce(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let iterator = this
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("Iterator.reduce requires an object"))?;
    let _iterator_root = iterator.clone().root();
    let Some(callback) = args.first().and_then(JsValue::as_callable) else {
        return IteratorRecord::new(iterator, JsValue::undefined()).close(
            Err(JsNativeError::typ()
                .with_message("reducer must be callable")
                .into()),
            context,
        );
    };
    let _callback_root = callback.clone().root();
    let mut accumulator = args.get(1).cloned();
    let mut accumulator_root = accumulator
        .as_ref()
        .and_then(JsValue::as_object)
        .map(JsObject::root);
    let next = iterator.get(js_string!("next"), context)?;
    let mut record = IteratorRecord::new(iterator, next);
    let mut index = 0_u64;
    if accumulator.is_none() {
        accumulator = Some(record.step_value(context)?.ok_or_else(|| {
            JsNativeError::typ()
                .with_message("cannot reduce an empty iterator without an initial value")
        })?);
        accumulator_root = accumulator
            .as_ref()
            .and_then(JsValue::as_object)
            .map(JsObject::root);
        index = 1;
    }
    let mut accumulator = accumulator.expect("initial accumulator established");
    while let Some(value) = record.step_value(context)? {
        let _value_root = value.as_object().map(JsObject::root);
        if index >= 9_007_199_254_740_991 {
            return record.close(
                Err(JsNativeError::typ()
                    .with_message("iterator index exceeds safe integer")
                    .into()),
                context,
            );
        }
        accumulator = match callback.call(
            &JsValue::undefined(),
            &[accumulator.clone(), value, index.into()],
            context,
        ) {
            Ok(result) => result,
            Err(error) => return record.close(Err(error), context),
        };
        accumulator_root = accumulator.as_object().map(JsObject::root);
        index += 1;
    }
    drop(accumulator_root);
    Ok(accumulator)
}
