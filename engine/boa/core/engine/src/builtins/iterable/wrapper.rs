//! GC-traced Iterator.from wrappers holding an iterator and its captured next.
use super::create_iter_result_object;
use crate::{
    Context, JsData, JsNativeError, JsResult, JsValue,
    builtins::{BuiltInBuilder, IntrinsicObject},
    context::intrinsics::Intrinsics,
    js_string,
    object::JsObject,
    realm::Realm,
};
use boa_gc::{Finalize, Trace};

#[derive(Debug, Trace, Finalize, JsData)]
pub(super) struct IteratorWrapper {
    iterator: JsObject,
    next: JsValue,
}

impl IntrinsicObject for IteratorWrapper {
    fn init(realm: &Realm) {
        BuiltInBuilder::with_intrinsic::<Self>(realm)
            .static_method(Self::next, js_string!("next"), 0)
            .static_method(Self::return_value, js_string!("return"), 0)
            .build();
        Self::get(realm.intrinsics()).set_prototype(Some(
            realm.intrinsics().constructors().iterator().prototype(),
        ));
    }
    fn get(intrinsics: &Intrinsics) -> JsObject {
        intrinsics.objects().iterator_wrapper()
    }
}

impl IteratorWrapper {
    pub(super) fn create(iterator: JsObject, next: JsValue, context: &mut Context) -> JsObject {
        JsObject::from_proto_and_data_with_shared_shape(
            context.root_shape(),
            Self::get(context.intrinsics()),
            Self { iterator, next },
        )
    }
    fn require(this: &JsValue) -> JsResult<JsObject<Self>> {
        this.as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ()
                    .with_message("receiver must be an Iterator.from wrapper")
                    .into()
            })
    }
    fn next(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let wrapper = Self::require(this)?;
        let (iterator, next) = {
            let data = wrapper.borrow();
            (data.data().iterator.clone(), data.data().next.clone())
        };
        let method = next
            .as_callable()
            .ok_or_else(|| JsNativeError::typ().with_message("next must be callable"))?;
        method.call(&iterator.into(), &[], context)
    }
    fn return_value(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let wrapper = Self::require(this)?;
        let iterator = wrapper.borrow().data().iterator.clone();
        let _root = iterator.clone().root();
        if let Some(method) = iterator.get_method(js_string!("return"), context)? {
            method.call(&iterator.into(), &[], context)
        } else {
            Ok(create_iter_result_object(
                JsValue::undefined(),
                true,
                context,
            ))
        }
    }
}
