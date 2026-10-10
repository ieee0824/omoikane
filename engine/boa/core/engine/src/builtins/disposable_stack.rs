//! Synchronous explicit resource management with owned disposal records.

use crate::{
    Context, JsArgs, JsData, JsError, JsNativeError, JsResult, JsString, JsValue,
    builtins::{
        BuiltInBuilder, BuiltInConstructor, BuiltInObject, IntrinsicObject,
        suppressed_error::SuppressedError,
    },
    context::intrinsics::{Intrinsics, StandardConstructor, StandardConstructors},
    js_string,
    object::{JsObject, internal_methods::get_prototype_from_constructor},
    property::Attribute,
    realm::Realm,
    string::StaticJsStrings,
    symbol::JsSymbol,
};
use boa_gc::{Finalize, Trace};

#[derive(Debug, Trace, Finalize)]
struct Resource {
    value: JsValue,
    method: JsObject,
    argument: bool,
}

#[derive(Debug, Trace, Finalize, JsData)]
pub(crate) struct DisposableStack {
    disposed: bool,
    resources: Vec<Resource>,
}

impl BuiltInObject for DisposableStack {
    const NAME: JsString = StaticJsStrings::DISPOSABLE_STACK;
}

impl IntrinsicObject for DisposableStack {
    fn init(realm: &Realm) {
        let attribute = Attribute::WRITABLE | Attribute::NON_ENUMERABLE | Attribute::CONFIGURABLE;
        let dispose = BuiltInBuilder::callable(realm, Self::dispose)
            .name(js_string!("dispose"))
            .build();
        let disposed = BuiltInBuilder::callable(realm, Self::get_disposed)
            .name(js_string!("get disposed"))
            .build();
        BuiltInBuilder::from_standard_constructor::<Self>(realm)
            .property(js_string!("dispose"), dispose.clone(), attribute)
            .property(JsSymbol::dispose(), dispose, attribute)
            .property(
                JsSymbol::to_string_tag(),
                Self::NAME,
                Attribute::CONFIGURABLE,
            )
            .accessor(
                js_string!("disposed"),
                Some(disposed),
                None,
                Attribute::CONFIGURABLE,
            )
            .method(Self::use_resource, js_string!("use"), 1)
            .method(Self::adopt, js_string!("adopt"), 2)
            .method(Self::defer, js_string!("defer"), 1)
            .method(Self::move_resources, js_string!("move"), 0)
            .build();
    }

    fn get(intrinsics: &Intrinsics) -> JsObject {
        intrinsics.constructors().disposable_stack().constructor()
    }
}

impl BuiltInConstructor for DisposableStack {
    const CONSTRUCTOR_ARGUMENTS: usize = 0;
    const PROTOTYPE_STORAGE_SLOTS: usize = 9;
    const CONSTRUCTOR_STORAGE_SLOTS: usize = 0;
    const STANDARD_CONSTRUCTOR: fn(&StandardConstructors) -> &StandardConstructor =
        StandardConstructors::disposable_stack;

    fn constructor(
        new_target: &JsValue,
        _: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        if new_target.is_undefined() {
            return Err(JsNativeError::typ()
                .with_message("DisposableStack requires new")
                .into());
        }
        let prototype = get_prototype_from_constructor(
            new_target,
            StandardConstructors::disposable_stack,
            context,
        )?;
        Ok(JsObject::from_proto_and_data_with_shared_shape(
            context.root_shape(),
            prototype,
            Self {
                disposed: false,
                resources: Vec::new(),
            },
        )
        .into())
    }
}

impl DisposableStack {
    fn require(this: &JsValue, active: bool) -> JsResult<JsObject<Self>> {
        let stack = this
            .as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ().with_message("receiver must be a DisposableStack")
            })?;
        if active && stack.borrow().data().disposed {
            return Err(JsNativeError::reference()
                .with_message("DisposableStack is disposed")
                .into());
        }
        Ok(stack)
    }

    fn get_disposed(this: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
        Ok(Self::require(this, false)?.borrow().data().disposed.into())
    }

    fn use_resource(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let stack = Self::require(this, true)?;
        let value = args.get_or_undefined(0).clone();
        if !value.is_null_or_undefined() {
            if !value.is_object() {
                return Err(JsNativeError::typ()
                    .with_message("resource must be an object")
                    .into());
            }
            let method = value
                .get_method(JsSymbol::dispose(), context)?
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("resource has no dispose method")
                })?;
            stack.borrow_mut().data_mut().resources.push(Resource {
                value: value.clone(),
                method,
                argument: false,
            });
        }
        Ok(value)
    }

    fn adopt(this: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
        let stack = Self::require(this, true)?;
        let value = args.get_or_undefined(0).clone();
        let method = args
            .get_or_undefined(1)
            .as_callable()
            .ok_or_else(|| JsNativeError::typ().with_message("onDispose must be callable"))?;
        stack.borrow_mut().data_mut().resources.push(Resource {
            value: value.clone(),
            method,
            argument: true,
        });
        Ok(value)
    }

    fn defer(this: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
        let stack = Self::require(this, true)?;
        let method = args
            .get_or_undefined(0)
            .as_callable()
            .ok_or_else(|| JsNativeError::typ().with_message("onDispose must be callable"))?;
        stack.borrow_mut().data_mut().resources.push(Resource {
            value: JsValue::undefined(),
            method,
            argument: false,
        });
        Ok(JsValue::undefined())
    }

    fn move_resources(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let stack = Self::require(this, true)?;
        let target = Self::constructor(&Self::get(context.intrinsics()).into(), &[], context)?;
        let target_stack = Self::require(&target, false)?;
        let mut source = stack.borrow_mut();
        let resources = std::mem::take(&mut source.data_mut().resources);
        source.data_mut().disposed = true;
        target_stack.borrow_mut().data_mut().resources = resources;
        Ok(target)
    }

    fn dispose(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let stack = Self::require(this, false)?;
        let _stack_root = stack.clone().root();
        if stack.borrow().data().disposed {
            return Ok(JsValue::undefined());
        }
        stack.borrow_mut().data_mut().disposed = true;
        let mut failure: Option<JsValue> = None;
        loop {
            // Unprocessed resources stay traced by the stack. Release the borrow
            // before author code, which may reenter dispose or collect garbage.
            let Some(resource) = stack.borrow_mut().data_mut().resources.pop() else {
                break;
            };
            let _method_root = resource.method.clone().root();
            let _value_root = resource.value.as_object().map(JsObject::root);
            let _failure_root = failure
                .as_ref()
                .and_then(JsValue::as_object)
                .map(JsObject::root);
            let (receiver, arguments) = if resource.argument {
                (JsValue::undefined(), vec![resource.value.clone()])
            } else {
                (resource.value.clone(), Vec::new())
            };
            if let Err(error) = resource.method.call(&receiver, &arguments, context) {
                let error = error.to_opaque(context);
                failure = Some(if let Some(previous) = failure {
                    SuppressedError::constructor(
                        &SuppressedError::get(context.intrinsics()).into(),
                        &[error, previous],
                        context,
                    )?
                } else {
                    error
                });
            }
        }
        failure.map_or_else(
            || Ok(JsValue::undefined()),
            |value| Err(JsError::from_opaque(value)),
        )
    }
}
