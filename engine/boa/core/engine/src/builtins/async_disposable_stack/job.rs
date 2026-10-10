//! Owned continuation state for sequential asynchronous disposal.

use super::{AsyncDisposableStack, Resource};
use crate::{
    Context, JsArgs, JsData, JsResult, JsValue, NativeFunction,
    builtins::{
        BuiltInConstructor, IntrinsicObject,
        promise::{Promise, ResolvingFunctionsEdge},
        suppressed_error::SuppressedError,
    },
    object::{FunctionObjectBuilder, JsObject},
};
use boa_gc::{Finalize, Trace};

#[derive(Debug, Trace, Finalize, JsData)]
pub(super) struct DisposalJob {
    stack: JsObject<AsyncDisposableStack>,
    resolvers: ResolvingFunctionsEdge,
    failure: Option<JsValue>,
    needs_await: bool,
    has_awaited: bool,
}

impl DisposalJob {
    pub(super) fn start(
        stack: JsObject<AsyncDisposableStack>,
        resolvers: ResolvingFunctionsEdge,
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let _stack_root = stack.clone().root();
        let _resolve_root = resolvers.resolve.root();
        let _reject_root = resolvers.reject.root();
        let job = JsObject::from_proto_and_data_with_shared_shape(
            context.root_shape(),
            None,
            Self {
                stack,
                resolvers,
                failure: None,
                needs_await: false,
                has_awaited: false,
            },
        );
        let job = job
            .downcast::<Self>()
            .expect("new disposal job has its native data");
        Self::run(&job, context)
    }

    fn run(job: &JsObject<Self>, context: &mut Context) -> JsResult<JsValue> {
        let _job_root = job.clone().root();
        let stack = job.borrow().data().stack.clone();
        loop {
            let Some(resource) = stack.borrow_mut().data_mut().resources.pop() else {
                break;
            };
            if resource.method.is_none() {
                job.borrow_mut().data_mut().needs_await = true;
                continue;
            }
            let result = Self::call_resource(&resource, context);
            match result.and_then(|value| Self::await_value(job, value, context)) {
                Ok(()) => return Ok(JsValue::undefined()),
                Err(error) => {
                    if !error.is_catchable() {
                        return Err(error);
                    }
                    let error = error.to_opaque(context);
                    Self::record_failure(job, error, context)?;
                }
            }
        }
        let must_await = {
            let state = job.borrow();
            state.data().needs_await && !state.data().has_awaited
        };
        if must_await {
            Self::await_value(job, JsValue::undefined(), context)?;
        } else {
            let (resolver, value) = {
                let state = job.borrow();
                if let Some(value) = &state.data().failure {
                    (state.data().resolvers.reject.root(), value.clone())
                } else {
                    (state.data().resolvers.resolve.root(), JsValue::undefined())
                }
            };
            resolver.call(&JsValue::undefined(), &[value], context)?;
        }
        Ok(JsValue::undefined())
    }

    fn call_resource(resource: &Resource, context: &mut Context) -> JsResult<JsValue> {
        let method = resource.method.as_ref().expect("checked by disposal loop");
        let _method_root = method.clone().root();
        let _value_root = resource.value.as_object().map(JsObject::root);
        if resource.argument {
            method.call(
                &JsValue::undefined(),
                std::slice::from_ref(&resource.value),
                context,
            )
        } else {
            method.call(&resource.value, &[], context)
        }
    }

    fn await_value(job: &JsObject<Self>, value: JsValue, context: &mut Context) -> JsResult<()> {
        let constructor = context.intrinsics().constructors().promise().constructor();
        let promise = Promise::promise_resolve(&constructor, value, context)?;
        let _promise_root = promise.clone().root();
        let fulfilled = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_copy_closure_with_captures(
                |_, _, job, context| Self::run(job, context),
                job.clone(),
            ),
        )
        .build();
        let rejected = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_copy_closure_with_captures(
                |_, args, job, context| {
                    Self::record_failure(job, args.get_or_undefined(0).clone(), context)?;
                    Self::run(job, context)
                },
                job.clone(),
            ),
        )
        .build();
        // Await uses internal reactions: do not read author .then or @@species.
        Promise::perform_promise_then(&promise, Some(fulfilled), Some(rejected), None, context);
        job.borrow_mut().data_mut().has_awaited = true;
        Ok(())
    }

    fn record_failure(job: &JsObject<Self>, error: JsValue, context: &mut Context) -> JsResult<()> {
        let _job_root = job.clone().root();
        let _error_root = error.as_object().map(JsObject::root);
        let previous = job.borrow().data().failure.clone();
        let error = if let Some(previous) = previous {
            SuppressedError::constructor(
                &SuppressedError::get(context.intrinsics()).into(),
                &[error, previous],
                context,
            )?
        } else {
            error
        };
        job.borrow_mut().data_mut().failure = Some(error);
        Ok(())
    }
}
