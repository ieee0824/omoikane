//! Owned, GC-traced state for lazy iterator helpers.
use super::{IteratorRecord, create_iter_result_object};
use crate::{
    Context, JsData, JsNativeError, JsResult, JsValue,
    builtins::{BuiltInBuilder, IntrinsicObject},
    context::intrinsics::Intrinsics,
    js_string,
    object::JsObject,
    property::Attribute,
    realm::Realm,
    symbol::JsSymbol,
};
use boa_gc::{Finalize, Trace};

mod flat_map;

#[derive(Debug, Clone, Copy)]
enum Mode {
    Map,
    Filter,
    Take,
    Drop,
    FlatMap,
}

#[derive(Debug, Trace, Finalize)]
struct IteratorEdge {
    iterator: JsObject,
    next: JsValue,
}

#[derive(Debug, Trace, Finalize, JsData)]
pub(super) struct IteratorHelper {
    iterator: JsObject,
    next: JsValue,
    callback: Option<JsObject>,
    remaining: f64,
    inner: Option<IteratorEdge>,
    #[unsafe_ignore_trace]
    mode: Mode,
    index: u64,
    executing: bool,
    done: bool,
}

impl IntrinsicObject for IteratorHelper {
    fn init(realm: &Realm) {
        BuiltInBuilder::with_intrinsic::<Self>(realm)
            .static_method(Self::next, js_string!("next"), 0)
            .static_method(Self::return_value, js_string!("return"), 0)
            .static_property(
                JsSymbol::to_string_tag(),
                js_string!("Iterator Helper"),
                Attribute::CONFIGURABLE,
            )
            .build();
        Self::get(realm.intrinsics()).set_prototype(Some(
            realm.intrinsics().constructors().iterator().prototype(),
        ));
    }
    fn get(intrinsics: &Intrinsics) -> JsObject {
        intrinsics.objects().iterator_helper()
    }
}

