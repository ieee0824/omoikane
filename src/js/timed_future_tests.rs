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

fn callback_timeout_leaves_later_calls_independent(asynchronous: bool, native_entry: bool) {
    let sandbox = SandboxConfig {
        timeout: Duration::from_millis(100),
        max_loop_iterations: u64::MAX,
    };
    let mut runtime =
        JsRuntime::with_document_and_sandbox(NodeHandle::document(), sandbox).unwrap();
    runtime
        .context
        .register_global_builtin_callable(
            js_string!("bridge"),
            1,
            NativeFunction::from_copy_closure(|_, args, context| {
                let callback = args.first().and_then(JsValue::as_callable).unwrap();
                context.call_with_native_continuation(
                    &callback,
                    &JsValue::undefined(),
                    &[],
                    NativeCallContinuation::from_copy_closure_with_captures(
                        |result, _, _| result.map(|_| JsValue::from(999)),
                        (),
                    ),
                )
            }),
        )
        .unwrap();
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
    runtime.eval("function abortBody(){crossDeadline()}function aborted(){return bridge(abortBody)}function fresh(){return 7}function runPriority(p,callback){return callback()}globalThis.control=bridge(()=>1)").unwrap();
    assert_eq!(runtime.eval("control").unwrap().as_number(), Some(999.0));
    let error = runtime
        .with_active_host(|context| {
            let name = if native_entry {
                js_string!("bridge")
            } else {
                js_string!("aborted")
            };
            let callback = context
                .global_object()
                .get(name, context)?
                .as_callable()
                .unwrap();
            let args = if native_entry {
                vec![
                    context
                        .global_object()
                        .get(js_string!("abortBody"), context)?,
                ]
            } else {
                Vec::new()
            };
            let this = JsValue::undefined();
            if asynchronous {
                let mut future = Box::pin(callback.call_async(&this, &args, context));
                let mut task_context = TaskContext::from_waker(Waker::noop());
                loop {
                    if let Poll::Ready(result) = future.as_mut().poll(&mut task_context) {
                        break result;
                    }
                }
            } else {
                callback.call(&this, &args, context)
            }
        })
        .unwrap_err();
    assert!(is_wall_clock_timeout(&error), "{error}");
    assert_eq!(
        runtime.eval("runPriority(3,fresh)").unwrap().as_number(),
        Some(7.0),
        "an aborted callback must not replace an unrelated function's return value"
    );
}

#[test]
fn direct_callback_timeout_does_not_leak_native_continuations() {
    callback_timeout_leaves_later_calls_independent(false, false);
}

#[test]
fn async_callback_timeout_does_not_leak_native_continuations() {
    callback_timeout_leaves_later_calls_independent(true, false);
}

#[test]
fn direct_native_entry_timeout_does_not_leak_native_continuations() {
    callback_timeout_leaves_later_calls_independent(false, true);
}

#[test]
fn async_native_entry_timeout_does_not_leak_native_continuations() {
    callback_timeout_leaves_later_calls_independent(true, true);
}

#[derive(boa_gc::Trace, boa_gc::Finalize)]
struct ContinuationCaptureMarker {
    #[unsafe_ignore_trace]
    owners: std::sync::Arc<()>,
}

fn native_suspension_abort_releases_continuation(jit: bool) {
    let owners = std::sync::Arc::new(());
    let mut context = boa_engine::Context::default();
    #[cfg(feature = "baseline-jit")]
    context.set_baseline_jit_enabled(jit);
    #[cfg(not(feature = "baseline-jit"))]
    let _ = jit;
    context
        .register_global_builtin_callable(
            js_string!("bridge"),
            1,
            NativeFunction::from_copy_closure_with_captures(
                |_, args, captures, context| {
                    let callback = args[0].as_callable().unwrap().clone();
                    let continuation = NativeCallContinuation::from_copy_closure_with_captures(
                        |result, _, _| result,
                        ContinuationCaptureMarker {
                            owners: std::sync::Arc::clone(&captures.owners),
                        },
                    );
                    context.call_with_native_continuation(
                        &callback,
                        &JsValue::undefined(),
                        &[],
                        continuation,
                    )
                },
                ContinuationCaptureMarker {
                    owners: std::sync::Arc::clone(&owners),
                },
            ),
        )
        .unwrap();
    context
        .register_global_builtin_callable(
            js_string!("suspendNow"),
            0,
            NativeFunction::from_copy_closure(|_, _, context| {
                let suspension = context.suspend_native_call()?;
                // Expire deterministically only after creating the suspension.
                context
                    .runtime_limits_mut()
                    .set_deadline(Some(Instant::now() - Duration::from_millis(1)));
                suspension.resume(Ok(JsValue::from(1))).unwrap();
                Ok(JsValue::undefined())
            }),
        )
        .unwrap();
    context.eval(Source::from_bytes("function aborted(callback){return bridge(callback)};for(let i=0;i<80;i++)aborted(()=>7)")).unwrap();
    context.clear_kept_objects();
    boa_gc::force_collect();
    let baseline = std::sync::Arc::strong_count(&owners);
    assert_eq!(
        baseline, 2,
        "completed continuations must release their captures"
    );
    #[cfg(feature = "baseline-jit")]
    let before = context.jit_exception_diagnostics().generated_entries;
    let callback = context
        .global_object()
        .get(js_string!("aborted"), &mut context)
        .unwrap()
        .as_callable()
        .unwrap()
        .clone();
    let args = [context
        .global_object()
        .get(js_string!("suspendNow"), &mut context)
        .unwrap()];
    let error = {
        let this = JsValue::undefined();
        let mut future = Box::pin(callback.call_async(&this, &args, &mut context));
        let mut task = TaskContext::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(result) = future.as_mut().poll(&mut task) {
                break result.unwrap_err();
            }
        }
    };
    assert!(is_wall_clock_timeout(&error), "{error}");
    #[cfg(feature = "baseline-jit")]
    if jit {
        assert!(
            context.jit_exception_diagnostics().generated_entries > before,
            "the aborted call must execute a generated runtime helper"
        );
    }
    context.runtime_limits_mut().set_deadline(None);
    context.clear_kept_objects();
    boa_gc::force_collect();
    boa_gc::force_collect();
    assert_eq!(
        std::sync::Arc::strong_count(&owners),
        baseline,
        "an aborted native suspension must release its continuation capture"
    );
    assert_eq!(
        context
            .eval(Source::from_bytes("aborted(()=>23)"))
            .unwrap()
            .as_number(),
        Some(23.0)
    );
}

#[test]
fn aborted_native_suspension_releases_its_continuation_capture() {
    native_suspension_abort_releases_continuation(false);
}

#[cfg(feature = "baseline-jit")]
#[test]
fn jit_aborted_native_suspension_releases_its_continuation_capture() {
    native_suspension_abort_releases_continuation(true);
}
