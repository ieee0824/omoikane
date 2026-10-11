//! Normative optional constructor chaining using Intl's shared fallback symbol.
use super::DateTimeFormat;
use crate::{Context, JsNativeError, JsObject, JsResult, JsValue, property::PropertyDescriptor};

/// Runs observable prototype checks and property definition without borrowing
/// formatter data. The caller roots the receiver and fresh formatter.
pub(super) fn chain(
    formatter: JsObject,
    new_target: &JsValue,
    this: JsValue,
    context: &mut Context,
) -> JsResult<JsValue> {
    if new_target.is_undefined()
        && JsValue::ordinary_has_instance(
            &context
                .intrinsics()
                .constructors()
                .date_time_format()
                .constructor()
                .into(),
            &this,
            context,
        )?
    {
        let symbol = context
            .intrinsics()
            .objects()
            .intl()
            .borrow()
            .data()
            .fallback_symbol();
        this.as_object()
            .expect("instance is an object")
            .define_property_or_throw(
                symbol,
                PropertyDescriptor::builder()
                    .value(formatter)
                    .writable(false)
                    .enumerable(false)
                    .configurable(false),
                context,
            )?;
        return Ok(this);
    }
    Ok(formatter.into())
}

/// Unwraps the fallback only for format and resolvedOptions. Property access
/// may execute author code; no native formatter borrow spans that access.
pub(super) fn unwrap(this: &JsValue, context: &mut Context) -> JsResult<JsObject<DateTimeFormat>> {
    let object = this.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("receiver is not an Intl.DateTimeFormat")
    })?;
    let _receiver_root = object.clone().root();
    if let Ok(formatter) = object.clone().downcast::<DateTimeFormat>() {
        return Ok(formatter);
    }
    if JsValue::ordinary_has_instance(
        &context
            .intrinsics()
            .constructors()
            .date_time_format()
            .constructor()
            .into(),
        this,
        context,
    )? {
        let symbol = context
            .intrinsics()
            .objects()
            .intl()
            .borrow()
            .data()
            .fallback_symbol();
        if let Some(formatter) = object
            .get(symbol, context)?
            .as_object()
            .and_then(|object| object.downcast::<DateTimeFormat>().ok())
        {
            return Ok(formatter);
        }
    }
    Err(JsNativeError::typ()
        .with_message("receiver is not an Intl.DateTimeFormat")
        .into())
}
