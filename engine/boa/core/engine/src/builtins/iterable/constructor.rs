//! The abstract Iterator constructor and intrinsic prototype accessors.

use super::wrapper::IteratorWrapper;
use crate::{
    Context, JsArgs, JsNativeError, JsResult, JsString, JsValue,
    builtins::{
        BuiltInBuilder, BuiltInConstructor, BuiltInObject, IntrinsicObject, OrdinaryObject,
    },
    context::intrinsics::{Intrinsics, StandardConstructor, StandardConstructors},
    js_string,
    object::{
        JsObject,
        internal_methods::{InternalMethodPropertyContext, get_prototype_from_constructor},
    },
    property::{PropertyDescriptor, PropertyKey},
    realm::Realm,
    string::StaticJsStrings,
    symbol::JsSymbol,
};

pub(crate) struct IteratorConstructor;

impl BuiltInObject for IteratorConstructor {
    const NAME: JsString = StaticJsStrings::ITERATOR;
}

impl IntrinsicObject for IteratorConstructor {
    fn init(realm: &Realm) {
        BuiltInBuilder::from_standard_constructor::<Self>(realm)
            .static_method(Self::from, js_string!("from"), 1)
            .method(super::consumers::to_array, js_string!("toArray"), 0)
            .method(super::consumers::for_each, js_string!("forEach"), 1)
            .method(super::consumers::some, js_string!("some"), 1)
            .method(super::consumers::every, js_string!("every"), 1)
            .method(super::consumers::find, js_string!("find"), 1)
            .method(super::consumers::reduce, js_string!("reduce"), 1)
            .method(super::helper::IteratorHelper::map, js_string!("map"), 1)
            .method(
                super::helper::IteratorHelper::filter,
                js_string!("filter"),
                1,
            )
            .method(super::helper::IteratorHelper::take, js_string!("take"), 1)
            .method(super::helper::IteratorHelper::drop, js_string!("drop"), 1)
            .method(
                super::helper::IteratorHelper::flat_map,
                js_string!("flatMap"),
                1,
            )
            .method(super::consumers::dispose, JsSymbol::dispose(), 0)
            .method(|this, _, _| Ok(this.clone()), JsSymbol::iterator(), 0)
            .build();
        let get_constructor = BuiltInBuilder::callable(realm, |_, _, context| {
            Ok(Self::get(context.intrinsics()).into())
        })
        .name(js_string!("get constructor"))
        .build();
        let set_constructor = BuiltInBuilder::callable(realm, |this, args, context| {
            Self::set_inherited(
                this,
                js_string!("constructor").into(),
                args.get_or_undefined(0),
                context,
            )
        })
        .name(js_string!("set constructor"))
        .length(1)
        .build();
        let get_tag = BuiltInBuilder::callable(realm, |_, _, _| Ok(Self::NAME.into()))
            .name(js_string!("get [Symbol.toStringTag]"))
            .build();
        let set_tag = BuiltInBuilder::callable(realm, |this, args, context| {
            Self::set_inherited(
                this,
                JsSymbol::to_string_tag().into(),
                args.get_or_undefined(0),
                context,
            )
        })
        .name(js_string!("set [Symbol.toStringTag]"))
        .length(1)
        .build();
        let prototype = realm.intrinsics().constructors().iterator().prototype();
        prototype.insert(
            js_string!("constructor"),
            PropertyDescriptor::builder()
                .get(get_constructor)
                .set(set_constructor)
                .enumerable(false)
                .configurable(true),
        );
        prototype.insert(
            JsSymbol::to_string_tag(),
            PropertyDescriptor::builder()
                .get(get_tag)
                .set(set_tag)
                .enumerable(false)
                .configurable(true),
        );
        IteratorWrapper::init(realm);
        super::helper::IteratorHelper::init(realm);
    }

    fn get(intrinsics: &Intrinsics) -> JsObject {
        intrinsics.constructors().iterator().constructor()
    }
}

impl BuiltInConstructor for IteratorConstructor {
    const CONSTRUCTOR_ARGUMENTS: usize = 0;
    const PROTOTYPE_STORAGE_SLOTS: usize = 13;
    const CONSTRUCTOR_STORAGE_SLOTS: usize = 1;
    const STANDARD_CONSTRUCTOR: fn(&StandardConstructors) -> &StandardConstructor =
        StandardConstructors::iterator;

    fn constructor(
        new_target: &JsValue,
        _: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        if new_target.is_undefined()
            || new_target
                .as_object()
                .is_some_and(|target| JsObject::equals(&target, &Self::get(context.intrinsics())))
        {
            return Err(JsNativeError::typ()
                .with_message("Iterator is an abstract constructor")
                .into());
        }
        let prototype =
            get_prototype_from_constructor(new_target, StandardConstructors::iterator, context)?;
        Ok(JsObject::from_proto_and_data_with_shared_shape(
            context.root_shape(),
            prototype,
            OrdinaryObject,
        )
        .into())
    }
}

impl IteratorConstructor {
    fn from(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let input = args.get_or_undefined(0);
        if !input.is_object() && !input.is_string() {
            return Err(JsNativeError::typ()
                .with_message("Iterator.from requires an object or string")
                .into());
        }
        let method = input.get_v(JsSymbol::iterator(), context)?;
        let value = if method.is_null_or_undefined() {
            input.clone()
        } else {
            method
                .as_callable()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("iterator method must be callable")
                })?
                .call(input, &[], context)?
        };
        let iterator = value
            .as_object()
            .ok_or_else(|| JsNativeError::typ().with_message("iterator must be an object"))?;
        let _root = iterator.clone().root();
        let next = iterator.get(js_string!("next"), context)?;
        let _next_root = next.as_object().map(JsObject::root);
        if JsValue::ordinary_has_instance(&Self::get(context.intrinsics()).into(), &value, context)?
        {
            return Ok(value);
        }
        Ok(IteratorWrapper::create(iterator, next, context).into())
    }

    fn set_inherited(
        this: &JsValue,
        key: PropertyKey,
        value: &JsValue,
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let object = this.as_object().ok_or_else(|| {
            JsNativeError::typ().with_message("iterator setter requires an object")
        })?;
        if JsObject::equals(
            &object,
            &context.intrinsics().constructors().iterator().prototype(),
        ) {
            return Err(JsNativeError::typ()
                .with_message("cannot assign this property on Iterator.prototype")
                .into());
        }
        let own =
            object.__get_own_property__(&key, &mut InternalMethodPropertyContext::new(context))?;
        if own.is_none() {
            object.create_data_property_or_throw(key, value.clone(), context)?;
        } else {
            object.set(key, value.clone(), true, context)?;
        }
        Ok(JsValue::undefined())
    }
}
