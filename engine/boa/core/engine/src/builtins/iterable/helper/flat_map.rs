//! Flat-map traversal keeps its current inner iterator in the traced helper state.
use super::{IteratorEdge, IteratorHelper, IteratorRecord};
use crate::{
    Context, JsNativeError, JsResult, JsValue, js_string, object::JsObject, symbol::JsSymbol,
};

impl IteratorHelper {
    pub(super) fn advance_flat_map(
        helper: &JsObject<Self>,
        outer: &mut IteratorRecord,
        context: &mut Context,
    ) -> JsResult<Option<JsValue>> {
        loop {
            let inner =
                {
                    let object = helper.borrow();
                    object.data().inner.as_ref().map(|inner| {
                        IteratorRecord::new(inner.iterator.clone(), inner.next.clone())
                    })
                };
            if let Some(mut inner) = inner {
                match inner.step_value(context) {
                    Ok(Some(value)) => return Ok(Some(value)),
                    Ok(None) => helper.borrow_mut().data_mut().inner = None,
                    Err(error) => return outer.close(Err(error), context).map(Some),
                }
            }
            let Some(value) = outer.step_value(context)? else {
                return Ok(None);
            };
            let _value_root = value.as_object().map(JsObject::root);
            let (callback, index) = {
                let object = helper.borrow();
                let state = object.data();
                (
                    state.callback.clone().expect("flatMap callback"),
                    state.index,
                )
            };
            if index >= 9_007_199_254_740_991 {
                return outer
                    .close(
                        Err(JsNativeError::typ()
                            .with_message("iterator index exceeds safe integer")
                            .into()),
                        context,
                    )
                    .map(Some);
            }
            let mapped = match callback.call(&JsValue::undefined(), &[value, index.into()], context)
            {
                Ok(mapped) => mapped,
                Err(error) => return outer.close(Err(error), context).map(Some),
            };
            let _mapped_root = mapped.as_object().map(JsObject::root);
            let inner = match Self::flattenable(mapped, context) {
                Ok(inner) => inner,
                Err(error) => return outer.close(Err(error), context).map(Some),
            };
            let mut object = helper.borrow_mut();
            let state = object.data_mut();
            state.inner = Some(inner);
            state.index += 1;
        }
    }

    fn flattenable(mapped: JsValue, context: &mut Context) -> JsResult<IteratorEdge> {
        let object = mapped.as_object().ok_or_else(|| {
            JsNativeError::typ().with_message("flatMap callback must return an object")
        })?;
        let iterator = if let Some(method) = object.get_method(JsSymbol::iterator(), context)? {
            method
                .call(&mapped, &[], context)?
                .as_object()
                .ok_or_else(|| JsNativeError::typ().with_message("iterator must be an object"))?
        } else {
            object
        };
        let _root = iterator.clone().root();
        let next = iterator.get(js_string!("next"), context)?;
        Ok(IteratorEdge { iterator, next })
    }

    pub(super) fn close_records(
        helper: &JsObject<Self>,
        outer: &IteratorRecord,
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let inner = {
            let object = helper.borrow();
            object
                .data()
                .inner
                .as_ref()
                .map(|inner| IteratorRecord::new(inner.iterator.clone(), inner.next.clone()))
        };
        let completion = match inner {
            Some(inner) => inner.close(Ok(JsValue::undefined()), context),
            None => Ok(JsValue::undefined()),
        };
        outer.close(completion, context)
    }
}