impl IteratorHelper {
    pub(super) fn map(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        Self::create(this, args, Mode::Map, context)
    }
    pub(super) fn filter(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        Self::create(this, args, Mode::Filter, context)
    }
    fn create(
        this: &JsValue,
        args: &[JsValue],
        mode: Mode,
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let iterator = this.as_object().ok_or_else(|| {
            JsNativeError::typ().with_message("iterator helper requires an object")
        })?;
        let _iterator_root = iterator.clone().root();
        let Some(callback) = args.first().and_then(JsValue::as_callable) else {
            return IteratorRecord::new(iterator, JsValue::undefined()).close(
                Err(JsNativeError::typ()
                    .with_message("callback must be callable")
                    .into()),
                context,
            );
        };
        let _callback_root = callback.clone().root();
        let next = iterator.get(js_string!("next"), context)?;
        let _next_root = next.as_object().map(JsObject::root);
        Ok(JsObject::from_proto_and_data_with_shared_shape(
            context.root_shape(),
            Self::get(context.intrinsics()),
            Self {
                iterator,
                next,
                callback: Some(callback),
                remaining: 0.0,
                inner: None,
                mode,
                index: 0,
                executing: false,
                done: false,
            },
        )
        .into())
    }
    pub(super) fn flat_map(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        Self::create(this, args, Mode::FlatMap, context)
    }
    pub(super) fn take(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        Self::create_count(this, args, Mode::Take, context)
    }
    pub(super) fn drop(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        Self::create_count(this, args, Mode::Drop, context)
    }
    fn create_count(
        this: &JsValue,
        args: &[JsValue],
        mode: Mode,
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let iterator = this.as_object().ok_or_else(|| {
            JsNativeError::typ().with_message("iterator helper requires an object")
        })?;
        let _root = iterator.clone().root();
        let limit = args
            .first()
            .unwrap_or(&JsValue::undefined())
            .to_number(context);
        let limit = match limit {
            Ok(limit)
                if !limit.is_nan()
                    && limit.trunc() >= 0.0
                    && (!limit.is_finite() || limit <= 9_007_199_254_740_991.0) =>
            {
                limit.trunc()
            }
            other => {
                let error = match other {
                    Err(error) => error,
                    Ok(_) => JsNativeError::range()
                        .with_message("invalid iterator limit")
                        .into(),
                };
                return IteratorRecord::new(iterator, JsValue::undefined())
                    .close(Err(error), context);
            }
        };
        let next = iterator.get(js_string!("next"), context)?;
        let _next_root = next.as_object().map(JsObject::root);
        Ok(JsObject::from_proto_and_data_with_shared_shape(
            context.root_shape(),
            Self::get(context.intrinsics()),
            Self {
                iterator,
                next,
                callback: None,
                mode,
                remaining: limit,
                inner: None,
                index: 0,
                executing: false,
                done: false,
            },
        )
        .into())
    }
    fn require(this: &JsValue) -> JsResult<JsObject<Self>> {
        this.as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ()
                    .with_message("receiver must be an Iterator Helper")
                    .into()
            })
    }
    fn begin(helper: &JsObject<Self>) -> JsResult<bool> {
        let mut object = helper.borrow_mut();
        let state = object.data_mut();
        if state.executing {
            return Err(JsNativeError::typ()
                .with_message("iterator helper is already executing")
                .into());
        }
        if state.done {
            return Ok(false);
        }
        state.executing = true;
        Ok(true)
    }
    fn next(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let helper = Self::require(this)?;
        let _root = helper.clone().root();
        if !Self::begin(&helper)? {
            return Ok(create_iter_result_object(
                JsValue::undefined(),
                true,
                context,
            ));
        }
        let result = Self::advance(&helper, context);
        {
            let mut object = helper.borrow_mut();
            let state = object.data_mut();
            state.executing = false;
            state.done = !matches!(&result, Ok(Some(_)));
        }
        let value = result?;
        let done = value.is_none();
        let value = value.unwrap_or_default();
        let _value_root = value.as_object().map(JsObject::root);
        Ok(create_iter_result_object(value, done, context))
    }
    fn advance(helper: &JsObject<Self>, context: &mut Context) -> JsResult<Option<JsValue>> {
        let (iterator, next, callback, mode, mut index) = {
            let object = helper.borrow();
            let state = object.data();
            (
                state.iterator.clone(),
                state.next.clone(),
                state.callback.clone(),
                state.mode,
                state.index,
            )
        };
        let mut record = IteratorRecord::new(iterator, next);
        if matches!(mode, Mode::FlatMap) {
            return Self::advance_flat_map(helper, &mut record, context);
        }
        if matches!(mode, Mode::Take | Mode::Drop) {
            return Self::advance_count(helper, &mut record, mode, context);
        }
        let callback = callback.expect("callback helpers retain a callback");
        while let Some(value) = record.step_value(context)? {
            let _value_root = value.as_object().map(JsObject::root);
            if index >= 9_007_199_254_740_991 {
                return record
                    .close(
                        Err(JsNativeError::typ()
                            .with_message("iterator index exceeds safe integer")
                            .into()),
                        context,
                    )
                    .map(Some);
            }
            let result = match callback.call(
                &JsValue::undefined(),
                &[value.clone(), index.into()],
                context,
            ) {
                Ok(result) => result,
                Err(error) => return record.close(Err(error), context).map(Some),
            };
            index += 1;
            helper.borrow_mut().data_mut().index = index;
            match mode {
                Mode::Map => return Ok(Some(result)),
                Mode::Filter if result.to_boolean() => return Ok(Some(value)),
                Mode::Filter => {}
                Mode::Take | Mode::Drop | Mode::FlatMap => {
                    unreachable!("count helper handled separately")
                }
            }
        }
        Ok(None)
    }
    fn advance_count(
        helper: &JsObject<Self>,
        record: &mut IteratorRecord,
        mode: Mode,
        context: &mut Context,
    ) -> JsResult<Option<JsValue>> {
        let mut remaining = helper.borrow().data().remaining;
        if matches!(mode, Mode::Take) {
            if remaining == 0.0 {
                record.close(Ok(JsValue::undefined()), context)?;
                return Ok(None);
            }
            helper.borrow_mut().data_mut().remaining = remaining - 1.0;
            return record.step_value(context);
        }
        while remaining > 0.0 {
            if record.step_value(context)?.is_none() {
                return Ok(None);
            }
            remaining -= 1.0;
            helper.borrow_mut().data_mut().remaining = remaining;
        }
        record.step_value(context)
    }
    fn return_value(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let helper = Self::require(this)?;
        let _root = helper.clone().root();
        if !Self::begin(&helper)? {
            return Ok(create_iter_result_object(
                JsValue::undefined(),
                true,
                context,
            ));
        }
        let record = {
            let object = helper.borrow();
            let state = object.data();
            IteratorRecord::new(state.iterator.clone(), state.next.clone())
        };
        let result = Self::close_records(&helper, &record, context);
        {
            let mut object = helper.borrow_mut();
            let state = object.data_mut();
            state.executing = false;
            state.done = true;
        }
        result?;
        Ok(create_iter_result_object(
            JsValue::undefined(),
            true,
            context,
        ))
    }
}
