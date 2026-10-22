//! Preserve VM timeout diagnostics through synchronous and asynchronous jobs.
use boa_engine::job::{AsyncContext, NativeAsyncJob, NativeJob};
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue};
use std::pin::pin;
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::Instant;

fn vm_timeout() -> JsError {
    JsNativeError::runtime_limit()
        .with_message(boa_engine::vm::WALL_CLOCK_TIMEOUT_MESSAGE)
        .with_cause(JsNativeError::error().with_message("VM diagnostic"))
        .into()
}

fn assert_timeout(error: &JsError, retained: bool) {
    let native = error.as_native().expect("native safety limit");
    assert!(native.is_runtime_limit());
    assert_eq!(native.message(), boa_engine::vm::WALL_CLOCK_TIMEOUT_MESSAGE);
    assert_eq!(
        native
            .cause()
            .and_then(JsError::as_native)
            .map(JsNativeError::message),
        retained.then_some("VM diagnostic")
    );
}

fn sync_job(result: JsResult<JsValue>) -> JsError {
    let mut context = Context::default();
    NativeJob::new(move |context| {
        context
            .runtime_limits_mut()
            .set_deadline(Some(Instant::now()));
        result
    })
    .call(&mut context)
    .expect_err("a job crossing its deadline must fail")
}

fn async_job(result: JsResult<JsValue>) -> JsError {
    let mut context = Context::default();
    let storage = AsyncContext::new(&mut context);
    let job = NativeAsyncJob::new(async move |context| {
        context
            .borrow_mut()
            .runtime_limits_mut()
            .set_deadline(Some(Instant::now()));
        result
    });
    let mut future = pin!(job.call(&storage));
    let mut cx = TaskContext::from_waker(Waker::noop());
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(result) => result.expect_err("a late async job must fail"),
        Poll::Pending => panic!("the test job completes on its first poll"),
    }
}

#[test]
fn sync_job_timeout_retains_original_vm_diagnostics() {
    assert_timeout(&sync_job(Err(vm_timeout())), true);
}

#[test]
fn async_job_timeout_retains_original_vm_diagnostics() {
    assert_timeout(&async_job(Err(vm_timeout())), true);
}

#[test]
fn sync_job_timeout_overrides_late_success_and_author_error() {
    for result in [Ok(JsValue::undefined()), Err(JsNativeError::typ().into())] {
        assert_timeout(&sync_job(result), false);
    }
}

#[test]
fn async_job_timeout_overrides_late_success_and_author_error() {
    for result in [Ok(JsValue::undefined()), Err(JsNativeError::typ().into())] {
        assert_timeout(&async_job(result), false);
    }
}
