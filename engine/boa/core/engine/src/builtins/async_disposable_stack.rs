//! Asynchronous explicit resource management with owned disposal continuations.

use crate::object::builtins::JsPromise;
use crate::{
    Context, JsArgs, JsData, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
    builtins::{BuiltInBuilder, BuiltInConstructor, BuiltInObject, IntrinsicObject},
    context::intrinsics::{Intrinsics, StandardConstructor, StandardConstructors},
    js_string,
    object::{FunctionObjectBuilder, JsObject, internal_methods::get_prototype_from_constructor},
    property::Attribute,
    realm::Realm,
    string::StaticJsStrings,
    symbol::JsSymbol,
};
use boa_gc::{Finalize, Trace};
mod job;
use job::DisposalJob;

#[derive(Debug, Trace, Finalize)]
struct Resource {
    value: JsValue,
    method: Option<JsObject>,
    argument: bool,
}

#[derive(Debug, Trace, Finalize, JsData)]
pub(crate) struct AsyncDisposableStack {
    disposed: bool,
    resources: Vec<Resource>,
}

impl BuiltInObject for AsyncDisposableStack {
    const NAME: JsString = StaticJsStrings::ASYNC_DISPOSABLE_STACK;
}

impl IntrinsicObject for AsyncDisposableStack {
    fn init(realm: &Realm) {
        let attribute = Attribute::WRITABLE | Attribute::NON_ENUMERABLE | Attribute::CONFIGURABLE;
        let dispose = BuiltInBuilder::callable(realm, Self::dispose)
            .name(js_string!("disposeAsync"))
            .build();
        let disposed = BuiltInBuilder::callable(realm, Self::get_disposed)
            .name(js_string!("get disposed"))
            .build();
        BuiltInBuilder::from_standard_constructor::<Self>(realm)
            .property(js_string!("disposeAsync"), dispose.clone(), attribute)
            .property(JsSymbol::async_dispose(), dispose, attribute)
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
        intrinsics
            .constructors()
            .async_disposable_stack()
            .constructor()
    }
}

impl BuiltInConstructor for AsyncDisposableStack {
    const CONSTRUCTOR_ARGUMENTS: usize = 0;
    const PROTOTYPE_STORAGE_SLOTS: usize = 9;
    const CONSTRUCTOR_STORAGE_SLOTS: usize = 0;
    const STANDARD_CONSTRUCTOR: fn(&StandardConstructors) -> &StandardConstructor =
        StandardConstructors::async_disposable_stack;

    fn constructor(
        new_target: &JsValue,
        _: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        if new_target.is_undefined() {
            return Err(JsNativeError::typ()
                .with_message("AsyncDisposableStack requires new")
                .into());
        }
        let prototype = get_prototype_from_constructor(
            new_target,
            StandardConstructors::async_disposable_stack,
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

impl AsyncDisposableStack {
    fn require(this: &JsValue, active: bool) -> JsResult<JsObject<Self>> {
        let stack = this
            .as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ().with_message("receiver must be a AsyncDisposableStack")
            })?;
        if active && stack.borrow().data().disposed {
            return Err(JsNativeError::reference()
                .with_message("AsyncDisposableStack is disposed")
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
        let method = if value.is_null_or_undefined() {
            None
        } else {
            if !value.is_object() {
                return Err(JsNativeError::typ()
                    .with_message("resource must be an object")
                    .into());
            }
            if let Some(method) = value.get_method(JsSymbol::async_dispose(), context)? {
                Some(method)
            } else {
                let method = value
                    .get_method(JsSymbol::dispose(), context)?
                    .ok_or_else(|| {
                        JsNativeError::typ().with_message("resource has no dispose method")
                    })?;
                Some(Self::wrap_sync_disposer(method, context))
            }
        };
        stack.borrow_mut().data_mut().resources.push(Resource {
            value: value.clone(),
            method,
            argument: false,
        });
        Ok(value)
    }

    /// Creates the native wrapper in the use method's realm. Its return promise
    /// is observable by Await even though the sync method's value is discarded.
    fn wrap_sync_disposer(method: JsObject, context: &mut Context) -> JsObject {
        FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_copy_closure_with_captures(
                |this, _, method, context| {
                    let (promise, resolvers) = JsPromise::new_pending(context);
                    let object: JsObject = promise.clone().into();
                    let _promise_root = object.root();
                    match method.call(this, &[], context) {
                        Ok(_) => {
                            resolvers
                                .resolve
                                .call(&JsValue::undefined(), &[], context)?;
                        }
                        Err(error) => {
                            if !error.is_catchable() {
                                return Err(error);
                            }
                            let error = error.to_opaque(context);
                            resolvers
                                .reject
                                .call(&JsValue::undefined(), &[error], context)?;
                        }
                    }
                    Ok(promise.into())
                },
                method,
            ),
        )
        .build()
        .into()
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
            method: Some(method),
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
            method: Some(method),
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
        let (promise, resolvers) = JsPromise::new_pending(context);
        let promise_object: JsObject = promise.clone().into();
        let _promise_root = promise_object.root();
        let stack = match Self::require(this, false) {
            Ok(stack) => stack,
            Err(error) => {
                let error = error.to_opaque(context);
                resolvers
                    .reject
                    .call(&JsValue::undefined(), &[error], context)?;
                return Ok(promise.into());
            }
        };
        if stack.borrow().data().disposed {
            resolvers
                .resolve
                .call(&JsValue::undefined(), &[], context)?;
            return Ok(promise.into());
        }
        stack.borrow_mut().data_mut().disposed = true;
        DisposalJob::start(stack, resolvers.into_edge(), context)?;
        Ok(promise.into())
    }
}
