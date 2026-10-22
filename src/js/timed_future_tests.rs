//! Deadline checks must retain VM diagnostics without accepting late results.
use super::*;
use std::cell::Cell;
use std::task::Waker;

struct CompletesAfterDeadline {
    deadline: Instant,
    result: Option<JsResult<()>>,
}

impl Future for CompletesAfterDeadline {
    type Output = JsResult<()>;

    fn poll(mut self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<Self::Output> {
        // Deterministically model a VM slice that crosses the host deadline.
        while Instant::now() < self.deadline {
            std::hint::spin_loop();
        }
        Poll::Ready(self.result.take().unwrap())
    }
}

fn finish_late(result: JsResult<()>) -> JsError {
    let deadline = Instant::now() + Duration::from_millis(20);
    let inner = CompletesAfterDeadline {
        deadline,
        result: Some(result),
    };
    let mut future = Box::pin(TimedJsFuture::new(inner, deadline));
    let mut context = TaskContext::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(Err(error)) => error,
        other => panic!("a late VM slice must time out: {other:?}"),
    }
}

#[test]
fn timed_future_preserves_vm_timeout_diagnostics() {
    let cause = JsNativeError::error().with_message("original VM diagnostic");
    let error = JsNativeError::runtime_limit()
        .with_message(WALL_CLOCK_TIMEOUT_MESSAGE)
        .with_cause(cause);
    let error = finish_late(Err(error.into()));
    assert!(is_wall_clock_timeout(&error));
    assert_eq!(
        error
            .as_native()
            .unwrap()
            .cause()
            .and_then(JsError::as_native)
            .map(JsNativeError::message),
        Some("original VM diagnostic")
    );
}

#[test]
fn timed_future_rejects_late_success_and_author_exception() {
    for result in [
        Ok(()),
        Err(JsNativeError::typ().with_message("author").into()),
    ] {
        let error = finish_late(result);
        assert!(is_wall_clock_timeout(&error));
        assert!(error.as_native().unwrap().cause().is_none());
    }
}

#[test]
fn timed_future_does_not_poll_after_expiry() {
    let polled = Cell::new(false);
    let inner = std::future::poll_fn(|_| {
        polled.set(true);
        Poll::Ready(Ok::<_, JsError>(()))
    });
    let mut future = Box::pin(TimedJsFuture::new(inner, Instant::now()));
    let mut context = TaskContext::from_waker(Waker::noop());
    let Poll::Ready(Err(error)) = future.as_mut().poll(&mut context) else {
        panic!("an expired future must time out");
    };
    assert!(is_wall_clock_timeout(&error));
    assert!(!polled.get());
}

#[test]
fn timed_future_retains_real_interrupted_javascript_location() {
    let document = crate::dom::NodeHandle::document();
    let sandbox = SandboxConfig {
        timeout: Duration::from_millis(50),
        max_loop_iterations: u64::MAX,
    };
    let mut runtime = JsRuntime::with_document_and_sandbox(document, sandbox).unwrap();
    runtime
        .context
        .register_global_builtin_callable(
            js_string!("crossDeadline"),
            0,
            NativeFunction::from_copy_closure(|_, _, context| {
                let deadline = context.runtime_limits().deadline().unwrap();
                while Instant::now() < deadline {
                    std::hint::spin_loop();
                }
                Ok(JsValue::undefined())
            }),
        )
        .unwrap();
    let mut evaluation = Box::pin(runtime.eval_async_source(
        "function diagnosticTimeout() { crossDeadline(); } diagnosticTimeout();",
        Some("timeout-probe.js"),
        None,
        false,
        (1, 1),
        None,
    ));
    let mut context = TaskContext::from_waker(Waker::noop());
    let error = loop {
        match evaluation.as_mut().poll(&mut context) {
            Poll::Ready(result) => break result.expect_err("the VM must time out"),
            Poll::Pending => {}
        }
    };
    assert!(is_wall_clock_timeout(&error));
    let (filename, line, column) = error.source_location().expect("retain mapped VM frame");
    assert_eq!(filename.as_deref(), Some("timeout-probe.js"));
    assert_eq!(line, 1);
    assert!(column > 0);
    assert!(error.to_string().contains("diagnosticTimeout"), "{error}");
    drop(evaluation);
    assert_eq!(runtime.eval("1+1").unwrap().as_number(), Some(2.0));
}
