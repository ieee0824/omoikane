//! ECMA-402 conversion order and fresh range results around short native borrows.
use super::{DateTimeFormat, range_parts::RangePart};
use crate::{
    Context, JsArgs, JsNativeError, JsResult, JsValue,
    builtins::{OrdinaryObject, array::Array},
    js_string,
};

impl DateTimeFormat {
    pub(super) fn format_range(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let parts = range_parts(this, args, context)?;
        let text: String = parts.into_iter().map(|part| part.part.value).collect();
        Ok(js_string!(text).into())
    }

    pub(super) fn format_range_to_parts(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let parts = range_parts(this, args, context)?;
        parts_array(parts, context).map(Into::into)
    }
}

fn range_parts(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<Vec<RangePart>> {
    let object = this
        .as_object()
        .and_then(|object| object.downcast::<DateTimeFormat>().ok())
        .ok_or_else(|| {
            JsNativeError::typ().with_message("receiver is not an Intl.DateTimeFormat")
        })?;
    let _receiver_root = object.clone().root();
    let start = args.get_or_undefined(0);
    let end = args.get_or_undefined(1);
    let _start_root = start.as_object().map(|object| object.root());
    let _end_root = end.as_object().map(|object| object.root());
    // Undefined is checked for both arguments before either observable coercion.
    if start.is_undefined() || end.is_undefined() {
        return Err(JsNativeError::typ()
            .with_message("range endpoints must be defined")
            .into());
    }
    let start = start.to_number(context)?;
    let end = end.to_number(context)?;
    // Both conversions precede TimeClip/NaN checks. Descending numeric endpoints
    // are accepted by the current specification and pinned Test262 contract.
    let mut data = object.borrow_mut();
    let data = data.data_mut();
    let (first, first_offset) = data.epoch_datetime(start, context)?;
    let (last, last_offset) = data.epoch_datetime(end, context)?;
    data.backend
        .format_range_parts(
            context.intl_provider().erased_provider(),
            &first,
            &last,
            data.components.widths(),
            first_offset,
            last_offset,
        )
        .map_err(|error| JsNativeError::range().with_message(error).into())
}

/// Publishes fresh ordinary objects after the receiver's native borrow ends.
fn parts_array(parts: Vec<RangePart>, context: &mut Context) -> JsResult<crate::object::JsObject> {
    let array = Array::array_create(0, None, context)?;
    let _array_root = array.clone().root();
    for (index, part) in parts.into_iter().enumerate() {
        let object = context
            .intrinsics()
            .templates()
            .ordinary_object()
            .create(OrdinaryObject, vec![]);
        let _object_root = object.clone().root();
        for (key, value) in [
            ("type", part.part.kind.as_str().to_owned()),
            ("value", part.part.value),
            ("source", part.source.as_str().to_owned()),
        ] {
            object.create_data_property_or_throw(js_string!(key), js_string!(value), context)?;
        }
        array.create_data_property_or_throw(index, object, context)?;
    }
    Ok(array)
}

#[cfg(test)]
mod tests;
